use std::path::Path;

use crate::consts::BTRFS_TOPLEVEL_SUBVOLID;
use crate::{check, swap, tools};

/// Roll back to a named snapshot.
/// 1. Mount top-level subvolume
/// 2. Verify snapshot is bootable (P2-P5, skip ESP)
/// 3. RENAME_EXCHANGE root <-> snapshot
/// 4. Update default subvolume to match new root
///
/// Verification BEFORE the irreversible swap. By construction, not by discipline.
/// Cannot use with_toplevel because undo-on-failure needs direct control.
pub fn rollback(snapshot_name: &str) -> Result<(), String> {
    let toplevel = "/mnt/atomic-rollback-toplevel";
    let (device, fstab) = tools::root_device()?;
    let root_subvol = tools::root_subvol_name(&fstab)?;

    std::fs::create_dir_all(toplevel)
        .map_err(|e| format!("mkdir {toplevel}: {e}"))?;
    tools::mount_subvolid(&device, toplevel, BTRFS_TOPLEVEL_SUBVOLID)?;

    // Verify snapshot exists
    let snapshot_path = format!("{toplevel}/{snapshot_name}");
    if !Path::new(&snapshot_path).exists() {
        tools::umount(toplevel)?;
        return Err(format!("snapshot '{snapshot_name}' not found at top-level"));
    }

    // Verify snapshot contents BEFORE the irreversible swap.
    // P1 (ESP) skipped: vfat, external to Btrfs, files don't change.
    // Default subvol match skipped: set-default comes after swap.
    println!("\n  Verifying snapshot '{snapshot_name}' is bootable...");
    match check::verify_snapshot_bootable(Path::new(&snapshot_path)) {
        check::BootStatus::Pass | check::BootStatus::Warn => {
            println!("  Snapshot verified.\n");
        }
        check::BootStatus::Fail(failures) => {
            println!("\n  Snapshot verification FAILED:");
            for f in &failures {
                eprintln!("    {f}");
            }
            tools::umount(toplevel)?;
            let _ = std::fs::remove_dir(toplevel);
            return Err("rollback aborted: snapshot is not bootable".into());
        }
    }

    // RENAME_EXCHANGE: root_subvol <-> snapshot
    println!("  RENAME_EXCHANGE: {root_subvol} <-> {snapshot_name}");
    swap::atomic_swap(Path::new(toplevel), &root_subvol, snapshot_name)?;

    // Update default subvolume to match the new root.
    // If this fails, UNDO the swap so the system is unchanged.
    // The model treats rollback as atomic: both happen or neither.
    let new_root_id = match tools::btrfs_subvol_id_by_name(toplevel, &root_subvol) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("  set-default failed; undoing swap to restore original state");
            let _ = swap::atomic_swap(Path::new(toplevel), &root_subvol, snapshot_name);
            tools::umount(toplevel)?;
            let _ = std::fs::remove_dir(toplevel);
            return Err(format!("rollback aborted: cannot determine new root ID: {e}"));
        }
    };
    println!("  set-default: ID {new_root_id}");
    if let Err(e) = tools::btrfs_subvol_set_default(new_root_id, toplevel) {
        eprintln!("  set-default failed; undoing swap to restore original state");
        let _ = swap::atomic_swap(Path::new(toplevel), &root_subvol, snapshot_name);
        tools::umount(toplevel)?;
        let _ = std::fs::remove_dir(toplevel);
        return Err(format!("rollback aborted: set-default failed: {e}"));
    }

    // Flush to disk before telling the user to reboot.
    tools::sync_filesystem(toplevel)?;

    // Rollback succeeded. Cleanup is best-effort; a stale mount
    // doesn't affect the boot chain or the rollback.
    let _ = tools::umount(toplevel);
    let _ = std::fs::remove_dir(toplevel);

    Ok(())
}

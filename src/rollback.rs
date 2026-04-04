//! Rollback: verify snapshot is bootable, swap root subvolume names
//! via RENAME_EXCHANGE, update default subvolume. Undoes the swap
//! if set-default fails.

use std::path::Path;

use crate::consts::{BTRFS_TOPLEVEL_SUBVOLID, TOPLEVEL_MOUNT};
use crate::{check, swap, tools};

/// Cannot use with_toplevel because the undo path needs to swap back
/// if set-default fails, which requires the mount to stay alive.
pub fn rollback(snapshot_name: &str) -> Result<(), String> {
    let toplevel = TOPLEVEL_MOUNT;
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
    swap::rename_exchange(Path::new(toplevel), root_subvol.as_str(), snapshot_name)?;

    // Update default subvolume to match the new root.
    // If this fails, UNDO the swap so the system is unchanged.
    // The model treats rollback as atomic: both happen or neither.
    let new_root_id = match tools::btrfs_subvol_id_by_name(toplevel, &root_subvol) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("  set-default failed; undoing swap to restore original state");
            if let Err(undo_err) = swap::rename_exchange(Path::new(toplevel), root_subvol.as_str(), snapshot_name) {
                eprintln!("  CRITICAL: undo swap also failed: {undo_err}");
                eprintln!("  System is in an inconsistent state. Manual recovery required.");
            }
            tools::umount(toplevel)?;
            let _ = std::fs::remove_dir(toplevel);
            return Err(format!("rollback aborted: cannot determine new root ID: {e}"));
        }
    };
    println!("  set-default: ID {new_root_id}");
    if let Err(e) = tools::btrfs_subvol_set_default(new_root_id, toplevel) {
        eprintln!("  set-default failed; undoing swap to restore original state");
        if let Err(undo_err) = swap::rename_exchange(Path::new(toplevel), root_subvol.as_str(), snapshot_name) {
            eprintln!("  CRITICAL: undo swap also failed: {undo_err}");
            eprintln!("  System is in an inconsistent state. Manual recovery required.");
        }
        tools::umount(toplevel)?;
        let _ = std::fs::remove_dir(toplevel);
        return Err(format!("rollback aborted: set-default failed: {e}"));
    }

    // Swap and set-default are in the btrfs in-memory journal only.
    tools::sync_filesystem(toplevel)?;

    // Rollback succeeded. Cleanup is best-effort; a stale mount
    // doesn't affect the boot chain or the rollback.
    let _ = tools::umount(toplevel);
    let _ = std::fs::remove_dir(toplevel);

    Ok(())
}

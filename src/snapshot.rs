use std::fs;
use std::path::Path;

use crate::{parse, tools};

const TOPLEVEL: &str = "/mnt/atomic-rollback-toplevel";

/// Create a snapshot of the root subvolume for later rollback.
///
/// Postcondition (success): snapshot exists at top-level, top-level unmounted.
/// Postcondition (failure): no snapshot created, top-level unmounted.
pub fn snapshot(name: Option<&str>) -> Result<String, String> {
    let name = name.unwrap_or("root.pre-update");

    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let root_device = fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(1).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().next())
        .ok_or("Cannot find root entry in /etc/fstab")?
        .to_string();

    let device = tools::resolve_fstab_device(&root_device)?;

    fs::create_dir_all(TOPLEVEL).map_err(|e| format!("mkdir {TOPLEVEL}: {e}"))?;
    tools::mount_subvolid(&device, TOPLEVEL, 5)?;

    // From here, every exit path must unmount. Use a helper that guarantees it.
    let result = create_snapshot(name, &fstab);

    // Postcondition: top-level unmounted regardless of success or failure.
    let _ = tools::umount(TOPLEVEL);
    let _ = fs::remove_dir(TOPLEVEL);

    result
}

/// Inner function. The caller guarantees unmount.
fn create_snapshot(name: &str, fstab: &str) -> Result<String, String> {
    let snap_path = format!("{TOPLEVEL}/{name}");
    if Path::new(&snap_path).exists() {
        // Snapshot already exists — the user is protected. Not an error.
        // This makes the command idempotent: safe for dnf pre_transaction hooks
        // that may fire multiple times.
        eprintln!("Snapshot '{name}' already exists; using existing protection.");
        return Ok(name.to_string());
    }

    let root_subvol = fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(1).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|opts| parse::extract_mount_option(opts, "subvol"))
        .ok_or("cannot determine root subvolume name from fstab")?;

    tools::btrfs_subvol_snapshot(&format!("{TOPLEVEL}/{root_subvol}"), &snap_path)?;
    Ok(name.to_string())
}

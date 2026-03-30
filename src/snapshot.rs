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
        // Snapshot already exists; the user is protected. Not an error.
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

/// List all snapshots at the top level.
///
/// Postcondition: top-level unmounted. Read-only, no state change.
pub fn list() -> Result<Vec<String>, String> {
    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let root_device = fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(1).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().next())
        .ok_or("Cannot find root entry in /etc/fstab")?
        .to_string();

    let device = tools::resolve_fstab_device(&root_device)?;
    let protected = fstab_subvol_names(&fstab);

    fs::create_dir_all(TOPLEVEL).map_err(|e| format!("mkdir {TOPLEVEL}: {e}"))?;
    tools::mount_subvolid(&device, TOPLEVEL, 5)?;

    let result = list_snapshots(&protected);

    let _ = tools::umount(TOPLEVEL);
    let _ = fs::remove_dir(TOPLEVEL);

    result
}

fn list_snapshots(protected: &[String]) -> Result<Vec<String>, String> {
    let mut snapshots = Vec::new();
    let entries = fs::read_dir(TOPLEVEL)
        .map_err(|e| format!("read {TOPLEVEL}: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if protected.contains(&name) {
            continue; // system subvolume, not a snapshot
        }
        snapshots.push(name);
    }
    snapshots.sort();
    Ok(snapshots)
}

/// Delete a snapshot by name.
///
/// Guards:
/// 1. Refuses subvolumes referenced by fstab (system subvolumes).
/// 2. Refuses currently mounted subvolumes (checked via findmnt).
/// 3. Kernel refuses mounted subvolumes (defense in depth).
///
/// Postcondition (success): snapshot deleted, top-level unmounted.
/// Postcondition (failure): snapshot NOT deleted, top-level unmounted.
pub fn delete(name: &str) -> Result<(), String> {
    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let root_device = fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(1).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().next())
        .ok_or("Cannot find root entry in /etc/fstab")?
        .to_string();

    // Guard 1: refuse fstab-referenced subvolumes
    let protected = fstab_subvol_names(&fstab);
    if protected.contains(&name.to_string()) {
        return Err(format!(
            "Cannot delete '{name}': referenced by /etc/fstab as a system subvolume. \
             Deleting it would break the system."));
    }

    // Guard 2: refuse mounted subvolumes
    let mount_check = tools::run_stdout("findmnt", &["-n", "-o", "TARGET", &format!("/{name}")]);
    if let Ok(target) = &mount_check {
        if !target.is_empty() {
            return Err(format!(
                "Cannot delete '{name}': currently mounted at {target}. \
                 Unmount it first or use rollback to switch away from it."));
        }
    }

    let device = tools::resolve_fstab_device(&root_device)?;

    fs::create_dir_all(TOPLEVEL).map_err(|e| format!("mkdir {TOPLEVEL}: {e}"))?;
    tools::mount_subvolid(&device, TOPLEVEL, 5)?;

    let result = delete_snapshot(name);

    let _ = tools::umount(TOPLEVEL);
    let _ = fs::remove_dir(TOPLEVEL);

    result
}

fn delete_snapshot(name: &str) -> Result<(), String> {
    let snap_path = format!("{TOPLEVEL}/{name}");
    if !Path::new(&snap_path).exists() {
        return Err(format!("Snapshot '{name}' not found."));
    }
    tools::run_stdout("btrfs", &["subvolume", "delete", &snap_path]).map(|_| ())
}

/// Extract all subvol= names from fstab. These are system subvolumes
/// that must never be deleted.
fn fstab_subvol_names(fstab: &str) -> Vec<String> {
    fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .filter_map(|l| l.split_whitespace().nth(3))
        .filter_map(|opts| parse::extract_mount_option(opts, "subvol"))
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fstab_extracts_protected_names() {
        let fstab = "\
UUID=abc / btrfs subvol=root,compress=zstd:1 0 0
UUID=abc /home btrfs subvol=home,compress=zstd:1 0 0
UUID=abc /var btrfs subvol=var,compress=zstd:1 0 0";
        let names = fstab_subvol_names(fstab);
        assert_eq!(names, vec!["root", "home", "var"]);
    }

    #[test]
    fn fstab_skips_comments() {
        let fstab = "\
# UUID=abc / btrfs subvol=old-root 0 0
UUID=abc / btrfs subvol=root 0 0
  # indented comment subvol=fake
UUID=abc /home btrfs subvol=home 0 0";
        let names = fstab_subvol_names(fstab);
        assert_eq!(names, vec!["root", "home"]);
    }

    #[test]
    fn fstab_handles_non_btrfs_entries() {
        let fstab = "\
UUID=abc / btrfs subvol=root 0 0
UUID=def /boot ext4 defaults 0 0
UUID=ghi /boot/efi vfat umask=0077 0 0
UUID=abc /home btrfs subvol=home 0 0";
        let names = fstab_subvol_names(fstab);
        assert_eq!(names, vec!["root", "home"]);
    }

    #[test]
    fn guard_refuses_protected_names() {
        let protected = fstab_subvol_names(
            "UUID=abc / btrfs subvol=root 0 0\nUUID=abc /home btrfs subvol=home 0 0");
        assert!(protected.contains(&"root".to_string()));
        assert!(protected.contains(&"home".to_string()));
        assert!(!protected.contains(&"my-snapshot".to_string()));
    }
}

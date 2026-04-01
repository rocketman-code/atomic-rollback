//! Snapshot lifecycle: create, list, delete. The dnf plugin calls
//! create before every transaction. List and delete delegate to
//! btrfs-progs; delete adds an fstab guard that btrfs-progs lacks.

use std::fs;
use std::path::Path;

use crate::consts::{DEFAULT_SNAPSHOT_NAME, FSTAB_OPTIONS};
use crate::{parse, tools};

/// Creates a snapshot of the root subvolume at the btrfs top level.
/// Idempotent: returns Ok if the snapshot already exists.
pub fn snapshot(name: Option<&str>) -> Result<String, String> {
    let name = name.unwrap_or(DEFAULT_SNAPSHOT_NAME);
    let (_, fstab) = tools::root_device()?;
    let root_subvol = tools::root_subvol_name(&fstab)?;

    tools::with_toplevel(|toplevel| {
        let snap_path = format!("{toplevel}/{name}");
        if Path::new(&snap_path).exists() {
            eprintln!("Snapshot '{name}' already exists; using existing protection.");
            return Ok(name.to_string());
        }
        tools::btrfs_subvol_snapshot(&format!("{toplevel}/{root_subvol}"), &snap_path)?;
        Ok(name.to_string())
    })
}

/// Returns top-level subvolume names, excluding fstab system subvolumes.
pub fn list() -> Result<Vec<String>, String> {
    let protected = fstab_subvol_names()?;
    let output = tools::btrfs_subvol_list("/")?;
    let mut snapshots = Vec::new();
    for line in output.lines() {
        if let Some(name) = line.split_whitespace().last() {
            if !protected.contains(&name.to_string()) {
                snapshots.push(name.to_string());
            }
        }
    }
    snapshots.sort();
    Ok(snapshots)
}

/// Refuses subvolumes referenced by fstab (system subvolumes).
/// Mounted and default subvolume protection from kernel and btrfs-progs.
pub fn delete(name: &str) -> Result<(), String> {
    let protected = fstab_subvol_names()?;
    if protected.contains(&name.to_string()) {
        return Err(format!(
            "Cannot delete '{name}': referenced by /etc/fstab as a system subvolume. \
             Deleting it would break the system."));
    }

    let id = tools::btrfs_subvol_id_by_name("/", name)?;
    tools::run_stdout("btrfs", &["subvolume", "delete", "--subvolid", &id.to_string(), "/"])
        .map(|_| ())
}

// System subvolumes from fstab. These must never be deleted.
fn fstab_subvol_names() -> Result<Vec<String>, String> {
    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    Ok(fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .filter_map(|l| l.split_whitespace().nth(FSTAB_OPTIONS))
        .filter_map(|opts| parse::extract_mount_option(opts, "subvol"))
        .map(|s| s.to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::consts::FSTAB_OPTIONS;
    use crate::parse;

    fn fstab_subvol_names_from(fstab: &str) -> Vec<String> {
        fstab.lines()
            .filter(|l| !l.trim().starts_with('#'))
            .filter_map(|l| l.split_whitespace().nth(FSTAB_OPTIONS))
            .filter_map(|opts| parse::extract_mount_option(opts, "subvol"))
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn fstab_extracts_protected_names() {
        let fstab = "\
UUID=abc / btrfs subvol=root,compress=zstd:1 0 0
UUID=abc /home btrfs subvol=home,compress=zstd:1 0 0
UUID=abc /var btrfs subvol=var,compress=zstd:1 0 0";
        let names = fstab_subvol_names_from(fstab);
        assert_eq!(names, vec!["root", "home", "var"]);
    }

    #[test]
    fn fstab_skips_comments() {
        let fstab = "\
# UUID=abc / btrfs subvol=old-root 0 0
UUID=abc / btrfs subvol=root 0 0
  # indented comment subvol=fake
UUID=abc /home btrfs subvol=home 0 0";
        let names = fstab_subvol_names_from(fstab);
        assert_eq!(names, vec!["root", "home"]);
    }

    #[test]
    fn fstab_handles_non_btrfs_entries() {
        let fstab = "\
UUID=abc / btrfs subvol=root 0 0
UUID=def /boot ext4 defaults 0 0
UUID=ghi /boot/efi vfat umask=0077 0 0
UUID=abc /home btrfs subvol=home 0 0";
        let names = fstab_subvol_names_from(fstab);
        assert_eq!(names, vec!["root", "home"]);
    }

    #[test]
    fn guard_refuses_protected_names() {
        let protected = fstab_subvol_names_from(
            "UUID=abc / btrfs subvol=root 0 0\nUUID=abc /home btrfs subvol=home 0 0");
        assert!(protected.contains(&"root".to_string()));
        assert!(protected.contains(&"home".to_string()));
        assert!(!protected.contains(&"my-snapshot".to_string()));
    }
}

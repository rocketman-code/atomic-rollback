//! Snapshot lifecycle: create, list, delete. The dnf plugin calls
//! create before every transaction. List and delete delegate to
//! btrfs-progs; delete adds an fstab guard that btrfs-progs lacks.

use std::fs;
use std::path::Path;

use crate::consts::DEFAULT_SNAPSHOT_NAME;
use crate::{parse, tools};

/// Result of a snapshot operation.
pub enum SnapshotResult {
    Created(String),
    Existed(String),
    NotBtrfs,
}

/// Creates a snapshot of the root subvolume at the btrfs top level.
/// Idempotent: returns Existed if the snapshot already exists.
/// Returns NotBtrfs if the root filesystem is not btrfs (nothing to protect).
pub fn snapshot(name: Option<&str>) -> Result<SnapshotResult, String> {
    let name = name.unwrap_or(DEFAULT_SNAPSHOT_NAME);
    let (_, fstab) = tools::root_device()?;
    let root_subvol = match tools::root_subvol_name(&fstab) {
        Ok(s) => s,
        Err(_) => return Ok(SnapshotResult::NotBtrfs),
    };

    tools::with_toplevel(|toplevel| {
        let snap_path = format!("{toplevel}/{name}");
        if Path::new(&snap_path).exists() {
            return Ok(SnapshotResult::Existed(name.to_string()));
        }
        tools::btrfs_subvol_snapshot(&format!("{toplevel}/{}", root_subvol.as_str()), &snap_path)?;
        Ok(SnapshotResult::Created(name.to_string()))
    })
}

/// Returns top-level subvolume names, excluding fstab system subvolumes.
pub fn list() -> Result<Vec<String>, String> {
    let protected = fstab_subvol_names()?;
    let entries = tools::btrfs_subvol_list("/")?;
    let mut snapshots: Vec<String> = entries.iter()
        .filter(|e| !protected.iter().any(|p| p.as_str() == e.path))
        .map(|e| e.path.clone())
        .collect();
    snapshots.sort();
    Ok(snapshots)
}

/// Refuses subvolumes referenced by fstab (system subvolumes).
/// Mounted and default subvolume protection from kernel and btrfs-progs.
pub fn delete(name: &str) -> Result<(), String> {
    let protected = fstab_subvol_names()?;
    if protected.contains(&tools::SubvolName::new(name.to_string())) {
        return Err(format!(
            "Cannot delete '{name}': referenced by /etc/fstab as a system subvolume. \
             Deleting it would break the system."));
    }

    let id = tools::btrfs_subvol_id_by_name("/", &tools::SubvolName::new(name.to_string()))?;
    tools::run_stdout("btrfs", &["subvolume", "delete", "--subvolid", &id.to_string(), "/"])
        .map(|_| ())
}

// System subvolumes from fstab. These must never be deleted.
fn fstab_subvol_names() -> Result<Vec<tools::SubvolName>, String> {
    let content = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let lines = tools::parse_fstab(&content);
    Ok(tools::fstab_entries(&lines).into_iter()
        .filter_map(|e| parse::extract_mount_option(&e.fs_mntops, "subvol"))
        .map(|s| tools::SubvolName::new(s.to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::{parse, tools};

    fn fstab_subvol_names_from(fstab: &str) -> Vec<tools::SubvolName> {
        let lines = tools::parse_fstab(fstab);
        tools::fstab_entries(&lines).into_iter()
            .filter_map(|e| parse::extract_mount_option(&e.fs_mntops, "subvol"))
            .map(|s| tools::SubvolName::new(s.to_string()))
            .collect()
    }

    #[test]
    fn fstab_extracts_protected_names() {
        let fstab = "\
UUID=abc / btrfs subvol=root,compress=zstd:1 0 0
UUID=abc /home btrfs subvol=home,compress=zstd:1 0 0
UUID=abc /var btrfs subvol=var,compress=zstd:1 0 0";
        let names = fstab_subvol_names_from(fstab);
        let names: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
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
        let names: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
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
        let names: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        assert_eq!(names, vec!["root", "home"]);
    }

    #[test]
    fn guard_refuses_protected_names() {
        let protected = fstab_subvol_names_from(
            "UUID=abc / btrfs subvol=root 0 0\nUUID=abc /home btrfs subvol=home 0 0");
        assert!(protected.contains(&tools::SubvolName::new("root".into())));
        assert!(protected.contains(&tools::SubvolName::new("home".into())));
        assert!(!protected.contains(&tools::SubvolName::new("my-snapshot".into())));
    }
}

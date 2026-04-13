//! Snapshot lifecycle: create, list, delete, retain. The RPM plugin
//! calls create before every transaction. Retention evicts the oldest
//! automatic snapshots when the count exceeds MAX_SNAPSHOTS. List and
//! delete delegate to btrfs-progs; delete adds an fstab guard that
//! btrfs-progs lacks.

use std::fs;
use std::path::Path;

use crate::{consts, parse, tools};

/// Returns true if a name matches the auto-generated snapshot format (YYYY-MM-DD_HH-MM-SS).
fn is_auto_name(name: &str) -> bool {
    if name.len() != 19 { return false; }
    let b = name.as_bytes();
    b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-' && b[7] == b'-' && b[10] == b'_'
        && b[13] == b'-' && b[16] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[17..19].iter().all(u8::is_ascii_digit)
}

/// Parses MAX_SNAPSHOTS from config file content.
fn parse_max_snapshots(content: &str) -> usize {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("MAX_SNAPSHOTS=") {
            let value = value.trim().trim_matches('"');
            if let Ok(n) = value.parse::<usize>() {
                return n;
            }
        }
    }
    consts::MAX_SNAPSHOTS
}

/// Reads MAX_SNAPSHOTS from the config file, falling back to the compiled-in default.
fn max_snapshots() -> usize {
    match fs::read_to_string(consts::CONFIG_PATH) {
        Ok(content) => parse_max_snapshots(&content),
        Err(_) => consts::MAX_SNAPSHOTS,
    }
}

/// Evicts the oldest auto-named snapshots if count exceeds MAX_SNAPSHOTS.
/// User-named snapshots are never touched. Failures are logged, not fatal.
fn retain_auto_snapshots() {
    let max = max_snapshots();
    if max == 0 { return; }

    let protected = match fstab_subvol_names() {
        Ok(p) => p,
        Err(e) => { eprintln!("Retention warning: {e}"); return; }
    };
    let entries = match tools::btrfs_subvol_list("/") {
        Ok(e) => e,
        Err(e) => { eprintln!("Retention warning: {e}"); return; }
    };

    let mut auto_entries: Vec<&tools::SubvolEntry> = entries.iter()
        .filter(|e| is_auto_name(&e.path))
        .filter(|e| !protected.iter().any(|p| p.as_str() == e.path))
        .collect();

    if auto_entries.len() <= max {
        return;
    }

    auto_entries.sort_by_key(|e| e.id);
    let to_evict = auto_entries.len() - max;

    eprintln!("Keeping {max} automatic snapshots, removing oldest.");
    for entry in auto_entries.iter().take(to_evict) {
        match delete(&entry.path) {
            Ok(..) => eprintln!("Snapshot '{}' with ID {} removed.", entry.path, entry.id),
            Err(e) => eprintln!("Warning: failed to remove '{}': {e}", entry.path),
        }
    }
}

/// Generates an auto-name for automatic snapshots using local time.
/// Format: %Y-%m-%d_%H-%M-%S (e.g., "2026-04-11_15-30-07").
fn generate_auto_name() -> String {
    let now: std::time::SystemTime = std::time::SystemTime::now();
    let duration = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs() as i64;

    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs as *const i64, &mut tm) };

    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
    )
}

/// Result of a snapshot operation.
pub enum SnapshotResult {
    Created(String, u64),
    Existed(String, u64),
    NotBtrfs,
}

/// Creates a snapshot of the root subvolume at the btrfs top level.
/// Auto-generates a timestamped name if none is given.
/// Returns Existed if a snapshot with the same name already exists.
/// Returns NotBtrfs if the root filesystem is not btrfs (nothing to protect).
pub fn snapshot(name: Option<&str>) -> Result<SnapshotResult, String> {
    let is_auto = name.is_none();
    let name = match name {
        Some(n) => {
            if is_auto_name(n) {
                return Err(format!(
                    "Name '{n}' uses the format reserved for automatic snapshots. \
                     Choose a different name."));
            }
            n.to_string()
        }
        None => generate_auto_name(),
    };
    let (_, fstab) = tools::root_device()?;
    let root_subvol = match tools::root_subvol_name(&fstab) {
        Ok(s) => s,
        Err(_) => return Ok(SnapshotResult::NotBtrfs),
    };

    let (name, created) = tools::with_toplevel(|toplevel| {
        let snap_path = format!("{toplevel}/{name}");
        if Path::new(&snap_path).exists() {
            return Ok((name.to_string(), false));
        }
        tools::btrfs_subvol_snapshot(&format!("{toplevel}/{}", root_subvol.as_str()), &snap_path)?;
        Ok((name.to_string(), true))
    })?;

    let id = tools::btrfs_subvol_id_by_name("/", &tools::SubvolName::new(name.clone()))?;

    if is_auto && created {
        retain_auto_snapshots();
    }

    if created {
        Ok(SnapshotResult::Created(name, id))
    } else {
        Ok(SnapshotResult::Existed(name, id))
    }
}

/// Snapshot metadata for the list command.
pub struct SnapshotInfo {
    pub id: u64,
    pub name: String,
    pub created: String,
}

/// Returns snapshot info (ID, name, creation time), excluding fstab system subvolumes.
/// Sorted by ID ascending (chronological order).
pub fn list() -> Result<Vec<SnapshotInfo>, String> {
    let protected = fstab_subvol_names()?;
    let entries = tools::btrfs_subvol_list("/")?;
    let snapshot_entries: Vec<&tools::SubvolEntry> = entries.iter()
        .filter(|e| !protected.iter().any(|p| p.as_str() == e.path))
        .collect();

    let mut snapshots = Vec::new();
    tools::with_toplevel(|toplevel| {
        for entry in &snapshot_entries {
            let path = format!("{toplevel}/{}", entry.path);
            let created = tools::btrfs_subvol_creation_time(&path)
                .unwrap_or_else(|_| "unknown".to_string());
            snapshots.push(SnapshotInfo {
                id: entry.id,
                name: entry.path.clone(),
                created,
            });
        }
        Ok(())
    })?;

    snapshots.sort_by_key(|s| s.id);
    Ok(snapshots)
}

/// Refuses subvolumes referenced by fstab (system subvolumes).
/// Mounted and default subvolume protection from kernel and btrfs-progs.
pub fn delete(name: &str) -> Result<u64, String> {
    let protected = fstab_subvol_names()?;
    if protected.contains(&tools::SubvolName::new(name.to_string())) {
        return Err(format!(
            "Cannot delete '{name}': referenced by /etc/fstab as a system subvolume. \
             Deleting it would break the system."));
    }

    let id = tools::btrfs_subvol_id_by_name("/", &tools::SubvolName::new(name.to_string()))?;
    tools::run_stdout("btrfs", &["subvolume", "delete", "--subvolid", &id.to_string(), "/"])
        .map(|_| id)
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
    fn auto_name_detection() {
        assert!(super::is_auto_name("2026-04-11_15-30-07"));
        assert!(super::is_auto_name("1999-01-01_00-00-00"));
        assert!(!super::is_auto_name("my-snapshot"));
        assert!(!super::is_auto_name("root.pre-update"));
        assert!(!super::is_auto_name("2026-04-11_15-30-0"));  // 18 chars
        assert!(!super::is_auto_name("2026-04-11_15-30-070")); // 20 chars
        assert!(!super::is_auto_name("2026-04-11 15:30:07")); // wrong separators
        assert!(!super::is_auto_name("abcd-ef-gh_ij-kl-mn")); // letters
    }

    #[test]
    fn config_parsing() {
        assert_eq!(super::parse_max_snapshots("MAX_SNAPSHOTS=50"), 50);
        assert_eq!(super::parse_max_snapshots("MAX_SNAPSHOTS=10"), 10);
        assert_eq!(super::parse_max_snapshots("MAX_SNAPSHOTS=\"25\""), 25);
        assert_eq!(super::parse_max_snapshots("# comment\nMAX_SNAPSHOTS=30\n"), 30);
        assert_eq!(super::parse_max_snapshots(""), crate::consts::MAX_SNAPSHOTS);
        assert_eq!(super::parse_max_snapshots("MAX_SNAPSHOTS=abc"), crate::consts::MAX_SNAPSHOTS);
        assert_eq!(super::parse_max_snapshots("OTHER_KEY=50"), crate::consts::MAX_SNAPSHOTS);
        assert_eq!(super::parse_max_snapshots("MAX_SNAPSHOTS=0"), 0);
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

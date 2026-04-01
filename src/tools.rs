//! Wrappers for external tools (btrfs-progs, blkid, findmnt, mount,
//! dracut, grub2-mkconfig, rsync) and fstab parsing helpers. Each
//! function delegates to a system tool and returns structured results.

use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::fs;

use crate::consts::{BTRFS_TOPLEVEL_SUBVOLID, FSTAB_MOUNT_POINT, FSTAB_OPTIONS, PROBE_MOUNT_PREFIX, TOPLEVEL_MOUNT};

/// Flush all pending filesystem changes to disk.
/// Btrfs operations (RENAME_EXCHANGE, set-default) use btrfs_end_transaction,
/// which commits to the in-memory journal but NOT to disk. Changes are lost
/// on power failure until the next btrfs transaction commit (up to 30s).
/// syncfs forces the commit.
pub fn sync_filesystem(path: &str) -> Result<(), String> {
    let f = fs::File::open(path)
        .map_err(|e| format!("open {path} for sync: {e}"))?;
    let ret = unsafe { libc::syncfs(f.as_raw_fd()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(format!("syncfs {path}: {}", std::io::Error::last_os_error()))
    }
}

/// Runs a command and returns stdout as a trimmed string. Fails on non-zero exit.
/// Uses from_utf8_lossy: all wrapped tools (btrfs, blkid, findmnt) produce ASCII.
pub fn run_stdout(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd).args(args).output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{cmd} {}: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Runs a command. On failure, includes stderr in the error message.
fn run_ok(cmd: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(cmd).args(args).output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{cmd} {}: {stderr}", args.join(" ")))
    }
}

// --- blkid ---

/// Returns the block device path for a filesystem UUID (e.g. "UUID" -> "/dev/sda2").
pub fn blkid_device_for_uuid(uuid: &str) -> Result<String, String> {
    run_stdout("blkid", &["--uuid", uuid])
}

/// Resolves a /dev/disk/ symlink to the real device path.
fn resolve_udev_symlink(subdir: &str, value: &str) -> Result<String, String> {
    let link = format!("/dev/disk/{subdir}/{value}");
    fs::canonicalize(&link)
        .map_err(|e| format!("{link}: {e}"))?
        .to_str()
        .ok_or_else(|| format!("{link}: non-UTF8 device path"))
        .map(|s| s.to_string())
}

/// Resolves a fstab device field to a block device path.
/// Handles all six mount(8) tag formats defined in libmount's
/// mnt_valid_tagname() (libmount/src/utils.c:47): UUID=, LABEL=,
/// PARTUUID=, PARTLABEL=, ID=, and raw /dev/ paths.
/// PARTUUID/PARTLABEL/ID resolve via /dev/disk/ symlinks
/// (udev 60-persistent-storage.rules).
/// Note: systemd fstab-generator only handles four tags (no ID=).
/// ID= in fstab works with mount(8) but not with systemd boot.
pub fn resolve_fstab_device(device: &str) -> Result<String, String> {
    if let Some(uuid) = device.strip_prefix("UUID=") {
        blkid_device_for_uuid(uuid)
    } else if let Some(label) = device.strip_prefix("LABEL=") {
        run_stdout("blkid", &["-L", label])
    } else if let Some(partuuid) = device.strip_prefix("PARTUUID=") {
        resolve_udev_symlink("by-partuuid", partuuid)
    } else if let Some(partlabel) = device.strip_prefix("PARTLABEL=") {
        resolve_udev_symlink("by-partlabel", partlabel)
    } else if let Some(id) = device.strip_prefix("ID=") {
        resolve_udev_symlink("by-id", id)
    } else {
        Ok(device.to_string())
    }
}

/// Returns the filesystem type for a UUID (e.g. "btrfs", "ext4", "vfat").
pub fn blkid_fstype(uuid: &str) -> Result<String, String> {
    let device = blkid_device_for_uuid(uuid)?;
    run_stdout("blkid", &["-s", "TYPE", "-o", "value", &device])
}

// --- findmnt ---

/// Find the mount point for a UUID that GRUB path resolution should use.
/// - ext4/vfat: single mount point (e.g., /boot). Return it.
/// - Btrfs: multiple mount points (/, /home, /var). Return / specifically,
///   because that's where the root subvolume is mounted and where GRUB
///   paths resolve to Linux paths.
pub fn findmnt_target_for_uuid(uuid: &str) -> Result<String, String> {
    let out = run_stdout("findmnt", &["-n", "-o", "TARGET", "-S", &format!("UUID={uuid}")])?;
    let targets: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    match targets.len() {
        0 => Err(format!("UUID={uuid} not mounted")),
        1 => Ok(targets[0].to_string()),
        _ => {
            targets.iter()
                .find(|&&t| t == "/")
                .map(|t| t.to_string())
                .ok_or_else(|| format!("UUID={uuid} mounted at {targets:?} but not at /"))
        }
    }
}

/// Checks whether a path is an active mount point.
pub fn is_mountpoint(path: &Path) -> bool {
    Command::new("mountpoint").arg("-q").arg(path).status()
        .is_ok_and(|s| s.success())
}

// --- btrfs ---
// Output parsed against format: "ID %llu gen %llu top level %llu path %s\n"
// (cmds/subvolume-list.c:822). JSON output exists but is behind #if EXPERIMENTAL.

/// Lists all subvolumes on the filesystem containing mount_point.
pub fn btrfs_subvol_list(mount_point: &str) -> Result<String, String> {
    run_stdout("btrfs", &["subvolume", "list", mount_point])
}

/// Returns the default subvolume ID for the filesystem at mount_point.
pub fn btrfs_subvol_get_default(mount_point: &str) -> Result<u64, String> {
    let out = run_stdout("btrfs", &["subvolume", "get-default", mount_point])?;
    out.split_whitespace().nth(1)
        .and_then(|id| id.parse::<u64>().ok())
        .ok_or_else(|| format!("cannot parse default subvol ID from: {out}"))
}

/// Sets the default subvolume for the filesystem at mount_point.
pub fn btrfs_subvol_set_default(id: u64, mount_point: &str) -> Result<(), String> {
    run_ok("btrfs", &["subvolume", "set-default", &id.to_string(), mount_point])
}

/// Creates a btrfs snapshot of src at dst.
/// Captures stdout because btrfs prints to stdout, which conflicts
/// with the libdnf5 actions plugin's pipe when called from the dnf hook.
pub fn btrfs_subvol_snapshot(src: &str, dst: &str) -> Result<(), String> {
    run_stdout("btrfs", &["subvolume", "snapshot", src, dst]).map(|_| ())
}

/// Looks up a subvolume's ID by name. Parses btrfs subvolume list output.
pub fn btrfs_subvol_id_by_name(mount_point: &str, name: &str) -> Result<u64, String> {
    let list = btrfs_subvol_list(mount_point)?;
    list.lines()
        .find(|line| line.split_whitespace().last().is_some_and(|n| n == name))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|id| id.parse::<u64>().ok())
        .ok_or_else(|| format!("subvol '{name}' not found on {mount_point}"))
}

// --- mount/umount ---

pub fn mount_ro(device: &str, target: &str) -> Result<(), String> {
    run_ok("mount", &["-o", "ro", device, target])
}

pub fn mount_subvolid(device: &str, target: &str, subvolid: u64) -> Result<(), String> {
    run_ok("mount", &["-o", &format!("subvolid={subvolid}"), device, target])
}

pub fn umount(target: &str) -> Result<(), String> {
    run_ok("umount", &[target])
}

// --- dracut ---

pub fn dracut_rebuild(output: &str, kver: &str) -> Result<(), String> {
    run_ok("dracut", &[output, kver])
}

// --- grub ---

pub fn grub2_mkconfig(output: &str) -> Result<(), String> {
    run_ok("grub2-mkconfig", &["-o", output])
}

// --- rsync ---

pub fn rsync(src: &str, dst: &str) -> Result<(), String> {
    run_ok("rsync", &["-a", src, dst])
}

// --- fstab helpers ---

/// Read /etc/fstab and return the root device path (resolved from UUID=/LABEL=/dev path).
pub fn root_device() -> Result<(String, String), String> {
    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let root_device = fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(FSTAB_MOUNT_POINT).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().next())
        .ok_or("Cannot find root entry in /etc/fstab")?
        .to_string();
    let device = resolve_fstab_device(&root_device)?;
    Ok((device, fstab))
}

/// Extract the root subvolume name from fstab (the subvol= value for /).
pub fn root_subvol_name(fstab: &str) -> Result<String, String> {
    fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(FSTAB_MOUNT_POINT).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().nth(FSTAB_OPTIONS))
        .and_then(|opts| crate::parse::extract_mount_option(opts, "subvol"))
        .map(|s| s.to_string())
        .ok_or_else(|| "Cannot determine root subvolume name from /etc/fstab".into())
}

/// Mount the top-level subvolume (subvolid=5), run a closure, unmount.
/// Guarantees unmount on both success and failure.
pub fn with_toplevel<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&str) -> Result<T, String>,
{
    let toplevel = TOPLEVEL_MOUNT;
    let (device, _) = root_device()?;

    fs::create_dir_all(toplevel).map_err(|e| format!("mkdir {toplevel}: {e}"))?;
    mount_subvolid(&device, toplevel, BTRFS_TOPLEVEL_SUBVOLID)?;

    let result = f(toplevel);

    // Best-effort cleanup. A stale mount or temp dir persists until
    // reboot but does not affect the boot chain.
    let _ = umount(toplevel);
    let _ = fs::remove_dir(toplevel);

    result
}

// --- probe mount: mount a UUID temporarily if not already mounted ---

pub fn get_mount_point(uuid: &str) -> Result<MountPoint, String> {
    if let Ok(target) = findmnt_target_for_uuid(uuid) {
        if !target.is_empty() {
            return Ok(MountPoint::Existing(target));
        }
    }

    let probe_dir = format!("{}{}", PROBE_MOUNT_PREFIX, &uuid[..8.min(uuid.len())]);
    fs::create_dir_all(&probe_dir).map_err(|e| format!("mkdir {probe_dir}: {e}"))?;
    let device = blkid_device_for_uuid(uuid)?;
    mount_ro(&device, &probe_dir)?;
    Ok(MountPoint::Probed(probe_dir))
}

/// A filesystem mount point, either already mounted or probed temporarily.
/// Probed mounts are unmounted on drop.
pub enum MountPoint {
    /// Already mounted by the system (e.g. / or /home).
    Existing(String),
    /// Temporarily mounted by this tool for inspection.
    Probed(String),
}

impl MountPoint {
    pub fn path(&self) -> &str {
        match self {
            MountPoint::Existing(p) | MountPoint::Probed(p) => p,
        }
    }
}

impl Drop for MountPoint {
    fn drop(&mut self) {
        if let MountPoint::Probed(p) = self {
            // Best-effort. Failure means the mount persists until reboot.
            let _ = Command::new("umount").arg(p.as_str()).output();
            let _ = fs::remove_dir(p.as_str());
        }
    }
}

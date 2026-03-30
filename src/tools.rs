use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::fs;

/// Flush all pending filesystem changes to disk.
/// Btrfs operations (RENAME_EXCHANGE, set-default) use btrfs_end_transaction,
/// which commits to the in-memory journal but NOT to disk. Changes are lost
/// on power failure until the next btrfs transaction commit (up to 30s).
/// syncfs forces the commit. Call before any exit point where the user reboots.
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

/// Run a command, return stdout as trimmed string. Fails if exit code != 0.
pub fn run_stdout(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd).args(args).output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{cmd} {}: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a command, return success/failure without capturing output.
fn run_ok(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(cmd).args(args).status()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd} {} failed", args.join(" "))) }
}

// --- blkid ---

pub fn blkid_device_for_uuid(uuid: &str) -> Result<String, String> {
    run_stdout("blkid", &["--uuid", uuid])
}

/// Resolve a fstab device field (UUID=, LABEL=, or /dev/ path) to a block device path.
pub fn resolve_fstab_device(device: &str) -> Result<String, String> {
    if let Some(uuid) = device.strip_prefix("UUID=") {
        blkid_device_for_uuid(uuid)
    } else if let Some(label) = device.strip_prefix("LABEL=") {
        run_stdout("blkid", &["-L", label])
    } else {
        Ok(device.to_string())
    }
}

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
            // Multiple mounts (Btrfs with subvolumes). Use / specifically.
            targets.iter()
                .find(|&&t| t == "/")
                .map(|t| t.to_string())
                .ok_or_else(|| format!("UUID={uuid} mounted at {targets:?} but not at /"))
        }
    }
}

pub fn is_mountpoint(path: &Path) -> bool {
    Command::new("mountpoint").arg("-q").arg(path).status()
        .is_ok_and(|s| s.success())
}

// --- btrfs ---

pub fn btrfs_subvol_list(mount_point: &str) -> Result<String, String> {
    run_stdout("btrfs", &["subvolume", "list", mount_point])
}

pub fn btrfs_subvol_get_default(mount_point: &str) -> Result<u64, String> {
    let out = run_stdout("btrfs", &["subvolume", "get-default", mount_point])?;
    out.split_whitespace().nth(1)
        .and_then(|id| id.parse::<u64>().ok())
        .ok_or_else(|| format!("cannot parse default subvol ID from: {out}"))
}

pub fn btrfs_subvol_set_default(id: u64, mount_point: &str) -> Result<(), String> {
    run_ok("btrfs", &["subvolume", "set-default", &id.to_string(), mount_point])
}

pub fn btrfs_subvol_snapshot(src: &str, dst: &str) -> Result<(), String> {
    // Use run_stdout to capture (and discard) stdout.
    // btrfs prints "Create snapshot of '...' in '...'" which conflicts
    // with the libdnf5 actions plugin's stdout pipe.
    run_stdout("btrfs", &["subvolume", "snapshot", src, dst]).map(|_| ())
}

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

// --- probe mount: mount a UUID temporarily if not already mounted ---

pub fn get_mount_point(uuid: &str) -> Result<MountPoint, String> {
    if let Ok(target) = findmnt_target_for_uuid(uuid) {
        if !target.is_empty() {
            return Ok(MountPoint::Existing(target));
        }
    }

    let probe_dir = format!("/tmp/atomic-rollback-probe-{}", &uuid[..8.min(uuid.len())]);
    fs::create_dir_all(&probe_dir).map_err(|e| format!("mkdir {probe_dir}: {e}"))?;
    let device = blkid_device_for_uuid(uuid)?;
    mount_ro(&device, &probe_dir)?;
    Ok(MountPoint::Probed(probe_dir))
}

pub enum MountPoint {
    Existing(String),
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
            let _ = Command::new("umount").arg(p.as_str()).output();
            let _ = fs::remove_dir(p.as_str());
        }
    }
}

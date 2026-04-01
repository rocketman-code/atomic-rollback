//! GRUB path resolution context. Translates GRUB filesystem paths to
//! Linux paths for boot chain verification. GRUB resolves paths differently
//! depending on the target filesystem (ext4 vs btrfs) and whether
//! btrfs_relative_path is set.

use std::fs;
use std::path::{Path, PathBuf};

use crate::platform::FEDORA as P;
use crate::tools;

/// Captures how GRUB resolves paths on the target filesystem.
/// Used by check.rs to verify that GRUB can find kernels and configs.
pub struct GrubContext {
    pub target_fstype: String,
    pub btrfs_relative: bool,
    pub linux_mount_point: String,
    // Never read directly. Present so that MountPoint::Probed is
    // unmounted when GrubContext is dropped (Rust Drop convention).
    _mount: Option<tools::MountPoint>,
}

impl GrubContext {
    /// Builds a context from the live system's ESP grub.cfg.
    /// Reads the target UUID, determines filesystem type, and mounts
    /// the target if not already mounted.
    pub fn from_system(root: &Path) -> Result<Self, String> {
        let esp_cfg = root.join(&P.esp_dir[1..]).join("grub.cfg");
        let content = fs::read_to_string(&esp_cfg)
            .map_err(|e| format!("cannot read ESP grub.cfg: {e}"))?;

        let target_uuid = content.lines()
            .find(|l| l.contains("search") && l.contains("--fs-uuid"))
            .and_then(|l| l.split_whitespace().last())
            .map(|s| s.to_string())
            .ok_or("ESP grub.cfg: cannot determine target UUID")?;

        let btrfs_relative = content.lines()
            .any(|l| l.contains("btrfs_relative_path") && l.contains("yes"));

        let target_fstype = tools::blkid_fstype(&target_uuid).unwrap_or_default();

        let mount = tools::get_mount_point(&target_uuid)?;
        let linux_mount_point = mount.path().to_string();

        Ok(Self { target_fstype, btrfs_relative, linux_mount_point, _mount: Some(mount) })
    }

    /// Builds a context for verifying a snapshot before rollback.
    /// Reads ESP config from the live system (ESP is vfat, not in the snapshot).
    /// Resolves GRUB paths against the snapshot mount, not the live root.
    pub fn for_snapshot(snapshot_root: &Path) -> Result<Self, String> {
        let esp_cfg = Path::new(P.esp_dir).join("grub.cfg");
        let content = fs::read_to_string(esp_cfg)
            .map_err(|e| format!("cannot read ESP grub.cfg: {e}"))?;

        let target_uuid = content.lines()
            .find(|l| l.contains("search") && l.contains("--fs-uuid"))
            .and_then(|l| l.split_whitespace().last())
            .map(|s| s.to_string())
            .ok_or("ESP grub.cfg: cannot determine target UUID")?;

        let btrfs_relative = content.lines()
            .any(|l| l.contains("btrfs_relative_path") && l.contains("yes"));

        let target_fstype = tools::blkid_fstype(&target_uuid).unwrap_or_default();

        let linux_mount_point = snapshot_root.to_string_lossy().to_string();

        Ok(Self { target_fstype, btrfs_relative, linux_mount_point, _mount: None })
    }

    /// Translates a GRUB path to a Linux filesystem path.
    pub fn resolve_to_linux_path(&self, grub_path: &str) -> PathBuf {
        let clean = grub_path.trim_start_matches('/');
        Path::new(&self.linux_mount_point).join(clean)
    }

    /// Returns Ok if the GRUB path resolves to an existing file on disk.
    pub fn check_path_exists(&self, grub_path: &str) -> Result<(), String> {
        let linux_path = self.resolve_to_linux_path(grub_path);
        if linux_path.exists() {
            Ok(())
        } else {
            Err(format!("'{grub_path}' resolves to '{}', not found", linux_path.display()))
        }
    }
}

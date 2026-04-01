//! Shared constants used across multiple modules.

// Fstab field positions (columns in whitespace-delimited /etc/fstab lines).
// fs_spec  fs_file  fs_vfstype  fs_mntops  fs_freq  fs_passno
// See fstab(5).
pub const FSTAB_MOUNT_POINT: usize = 1;
pub const FSTAB_FSTYPE: usize = 2;
pub const FSTAB_OPTIONS: usize = 3;

// Default snapshot name, used by the dnf plugin and bare `snapshot` command.
pub const DEFAULT_SNAPSHOT_NAME: &str = "root.pre-update";

// Temporary mount point for the btrfs top-level subvolume.
pub const TOPLEVEL_MOUNT: &str = "/mnt/atomic-rollback-toplevel";

// Temporary mount point prefix for probing unmounted filesystems.
pub const PROBE_MOUNT_PREFIX: &str = "/tmp/atomic-rollback-probe-";

// Btrfs top-level subvolume ID. Always 5 by definition.
// All user subvolumes (root, home, var) are children of this.
pub const BTRFS_TOPLEVEL_SUBVOLID: u64 = 5;

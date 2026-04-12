//! Shared constants used across multiple modules.

// Default snapshot name, used by the RPM plugin and bare `snapshot` command.
pub const DEFAULT_SNAPSHOT_NAME: &str = "root.pre-update";

// Temporary mount point for the btrfs top-level subvolume.
pub const TOPLEVEL_MOUNT: &str = "/mnt/atomic-rollback-toplevel";

// Temporary mount point prefix for probing unmounted filesystems.
pub const PROBE_MOUNT_PREFIX: &str = "/tmp/atomic-rollback-probe-";

// Btrfs top-level subvolume ID. Always 5 by definition.
// All user subvolumes (root, home, var) are children of this.
pub const BTRFS_TOPLEVEL_SUBVOLID: u64 = 5;

// Configuration file path. Shell key=value format.
pub const CONFIG_PATH: &str = "/etc/atomic-rollback.conf";

// Maximum number of automatic snapshots to retain. Overridden by
// MAX_SNAPSHOTS in CONFIG_PATH if present.
pub const MAX_SNAPSHOTS: usize = 50;

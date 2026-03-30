// Fstab field positions (columns in whitespace-delimited /etc/fstab lines).
// fs_spec  fs_file  fs_vfstype  fs_mntops  fs_freq  fs_passno
// See fstab(5).
pub const FSTAB_MOUNT_POINT: usize = 1;
pub const FSTAB_FSTYPE: usize = 2;
pub const FSTAB_OPTIONS: usize = 3;

// Btrfs top-level subvolume ID. Always 5 by definition.
// All user subvolumes (root, home, var) are children of this.
pub const BTRFS_TOPLEVEL_SUBVOLID: u64 = 5;

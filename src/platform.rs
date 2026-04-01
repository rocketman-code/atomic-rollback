//! Distro-specific filesystem paths. One const per supported distro.
//! All paths are absolute. When joining with a root path (e.g. a snapshot
//! mount), strip the leading / with &path[1..].

/// Paths that vary between Linux distributions.
pub struct Platform {
    /// EFI System Partition GRUB directory (contains shim, grub.cfg)
    pub esp_dir: &'static str,
    /// Main GRUB directory on the boot filesystem
    pub grub_dir: &'static str,
    /// Boot Loader Specification entry directory
    pub bls_dir: &'static str,
    /// systemd machine ID file
    pub machine_id: &'static str,
    /// systemd binary (used to detect systemd-based init)
    pub systemd_path: &'static str,
}

pub const FEDORA: Platform = Platform {
    esp_dir: "/boot/efi/EFI/fedora",
    grub_dir: "/boot/grub2",
    bls_dir: "/boot/loader/entries",
    machine_id: "/etc/machine-id",
    systemd_path: "/usr/lib/systemd/systemd",
};

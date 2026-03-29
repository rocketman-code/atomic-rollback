/// Platform-specific paths. One const per distro.
/// All paths stored with leading / for absolute use.
/// For root.join(), strip with &path[1..].
pub struct Platform {
    pub esp_dir: &'static str,
    pub grub_dir: &'static str,
    pub bls_dir: &'static str,
    pub machine_id: &'static str,
    pub systemd_path: &'static str,
}

pub const FEDORA: Platform = Platform {
    esp_dir: "/boot/efi/EFI/fedora",
    grub_dir: "/boot/grub2",
    bls_dir: "/boot/loader/entries",
    machine_id: "/etc/machine-id",
    systemd_path: "/usr/lib/systemd/systemd",
};

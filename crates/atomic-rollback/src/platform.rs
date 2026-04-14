//! Distro-specific filesystem paths. One const per supported distro.
//! All paths are absolute. When joining with a root path (e.g. a snapshot
//! mount), strip the leading / with &path[1..].

/// Paths that vary between Linux distributions.
pub struct Platform {
    /// UEFI architecture suffix (Spec 2.10 Section 3.5.1.1)
    pub efi_suffix: &'static str,
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
    // UEFI suffix: x86_64 -> x64, aarch64 -> aa64, riscv64 -> riscv64
    // Full table: grub.macros in Fedora grub2 source, or UEFI Spec 2.10 Table 3.2
    #[cfg(target_arch = "x86_64")]
    efi_suffix: "x64",
    #[cfg(target_arch = "aarch64")]
    efi_suffix: "aa64",
    esp_dir: "/boot/efi/EFI/fedora",
    grub_dir: "/boot/grub2",
    bls_dir: "/boot/loader/entries",
    machine_id: "/etc/machine-id",
    systemd_path: "/usr/lib/systemd/systemd",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_has_efi_suffix() {
        assert!(!FEDORA.efi_suffix.is_empty());
        if cfg!(target_arch = "x86_64") {
            assert_eq!(FEDORA.efi_suffix, "x64");
        } else if cfg!(target_arch = "aarch64") {
            assert_eq!(FEDORA.efi_suffix, "aa64");
        }
    }
}

//! BOOTS predicate: evaluates whether the boot chain is valid by tracing
//! the boot chain from UEFI firmware through GRUB to the root filesystem.
//! Each check corresponds to a link in the chain:
//!   UEFI -> shim -> GRUB -> grub.cfg -> BLS entries -> kernel -> root mount -> fstab

use std::fs;
use std::path::Path;

use crate::grub::GrubContext;
use crate::parse;
use crate::platform::FEDORA as P;
use crate::tools;

type CheckResult = Result<(), String>;

/// Result of the BOOTS predicate evaluation.
pub enum BootStatus {
    Pass,
    Warn,
    Fail(Vec<String>),
}

fn evaluate_checks(checks: Vec<(&str, Vec<CheckResult>)>) -> BootStatus {
    let mut all_failures = Vec::new();
    let mut warned = false;
    for (name, results) in &checks {
        if results.is_empty() {
            warned = true; // Check group handled its own WARN display
            continue;
        }
        let failures: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        if failures.is_empty() {
            println!("  PASS  {name}");
        } else {
            println!("  FAIL  {name}");
            for f in &failures {
                println!("        {f}");
                all_failures.push(format!("{name}: {f}"));
            }
        }
    }
    if !all_failures.is_empty() {
        BootStatus::Fail(all_failures)
    } else if warned {
        BootStatus::Warn
    } else {
        BootStatus::Pass
    }
}

pub fn verify_bootable(root: &Path) -> BootStatus {
    let grub = match GrubContext::from_system(root) {
        Ok(g) => g,
        Err(e) => return BootStatus::Fail(vec![format!(
            "Cannot determine how GRUB boots this system: {e}. \
             Ensure {}/grub.cfg exists and the root filesystem is mounted.", P.esp_dir
        )]),
    };

    evaluate_checks(vec![
        ("EFI boot files", check_esp(root)),
        ("GRUB configuration", check_grub_config(root, &grub)),
        ("Kernel boot entry", check_bls_entries(root, &grub)),
        ("Root filesystem", check_root_mountable(root)),
        ("System mounts", check_fstab_mounts(root)),
    ])
}

/// Skips ESP checks (vfat, external to the btrfs snapshot) and
/// default subvol match (set-default comes after the swap).
pub fn verify_snapshot_bootable(root: &Path) -> BootStatus {
    let grub = match GrubContext::for_snapshot(root) {
        Ok(g) => g,
        Err(e) => return BootStatus::Fail(vec![format!(
            "Cannot determine boot configuration: {e}. \
             Ensure {}/grub.cfg exists.", P.esp_dir
        )]),
    };

    let grub_cfg = root.join(&P.grub_dir[1..]).join("grub.cfg");
    let grub_checks = vec![
        check_file_exists_nonempty(&grub_cfg).map_err(|_|
            format!("{} does not exist. \
                     GRUB will drop to a rescue shell on next boot. \
                     The snapshot is missing its GRUB configuration.",
                    grub_cfg.display())),
        check_file_contains(&grub_cfg, "blscfg",
            "Snapshot's GRUB configuration does not use Boot Loader Specification entries. \
             Kernel boot entries will not appear in the GRUB menu.".into()),
    ];

    evaluate_checks(vec![
        ("GRUB configuration", grub_checks),
        ("Kernel boot entry", check_bls_entries(root, &grub)),
        ("Root filesystem", check_root_mountable(root)),
        ("System mounts", check_fstab_mounts(root)),
    ])
}

/// Gate: run verify_bootable, exit on failure.
/// swap_artifact: if this gate follows a RENAME_EXCHANGE, the path where
/// the old file now lives (e.g., "/boot/grub2/grub.cfg.new"). On failure,
/// the user is told the swap completed and where the old file is.
pub fn gate(step: &str, root: &Path, swap_artifact: Option<&str>) {
    println!();
    match verify_bootable(root) {
        BootStatus::Pass => println!("  GATE {step}: PASS\n"),
        BootStatus::Warn => println!("  GATE {step}: PASS\n"),
        BootStatus::Fail(failures) => {
            println!("\n  GATE {step}: FAIL");
            if let Some(old) = swap_artifact {
                eprintln!("    The swap completed. Old file preserved at {old}.");
            }
            for f in &failures {
                eprintln!("    {f}");
            }
            std::process::exit(1);
        }
    }
}

// --- EFI boot files ---

fn check_esp(root: &Path) -> Vec<CheckResult> {
    let efi_dir = root.join(&P.esp_dir[1..]);
    let mut results = Vec::new();

    let s = P.efi_suffix;
    for name in [format!("shim{s}.efi"), format!("grub{s}.efi")] {
        let path = efi_dir.join(&name);
        results.push(check_file_exists_nonempty(&path).map_err(|_|
            format!("{name} is missing from the EFI partition. \
                     The system cannot boot without it. \
                     Reinstall the grub2-efi package: sudo dnf reinstall grub2-efi-{s}")
        ));
    }

    let grub_cfg = efi_dir.join("grub.cfg");
    results.push(check_file_exists_nonempty(&grub_cfg).map_err(|_|
        format!("EFI grub.cfg is missing at {}. \
                 GRUB will drop to a rescue shell on next boot. \
                 Reinstall: sudo dnf reinstall grub2-efi-{s}", grub_cfg.display())
    ));
    results.push(check_file_contains(&grub_cfg, "--fs-uuid",
        "EFI grub.cfg does not contain a filesystem search directive. \
         GRUB will not find the boot partition. \
         Regenerate with: sudo grub2-switch-to-blscfg"));
    results.push(check_file_contains(&grub_cfg, "configfile",
        "EFI grub.cfg does not load the main GRUB configuration. \
         GRUB will drop to a rescue shell. \
         Regenerate with: sudo grub2-switch-to-blscfg"));

    results
}

// --- GRUB configuration ---

fn check_grub_config(root: &Path, grub: &GrubContext) -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check grub.cfg exists where it always is on Fedora.
    // The ESP's configfile directive (whether $prefix/grub.cfg or ($root)/boot/grub2/grub.cfg)
    // is GRUB's mechanism for reaching this file. The other checks (--fs-uuid, default subvol,
    // btrfs_relative_path) guarantee GRUB can reach it. We verify the file, not the mechanism.
    let grub_cfg = root.join(&P.grub_dir[1..]).join("grub.cfg");
    let grub_cfg_path = format!("{}/grub.cfg", P.grub_dir);
    results.push(check_file_exists_nonempty(&grub_cfg).map_err(|_|
        format!("{} does not exist. \
                 GRUB will drop to a rescue shell on next boot. \
                 Regenerate with: sudo grub2-mkconfig -o {grub_cfg_path}",
                grub_cfg.display())));

    results.push(check_file_contains(&grub_cfg, "blscfg",
        &format!("GRUB configuration does not use Boot Loader Specification entries. \
         Kernel boot entries will not appear in the GRUB menu. \
         Regenerate with: sudo grub2-mkconfig -o {grub_cfg_path}")));

    if matches!(grub.target_fstype, tools::FsType::Btrfs) && grub.btrfs_relative {
        results.push(check_default_subvol_matches_root(&grub.linux_mount_point, root));
    }

    results
}

fn check_default_subvol_matches_root(mount_point: &str, root: &Path) -> CheckResult {
    let default_id = tools::btrfs_subvol_get_default(mount_point)?;

    let fstab_content = fs::read_to_string(root.join("etc/fstab"))
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let lines = tools::parse_fstab(&fstab_content);

    let root_subvol_name = tools::fstab_entries(&lines).into_iter()
        .find(|e| e.fs_file == "/")
        .and_then(|e| parse::extract_mount_option(&e.fs_mntops, "subvol"))
        .map(|s| tools::SubvolName::new(s.to_string()));

    let root_subvol_name = match root_subvol_name {
        Some(name) => name,
        None => return Err(
            "Cannot determine root subvolume name from /etc/fstab. \
             The fstab entry for / must include subvol=<name>.".into()),
    };

    let root_subvol_id = tools::btrfs_subvol_id_by_name(mount_point, &root_subvol_name)?;

    if default_id == root_subvol_id {
        Ok(())
    } else {
        Err(format!(
            "GRUB and Linux see different root filesystems. \
             GRUB resolves paths from subvolume ID {default_id}, \
             but the subvolume '{root_subvol_name}' (mounted as /) has ID {root_subvol_id}. \
             The system will not boot correctly. \
             Fix: sudo btrfs subvolume set-default {root_subvol_id} /"
        ))
    }
}

// --- Kernel boot entry ---

fn check_bls_entries(root: &Path, grub: &GrubContext) -> Vec<CheckResult> {
    let entries_dir = root.join(&P.bls_dir[1..]);

    let confs: Vec<_> = match fs::read_dir(&entries_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "conf"))
            .collect(),
        Err(e) => {
            return vec![Err(format!(
                "Cannot read boot entries at {}: {e}. \
                 GRUB will show an empty menu.",
                entries_dir.display()))];
        }
    };

    if confs.is_empty() {
        return vec![Err(format!(
            "No boot entries found in {}. \
             GRUB will show an empty menu with no kernels to boot. \
             Reinstall the kernel: sudo dnf reinstall kernel-core",
            entries_dir.display()))];
    }

    // Validate ALL entries, not just until the first valid one
    let mut valid = 0;
    let mut failures: Vec<String> = Vec::new();
    for conf in &confs {
        match validate_bls_entry(grub, conf) {
            Ok(()) => valid += 1,
            Err(e) => failures.push(e),
        }
    }

    let total = confs.len();

    if valid == total {
        return vec![Ok(())];
    }

    if valid > 0 {
        // System boots but some entries are broken. WARN, not PASS.
        // Print here; return empty so evaluate_checks skips this group.
        println!("  WARN  Kernel boot entry ({valid} of {total} valid)");
        for f in &failures {
            println!("        {f}");
        }
        return vec![];
    }

    let mut results: Vec<CheckResult> = failures.into_iter().map(Err).collect();
    results.push(Err(
        "No boot entry has a valid kernel, initramfs, and root parameter. \
         The system cannot boot. \
         Rebuild the boot entry: sudo kernel-install add $(uname -r) /lib/modules/$(uname -r)/vmlinuz".into()));
    results
}

// BLS fields may contain GRUB variables after the file path:
//   initrd /initramfs-6.19.9.img $tuned_initrd
// Only the first token is a file path. The rest is GRUB's concern.
fn validate_bls_entry(grub: &GrubContext, conf: &Path) -> Result<(), String> {
    let content = fs::read_to_string(conf)
        .map_err(|e| format!("Cannot read boot entry {}: {e}", conf.display()))?;

    let linux = parse::bls_field(&content, "linux")
        .ok_or_else(|| format!(
            "Boot entry {} has no 'linux' field. \
             GRUB does not know which kernel to load.", conf.display()))?;
    let options = parse::bls_field(&content, "options")
        .ok_or_else(|| format!(
            "Boot entry {} has no 'options' field. \
             The kernel will not know which root filesystem to mount.", conf.display()))?;

    grub.check_path_exists(linux).map_err(|fact| format!(
        "GRUB cannot find the kernel ({fact}). \
         The system will not boot with this entry."))?;

    // initrd may appear on multiple lines (BLS spec). Each value may contain
    // GRUB variables ($tuned_initrd) which are resolved at boot, not checkable here.
    // GRUB stores everything after the first delimiter as the value (blsuki.c:316).
    let initrd_values = parse::bls_field_all(&content, "initrd");
    if initrd_values.is_empty() {
        return Err(format!(
            "Boot entry {} has no 'initrd' field. \
             The kernel will panic without an initial ramdisk.", conf.display()));
    }
    for initrd_val in &initrd_values {
        for path in initrd_val.split_whitespace() {
            if path.starts_with('$') { continue; } // GRUB variable
            grub.check_path_exists(path).map_err(|fact| format!(
                "GRUB cannot find the initial ramdisk ({fact}). \
                 The kernel will panic during boot."))?;
        }
    }

    // root= accepts UUID=, LABEL= (initramfs-resolved), PARTUUID=, PARTLABEL=,
    // /dev/ paths (kernel early_lookup_bdev, block/early-lookup.c:244).
    if !options.contains("root=") {
        return Err(format!(
            "Boot entry {} does not specify a root filesystem (missing root= parameter). \
             The kernel will not know what to mount.",
            conf.display()));
    }

    Ok(())
}

// --- Root filesystem ---

fn check_root_mountable(root: &Path) -> Vec<CheckResult> {
    let mut results = Vec::new();

    let root_uuid = match extract_root_uuid(root) {
        Some(uuid) => uuid,
        None => {
            results.push(Err(
                "Cannot determine root filesystem UUID from boot entries. \
                 No boot entry specifies root=UUID=. \
                 The system may not boot.".into()));
            return results;
        }
    };

    results.push(check_blkid_uuid_fstype(&root_uuid, tools::FsType::Btrfs).map_err(|e| format!(
        "Root filesystem UUID {} is not a Btrfs partition: {e}. \
         This tool requires Btrfs as the root filesystem.", root_uuid.as_str())));

    let fstab_content = match fs::read_to_string(root.join("etc/fstab")) {
        Ok(c) => c,
        Err(e) => {
            results.push(Err(format!(
                "Cannot read /etc/fstab to determine root subvolume: {e}. \
                 The system may not boot.")));
            return results;
        }
    };

    let root_subvol_name = match tools::root_subvol_name(&fstab_content) {
        Ok(name) => name,
        Err(e) => {
            results.push(Err(format!(
                "{e}. The fstab entry for / must include subvol=<name>.")));
            return results;
        }
    };

    let root_device = root_uuid.clone().into_device_spec();
    results.push(check_btrfs_subvol_exists(&root_device, &root_subvol_name).map_err(|e| format!(
        "Btrfs subvolume '{root_subvol_name}' not found: {e}. \
         The kernel expects to mount subvol={root_subvol_name} as /. \
         Without it, the system drops to an emergency shell.")));

    results.push(check_file_exists_nonempty(&root.join(&P.systemd_path[1..])).map_err(|_|
        format!("systemd (PID 1) is missing at {}. \
         The kernel will panic after mounting root. \
         Reinstall: sudo dnf reinstall systemd", P.systemd_path)));

    results
}

// --- System mounts ---

fn check_fstab_mounts(root: &Path) -> Vec<CheckResult> {
    let content = match fs::read_to_string(root.join("etc/fstab")) {
        Ok(c) => c,
        Err(e) => return vec![Err(format!(
            "Cannot read /etc/fstab: {e}. \
             systemd will not mount any filesystems."))],
    };

    let lines = tools::parse_fstab(&content);
    let mut results = Vec::new();
    for e in tools::fstab_entries(&lines) {
        if e.fs_mntops.contains("nofail") || e.fs_vfstype == tools::FsType::Swap
           || e.fs_file == "none" || e.fs_file == "swap" { continue; }

        results.push(check_fstab_entry(&e.fs_spec, &e.fs_file, &e.fs_vfstype, &e.fs_mntops));
    }

    results
}

fn check_fstab_entry(device: &tools::DeviceSpec, mount_point: &str, fstype: &tools::FsType, options: &str) -> CheckResult {
    tools::resolve_fstab_device(device)
        .map_err(|_| format!(
            "Mount {mount_point}: device {device} does not resolve. \
             systemd will fail to mount it and the system may hang at boot. \
             Check /etc/fstab or add 'nofail' to the mount options."))?;

    if *fstype == tools::FsType::Btrfs {
        if let Some(subvol) = parse::extract_mount_option(options, "subvol") {
            return check_btrfs_subvol_exists(device, &tools::SubvolName::new(subvol.to_string()))
                .map_err(|_| format!(
                    "Mount {mount_point}: Btrfs subvolume '{subvol}' does not exist. \
                     systemd will fail to mount it and the system may hang at boot. \
                     Create the subvolume or remove the fstab entry."));
        }
    }

    Ok(())
}

// --- Leaf helpers ---

fn check_file_exists_nonempty(path: &Path) -> CheckResult {
    match fs::metadata(path) {
        Ok(meta) if meta.len() > 0 => Ok(()),
        Ok(_) => Err(format!("{} exists but is empty", path.display())),
        Err(_) => Err(format!("{} does not exist", path.display())),
    }
}

fn check_file_contains(path: &Path, needle: &str, msg: &str) -> CheckResult {
    match fs::read_to_string(path) {
        Ok(content) if content.contains(needle) => Ok(()),
        Ok(_) => Err(msg.to_string()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

fn check_blkid_uuid_fstype(uuid: &tools::BareUuid, expected: tools::FsType) -> CheckResult {
    let fstype = tools::blkid_fstype(uuid)?;
    if fstype == expected { Ok(()) } else {
        Err(format!("UUID={} has unexpected filesystem type", uuid.as_str()))
    }
}

fn check_btrfs_subvol_exists(device_spec: &tools::DeviceSpec, name: &tools::SubvolName) -> CheckResult {
    let mount = tools::get_mount_point(device_spec)?;
    let entries = tools::btrfs_subvol_list(mount.path())?;
    if entries.iter().any(|e| e.path == name.as_str()) {
        Ok(())
    } else {
        Err(format!("subvolume '{name}' not found on {device_spec}"))
    }
}

pub fn print_rollback_scope(fstab: &str) {
    let lines = tools::parse_fstab(fstab);
    let entries = tools::fstab_entries(&lines);

    let protected: Vec<&str> = entries.iter()
        .filter(|e| e.fs_file != "/")
        .filter(|e| parse::extract_mount_option(&e.fs_mntops, "subvol").is_some())
        .map(|e| e.fs_file.as_str())
        .collect();

    println!("  Rollback scope:\n");

    if !protected.is_empty() {
        println!("  SAFE  Separate subvolumes (not affected by rollback):");
        for p in &protected {
            println!("        {p}");
        }
        println!();
    }

    let at_risk: Vec<String> = match fs::read_dir("/") {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
            .map(|e| format!("/{}", e.file_name().to_string_lossy()))
            .filter(|dir| !tools::is_mountpoint(Path::new(dir)))
            .filter(|dir| fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()))
            .collect(),
        Err(_) => vec![],
    };

    if !at_risk.is_empty() {
        println!("  RISK  Inside root subvolume (will revert on rollback):");
        for r in &at_risk {
            println!("        {r}");
        }
        println!();
    }
}

fn extract_root_uuid(root: &Path) -> Option<tools::BareUuid> {
    let entries_dir = root.join(&P.bls_dir[1..]);
    for entry in fs::read_dir(&entries_dir).ok()?.flatten() {
        if entry.path().extension().is_some_and(|ext| ext == "conf") {
            let content = fs::read_to_string(entry.path()).ok()?;
            if let Some(options) = parse::bls_field(&content, "options") {
                return parse::extract_root_uuid_from_options(options);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn check_esp_uses_arch_correct_filenames() {
        let dir = std::env::temp_dir().join("atomic-rollback-test-esp");
        let _ = fs::remove_dir_all(&dir);
        let efi_dir = dir.join(&P.esp_dir[1..]);
        fs::create_dir_all(&efi_dir).unwrap();

        fs::write(efi_dir.join(format!("shim{}.efi", P.efi_suffix)), b"shim").unwrap();
        fs::write(efi_dir.join(format!("grub{}.efi", P.efi_suffix)), b"grub").unwrap();

        fs::write(
            efi_dir.join("grub.cfg"),
            "search --no-floppy --fs-uuid --set=dev abc-123\nconfigfile $prefix/grub.cfg\n",
        ).unwrap();

        let results = check_esp(&dir);
        for (i, r) in results.iter().enumerate() {
            assert!(r.is_ok(), "check {i} failed: {:?}", r.as_ref().err());
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn root_subvol_name_reads_at_sign_layout() {
        let fstab = "\
UUID=abc / btrfs subvol=@,compress=zstd:1 0 0
UUID=abc /home btrfs subvol=@home,compress=zstd:1 0 0
UUID=abc /var btrfs subvol=@var 0 0
UUID=abc /opt btrfs subvol=@opt 0 0";
        let name = tools::root_subvol_name(fstab).unwrap();
        assert_eq!(name.as_str(), "@");
    }

    #[test]
    fn root_subvol_name_reads_fedora_default() {
        let fstab = "UUID=abc / btrfs subvol=root,compress=zstd:1 0 0";
        let name = tools::root_subvol_name(fstab).unwrap();
        assert_eq!(name.as_str(), "root");
    }

    #[test]
    fn root_subvol_name_errors_without_subvol_option() {
        let fstab = "UUID=abc / btrfs defaults 0 0";
        assert!(tools::root_subvol_name(fstab).is_err());
    }
}

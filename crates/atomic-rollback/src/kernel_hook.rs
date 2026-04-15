//! Kernel-install plugin logic. Called during dnf kernel upgrades to
//! create symlinks and fix BLS entry paths so GRUB resolves kernels
//! correctly on a migrated btrfs layout.

use std::fs;
use std::path::Path;

use crate::{check, parse, platform::FEDORA as P, swap, tools};

const HOOK_PATH: &str = "/usr/lib/kernel/install.d/90-atomic-rollback.install";
const HOOK_CONTENT: &str = "#!/usr/bin/bash\n\
# kernel-install plugin for atomic-rollback.\n\
# Thin dispatcher. All logic lives in the Rust binary, inside the proof boundary.\n\
exec /usr/bin/atomic-rollback kernel-hook \"$@\"\n";

/// Writes the kernel-install hook if the system is migrated and the hook is missing.
/// Called by `migrate` (as a migration artifact) and by `ensure-hooks` (for upgrades).
/// Non-fatal: logs errors but always returns Ok for use in %posttrans context.
pub fn ensure_hooks() {
    let root_fstype = match tools::run_stdout("findmnt", &["-n", "-o", "FSTYPE", "/"]) {
        Ok(s) => s,
        Err(_) => return,
    };
    if root_fstype != "btrfs" {
        return;
    }
    if tools::is_mountpoint(Path::new("/boot")) {
        return;
    }

    let hook = Path::new(HOOK_PATH);
    if hook.exists() {
        return;
    }

    if let Some(parent) = hook.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("atomic-rollback: cannot create {}: {e}", parent.display());
            return;
        }
    }

    if let Err(e) = fs::write(hook, HOOK_CONTENT) {
        eprintln!("atomic-rollback: cannot write {HOOK_PATH}: {e}");
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(hook, fs::Permissions::from_mode(0o755));
    }
}

/// Dispatched by /usr/lib/kernel/install.d/90-atomic-rollback.install.
/// Only acts when root is btrfs and /boot is not a separate mount
/// (i.e. the full migration has been applied).
pub fn handle(command: &str, kernel_version: &str) -> Result<(), String> {
    let root_fstype = tools::run_stdout("findmnt", &["-n", "-o", "FSTYPE", "/"])?;
    if root_fstype != "btrfs" {
        return Ok(()); // Not our business
    }
    if tools::is_mountpoint(Path::new("/boot")) {
        return Ok(()); // /boot is separate partition, migration not applied
    }

    match command {
        "add" => handle_add(kernel_version),
        "remove" => handle_remove(kernel_version),
        _ => Ok(()), // Unknown command, ignore
    }
}

fn handle_add(kver: &str) -> Result<(), String> {
    // Create symlinks at / so GRUB resolves /vmlinuz-$ver -> boot/vmlinuz-$ver
    create_symlink_if_needed(
        &format!("boot/vmlinuz-{kver}"),
        &format!("/vmlinuz-{kver}"),
    )?;
    create_symlink_if_needed(
        &format!("boot/initramfs-{kver}.img"),
        &format!("/initramfs-{kver}.img"),
    )?;

    // Fix BLS entry paths if grub2-mkrelpath wrote wrong prefix.
    fix_bls_paths(kver)?;

    // Symlinks and BLS swap are in the btrfs in-memory journal only.
    tools::sync_filesystem("/")?;

    // Gate: verify the boot chain is still valid.
    match check::verify_bootable(Path::new("/")) {
        check::BootStatus::Pass | check::BootStatus::Warn => Ok(()),
        check::BootStatus::Fail(failures) => {
            for f in &failures {
                eprintln!("atomic-rollback: WARNING after kernel {kver} install: {f}");
            }
            // Don't return Err; that would abort kernel-install and leave worse state.
            Ok(())
        }
        check::BootStatus::Inaccessible { reason, hint } => {
            // Kernel-install runs as root; reaching this branch is unusual.
            // Log and continue; do not abort kernel-install.
            eprintln!("atomic-rollback: WARNING after kernel {kver} install: cannot verify boot chain ({reason}). {hint}");
            Ok(())
        }
    }
}

fn handle_remove(kver: &str) -> Result<(), String> {
    // Best-effort: symlinks may not exist if the hook wasn't active
    // when this kernel was installed.
    let _ = fs::remove_file(format!("/vmlinuz-{kver}"));
    let _ = fs::remove_file(format!("/initramfs-{kver}.img"));
    Ok(())
}

fn create_symlink_if_needed(target: &str, link: &str) -> Result<(), String> {
    let link_path = Path::new(link);
    if link_path.exists() || link_path.symlink_metadata().is_ok() {
        return Ok(()); // Already exists (symlink or real file)
    }
    // Only create if the target file actually exists in /boot
    let boot_file = Path::new("/").join(target);
    if !boot_file.exists() {
        return Ok(()); // Target file does not exist
    }
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| format!("symlink {link} -> {target}: {e}"))
}

fn fix_bls_paths(kver: &str) -> Result<(), String> {
    let machine_id = fs::read_to_string(P.machine_id)
        .map_err(|e| format!("read machine-id: {e}"))?;
    let machine_id = machine_id.trim();

    let bls_dir = Path::new(P.bls_dir);
    let bls_name = format!("{machine_id}-{kver}.conf");
    let bls_path = bls_dir.join(&bls_name);
    if !bls_path.exists() {
        return Ok(()); // No BLS entry for this version
    }

    let content = fs::read_to_string(&bls_path)
        .map_err(|e| format!("read {}: {e}", bls_path.display()))?;

    // Correctness decision via verified parser (get half of the lens).
    // linux is single-valued, initrd may appear multiple times (BLS spec).
    let has_bad_path = |v: &str| -> bool {
        v.contains("/root/boot/") || v.contains("/boot/vmlinuz-") || v.contains("/boot/initramfs-")
    };
    let needs_fix = parse::bls_field(&content, "linux").is_some_and(|v| has_bad_path(v))
        || parse::bls_field_all(&content, "initrd").iter().any(|v| has_bad_path(v));

    // Line structure for transformation (put half of the lens).
    let bls_lines = tools::parse_bls(&content);

    if !needs_fix {
        return Ok(());
    }

    let linux_path = format!("/vmlinuz-{kver}");
    let initrd_path = format!("/initramfs-{kver}.img");

    // Create alongside
    let new_name = format!("{bls_name}.new");
    let new_path = bls_dir.join(&new_name);
    let _ = fs::remove_file(&new_path); // Clean up any leftover from interrupted run

    let new_content: String = bls_lines.iter()
        .map(|line| match line {
            tools::BlsLine::Field { key, value, prefix } if key == "linux" =>
                format!("{prefix}{linux_path}"),
            tools::BlsLine::Field { key, value, prefix } if key == "initrd" => {
                if value.contains("$tuned_initrd") {
                    format!("{prefix}{initrd_path} $tuned_initrd")
                } else {
                    format!("{prefix}{initrd_path}")
                }
            }
            _ => line.raw(),
        })
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(&new_path, &new_content)
        .map_err(|e| format!("write {}: {e}", new_path.display()))?;

    // Verify new paths resolve BEFORE the swap
    if !Path::new(&linux_path).exists() {
        let _ = fs::remove_file(&new_path);
        return Err(format!(
            "kernel symlink {linux_path} does not resolve. \
             BLS entry not updated; old entry preserved."));
    }
    if !Path::new(&initrd_path).exists() {
        let _ = fs::remove_file(&new_path);
        return Err(format!(
            "initramfs symlink {initrd_path} does not resolve. \
             BLS entry not updated; old entry preserved."));
    }

    // RENAME_EXCHANGE: old BLS entry preserved at .new
    swap::rename_exchange(bls_dir, &bls_name, &new_name)?;

    // Old BLS content is at .new after the swap. Stale file is harmless.
    let _ = fs::remove_file(&new_path);

    Ok(())
}

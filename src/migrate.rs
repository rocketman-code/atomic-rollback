use std::fs;
use std::path::Path;

use crate::{check, parse, platform::FEDORA as P, swap, tools};

/// Mount a target using fstab lookup (like `mount /boot/efi`).
fn run_mount_fstab(target: &str) -> Result<(), String> {
    let status = std::process::Command::new("mount").arg(target).status()
        .map_err(|e| format!("mount {target}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("mount {target} failed")) }
}

/// Setup: separate /var and enable root snapshots and rollback.
/// No /boot changes, no ESP modification, no GRUB Btrfs dependency.
/// Works on stock Fedora partition layout.
/// Proof: theorem 12 (setup_is_safe).
pub fn setup() -> Result<(), String> {
    let root = Path::new("/");

    check::gate("0-baseline", root, None);

    step3_set_default_subvol()?;
    check::gate("1-default-subvol", root, None);

    step10_separate_var()?;
    check::gate("2-var", root, Some("/etc/fstab.new"));

    tools::sync_filesystem("/")?;

    println!("Setup complete. Snapshots and rollback are enabled.");
    println!("Install the dnf plugin for automatic pre-update snapshots.");
    Ok(())
}

/// Full boot migration for atomic rollback with kernel rollback.
/// Moves /boot from ext4 to Btrfs, updates ESP, installs kernel-install hook.
/// 10 steps, each gated by the BOOTS predicate.
/// Each step: create alongside, verify, atomic swap.
pub fn migrate() -> Result<(), String> {
    let root = Path::new("/");

    check::gate("0-baseline", root, None);

    step1_ensure_boot_on_btrfs()?;
    check::gate("1-boot-on-btrfs", root, None);

    step2_create_symlinks()?;
    check::gate("2-symlinks", root, None);

    step3_set_default_subvol()?;
    check::gate("3-default-subvol", root, None);

    step4_switch_boot_mount()?;
    check::gate("4-boot-mount", root, None);

    step5_update_fstab()?;
    check::gate("5-fstab", root, Some("/etc/fstab.new"));

    step6_rebuild_initramfs()?;
    let kver = current_kernel_version()?;
    check::gate("6-initramfs", root, Some(&format!("/boot/initramfs-{kver}.img.new")));

    step7_regenerate_grub_cfg()?;
    check::gate("7-grub-cfg", root, Some(&format!("{}/grub.cfg.new", P.grub_dir)));

    step8_fix_grubenv()?;
    check::gate("8-grubenv", root, None);

    step9_update_esp()?;
    check::gate("9-esp", root, Some(&format!("{}/grub.cfg.new", P.esp_dir)));

    step10_separate_var()?;
    check::gate("10-var", root, Some("/etc/fstab.new"));

    // Flush all changes to disk before telling the user to reboot.
    // Btrfs RENAME_EXCHANGE and set-default use btrfs_end_transaction
    // (in-memory journal), not btrfs_commit_transaction (on-disk).
    tools::sync_filesystem("/")?;

    println!("Migration complete. All gates passed.");
    Ok(())
}

fn step1_ensure_boot_on_btrfs() -> Result<(), String> {
    println!("=== Step 1: Ensure /boot contents on Btrfs ===");

    if tools::is_mountpoint(Path::new("/boot")) {
        println!("  ext4 /boot is mounted. Copying to Btrfs");
        tools::umount("/boot/efi").ok();
        tools::umount("/boot")?;

        let _ = fs::create_dir_all("/mnt/old-boot");
        // Find the ext4 boot device from fstab
        let fstab = fs::read_to_string("/etc/fstab").map_err(|e| format!("read fstab: {e}"))?;
        let boot_dev = fstab.lines()
            .filter(|l| !l.trim().starts_with('#'))
            .find(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                parts.len() >= 2 && parts[1] == "/boot" && parts.get(2).is_some_and(|t| *t == "ext4")
            })
            .and_then(|l| l.split_whitespace().next())
            .and_then(|dev| dev.strip_prefix("UUID="))
            .ok_or("cannot find ext4 /boot in fstab")?;

        let device = tools::blkid_device_for_uuid(boot_dev)?;
        tools::mount_ro(&device, "/mnt/old-boot")?;
        tools::rsync("/mnt/old-boot/", "/boot/")?;
        tools::umount("/mnt/old-boot")?;

        // Remount ext4 /boot and /boot/efi (so current predicate holds)
        // Use `mount <target>` which looks up fstab. Matches the proven bash script.
        run_mount_fstab("/boot")?;
        run_mount_fstab("/boot/efi")?;
    } else {
        println!("  /boot already on Btrfs");
        if fs::read_dir("/boot").map_or(true, |mut d| d.next().is_none()) {
            return Err("/boot exists but is empty".into());
        }
    }

    Ok(())
}

fn step2_create_symlinks() -> Result<(), String> {
    println!("=== Step 2: Create kernel/initramfs symlinks ===");

    for entry in fs::read_dir("/boot").map_err(|e| format!("read /boot: {e}"))?.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let is_kernel = name_str.starts_with("vmlinuz-") && !name_str.ends_with(".new");
        let is_initrd = name_str.starts_with("initramfs-") && name_str.ends_with(".img")
                        && !name_str.ends_with(".img.new");
        if is_kernel || is_initrd {
            let link = Path::new("/").join(&*name_str);
            if !link.exists() {
                let target = format!("boot/{name_str}");
                std::os::unix::fs::symlink(&target, &link)
                    .map_err(|e| format!("symlink {} -> {target}: {e}", link.display()))?;
                println!("  {link} -> {target}", link = link.display());
            }
        }
    }

    Ok(())
}

fn step3_set_default_subvol() -> Result<(), String> {
    println!("=== Step 3: Set default subvol to root ===");

    let root_subvol = root_subvol_name()?;
    let root_id = tools::btrfs_subvol_id_by_name("/", &root_subvol)?;
    tools::btrfs_subvol_set_default(root_id, "/")?;
    println!("  default subvol '{root_subvol}' set to ID {root_id}");

    Ok(())
}

fn step4_switch_boot_mount() -> Result<(), String> {
    println!("=== Step 4: Switch /boot to Btrfs ===");

    if tools::is_mountpoint(Path::new("/boot")) {
        tools::umount("/boot/efi").ok();
        tools::umount("/boot")?;
        run_mount_fstab("/boot/efi")?;
        println!("  unmounted ext4, /boot now on Btrfs");
    } else {
        println!("  /boot already on Btrfs");
    }

    Ok(())
}

/// Remove a stale .new artifact from a previous interrupted migration.
/// RENAME_EXCHANGE always leaves the old file at the .new name.
/// On retry, the artifact blocks the step. Safe to remove: it's the
/// old version, which has no value after the swap succeeded.
fn remove_stale(path: &str) {
    if Path::new(path).exists() {
        let _ = fs::remove_file(path);
    }
}

fn step5_update_fstab() -> Result<(), String> {
    println!("=== Step 5: Update fstab ===");
    remove_stale("/etc/fstab.new");

    let content = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("read fstab: {e}"))?;

    let new_content: String = content.lines()
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[1] == "/boot" && parts[2] == "ext4"
               && !line.trim().starts_with('#')
            {
                format!("#MIGRATED: {line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    fs::write("/etc/fstab.new", &new_content)
        .map_err(|e| format!("write fstab.new: {e}"))?;
    swap::atomic_swap(Path::new("/etc"), "fstab", "fstab.new")?;

    Ok(())
}

fn step6_rebuild_initramfs() -> Result<(), String> {
    println!("=== Step 6: Rebuild initramfs ===");

    let kver = current_kernel_version()?;
    let new_path = format!("/boot/initramfs-{kver}.img.new");
    remove_stale(&new_path);
    tools::dracut_rebuild(&new_path, &kver)?;
    swap::atomic_swap(
        Path::new("/boot"),
        &format!("initramfs-{kver}.img"),
        &format!("initramfs-{kver}.img.new"),
    )?;

    Ok(())
}

fn step7_regenerate_grub_cfg() -> Result<(), String> {
    println!("=== Step 7: Regenerate grub.cfg ===");
    let new_path = format!("{}/grub.cfg.new", P.grub_dir);
    remove_stale(&new_path);

    tools::grub2_mkconfig(&new_path)?;

    // GRUB cannot write to Btrfs (definitional: the driver is read-only).
    // save_env requires write access (definitional: saving is writing).
    // Therefore save_env is a guaranteed failure on Btrfs.
    // Strip it from the generated grub.cfg to prevent the error flash.
    let content = fs::read_to_string(&new_path)
        .map_err(|e| format!("read grub.cfg.new: {e}"))?;
    let cleaned: String = content.lines()
        .filter(|line| !line.trim().starts_with("save_env"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&new_path, &cleaned)
        .map_err(|e| format!("write grub.cfg.new: {e}"))?;

    swap::atomic_swap(Path::new(P.grub_dir), "grub.cfg", "grub.cfg.new")?;

    Ok(())
}

fn step9_update_esp() -> Result<(), String> {
    println!("=== Step 9: Update ESP grub.cfg ===");
    let esp_cfg = format!("{}/grub.cfg", P.esp_dir);
    let esp_cfg_new = format!("{}/grub.cfg.new", P.esp_dir);
    remove_stale(&esp_cfg_new);

    // UUID for the filesystem containing /boot, from grub2-probe.
    let new_uuid = tools::run_stdout("grub2-probe", &["--target=fs_uuid", "/boot"])?;

    // Read the existing ESP grub.cfg. Preserve its format, variables, and flags.
    // Only substitute the UUID and add btrfs_relative_path if needed.
    let existing = fs::read_to_string(&esp_cfg)
        .map_err(|e| format!("read {esp_cfg}: {e}"))?;

    // Apply the two transformations required by the ext4→Btrfs transition:
    // 1. Replace the UUID (ext4 → Btrfs)
    // 2. Add btrfs_relative_path="yes" if not present (required for GRUB on Btrfs)
    // Everything else is preserved exactly as the package installed it.

    let has_btrfs_relative = existing.lines()
        .any(|l| l.contains("btrfs_relative_path"));

    // On ext4, GRUB prefix is /grub2 (partition-relative).
    // On Btrfs with default subvol, it needs /boot/grub2.
    let grub_basename = Path::new(P.grub_dir).file_name()
        .and_then(|n| n.to_str()).unwrap_or("grub2");
    let short_grub = format!("/{grub_basename}");
    let full_grub = format!("/boot/{grub_basename}");

    let mut lines: Vec<String> = existing.lines()
        .map(|line| {
            if line.contains("--fs-uuid") {
                let mut parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(last) = parts.last_mut() {
                    *last = &new_uuid;
                }
                let indent = &line[..line.len() - line.trim_start().len()];
                format!("{indent}{}", parts.join(" "))
            } else if line.contains("prefix=") && line.contains(&short_grub)
                      && !line.contains(&full_grub) {
                line.replace(&short_grub, &full_grub)
            } else if line.contains("configfile") && line.contains(&short_grub)
                      && !line.contains(&full_grub) {
                line.replace(&short_grub, &full_grub)
            } else {
                line.to_string()
            }
        })
        .collect();

    if !has_btrfs_relative {
        // Insert before the search line. GRUB needs it set before resolving paths.
        let search_idx = lines.iter().position(|l| l.contains("--fs-uuid")).unwrap_or(0);
        lines.insert(search_idx, "set btrfs_relative_path=\"yes\"".to_string());
    }

    let new_cfg = lines.join("\n");

    // Add trailing newline if original had one
    let new_cfg = if existing.ends_with('\n') && !new_cfg.ends_with('\n') {
        new_cfg + "\n"
    } else {
        new_cfg
    };

    println!("  Old UUID: {}", existing.lines()
        .find(|l| l.contains("--fs-uuid"))
        .and_then(|l| l.split_whitespace().last())
        .unwrap_or("?"));
    println!("  New UUID: {new_uuid}");

    fs::write(&esp_cfg_new, &new_cfg)
        .map_err(|e| format!("write {esp_cfg_new}: {e}"))?;

    // Verify the three ESP properties the model requires before swap.
    // BOOTS needs: esp_target_uuid correct, esp_has_btrfs_relative, esp_prefix_has_boot.
    if !new_cfg.contains(&new_uuid) {
        let _ = fs::remove_file(&esp_cfg_new);
        return Err(format!(
            "ESP grub.cfg.new does not contain the target UUID {new_uuid}. \
             Substitution failed. Old ESP preserved."));
    }
    if !new_cfg.lines().any(|l| l.contains("btrfs_relative_path") && l.contains("yes")) {
        let _ = fs::remove_file(&esp_cfg_new);
        return Err(
            "ESP grub.cfg.new missing btrfs_relative_path. \
             GRUB will not resolve paths from the default subvolume. \
             Old ESP preserved.".into());
    }
    if !new_cfg.lines().any(|l|
        (l.contains("prefix=") || l.contains("configfile")) && l.contains(&full_grub))
    {
        let _ = fs::remove_file(&esp_cfg_new);
        return Err(format!(
            "ESP grub.cfg.new missing {full_grub} in prefix or configfile path. \
             GRUB will not find the main configuration. \
             Old ESP preserved."));
    }

    swap::atomic_swap(Path::new(P.esp_dir), "grub.cfg", "grub.cfg.new")?;

    Ok(())
}

fn step10_separate_var() -> Result<(), String> {
    println!("=== Step 10: Separate /var into its own subvolume ===");

    // Check if /var is already a separate mount (Cloud VM has this)
    if tools::is_mountpoint(Path::new("/var")) {
        println!("  /var is already a separate mount. Skipping");
        return Ok(());
    }

    // /var is inside root. Separate it using Btrfs snapshot for consistent capture.
    println!("  /var is inside root. Separating into its own subvolume");

    // Read root mount entry from fstab. Derive /var entry from it.
    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("read fstab: {e}"))?;
    let root_entry: Vec<&str> = fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(1).is_some_and(|mp| mp == "/"))
        .ok_or("cannot find root entry in fstab")?
        .split_whitespace()
        .collect();
    let root_device = root_entry.first()
        .ok_or("fstab root entry has no device field")?;
    let root_options = root_entry.get(3)
        .ok_or("fstab root entry has no options field")?;

    let device = tools::resolve_fstab_device(root_device)?;

    // Mount top-level to create the new subvolume alongside root and home
    let toplevel = "/mnt/atomic-rollback-toplevel";
    fs::create_dir_all(toplevel).map_err(|e| format!("mkdir {toplevel}: {e}"))?;
    tools::mount_subvolid(&device, toplevel, 5)?;

    // 1. Snapshot root (atomic, captures /var at a consistent point)
    let root_subvol = root_subvol_name()?;
    let snap = format!("{toplevel}/{root_subvol}.var-snapshot");
    if Path::new(&snap).exists() {
        tools::run_stdout("btrfs", &["subvolume", "delete", &snap])?;
    }
    tools::btrfs_subvol_snapshot(&format!("{toplevel}/{root_subvol}"), &snap)?;
    println!("  Snapshot taken for consistent /var capture");

    // 2. Create new /var subvolume at top level.
    //    If a partial subvolume exists from a previous interrupted attempt, delete it.
    let new_var = format!("{toplevel}/var");
    if Path::new(&new_var).exists() {
        println!("  Cleaning up partial /var subvolume from previous attempt");
        tools::run_stdout("btrfs", &["subvolume", "delete", &new_var])?;
    }
    tools::run_stdout("btrfs", &["subvolume", "create", &new_var])?;
    println!("  Created /var subvolume");

    // 3. Copy from snapshot's /var to new subvolume.
    //    --reflink=auto: use reflink where possible, fall back to regular copy
    //    for NOCOW files (e.g., systemd journal files with chattr +C).
    //    Source is the frozen snapshot. No races.
    tools::run_stdout("cp", &["-a", "--reflink=auto",
        &format!("{snap}/var/."), &new_var])?;
    println!("  Reflink copied /var from snapshot");

    // 4. Clean up snapshot
    tools::run_stdout("btrfs", &["subvolume", "delete", &snap])?;

    tools::umount(toplevel)?;
    let _ = fs::remove_dir(toplevel);

    // 5. Add /var mount to fstab via RENAME_EXCHANGE
    let fstab_content = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("read fstab: {e}"))?;
    // Derive /var mount options from root. Replace subvol=<name> with subvol=var.
    // Preserve device reference format (UUID=, /dev/, LABEL=) and compression.
    let var_options = root_options.replace(&format!("subvol={root_subvol}"), "subvol=var");
    let var_line = format!("{root_device} /var btrfs {var_options} 0 0");
    let new_fstab = format!("{fstab_content}\n{var_line}\n");

    fs::write("/etc/fstab.new", &new_fstab)
        .map_err(|e| format!("write fstab.new: {e}"))?;
    swap::atomic_swap(Path::new("/etc"), "fstab", "fstab.new")?;
    println!("  fstab updated with /var mount");

    // 6. Mount the new subvolume (so the system uses it immediately)
    run_mount_fstab("/var")?;
    println!("  /var mounted from new subvolume");

    Ok(())
}

/// Step 10: Set NOCOW on /boot/grub2/ and recreate grubenv.
///
/// Postcondition: grubenv is a flat 1024-byte extent, not compressed or inline.
///
/// On Btrfs with compress=zstd, grubenv gets compressed and stored inline.
/// GRUB's Btrfs driver (loadenv.c:216) rejects encoded/sparse extents.
/// chattr +C on the directory makes new files inherit NOCOW.
/// grub2-editenv create writes to grubenv.new then rename(2) to grubenv.
/// The new inode inherits NOCOW from the parent directory.
fn step8_fix_grubenv() -> Result<(), String> {
    println!("=== Step 8: Fix grubenv for GRUB Btrfs driver ===");

    let grub2_dir = P.grub_dir;

    // Set NOCOW on directory. Idempotent: chattr +C on an already +C dir is a no-op.
    tools::run_stdout("chattr", &["+C", grub2_dir])?;

    // Recreate grubenv. grub2-editenv uses write-to-.new-then-rename,
    // so the new file inherits NOCOW from the directory.
    remove_stale(&format!("{grub2_dir}/grubenv.new"));
    tools::run_stdout("grub2-editenv", &[&format!("{grub2_dir}/grubenv"), "create"])?;

    println!("  {}/ set NOCOW, grubenv recreated", P.grub_dir);
    Ok(())
}

fn root_subvol_name() -> Result<String, String> {
    let fstab = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("read fstab: {e}"))?;
    fstab.lines()
        .filter(|l| !l.trim().starts_with('#'))
        .find(|l| l.split_whitespace().nth(1).is_some_and(|mp| mp == "/"))
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|opts| parse::extract_mount_option(opts, "subvol"))
        .map(|s| s.to_string())
        .ok_or("cannot determine root subvolume name from fstab (missing subvol= option)".into())
}

fn current_kernel_version() -> Result<String, String> {
    let uname = std::process::Command::new("uname").arg("-r").output()
        .map_err(|e| format!("uname: {e}"))?;
    Ok(String::from_utf8_lossy(&uname.stdout).trim().to_string())
}

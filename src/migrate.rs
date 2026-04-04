//! Migration: restructures Fedora's default btrfs layout for rollback.
//! setup() separates /var only. migrate() does the full 10-step boot
//! migration. Each step follows create-alongside-verify-switch.

use std::fs;
use std::path::Path;

use crate::consts::{BTRFS_TOPLEVEL_SUBVOLID, TOPLEVEL_MOUNT};
use crate::{check, platform::FEDORA as P, swap, tools};

/// Delegates to mount(1) which resolves the device from fstab.
fn run_mount_fstab(target: &str) -> Result<(), String> {
    let status = std::process::Command::new("mount").arg(target).status()
        .map_err(|e| format!("mount {target}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("mount {target} failed")) }
}

/// Separates /var and enables rollback on the stock Fedora layout.
/// No /boot changes, no ESP modification, no GRUB Btrfs dependency.
pub fn setup() -> Result<(), String> {
    let root = Path::new("/");

    check::gate("0-baseline", root, None);

    step3_set_default_subvol()?;
    check::gate("1-default-subvol", root, None);

    step10_separate_var()?;
    check::gate("2-var", root, Some("/etc/fstab.new"));

    // set-default and fstab swap are in the btrfs in-memory journal only.
    tools::sync_filesystem("/")?;

    println!("Setup complete. Snapshots and rollback are enabled.");
    println!("Install the dnf plugin for automatic pre-update snapshots.");
    Ok(())
}

/// Full 10-step boot migration. Moves /boot from ext4 to Btrfs, updates
/// ESP, fixes grubenv for btrfs, separates /var. Each step is gated by
/// the BOOTS predicate.
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

    // All 10 steps complete. Without syncfs, changes from RENAME_EXCHANGE
    // and set-default are in the btrfs in-memory journal only.
    tools::sync_filesystem("/")?;

    println!("Migration complete. All gates passed.");
    Ok(())
}

fn step1_ensure_boot_on_btrfs() -> Result<(), String> {
    println!("=== Step 1: Ensure /boot contents on Btrfs ===");

    if tools::is_mountpoint(Path::new("/boot")) {
        println!("  ext4 /boot is mounted. Copying to Btrfs");
        tools::umount("/boot/efi").ok(); // may not be mounted
        tools::umount("/boot")?;

        let old_boot = "/mnt/old-boot";
        fs::create_dir_all(old_boot)
            .map_err(|e| format!("mkdir {old_boot}: {e}"))?;
        let fstab_content = fs::read_to_string("/etc/fstab").map_err(|e| format!("read fstab: {e}"))?;
        let fstab_lines = tools::parse_fstab(&fstab_content);
        let boot_entry = tools::fstab_entries(&fstab_lines).into_iter()
            .find(|e| e.fs_file == "/boot" && e.fs_vfstype == tools::FsType::Ext4)
            .ok_or("cannot find ext4 /boot in fstab")?;

        let device = tools::resolve_fstab_device(&boot_entry.fs_spec)?;
        tools::mount_ro(&device, old_boot)?;
        tools::rsync(&format!("{old_boot}/"), "/boot/")?;
        tools::umount(old_boot)?;

        // Remount ext4 /boot and /boot/efi so BOOTS holds at the gate.
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

    let (_, fstab) = tools::root_device()?;
    let root_subvol = tools::root_subvol_name(&fstab)?;
    let root_id = tools::btrfs_subvol_id_by_name("/", &root_subvol)?;
    tools::btrfs_subvol_set_default(root_id, "/")?;
    println!("  default subvol '{root_subvol}' set to ID {root_id}");

    Ok(())
}

fn step4_switch_boot_mount() -> Result<(), String> {
    println!("=== Step 4: Switch /boot to Btrfs ===");

    if tools::is_mountpoint(Path::new("/boot")) {
        tools::umount("/boot/efi").ok(); // may not be mounted
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

    let new_content: String = tools::parse_fstab(&content).iter()
        .map(|line| match line {
            tools::FstabLine::Entry(e)
                if e.fs_file == "/boot" && e.fs_vfstype == tools::FsType::Ext4 =>
                format!("#MIGRATED: {}", e.raw),
            _ => line.raw().to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");

    fs::write("/etc/fstab.new", &new_content)
        .map_err(|e| format!("write fstab.new: {e}"))?;
    swap::rename_exchange(Path::new("/etc"), "fstab", "fstab.new")?;

    Ok(())
}

fn step6_rebuild_initramfs() -> Result<(), String> {
    println!("=== Step 6: Rebuild initramfs ===");

    let kver = current_kernel_version()?;
    let new_path = format!("/boot/initramfs-{kver}.img.new");
    remove_stale(&new_path);
    tools::dracut_rebuild(&new_path, &kver)?;
    swap::rename_exchange(
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

    // GRUB's btrfs driver is read-only. save_env fails on every boot.
    // Strip it from the generated grub.cfg.
    let content = fs::read_to_string(&new_path)
        .map_err(|e| format!("read grub.cfg.new: {e}"))?;
    let cleaned: String = content.lines()
        .filter(|line| !line.trim().starts_with("save_env"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&new_path, &cleaned)
        .map_err(|e| format!("write grub.cfg.new: {e}"))?;

    swap::rename_exchange(Path::new(P.grub_dir), "grub.cfg", "grub.cfg.new")?;

    Ok(())
}

fn step9_update_esp() -> Result<(), String> {
    println!("=== Step 9: Update ESP grub.cfg ===");
    let esp_cfg = format!("{}/grub.cfg", P.esp_dir);
    let esp_cfg_new = format!("{}/grub.cfg.new", P.esp_dir);
    remove_stale(&esp_cfg_new);

    // Parse the existing ESP grub.cfg contract.
    let existing = fs::read_to_string(&esp_cfg)
        .map_err(|e| format!("read {esp_cfg}: {e}"))?;
    let old_stub = tools::parse_esp_stub(&existing)?;

    // New UUID for the filesystem containing /boot (Btrfs after migration).
    let new_uuid = tools::BareUuid::new(
        tools::run_stdout("grub2-probe", &["--target=fs_uuid", "/boot"])?,
    );

    // Derive the new grub_dir: on Btrfs with default subvol, prefix is /boot/grub2.
    let grub_basename = Path::new(P.grub_dir).file_name()
        .and_then(|n| n.to_str()).unwrap_or("grub2");
    let new_grub_dir = format!("/boot/{grub_basename}");

    println!("  Old UUID: {}", old_stub.boot_uuid.as_str());
    println!("  New UUID: {}", new_uuid.as_str());

    // Modify contract, render from template, write.
    let new_stub = tools::EspStub {
        boot_uuid: new_uuid.clone(),
        grub_dir: new_grub_dir,
        btrfs_relative: true,
    };
    let new_cfg = tools::render_esp_stub(&new_stub);

    fs::write(&esp_cfg_new, &new_cfg)
        .map_err(|e| format!("write {esp_cfg_new}: {e}"))?;

    // Verify round-trip: parse what we just rendered.
    let verified = tools::parse_esp_stub(&new_cfg).map_err(|e| {
        let _ = fs::remove_file(&esp_cfg_new);
        format!("ESP grub.cfg.new failed to parse: {e}. Old ESP preserved.")
    })?;
    if verified.boot_uuid.as_str() != new_uuid.as_str() {
        let _ = fs::remove_file(&esp_cfg_new);
        return Err(format!(
            "ESP grub.cfg.new has UUID {} but expected {}. Old ESP preserved.",
            verified.boot_uuid.as_str(), new_uuid.as_str()));
    }

    swap::rename_exchange(Path::new(P.esp_dir), "grub.cfg", "grub.cfg.new")?;

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

    // Read root mount info from fstab. Derive /var entry from it.
    let (device, fstab) = tools::root_device()?;
    let fstab_lines = tools::parse_fstab(&fstab);
    let root_entry = tools::fstab_entries(&fstab_lines).into_iter()
        .find(|e| e.fs_file == "/")
        .ok_or("cannot find root entry in fstab")?;
    let root_device_field = root_entry.fs_spec.clone();
    let root_options = root_entry.fs_mntops.clone();

    // Mount top-level to create the new subvolume alongside root and home
    let toplevel = TOPLEVEL_MOUNT;
    fs::create_dir_all(toplevel).map_err(|e| format!("mkdir {toplevel}: {e}"))?;
    tools::mount_subvolid(&device, toplevel, BTRFS_TOPLEVEL_SUBVOLID)?;

    // 1. Snapshot root (atomic, captures /var at a consistent point)
    let root_subvol = tools::root_subvol_name(&fstab)?;
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
    // Derive /var mount options from root. Replace subvol=<name> with subvol=var.
    // Preserve device reference format (UUID=, /dev/, LABEL=) and compression.
    let var_options = root_options.replace(&format!("subvol={root_subvol}"), "subvol=var");
    let var_line = format!("{root_device_field} /var btrfs {var_options} 0 0");
    let new_fstab = format!("{fstab}\n{var_line}\n");

    fs::write("/etc/fstab.new", &new_fstab)
        .map_err(|e| format!("write fstab.new: {e}"))?;
    swap::rename_exchange(Path::new("/etc"), "fstab", "fstab.new")?;
    println!("  fstab updated with /var mount");

    // 6. Mount the new subvolume (so the system uses it immediately)
    run_mount_fstab("/var")?;
    println!("  /var mounted from new subvolume");

    Ok(())
}

/// GRUB's btrfs driver (loadenv.c:216) rejects compressed/inline extents.
/// On btrfs with compress=zstd, grubenv gets compressed by default.
/// NOCOW on the directory makes new files (including grub2-editenv's
/// write-to-.new-then-rename pattern) inherit flat extent storage.
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

fn current_kernel_version() -> Result<String, String> {
    let uname = std::process::Command::new("uname").arg("-r").output()
        .map_err(|e| format!("uname: {e}"))?;
    Ok(String::from_utf8_lossy(&uname.stdout).trim().to_string())
}

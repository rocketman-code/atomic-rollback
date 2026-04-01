//! Wrappers for external tools (btrfs-progs, blkid, findmnt, mount,
//! dracut, grub2-mkconfig, rsync) and fstab parsing helpers. Each
//! function delegates to a system tool and returns structured results.

use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::fs;

use crate::consts::{BTRFS_TOPLEVEL_SUBVOLID, PROBE_MOUNT_PREFIX, TOPLEVEL_MOUNT};

/// Flush all pending filesystem changes to disk.
/// Btrfs operations (RENAME_EXCHANGE, set-default) use btrfs_end_transaction,
/// which commits to the in-memory journal but NOT to disk. Changes are lost
/// on power failure until the next btrfs transaction commit (up to 30s).
/// syncfs forces the commit.
pub fn sync_filesystem(path: &str) -> Result<(), String> {
    let f = fs::File::open(path)
        .map_err(|e| format!("open {path} for sync: {e}"))?;
    let ret = unsafe { libc::syncfs(f.as_raw_fd()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(format!("syncfs {path}: {}", std::io::Error::last_os_error()))
    }
}

/// Runs a command and returns stdout as a trimmed string. Fails on non-zero exit.
/// Uses from_utf8_lossy: all wrapped tools (btrfs, blkid, findmnt) produce ASCII.
pub fn run_stdout(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd).args(args).output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{cmd} {}: {stderr}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Runs a command. On failure, includes stderr in the error message.
fn run_ok(cmd: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(cmd).args(args).output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{cmd} {}: {stderr}", args.join(" ")))
    }
}

// --- blkid ---

/// Returns the block device path for a filesystem UUID (e.g. "UUID" -> "/dev/sda2").
pub fn blkid_device_for_uuid(uuid: &str) -> Result<String, String> {
    run_stdout("blkid", &["--uuid", uuid])
}

/// Resolves a /dev/disk/ symlink to the real device path.
fn resolve_udev_symlink(subdir: &str, value: &str) -> Result<String, String> {
    let link = format!("/dev/disk/{subdir}/{value}");
    fs::canonicalize(&link)
        .map_err(|e| format!("{link}: {e}"))?
        .to_str()
        .ok_or_else(|| format!("{link}: non-UTF8 device path"))
        .map(|s| s.to_string())
}

/// Resolves a fstab device field to a block device path.
/// Handles all six mount(8) tag formats defined in libmount's
/// mnt_valid_tagname() (libmount/src/utils.c:47): UUID=, LABEL=,
/// PARTUUID=, PARTLABEL=, ID=, and raw /dev/ paths.
/// PARTUUID/PARTLABEL/ID resolve via /dev/disk/ symlinks
/// (udev 60-persistent-storage.rules).
/// Note: systemd fstab-generator only handles four tags (no ID=).
/// ID= in fstab works with mount(8) but not with systemd boot.
pub fn resolve_fstab_device(device: &str) -> Result<String, String> {
    if let Some(uuid) = device.strip_prefix("UUID=") {
        blkid_device_for_uuid(uuid)
    } else if let Some(label) = device.strip_prefix("LABEL=") {
        run_stdout("blkid", &["-L", label])
    } else if let Some(partuuid) = device.strip_prefix("PARTUUID=") {
        resolve_udev_symlink("by-partuuid", partuuid)
    } else if let Some(partlabel) = device.strip_prefix("PARTLABEL=") {
        resolve_udev_symlink("by-partlabel", partlabel)
    } else if let Some(id) = device.strip_prefix("ID=") {
        resolve_udev_symlink("by-id", id)
    } else {
        Ok(device.to_string())
    }
}

/// Filesystem type. The name string is defined by each kernel filesystem
/// driver (e.g. fs/btrfs/super.c: .name = "btrfs"). Variants for types
/// the tool discriminates on; everything else in Other.
#[derive(PartialEq)]
pub enum FsType {
    Btrfs,
    Ext4,
    Swap,
    Other(String),
}

/// Parses a filesystem type name string into FsType.
pub fn parse_fstype(name: &str) -> FsType {
    match name {
        "btrfs" => FsType::Btrfs,
        "ext4" => FsType::Ext4,
        "swap" => FsType::Swap,
        _ => FsType::Other(name.to_string()),
    }
}

/// Returns the filesystem type for a UUID.
pub fn blkid_fstype(uuid: &str) -> Result<FsType, String> {
    let device = blkid_device_for_uuid(uuid)?;
    let name = run_stdout("blkid", &["-s", "TYPE", "-o", "value", &device])?;
    Ok(parse_fstype(&name))
}

// --- findmnt ---

/// Find the mount point for a device that GRUB path resolution should use.
/// Accepts any device spec findmnt supports: UUID=, LABEL=, PARTUUID=,
/// PARTLABEL=, /dev/ path (findmnt(8) source specification).
/// - ext4/vfat: single mount point (e.g., /boot). Return it.
/// - Btrfs: multiple mount points (/, /home, /var). Return / specifically,
///   because that's where the root subvolume is mounted and where GRUB
///   paths resolve to Linux paths.
pub fn findmnt_target(device_spec: &str) -> Result<String, String> {
    let out = run_stdout("findmnt", &["-n", "-o", "TARGET", "-S", device_spec])?;
    let targets: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    match targets.len() {
        0 => Err(format!("{device_spec} not mounted")),
        1 => Ok(targets[0].to_string()),
        _ => {
            targets.iter()
                .find(|&&t| t == "/")
                .map(|t| t.to_string())
                .ok_or_else(|| format!("{device_spec} mounted at {targets:?} but not at /"))
        }
    }
}

/// Checks whether a path is an active mount point.
pub fn is_mountpoint(path: &Path) -> bool {
    Command::new("mountpoint").arg("-q").arg(path).status()
        .is_ok_and(|s| s.success())
}

// --- btrfs ---

/// Parsed btrfs subvolume entry. Derived from the output grammar:
///   "ID " u64 " gen " u64 " top level " u64 " path " string "\n"
/// Source: cmds/subvolume-list.c:1249 (list), cmds/subvolume.c:822 (get-default).
pub struct SubvolEntry {
    pub id: u64,
    pub path: String,
    // Parsed for format validation, not currently consumed.
    pub _generation: u64,
    pub _top_level: u64,
}

/// Parses one line of btrfs subvolume list/get-default output.
/// Uses sentinel-based extraction (not whitespace splitting) because
/// the path field can contain spaces.
fn parse_subvol_line(line: &str) -> Option<SubvolEntry> {
    let rest = line.strip_prefix("ID ")?;
    let (id_str, rest) = rest.split_once(" gen ")?;
    let (gen_str, rest) = rest.split_once(" top level ")?;
    let (top_str, path) = rest.split_once(" path ")?;
    Some(SubvolEntry {
        id: id_str.parse().ok()?,
        _generation: gen_str.parse().ok()?,
        _top_level: top_str.parse().ok()?,
        path: path.to_string(),
    })
}

/// Lists all subvolumes on the filesystem containing mount_point.
pub fn btrfs_subvol_list(mount_point: &str) -> Result<Vec<SubvolEntry>, String> {
    let out = run_stdout("btrfs", &["subvolume", "list", mount_point])?;
    out.lines()
        .map(|line| parse_subvol_line(line)
            .ok_or_else(|| format!("cannot parse subvol line: {line}")))
        .collect()
}

/// Returns the default subvolume ID for the filesystem at mount_point.
/// Two productions (cmds/subvolume.c:789-823):
///   FS_TREE: literal "ID 5 (FS_TREE)" -> BTRFS_TOPLEVEL_SUBVOLID
///   Subvol:  standard subvol line format -> parsed entry ID
pub fn btrfs_subvol_get_default(mount_point: &str) -> Result<u64, String> {
    let out = run_stdout("btrfs", &["subvolume", "get-default", mount_point])?;
    if let Some(entry) = parse_subvol_line(&out) {
        return Ok(entry.id);
    }
    if out.trim() == "ID 5 (FS_TREE)" {
        return Ok(BTRFS_TOPLEVEL_SUBVOLID);
    }
    Err(format!("cannot parse default subvol ID from: {out}"))
}

/// Sets the default subvolume for the filesystem at mount_point.
pub fn btrfs_subvol_set_default(id: u64, mount_point: &str) -> Result<(), String> {
    run_ok("btrfs", &["subvolume", "set-default", &id.to_string(), mount_point])
}

/// Creates a btrfs snapshot of src at dst.
/// Captures stdout because btrfs prints to stdout, which conflicts
/// with the libdnf5 actions plugin's pipe when called from the dnf hook.
pub fn btrfs_subvol_snapshot(src: &str, dst: &str) -> Result<(), String> {
    run_stdout("btrfs", &["subvolume", "snapshot", src, dst]).map(|_| ())
}

/// Looks up a subvolume's ID by name.
pub fn btrfs_subvol_id_by_name(mount_point: &str, name: &str) -> Result<u64, String> {
    let entries = btrfs_subvol_list(mount_point)?;
    entries.iter()
        .find(|e| e.path == name)
        .map(|e| e.id)
        .ok_or_else(|| format!("subvol '{name}' not found on {mount_point}"))
}

// --- mount/umount ---

pub fn mount_ro(device: &str, target: &str) -> Result<(), String> {
    run_ok("mount", &["-o", "ro", device, target])
}

pub fn mount_subvolid(device: &str, target: &str, subvolid: u64) -> Result<(), String> {
    run_ok("mount", &["-o", &format!("subvolid={subvolid}"), device, target])
}

pub fn umount(target: &str) -> Result<(), String> {
    run_ok("umount", &[target])
}

// --- dracut ---

pub fn dracut_rebuild(output: &str, kver: &str) -> Result<(), String> {
    run_ok("dracut", &[output, kver])
}

// --- grub ---

/// ESP grub.cfg stub contract. Derived from gen_grub_cfgstub
/// (/usr/bin/gen_grub_cfgstub, grub2-efi-x64 package).
/// The generator's inputs are boot_uuid and grub_dir.
/// btrfs_relative is added by our migration.
pub struct EspStub {
    pub boot_uuid: String,
    pub grub_dir: String,
    pub btrfs_relative: bool,
}

/// Extracts the generator's contract values from an ESP grub.cfg.
/// The script format varies across versions; we extract by content,
/// not by line position.
pub fn parse_esp_stub(content: &str) -> Result<EspStub, String> {
    // UUID is the final positional argument to GRUB's search command
    // (search_wrap.c:176,218: "NAME" is the last arg after flags).
    // gen_grub_cfgstub places ${BOOT_UUID} at end of the line.
    let boot_uuid = content.lines()
        .find(|l| l.contains("--fs-uuid"))
        .and_then(|l| l.split_whitespace().last())
        .map(|s| s.to_string())
        .ok_or("ESP grub.cfg: no search --fs-uuid line")?;

    let grub_dir = content.lines()
        .find(|l| l.contains("prefix="))
        .and_then(|l| {
            // "set prefix=($dev)/boot/grub2" -> "/boot/grub2"
            let after_paren = l.split(')').nth(1)?;
            Some(after_paren.trim().to_string())
        })
        .ok_or("ESP grub.cfg: no prefix= line")?;

    let btrfs_relative = content.lines()
        .any(|l| l.contains("btrfs_relative_path") && l.contains("yes"));

    Ok(EspStub { boot_uuid, grub_dir, btrfs_relative })
}

/// Renders an ESP grub.cfg stub from the contract values.
/// Template matches gen_grub_cfgstub (/usr/bin/gen_grub_cfgstub).
/// Variable is $dev to match the generator (--set=dev).
pub fn render_esp_stub(stub: &EspStub) -> String {
    let mut lines = Vec::new();
    if stub.btrfs_relative {
        lines.push("set btrfs_relative_path=\"yes\"".to_string());
    }
    lines.push(format!(
        "search --no-floppy --root-dev-only --fs-uuid --set=dev {}", stub.boot_uuid));
    lines.push(format!("set prefix=($dev){}", stub.grub_dir));
    lines.push("export $prefix".to_string());
    lines.push("configfile $prefix/grub.cfg".to_string());
    lines.join("\n") + "\n"
}

pub fn grub2_mkconfig(output: &str) -> Result<(), String> {
    run_ok("grub2-mkconfig", &["-o", output])
}

// --- rsync ---

pub fn rsync(src: &str, dst: &str) -> Result<(), String> {
    run_ok("rsync", &["-a", src, dst])
}

// --- fstab ---

/// Parsed fstab line. Derived from the fstab(5) grammar:
///   line ::= comment | blank | entry
///   entry ::= fs_spec ws fs_file ws fs_vfstype ws fs_mntops [ws fs_freq [ws fs_passno]]
/// Entry fields mirror struct mntent from getmntent(3).
/// Octal escapes (\040, \011, \012, \\) decoded per fstab(5).
pub enum FstabLine {
    Comment(String),
    Blank(String),
    Entry(FstabEntry),
}

impl FstabLine {
    pub fn raw(&self) -> &str {
        match self {
            FstabLine::Comment(s) | FstabLine::Blank(s) => s,
            FstabLine::Entry(e) => &e.raw,
        }
    }
}

pub struct FstabEntry {
    pub fs_spec: String,
    pub fs_file: String,
    pub fs_vfstype: FsType,
    pub fs_mntops: String,
    pub raw: String,
    // Parsed for format validation, not currently consumed.
    pub _fs_freq: i32,
    pub _fs_passno: i32,
}

/// Decodes fstab octal escapes: \040 (space), \011 (tab), \012 (newline),
/// \134 (backslash). Per getmntent(3).
fn fstab_decode(field: &str) -> String {
    let mut result = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let escape: String = chars.by_ref().take(3).collect();
            match escape.as_str() {
                "040" => result.push(' '),
                "011" => result.push('\t'),
                "012" => result.push('\n'),
                "134" => result.push('\\'),
                _ => { result.push('\\'); result.push_str(&escape); }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Parses fstab content into the full grammar: comments, blanks, and entries.
/// Fields 5-6 default to 0 per fstab(5).
pub fn parse_fstab(content: &str) -> Vec<FstabLine> {
    content.lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                FstabLine::Blank(line.to_string())
            } else if trimmed.starts_with('#') {
                FstabLine::Comment(line.to_string())
            } else {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() < 4 {
                    FstabLine::Comment(line.to_string()) // malformed, preserve as-is
                } else {
                    FstabLine::Entry(FstabEntry {
                        fs_spec: fstab_decode(fields[0]),
                        fs_file: fstab_decode(fields[1]),
                        fs_vfstype: parse_fstype(fields[2]),
                        fs_mntops: fields[3].to_string(),
                        raw: line.to_string(),
                        _fs_freq: fields.get(4).and_then(|f| f.parse().ok()).unwrap_or(0),
                        _fs_passno: fields.get(5).and_then(|f| f.parse().ok()).unwrap_or(0),
                    })
                }
            }
        })
        .collect()
}

/// Returns only the entries from parsed fstab lines.
pub fn fstab_entries(lines: &[FstabLine]) -> Vec<&FstabEntry> {
    lines.iter().filter_map(|l| match l {
        FstabLine::Entry(e) => Some(e),
        _ => None,
    }).collect()
}

/// Read /etc/fstab and return the root device path (resolved from UUID=/LABEL=/dev path).
pub fn root_device() -> Result<(String, String), String> {
    let content = fs::read_to_string("/etc/fstab")
        .map_err(|e| format!("Cannot read /etc/fstab: {e}"))?;
    let lines = parse_fstab(&content);
    let root = fstab_entries(&lines).into_iter()
        .find(|e| e.fs_file == "/")
        .ok_or("Cannot find root entry in /etc/fstab")?;
    let device = resolve_fstab_device(&root.fs_spec)?;
    Ok((device, content))
}

/// Extract the root subvolume name from fstab (the subvol= value for /).
pub fn root_subvol_name(fstab: &str) -> Result<String, String> {
    let lines = parse_fstab(fstab);
    fstab_entries(&lines).into_iter()
        .find(|e| e.fs_file == "/")
        .and_then(|e| crate::parse::extract_mount_option(&e.fs_mntops, "subvol"))
        .map(|s| s.to_string())
        .ok_or_else(|| "Cannot determine root subvolume name from /etc/fstab".into())
}

/// Mount the top-level subvolume (subvolid=5), run a closure, unmount.
/// Guarantees unmount on both success and failure.
pub fn with_toplevel<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&str) -> Result<T, String>,
{
    let toplevel = TOPLEVEL_MOUNT;
    let (device, _) = root_device()?;

    fs::create_dir_all(toplevel).map_err(|e| format!("mkdir {toplevel}: {e}"))?;
    mount_subvolid(&device, toplevel, BTRFS_TOPLEVEL_SUBVOLID)?;

    let result = f(toplevel);

    // Best-effort cleanup. A stale mount or temp dir persists until
    // reboot but does not affect the boot chain.
    let _ = umount(toplevel);
    let _ = fs::remove_dir(toplevel);

    result
}

// --- BLS entry ---

/// Parsed Boot Loader Specification entry line.
/// Derived from the BLS grammar (uapi-group.org/specifications/specs/boot_loader_specification):
///   line ::= comment | blank | field
///   field ::= key whitespace value
/// Value is everything after the first delimiter (blsuki.c:316, grub_strtok_r).
/// initrd may appear on multiple lines (BLS spec).
/// prefix stores key + original whitespace so transformers preserve formatting.
pub enum BlsLine {
    Comment(String),
    Blank(String),
    Field { key: String, value: String, prefix: String },
}

impl BlsLine {
    /// Reconstructs the original line text. Lossless for all variants.
    pub fn raw(&self) -> String {
        match self {
            BlsLine::Comment(s) | BlsLine::Blank(s) => s.clone(),
            BlsLine::Field { prefix, value, .. } => format!("{prefix}{value}"),
        }
    }
}

/// Parses BLS entry content into the full grammar.
pub fn parse_bls(content: &str) -> Vec<BlsLine> {
    content.lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                BlsLine::Blank(line.to_string())
            } else if trimmed.starts_with('#') {
                BlsLine::Comment(line.to_string())
            } else if let Some(sep_pos) = trimmed.find(|c: char| c == ' ' || c == '\t') {
                let key = trimmed[..sep_pos].to_string();
                let rest = &trimmed[sep_pos..];
                let value_start = rest.len() - rest.trim_start().len();
                let prefix_len = sep_pos + value_start;
                BlsLine::Field {
                    key,
                    value: trimmed[prefix_len..].to_string(),
                    prefix: trimmed[..prefix_len].to_string(),
                }
            } else {
                // Key with no value
                BlsLine::Field {
                    key: trimmed.to_string(),
                    value: String::new(),
                    prefix: trimmed.to_string(),
                }
            }
        })
        .collect()
}

/// Returns only the fields from parsed BLS lines.
pub fn bls_fields(lines: &[BlsLine]) -> Vec<(&str, &str)> {
    lines.iter().filter_map(|l| match l {
        BlsLine::Field { key, value, .. } => Some((key.as_str(), value.as_str())),
        _ => None,
    }).collect()
}

// --- probe mount: mount a device temporarily if not already mounted ---

/// Accepts any fstab device spec (UUID=, LABEL=, PARTUUID=, PARTLABEL=,
/// ID=, /dev/ path). Checks findmnt first; if not mounted, resolves to
/// a device path and probe-mounts read-only.
pub fn get_mount_point(device_spec: &str) -> Result<MountPoint, String> {
    if let Ok(target) = findmnt_target(device_spec) {
        if !target.is_empty() {
            return Ok(MountPoint::Existing(target));
        }
    }

    let suffix = &device_spec[device_spec.len().saturating_sub(8)..];
    let probe_dir = format!("{PROBE_MOUNT_PREFIX}{suffix}");
    fs::create_dir_all(&probe_dir).map_err(|e| format!("mkdir {probe_dir}: {e}"))?;
    let device = resolve_fstab_device(device_spec)?;
    mount_ro(&device, &probe_dir)?;
    Ok(MountPoint::Probed(probe_dir))
}

/// A filesystem mount point, either already mounted or probed temporarily.
/// Probed mounts are unmounted on drop.
pub enum MountPoint {
    /// Already mounted by the system (e.g. / or /home).
    Existing(String),
    /// Temporarily mounted by this tool for inspection.
    Probed(String),
}

impl MountPoint {
    pub fn path(&self) -> &str {
        match self {
            MountPoint::Existing(p) | MountPoint::Probed(p) => p,
        }
    }
}

impl Drop for MountPoint {
    fn drop(&mut self) {
        if let MountPoint::Probed(p) = self {
            // Best-effort. Failure means the mount persists until reboot.
            let _ = Command::new("umount").arg(p.as_str()).output();
            let _ = fs::remove_dir(p.as_str());
        }
    }
}

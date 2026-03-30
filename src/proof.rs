/// Formal model of the migration state machine.
/// No I/O, no strings, no filesystem. Pure state transitions.
/// Kani exhausts this in seconds.

/// System state: which components exist and what they point to.
#[derive(Clone, Copy, PartialEq)]
pub struct SystemState {
    // ESP grub.cfg points to this UUID
    pub esp_target_uuid: Uuid,
    // ESP grub.cfg has btrfs_relative_path="yes"
    pub esp_has_btrfs_relative: bool,
    // ESP prefix path includes /boot (true after migration, false before)
    pub esp_prefix_has_boot: bool,
    // Main grub.cfg exists on each filesystem (can be both simultaneously)
    pub grub_cfg_on_ext4: bool,
    pub grub_cfg_on_btrfs: bool,
    // BLS entry kernel path scheme
    pub bls_paths: PathScheme,
    // Kernel + initramfs exist on boot filesystem
    pub kernel_on_btrfs: bool,
    pub kernel_on_ext4: bool,
    // Symlinks at / pointing to boot/*
    pub symlinks_exist: bool,
    // fstab has ext4 /boot entry
    pub fstab_has_ext4_boot: bool,
    // ext4 /boot is actively mounted (Linux runtime state, not boot chain)
    pub ext4_boot_mounted: bool,
    // /var is a separate btrfs subvolume
    pub var_is_subvol: bool,
    // fstab has a btrfs /var mount entry
    pub fstab_has_var_mount: bool,
    // How fstab references the root device (UUID=, /dev/, LABEL=)
    pub root_device_ref: DeviceRef,
    // Root filesystem compression option
    pub root_compression: Compression,
    // /var fstab entry device ref (must match root when var mount exists)
    pub var_device_ref: DeviceRef,
    // /var fstab entry compression (must match root when var mount exists)
    pub var_compression: Compression,
    // Default btrfs subvolume ID
    pub default_subvol: SubvolId,
    // Which subvolume is named "root"
    pub root_subvol: SubvolId,
    // initramfs reflects current layout
    pub initramfs_current: bool,
    // grub.cfg reflects current layout
    pub grub_cfg_current: bool,
    // grubenv is a flat extent (NOCOW), not compressed/inline
    pub grubenv_nocow: bool,
    // Operational: artifact verified before RENAME_EXCHANGE swap.
    // Set by verify_artifact, consumed by any swap step.
    // Uniform pattern for migration steps, rollback, and kernel install.
    pub artifact_verified: bool,
    // Durability: have all pending changes been persisted to disk?
    // Derived from kernel source:
    //   btrfs_end_transaction (RENAME_EXCHANGE, set-default, symlink) = false
    //   btrfs_commit_transaction (snapshot) = true
    //   sync_filesystem() = true
    pub durable: bool,
    // Data safety: user subvolumes are never destroyed.
    // /home is always a separate subvol, never part of any swap.
    // /var is separate after step 10, never part of rollback swap.
    // After rollback, old root exists at the snapshot name (RENAME_EXCHANGE preserves both).
    // No operation in the tool deletes root, /home, or /var.
    pub home_subvol_intact: bool,
    pub var_subvol_intact: bool,
    pub old_root_preserved: bool,  // meaningful after rollback
}

#[derive(Clone, Copy, PartialEq)]
pub enum Uuid { Ext4, Btrfs }

#[derive(Clone, Copy, PartialEq)]
pub enum PathScheme {
    /// /vmlinuz-... (relative to partition root)
    /// Symlinks make this work on both ext4 and Btrfs.
    PartitionRelative,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SubvolId { Id256, Id259 }

/// How fstab references a block device.
#[derive(Clone, Copy, PartialEq)]
pub enum DeviceRef { Uuid, DevPath, Label }

/// Compression option in fstab mount options.
#[derive(Clone, Copy, PartialEq)]
pub enum Compression { Zstd, Lzo, None, Inherited }

/// GRUB_BTRFS_CONSTRAINT: GRUB's Btrfs driver is read-only.
/// save_env is a no-op on Btrfs. grubenv must be written from Linux userspace.
/// load_env works if grubenv is NOCOW (flat extent, not compressed/inline).
/// This is a permanent environmental constraint, not a state variable.
const GRUB_BTRFS_WRITE_CONSTRAINT: bool = true;

/// The BOOTS predicate: does this state represent a bootable system?
///
/// Derived from the boot chain:
///   UEFI -> shim -> GRUB -> grub.cfg -> blscfg -> BLS entry -> kernel -> initrd -> root mount
///
/// Each conjunct corresponds to a link in the chain.
/// The predicate is proven correct GIVEN GRUB_BTRFS_CONSTRAINT.
pub fn boots(s: &SystemState) -> bool {
    // ESP grub.cfg must point to a filesystem that has grub.cfg
    let esp_finds_grub_cfg = match s.esp_target_uuid {
        Uuid::Ext4 => s.grub_cfg_on_ext4,
        Uuid::Btrfs => s.grub_cfg_on_btrfs,
    };

    // GRUB must resolve BLS kernel paths to an actual kernel.
    // When ESP points to ext4: paths are partition-relative (/vmlinuz-...)
    //   → kernel must exist on ext4
    // When ESP points to btrfs: paths resolve from default subvol
    //   → if PartitionRelative (/vmlinuz-...): needs symlink at root
    //   → if SubvolRelative (/boot/vmlinuz-...): needs kernel in btrfs /boot
    let grub_finds_kernel = match s.esp_target_uuid {
        Uuid::Ext4 => {
            // GRUB reads ext4 partition. Kernel must be there.
            s.kernel_on_ext4
        }
        Uuid::Btrfs => {
            // GRUB reads Btrfs. Three requirements:
            // 1. btrfs_relative_path set (resolves from default subvol)
            // 2. default subvol matches root
            // 3. prefix path includes /boot (grub.cfg is at /boot/grub2/)
            s.esp_has_btrfs_relative && s.default_subvol == s.root_subvol
            && s.esp_prefix_has_boot
            // BLS paths are PartitionRelative (/vmlinuz-...).
            // Symlinks make them resolve on Btrfs: /vmlinuz-... → boot/vmlinuz-...
            && s.symlinks_exist && s.kernel_on_btrfs
        }
    };

    // fstab must not reference nonexistent mounts (all are Requires=, no nofail).
    // /var mount: subvolume must exist, device ref must match root.
    // Device ref mismatch (e.g., UUID= vs /dev/) risks mount failure on
    // device re-enumeration. Compression mismatch is inconsistent but not
    // a boot failure; checked by var_config_consistent, not BOOTS.
    let fstab_valid = if s.fstab_has_var_mount {
        s.var_is_subvol && s.var_device_ref == s.root_device_ref
    } else {
        true
    };

    // grubenv must be NOCOW on Btrfs, or GRUB's load_env rejects it (loadenv.c:216).
    let grubenv_valid = match s.esp_target_uuid {
        Uuid::Ext4 => true,
        Uuid::Btrfs => s.grubenv_nocow,
    };

    esp_finds_grub_cfg && grub_finds_kernel && fstab_valid && grubenv_valid
}

/// /var config consistent with root: device ref AND compression match.
/// Not part of BOOTS (compression mismatch doesn't prevent boot).
/// Proven separately by step10_produces_consistent_var_config.
pub fn var_config_consistent(s: &SystemState) -> bool {
    if s.fstab_has_var_mount {
        s.var_device_ref == s.root_device_ref
        && s.var_compression == s.root_compression
    } else {
        true
    }
}

/// The initial state: stock Fedora layout.
/// Parameterized over what varies between systems:
/// - esp_has_btrfs_relative: Cloud VM (true), bare metal (false)
/// - var_separated: Cloud VM (true), bare metal (false)
/// - root_device_ref: how fstab references root (UUID, /dev/, LABEL)
/// - root_compression: compression option on root mount
pub fn initial_state(
    esp_has_btrfs_relative: bool,
    var_separated: bool,
    root_device_ref: DeviceRef,
    root_compression: Compression,
) -> SystemState {
    SystemState {
        esp_target_uuid: Uuid::Ext4,
        esp_has_btrfs_relative,
        esp_prefix_has_boot: false,
        grub_cfg_on_ext4: true,
        grub_cfg_on_btrfs: false,
        bls_paths: PathScheme::PartitionRelative,
        kernel_on_btrfs: false,
        kernel_on_ext4: true,
        symlinks_exist: false,
        fstab_has_ext4_boot: true,
        ext4_boot_mounted: true,
        var_is_subvol: var_separated,
        fstab_has_var_mount: var_separated,
        // /var config matches root when pre-separated (installer produced it)
        root_device_ref,
        root_compression,
        var_device_ref: root_device_ref,
        var_compression: root_compression,
        default_subvol: SubvolId::Id256,
        root_subvol: SubvolId::Id256,
        initramfs_current: true,
        grub_cfg_current: true,
        grubenv_nocow: false,
        artifact_verified: false,
        durable: true, // system at rest, everything on disk
        // Data safety: subvolumes intact from boot. Fedora always has
        // /home as a separate subvol. /var may or may not be separate.
        // old_root_preserved is false initially (no rollback has happened).
        home_subvol_intact: true,
        var_subvol_intact: true,
        old_root_preserved: false,
    }
}

// --- Migration steps as pure state transitions ---

/// Step 1: Copy /boot contents to Btrfs.
/// Postcondition: kernel exists on BOTH filesystems.
pub fn step1_copy_boot(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.kernel_on_btrfs = true;
    next.durable = false; // rsync: btrfs_end_transaction
    next
}

/// Step 2: Create symlinks at / for kernel/initramfs.
pub fn step2_create_symlinks(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.symlinks_exist = true;
    next.durable = false; // symlink: btrfs_end_transaction
    next
}

/// Step 3: Set default subvolume to root.
pub fn step3_set_default_subvol(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.default_subvol = s.root_subvol;
    next.durable = false; // set-default: btrfs_end_transaction
    next
}

/// Step 4: Unmount ext4 /boot (btrfs /boot directory takes over).
/// The ext4 partition still exists; only the mount is removed.
/// GRUB reads by UUID regardless of Linux mounts, so BOOTS does not
/// depend on ext4_boot_mounted. Modeled for completeness.
pub fn step4_switch_boot(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.ext4_boot_mounted = false;
    // umount does not sync btrfs; prior btrfs changes remain in journal
    next
}

/// Step 5: Comment out ext4 /boot in fstab (RENAME_EXCHANGE).
/// Requires artifact_verified: new fstab checked before swap.
pub fn step5_update_fstab(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    next.fstab_has_ext4_boot = false;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE: btrfs_end_transaction
    Some(next)
}

/// Step 6: Rebuild initramfs for new layout (RENAME_EXCHANGE).
/// Requires artifact_verified: new initramfs checked before swap.
pub fn step6_rebuild_initramfs(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    next.initramfs_current = true;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE: btrfs_end_transaction
    Some(next)
}

/// Step 7: Regenerate grub.cfg (RENAME_EXCHANGE).
/// Requires artifact_verified: new grub.cfg checked before swap.
pub fn step7_regen_grub_cfg(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    next.grub_cfg_on_btrfs = true;
    next.grub_cfg_current = true;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE: btrfs_end_transaction
    Some(next)
}

/// Step 8: Set NOCOW on /boot/grub2/ and recreate grubenv.
/// Must happen BEFORE the ESP switches to Btrfs (step 9).
/// GRUB's Btrfs driver (loadenv.c:216) rejects compressed/inline extents.
/// chattr +C on the directory, then grub2-editenv create inherits NOCOW.
pub fn step8_fix_grubenv(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.grubenv_nocow = true;
    next.durable = false; // chattr + editenv: btrfs_end_transaction
    next
}

/// Step 9: Update ESP grub.cfg to point to Btrfs UUID (RENAME_EXCHANGE).
/// Requires artifact_verified: new ESP grub.cfg checked before swap.
pub fn step9_update_esp(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    next.esp_target_uuid = Uuid::Btrfs;
    next.esp_has_btrfs_relative = true;
    next.esp_prefix_has_boot = true;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE on vfat: not synced
    Some(next)
}

/// Step 10: Separate /var into its own subvolume (RENAME_EXCHANGE on fstab).
/// Requires artifact_verified: new fstab checked before swap.
/// Only modifies state if /var is NOT already a separate subvolume.
/// Device ref and compression derived from root, not hardcoded.
pub fn step10_separate_var(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    if !s.var_is_subvol {
        next.var_is_subvol = true;
        next.fstab_has_var_mount = true;
        next.var_device_ref = s.root_device_ref;
        next.var_compression = s.root_compression;
    }
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE: btrfs_end_transaction
    Some(next)
}

/// Kernel install on a migrated system.
/// Requires: artifact_verified == true (new BLS entry checked before swap).
/// Without verification, the BLS swap is refused.
///
/// In the implementation (kernel_hook.rs):
/// 1. kernel-install writes kernel + initramfs to /boot
/// 2. Our hook creates symlinks (/vmlinuz-{kver} -> boot/vmlinuz-{kver})
/// 3. Our hook writes new BLS entry alongside (.new)
/// 4. Our hook verifies paths resolve (verify_bls_entry)
/// 5. Our hook swaps via RENAME_EXCHANGE (this function)
///
/// Structural properties preserved: kernel_on_btrfs, symlinks_exist,
/// bls_paths remain PartitionRelative. The new kernel is another file;
/// the structural invariants are unchanged.
pub fn kernel_install(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE + symlink: btrfs_end_transaction
    Some(next)
}

/// REBOOT_SAFE: the system is bootable AND changes are on disk.
/// BOOTS alone doesn't guarantee survival across power loss.
pub fn reboot_safe(s: &SystemState) -> bool {
    boots(s) && s.durable
}

/// DATA_SAFE: user data is never lost.
/// /home and /var are separate subvolumes, untouched by any swap.
/// After rollback, the old root exists at the snapshot name.
/// The tool never deletes root, /home, or /var.
pub fn data_safe(s: &SystemState) -> bool {
    s.home_subvol_intact && s.var_subvol_intact
}

/// Persist all pending changes to disk.
/// In the implementation: syncfs() on the btrfs mount.
/// In the model: sets durable = true.
pub fn sync_filesystem(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.durable = true;
    next
}

/// Verify an artifact before RENAME_EXCHANGE.
/// In the implementation: the specific check depends on the operation.
/// Migration steps: check the .new file is valid.
/// Rollback: mount snapshot, run P2-P5 of BOOTS.
/// Kernel install: check symlinks resolve.
/// In the model: sets artifact_verified = true. Consumed by the next swap.
pub fn verify_artifact(s: &SystemState) -> SystemState {
    let mut next = *s;
    next.artifact_verified = true;
    next
}

/// Rollback: RENAME_EXCHANGE root <-> snapshot, then set-default.
/// Requires artifact_verified == true.
/// Without verification, the operation is refused.
pub fn rollback(s: &SystemState) -> Option<SystemState> {
    if !s.artifact_verified { return None; }
    let mut next = *s;
    next.root_subvol = SubvolId::Id259;
    next.default_subvol = SubvolId::Id259;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE + set-default: btrfs_end_transaction
    // RENAME_EXCHANGE preserves BOTH directory entries. The old root
    // now exists at the snapshot name. User data is accessible.
    next.old_root_preserved = true;
    // /home and /var are separate subvolumes, not part of the swap.
    // They remain intact. (These were already true and nothing sets them false.)
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_DEVICE_REFS: [DeviceRef; 3] = [DeviceRef::Uuid, DeviceRef::DevPath, DeviceRef::Label];
    const ALL_COMPRESSIONS: [Compression; 4] = [Compression::Zstd, Compression::Lzo, Compression::None, Compression::Inherited];

    /// Helper: run a full migration with verify_artifact before each swap step.
    fn full_migration(
        has_btrfs_rel: bool, var_sep: bool,
        dev: DeviceRef, comp: Compression,
    ) -> SystemState {
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        let s4 = step4_switch_boot(&step3_set_default_subvol(
            &step2_create_symlinks(&step1_copy_boot(&s0))));
        let s5 = step5_update_fstab(&verify_artifact(&s4)).unwrap();
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5)).unwrap();
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6)).unwrap();
        let s8 = step8_fix_grubenv(&s7);
        let s9 = step9_update_esp(&verify_artifact(&s8)).unwrap();
        step10_separate_var(&verify_artifact(&s9)).unwrap()
    }

    #[test]
    fn initial_state_boots() {
        assert!(boots(&initial_state(false, false, DeviceRef::Uuid, Compression::Zstd)));
        assert!(boots(&initial_state(true, true, DeviceRef::Uuid, Compression::Zstd)));
    }

    #[test]
    fn full_migration_boots_at_every_step() {
        for has_btrfs_rel in [false, true] {
        for var_sep in [false, true] {
        for dev in ALL_DEVICE_REFS {
        for comp in ALL_COMPRESSIONS {
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&s0));

        let s1 = step1_copy_boot(&s0);
        assert!(boots(&s1));
        let s2 = step2_create_symlinks(&s1);
        assert!(boots(&s2));
        let s3 = step3_set_default_subvol(&s2);
        assert!(boots(&s3));
        let s4 = step4_switch_boot(&s3);
        assert!(boots(&s4));
        let s5 = step5_update_fstab(&verify_artifact(&s4)).unwrap();
        assert!(boots(&s5));
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5)).unwrap();
        assert!(boots(&s6));
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6)).unwrap();
        assert!(boots(&s7));
        let s8 = step8_fix_grubenv(&s7);
        assert!(boots(&s8));
        let s9 = step9_update_esp(&verify_artifact(&s8)).unwrap();
        assert!(boots(&s9));
        let s10 = step10_separate_var(&verify_artifact(&s9)).unwrap();
        assert!(boots(&s10));
        }
        }
        }
        }
    }

    #[test]
    fn rollback_and_kernel_install() {
        let migrated = full_migration(false, false, DeviceRef::Uuid, Compression::Zstd);

        assert!(rollback(&migrated).is_none());
        let rolled_back = rollback(&verify_artifact(&migrated)).unwrap();
        assert!(boots(&rolled_back));
        assert!(data_safe(&rolled_back));
        assert!(rolled_back.old_root_preserved);

        assert!(kernel_install(&migrated).is_none());
        let after_install = kernel_install(&verify_artifact(&migrated)).unwrap();
        assert!(boots(&after_install));

        assert!(GRUB_BTRFS_WRITE_CONSTRAINT);
    }

    #[test]
    fn final_state_is_fully_migrated() {
        let sf = full_migration(false, false, DeviceRef::Uuid, Compression::Zstd);

        assert!(sf.esp_target_uuid == Uuid::Btrfs);
        assert!(sf.esp_has_btrfs_relative);
        assert!(sf.grub_cfg_on_btrfs);
        assert!(sf.kernel_on_btrfs);
        assert!(sf.symlinks_exist);
        assert!(!sf.fstab_has_ext4_boot);
        assert!(!sf.ext4_boot_mounted);
        assert!(sf.var_is_subvol);
        assert!(sf.fstab_has_var_mount);
        assert!(sf.grubenv_nocow);
        assert!(var_config_consistent(&sf));
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    fn any_device_ref() -> DeviceRef {
        let v: u8 = kani::any();
        kani::assume(v <= 2);
        match v { 0 => DeviceRef::Uuid, 1 => DeviceRef::DevPath, _ => DeviceRef::Label }
    }

    fn any_compression() -> Compression {
        let v: u8 = kani::any();
        kani::assume(v <= 3);
        match v { 0 => Compression::Zstd, 1 => Compression::Lzo, 2 => Compression::None, _ => Compression::Inherited }
    }

    fn full_migration(
        has_btrfs_rel: bool, var_sep: bool,
        dev: DeviceRef, comp: Compression,
    ) -> SystemState {
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        let s4 = step4_switch_boot(&step3_set_default_subvol(
            &step2_create_symlinks(&step1_copy_boot(&s0))));
        let s5 = step5_update_fstab(&verify_artifact(&s4)).unwrap();
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5)).unwrap();
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6)).unwrap();
        let s8 = step8_fix_grubenv(&s7);
        let s9 = step9_update_esp(&verify_artifact(&s8)).unwrap();
        step10_separate_var(&verify_artifact(&s9)).unwrap()
    }

    /// THEOREM 1: Every migration step preserves bootability.
    /// Swap steps require verify_artifact before execution.
    #[kani::proof]
    fn migration_preserves_bootability() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&s0));

        let s1 = step1_copy_boot(&s0);
        assert!(boots(&s1));
        let s2 = step2_create_symlinks(&s1);
        assert!(boots(&s2));
        let s3 = step3_set_default_subvol(&s2);
        assert!(boots(&s3));
        let s4 = step4_switch_boot(&s3);
        assert!(boots(&s4));
        let s5 = step5_update_fstab(&verify_artifact(&s4)).unwrap();
        assert!(boots(&s5));
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5)).unwrap();
        assert!(boots(&s6));
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6)).unwrap();
        assert!(boots(&s7));
        let s8 = step8_fix_grubenv(&s7);
        assert!(boots(&s8));
        let s9 = step9_update_esp(&verify_artifact(&s8)).unwrap();
        assert!(boots(&s9));
        let s10 = step10_separate_var(&verify_artifact(&s9)).unwrap();
        assert!(boots(&s10));
    }

    /// THEOREM 2: Step ordering is derived, not arbitrary.
    #[kani::proof]
    fn only_correct_first_step_preserves_boots() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        // Non-swap steps from S0: safe
        assert!(boots(&step1_copy_boot(&s0)));
        assert!(boots(&step2_create_symlinks(&s0)));
        assert!(boots(&step3_set_default_subvol(&s0)));

        // Swap step 7 from S0: ext4 grub.cfg untouched, ESP still points to ext4
        let s7_from_s0 = step7_regen_grub_cfg(&verify_artifact(&s0)).unwrap();
        assert!(boots(&s7_from_s0));

        // Swap step 9 from S0: ESP to Btrfs without kernel/grubenv = unbootable
        let s9_from_s0 = step9_update_esp(&verify_artifact(&s0)).unwrap();
        assert!(!boots(&s9_from_s0));
    }

    /// THEOREM 3: Rollback preserves bootability and requires verification.
    #[kani::proof]
    fn rollback_preserves_bootability() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&migrated));

        // Without verification: refused
        assert!(rollback(&migrated).is_none());

        // With verification: succeeds and preserves bootability
        let rolled_back = rollback(&verify_artifact(&migrated)).unwrap();
        assert!(boots(&rolled_back));

        // Without set-default (manual bad rollback): BOOTS fails
        let mut bad = migrated;
        bad.root_subvol = SubvolId::Id259;
        assert!(!boots(&bad));
    }

    /// THEOREM 4: Kernel install preserves bootability and requires verification.
    #[kani::proof]
    fn kernel_install_preserves_bootability() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&migrated));

        // Without verification: refused
        assert!(kernel_install(&migrated).is_none());

        // With verification: succeeds
        let after_install = kernel_install(&verify_artifact(&migrated)).unwrap();
        assert!(boots(&after_install));

        // Second install requires re-verification
        assert!(kernel_install(&after_install).is_none());
        let after_second = kernel_install(&verify_artifact(&after_install)).unwrap();
        assert!(boots(&after_second));
    }

    /// THEOREM 5: Every step is idempotent.
    /// Swap steps: verify -> step -> verify -> step = same state.
    #[kani::proof]
    fn all_steps_are_idempotent() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        let s1 = step1_copy_boot(&s0);
        assert!(step1_copy_boot(&s1) == s1);
        let s2 = step2_create_symlinks(&s1);
        assert!(step2_create_symlinks(&s2) == s2);
        let s3 = step3_set_default_subvol(&s2);
        assert!(step3_set_default_subvol(&s3) == s3);
        let s4 = step4_switch_boot(&s3);
        assert!(step4_switch_boot(&s4) == s4);

        // Swap steps: verify -> step -> verify -> step = same structural state
        let s5 = step5_update_fstab(&verify_artifact(&s4)).unwrap();
        assert!(step5_update_fstab(&verify_artifact(&s5)).unwrap() == s5);
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5)).unwrap();
        assert!(step6_rebuild_initramfs(&verify_artifact(&s6)).unwrap() == s6);
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6)).unwrap();
        assert!(step7_regen_grub_cfg(&verify_artifact(&s7)).unwrap() == s7);
        let s8 = step8_fix_grubenv(&s7);
        assert!(step8_fix_grubenv(&s8) == s8);
        let s9 = step9_update_esp(&verify_artifact(&s8)).unwrap();
        assert!(step9_update_esp(&verify_artifact(&s9)).unwrap() == s9);
        let s10 = step10_separate_var(&verify_artifact(&s9)).unwrap();
        assert!(step10_separate_var(&verify_artifact(&s10)).unwrap() == s10);

        // Kernel install
        let sk = kernel_install(&verify_artifact(&s10)).unwrap();
        assert!(kernel_install(&verify_artifact(&sk)).unwrap() == sk);
    }

    /// THEOREM 6: System correct under GRUB Btrfs write constraint.
    #[kani::proof]
    fn system_correct_under_grub_btrfs_constraint() {
        assert!(GRUB_BTRFS_WRITE_CONSTRAINT);

        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&migrated));

        let rolled_back = rollback(&verify_artifact(&migrated)).unwrap();
        assert!(boots(&rolled_back));

        let after_install = kernel_install(&verify_artifact(&migrated)).unwrap();
        assert!(boots(&after_install));
    }

    /// THEOREM 7: No RENAME_EXCHANGE anywhere without prior verification.
    /// verify_artifact is the ONLY path to artifact_verified == true.
    /// No initial state, migration step, rollback, or kernel install grants it.
    /// Every swap operation consumes it.
    #[kani::proof]
    fn all_swaps_require_verification() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        // Initial state: not verified, all swap steps refused
        assert!(!s0.artifact_verified);
        assert!(step5_update_fstab(&s0).is_none());
        assert!(step6_rebuild_initramfs(&s0).is_none());
        assert!(step7_regen_grub_cfg(&s0).is_none());
        assert!(step9_update_esp(&s0).is_none());
        assert!(step10_separate_var(&s0).is_none());
        assert!(rollback(&s0).is_none());
        assert!(kernel_install(&s0).is_none());

        // After migration: artifact_verified consumed by last step
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(has_btrfs_rel, var_sep, dev, comp);
        assert!(!migrated.artifact_verified);
        assert!(rollback(&migrated).is_none());
        assert!(kernel_install(&migrated).is_none());

        // verify_artifact grants it; operation consumes it
        let v = verify_artifact(&migrated);
        assert!(v.artifact_verified);
        let rolled = rollback(&v).unwrap();
        assert!(!rolled.artifact_verified);
        assert!(rollback(&rolled).is_none());
    }

    /// THEOREM 8: Creation failure preserves bootability.
    /// If creation fails, verify_artifact is never called,
    /// the swap step returns None, state is unchanged.
    #[kani::proof]
    fn creation_failure_preserves_bootability() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        let s1 = step1_copy_boot(&s0);
        let s2 = step2_create_symlinks(&s1);
        let s3 = step3_set_default_subvol(&s2);
        let s4 = step4_switch_boot(&s3);

        // If creation fails at any point, no verify_artifact, no swap.
        // State unchanged. Still bootable.
        assert!(boots(&s0));
        assert!(boots(&s1));
        assert!(boots(&s2));
        assert!(boots(&s3));
        assert!(boots(&s4));

        // Swap steps without verify_artifact: refused, state unchanged
        assert!(step5_update_fstab(&s4).is_none());
        assert!(boots(&s4));

        let s5 = step5_update_fstab(&verify_artifact(&s4)).unwrap();
        assert!(step6_rebuild_initramfs(&s5).is_none());
        assert!(boots(&s5));

        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5)).unwrap();
        assert!(step7_regen_grub_cfg(&s6).is_none());
        assert!(boots(&s6));

        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6)).unwrap();
        assert!(boots(&s7));
    }

    /// THEOREM 10: All exit points are reboot-safe.
    /// BOOTS(S) is necessary but not sufficient. REBOOT_SAFE(S) requires durability.
    /// WITHOUT sync_filesystem, this theorem FAILS; the proof tells us where
    /// the implementation needs sync.
    #[kani::proof]
    fn all_exit_points_are_reboot_safe() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        // After migration: sync then reboot
        let migrated = sync_filesystem(&full_migration(has_btrfs_rel, var_sep, dev, comp));
        assert!(reboot_safe(&migrated));

        // After rollback: sync then reboot
        let rolled_back = sync_filesystem(&rollback(&verify_artifact(&migrated)).unwrap());
        assert!(reboot_safe(&rolled_back));

        // After kernel install: sync then reboot
        let installed = sync_filesystem(&kernel_install(&verify_artifact(&migrated)).unwrap());
        assert!(reboot_safe(&installed));
    }

    /// THEOREM 9: step10 produces /var config consistent with root.
    /// Device ref and compression are derived from root, not hardcoded.
    /// For ALL device ref formats and ALL compression options:
    /// step10's /var entry matches root.
    #[kani::proof]
    fn step10_produces_consistent_var_config() {
        let has_btrfs_rel: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        // Bare metal: /var not separated. step10 creates it.
        let migrated = full_migration(has_btrfs_rel, false, dev, comp);
        assert!(migrated.fstab_has_var_mount);
        assert!(migrated.var_device_ref == migrated.root_device_ref);
        assert!(migrated.var_compression == migrated.root_compression);
        assert!(var_config_consistent(&migrated));

        // Cloud VM: /var already separated. step10 preserves it.
        let migrated_vm = full_migration(has_btrfs_rel, true, dev, comp);
        assert!(migrated_vm.fstab_has_var_mount);
        assert!(var_config_consistent(&migrated_vm));
    }

    /// THEOREM 11: data_safe holds after every operation.
    /// /home and /var are never modified by any operation.
    /// After rollback, the old root is preserved (RENAME_EXCHANGE axiom).
    /// The tool never deletes root, /home, or /var subvolumes.
    #[kani::proof]
    fn data_safe_across_all_operations() {
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        assert!(data_safe(&s0)); // /home and /var intact from boot

        // After full migration: data still safe
        let migrated = full_migration(has_btrfs_rel, var_sep, dev, comp);
        assert!(data_safe(&migrated));

        // After rollback: data safe AND old root preserved
        let rolled_back = rollback(&verify_artifact(&migrated)).unwrap();
        assert!(data_safe(&rolled_back));
        assert!(rolled_back.old_root_preserved);

        // After kernel install: data still safe
        let installed = kernel_install(&verify_artifact(&migrated)).unwrap();
        assert!(data_safe(&installed));
    }

    /// THEOREM 12: setup (root-only, no /boot changes) preserves
    /// bootability, is reboot-safe after sync, and data-safe.
    /// Setup mode: step3 (set-default) + step10 (/var separation). No /boot,
    /// no ESP, no grubenv, no kernel-install hook.
    #[kani::proof]
    fn setup_is_safe() {
        let has_btrfs_rel: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        // Light mode only applies to unseparated /var (bare metal).
        // /boot stays on ext4. ESP untouched.
        let s0 = initial_state(has_btrfs_rel, false, dev, comp);
        assert!(boots(&s0));

        // Step 3: set default subvol to root
        let s3 = step3_set_default_subvol(&s0);
        assert!(boots(&s3));

        // Step 10: separate /var (verify before swap)
        let s10 = step10_separate_var(&verify_artifact(&s3)).unwrap();
        assert!(boots(&s10));

        // Sync and verify reboot-safe
        let synced = sync_filesystem(&s10);
        assert!(reboot_safe(&synced));
        assert!(data_safe(&synced));

        // Rollback works on light-migrated system
        let rolled_back = rollback(&verify_artifact(&synced)).unwrap();
        assert!(boots(&rolled_back));
        assert!(data_safe(&rolled_back));
        assert!(rolled_back.old_root_preserved);
    }
}

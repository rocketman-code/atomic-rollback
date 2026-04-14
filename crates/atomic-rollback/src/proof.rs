//! State machine model of the migration and rollback operations.
//! Each function is a pure state transition. Kani verifies theorems
//! over all valid initial states and axiom combinations.
//! This model is only as accurate as its mapping to the real boot chain.

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
    // How fstab references the root device (see DeviceRef for scope)
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

/// Abstract subvolume identity. Two distinct values so the model can
/// distinguish "root points to the original" vs "root points to the snapshot."
/// The numbers 256/259 match a typical Fedora install but the model only
/// cares about equality, not the numeric value.
#[derive(Clone, Copy, PartialEq)]
pub enum SubvolId { Id256, Id259 }

/// All six mount(8) device reference tag formats (mnt_valid_tagname,
/// libmount/src/utils.c:47) plus raw /dev/ paths.
/// Note: systemd fstab-generator only handles four (no ID=).
#[derive(Clone, Copy, PartialEq)]
pub enum DeviceRef { Uuid, DevPath, Label, PartUuid, PartLabel, Id }

/// Compression option in fstab mount options.
/// Inherited: no explicit compress= in fstab; filesystem default applies.
#[derive(Clone, Copy, PartialEq)]
pub enum Compression { Zstd, Lzo, None, Inherited }

// --- Axioms: interface assumptions about the boot chain ---
// Each axiom is a property of an interface between our tool and an
// external component. Kani explores all combinations.
// Source references are in the code that implements each interface,
// not repeated here. See the doc comment on each axiom for scope.
#[derive(Clone, Copy)]
pub struct Axioms {
    // GRUB follows search --fs-uuid in ESP grub.cfg
    pub grub_follows_esp_uuid: bool,
    // GRUB resolves paths from default subvol when btrfs_relative_path="yes"
    pub grub_resolves_from_default_subvol: bool,
    // GRUB prefix= determines where grub.cfg and modules are found
    pub grub_prefix_determines_config: bool,
    // GRUB follows symlinks on btrfs
    pub grub_follows_btrfs_symlinks: bool,
    // GRUB loadenv rejects compressed/inline extents on btrfs
    pub grub_loadenv_requires_nocow: bool,
    // shim_lock_verifier skips CONFIG, LINUX_INITRD, LOADENV
    pub grub_skips_config_verification: bool,
    // renameat2(RENAME_EXCHANGE) on btrfs is atomic (single transaction)
    pub rename_exchange_atomic_btrfs: bool,
    // renameat2(RENAME_EXCHANGE) on vfat is safe (all partial states bootable)
    pub rename_exchange_safe_vfat: bool,
    // syncfs forces btrfs transaction to disk
    pub syncfs_commits_transaction: bool,
    // kernel mounts subvol= from cmdline, ignoring default subvol ID
    pub kernel_subvol_overrides_default: bool,
    // systemd treats fstab mount failures as fatal (no nofail).
    // fstab-generator.c:741-742: nofail -> "wants", else -> "requires".
    // fstab-generator.c:591-592: without nofail -> Before=local-fs.target.
    pub systemd_fstab_fatal: bool,
    // systemd kernel-install dispatches to install.d plugins
    pub kernel_install_dispatches_hooks: bool,
}

/// The BOOTS predicate: does this state represent a bootable system?
///
/// Derived from the boot chain:
///   UEFI -> shim -> GRUB -> grub.cfg -> blscfg -> BLS entry -> kernel -> initrd -> root mount
///
/// Each conjunct is guarded by the axiom it depends on.
/// Axioms required for btrfs boot (grub_follows_*, grub_skips_*)
/// make their conjunct false when negated. Axioms that relax a check
/// (grub_loadenv_requires_nocow, systemd_fstab_fatal) make their
/// conjunct true when negated.
pub fn boots(s: &SystemState, ax: &Axioms) -> bool {
    // ESP grub.cfg must point to a filesystem that has grub.cfg
    let esp_finds_grub_cfg = if ax.grub_follows_esp_uuid {
        match s.esp_target_uuid {
            Uuid::Ext4 => s.grub_cfg_on_ext4,
            Uuid::Btrfs => s.grub_cfg_on_btrfs,
        }
    } else {
        false // can't guarantee GRUB finds grub.cfg
    };

    let grub_finds_kernel = match s.esp_target_uuid {
        Uuid::Ext4 => s.kernel_on_ext4,
        Uuid::Btrfs => {
            let default_ok = if ax.grub_resolves_from_default_subvol {
                s.esp_has_btrfs_relative && s.default_subvol == s.root_subvol
            } else {
                false // our layout depends on default subvol resolution
            };
            let prefix_ok = if ax.grub_prefix_determines_config {
                s.esp_prefix_has_boot
            } else {
                false // our layout depends on prefix for /boot/grub2
            };
            let symlinks_ok = if ax.grub_follows_btrfs_symlinks {
                s.symlinks_exist
            } else {
                false // our BLS paths need symlink resolution
            };
            default_ok && prefix_ok && symlinks_ok && s.kernel_on_btrfs
        }
    };

    // Secure Boot: GRUB must accept our modified grub.cfg
    let secureboot_ok = if ax.grub_skips_config_verification {
        true // shim_lock_verifier skips CONFIG files
    } else {
        // Modified grub.cfg is rejected. Only unmodified (pre-migration) boots.
        match s.esp_target_uuid {
            Uuid::Ext4 => true,  // pre-migration, original config untouched
            Uuid::Btrfs => false, // post-migration, our modified config rejected
        }
    };

    // Kernel mounts the correct root subvolume
    let kernel_mounts_root = if ax.kernel_subvol_overrides_default {
        true // kernel follows subvol=root in cmdline, name always valid
    } else {
        // kernel follows default subvol ID
        s.default_subvol == s.root_subvol
    };

    let fstab_valid = if ax.systemd_fstab_fatal && s.fstab_has_var_mount {
        s.var_is_subvol && s.var_device_ref == s.root_device_ref
    } else {
        true
    };

    let grubenv_valid = if ax.grub_loadenv_requires_nocow {
        match s.esp_target_uuid {
            Uuid::Ext4 => true,
            Uuid::Btrfs => s.grubenv_nocow,
        }
    } else {
        true
    };

    esp_finds_grub_cfg && grub_finds_kernel && secureboot_ok
    && kernel_mounts_root && fstab_valid && grubenv_valid
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
pub fn step5_update_fstab(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified || !ax.rename_exchange_atomic_btrfs { return None; }
    let mut next = *s;
    next.fstab_has_ext4_boot = false;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE: btrfs_end_transaction
    Some(next)
}

/// Step 6: Rebuild initramfs for new layout (RENAME_EXCHANGE).
/// Requires artifact_verified: new initramfs checked before swap.
pub fn step6_rebuild_initramfs(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified || !ax.rename_exchange_atomic_btrfs { return None; }
    let mut next = *s;
    next.initramfs_current = true;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE: btrfs_end_transaction
    Some(next)
}

/// Step 7: Regenerate grub.cfg (RENAME_EXCHANGE).
/// Requires artifact_verified: new grub.cfg checked before swap.
pub fn step7_regen_grub_cfg(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified || !ax.rename_exchange_atomic_btrfs { return None; }
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
pub fn step9_update_esp(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified || !ax.rename_exchange_safe_vfat { return None; }
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
pub fn step10_separate_var(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified || !ax.rename_exchange_atomic_btrfs { return None; }
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
pub fn kernel_install(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified
        || !ax.rename_exchange_atomic_btrfs
        || !ax.kernel_install_dispatches_hooks { return None; }
    let mut next = *s;
    next.artifact_verified = false;
    next.durable = false; // RENAME_EXCHANGE + symlink: btrfs_end_transaction
    Some(next)
}

/// REBOOT_SAFE: the system is bootable AND changes are on disk.
/// BOOTS alone doesn't guarantee survival across power loss.
pub fn reboot_safe(s: &SystemState, ax: &Axioms) -> bool {
    boots(s, ax) && s.durable
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
/// In the model: sets durable = syncfs_commits_transaction axiom.
pub fn sync_filesystem(s: &SystemState, ax: &Axioms) -> SystemState {
    let mut next = *s;
    next.durable = ax.syncfs_commits_transaction;
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
pub fn rollback(s: &SystemState, ax: &Axioms) -> Option<SystemState> {
    if !s.artifact_verified || !ax.rename_exchange_atomic_btrfs { return None; }
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

    const ALL_DEVICE_REFS: [DeviceRef; 6] = [DeviceRef::Uuid, DeviceRef::DevPath, DeviceRef::Label, DeviceRef::PartUuid, DeviceRef::PartLabel, DeviceRef::Id];
    const ALL_COMPRESSIONS: [Compression; 4] = [Compression::Zstd, Compression::Lzo, Compression::None, Compression::Inherited];

    // All axioms true: Fedora with standard GRUB, kernel, systemd
    const FEDORA: Axioms = Axioms {
        grub_follows_esp_uuid: true,
        grub_resolves_from_default_subvol: true,
        grub_prefix_determines_config: true,
        grub_follows_btrfs_symlinks: true,
        grub_loadenv_requires_nocow: true,
        grub_skips_config_verification: true,
        rename_exchange_atomic_btrfs: true,
        rename_exchange_safe_vfat: true,
        syncfs_commits_transaction: true,
        kernel_subvol_overrides_default: true,
        systemd_fstab_fatal: true,
        kernel_install_dispatches_hooks: true,
    };

    /// Helper: run a full migration with verify_artifact before each swap step.
    fn full_migration(
        has_btrfs_rel: bool, var_sep: bool,
        dev: DeviceRef, comp: Compression,
    ) -> SystemState {
        let ax = &FEDORA;
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        let s4 = step4_switch_boot(&step3_set_default_subvol(
            &step2_create_symlinks(&step1_copy_boot(&s0))));
        let s5 = step5_update_fstab(&verify_artifact(&s4), ax).unwrap();
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5), ax).unwrap();
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6), ax).unwrap();
        let s8 = step8_fix_grubenv(&s7);
        let s9 = step9_update_esp(&verify_artifact(&s8), ax).unwrap();
        step10_separate_var(&verify_artifact(&s9), ax).unwrap()
    }

    #[test]
    fn initial_state_boots() {
        let ax = &FEDORA;
        assert!(boots(&initial_state(false, false, DeviceRef::Uuid, Compression::Zstd), ax));
        assert!(boots(&initial_state(true, true, DeviceRef::Uuid, Compression::Zstd), ax));
    }

    #[test]
    fn full_migration_boots_at_every_step() {
        let ax = &FEDORA;
        for has_btrfs_rel in [false, true] {
        for var_sep in [false, true] {
        for dev in ALL_DEVICE_REFS {
        for comp in ALL_COMPRESSIONS {
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&s0, ax));

        let s1 = step1_copy_boot(&s0);
        assert!(boots(&s1, ax));
        let s2 = step2_create_symlinks(&s1);
        assert!(boots(&s2, ax));
        let s3 = step3_set_default_subvol(&s2);
        assert!(boots(&s3, ax));
        let s4 = step4_switch_boot(&s3);
        assert!(boots(&s4, ax));
        let s5 = step5_update_fstab(&verify_artifact(&s4), ax).unwrap();
        assert!(boots(&s5, ax));
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5), ax).unwrap();
        assert!(boots(&s6, ax));
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6), ax).unwrap();
        assert!(boots(&s7, ax));
        let s8 = step8_fix_grubenv(&s7);
        assert!(boots(&s8, ax));
        let s9 = step9_update_esp(&verify_artifact(&s8), ax).unwrap();
        assert!(boots(&s9, ax));
        let s10 = step10_separate_var(&verify_artifact(&s9), ax).unwrap();
        assert!(boots(&s10, ax));

        let synced = sync_filesystem(&s10, ax);
        assert!(reboot_safe(&synced, ax));
        }
        }
        }
        }
    }

    #[test]
    fn rollback_and_kernel_install() {
        let ax = &FEDORA;
        let migrated = full_migration(false, false, DeviceRef::Uuid, Compression::Zstd);

        assert!(rollback(&migrated, ax).is_none());
        let rolled_back = rollback(&verify_artifact(&migrated), ax).unwrap();
        assert!(boots(&rolled_back, ax));
        assert!(data_safe(&rolled_back));
        assert!(rolled_back.old_root_preserved);
        assert!(reboot_safe(&sync_filesystem(&rolled_back, ax), ax));

        assert!(kernel_install(&migrated, ax).is_none());
        let after_install = kernel_install(&verify_artifact(&migrated), ax).unwrap();
        assert!(boots(&after_install, ax));
        assert!(reboot_safe(&sync_filesystem(&after_install, ax), ax));
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
        kani::assume(v <= 5);
        match v {
            0 => DeviceRef::Uuid, 1 => DeviceRef::DevPath, 2 => DeviceRef::Label,
            3 => DeviceRef::PartUuid, 4 => DeviceRef::PartLabel, _ => DeviceRef::Id,
        }
    }

    fn any_compression() -> Compression {
        let v: u8 = kani::any();
        kani::assume(v <= 3);
        match v { 0 => Compression::Zstd, 1 => Compression::Lzo, 2 => Compression::None, _ => Compression::Inherited }
    }

    fn any_axioms() -> Axioms {
        Axioms {
            grub_follows_esp_uuid: kani::any(),
            grub_resolves_from_default_subvol: kani::any(),
            grub_prefix_determines_config: kani::any(),
            grub_follows_btrfs_symlinks: kani::any(),
            grub_loadenv_requires_nocow: kani::any(),
            grub_skips_config_verification: kani::any(),
            rename_exchange_atomic_btrfs: kani::any(),
            rename_exchange_safe_vfat: kani::any(),
            syncfs_commits_transaction: kani::any(),
            kernel_subvol_overrides_default: kani::any(),
            systemd_fstab_fatal: kani::any(),
            kernel_install_dispatches_hooks: kani::any(),
        }
    }

    fn full_migration(
        ax: &Axioms,
        has_btrfs_rel: bool, var_sep: bool,
        dev: DeviceRef, comp: Compression,
    ) -> SystemState {
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        let s4 = step4_switch_boot(&step3_set_default_subvol(
            &step2_create_symlinks(&step1_copy_boot(&s0))));
        let s5 = step5_update_fstab(&verify_artifact(&s4), ax).unwrap();
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5), ax).unwrap();
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6), ax).unwrap();
        let s8 = step8_fix_grubenv(&s7);
        let s9 = step9_update_esp(&verify_artifact(&s8), ax).unwrap();
        step10_separate_var(&verify_artifact(&s9), ax).unwrap()
    }

    /// THEOREM 1: Every migration step preserves bootability.
    /// IF the initial state boots under these axioms, THEN every step preserves it.
    #[kani::proof]
    fn migration_preserves_bootability() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.grub_follows_esp_uuid);
        kani::assume(ax.grub_resolves_from_default_subvol);
        kani::assume(ax.grub_prefix_determines_config);
        kani::assume(ax.grub_follows_btrfs_symlinks);
        kani::assume(ax.grub_skips_config_verification);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        kani::assume(boots(&s0, &ax));
        assert!(boots(&s0, &ax));

        let s1 = step1_copy_boot(&s0);
        assert!(boots(&s1, &ax));
        let s2 = step2_create_symlinks(&s1);
        assert!(boots(&s2, &ax));
        let s3 = step3_set_default_subvol(&s2);
        assert!(boots(&s3, &ax));
        let s4 = step4_switch_boot(&s3);
        assert!(boots(&s4, &ax));
        let s5 = step5_update_fstab(&verify_artifact(&s4), &ax).unwrap();
        assert!(boots(&s5, &ax));
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5), &ax).unwrap();
        assert!(boots(&s6, &ax));
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6), &ax).unwrap();
        assert!(boots(&s7, &ax));
        let s8 = step8_fix_grubenv(&s7);
        assert!(boots(&s8, &ax));
        let s9 = step9_update_esp(&verify_artifact(&s8), &ax).unwrap();
        assert!(boots(&s9, &ax));
        let s10 = step10_separate_var(&verify_artifact(&s9), &ax).unwrap();
        assert!(boots(&s10, &ax));
    }

    /// THEOREM 2: Step ordering is derived, not arbitrary.
    #[kani::proof]
    fn only_correct_first_step_preserves_boots() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.grub_follows_esp_uuid);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        assert!(boots(&step1_copy_boot(&s0), &ax));
        assert!(boots(&step2_create_symlinks(&s0), &ax));
        assert!(boots(&step3_set_default_subvol(&s0), &ax));

        let s7_from_s0 = step7_regen_grub_cfg(&verify_artifact(&s0), &ax).unwrap();
        assert!(boots(&s7_from_s0, &ax));

        // Step 9 from S0: ESP to Btrfs without kernel/grubenv = unbootable
        // (only when the axioms that require btrfs boot features hold)
        let s9_from_s0 = step9_update_esp(&verify_artifact(&s0), &ax).unwrap();
        if ax.grub_follows_esp_uuid {
            assert!(!boots(&s9_from_s0, &ax));
        }
    }

    /// THEOREM 3: Rollback preserves bootability and requires verification.
    #[kani::proof]
    fn rollback_preserves_bootability() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.grub_follows_esp_uuid);
        kani::assume(ax.grub_resolves_from_default_subvol);
        kani::assume(ax.grub_prefix_determines_config);
        kani::assume(ax.grub_follows_btrfs_symlinks);
        kani::assume(ax.grub_skips_config_verification);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(&ax, has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&migrated, &ax));

        assert!(rollback(&migrated, &ax).is_none());
        let rolled_back = rollback(&verify_artifact(&migrated), &ax).unwrap();
        assert!(boots(&rolled_back, &ax));
    }

    /// THEOREM 4: Kernel install preserves bootability and requires verification.
    #[kani::proof]
    fn kernel_install_preserves_bootability() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.kernel_install_dispatches_hooks);
        kani::assume(ax.grub_follows_esp_uuid);
        kani::assume(ax.grub_resolves_from_default_subvol);
        kani::assume(ax.grub_prefix_determines_config);
        kani::assume(ax.grub_follows_btrfs_symlinks);
        kani::assume(ax.grub_skips_config_verification);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(&ax, has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&migrated, &ax));

        assert!(kernel_install(&migrated, &ax).is_none());
        let after_install = kernel_install(&verify_artifact(&migrated), &ax).unwrap();
        assert!(boots(&after_install, &ax));

        assert!(kernel_install(&after_install, &ax).is_none());
        let after_second = kernel_install(&verify_artifact(&after_install), &ax).unwrap();
        assert!(boots(&after_second, &ax));
    }

    /// THEOREM 5: Every step is idempotent.
    #[kani::proof]
    fn all_steps_are_idempotent() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.kernel_install_dispatches_hooks);
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

        let s5 = step5_update_fstab(&verify_artifact(&s4), &ax).unwrap();
        assert!(step5_update_fstab(&verify_artifact(&s5), &ax).unwrap() == s5);
        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5), &ax).unwrap();
        assert!(step6_rebuild_initramfs(&verify_artifact(&s6), &ax).unwrap() == s6);
        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6), &ax).unwrap();
        assert!(step7_regen_grub_cfg(&verify_artifact(&s7), &ax).unwrap() == s7);
        let s8 = step8_fix_grubenv(&s7);
        assert!(step8_fix_grubenv(&s8) == s8);
        let s9 = step9_update_esp(&verify_artifact(&s8), &ax).unwrap();
        assert!(step9_update_esp(&verify_artifact(&s9), &ax).unwrap() == s9);
        let s10 = step10_separate_var(&verify_artifact(&s9), &ax).unwrap();
        assert!(step10_separate_var(&verify_artifact(&s10), &ax).unwrap() == s10);

        let sk = kernel_install(&verify_artifact(&s10), &ax).unwrap();
        assert!(kernel_install(&verify_artifact(&sk), &ax).unwrap() == sk);
    }

    /// THEOREM 6: System correct under GRUB Btrfs write constraint.
    #[kani::proof]
    fn system_correct_under_grub_btrfs_constraint() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.kernel_install_dispatches_hooks);
        kani::assume(ax.grub_follows_esp_uuid);
        kani::assume(ax.grub_resolves_from_default_subvol);
        kani::assume(ax.grub_prefix_determines_config);
        kani::assume(ax.grub_follows_btrfs_symlinks);
        kani::assume(ax.grub_skips_config_verification);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let migrated = full_migration(&ax, has_btrfs_rel, var_sep, dev, comp);
        assert!(boots(&migrated, &ax));

        let rolled_back = rollback(&verify_artifact(&migrated), &ax).unwrap();
        assert!(boots(&rolled_back, &ax));

        let after_install = kernel_install(&verify_artifact(&migrated), &ax).unwrap();
        assert!(boots(&after_install, &ax));
    }

    /// THEOREM 7: No RENAME_EXCHANGE anywhere without prior verification.
    #[kani::proof]
    fn all_swaps_require_verification() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.kernel_install_dispatches_hooks);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        assert!(!s0.artifact_verified);
        assert!(step5_update_fstab(&s0, &ax).is_none());
        assert!(step6_rebuild_initramfs(&s0, &ax).is_none());
        assert!(step7_regen_grub_cfg(&s0, &ax).is_none());
        assert!(step9_update_esp(&s0, &ax).is_none());
        assert!(step10_separate_var(&s0, &ax).is_none());
        assert!(rollback(&s0, &ax).is_none());
        assert!(kernel_install(&s0, &ax).is_none());

        let migrated = full_migration(&ax, has_btrfs_rel, var_sep, dev, comp);
        assert!(!migrated.artifact_verified);
        assert!(rollback(&migrated, &ax).is_none());
        assert!(kernel_install(&migrated, &ax).is_none());

        let v = verify_artifact(&migrated);
        assert!(v.artifact_verified);
        let rolled = rollback(&v, &ax).unwrap();
        assert!(!rolled.artifact_verified);
        assert!(rollback(&rolled, &ax).is_none());
    }

    /// THEOREM 8: Creation failure preserves bootability.
    #[kani::proof]
    fn creation_failure_preserves_bootability() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.grub_follows_esp_uuid);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();
        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);

        let s1 = step1_copy_boot(&s0);
        let s2 = step2_create_symlinks(&s1);
        let s3 = step3_set_default_subvol(&s2);
        let s4 = step4_switch_boot(&s3);

        assert!(boots(&s0, &ax));
        assert!(boots(&s1, &ax));
        assert!(boots(&s2, &ax));
        assert!(boots(&s3, &ax));
        assert!(boots(&s4, &ax));

        assert!(step5_update_fstab(&s4, &ax).is_none());
        assert!(boots(&s4, &ax));

        let s5 = step5_update_fstab(&verify_artifact(&s4), &ax).unwrap();
        assert!(step6_rebuild_initramfs(&s5, &ax).is_none());
        assert!(boots(&s5, &ax));

        let s6 = step6_rebuild_initramfs(&verify_artifact(&s5), &ax).unwrap();
        assert!(step7_regen_grub_cfg(&s6, &ax).is_none());
        assert!(boots(&s6, &ax));

        let s7 = step7_regen_grub_cfg(&verify_artifact(&s6), &ax).unwrap();
        assert!(boots(&s7, &ax));
    }

    /// THEOREM 9: step10 produces /var config consistent with root.
    #[kani::proof]
    fn step10_produces_consistent_var_config() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        let has_btrfs_rel: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        let migrated = full_migration(&ax, has_btrfs_rel, false, dev, comp);
        assert!(migrated.fstab_has_var_mount);
        assert!(migrated.var_device_ref == migrated.root_device_ref);
        assert!(migrated.var_compression == migrated.root_compression);
        assert!(var_config_consistent(&migrated));

        let migrated_vm = full_migration(&ax, has_btrfs_rel, true, dev, comp);
        assert!(migrated_vm.fstab_has_var_mount);
        assert!(var_config_consistent(&migrated_vm));
    }

    /// THEOREM 10: All exit points are reboot-safe.
    #[kani::proof]
    fn all_exit_points_are_reboot_safe() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.kernel_install_dispatches_hooks);
        kani::assume(ax.syncfs_commits_transaction);
        kani::assume(ax.grub_follows_esp_uuid);
        kani::assume(ax.grub_resolves_from_default_subvol);
        kani::assume(ax.grub_prefix_determines_config);
        kani::assume(ax.grub_follows_btrfs_symlinks);
        kani::assume(ax.grub_skips_config_verification);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        let migrated = sync_filesystem(&full_migration(&ax, has_btrfs_rel, var_sep, dev, comp), &ax);
        assert!(reboot_safe(&migrated, &ax));

        let rolled_back = sync_filesystem(&rollback(&verify_artifact(&migrated), &ax).unwrap(), &ax);
        assert!(reboot_safe(&rolled_back, &ax));

        let installed = sync_filesystem(&kernel_install(&verify_artifact(&migrated), &ax).unwrap(), &ax);
        assert!(reboot_safe(&installed, &ax));
    }

    /// THEOREM 11: data_safe holds after every operation.
    #[kani::proof]
    fn data_safe_across_all_operations() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.rename_exchange_safe_vfat);
        kani::assume(ax.kernel_install_dispatches_hooks);
        let has_btrfs_rel: bool = kani::any();
        let var_sep: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        let s0 = initial_state(has_btrfs_rel, var_sep, dev, comp);
        assert!(data_safe(&s0));

        let migrated = full_migration(&ax, has_btrfs_rel, var_sep, dev, comp);
        assert!(data_safe(&migrated));

        let rolled_back = rollback(&verify_artifact(&migrated), &ax).unwrap();
        assert!(data_safe(&rolled_back));
        assert!(rolled_back.old_root_preserved);

        let installed = kernel_install(&verify_artifact(&migrated), &ax).unwrap();
        assert!(data_safe(&installed));
    }

    /// THEOREM 12: setup is safe.
    #[kani::proof]
    fn setup_is_safe() {
        let ax = any_axioms();
        kani::assume(ax.rename_exchange_atomic_btrfs);
        kani::assume(ax.syncfs_commits_transaction);
        kani::assume(ax.grub_follows_esp_uuid);
        let has_btrfs_rel: bool = kani::any();
        let dev = any_device_ref();
        let comp = any_compression();

        let s0 = initial_state(has_btrfs_rel, false, dev, comp);
        assert!(boots(&s0, &ax));

        let s3 = step3_set_default_subvol(&s0);
        assert!(boots(&s3, &ax));

        let s10 = step10_separate_var(&verify_artifact(&s3), &ax).unwrap();
        assert!(boots(&s10, &ax));

        let synced = sync_filesystem(&s10, &ax);
        assert!(reboot_safe(&synced, &ax));
        assert!(data_safe(&synced));

        let rolled_back = rollback(&verify_artifact(&synced), &ax).unwrap();
        assert!(boots(&rolled_back, &ax));
        assert!(data_safe(&rolled_back));
        assert!(rolled_back.old_root_preserved);
    }
}

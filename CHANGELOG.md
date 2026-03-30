# Changelog

All notable changes to atomic-rollback are documented here.

## [0.1.2] - 2026-03-29

Durability, portability, honest output, automatic snapshots.

### Added

- `syncfs` at every exit point: migration, rollback, kernel hook. Btrfs `RENAME_EXCHANGE` and `set-default` use `btrfs_end_transaction` (in-memory journal), not `btrfs_commit_transaction` (on-disk). Without `syncfs`, changes could be lost on power failure within 30 seconds of completion. Derived from kernel source (inode.c:8534, ioctl.c:2806).
- 10th Kani theorem (`all_exit_points_are_reboot_safe`): every exit point is both bootable AND durable.
- 9th Kani theorem (`step10_produces_consistent_var_config`): /var config matches root across all device ref formats and compression options.
- Uniform verify-before-swap for ALL `RENAME_EXCHANGE` operations (8th theorem: `all_swaps_require_verification`).
- Rollback undoes the swap if `set-default` fails, restoring original state.
- `WARN` output for partially valid boot entries (e.g., 3 of 4 kernels valid). Exit code 2 for scripts.
- `platform.rs` centralizes all distro-specific paths for future multi-distro support.
- `dnf` plugin (`plugins/atomic-rollback.actions`) auto-snapshots before every transaction via libdnf5 actions. `raise_error=1` aborts `dnf` if snapshot cannot be created.
- Snapshot command is idempotent: existing snapshot returns success.
- `tools::resolve_fstab_device` handles `UUID=`, `/dev/`, and `LABEL=` in fstab.
- Inline Verus specs via `verus!` macro (15 conditions). One file, one source of truth.

### Fixed

- Rollback gate ran AFTER the irreversible swap. Now verifies BEFORE.
- Kernel hook used `fs::rename` (destructive). Now uses `RENAME_EXCHANGE` (preserves old BLS entry).
- Kernel hook verifies symlinks resolve before BLS swap.
- Migration step 10 hardcoded `UUID=` and `compress=zstd:1`. Now derives device ref, compression, and subvol name from fstab.
- Bootcheck said `PASS` when some boot entries were broken. Now says `WARN` with details.
- `btrfs subvolume snapshot` stdout suppressed to avoid libdnf5 actions pipe collision.
- Gate failure after swap now reports where the old file is preserved.

## [0.1.1] - 2026-03-29

Initial release with formally verified migration and rollback.

### Added

- `check`, `migrate`, `rollback`, `snapshot` commands.
- 10-step gated migration: /boot to Btrfs, /var separation, ESP update, grubenv NOCOW, save_env stripping, symlinks, kernel-install hook.
- 7 Kani-verified theorems for the migration state machine.
- Bootability predicate derived from the actual Fedora boot chain.
- `RENAME_EXCHANGE` for atomic swap at every level.
- RPM spec for Fedora packaging.

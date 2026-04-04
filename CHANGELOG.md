# Changelog

All notable changes to atomic-rollback are documented here.

## [0.3.4] - 2026-04-03

### Changed

- License changed from MIT OR Apache-2.0 to GPL-3.0-only. All future versions of this project are licensed under the GNU General Public License v3.0 only. Previously published versions (0.3.3 and earlier) remain under their original license. See LICENSE for the full text.

### Removed

- `scripts/monitor-reddit.sh` (one-time utility, not part of the distributed package).

## [0.3.3] - 2026-04-03

### Fixed

- `check` failed on vanilla Fedora 43 with "Can't lookup blockdev." The root UUID extracted from BLS boot entries was passed to mount without the `UUID=` prefix, so mount received a bare UUID string instead of a valid device spec. All stock Fedora installs using `UUID=` in fstab were affected.

## [0.3.2] - 2026-04-01

### Fixed

- Subvolume names with spaces now parse correctly. The btrfs output parser used whitespace splitting which truncated paths containing spaces.
- Verification chain now handles all fstab device formats (PARTUUID=, PARTLABEL=, ID=). Previously only UUID= entries were verified; other formats silently passed without checking.
- BLS initrd validation checks all initrd lines. The verified parser previously returned only the first match; entries with multiple initrd lines had subsequent lines unchecked.
- BLS root= parameter check accepts all kernel device formats (PARTUUID=, PARTLABEL=, /dev/). Previously only root=UUID= and root=/dev/ were accepted.
- ESP grub.cfg migration renders from the generator template instead of line surgery, eliminating the double-prefix bug class by construction.

### Changed

- Internal architecture: all external tool output parsed through grammar-derived types at the boundary. Filesystem type comparisons use an enum instead of string matching.

## [0.3.1] - 2026-03-31

### Fixed

- Kernel-install hook uses full binary path (/usr/bin/atomic-rollback). The bare command was not in RPM's scriptlet PATH, causing exit 127 on kernel upgrades.
- RPM spec rewritten for COPR vendored builds. The previous spec used %cargo_build which expects Fedora-packaged crates.
- COPR Makefile builds from cloned source with correct outdir contract.

### Changed

- Installation via COPR is the only supported method. The crate was removed from crates.io (binary alone is insufficient without the hook and plugin).

## [0.3.0] - 2026-03-30

### Added

- `snapshot create [name]` subcommand: explicit snapshot creation with optional name.
- `snapshot list` subcommand: shows available snapshots, excluding system subvolumes.
- `snapshot delete <name>` subcommand: refuses fstab-referenced system subvolumes (verified in VM that btrfs-progs does not check fstab). Mounted-subvolume and default-subvolume protection delegated to kernel and btrfs-progs respectively.
- `--help` and `-h` at top level and for snapshot subcommands.

### Changed

- `snapshot <name>` replaced by `snapshot create <name>`. Bare `snapshot` (no args) still creates with the default name. Unrecognized snapshot subcommands are now rejected instead of silently treated as snapshot names.

### Fixed

- Migration step 1 now handles all fstab device reference formats (UUID=, LABEL=, /dev/ paths). Previously only UUID= was supported.

## [0.2.0] - 2026-03-29

### Added

- `setup` command: separates /var and enables root snapshots and rollback without touching /boot or the ESP. Works on stock Fedora partition layout. No GRUB Btrfs dependency. Closes #1.
- 12th Kani theorem (`setup_is_safe`): setup preserves bootability, is reboot-safe after sync, data-safe, and rollback works on the setup'd system.

## [0.1.4] - 2026-03-29

### Added

- 11th Kani theorem (`data_safe_across_all_operations`): /home and /var are never modified by any operation (separate subvolumes, not part of any swap). After rollback, the old root is preserved at the snapshot name. No operation in the tool destroys user data.

### Fixed

- ESP grub.cfg substitution now verifies all three model properties (UUID, `btrfs_relative_path`, prefix path) on the output BEFORE the swap. Previously only the UUID was checked. If any property is missing, the swap is refused and the old ESP is preserved. Closes the gap that allowed prefix doubling to reach the swap during development.

## [0.1.3] - 2026-03-29

### Added

- `syncfs` at every exit point (migration, rollback, kernel hook). Btrfs `RENAME_EXCHANGE` and `set-default` use `btrfs_end_transaction` (in-memory journal only). Without `syncfs`, changes could be lost on power failure within 30 seconds of completion. Derived from kernel source (inode.c:8534, ioctl.c:2806).
- 10th Kani theorem (`all_exit_points_are_reboot_safe`): every exit point is both bootable AND durable. The model tracks `durable: bool` and requires `sync_filesystem` before `reboot_safe` can hold.

## [0.1.1] - 2026-03-29

Initial release.

### Added

- `check`, `migrate`, `rollback`, `snapshot` commands.
- 10-step gated migration: /boot to Btrfs, /var separation, ESP update, grubenv NOCOW, save_env stripping, symlinks, kernel-install hook.
- 9 Kani-verified theorems: migration preserves bootability, rollback preserves bootability, step ordering, kernel installs, idempotency, GRUB Btrfs constraint, creation failure safety, all swaps require verification, /var config consistency.
- 15 Verus-verified parser conditions inline via `verus!` macro.
- Verify-before-swap for all `RENAME_EXCHANGE` operations (rollback, migration, kernel hook).
- Rollback undoes swap if `set-default` fails.
- `WARN` output for partially valid boot entries (exit code 2).
- `platform.rs` centralizes distro-specific paths.
- `dnf` plugin for automatic pre-transaction snapshots via libdnf5 actions.
- Idempotent snapshot command (existing snapshot returns success).
- `resolve_fstab_device` handles `UUID=`, `/dev/`, `LABEL=`.
- All system-specific values (device ref, compression, subvol name) derived from fstab.
- Bootability predicate derived from the actual Fedora boot chain.
- RPM spec with kernel-install hook and dnf plugin.

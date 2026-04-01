# atomic-rollback

Atomic system rollback for Fedora via Btrfs subvolume swap.

If a bad update breaks your system, one command restores the previous state. The system is either in the old state or the new state, never in between.

## Requirements

- Fedora 43 or later
- Btrfs root filesystem (Fedora's default since Fedora 33)
- UEFI boot
- Traditional Fedora desktop (Workstation, KDE, etc.), not Silverblue, Kinoite, or other atomic desktops (they have their own rollback mechanism)
- Secure Boot compatible. No signed binaries are modified. All modified files (grub.cfg, initramfs, grubenv) are in GRUB's skip-verification list.

Running outside these requirements is running outside what the tool has been verified against. `atomic-rollback check` will tell you if your system is compatible.

## Install

```
sudo dnf copr enable rocketman-code/atomic-rollback
sudo dnf install atomic-rollback
```

## Quick start

```
# Quick setup (separates /var, enables snapshots and rollback)
sudo atomic-rollback setup

# Or full boot migration (also rolls back kernels automatically)
sudo atomic-rollback migrate
sudo reboot

# That's it. Every dnf update now automatically snapshots first.
# If an update breaks something:
sudo atomic-rollback rollback
sudo reboot
```

With the dnf plugin installed, snapshots are automatic. Manual snapshots are still available for non-dnf changes: `sudo atomic-rollback snapshot create [name]`.

## Commands

`sudo atomic-rollback check` verifies the system is bootable. Reports what failed, why it matters, and what to do about it.

`sudo atomic-rollback setup` separates /var and enables root snapshots and rollback. Works on the stock Fedora partition layout. No /boot changes, no ESP modification, no GRUB Btrfs dependency. After a rollback, the user selects the correct kernel from the GRUB menu if needed.

`sudo atomic-rollback migrate` full boot migration. Moves /boot from ext4 to Btrfs so kernels are included in snapshots. After rollback, the correct kernel boots automatically. Every step verifies the system remains bootable before proceeding. If any step fails, the system is unchanged.

`sudo atomic-rollback snapshot` creates a snapshot of the current system state with the default name `root.pre-update`. Automatic via the dnf plugin; manual use for non-dnf changes. Idempotent: if the snapshot already exists, the command succeeds (existing protection is in place).

`sudo atomic-rollback snapshot create [name]` creates a snapshot with an optional name. Defaults to `root.pre-update` if no name is given.

`sudo atomic-rollback snapshot list` shows available snapshots. System subvolumes (root, home, var) are excluded.

`sudo atomic-rollback snapshot delete <name>` deletes a snapshot. Refuses subvolumes referenced by fstab (system subvolumes). Mounted and default subvolume protection is provided by the kernel and btrfs-progs.

`sudo atomic-rollback rollback [name]` restores the system to a snapshot. Defaults to `root.pre-update`. Reboot after rollback. The previous (broken) state is preserved and can be inspected or deleted.

## What the migration changes

Each change is necessary for atomic rollback to work.

/boot moves from ext4 to Btrfs. The kernel and initramfs must be inside the Btrfs snapshot to be included in rollback. Previously they were on a separate ext4 partition outside the snapshot scope.

/var becomes a separate Btrfs subvolume. Without this, rolling back root would also roll back /var, which contains databases, logs, and container state. Separating /var means rollback affects the OS but not application data.

The Btrfs default subvolume is set to match the root subvolume. GRUB resolves file paths from the default subvolume. This must match what Linux mounts as /.

Symlinks are created at / for each kernel. `/vmlinuz-6.x` points to `boot/vmlinuz-6.x`. This allows GRUB to find kernels using the same path regardless of the boot layout.

The ESP grub.cfg is updated. The UUID changes from the ext4 partition to the Btrfs partition. `btrfs_relative_path` is added. The stub is regenerated from the same template as `gen_grub_cfgstub` (the tool that created the original).

The /boot/grub2 directory is set to NOCOW (chattr +C) and grubenv is recreated. GRUB's Btrfs driver cannot read compressed or inline extents. On Btrfs with zstd compression, grubenv gets compressed by default. NOCOW on the directory ensures all files created there (including by grub2-editenv) are stored as flat extents GRUB can read.

save_env is stripped from grub.cfg. GRUB's Btrfs driver is read-only. save_env requires write access. Since writing is impossible, save_env is a guaranteed failure. Removing it prevents an error message on every boot.

A kernel-install hook is installed. When new kernels are installed via dnf, the hook creates symlinks and ensures boot entry paths are correct. This keeps the system bootable across kernel updates.

## Automatic snapshots

A libdnf5 actions plugin snapshots the root subvolume before every dnf transaction. If the snapshot cannot be created, dnf aborts. If a snapshot already exists, dnf proceeds with the existing protection. The plugin is installed automatically by the RPM.

## Guarantees

Every intermediate state during migration is bootable. If power is lost at any point, the system boots.

The migration can be interrupted and resumed. Every step is idempotent.

Rollback is a single atomic operation. It either fully completes or does not happen. The snapshot is verified bootable before the swap; if verification fails, the swap is refused.

No RENAME_EXCHANGE anywhere in the tool proceeds without prior artifact verification.

/home and /var are not affected by rollback.

The old ext4 /boot partition is not deleted. It remains as a fallback.

## Limitations

GRUB's Btrfs driver is read-only. This means boot_success tracking does not work (GRUB cannot write to grubenv on Btrfs). The GRUB menu may show briefly on every boot (approximately one second) because GRUB cannot persist that the previous boot succeeded. This does not affect boot correctness or rollback functionality.

Rollback restores the previous state. It does not diagnose or fix the problem that caused the rollback. If the problem is in /var or /home (which are not rolled back), it will persist.

LUKS-encrypted /boot is not supported. GRUB can only decrypt LUKS version 1, and the interaction with Btrfs /boot has not been verified.

## How it works

The migration runs ten steps. Each step creates new state alongside the old state, verifies the system remains bootable, then atomically swaps the old and new using `renameat2(RENAME_EXCHANGE)`. This is a single kernel operation that exchanges two directory entries. Either both names swap or neither does. There is no intermediate state where the system is half-migrated.

Rollback uses the same mechanism. The root subvolume is exchanged with a snapshot in one `RENAME_EXCHANGE` call. The Btrfs default subvolume ID is updated to match. On reboot, the system boots into the snapshot.

The bootability check (`atomic-rollback check`) evaluates a predicate derived from tracing the actual Fedora boot chain: UEFI firmware loads shim, shim loads GRUB, GRUB parses BLS entries, the kernel mounts root per the command line, systemd mounts filesystems per fstab. Each link in this chain has a file that must exist and be correct. The check verifies every one.

## Verification

The state machine is formally verified using [Kani](https://github.com/model-checking/kani). The following theorems are machine-checked:

1. Migration preserves bootability at every step.
2. Rollback preserves bootability (and fails without updating the default subvolume).
3. Step ordering constraints hold.
4. Kernel installs on a migrated system preserve bootability.
5. All steps are idempotent (the migration can be interrupted and resumed).
6. The system is correct under the GRUB Btrfs write constraint (save_env failure does not affect boot, rollback, or kernel installs).
7. Non-atomic creation by external tools (dracut, grub2-mkconfig) is safe because the verification gate prevents the atomic swap from firing on a failed creation.
8. Every RENAME_EXCHANGE in the tool (migration, rollback, kernel install) requires prior artifact verification. No swap proceeds without it.
9. /var separation produces consistent config: device reference format and compression options match the root mount entry, for all valid initial configurations.
10. Every exit point (migration, rollback, kernel hook) is both bootable AND durable. `syncfs` forces the Btrfs transaction to disk before the user reboots. Derived from kernel source: `RENAME_EXCHANGE` and `set-default` use `btrfs_end_transaction` (in-memory only), not `btrfs_commit_transaction` (on-disk).
11. User data is never lost. /home and /var are separate subvolumes, untouched by any swap. After rollback, the old root is preserved at the snapshot name. No operation in the tool deletes root, /home, or /var.
12. Setup (root-only, no /boot changes) preserves bootability, is reboot-safe, and data-safe. Rollback works on the setup'd system.

The proofs are parameterized over hardware configurations (Cloud VM, bare metal, device reference formats, compression options) and boot chain axioms (GRUB behavior, kernel mount resolution, Secure Boot verification, transaction atomicity). Each theorem declares which axioms it requires. Kani explores all combinations symbolically. The source is in `src/proof.rs`.

The parser functions (bootability check, BLS entry parsing, mount option extraction) are formally verified inline using [Verus](https://github.com/verus-lang/verus). Every loop is proven to terminate, every array access is proven in bounds, and every returned range is proven valid. The specifications live in `src/parse.rs` alongside the code they verify, inside a `verus!` macro block. Under normal `cargo build`, the specs are erased. Under `cargo verus build`, all conditions are machine-checked.

## License

MIT OR Apache-2.0

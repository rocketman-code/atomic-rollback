# Contributing to atomic-rollback

Thanks for considering a contribution. This guide covers the essentials.

## Prerequisites

- Rust toolchain (`cargo`, `rustc`)
- Git
- For VM integration testing: [lima](https://lima-vm.io/) with a Fedora 43 template

## Build

```sh
cargo build --release
```

The workspace builds all member crates. The `atomic-rollback` binary lands at `target/release/atomic-rollback`.

## Test

```sh
cargo test --release
```

Tests run on Linux and macOS in CI. Production code uses Unix APIs only, so tests are skipped on Windows.

For VM integration testing of `atomic-rollback check` on a real Fedora environment, see `.github/workflows/ci.yml` (x86_64 lima job) for the canonical recipe.

## Commit format

[Conventional Commits](https://www.conventionalcommits.org/):

```
type(scope): lowercase imperative description
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `style`, `perf`, `ci`

Examples:

```
feat(snapshot): auto-named rolling snapshots
fix(hook): transfer kernel-install hook ownership to migrate
ci: add cargo test job on ubuntu + macos matrix
```

## Pull requests

- All changes go through PRs; no exceptions
- Branch protection requires all CI checks to pass before merge
- Rebase merge only (linear history)

PR test plans are mandatory. Unchecked task-list items block merge. Complete tests before merging; removing items is not a valid escape.

## CHANGELOG fragments

Every PR that modifies `crates/atomic-rollback/src/**` requires a Fragment in `crates/changelog-core/src/lib.rs`. Structural, not commit-type-based — the gate reads the diff directly so the requirement cannot be bypassed by relabeling commits.

See [docs/standards/changelog-fragments.md](docs/standards/changelog-fragments.md) for the full contract, including when `Status::InternalOnly` is appropriate.

## Project standards

Detailed standards:

- [CHANGELOG fragments](docs/standards/changelog-fragments.md) — how to add fragments, when they're required, when InternalOnly is appropriate, how releases consume them

# CHANGELOG Fragments

## What

`CHANGELOG.md` is a generated artifact. The source of truth is the `Fragment` enum in `crates/changelog-core/src/lib.rs`. Each variant is one atomic CHANGELOG bullet.

`cargo build` fails if `CHANGELOG.md` is out of sync with the Rust source (verified by `crates/changelog/build.rs`).

## When a fragment is required

Every PR that modifies `crates/atomic-rollback/src/**` must add at least one Fragment. This is a structural proxy for "user-facing change," enforced via `.github/workflows/pr-fragment.yml`. No commit-subject parsing, no self-reporting — the gate reads the diff directly.

Changes that do NOT touch `crates/atomic-rollback/src/**` (CI workflow edits, doc-only changes, workspace restructuring that preserves source, test-only changes elsewhere) do not require fragments.

## How to add a fragment

Two edits in `crates/changelog-core/src/lib.rs`:

1. Add a variant to the `fragments!` macro invocation:

```rust
fragments! {
    // ... existing variants ...
    DescribeYourChangeInPascalCase,
}
```

2. Add a match arm in `Fragment::status()` returning one of three variants:

For user-facing changes (the common case):

```rust
Self::DescribeYourChangeInPascalCase => Status::Unreleased {
    section: Section::Added, // or Changed, Fixed, Removed, Deprecated, Security
    text: "What users experience, in one sentence.",
},
```

For changes with no user-perceivable effect (rare):

```rust
Self::RenamedPrivateHelperFoo => Status::InternalOnly {
    description: "Renamed private helper Foo to Bar for consistency.",
},
```

The compiler enforces exhaustiveness on `status()` — a variant without a match arm produces a compile error.

After editing, regenerate `CHANGELOG.md`:

```sh
cargo run -p changelog > CHANGELOG.md
```

Commit both files together. If you don't, the next `cargo build` panics with a drift error.

## When InternalOnly is appropriate

`Status::InternalOnly` is for changes that genuinely have NO user-perceivable effect:

- Pure private renames (the symbol is private, no API change)
- Comment changes
- Test-only changes inside user-facing source (unusual)
- Whitespace/formatting

It is NOT appropriate for:

- Performance changes (user notices wall-clock)
- Behavior changes in private helpers reachable from pub API (user notices output)
- Error message tweaks (user notices)
- Dependency updates with any behavioral change
- Anything with an observable effect on the built binary

If you're unsure whether a change is truly internal-only, it probably isn't. Default to `Status::Unreleased` with a user-facing description. `InternalOnly` exists as an escape hatch, not a default.

Reviewers should challenge `InternalOnly` usage. The description field is for audit — "here's why this was user-invisible" — reviewers check whether the reasoning holds.

## Text conventions

Fragment text follows the existing CHANGELOG style:

- User-perspective synthesis, not commit-subject paraphrase
- Use backticks for commands, flags, paths
- Include root cause when it illuminates scope
- Append `Closes #N` at the end for closed issues
- One sentence when possible; two when required for clarity

## Section types

Keep a Changelog v1.1.0 defines six sections:

- `Added` — new features
- `Changed` — changes in existing functionality
- `Deprecated` — soon-to-be removed features
- `Removed` — now-removed features
- `Fixed` — bug fixes
- `Security` — security vulnerabilities

Pick the section that best describes the change from the user's perspective.

## Variant naming

PascalCase describing the change, NOT the PR or commit. Examples:

- `RollingTimestampNames` (not `Pr21Feature`)
- `BootChainTerminology` (not `Pr25Refactor`)
- `LegacyRootPreUpdateMigration`

Names describe what the bullet IS, not where it came from. Provenance (which PR, which commit) lives in git history.

## How releases consume fragments

At release time, the bump commit transitions unreleased fragments to released:

```rust
Self::DescribeYourChangeInPascalCase => Status::Released {
    version: VersionId::V0_5_0,
    section: Section::Added,
    text: "What users experience, in one sentence.",
},
```

The bump commit also adds a new `VersionId` variant if this is a new version (with semver and date in the corresponding match arms).

After transition, regenerate `CHANGELOG.md` and commit.

## Verification locally

Before pushing, verify locally:

```sh
cargo build --release   # fails if CHANGELOG.md is out of sync
cargo run -p changelog > CHANGELOG.md   # regenerate if needed
```

Then commit the regenerated `CHANGELOG.md` alongside your source changes.

## If you're stuck

Comment on your PR asking for help. A maintainer can add the Fragment for you. Rigor matters more than the contributor doing every step alone.

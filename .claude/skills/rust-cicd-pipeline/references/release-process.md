# Release Process

This document describes the complete release workflow for the Meerkat project.

## Overview

```
Development → make release-preflight → cargo release <level> → Push
```

The release pipeline enforces **version parity** across the entire stack:

```
  Cargo.toml ──→ pyproject.toml ──→ package.json     (package version)
  version.rs ──→ version.json  ──→ SDK CONTRACT_VERSION  (contract version)
```

## Version Concepts

There are two independent version numbers:

| Concept | Source of truth | Consumers |
|---------|----------------|-----------|
| **Package version** | `workspace.package.version` in `Cargo.toml` | Python `pyproject.toml`, TS `package.json` |
| **Contract version** | `ContractVersion::CURRENT` in `meerkat-contracts/src/version.rs` | `artifacts/schemas/version.json`, SDK `CONTRACT_VERSION` |

Package version = "what release are you running?"
Contract version = "what wire protocol do you speak?"

These are bumped independently. A contract version bump means the wire format changed.
A package version bump means any code changed (features, fixes, etc.).

## Step-by-Step Process

### 1. Pre-release Check

```bash
make release-preflight
```

This runs the full CI pipeline plus:
- **Version parity**: Rust, Python, TypeScript package versions match
- **Contract parity**: Rust `ContractVersion::CURRENT` matches emitted schemas and SDK types
- **Internal deps**: All `meerkat-*` deps in workspace `Cargo.toml` match workspace version
- **Schema freshness**: Committed artifacts match what `emit-schemas` produces
- **Changelog**: Warns if `CHANGELOG.md` has no uncommitted changes

### 2. Update CHANGELOG.md

Move items from `[Unreleased]` to a new version section:

```markdown
## [Unreleased]

## [0.3.0] - 2026-02-14
### Added
- New feature description

### Fixed
- Bug fix description
```

Update the comparison links at the bottom of the file.

### 3. Release with cargo-release

```bash
# Install if needed
cargo install cargo-release

# Dry run first (shows what would happen)
cargo release patch --dry-run
cargo release minor --dry-run
cargo release major --dry-run

# Execute
cargo release patch    # 0.2.0 → 0.2.1
cargo release minor    # 0.2.0 → 0.3.0
cargo release major    # 0.2.0 → 1.0.0
```

`cargo release` automatically:
1. Bumps `workspace.package.version` and all internal dep versions
2. Calls `scripts/release-hook.sh` which:
   - Bumps Python `pyproject.toml` and TypeScript `package.json`
   - Re-emits schemas (`emit-schemas`)
   - Re-runs SDK codegen (`generate.py`)
   - Runs `verify-version-parity.sh`
   - Stages all changed files
3. Creates a commit: `chore: release v0.3.0`
4. Creates an annotated tag: `v0.3.0`
5. Pushes commit + tag

### 4. Build Release Artifacts

```bash
make release
```

Creates optimized binaries in `target/release/`.

### 5. Publish to crates.io (when ready)

Currently disabled (`publish = false` in release config) due to the `hnsw_rs` vendor patch.
Once resolved:

```bash
# Dry run
make publish-dry-run

# Publish (topological order handled by cargo-release)
cargo release publish --execute
```

## Contract Version Bumps

When changing wire types in `meerkat-contracts`:

1. Bump `ContractVersion::CURRENT` in `meerkat-contracts/src/version.rs`
2. Run `make regen-schemas` to propagate to artifacts + SDKs
3. Run `make verify-version-parity` to confirm sync
4. Commit all generated files together

## Version Numbering (SemVer)

Follow [Semantic Versioning](https://semver.org/):

- **MAJOR** (1.x.x): Breaking API changes
- **MINOR** (x.1.x): New features, backwards compatible
- **PATCH** (x.x.1): Bug fixes, backwards compatible

### Pre-release Versions

```bash
cargo release alpha    # 0.2.0 → 0.3.0-alpha.1
cargo release beta     # 0.2.0 → 0.3.0-beta.1
cargo release rc       # 0.2.0 → 0.3.0-rc.1
```

## Makefile Targets Reference

| Target | Purpose |
|--------|---------|
| `verify-version-parity` | Check Rust/Python/TS/contract version sync |
| `verify-schema-freshness` | Check committed schemas match Rust source |
| `bump-sdk-versions` | Bump Python + TS versions to match Cargo |
| `regen-schemas` | Re-emit schemas + run SDK codegen |
| `release-preflight` | Full pre-release checklist (CI + freshness) |
| `publish-dry-run` | Dry-run cargo publish for all crates |
| `verify-version` | Verify Cargo version matches git tag |

## Troubleshooting

### Version Parity Failure

```
FAIL: Python SDK CONTRACT_VERSION is stale
```

**Fix**: Run `make regen-schemas` and commit the updated files.

### Schema Freshness Failure

**Fix**: Someone changed Rust types in `meerkat-contracts` without regenerating.
Run `make regen-schemas` and commit.

### Version Mismatch After Release Failure

If something fails after `cargo release` creates a tag:

```bash
# Delete the tag locally and remotely
git tag -d v0.3.0
git push origin :refs/tags/v0.3.0

# Fix the issue, then re-run
cargo release <level>
```

## Pre-Release Checklist

- [ ] All tests passing (`make ci`)
- [ ] Version parity verified (`make verify-version-parity`)
- [ ] Schemas fresh (`make verify-schema-freshness`)
- [ ] CHANGELOG.md updated with all changes
- [ ] On main branch with latest changes
- [ ] Release builds successfully (`make release`)

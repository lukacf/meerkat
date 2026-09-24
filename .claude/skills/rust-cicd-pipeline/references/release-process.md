# Meerkat Release Process

This reference describes the current release workflow for this repository.
The public distribution guide is `docs/guides/cd-and-distribution.md`.

## Overview

```text
notes under Unreleased
  -> make release-preflight
  -> ./scripts/repo-cargo release <level> --execute
  -> version commit and tag
  -> exact-main GitHub Actions release
```

Cargo is the local command backend. Automatic workflow dispatch selects
BuildBuddy validation and Linux/macOS packaging for the repository owner when
the required repository variable is enabled; other actors fall back to
GitHub-hosted runners. Neither path replaces exact-main GitHub Actions CI,
Windows packaging, or credentialed registry publication.

## Version Concepts

Two version projections move together for a release:

| Concept | Source of truth | Consumers |
|---------|-----------------|-----------|
| Package version | `workspace.package.version` in `Cargo.toml` | Rust crates, Python, TypeScript, Web SDK, docs |
| Contract version | `ContractVersion::CURRENT` in `crates/meerkat-contracts/src/version.rs` | Generated schemas and SDK contract constants |

Meerkat is pre-1.0. Patch releases may contain declared public API breaks, so
downstream Rust users must exact-pin the crate family and read every intervening
changelog section.

## 1. Prepare Release Notes

Keep all pending notes under the existing `## [Unreleased]` section. Name each
measured public API break under `### Breaking`; cargo-semver-checks cannot see
behavior-only breaks, so declare those manually too.

Do not rename the pending heading to the new version. The pre-release hook is
the authority that stamps the version and date in the same commit as the
version bump.

## 2. Run the Gates

```bash
make release-preflight
```

The preflight covers the release environment, Cargo and Bazel lock consistency,
normal CI, schema and wrapper freshness, Rust packaging, and declared public
API breaks. For the larger no-upload rehearsal, run:

```bash
make release-dry-run
```

That also exercises Rust, Python, TypeScript, and Web package dry-runs and
package smoke tests.

## 3. Create the Version Commit and Tag

Install `cargo-release` if needed, then inspect the dry run:

```bash
./scripts/repo-cargo release patch
```

Execute only after the dry run and preflight are clean:

```bash
./scripts/repo-cargo release patch --execute
```

Use `minor` or an explicit version in place of `patch` when appropriate.
Workspace release metadata in `Cargo.toml` owns the shared-version commit,
annotated `v<version>` tag, and push behavior.

During the version-bump commit, `scripts/release-hook.sh` runs once for the
workspace and:

1. bumps Python, TypeScript, Web, documentation, and contract versions;
2. stamps `CHANGELOG.md` as `## [<version>] - <YYYY-MM-DD>`, creates a fresh
   `## [Unreleased]` section, and advances the comparison links;
3. regenerates schemas, SDK types/wrappers, BuildBuddy BUILD files, and the
   Bazel module lock;
4. verifies version, RPC, and wrapper parity; and
5. stages the generated release files into the same commit.

Missing notes, malformed comparison links, or stale generated contracts fail the hook. Do not work around a
hook failure by manually committing only part of its output.

## 4. Publication

The pushed tag starts `.github/workflows/release.yml`. It verifies the
tree-bound attestation from successful exact-main CI, then builds and publishes:

- `rkat`, `rkat-rpc`, `rkat-rest`, and `rkat-mcp` archives for Linux, macOS,
  and Windows;
- `checksums.sha256` and `index.json`;
- the Homebrew formula update;
- publishable Rust crates;
- `meerkat-sdk` on PyPI;
- `@rkat/sdk` and `@rkat/web` on npm.

The workspace `cargo-release` setting has `publish = false`; registry
publication belongs to the release workflow and its ordered package scripts,
not to the local version-bump command.

## Contract Changes Outside a Release

When editing wire types in `meerkat-contracts`:

```bash
make regen-schemas
make verify-version-parity
make verify-sdk-codegen-freshness
make verify-sdk-wrapper-freshness
```

Commit source, schemas, generated SDK types, and wrapper projections together.
The release hook will later stamp their version for the release.

## Recovery

Do not move or delete a published release tag to repair one failed publication
surface. The release workflow has narrow manual recovery modes for assets,
packages, the Web SDK, and Web SDK publication. Each recovery selects an
existing release tag and still requires successful exact-main CI for that
commit.

Use the local release facade to print a recovery invocation without dispatching
it:

```bash
RELEASE_WORKFLOW_DRY_RUN=true \
RELEASE_BACKEND=github-hosted \
  make release-assets VERSION=vX.Y.Z
```

Use `release-packages` or `release-web-sdk` instead to inspect those narrow
recovery modes. `release-workflow` is the full release mode, not a narrow
recovery invocation.

`REGISTRY_DRY_RUN=true` is an input to a real dispatched workflow. It does not
suppress binary, GitHub Release, or Homebrew publication.

If a local dry run fails before any version commit or tag exists, fix the
cause, remove only the hook's documented local sentinel if necessary, rerun
the preflight, and retry the dry run.

## Useful Targets

| Target | Purpose |
|--------|---------|
| `release-doctor` | Validate release tooling and environment |
| `release-preflight` | Full local release gate |
| `release-dry-run` | No-upload package and artifact rehearsal |
| `verify-version-parity` | Check package, docs, schema, and SDK version projections |
| `verify-schema-freshness` | Compare committed schemas with Rust source |
| `verify-sdk-codegen-freshness` | Check generated SDK contract types |
| `verify-sdk-wrapper-freshness` | Check public generated RPC wrapper boundaries |
| `semver-breaks` | Compare publishable Rust APIs with released baselines |
| `release-workflow` | Run the selected release backend facade |

## Checklist

- [ ] Pending notes remain under `## [Unreleased]`.
- [ ] Every measured and behavior-only break is declared.
- [ ] `make release-preflight` is green.
- [ ] `make release-dry-run` is green when a full rehearsal is required.
- [ ] The release is cut from the intended exact-main commit.
- [ ] The installed pre-push hook is allowed to run.
- [ ] The tag workflow publishes checksums, index, binaries, registries, and
      Homebrew state without version skew.

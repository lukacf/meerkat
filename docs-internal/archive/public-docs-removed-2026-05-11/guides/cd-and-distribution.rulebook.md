---
title: "CD and distribution"
description: "Publication model for Meerkat binaries, Rust crates, Python SDK, and TypeScript SDK."
icon: "truck-fast"
---

# CD and Distribution Rulebook

This document captures the publication model for Meerkat binaries and SDKs
(`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`, Python SDK, and TypeScript SDK).

## Goals

- Publish a single source project to multiple ecosystems.
- Keep runtime binaries available for macOS/Linux/Windows users.
- Let Python (`pip`/`uv`) and Node (`npm`) users install from package managers
  without needing a Rust toolchain.
- Preserve reproducible releases with strong version/contract checks.

## Release Baseline

Run these before release:

1. `make release-preflight`
2. `make verify-version-parity`
3. `make verify-schema-freshness`

For a full local dry-run that includes registry-facing publish checks without uploading,
run:

4. `make release-dry-run`

Version and contract compatibility must already be in sync via:

- `Cargo.toml` workspace version
- `sdks/python/pyproject.toml` version
- `sdks/typescript/package.json` version
- generated contract artifacts

## Distribution Topology

### 1) Runtime binaries (GitHub Releases)

Release artifacts are built for each surface binary:

- `rkat` (CLI)
- `rkat-rpc`
- `rkat-rest`
- `rkat-mcp`

Build targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `x86_64-pc-windows-msvc`

Publish as release assets, for example:

- `rkat-rpc-X.Y.Z-x86_64-apple-darwin.tar.gz`
- `rkat-rest-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `rkat-X.Y.Z-x86_64-pc-windows-msvc.zip`

Include a checksum manifest with all artifacts:

- `checksums.sha256`
- `index.json`

### 2) Python SDK distribution

Package behavior should be pure-Python with optional runtime auto-download:

- `pip install meerkat-sdk` (and `uv` equivalent)
- Package should not require Rust/toolchain locally
- First connect:
  - If `MEERKAT_BIN_PATH` is set, use it
  - Otherwise, download correct `rkat-rpc` binary for platform/version from GitHub Releases
  - Cache in user-local location (`~/.cache/meerkat/bin/...`)

This gives users a true one-command install path for Python.

### 3) TypeScript SDK distribution

- `npm install @rkat/sdk`
- The SDK package remains JS-only; no native modules.
- `MeerkatClient` resolves server binary with the same strategy:
  - env override path first
  - otherwise download platform binary from GitHub Releases

For MCP-style tooling (`claude mcp add`, `npx`), provide a CLI entrypoint package:

- `npx @meerkat/mcp` (or equivalent) launches the JS shim
- shim resolves local override or downloaded `rkat-mcp`

## GitHub Actions Design (single-release flow)

### Job 1 — Validate

- Cargo/GitHub-hosted branch: run the release validation checks directly on
  GitHub-hosted runners.
- BuildBuddy branch: run the same release validation contract through
  `release-validate-rbe`.
- A release validation gate verifies exactly one branch ran, matching the
  selected backend.

### Job 2 — Build matrix

- The public, contributor-facing release path is GitHub Actions.
- Use GH hosted runners for:
  - `macos-latest`
  - `ubuntu-latest`
  - `windows-latest`
- Build release binaries for all required targets.
- The BuildBuddy branch builds the same binary set through the generated Bazel
  release lanes.
- A binary build gate verifies exactly one branch ran, matching the selected
  backend.

### Release backend selection

BuildBuddy release validation and binaries are an owner-only acceleration path,
not the public default. Keep GitHub-hosted release jobs as the standard public
interface for contributors and observers.

The release workflow has the same selected-backend shape as CI:

- Cargo/GitHub-hosted branch: default public path.
- BuildBuddy branch: owner-only path when explicitly selected.
- Gate job: confirms the selected branch passed and the other branch skipped.

For tag-triggered releases, repository owners may set the public repository
variable `MEERKAT_RELEASE_BUILDBUDDY=1` to select the BuildBuddy release branch.
For manual dispatches, use the `release_backend` input.

The Make dispatch wrappers use the same local flip as CI:

```bash
make release-workflow VERSION=vX.Y.Z
MEERKAT_BUILDBUDDY=1 make release-workflow VERSION=vX.Y.Z
MEERKAT_BUILDBUDDY=1 make release-assets VERSION=vX.Y.Z
```

The BuildBuddy branch routes the release matrix through the generated Bazel
release lanes:

- `release-build-linux-x86`
- `release-build-linux-arm64`
- `release-build-macos-arm64`
- `release-build-macos-x86`
- `release-build-windows-x86`

Those release lanes build the same public binary set as the GitHub-hosted path:
`rkat`, `rkat-rpc`, `rkat-rest`, and `rkat-mcp`. Mini binaries are deliberately
excluded from both public release paths; build them from source when a custom
distribution needs a reduced surface.

Do not commit private BuildBuddy endpoint names or enterprise infrastructure
details. Supply endpoint overrides through secrets or local environment only;
the public repository should document the variable shape, not the private
value.

`MEERKAT_RELEASE_BUILDBUDDY_JOBS` may be set as a repository variable to tune
release validation and build fanout for owner-only BuildBuddy dispatches. The
workflow defaults to `64` jobs when the variable is unset; keep this knob public
and endpoint values private.

Linux arm64 release binaries use extra RBE care because the release graph has a
few very large generated Rust libraries and AArch64 `ld.gold` can fail during
final binary links. The BuildBuddy lane requests a larger arm64 executor slot
and uses the system `bfd` linker for Linux arm64 release links. The generated
Bazel agent-factory bridge variants for `meerkat-machine-schema` and
`meerkat-runtime` compile at `opt-level=0` to avoid remote optimizer stalls in
that bridge graph; this is scoped to the generated Bazel release facade and does
not change the public Cargo release path.

### Job 3 — Publish GitHub release

- Create release tag `vX.Y.Z`
- Upload compiled binaries and checksum/index files.

### Job 4 — Publish crates (optional by timeline)

- `cargo release ...` (or equivalent scripted sequence)
- Rust crates should be published first or concurrently after checks pass.
- Dry-run mode: run the same validation/build path without registry writes using manual dispatch input `registry_dry_run=true`.

### Job 5 — Publish language packages

- Publish Python wheel (`meerkat-sdk`) from same version
- Publish TypeScript package (`@rkat/sdk`, optional MCP wrapper package)
- Both packages support dry-run behavior in registry publish mode so you can validate metadata/build without actually uploading.

### Job 6 — Post-release smoke

- Verify:
  - `pip install` succeeds
  - `uv pip` variant works
  - `npx` / MCP command resolves and launches binary for a platform from release assets

## Account / credential configuration

You only mentioned Cargo is already configured; Pip/npm still require separate credentials.

### Rust (already configured)

- Keep your Cargo token (already set).
- Ensure API token has permission to publish all crates.

### PyPI

- Create/import a PyPI account and API token.
- Store token in CI as `PYPI_API_TOKEN` (or `pypi-token` equivalent secret for your CI tool).
- Use API token auth in publishing:
  - user name: `__token__`
  - password: token value
- Optional separate TestPyPI flow with separate token.

### npm

- Create npm account with publish rights to package scope (`@meerkat` org/user scope).
- In CI, store token as `NPM_TOKEN`.
- Configure automation profile:
  - `npm config set //registry.npmjs.org/:_authToken=${NPM_TOKEN}`
- Ensure package versions are always incremented.

## MCP + CLI ergonomics decision

- Keep Python/TypeScript SDKs as pure clients.
- Keep runtime downloads in SDK layers (or thin wrapper binaries/entrypoints), not in end-user binaries.
- Surface contracts stay stable because transport endpoints are unchanged; only bootstrap/packaging changes.

## Hard constraints to enforce

1. Never publish mismatched versions:
   - Rust workspace, Python package, TypeScript package, contract artifacts.
2. Release only from tagged versions.
3. Never publish SDKs that point to a commit with stale schema/generator artifacts.
4. Keep runtime binary names and entrypoints stable per surface.
5. Keep checksum and index files for deterministic, auditable consumers.

## Minimal checklist for each release

- [ ] `make release-preflight`
- [ ] tag release (`git tag -a vX.Y.Z`)
- [ ] choose release backend: public default, or `MEERKAT_RELEASE_BUILDBUDDY=1`
      / `release_backend=buildbuddy` for owner-only BuildBuddy
- [ ] build + upload all binary assets in release workflow
- [ ] publish crates
- [ ] publish Python SDK
- [ ] publish TypeScript SDK (and MCP entrypoint package if present)
- [ ] confirm smoke checks (`pip`, `uv`, `npx`) across at least one platform

## Current state note

This is a plan/rulebook currently. Code and workflows should follow this sequence
with minimal surface-area, no behavior changes to runtime APIs.

## Registry publish dry-run

- Workflow dispatch inputs:
  - `release_backend` (default: `github-hosted`): select `github-hosted` or
    owner-only `buildbuddy` validation/binary branches.
  - `publish_release_packages` (default: false): enable publish job.
  - `registry_dry_run` (default: false): keep publish jobs in dry-run mode.
- Dry-run behavior:
  - Rust: `cargo publish --dry-run` for each crate in publish list.
  - Python: `python -m twine check` on the built wheel/sdist, no upload.
  - TypeScript: `npm publish --dry-run`.
- Local equivalent:
  - `make release-dry-run`
- Example:
  - `make release-workflow VERSION=vX.Y.Z REGISTRY_DRY_RUN=true`
  - `MEERKAT_BUILDBUDDY=1 make release-workflow VERSION=vX.Y.Z REGISTRY_DRY_RUN=true`

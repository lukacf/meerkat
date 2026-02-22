# CD and Distribution Rulebook

This document captures the publication model for Meerkat binaries and SDKs
(`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`, Python SDK,
and TypeScript SDK).

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

- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

Publish as release assets, for example:

- `rkat-rpc-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `rkat-rest-vX.Y.Z-x86_64-unknown-linux-gnu.zip`

Include a checksum manifest with all artifacts:

- `dist/checksums.sha256`
- `dist/index.json`

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

- `npm install @meerkat/sdk`
- The SDK package remains JS-only; no native modules.
- `MeerkatClient` resolves server binary with the same strategy:
  - env override path first
  - otherwise download platform binary from GitHub Releases

For MCP-style tooling (`claude mcp add`, `npx`), provide a CLI entrypoint package:

- `npx @meerkat/mcp` (or equivalent) launches the JS shim
- shim resolves local override or downloaded `rkat-mcp`

## GitHub Actions Design (single-release flow)

### Job 1 — Validate

- `make ci`
- `make verify-version-parity`

### Job 2 — Build matrix

- Use GH hosted runners for:
  - `macos-latest`
  - `ubuntu-latest`
  - `windows-latest`
- Build release binaries for all required targets.

### Job 3 — Publish GitHub release

- Create release tag `vX.Y.Z`
- Upload compiled binaries and checksum/index files.

### Job 4 — Publish crates (optional by timeline)

- `cargo release ...` (or equivalent scripted sequence)
- Rust crates should be published first or concurrently after checks pass.
- Dry-run mode: run the same validation/build path without registry writes using manual dispatch input `registry_dry_run=true`.

### Job 5 — Publish language packages

- Publish Python wheel (`meerkat-sdk`) from same version
- Publish TypeScript package (`@meerkat/sdk`, optional MCP wrapper package)
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
  - `publish_release_packages` (default: false): enable publish job.
  - `registry_dry_run` (default: false): keep publish jobs in dry-run mode.
- Dry-run behavior:
  - Rust: `cargo publish --dry-run` for each crate in publish list.
  - Python: `python -m twine check` on the built wheel/sdist, no upload.
  - TypeScript: `npm publish --dry-run`.
- Local equivalent:
  - `make release-dry-run`
- Example:
  - `gh workflow run release.yml -f publish_release_packages=true -f registry_dry_run=true`

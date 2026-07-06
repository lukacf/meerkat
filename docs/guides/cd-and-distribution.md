---
title: "CD and distribution"
description: "How Meerkat release artifacts are validated, built, and published across Rust, binaries, Python, and TypeScript."
icon: "truck-fast"
---

Meerkat publishes one source project across several consumer surfaces:

| Artifact | Audience | Published as |
|----------|----------|--------------|
| Rust crates | Rust library users and surface binaries | crates.io |
| `rkat` | CLI users | Homebrew tap for macOS/Linux, GitHub Release binary, Rust crate binary |
| `rkat-rpc` | SDK backends and JSON-RPC hosts | GitHub Release binary |
| `rkat-rest` | HTTP/SSE service hosts | GitHub Release binary |
| `rkat-mcp` | MCP host integrations | GitHub Release binary |
| Python SDK | Python applications | `meerkat-sdk` on PyPI |
| TypeScript SDK | Node applications | `@rkat/sdk` on npm |
| Web SDK | Browser applications | `@rkat/web` on npm |

The public release path is GitHub Actions. BuildBuddy release lanes are an
owner-only acceleration path and use the same Make-level contract.

## Versioning and Compatibility

Meerkat is pre-1.0 and releases on a fast `0.x.y` patch train. The policy,
stated plainly so downstream embedders can build against it:

- **Patch releases may change public APIs.** A `0.7.x` → `0.7.(x+1)` bump can
  add required fields, change function signatures, or remove items. Cargo's
  default caret requirement (`meerkat = "0.7"`) treats the whole `0.7`
  family as compatible, which is stronger than this project guarantees.
- **Embedders must pin exact versions.** Libraries and applications that
  build against Meerkat crates should declare `=0.7.19`-style exact pins and
  move deliberately, reading the changelog for each hop.
- **The only supported crate combination is exact version parity.** All
  workspace crates (`meerkat`, `meerkat-core`, `meerkat-runtime`, …), the
  Python/TypeScript/Web SDKs, and `ContractVersion::CURRENT` are lock-stepped
  to one version per release. Mixing crate versions across releases is
  unsupported.
- **Breaking API changes are flagged in the changelog.** Public-signature
  changes land under a `### Breaking` heading in `CHANGELOG.md` for the
  release that ships them; observable default-behavior changes land under
  `### Changed`. A release with neither heading is intended to be a drop-in
  replacement for the previous patch version.

### Downstream compatibility matrix

Known downstream projects that embed Meerkat, and the exact versions they
were built and verified against:

| Downstream | Downstream version | Meerkat version |
|------------|--------------------|-----------------|
| meerkat-mobkit | 0.7.22 | =0.7.15 |
| meerkat-mobkit | 0.7.23 | =0.7.17 |

Downstream projects should declare their supported Meerkat version as an
exact pin in their own `Cargo.toml` (not only in `Cargo.lock`), so consumers
can read the supported combination without archaeologizing lockfiles at
release tags.

## Release Checks

Run the release gate before cutting a tag:

```bash
make release-preflight
make verify-version-parity
make verify-schema-freshness
make release-dry-run
```

These checks keep the Rust workspace version, Python package version,
TypeScript package version, and generated contract artifacts aligned before any
registry publish happens.

## Binary Artifacts

Release assets are built for these binaries:

- `rkat`
- `rkat-rpc`
- `rkat-rest`
- `rkat-mcp`

Standard targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `x86_64-pc-windows-msvc`

Release assets include platform archives plus a checksum manifest:

- `checksums.sha256`
- `index.json`

## Homebrew Tap

The featured CLI install path is the Homebrew tap:

```bash
brew install lukacf/meerkat/rkat
```

The generated formula supports both macOS and Linux release assets. Linux users
should install Homebrew from the official
[Homebrew on Linux](https://docs.brew.sh/Homebrew-on-Linux) instructions before
using the same tap command.

The formula installs `rkat` plus the companion binaries:

- `rkat-rpc`
- `rkat-rest`
- `rkat-mcp`

Reduced distributions are source builds of the same crates with a narrower
feature set, not separate public binaries.

## SDK Bootstrap

Python and TypeScript SDK users should not need a local Rust toolchain.

| SDK | Install | Runtime resolution |
|-----|---------|--------------------|
| Python | `pip install meerkat-sdk` | Uses `MEERKAT_BIN_PATH` when set; otherwise resolves a matching `rkat-rpc` release binary |
| TypeScript | `npm install @rkat/sdk` | Uses an explicit binary path when configured; otherwise resolves a matching `rkat-rpc` release binary |

The SDKs are clients. They start or connect to the JSON-RPC surface rather than
embedding a separate runtime implementation.

## Release Workflow

1. Validate the release candidate.
2. Build platform binaries.
3. Create the GitHub release and upload binary assets.
4. Update the Homebrew tap formula.
5. Publish Rust crates.
6. Publish Python, TypeScript, and Web SDK packages.
7. Run install smoke checks for at least one platform.

Manual release dispatch supports dry-run registry validation. Locally, use:

```bash
make release-workflow VERSION=vX.Y.Z REGISTRY_DRY_RUN=true
MEERKAT_BUILDBUDDY=1 make release-workflow VERSION=vX.Y.Z REGISTRY_DRY_RUN=true
```

## BuildBuddy

Cargo is the default backend. BuildBuddy is selected explicitly:

```bash
MEERKAT_BUILDBUDDY=1 make release-preflight
MEERKAT_BUILDBUDDY=1 make release-assets VERSION=vX.Y.Z
```

Use `make buildbuddy-doctor` when the local BuildBuddy setup looks suspicious.
It checks the API key, pinned `bb` CLI, generated Bazel files, selector
behavior, and lane isolation without printing secrets.

## Credentials

Registry credentials are independent:

| Registry | Credential |
|----------|------------|
| Homebrew tap | `HOMEBREW_TAP_TOKEN` |
| crates.io | Cargo publish token |
| PyPI | `PYPI_API_TOKEN` |
| npm | `NPM_TOKEN` |

Keep tokens in CI secrets or a local secret store. Do not commit registry
tokens, private BuildBuddy endpoints, or enterprise infrastructure names.

## Hard Rules

- Release only from tagged versions.
- Never publish mismatched Rust, Python, TypeScript, or contract versions.
- Never publish SDKs from a commit with stale generated schema artifacts.
- Keep public binary names stable: `rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`.
- Publish checksums and an index for release binary consumers.

## See Also

- [Build and CI](/reference/build-and-ci)
- [CLI commands](/cli/commands)

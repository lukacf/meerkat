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

The public release path is GitHub Actions. Tag-triggered Linux/macOS
packaging selects BuildBuddy only for the literal actor `lukacf` when
`MEERKAT_RELEASE_BUILDBUDDY` is `true` or `1`; other actors use the hosted
fallback. Eligible manual dispatches select release validation and packaging
under the [BuildBuddy rules below](#buildbuddy). Both release backends still
require exact-main GitHub Actions CI, whose current components are GCP
BuildBuddy and hosted dense-Mob topology. Windows binaries are cross-compiled
and packaged on hosted Ubuntu, then verified on Windows; credentialed registry
publishing stays GitHub-hosted.

## Versioning and Compatibility

Meerkat is pre-1.0 and releases on a fast `0.x.y` patch train. The policy,
stated plainly so downstream embedders can build against it:

- **Patch releases may change public APIs.** A `0.8.x` to `0.8.(x+1)` bump can
  add required fields, change function signatures, or remove items. Cargo's
  default caret requirement (`meerkat = "0.8"`) treats the whole `0.8`
  family as compatible, which is stronger than this project guarantees.
- **Embedders must pin exact versions.** Libraries and applications that
  build against Meerkat crates should declare `=0.8.24`-style exact pins and
  move deliberately, reading the changelog for each hop.
- **The only supported crate combination is exact version parity.** All
  workspace crates (`meerkat`, `meerkat-core`, `meerkat-runtime`, …), the
  Python/TypeScript/Web SDKs, and `ContractVersion::CURRENT` are lock-stepped
  to one version per release. Mixing crate versions across releases is
  unsupported.
- **Breaking API changes are flagged in the changelog.** Public-signature and
  behavior-only compatibility breaks land under a `### Breaking` heading in
  `CHANGELOG.md` for the release that ships them. Non-breaking observable
  behavior changes land under `### Changed`. A release with neither heading
  is intended to be a drop-in replacement for the previous patch version.

### Downstream compatibility

This repository does not maintain a live cross-repository compatibility
matrix. Each downstream project owns its supported Meerkat version in its
release manifest and release notes. Downstream Rust projects should declare
that version as an exact pin in `Cargo.toml`, not only in `Cargo.lock`, so the
supported combination is visible without reconstructing historical lockfiles.

## Release Checks

Run the release gate before cutting a tag:

```bash
make release-preflight
```

This runs the release environment doctor, Cargo and Bazel lock checks, the
normal CI lane, schema freshness, Rust packaging checks, and the declared-break
gate. Use `make release-dry-run` for the larger no-upload rehearsal; it includes
the preflight plus Rust, Python, TypeScript, and Web SDK publish dry-runs and
package smoke tests.

Local preflight is not the only publication prerequisite. The independent
**Release semver readiness** workflow
(`.github/workflows/release-semver-readiness.yml`) must also supply the normal
tag path's exact-tree, exact-version evidence from a successful `main` push.
Its artifact has 30-day configured retention. Wait for both CI and readiness
on the prepared release tree before tagging to avoid racing the release
gate's artifact lookup; the [release workflow](#release-workflow) below
describes the enforcement point and narrower manual recovery paths.

## Declared Breaks (`semver-breaks`)

0.x patch releases may break public API; every break must be declared. The
`semver-breaks` gate runs cargo-semver-checks against the published crates.io
baseline and fails the release unless all three hold:

1. **Measured.** Every crate the release publishes was either rebuilt and
   compared, or proven identical to the baseline release. Only crates whose
   source directory or declared dependency specs differ from the baseline tag
   are rebuilt (`scripts/semver_changed_crates.py` classifies them; generated
   Bazel files and test, bench, example and doc trees do not count). Neither
   side is built inside cargo-semver-checks: the baseline is the rustdoc JSON
   the release workflow attached to the baseline's GitHub release
   (`semver-rustdoc-<version>.tar.zst`), the candidate is generated once for
   every publishable library crate by the same `scripts/semver-rustdoc-json.sh`
   on the pinned rustc, and the tool compares the two files per crate
   (`--baseline-rustdoc`, `--current-rustdoc`). One cargo invocation documents
   all library crates on each side so workspace feature unification is
   identical across releases. Only if the asset is missing or was built by a
   different rustc is the baseline tag checked out and the generator run
   there. A release measurement is therefore a minutes-scale job. Identical
   crates are recorded as
   reached by equivalence because cargo-semver-checks reports on a crate's
   own items, which cannot differ when nothing it is built from differs. The
   tool's exit code must agree with its report; a run that died halfway, or
   failed for a reason other than a detected break, is a failure rather than
   a pass.
2. **Named.** Every finding the tool reports is named in the pending release
   section's `### Breaking` body, at the granularity of the finding. A type
   gaining a field and the same type losing a derive are two findings; naming
   one does not declare the other.
3. **Stamped.** The pending section is stamped `## [VERSION] - DATE` against
   the version being released. Notes still sitting under `## [Unreleased]`
   after the version bump has landed would publish as release notes titled
   "Unreleased", and that is a failure.

Behaviour-only breaks - a public signature that keeps its shape and changes
what it does - are invisible to cargo-semver-checks and therefore to this gate.
Declare them by hand in `### Breaking`.

Three published crates are outside what cargo-semver-checks looks at, and the
gate prints them on every run rather than hiding the gap: `meerkat-machine-derive`
and `meerkat-machine-dsl` are proc-macro crates (a `--workspace` run emits no
output for them at all), and `rkat` has no lib target. Breaks in those three are
declared by hand or not at all.

### Changelog stamping is part of the release commit

Write pending notes under `## [Unreleased]` and leave them there for
`make release-preflight`. When `./scripts/repo-cargo release` creates the version-bump commit,
its `scripts/release-hook.sh` pre-release hook:

1. stamps the pending section as `## [<version>] - <YYYY-MM-DD>`, creates a
   fresh `## [Unreleased]` stub, and advances the comparison links;
2. bumps SDK, documentation, and contract versions;
3. regenerates schemas, SDK wrappers, and Bazel metadata; and
4. verifies and stages those generated release files in the same commit.

Missing release notes or malformed comparison links fail the hook. Do not stamp the heading by hand before
the version bump: the hook is the authority that binds the notes and released
version in one commit. On the normal tag/explicit-tag package recovery path,
`release_semver_gate` requires separate semver-readiness evidence that
declared breaks and stamped notes were verified for that exact tree and
version. Asset-only and Web-SDK-only recovery paths skip this Rust semver gate.

Where it runs:

| Lane | Entry point |
|------|-------------|
| Local preflight | `make semver-breaks` (part of `make release-preflight`) |
| Release semver readiness | Separate workflow on `Cargo.toml`/`CHANGELOG.md` changes to `main` or PRs, plus manual dispatch; measures unpublished candidates and uploads main-push evidence or preview evidence |
| Release workflow | job `release_semver_gate`, required on tag publication and package recovery; normally consumes unexpired exact-tree, exact-version main-push readiness evidence; asset-only and Web-SDK-only recovery explicitly accept a skipped gate |
| Parser self-test | `make semver-breaks-selftest`, included in local `make ci` and the separate reusable/manual Cargo workflow's `ratchets` job: unit-tests the report parser against committed real reports, without needing cargo-semver-checks installed |

The judgement lives in `scripts/check_semver_breaks.py`, which is a pure
function of (report, changelog, version, tool exit code) and has no environment
override that relaxes it. `scripts/check-semver-breaks.sh` only produces the
report and hands it over.

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

1. Prepare the release commit through cargo-release and its release hook, so
   version projections and stamped notes stay in one hook-owned tree. Do not
   manually edit versions or create tags to bypass that workflow.
2. Prefer waiting for both exact-main CI and **Release semver readiness** on
   that prepared tree before cargo-release publishes the tag. Ordinary CI and
   semver readiness produce separate artifacts; one cannot substitute for the
   other.
3. The tag path verifies the exact-tree CI attestation instead of recomputing
   the broad CI graph. After `require_ci_green`, `release_semver_gate` looks
   up `meerkat-semver-attestation-main-<tree_sha>` and requires unexpired
   evidence for that tree and version from a successful readiness `main` push.
4. In parallel, verify registry credentials, consume declared-break evidence,
   build platform binaries, and build the Web SDK package artifact.
5. Publish the macOS/Linux assets early when available, then complete the
   GitHub release with every platform archive, `checksums.sha256`, and
   `index.json`.
6. Update the Homebrew tap from the published assets.
7. Publish Rust crates plus the Python and TypeScript SDKs on GitHub-hosted
   runners. Their publish jobs build and smoke-test the package artifacts before
   upload.
8. Publish the prebuilt Web SDK artifact on a GitHub-hosted runner.

The default `./scripts/repo-cargo release patch --execute` flow combines
bump, commit, tag, and push; that command summary alone does not establish
readiness. A combined push can succeed if readiness completes before the
semver artifact lookup, but it races that lookup. The gate checks evidence
availability then, not whether readiness completed before tag creation.
Readiness evidence is configured to expire after 30 days. PR and manually
dispatched readiness artifacts use the `meerkat-semver-attestation-preview-`
prefix and do not qualify for the normal tag path.

The current schema-3 CI attestation binds the repository, commit SHA, Git tree
SHA, CI workflow run and attempt, branch, and event to backend
`gcp-buildbuddy+github-hosted-dense-mob`, its aggregate `validation_result`,
and both `component_results` (`gcp_buildbuddy`, `github_hosted_dense_mob`).
The release gate downloads it from the successful exact-main workflow run and
verifies every required field before tag-triggered publication starts. The
consumer also accepts the supported legacy schema-1 Cargo and schema-2 BuildBuddy
attestations; the current producer does not emit those formats.
Manual recovery dispatches still require successful exact-main CI for the
selected release commit. When release validation applies, the dispatch runs
the selected validation lane directly rather than requiring a retained **CI**
attestation artifact. Narrow `assets`,
`packages`, `web-sdk`, and `web-sdk-publish` modes repair one publication
surface without rebuilding or republishing unrelated surfaces.

This CI-artifact exception does not bypass the semver gate. An ordinary
explicit-`release_tag` package recovery still consumes main-push readiness
evidence. When this gate applies, a manual dispatch with neither `release_tag`
nor `semver_evidence_job_id` runs `make semver-breaks` directly. The separate
completed-measurement recovery path requires `release_tag`,
`semver_evidence_run_id`, and `semver_evidence_job_id`; its verifier checks
the exact completed measurement and amended declarations from the workflow
ref, while published source remains the release tag. It accepts only a
completed measurement on that exact immutable tag/SHA that failed solely for
missing breaking-change declarations, not an incomplete or failed setup.
A generic manual readiness preview is not that recovery path.

To inspect the exact GitHub CLI dispatch without starting a workflow, use:

```bash
RELEASE_WORKFLOW_DRY_RUN=true \
  RELEASE_BACKEND=github-hosted \
  make release-workflow VERSION=vX.Y.Z
```

`REGISTRY_DRY_RUN=true` changes registry publication inside a dispatched
workflow; it is not a workflow dry run and does not suppress binary, GitHub
Release, or Homebrew publication.

## BuildBuddy

Automatic tag packaging selects BuildBuddy only when `github.actor` is
literally `lukacf` and the repository variable
`MEERKAT_RELEASE_BUILDBUDDY` is `true` or `1`. For eligible manual dispatches,
explicit `release_backend=buildbuddy` selects BuildBuddy validation and
Linux/macOS packaging only for that same literal actor; it does not also
require the automatic-selection variable. This is a named-actor condition,
not a general owner/admin permission check.

The following explicit BuildBuddy dispatch is for `lukacf`:

```bash
RELEASE_BACKEND=buildbuddy make release-workflow VERSION=vX.Y.Z
```

On eligible manual dispatches, other actors use GitHub-hosted Cargo validation
and Linux/macOS packaging even when they request `release_backend=buildbuddy`.
Windows packaging and every credentialed registry publish still run on
GitHub-hosted runners, and the exact-main GitHub Actions CI requirement remains
in force. This release
backend selection is separate from per-push CI, which currently requires both
GCP BuildBuddy and hosted dense-Mob topology.

For local Make commands, `MEERKAT_BUILDBUDDY=1` selects the optional BuildBuddy
developer backend:

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

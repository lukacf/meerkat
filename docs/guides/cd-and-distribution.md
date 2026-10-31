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

The public release path is GitHub Actions. Tag-triggered packaging of the
critical platforms (Linux x86_64 and aarch64, macOS arm64) selects BuildBuddy only for the literal actor `lukacf` when
`MEERKAT_RELEASE_BUILDBUDDY` is `true` or `1`; other actors use the hosted
fallback. Eligible manual dispatches select release validation and packaging
under the [BuildBuddy rules below](#buildbuddy). Both release backends still
require exact-main GitHub Actions CI (the Cargo-only pull-request lanes; for
a [maintenance release](#maintenance-releases), the same push CI on its
release branch),
and the tag path additionally runs the full BuildBuddy graph as its
validation gate while the self-hosted graph is enabled
(`MEERKAT_GCP_BUILDBUDDY_ENABLED` is `true`); otherwise the tag skips the
Cargo validation job only when its verified exact-tree CI attestation proves
that exact-main run ran every lane the job repeats (the generated-contract
ratchets, which main CI runs only when its plan selects them, and the SDK
suites); any other tag, and every eligible manual dispatch that selects it,
still validates with Cargo. Windows binaries are cross-compiled
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
tag path's exact-tree, exact-version evidence from a successful `main` push
(for a [maintenance release](#maintenance-releases), from a push to its
release branch).
Its artifact has 30-day configured retention. Wait for both CI and readiness
on the prepared release tree before tagging to avoid racing the release
gate's artifact lookup; the [release workflow](#release-workflow) below
describes the enforcement point and narrower manual recovery paths.

## Declared Breaks (`semver-breaks`)

0.x patch releases may break public API; every break must be declared. The
`semver-breaks` gate runs cargo-semver-checks against the published crates.io
baseline and fails the release unless all three hold. After a release, until
the next version bump, the workspace version is still the stamped release and
new notes gather under a non-empty `## [Unreleased]` above it; those notes
declare the breaks since that release, so once HEAD has moved past its tag
the gate measures against the workspace version's own tag instead
(`scripts/semver-baseline.sh`). That is also the state `make release-preflight`
checks. Such a tree is not evidence for the released version: Release semver
readiness measures it without uploading an attestation, and the release
workflow's own measurement (`MEERKAT_SEMVER_REQUIRE_RELEASE_TREE=1`) refuses
it outright. Separately, every publishing run is bound to an allowed tag of
its version: `v<version>` in every mode, and `alpha/v<version>` only for the
crates-only alpha canary (`alpha_crates_only`). That means a tag push of
exactly that ref, or a dispatch whose `release_tag` resolves to that tag and
is the checked-out commit, or a dispatch on that tag ref. `require_ci_green`
refuses anything else, including a branch named after the version (such as
`release/v<version>`), so a branch tip cannot publish under whatever version
its `Cargo.toml` carries.

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
| Release semver readiness | Separate workflow on `Cargo.toml`/`CHANGELOG.md` changes pushed to `main` or a `release/**` branch, or in PRs, plus manual dispatch; measures unpublished candidates and uploads push evidence or preview evidence |
| Release workflow | job `release_semver_gate`, required on tag publication and package recovery; normally consumes unexpired exact-tree, exact-version push readiness evidence from `main` (or from the release branch of the tag's version, for a maintenance release); asset-only and Web-SDK-only recovery explicitly accept a skipped gate |
| Parser self-test | `make semver-breaks-selftest`, included in local `make ci` and the separate reusable/manual Cargo workflow's `ratchets` job: unit-tests the report parser against committed real reports and the baseline selection against scratch repositories, without needing cargo-semver-checks installed |

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
| Python | `pip install meerkat-sdk` | Runs `MEERKAT_BIN_PATH`, an explicit `rkat_path` or `rkat-rpc` on `PATH` as given; otherwise downloads the matching `rkat-rpc` release binary and verifies it against the digests the package pins |
| TypeScript | `npm install @rkat/sdk` | Runs `MEERKAT_BIN_PATH`, an explicit `rkatPath` or `rkat-rpc` on `PATH` as given; otherwise downloads the matching `rkat-rpc` release binary and verifies it against the digests the package pins |

The SDKs are clients. They start or connect to the JSON-RPC surface rather than
embedding a separate runtime implementation.

The digests are pinned when the package is built, from the release's
manifest, with each archive's build attestation verified; they are never
fetched at download time. Before either
SDK package builds, `publish_registries` waits for the release's complete
manifest (`checksums.sha256` and `index.json`;
`scripts/release-await-binary-manifest`), and
`scripts/release-stamp-sdk-binary-digests` re-hashes the five `rkat-rpc`
archives against it, verifies each archive's build attestation
(`gh attestation verify` with `--cert-identity-regex` for the repository's
`release.yml` run from `main` or the release tag,
`--deny-self-hosted-runners` and `--format json`, requiring a verified
subject that names the exact asset, version included, with the archive's
sha256; up to three attempts per archive), takes the digest of the binary
inside each verified archive, and stamps both SDKs. It refuses an incomplete
manifest, an archive that does not re-hash, carries no attestation that
passes or exceeds the SDKs' 256 MiB download cap, and SDK packages of
another version; nothing is stamped then and neither package publishes.
Before any crate publishes, the release preflight fails when
`MEERKAT_CI_RUNNER` (the runner the BuildBuddy build job attests on) names a
label that is not a GitHub-hosted Ubuntu label. It checks the label, not
where the runner is hosted: the stamp's `--deny-self-hosted-runners` stays
the enforcement. Each publish step then refuses a built package that does not pin
exactly those digests for its version. A source or git install pins nothing
and refuses to download (`BINARY_DIGEST_UNAVAILABLE`).

The residual: the pins are only as trustworthy as the release that produced
them. The attested identity names the workflow and its ref, and `main` names
no commit: the release version is bound by the attested subject name. The
GitHub-hosted runner is the job that attested; on a BuildBuddy release the
Linux (x86_64 and arm64) and macOS arm64 binaries are compiled by BuildBuddy
remote execution, so BuildBuddy's executors and cache are part of what the
pins trust. Anyone
able to run this repository's release workflow can produce attested
archives, and a package whose own files were replaced after it was built
pins whatever digests it then carries.

## Release Workflow

1. Prepare the release commit through cargo-release and its release hook, so
   version projections and stamped notes stay in one hook-owned tree. Do not
   manually edit versions or create tags to bypass that workflow.
2. Prefer waiting for both exact-main CI and **Release semver readiness** on
   that prepared tree before cargo-release publishes the tag. Ordinary CI and
   semver readiness produce separate artifacts; one cannot substitute for the
   other. To keep the release commit's CI short, merge the release notes and
   fixes first with the trailer `CI-Plan: workspace` (in the merge commit's
   message or on the merged branch's head commit), so that `main` commit gets
   a whole-workspace CI run, then land the version bump as its own commit on
   top of it once that run succeeded. When the release commit differs from
   its first parent only by the hook's forward patch projection, CI runs the
   release checks (version parity, lock consistency, schema and SDK
   freshness, release packaging and configuration, docs and the Bazel graph)
   and the tests that read the changed files, and reuses the parent's
   whole-workspace run for everything else instead of rebuilding every crate
   at the new version.
   Dispatch the [pre-tag prebuild](#pre-tag-prebuild)
   (`make release-prebuild VERSION=vX.Y.Z`) as soon as the release commit is
   on `main`, so the binaries and the Web SDK package build while CI and
   readiness run, and wait for it too.
3. The tag path verifies the exact-tree CI attestation instead of recomputing
   the broad CI graph: with the self-hosted graph off, an attestation that
   records the generated-contract ratchets and a green `sdk-host` job (a
   release commit's normally does) passes `release_validate_gate` without
   `release_validate_cargo`, so publication starts minutes after the tag when
   CI is already green.
   After `require_ci_green`, `release_semver_gate` looks
   up `meerkat-semver-attestation-main-<tree_sha>` and requires unexpired
   evidence for that tree and version from a successful readiness `main` push
   (or, for a [maintenance release](#maintenance-releases), a push to its
   release branch).
4. In parallel, verify registry credentials, consume declared-break evidence,
   and either find the complete prebuild of the tag commit or build the
   platform binaries and the Web SDK package artifact.
5. Publish the critical Linux (x86_64, aarch64) and macOS arm64 archives as
   soon as their lane passes (`publish_critical_release_assets`), without
   waiting for the trailing Intel macOS and Windows lanes. Those archives
   follow when their lanes finish (`publish_github_release`): added to the
   release, never replacing an asset already on it, then `checksums.sha256`
   and `index.json` for the complete set, written once and never replaced (a
   rerun checks a published manifest instead of uploading it). Until then
   the release is public with the critical archives and no manifest: a few
   minutes when a prebuild's trailing lanes already finished, otherwise
   until the Intel macOS and Windows lanes finish (or an asset recovery adds
   them). Every archive carries a build attestation meanwhile.
6. Update the Homebrew tap from the complete release's published checksums.
7. Publish Rust crates plus the Python and TypeScript SDKs on GitHub-hosted
   runners. The Rust crates publish by dependency level, derived from the
   canonical order in `scripts/release-rust-crates.sh` and `cargo metadata`:
   a level's crates upload concurrently and the next level starts once all of
   them are in the crates.io index. The SDK publish jobs build and smoke-test
   their package artifacts before upload. Before they build, the job stamps
   the release's `rkat-rpc` digests into both SDKs (see
   [SDK Bootstrap](#sdk-bootstrap)): a tag run waits for the complete manifest
   (up to 150 minutes; with a prebuild it is usually there already) and fails
   at once if `publish_github_release` fails, and a package recovery needs the
   manifest already published. Crate publication never waits for it.
8. Publish the prebuilt Web SDK artifact on a GitHub-hosted runner.
9. Off the publication path, build the rustdoc JSON baseline the next
   release's semver gate compares against (`publish_semver_baseline`, which
   starts with the release) and attach it as
   `semver-rustdoc-<version>.tar.zst` once the GitHub release is published
   (`attach_semver_baseline`, add-only: a baseline already attached stays).
   No job waits for either; if it fails, the next measurement rebuilds the
   baseline from the tag instead.

### Pre-tag prebuild

`make release-prebuild VERSION=vX.Y.Z` dispatches `release.yml` on `main` with
`prebuild_release_artifacts=true`. The run refuses any other ref, any
publishing or recovery input, a version whose tag already exists, and a
`release_backend` other than the one its tag run will select (BuildBuddy for
the owner while `MEERKAT_RELEASE_BUILDBUDDY` is `true` or `1`): on the other
backend its critical lanes queue behind other runners and miss the tag run's
wait, as v0.9.3's GitHub-hosted prebuild did. `make release-prebuild` reads that
backend from the repository variable. It
does not wait for CI: it builds every release binary (the critical Linux and
macOS arm64 ones on BuildBuddy or the GitHub-hosted lane, Intel macOS
cross-compiled on Apple silicon, Windows cross-compiled with cargo-xwin and
verified on a Windows runner) and the Web SDK package from exactly the
dispatched commit, stamps each archive with that commit, attests and uploads
everything, and publishes nothing. Its `Release prebuild gate` job passes
once the critical binaries and the Web SDK package succeeded; the trailing
Intel macOS and Windows lanes keep running in the same run.

The tag run looks the prebuild up after `require_ci_green`
(`release_prebuild`, `scripts/release-find-prebuild`): a `release.yml`
dispatch on `main` at exactly the tag commit, in this repository, started
and triggered by the repository owner, whose `Release prebuild gate` passed
and whose critical artifacts have not expired; the publishers verify the run
attempt the lookup examined and refuse it if the run was re-run since;
a gate that has not concluded yet is waited for, because the prebuild
finishes no later than a fresh build would, for up to 140 minutes from the
prebuild's start (`MEERKAT_PREBUILD_WAIT_SECONDS`, default 8400; v0.9.2's
BuildBuddy prebuild gate concluded after 94.7 minutes), unless a job the gate
needs (the request check, a critical build, the Web SDK package or the
critical binary build gate) already failed, timed out or was cancelled, so
the gate can no longer pass. When it finds one, the tag run
skips every build job and publishes that run's artifacts after the same
verification as an `--assets-run-id` recovery
(`verify-release-asset-provenance --source-run`: every archive and its stamp
attested by `release.yml` at that commit on `refs/heads/main`, the stamp
naming the tag commit, every binary embedding the release version; a
prebuild run is accepted as a source only by the tag run of exactly its
commit, never by an asset recovery, because a prebuild does not wait for
exact-main CI):
`publish_critical_release_assets` publishes the critical archives at once,
and `publish_github_release` adds the trailing ones after the prebuild's
`Release binary build gate` passed (if that run's trailing lanes failed, an
asset recovery publishes them). `publish_web_sdk` verifies the package's
attestation and version before publishing. Without a usable prebuild the tag
builds everything as before. The prebuild never relaxes a publication gate:
exact-main CI, semver evidence and registry credentials are still required by
the tag run itself.

The default `./scripts/repo-cargo release patch --execute` flow combines
bump, commit, tag, and push; that command summary alone does not establish
readiness. A combined push can succeed if readiness completes before the
semver artifact lookup, but it races that lookup. The gate checks evidence
availability then, not whether readiness completed before tag creation.
Readiness evidence is configured to expire after 30 days. PR and manually
dispatched readiness artifacts use the `meerkat-semver-attestation-preview-`
prefix and do not qualify for the normal tag path.

The current schema-4 CI attestation binds the repository, commit SHA, Git tree
SHA, CI workflow run and attempt, branch, and event to backend
`github-hosted-cargo`, its aggregate `validation_result`, the selected lane
`plan`, and every lane's `component_results`. The release gate downloads it
from the successful exact-main workflow run (for a maintenance release, the
push run on its release branch) and verifies every required field
before tag-triggered publication starts. A release-projection run (plan mode
`release-projection`) also records `reused_test_evidence`: the first-parent
commit and tree and the CI run and attempt it reused. The release gate then
runs the parent's own copies of the classifier and the evidence script to
check that the release commit is still a projection of that parent, and that
the recorded attempt is a completed, successful push run of `ci.yml` on
`main` at exactly that parent whose own attestation planned the whole
workspace, records every lane as successful and is not itself a reuse. The consumer also accepts the
supported legacy schema-1 Cargo, schema-2 BuildBuddy, and schema-3
BuildBuddy-plus-dense-Mob attestations; the current producer does not emit
those formats.
Manual recovery dispatches still require successful exact-main CI (for a
maintenance release, the push CI on its release branch) for the
selected release commit. When release validation applies, the dispatch runs
the selected validation lane directly rather than requiring a retained **CI**
attestation artifact. Narrow `assets`,
`packages`, `web-sdk`, and `web-sdk-publish` modes repair one publication
surface without rebuilding or republishing unrelated surfaces.

An `assets` recovery runs main's workflow against the tag, so its build
attestations name main's commit. Each build job therefore checks that its
checkout is the tag commit and attests a `<archive>.source-commit` stamp
naming it; the publisher requires every archive and stamp to be attested by
`release.yml` at the run's commit on `refs/heads/main`, and the stamp to name
the tag commit. When an earlier `assets` run built everything but failed to
publish, `--assets-run-id RUN_ID` (`ASSETS_RUN_ID=RUN_ID make
release-assets`) publishes that run's archives without rebuilding: the run
must be a `release.yml` dispatch on main at a commit in main's history whose
`Release binary build gate` succeeded, and every archive must be attested by
`release.yml` at that commit on `refs/heads/main`. A run from before archives
were stamped is accepted only if every path changed between the tag and its
commit is under `.github/` or `CHANGELOG.md`. Every published binary must
embed the release version in either case.

This CI-artifact exception does not bypass the semver gate. An ordinary
explicit-`release_tag` package recovery still consumes push readiness
evidence (`meerkat-semver-attestation-main-<tree>`, from `main` or the
release branch of the tag's version). When this gate applies, a manual
dispatch with neither `release_tag`
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

### Maintenance releases

A maintenance release ships a fix on top of an earlier release without the
unreleased changes on `main`. It is cut from that release's tag as the
branch `release/<version>`, named after the version it releases; the fix and
the cargo-release bump are merged there, and the tag goes on that branch's
head, which never reaches `main` before the tag.

- A push to `release/<version>` writes the exact-tree CI attestation and,
  for a `Cargo.toml` or `CHANGELOG.md` change, the readiness push evidence,
  as a push to `main` does. `require_ci_green` accepts a successful push run
  of `ci.yml` at exactly the release commit on `main` or on
  `release/<version>`, where `<version>` is the checked-out workspace version
  the tag is bound to, and requires the verified attestation's ref to be
  one of those two; `release_semver_gate` requires the same of the readiness
  evidence's ref and of its run's branch. No other branch qualifies, another
  version's release branch included, and pull-request, dispatch and preview
  runs never do.
- A release-branch push diffs against its payload base and never reuses a
  release projection's evidence, so give the release commit the trailer
  `CI-Plan: workspace` to plan the whole workspace.
- There is no pre-tag prebuild (`make release-prebuild` runs on `main` only),
  so the tag run builds every binary.
- Recovery dispatches run as for any release: the default `main` ref
  carries these rules and accepts such a tag
  (`VERSION=vX.Y.Z make release-assets`, and likewise `release-packages`
  and `release-web-sdk`; inspect each first with
  `RELEASE_WORKFLOW_DRY_RUN=true`). `RELEASE_REF=vX.Y.Z` runs the tag's own
  workflow instead, which carries the same rules, and `ASSETS_RUN_ID` reuse
  is for `main` only.
- After publication, merge the tag into `main` by pull request, so that
  `main` carries the version, the stamped notes and the tag as an ancestor,
  which the next post-release semver baseline needs. Never set `main`'s
  version to the released one without that merge.

## BuildBuddy

Automatic tag packaging selects BuildBuddy only when `github.actor` is
literally `lukacf` and the repository variable
`MEERKAT_RELEASE_BUILDBUDDY` is `true` or `1`. For eligible manual dispatches,
explicit `release_backend=buildbuddy` selects BuildBuddy validation and
Linux and macOS arm64 packaging only for that same literal actor; it does not also
require the automatic-selection variable. This is a named-actor condition,
not a general owner/admin permission check.

The following explicit BuildBuddy dispatch is for `lukacf`:

```bash
RELEASE_BACKEND=buildbuddy make release-workflow VERSION=vX.Y.Z
```

On eligible manual dispatches, other actors use GitHub-hosted Cargo validation
and Linux and macOS arm64 packaging even when they request
`release_backend=buildbuddy`. Windows and Intel macOS packaging and every
credentialed registry publish still run on GitHub-hosted runners, and the exact-main GitHub Actions CI requirement remains
in force. This release
backend selection is separate from per-push CI, which is Cargo-only on
GitHub-hosted runners; the full BuildBuddy graph runs nightly and on the tag
while `MEERKAT_GCP_BUILDBUDDY_ENABLED` is `true`.

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

The release workflow's own `GITHUB_TOKEN` is read-only by default: only the
build jobs may mint OIDC tokens and write build attestations, only the
release publishers may write the GitHub release, and neither the build nor
the publisher checkouts persist the token. The full-graph validation passes
`buildbuddy.yml` only the `MEERKAT_GCP_BUILDBUDDY_API_KEY` secret, and no
workflow in the repository uses `secrets: inherit`: the nightly full graph
lists the secrets `buildbuddy.yml` declares (`scripts/release-doctor` checks
all of this). No GitHub
Actions cache step in its jobs, or in the local actions they call (the
Windows SDK cache of `build-windows-cross`, the Bazel repository cache of
`setup-buildbuddy-ci`), is saved by a dispatch, which would write the `main`
scope every tag run restores from; no rust-cache stores `~/.cargo/bin`, so no
restored binary can shadow a tool on `PATH`; the jobs that hold registry or
publish tokens restore no cache at all; and the jobs that mint OIDC tokens or
attest artifacts restore only caches that no workflow in the repository saves
on a dispatch-like run (any trigger but `push`, `pull_request`, `schedule`
and `merge_group`). The workflows whose runs a tag trusts (`ci.yml`, which
writes the exact-tree attestation, and `release-semver-readiness.yml`,
which writes the semver attestation) and any workflow that checks out a
ref its inputs choose save no cache on a dispatch-like run either. No
release job restores a cache, on a tag or on a dispatch (a prebuild, a
recovery or the semver gate's measurement, which builds in a per-run
target directory), and neither does `release-semver-readiness.yml`'s
measurement, which builds the same way, on any of its triggers: the
release jobs run no
rust-cache step, `build-windows-cross` restores the Windows SDK cache only
for a caller that saves it (release.yml does not), the BuildBuddy release
jobs pass `repository-cache: "false"` and the full-graph validation
passes `buildbuddy.yml` `repository_cache: "false"`.
No workflow saves that cache on a dispatch-like run either
(`release-binaries-check.yml` and `release-turbo-s.yml` only restore it).
The doctor reads release.yml, the local actions it calls and, with the
caller's inputs, the reusable `buildbuddy.yml` that the full-graph
validation calls only on tag pushes; it does not cover the BuildBuddy remote
cache.
The macOS checks call the Apple tools by absolute path. The Web SDK package
job installs wasm-pack, wasm-bindgen (the version Cargo.lock locks) and
wasm-opt pinned by sha256 and runs wasm-pack with `--mode no-install`
(`MEERKAT_WEB_WASM_PACK_MODE`), so wasm-pack downloads nothing; a wasm-bindgen
bump in Cargo.lock must bump that pin (the doctor's `web-sdk-tools` check).

## Hard Rules

- Release only from tagged versions.
- Never publish mismatched Rust, Python, TypeScript, or contract versions.
- Never publish SDKs from a commit with stale generated schema artifacts.
- Keep public binary names stable: `rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`.
- Publish checksums and an index for release binary consumers.

## See Also

- [Build and CI](/reference/build-and-ci)
- [CLI commands](/cli/commands)

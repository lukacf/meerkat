# BuildBuddy Bazel Developer And CI Guide

Status: Production rollout guidance for the opt-in BuildBuddy/Bazel workspace
backend. Cargo remains the default developer path.
Baseline branch: `codex/buildbuddy-poc`
Initial POC commit: `a58e2613f1a5f3cfb952b4035e8f0d38c0c35a71`

## Goals

- Use Bazel remote execution and caching on macOS arm64 executors.
- Keep inner-loop commands honest: package-owned feedback, reverse-dependency
  affected feedback, full fast tests, and full clippy.
- Avoid same-checkout and multi-worktree build contention when multiple agents
  edit and test concurrently.
- Keep CI-like runs useful on fresh local output bases by leaning on remote
  cache hits.
- Preserve Cargo as the normal path for developers without BuildBuddy access.

## Local Developer DX

Install the pinned `bb` binary with `make buildbuddy-install` if it is not
already on `PATH`. The installer writes to the user cache by default and checks
the downloaded binary against a pinned SHA-256 digest.

For normal work, use the same Make targets as Cargo. They run Cargo by default
and switch to BuildBuddy only when `MEERKAT_BUILDBUDDY=1` is set:

| Cargo-default command | BuildBuddy opt-in |
| --- | --- |
| `make build` | `MEERKAT_BUILDBUDDY=1 make build` |
| `make check` | `MEERKAT_BUILDBUDDY=1 make check` |
| `make lint` | `MEERKAT_BUILDBUDDY=1 make lint` |
| `make test` | `MEERKAT_BUILDBUDDY=1 make test` |
| `make test-unit` | `MEERKAT_BUILDBUDDY=1 make test-unit` |
| `make test-int` | `MEERKAT_BUILDBUDDY=1 make test-int` |
| `make e2e-fast` | `MEERKAT_BUILDBUDDY=1 make e2e-fast` |
| `make e2e-system` | `MEERKAT_BUILDBUDDY=1 make e2e-system` |
| `make e2e-live` | `MEERKAT_BUILDBUDDY=1 make e2e-live` |
| `make e2e-smoke` | `MEERKAT_BUILDBUDDY=1 make e2e-smoke` |

The explicit BuildBuddy targets are:

- `make buildbuddy-build`: Build the generated Rust workspace labels.
- `make buildbuddy-check`: Same BuildBuddy compile lane, named for Cargo users
  reaching for `cargo check`.
- `make buildbuddy-clippy`: Run rules_rust clippy with `-D warnings`.
- `make buildbuddy-test`: Run unit + integration-fast lanes.
- `make buildbuddy-test-unit`: Run unit tests.
- `make buildbuddy-test-int`: Run integration-fast tests.
- `make buildbuddy-e2e-fast`: Run the deterministic e2e lane.
- `make buildbuddy-e2e-system`: Run the real-local-resource e2e lane.
- `make buildbuddy-e2e-live`: Run targeted live-provider e2e tests.
- `make buildbuddy-e2e-smoke`: Run kitchen-sink live smoke tests.

`BUILDBUDDY_ARGS='...'` is forwarded to `scripts/buildbuddy-dev` for explicit
targets. The lower-level facade also supports `--dry-run`, `--jobs N`, and an
optional explicit Bazel target:

```bash
BUILDBUDDY_DRY_RUN=1 make buildbuddy-test
scripts/buildbuddy-dev --dry-run build
scripts/buildbuddy-dev build //meerkat-cli:rkat
scripts/buildbuddy-dev clippy --keep_going
```

Run `make buildbuddy-doctor` when setup looks suspicious. It verifies
credentials, the pinned `bb` CLI, generated Bazel file freshness, selector
health, and lane isolation without printing secrets.

## Lower-Level Bazel Commands

Most developers should prefer the Make targets or `scripts/buildbuddy-dev`.
When extending the BuildBuddy integration, use `scripts/buildbuddy-bazel-poc`
with `BUILDBUDDY_BAZEL_COMMAND`:

- `workspace-test`: run the full Bazel workspace test suite.
- `workspace-test-rbe`: run the remote-compatible workspace test suite,
  excluding local-only Cargo/trybuild fixtures.
- `workspace-fast-rbe`: run the Cargo-fast-equivalent remote workspace suite,
  excluding local-only fixtures and dedicated e2e lane wrappers.
- `workspace-test-local`: run the full workspace test suite with local spawns.
- `workspace-fast-clippy-rbe`: run the Cargo-fast-equivalent remote workspace
  suite with the clippy aspect attached.
- `workspace-build-clippy-rbe`: run the clippy aspect for non-test build
  targets; used with `workspace-fast-clippy-rbe` to cover the full workspace
  clippy surface without rechecking test targets twice.
- `clippy-rbe`: run the rules_rust clippy aspect with `-D warnings`,
  excluding local-only, manual, and `noclippy` targets.
- `owned-clippy-rbe <path>`: run clippy for the owning build/test labels for a
  changed path, excluding local-only trybuild compile-fail tests.
- `affected-clippy-rbe <path>`: run clippy for the reverse-dependency closure of
  a changed path.
- `owned-build <path>`: build the owning package target for a changed path.
- `affected-build <path>`: build the reverse-dependency closure for a changed path.
- `owned-fast-test <path>`: run the owning fast suite, or the exact test target
  when `<path>` is an integration test crate root.
- `affected-fast-test <path>`: run fast suites for the reverse-dependency closure.
- `fast-test`: run the root `//:fast_tests` suite.
- `clippy`: run the rules_rust clippy aspect with `-D warnings`.
- `make buildbuddy-agent-gate`: derive build-relevant changed files from the
  current branch and working tree, then run the changed-path gate. Global
  Cargo/Bazel/nextest configuration changes escalate to `make buildbuddy-ci-warm`.
- `scripts/buildbuddy-agent-gate --dry-run ...`: print the selected paths,
  Bazel labels, and whether the changed gate will run combined test+clippy,
  combined build+clippy, or split lanes.

For local-spawn variants, append `-local` to the command name where available.
Remote execution remains the default because local-spawn often spends time
downloading large remote-cache inputs before rustc can run.

For a one-shot edit feedback gate, use `scripts/buildbuddy-changed-gate
--owned <path>`. It runs the selected fast test lane and selected clippy lane in
parallel. When both selectors resolve to the same test labels, it collapses the
gate into one `bb test` invocation with the clippy aspect attached. Use
`--affected` when the edit needs reverse-dependency confidence, and
`--local-test` when the selected test lane is already warm locally. For source
edits whose owning package has no fast tests, the gate runs the owning build and
owning clippy labels instead of falling back to the full root fast suite.
Use `--dry-run` to inspect the exact labels and plan before spending remote
execution, especially when several agents are changing the same checkout.

## Lane Isolation

The wrapper assigns a default Bazel `--output_base` outside the repo:

```text
${XDG_CACHE_HOME:-$HOME/.cache}/meerkat/buildbuddy-bazel/<workspace>-<workspace-hash>-<lane>
```

The lane key includes:

- the requested command and Bazel config,
- `BUILDBUDDY_OUTPUT_LANE`, if explicitly set, or
- `BUILDBUDDY_AGENT_LANE`, `RUST_LANE_ID`, or `CODEX_AGENT_ID`, when present.

This matters for same-worktree multi-agent runs. Two agents running the same
command in the same checkout should set distinct `RUST_LANE_ID` values for
stable warm lanes. `scripts/buildbuddy-changed-gate` falls back to a
process-scoped lane when no `RUST_LANE_ID`, `BUILDBUDDY_AGENT_LANE`, or
`CODEX_AGENT_ID` is set, which avoids same-checkout lock contention at the cost
of less cache warmth between repeated manual runs. Separate Git worktrees
automatically get different workspace hashes, so they do not share local Bazel
output bases.

`scripts/buildbuddy-bazel-poc` snapshots `MODULE.bazel.lock`, runs Bazel with
`--lockfile_mode=error` against checked-in metadata, and restores the checked-in
lockfile after Bazel exits so a successful BuildBuddy build does not dirty the
worktree. Persistent BUILD regeneration and `MODULE.bazel.lock` updates are
explicit maintenance steps, not normal local lane behavior.

## Simulation Harness

`scripts/buildbuddy-simulate-scenarios.mjs` exercises the expected operating
modes:

- `workspace-test`: full Bazel workspace test suite.
- `workspace-test-rbe`: remote-compatible workspace suite excluding local-only
  fixtures.
- `workspace-fast-rbe`: Cargo-fast-equivalent remote workspace suite excluding
  local-only fixtures plus e2e, system, live, integration, slow, snapshot,
  fixture, trybuild, and required-feature test targets.
- `workspace-test-local`: full Bazel workspace test suite with local spawns.
- `warm-noop`: warm full fast-test and clippy checks.
- `same-worktree`: two same-checkout agents using distinct lanes.
- `same-command`: two same-checkout agents running the same command with
  distinct lanes.
- `support-file`: exact selector behavior for a shared integration-test support
  file, measured with remote and local-spawn execution.
- `changed-clippy`: changed-scope clippy for representative source and shared
  support-file edits.
- `changed-gate`: combined changed-path test and clippy gate.
- `required-feature-gate`: exact build+clippy for a non-fast required-feature
  test, without executing the live/system lane.
- `optional-required-feature-gate`: exact build+clippy for a required-feature
  test whose feature enables an optional external crate.
- `agent-gate`: auto-detected changed-path agent gate for a representative
  source edit.
- `agent-gate-global`: auto-detected global-change escalation to the warmed
  workspace gate.
- `source-gate`: combined changed-path build and clippy gate for a source edit
  with no owning fast tests.
- `parallel-gates`: two same-worktree agents running changed-path gates in
  parallel with distinct lanes.
- `parallel-gates-auto`: two same-worktree changed-path gates without explicit
  lanes; each gate chooses a process-scoped fallback lane.
- `edit-probes`: creates a temporary detached worktree, makes harmless real
  edits to representative source/test/support files, runs the matching lanes,
  and removes the worktree.
- `edit-probes-warmed`: runs the same edit probes after prewarming the relevant
  lanes in the temporary worktree, separating first-touch materialization from
  steady-state edit feedback.
- `prewarm-dev`: runs the shared dev prewarm profile.
- `prewarm-ci`: runs the shared CI prewarm profile.
- `multi-worktree`: two temporary Git worktrees, each with its own lane.
- `multi-worktree-gates`: two temporary Git worktrees running changed-path
  gates in parallel.
- `ci-cold`: sequential CI-like fast-test and clippy on fresh output bases.
- `ci-parallel`: parallel CI-like fast-test and clippy on fresh output bases.
- `ci-workspace`: parallel CI-like `workspace-fast-clippy-rbe` and
  `workspace-build-clippy-rbe` on fresh output bases.
- `ci-dispatch-artifacts`: changed-path CI dispatch dry-run with log-root
  context artifact verification.

For an actual remote-compatible CI gate, use `scripts/buildbuddy-ci-workspace`.
It runs `workspace-fast-clippy-rbe` and `workspace-build-clippy-rbe` in parallel
on isolated fresh lanes, writes compact summaries and per-lane logs outside the
Bazel output root, and cleans up the temporary output roots. Use `--warm` to
reuse a stable output root for repeated local or agent gates. The optional
`.github/workflows/buildbuddy.yml` workflow is the reusable GCP BuildBuddy lane
called by CI. Top-level GitHub CI exposes two paths:

- `cargo`: the default GitHub Actions Cargo workflow for pushes, pull requests,
  and manual dispatches.
- `gcp-buildbuddy`: the owner-gated GCP self-hosted BuildBuddy control plane and
  executor fleet.

Manual `CI` workflow dispatch can run the GCP BuildBuddy lane, but the
secret-backed lane is restricted to the repository owner.

The dispatch modes are:

- `workspace-fresh`: full optional BuildBuddy workspace CI with fresh output
  roots.
- `workspace-warm`: full optional BuildBuddy workspace CI with a reusable output
  root.
- `changed-committed`: local changed-path BuildBuddy gate for branch commits
  relative to a base revision, via `scripts/buildbuddy-ci-dispatch`.
- `changed-paths`: local changed-path BuildBuddy gate for explicit whitespace or
  newline separated paths, via `scripts/buildbuddy-ci-dispatch`.

Use `owned` for the smallest honest changed-path closure, or `affected` when a
change touches shared crates and you want reverse-dependency confidence.

`scripts/buildbuddy-prewarm-lanes` prepares common lanes for a new worktree:

- `dev`: source-owned-build, changed-clippy, exact-test, and support-local
  feedback lanes.
- `ci`: split workspace fast test+clippy and build-clippy lanes in parallel.
- `ci-fast`: fast-test and clippy-RBE lanes in parallel.

The prewarm helper uses direct Bazel labels instead of path selectors so several
startup lanes do not contend on Cargo metadata/package-cache locks.
Path selectors cache `cargo metadata` under
`${XDG_CACHE_HOME:-$HOME/.cache}/meerkat/bazel-selector-metadata` and use a
small first-populator lock, so simultaneous agents do not serialize on Cargo
metadata after the cache is warm.

## GCP CI Shape

When the owner manually dispatches the GCP BuildBuddy lane, GitHub Actions is
only the submitter/coordinator:

1. `gcp-control-plane-up` authenticates to GCP, ensures the always-on
   self-hosted BuildBuddy control plane/cache VM is running, and publishes its
   HTTP and gRPC endpoints to downstream jobs.
2. `gcp-executors-up` syncs the self-hosted BuildBuddy gRPC URL/API key into
   Secret Manager for the executors, creates the executor managed instance group
   if it is missing, and scales it to
   `MEERKAT_GCP_BUILDBUDDY_TARGET_SIZE` (default `12`).
3. `prebuild-submit` runs one remote Bazel build of the non-live workspace
   graph. This is the cache-warming step: one Bazel invocation fans out
   compile/link/test-binary actions across the GCP executor pool, so common
   artifacts enter the self-hosted BuildBuddy CAS before targeted validation
   lanes start. It deliberately does not execute TLC/RMAT/governance or
   cargo-equivalent WASM/SDK wrappers; those are policy/shell lanes, not the
   shared Bazel-native compilation graph.
4. The static submitter runs format/static source checks in parallel with the
   prebuild because it does not consume warmed Rust test binaries. The native
   submitter then runs clippy, unit, and integration-fast checks against the
   warmed graph. It uses a small number of local lane submitters so the
   post-prebuild validation lanes can overlap without several GitHub jobs racing
   the same first-touch compile actions. Governance and
   WASM/SDK/feature/audit are path-aware edge lanes;
   they start in parallel after the executor pool is up and do not wait for the
   prebuild, because they do not consume the shared Bazel-native prebuild output.
   Governance is intentionally keyed to the machine DSL/generated-machine
   surfaces, not broad Bazel metadata such as `MODULE.bazel.lock`.
5. Bazel uses `--config=buildbuddy-linux-gcp-ci-rbe`, which selects
   `//platforms:linux_x86_64_gcp_ci`. That platform routes actions to
   BuildBuddy pool `meerkat-ci`, enables external network, and runs actions in
   the pre-baked Artifact Registry image
   `europe-west1-docker.pkg.dev/king-dnn-training-dev/meerkat-ci/meerkat-ci-rust:1.94.0`.
6. `executors-down` parks the managed instance group with stopped standby
   VMs after the submitters finish. In GitHub CI it sets running size to zero,
   preserves the configured stopped size, and can skip waiting for every stop to
   settle so teardown does not inflate the required CI status. Manual
   `scripts/gcp-buildbuddy-executor-pool down` still waits by default; set
   `MEERKAT_GCP_BUILDBUDDY_WAIT_ON_DOWN=0` for the asynchronous behavior.

The intended steady-state cost shape is one small always-on BuildBuddy
backend/cache plus stopped executor VMs with durable cache disks. A CI run
starts the parked executor MIG to the configured running size, then parks it
again. BuildBuddy CAS/action cache is the shared cache across the fleet; each
executor also has its own stateful SSD-backed `/buildbuddy` disk for
per-executor action/container/file-cache locality. The cache disk is not a
multi-writer shared filesystem; sharing happens through BuildBuddy CAS/action
cache, while stopped stateful disks avoid re-cold-starting every executor.
The `gcp-buildbuddy` submitter jobs restore GitHub Actions' Bazel repository
cache for fast local analysis/fetch setup, but intentionally skip the cache save
at job teardown because uploading runner-local cache tarballs adds latency and
duplicates the self-hosted BuildBuddy cache.

The control-plane and executor-pool scripts are intentionally idempotent and
secret-safe:

```bash
scripts/gcp-buildbuddy-control-plane status
scripts/gcp-buildbuddy-control-plane ensure
scripts/gcp-buildbuddy-control-plane endpoint
scripts/gcp-buildbuddy-executor-pool status
scripts/gcp-buildbuddy-executor-pool sync-secrets
scripts/gcp-buildbuddy-executor-pool up
scripts/gcp-buildbuddy-executor-pool down
```

The GCP platform intentionally does not set BuildBuddy Cloud's
`use-self-hosted-executors` execution property; on the self-hosted control plane
the GCP executors are the native scheduler pool.

The image bake is explicit and separate from per-PR CI. Rebuild it when
`Cargo.lock`, crate manifests, Rust toolchain, Node/Python versions, or CI tool
versions change:

```bash
scripts/gcp-buildbuddy-ci-image build
```

`scripts/gcp-buildbuddy-ci-image` creates a manifest-only Docker context,
fetches the locked Rust dependency graph for Linux and wasm targets into
`/usr/local/cargo`, and removes the temporary source context from the final
image. The Bazel cargo-equivalent lanes receive
`MEERKAT_HOST_CARGO_HOME=/usr/local/cargo` so nested Cargo checks use the baked
registry instead of starting from an empty test sandbox.

`oai-rt-rs` resolves from crates.io, so the lockfile no longer carries a
worktree-local vendored path or a generated vendored BUILD file. The wrapper
still snapshots and restores `MODULE.bazel.lock` around the invocation as a
defensive measure, including failed analysis exits.

When Cargo metadata changes require a persistent refresh, run
`make buildbuddy-generate` for checked-in BUILD files and refresh
`MODULE.bazel.lock` intentionally with the BuildBuddy CLI. Review the lockfile
diff before committing it.

The macOS RBE lanes also pin the Apple SDK selection to the current enterprise
executor pool default (`BUILDBUDDY_MACOS_SDK_VERSION`, default `26.2`) and a
modern deployment target (`BUILDBUDDY_MACOS_MINIMUM_OS`, default `13.0`). This
avoids apple_support falling back to `MacOSX10.11`, which is not installed on
the current arm64 macOS executors.

Generated `rust_test` targets only carry `:package_runfiles` when their source
appears to read package/workspace files at runtime. This keeps ordinary tests
from being invalidated by unrelated non-source package files while preserving
runtime data for tests that scan source trees, read manifests, load skills, or
inspect workspace fixtures.

## Observed Results

Representative measurements from the POC environment:

Apples-to-apples Cargo vs BuildBuddy fast lanes after excluding dedicated e2e
lane wrappers from both systems:

| Scenario | Cargo | BuildBuddy/Bazel |
| --- | ---: | ---: |
| Fast deterministic tests, compile/no-run first pass | `122.71s` | `30.47s` first new Bazel lane run |
| Fast deterministic tests, warm execution | `47.96s` (`5112` tests) | `4.36s` (now `117` Bazel fast test targets, `0` executed when cached) |
| Fast deterministic tests, latest artifacted warm run | `29s` (`5114` tests) | `4s` (`117` Bazel fast targets, `0` executed when cached) |
| Fast deterministic tests after disabling duplicate mini bin harnesses | `18s` (`4973` tests) | unchanged |
| Fast deterministic tests after pruning retry-backoff hotspots | `12s` (`4973` tests) | unchanged |
| Workspace clippy, warm after fix | `48.13s` | `4s` split fast-test-clippy + build-clippy gate |

The Cargo fast lane is now `./scripts/repo-cargo fast`, `cargo rct`, or
`make test`. It excludes `e2e_*_lane` wrappers; those remain explicit lanes via
`cargo e2e-fast`, `cargo e2e-system`, `cargo e2e-live`, `cargo e2e-smoke`,
`cargo e2e-models`, and `cargo e2e-auth`. The BuildBuddy equivalent is
`make buildbuddy-fast` or
`BUILDBUDDY_BAZEL_COMMAND=workspace-fast-rbe scripts/buildbuddy-bazel-poc`.
The workspace-fast BuildBuddy command uses the same non-fast tag exclusions as
the generated root `//:fast_tests` suite, so newly generated required-feature
targets are available for exact build/clippy gates without joining the default
fast execution lane.
The Cargo exclusion filter lives in `.config/nextest.toml` as the `fast`
profile so `cargo fast`, `cargo rct`, `cargo int`, and
`scripts/cargo-fast-nextest` do not maintain separate copies.

| Scenario | Result |
| --- | ---: |
| Full workspace test lane (`152` tests), remote/cache | `23.25s` wall |
| Full workspace test lane (`152` tests), warm remote/cache | `3.96-5.33s` wall |
| Remote-compatible workspace lane (`151` tests), first touch | `22.27s` wall |
| Remote-compatible workspace lane (`151` tests), warm | `4.10-4.38s` wall |
| Cargo-fast-equivalent workspace lane (`117` Bazel fast targets), first new lane run | `30.47s` wall |
| Cargo-fast-equivalent workspace lane (`117` Bazel fast targets), warm | `4.36s` wall |
| Full workspace test lane (`152` tests), local-spawn first pass | `67.01s` wall |
| Full workspace test lane (`152` tests), warm local-spawn | `4.75s` wall |
| Warm root fast suite (`117` tests) | `3.99s` wall |
| Warm full clippy aspect (`288` targets) | `4.99s` wall |
| Same-worktree, two different warmed lanes | `6.42s` / `6.70s` wall |
| Same-worktree, same command, warmed lanes | `4.44s` / `4.34s` wall |
| Shared test-support edit, exact remote selector | `19.10s` wall |
| Shared test-support edit, exact local-spawn selector | `8.25s` wall |
| Changed source clippy, owned selector, first touch | `16.95s` wall |
| Changed source clippy, owned selector, warm | `0.50s` wall |
| Shared support clippy, exact owned selector, first touch | `21.77s` wall |
| Shared support clippy, exact owned selector, warm | `0.91s` wall |
| Changed support gate, combined exact tests + exact clippy, first touch | `20-20.88s` script wall |
| Changed support gate, combined exact tests + exact clippy, warm | `3.85-3.856s` script wall |
| Changed source gate, combined owning build + clippy, first touch | `16.97-17.16s` script wall |
| Changed source gate, combined owning build + clippy, warm | `3.79-3.927s` script wall |
| Agent gate, owned source path (`meerkat-runtime/src/input_ledger.rs`) | `23s` wall, selected `24` tests / clippy |
| Agent gate, global BuildBuddy/Makefile edit escalation | `39s` warmed workspace wall after script edits |
| Two same-worktree changed gates, combined first touch | `24.91s` / `25.10s` wall |
| Two same-worktree changed gates, combined warm | `4.47s` / `4.63s` wall |
| Two same-worktree changed gates, auto fallback lanes | `28.37s` / `28.59s` wall |
| Two temp worktree changed gates, source/support | `27.61s` / `33.29s` wall |
| Three cold parallel selectors with metadata cache lock | `0` Cargo lock waits |
| Package non-source probe after runfiles trim | `13/151` tests executed, `53` actions |
| Fresh temp worktree source edit probe | `23.98s` wall |
| Fresh temp worktree exact-test edit probe | `48.31s` wall |
| Fresh temp worktree support-local edit probe | `49.99s` wall |
| Prewarmed temp worktree source edit probe | `3.85s` wall |
| Prewarmed temp worktree exact-test edit probe | `5.62s` wall |
| Prewarmed temp worktree support-local edit probe | `5.62s` wall |
| Direct-label dev prewarm with changed-clippy lanes, first touch | `46.73s` wall |
| Direct-label dev prewarm with changed-clippy lanes, warm | `4.729s` wall |
| Direct-label CI prewarm, first touch | `29.88s` wall |
| Direct-label CI prewarm, warm | `6.15s` wall |
| Multi-worktree first-touch lanes | `38.35s` / `44.00s` wall |
| CI-like sequential fresh output bases | `25.48s` fast-test + `26.89s` clippy |
| CI-like parallel fresh output bases | `33.28s` max wall |
| CI-like workspace-RBE + clippy-RBE, fresh output bases | `26-44.33s` max wall |
| Dedicated `buildbuddy-ci-workspace` script, first after script/doc edits | `45s` script wall |
| Dedicated `buildbuddy-ci-workspace` script, remote cache warm | `28-30s` script wall |
| Dedicated `buildbuddy-ci-workspace --warm`, warm | `6-8s` script wall |
| Dedicated `buildbuddy-ci-workspace --warm` with external CI logs, warm | `5s` script wall; both split lanes cached |
| Split fast workspace CI after lane change, first warm-root run | `51s` script wall |
| Split fast workspace CI after lane change, warm | `4s` script wall |
| Warm workspace CI after dispatch/test-speed hardening | `9s` script wall, `140` tests pass |
| Exact BuildBuddy gate for `meerkat_machine` test edit | `21s` wall, `1` exact test + clippy combined |
| Cargo agent gate for `meerkat_machine` test edit, tests only | `0.10s` Cargo work, `0.258s` nextest runtime, `37` tests pass |
| Cargo agent gate for `meerkat_machine` test edit, clippy only | `0.20s` Cargo work |
| Cargo agent gate for shared `test_session_store` support edit, first touch | `25.39s` Cargo work, `0.031s` nextest runtime, `6` tests pass |
| Cargo agent gate for shared `test_session_store` support edit, warm after shared selector extraction | `0.15s` Cargo work, `0.045s` nextest runtime, `6` tests pass |
| Cargo agent gate for shared `test_session_store` support edit, exact clippy warm | `0.18s` Cargo work |
| BuildBuddy changed gate for shared `test_session_store` support edit after shared selector extraction | `27s` wall, `2` exact Bazel tests + clippy combined |
| BuildBuddy broad CLI/store source changed gate, parallel split first-touch | `154s` wall; test/clippy duplicated ~1300 first-touch actions |
| BuildBuddy broad CLI/store source changed gate, serial split after cache-aware routing | `48s` wall; test lane warms remote cache before clippy |
| BuildBuddy changed gate for required-feature `cli_mobpack_live_smoke` edit before Bazel required-feature test generation | `25s` wall, package build + clippy only, no unrelated fast tests |
| BuildBuddy changed gate for required-feature `cli_mobpack_live_smoke` edit after Bazel required-feature test generation | `130s` first touch after graph change, `21-23s` warm; exact test target build + clippy, no live test execution |
| BuildBuddy changed gate for required-feature `machines_contracts` edit with optional external deps | `39s` first successful exact build + clippy, `22s` warm |
| BuildBuddy fast lane after non-fast tag hardening | `32.387s` first run after graph/filter changes; `117` fast targets pass, `7` executed |
| BuildBuddy fast lane after marking generated fast tests `small` | `11.219s`; `117` fast targets executed and passed, no oversized-test warning |
| BuildBuddy fast lane with benchmark context artifacts | `8s` script wall; Bazel elapsed `4.726s`; `117` fast targets pass, `1` executed |

The first touch of a new local lane pays Bazel analysis and remote-cache
materialization cost. Once warmed, the wall-clock floor is mostly the `bb`/Bazel
client startup path rather than Rust compilation.

The full workspace lane runs the trybuild compile-fail fixture locally because
trybuild shells out to Cargo in offline mode and expects a host Cargo registry
cache. The rest of the suite remains eligible for remote execution/cache hits.

For new agent worktrees, prewarming the small set of expected lanes is worth it:
the edit-probe harness showed first edit feedback dropping from tens of seconds
to roughly `4-6s` once those lanes were prepared.

## Current Guidance

- For leaf or package-local edits, start with `owned-build` or
  `owned-fast-test`.
- For an integration test file edit, pass the test file path to
  `owned-fast-test`; the selector targets the exact Bazel test when it is fast.
  Non-fast e2e/system/live test edits stay honest by compiling and linting the
  exact Bazel test target when one exists, without broadening the run lane to
  unrelated package fast tests.
- For a shared integration-test support file edit, pass the support path to
  `owned-fast-test`; the selector targets only the fast tests that import it.
  If the local lane is warm, `owned-fast-test-local` can beat remote execution
  for this shape.
- For clippy during inner-loop work, use `owned-clippy-rbe <path>` first. Warm
  changed-scope clippy is currently sub-second for a leaf source edit and around
  one second for the representative shared support-file case.
- For a normal post-edit agent gate, use `scripts/buildbuddy-changed-gate
  --owned <path>`; it overlaps the selected fast tests and selected clippy.
  Exact small lanes still run together or combine test+clippy. Broad package
  suite lanes run serially by default so the test/build lane can populate remote
  cache before clippy starts; use `--parallel-split` or `--serial-split` to
  override that auto choice while benchmarking.
- For the default developer entrypoint, use `scripts/agent-gate`. It runs the
  Cargo changed-path gate unless `--buildbuddy` or
  `MEERKAT_BUILDBUDDY=1` opts into BuildBuddy. Direct integration
  test files and importable `tests/support` modules are narrowed to exact Cargo
  test binaries for both nextest and clippy when the mapping is unambiguous.
  Cargo and BuildBuddy selectors share the same Cargo metadata and test-module
  import traversal logic in `scripts/rust-test-selector.mjs`.
  Required-feature Cargo test edits are narrowed to the exact test binary with
  the needed features. BuildBuddy also generates those Bazel test targets, but
  keeps e2e/system/live tests out of the fast test lane; the changed gate
  compiles and clippies the exact required-feature test target without running
  the live test or broadening to unrelated package tests.
- For the common multi-agent case where the agent has local edits and just
  needs the right check, use `make buildbuddy-agent-gate`. It includes
  committed branch changes, staged changes, unstaged changes, and untracked
  build-relevant files. Use `scripts/buildbuddy-agent-gate --dry-run` to
  inspect the selected changed-path or workspace gate without running it. Use
  `--committed`, `--staged`, or `--working-tree` when a hook or CI lane should
  restrict the diff scope.
- For deeper shared crates, use `affected-*` when you need reverse-dependency
  confidence, and expect broad closures for high-fanout crates.
- For broad local developer lanes, prefer `MEERKAT_BUILDBUDDY=1 make build`,
  `MEERKAT_BUILDBUDDY=1 make check`, `MEERKAT_BUILDBUDDY=1 make lint`, and
  `MEERKAT_BUILDBUDDY=1 make test`. These route through
  `scripts/buildbuddy-dev` while keeping Cargo as the default for everyone else.
- For explicit BuildBuddy local lanes without environment switching, run
  `make buildbuddy-build`, `make buildbuddy-check`, `make buildbuddy-clippy`,
  `make buildbuddy-test`, `make buildbuddy-test-unit`,
  `make buildbuddy-test-int`, `make buildbuddy-e2e-fast`, or
  `make buildbuddy-e2e-system`. The live-provider forms are
  `make buildbuddy-e2e-live` and `make buildbuddy-e2e-smoke`.
- For an opt-in BuildBuddy fast test pass over the generated fast suite, run
  `make buildbuddy-fast`.
- To install the pinned BuildBuddy CLI without changing system directories, run
  `make buildbuddy-install`. The BuildBuddy scripts also check that cache path
  when `bb` is not already on `PATH`.
- To refresh the checked-in optional Bazel BUILD files after Cargo metadata or
  generator changes, run `make buildbuddy-generate`. To verify freshness without
  mutating the worktree, run `make buildbuddy-generate-check`; `make
  buildbuddy-doctor` runs the same check and verifies that Cargo fast test
  targets map into the generated Bazel `//:fast_tests` suite.
- To check the Cargo/default side of the lane contract, run
  `make rust-lane-doctor`. It verifies that `repo-cargo` keeps caches outside
  the checkout, `RUST_LANE_ID` and `CODEX_AGENT_ID` create distinct
  same-checkout target dirs, GitHub workflow Rust entrypoints use the wrapper,
  and broad fast lanes still use the filtered nextest profile. It also guards
  the Cargo agent gate's exact integration-test routing for direct `tests/*.rs`
  edits.
- To check whether a machine is ready for the optional BuildBuddy lanes, run
  `make buildbuddy-doctor`. It runs the Rust lane doctor first, then checks
  credentials, `bb`, Bazel config, selector health, exact support-file test
  routing, and BuildBuddy lane isolation without running a build or printing
  secrets.
- To compare the apples-to-apples fast lanes, run `make buildbuddy-benchmark`.
  It times Cargo fast tests and the equivalent BuildBuddy fast lane, then keeps
  compact logs, `benchmark-context.txt`, and `summary.tsv` under the benchmark
  cache directory. Use `scripts/buildbuddy-benchmark-fast-lanes --cargo` or
  `--buildbuddy` to refresh one side without rerunning the other.
- For the remote-compatible BuildBuddy gate, run `make buildbuddy-ci`, or
  `make buildbuddy-ci-warm` when repeated agents can reuse a stable output root.
- To mirror the manual GitHub workflow locally, run
  `scripts/buildbuddy-ci-dispatch --mode workspace-fresh`. For local changed
  path checks, run
  `scripts/buildbuddy-ci-dispatch --mode changed-committed --base origin/main`
  or
  `scripts/buildbuddy-ci-dispatch --mode changed-paths --paths '<paths>'`.
- To enable the optional BuildBuddy job inside normal GitHub CI, add the
  `BUILDBUDDY_API_KEY` repository secret and set the repository variable
  `MEERKAT_BUILDBUDDY=true` (or `1`). The default reusable-workflow mode is
  `full-fresh`; override it with `MEERKAT_BUILDBUDDY_CI_MODE` when a manual or
  reusable run needs a single standard lane.
- For same-checkout multi-agent work, prefer a distinct `RUST_LANE_ID` per
  agent for stable warm lanes. `scripts/repo-cargo` also falls back to
  `MEERKAT_AGENT_LANE` and `CODEX_AGENT_ID`, so Codex-style agent processes get
  isolated Cargo target dirs even when they do not set `RUST_LANE_ID`
  explicitly.

## Known Annoyance

`bb` currently prints:

```text
WARNING: The following rc files are no longer being read...
```

The warning is noisy but non-blocking in this spike: configs are applied, remote
execution is used, and invocations stream to BuildBuddy.

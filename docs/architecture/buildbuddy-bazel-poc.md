# BuildBuddy Bazel POC

Status: Spike notes for the BuildBuddy/Bazel workspace experiment
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

## Entry Points

Use `scripts/buildbuddy-bazel-poc` with `BUILDBUDDY_BAZEL_COMMAND`:

- `workspace-test`: run the full Bazel workspace test suite.
- `workspace-test-rbe`: run the remote-compatible workspace test suite,
  excluding local-only Cargo/trybuild fixtures.
- `workspace-test-local`: run the full workspace test suite with local spawns.
- `clippy-rbe`: run the rules_rust clippy aspect with `-D warnings`,
  excluding local-only, manual, and `noclippy` targets.
- `owned-build <path>`: build the owning package target for a changed path.
- `affected-build <path>`: build the reverse-dependency closure for a changed path.
- `owned-fast-test <path>`: run the owning fast suite, or the exact test target
  when `<path>` is an integration test crate root.
- `affected-fast-test <path>`: run fast suites for the reverse-dependency closure.
- `fast-test`: run the root `//:fast_tests` suite.
- `clippy`: run the rules_rust clippy aspect with `-D warnings`.

For local-spawn variants, append `-local` to the command name where available.
Remote execution remains the default because local-spawn often spends time
downloading large remote-cache inputs before rustc can run.

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
command in the same checkout should set distinct `RUST_LANE_ID` values. Separate
Git worktrees automatically get different workspace hashes, so they do not share
local Bazel output bases.

## Simulation Harness

`scripts/buildbuddy-simulate-scenarios.mjs` exercises the expected operating
modes:

- `workspace-test`: full Bazel workspace test suite.
- `workspace-test-rbe`: remote-compatible workspace suite excluding local-only
  fixtures.
- `workspace-test-local`: full Bazel workspace test suite with local spawns.
- `warm-noop`: warm full fast-test and clippy checks.
- `same-worktree`: two same-checkout agents using distinct lanes.
- `same-command`: two same-checkout agents running the same command with
  distinct lanes.
- `support-file`: exact selector behavior for a shared integration-test support
  file, measured with remote and local-spawn execution.
- `edit-probes`: creates a temporary detached worktree, makes harmless real
  edits to representative source/test/support files, runs the matching lanes,
  and removes the worktree.
- `edit-probes-warmed`: runs the same edit probes after prewarming the relevant
  lanes in the temporary worktree, separating first-touch materialization from
  steady-state edit feedback.
- `prewarm-dev`: runs the shared dev prewarm profile.
- `prewarm-ci`: runs the shared CI prewarm profile.
- `multi-worktree`: two temporary Git worktrees, each with its own lane.
- `ci-cold`: sequential CI-like fast-test and clippy on fresh output bases.
- `ci-parallel`: parallel CI-like fast-test and clippy on fresh output bases.
- `ci-workspace`: parallel CI-like `workspace-test-rbe` and `clippy-rbe` on fresh
  output bases.

For an actual remote-compatible CI gate, use `scripts/buildbuddy-ci-workspace`.
It runs `workspace-test-rbe` and `clippy-rbe` in parallel on isolated fresh
lanes, prints compact summaries, and cleans up the temporary output roots.
Use `--warm` to reuse a stable output root for repeated local/agent gates.

`scripts/buildbuddy-prewarm-lanes` prepares common lanes for a new worktree:

- `dev`: source-owned-build, exact-test, and support-local feedback lanes.
- `ci`: remote-compatible workspace-test and clippy lanes in parallel.
- `ci-fast`: fast-test and clippy-RBE lanes in parallel.

The prewarm helper uses direct Bazel labels instead of path selectors so several
startup lanes do not contend on Cargo metadata/package-cache locks.

## Observed Results

Representative measurements from the POC environment:

| Scenario | Result |
| --- | ---: |
| Full workspace test lane (`152` tests), remote/cache | `23.25s` wall |
| Full workspace test lane (`152` tests), warm remote/cache | `3.96-5.33s` wall |
| Remote-compatible workspace lane (`151` tests), first touch | `22.27s` wall |
| Remote-compatible workspace lane (`151` tests), warm | `4.10-4.38s` wall |
| Full workspace test lane (`152` tests), local-spawn first pass | `67.01s` wall |
| Full workspace test lane (`152` tests), warm local-spawn | `4.75s` wall |
| Warm root fast suite (`117` tests) | `3.99s` wall |
| Warm full clippy aspect (`288` targets) | `4.99s` wall |
| Same-worktree, two different warmed lanes | `6.42s` / `6.70s` wall |
| Same-worktree, same command, warmed lanes | `4.44s` / `4.34s` wall |
| Shared test-support edit, exact remote selector | `19.10s` wall |
| Shared test-support edit, exact local-spawn selector | `8.25s` wall |
| Fresh temp worktree source edit probe | `23.98s` wall |
| Fresh temp worktree exact-test edit probe | `48.31s` wall |
| Fresh temp worktree support-local edit probe | `49.99s` wall |
| Prewarmed temp worktree source edit probe | `3.85s` wall |
| Prewarmed temp worktree exact-test edit probe | `5.62s` wall |
| Prewarmed temp worktree support-local edit probe | `5.62s` wall |
| Direct-label dev prewarm, first touch | `46.13s` wall |
| Direct-label dev prewarm, warm | `4.62s` wall |
| Direct-label CI prewarm, first touch | `29.88s` wall |
| Direct-label CI prewarm, warm | `6.15s` wall |
| Multi-worktree first-touch lanes | `38.35s` / `44.00s` wall |
| CI-like sequential fresh output bases | `25.48s` fast-test + `26.89s` clippy |
| CI-like parallel fresh output bases | `33.28s` max wall |
| CI-like workspace-RBE + clippy-RBE, fresh output bases | `26-37.14s` max wall |
| Dedicated `buildbuddy-ci-workspace` script | `28-30s` script wall |
| Dedicated `buildbuddy-ci-workspace --warm`, warm | `6-8s` script wall |

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
- For a shared integration-test support file edit, pass the support path to
  `owned-fast-test`; the selector targets only the fast tests that import it.
  If the local lane is warm, `owned-fast-test-local` can beat remote execution
  for this shape.
- For deeper shared crates, use `affected-*` when you need reverse-dependency
  confidence, and expect broad closures for high-fanout crates.
- For the remote-compatible BuildBuddy gate, run `make buildbuddy-ci`, or
  `make buildbuddy-ci-warm` when repeated agents can reuse a stable output root.
- For same-checkout multi-agent work, always set a distinct `RUST_LANE_ID` per
  agent unless the commands are known to use different lanes already.

## Known Annoyance

`bb` currently prints:

```text
WARNING: The following rc files are no longer being read...
```

The warning is noisy but non-blocking in this spike: configs are applied, remote
execution is used, and invocations stream to BuildBuddy.

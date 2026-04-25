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

- `warm-noop`: warm full fast-test and clippy checks.
- `same-worktree`: two same-checkout agents using distinct lanes.
- `same-command`: two same-checkout agents running the same command with
  distinct lanes.
- `multi-worktree`: two temporary Git worktrees, each with its own lane.
- `ci-cold`: sequential CI-like fast-test and clippy on fresh output bases.
- `ci-parallel`: parallel CI-like fast-test and clippy on fresh output bases.

## Observed Results

Representative measurements from the POC environment:

| Scenario | Result |
| --- | ---: |
| Warm root fast suite (`117` tests) | `3.99s` wall |
| Warm full clippy aspect (`288` targets) | `4.99s` wall |
| Same-worktree, two different warmed lanes | `6.42s` / `6.70s` wall |
| Same-worktree, same command, warmed lanes | `4.44s` / `4.34s` wall |
| Multi-worktree first-touch lanes | `38.35s` / `44.00s` wall |
| CI-like sequential fresh output bases | `25.48s` fast-test + `26.89s` clippy |
| CI-like parallel fresh output bases | `33.28s` max wall |

The first touch of a new local lane pays Bazel analysis and remote-cache
materialization cost. Once warmed, the wall-clock floor is mostly the `bb`/Bazel
client startup path rather than Rust compilation.

## Current Guidance

- For leaf or package-local edits, start with `owned-build` or
  `owned-fast-test`.
- For an integration test file edit, pass the test file path to
  `owned-fast-test`; the selector targets the exact Bazel test when it is fast.
- For deeper shared crates, use `affected-*` when you need reverse-dependency
  confidence, and expect broad closures for high-fanout crates.
- For CI, run fast-test and clippy in parallel with separate `RUST_LANE_ID`s.
- For same-checkout multi-agent work, always set a distinct `RUST_LANE_ID` per
  agent unless the commands are known to use different lanes already.

## Known Annoyance

`bb` currently prints:

```text
WARNING: The following rc files are no longer being read...
```

The warning is noisy but non-blocking in this spike: configs are applied, remote
execution is used, and invocations stream to BuildBuddy.

---
name: vm-disk-pressure-build-caches
description: The Linux release VM (now 2.9 TiB root disk) can fill up within hours from Rust build caches; what consumes space and what is safe to delete
metadata: 
  node_type: memory
  type: project
  originSessionId: 5df1b3d1-6021-4c86-b59d-623dd339f75b
  modified: 2026-09-04T16:33:22.342Z
---

UPDATE 2026-09-04 ~16:40Z: Luka grew the root disk online to 2.9 TiB (GCE resize + growpart + resize2fs), so pressure is relieved; the consumption pattern below still holds. Observed earlier that day on the 968 GB disk: usage went 180 GB -> 968 GB (ENOSPC, killed three concurrent pre-push gates) -> pruned -> 814 GB again within an hour.

Consumers (per `~/.cache/rust-workspaces/<repo-key>/targets/...`):
- A MobKit `check --workspace --all-targets` + `clippy --workspace --all-targets` + `nextest --workspace` produced a 314 GB `debug/` target.
- Each meerkat pre-push gate lane (`pre-push-<16hex>`) is 25-65 GB; each worktree dev target 25-90 GB.
- Each pre-push machine gate builds xtask into `/tmp/meerkat-xtask-target-<hash>` (5.5 GB each, never cleaned).
- Bazel/BuildBuddy output bases are small (2-3 GB).

Safe to delete when no cargo/rustc process references the path (`ps -eo args | grep <dir>`): finished worker worktrees' targets, dev targets of checkouts not mid-build (rebuild ~10 min on 192 cores), `/tmp/meerkat-xtask-target-*` older than 30 min. Pre-push lanes are now governed by PR #1099's retention pruner (passed-gate only, 6 h recency).

**Why:** ENOSPC manifests as confusing compile errors (`failed to write file`, `failed to write query cache`) in otherwise unrelated gates.

**How to apply:** check `df -h /` before launching parallel gates or agents; delete finished workers' caches as soon as their branch is pushed. See [[release-train-pr-inventory]].


CPU contention (2026-09-05 01:53Z): with five worker worktrees compiling from cold plus one pre-push gate, the 1-minute load hit 257 on 192 cores and the gate's `workspace deterministic unit + integration + e2e gate` failed 12 timeout-bound tests (meerkat-mob host_materialize_serving, cold_restart_*, cross_host_events, host_bind_ceremony: `Elapsed(())`, "must not wait for the 30s reconciliation grace") on a branch that touched only release-doctor scripts; the same suite had passed an hour earlier. Treat such a burst of unrelated timeout failures as load-induced: check /proc/loadavg, wait for load under ~110, re-push. Cap concurrent cold builds (ask workers for CARGO_BUILD_JOBS=48, nextest --test-threads 16) while a gate runs.


wasm-opt thread storm (2026-09-05 08:25Z): `make test-sdk-web` on this 192-core VM runs binaryen's wasm-opt with 193 threads; it burned 2d15h of CPU time in 21 minutes at load 190 without finishing (lock contention). Always run it with `BINARYEN_CORES=16` (binaryen honours that env var), including in the environment of any `git push` whose pre-push gate will build the web SDK (branches touching sdks/web/**).

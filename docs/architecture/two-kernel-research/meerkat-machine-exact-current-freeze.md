# MeerkatMachine Exact-Current Freeze

Status: frozen exact-current-state asset

This note records the exact-current Meerkat boundary as it exists in the live
codebase today.

It is the implementation baseline for the target-state `MeerkatMachine`
freeze, not the final architectural target by itself.

## What it is for

Use this asset when the question is:

- what does the current runtime actually do?
- what can we model today without inventing behavior?
- what should the implementation be checked against while the target machine is
  being built?

Use `meerkat-machine-freeze.md` when the question is:

- what is the full target single-machine design we intend to prove and cut to?

## What is frozen

The full exact-current `MeerkatMachine` boundary includes:

- control and binding
- admission and input ledger
- completion-waiter carrier
- turn execution
- async operations and `wait_all`
- detached wake / continuation injection
- peer ingress
- tool visibility
- external tool surface
- drain / keep-alive lifecycle

The exact-current freeze assets that compose this machine are:

- `meerkat-interrupt-freeze.md`
- `meerkat-detached-wake-freeze.md`
- `meerkat-turn-ops-barrier-freeze.md`
- `meerkat-peer-ingress-freeze.md`
- `meerkat-tool-visibility-freeze.md`
- `meerkat-tool-surface-freeze.md`
- `meerkat-drain-freeze.md`
- `meerkat-input-effect-alphabet.md`
- `meerkat-lowering-map.md`
- `meerkat-ownership-decisions.md`

## Exact-current-state classification

This freeze is exact-current, not target-state.

That means it intentionally includes:

- current split-lifetime behavior between input completion waiters and ops-owned
  `wait_all`
- current plain vs attached lifecycle differences
- current steer-lane differences between retire, reset, stop, and destroy
- current public peer-ingress and drain seams
- current durable tool-visibility owner and projection seam
- current published tool-surface snapshot scope
- current compatibility fallback for detached wake when no completion feed is
  present

It also intentionally excludes or classifies out of scope:

- durable replay of peer-ingress queue state
- a second store-backed tool-surface recovery protocol
- public lowering of non-`PersistentHost` drain modes
- stronger hidden epoch guarantees than the live code and ADR currently support

Those exclusions are part of the honest exact-current freeze. They are not
hidden debt inside the baseline.

## Verification Lane

Focused freeze lanes:

- `cargo test -p meerkat-runtime --lib`
- `cargo test -p meerkat-runtime --test detached_wake_contract`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_turn_ops_barrier_violations`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_trust_shape_violations`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_stopped_comms_drain_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_drain_shape_violations`

Widened freeze lanes:

- `cargo test --workspace --tests --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo check --workspace --tests --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Freeze decision

The exact-current Meerkat baseline is considered frozen.

The next architectural step is not more exact-current freeze work. It is:

1. use this note as implementation baseline
2. use `meerkat-machine-freeze.md` as the target-state machine asset
3. prove the target machine in TLA+
4. compare implementation and target explicitly instead of blurring them

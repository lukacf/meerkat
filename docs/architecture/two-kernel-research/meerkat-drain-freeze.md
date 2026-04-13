# Meerkat Drain And Keep-Alive Freeze

Status: frozen exact-current-state asset

This note closes `K6` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for comms-drain /
keep-alive lifecycle at the session-adapter boundary.

## Scope

This freeze covers:

- `MeerkatMachine::update_peer_ingress_context(...)`
- `MeerkatMachine::maybe_spawn_comms_drain(...)`
- `CommsDrainLifecycleAuthority`
- drain-slot / phase / mode / handle snapshot shape
- unregister / abort / stopped-slot behavior

This freeze does **not** claim:

- that all generated drain modes are surfaced by the current public adapter
- a separate Meerkat durability layer for drain lifecycle state

## Exact current semantics

### 1. `update_peer_ingress_context(...)` is the live public seam

Exact current live behavior:

- CLI / REST / RPC / MCP callers use `update_peer_ingress_context(...)`
- that public seam delegates to the canonical owner method
  `maybe_spawn_comms_drain(...)`

### 2. Current public lowering only surfaces `PersistentHost`

Exact current live behavior:

- `maybe_spawn_comms_drain(...)` hardcodes `CommsDrainMode::PersistentHost`
- `Timed` and `AttachedSession` remain authority/model shapes but are not
  produced by the current adapter lowering

### 3. Keep-alive directly controls spawn vs abort

Exact current live behavior:

- `keep_alive = false` aborts any active drain and returns `false`
- `keep_alive = true` spawns only when:
  - the session is registered
  - a comms runtime is present
  - the authority accepts `EnsureRunning`

### 4. The drain slot shape is machine-owned today

The exact current joined validator freezes the following drain rules:

- no phase / mode / handle may appear without a published slot
- a published slot must have a phase
- `Inactive` may not carry mode or handle
- stopped / terminal phases may not carry a live handle
- running-ish phases require a mode

### 5. Unregister and abort are current cutover semantics

Exact current live behavior:

- unregister aborts and removes the drain slot
- stopped drains remain snapshot-visible as stopped slots
- `abort_comms_drain(...)` is part of the current runtime lifecycle story, not
  compatibility debt

## Exact-vs-target-state classification

What is frozen as exact current live behavior:

- the public keep-alive seam
- canonical `maybe_spawn_comms_drain(...)` ownership
- `PersistentHost` drain lowering
- slot / phase / mode / handle invariants
- unregister / abort / stopped-slot behavior

What is explicitly **not** frozen as exact current live behavior:

- public lowering for non-`PersistentHost` drain modes
- a separate durable recovery protocol for drain slots

## Proof inventory

Runtime adapter proofs:

- `meerkat_machine_spine_snapshot_tracks_comms_drain_state`
- `meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state`
- `unregister_session_aborts_and_removes_drain_slot`

Joined Meerkat proofs:

- `capture_meerkat_machine_snapshot_joins_stopped_comms_drain_state`
- `validate_meerkat_machine_snapshot_reports_drain_shape_violations`

Rebase-audit seam classification:

- `update_peer_ingress_context(...)` is the live public seam
- `drain_peer_input_candidates(...)` remains a live runtime drain bridge

## Reviewer Verification Lane

The narrow verification lane for this freeze asset is:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_comms_drain_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state`
- `cargo test -p meerkat-runtime --lib unregister_session_aborts_and_removes_drain_slot`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_stopped_comms_drain_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_drain_shape_violations`
- `rg -n "update_peer_ingress_context\\(|maybe_spawn_comms_drain\\(|abort_comms_drain\\(" meerkat-runtime meerkat-core meerkat/src`

## Freeze decision

`K6` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- live keep-alive / drain lifecycle semantics
- the current public adapter lowering
- slot / phase / mode / handle shape

What is explicitly not frozen:

- public lowering for non-`PersistentHost` drain modes
- a separate durable recovery story for drain slots

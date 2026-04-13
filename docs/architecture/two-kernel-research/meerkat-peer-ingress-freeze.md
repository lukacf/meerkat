# Meerkat Peer-Ingress Freeze

Status: frozen exact-current-state asset

This note closes `K4` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for peer ingress at the
classified inbox / peer authority / runtime snapshot boundary.

## Scope

This freeze covers:

- classified inbox queue snapshot shape
- peer authority phase and submission count
- trust mutation and trust snapshots
- auth-required vs auth-open admission behavior
- the public runtime seam `update_peer_ingress_context(...)`

This freeze does **not** claim:

- durable store-backed replay of peer-ingress queue state
- post-admission raw envelope lineage as a canonical long-lived Meerkat fact
- that peer ingress is already fully folded into a larger Mob/transport model

## Exact current semantics

### 1. Peer ingress is observed through the live classified inbox

Exact current live behavior:

- the Meerkat snapshot reads peer ingress through
  `CommsRuntime::peer_ingress_runtime_snapshot()`
- that live snapshot includes:
  - `self_peer_id`
  - `auth_required`
  - `trusted_peers`
  - `authority_phase`
  - `submission_queue_len`
  - classified queue contents

The current exact boundary is therefore a live comms-runtime observation seam,
not a store-backed recovery seam.

### 2. Trust shape is explicit

Exact current live behavior:

- non-plain peer entries carry `trusted_snapshot`
- plain events do **not** carry `trusted_snapshot`
- on auth-required runtimes, admitted peer entries may not be untrusted
- on auth-open runtimes, admitted peer entries may be present with
  `trusted_snapshot = Some(false)`

### 3. Authority-tracked submission count ignores plain events

Exact current live behavior:

- `submission_queue_len` counts the authority-tracked actionable peer entries
- plain external events can coexist in the queue without contributing to that
  count
- the joined validator therefore compares `submission_queue_len` only against
  queued non-plain authority entries

### 4. Trust mutation has named canonical seams

Exact current live behavior:

- canonical runtime trust mutation uses named register/unregister seams
- the older helper shape is explicitly non-canonical
- the public surface-facing seam for peer ingress and keep-alive remains
  `MeerkatMachine::update_peer_ingress_context(...)`

### 5. Recovery is exact-current live re-observation, not durable replay

Exact current live behavior:

- there is no separate Meerkat store-backed recovery path that replays a saved
  peer-ingress queue
- peer-ingress truth is re-observed from the live comms runtime / authority
  after registration or rebind
- post-admission raw envelope lineage is not treated as a canonical machine
  fact past the queue/authority snapshot

That absence is part of the current frozen boundary; it is not a deferred
promise.

## Exact-vs-target-state classification

What is frozen as exact current live behavior:

- classified queue shape
- trust snapshot rules
- auth-required vs auth-open admission behavior
- dropped / received authority phases
- public peer-ingress context update seam

What is explicitly **not** frozen as exact current live behavior:

- durable recovery of classified queue contents
- post-admission raw envelope lineage as a machine-owned long-lived fact

## Proof inventory

Comms-runtime proofs:

- `test_peer_ingress_runtime_snapshot_reflects_trusted_peers_and_queue`
- `test_peer_ingress_runtime_snapshot_reflects_dropped_peer_authority_state`
- `test_live_peer_authority_syncs_trust_receive_and_drain`
- `test_live_peer_authority_accepts_unknown_peer_when_auth_open`

Joined Meerkat proofs:

- `capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `validate_meerkat_machine_snapshot_reports_peer_shape_violations`
- `validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `validate_meerkat_machine_snapshot_reports_peer_trust_shape_violations`

## Reviewer Verification Lane

The narrow verification lane for this freeze asset is:

- `cargo test -p meerkat-comms --lib test_peer_ingress_runtime_snapshot_reflects_trusted_peers_and_queue`
- `cargo test -p meerkat-comms --lib test_peer_ingress_runtime_snapshot_reflects_dropped_peer_authority_state`
- `cargo test -p meerkat-comms --lib test_live_peer_authority_syncs_trust_receive_and_drain`
- `cargo test -p meerkat-comms --lib test_live_peer_authority_accepts_unknown_peer_when_auth_open`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_trust_shape_violations`

## Freeze decision

`K4` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- live peer-ingress queue truth
- trust mutation / trust snapshot semantics
- public peer-ingress context seam

What is explicitly not frozen:

- durable peer queue replay
- post-admission raw envelope lineage beyond the current queue snapshot

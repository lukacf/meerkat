# Track-B peer-projection producer wiring — deferred from wave-c #37 as no-op-today

Owner: wave-d (TBD assignee). Forwarded from wave-c task #37 after investigation
showed the deliverable reduces to a no-op under the current wave-c code shape.
This doc captures the architectural findings, the three-seam plan to close the
gap, and the catching assertion that proves the eventual follow-up task closed.

## Background

Wave-c C-T (commit `b0e881535`) ported `meerkat-runtime/src/comms_trust_reconcile.rs`
from PR #340 — the `CommsTrustReconciler` struct with
`reconcile(epoch, BTreeSet<PeerEndpoint>) -> Result<ReconcileReport, _>`, a
`tokio::sync::Mutex<AppliedView>` concurrency guard, and 3 blocker-list tests
at commit `d2ec89c5e`. Wave-c task #37 was scoped as "wire the effect
terminator in `comms_drain.rs`" — bind the DSL-emitted
`CommsTrustReconcileRequested` effect to `CommsTrustReconciler::reconcile`.

Investigation during #37 showed that the effect terminator cannot be wired
without three additional architectural seams that are NOT present in the
current wave-c tree. This doc forwards the three-seam plan to wave-d and
records the findings verbatim as the spec for the follow-up task.

## Findings

### Finding 1 — the effect has no emitter on any production path

`CommsTrustReconcileRequested` is emitted by exactly 3 DSL transitions
(`meerkat-runtime/src/meerkat_machine/dsl.rs` lines 6488, 6501, 6515):

- `AddDirectPeerEndpoint`
- `RemoveDirectPeerEndpoint`
- `ApplyMobPeerOverlay`

`grep -rn "AddDirectPeerEndpoint\|RemoveDirectPeerEndpoint\|ApplyMobPeerOverlay" meerkat-runtime/src/ --include="*.rs"`
returns ZERO production hits — the only callers are inside
`meerkat-runtime/tests/peer_projection_dsl.rs`. The effect payload is only
`peer_projection_epoch: u64`; the reconciler's `BTreeSet<PeerEndpoint>` must
be derived by reading `direct_peer_endpoints ∪ mob_overlay_peer_endpoints`
from DSL state at observation time.

### Finding 2 — the effect has no consumer / observer terminator

`grep -rn "CommsTrustReconcileRequested" meerkat-runtime/src/` returns only
the DSL emit sites, the module doc in `comms_trust_reconcile.rs`, and one
comment in `dsl.rs` — no shell-side consumer, no handle observer, no drain
site. The effect terminates nowhere today. There is NO effect delivery seam
in `comms_drain.rs` to place the reconciler wire.

### Finding 3 — the scattered `add_trusted_peer` / `remove_trusted_peer` calls in `comms_drain.rs` are NOT DSL-emitted-trust-reconcile paths

The 6 production sites in `meerkat-runtime/src/comms_drain.rs` all correspond
to bridge-protocol handlers, not the peer-projection state machine:

- Line 1032 (`BridgeCommand::BindMember`) — stages `BindSupervisor` DSL input,
  which mutates `supervisor_bound_*` state fields (distinct from
  `direct_peer_endpoints` / `mob_overlay_peer_endpoints`).
- Line 1142 (`BridgeCommand::AuthorizeSupervisor` — new trust publication)
  and lines 1169-1181 (same handler — previous-supervisor trust removal +
  cleanup) — stage `AuthorizeSupervisor` DSL input.
- Line 1220 (`BridgeCommand::RevokeSupervisor`) — stages `RevokeSupervisor`
  DSL input.
- Line 1496 (`BridgeCommand::WireMember`) — does NOT stage any DSL input
  today. Goes straight to `comms_runtime.add_trusted_peer` with no DSL
  mutation.
- Line 1531 (`BridgeCommand::UnwireMember`) — does NOT stage any DSL input
  today. Goes straight to `comms_runtime.remove_trusted_peer` with no DSL
  mutation.

Verified at `meerkat-runtime/src/meerkat_machine/dsl.rs:6384-6444` — the
`BindSupervisor` / `AuthorizeSupervisor` / `RevokeSupervisor` transitions
have NO `emit` clauses at all; they do not emit
`CommsTrustReconcileRequested` (or any effect). The supervisor-binding state
machine (`supervisor_binding_kind` / `supervisor_bound_*`) and the
peer-projection state machine (`direct_peer_endpoints` /
`mob_overlay_peer_endpoints` / `peer_projection_epoch`) are architecturally
disjoint — separate DSL fields, separate transitions, separate effect
vocabulary.

## Path A three-seam plan

To wire the effect terminator as originally envisioned, three seams must be
built in the same task — any one alone is incomplete:

### Seam 1 — Producer wiring (stager helpers + bridge re-wire)

Add three stager helpers on `MeerkatMachine` (co-located with the existing
`stage_supervisor_bind` / `stage_supervisor_authorize` / `stage_supervisor_revoke`
at `meerkat-runtime/src/meerkat_machine/comms_drain.rs`):

- `stage_add_direct_peer_endpoint(session_id, endpoint) -> Result<Vec<Effect>, _>`
- `stage_remove_direct_peer_endpoint(session_id, endpoint) -> Result<Vec<Effect>, _>`
- `stage_apply_mob_peer_overlay(session_id, epoch, endpoints) -> Result<Vec<Effect>, _>`

Each stager applies the matching DSL input and returns the emitted effects
(using `apply_input_with_effects_and_sample` semantics so the effect and the
post-transition state snapshot are observed under the same DSL lock).

Re-wire `BridgeCommand::WireMember` (`comms_drain.rs:1471-1517`) to call
`stage_add_direct_peer_endpoint` instead of a direct
`comms_runtime.add_trusted_peer`. Re-wire
`BridgeCommand::UnwireMember` (`comms_drain.rs:1518-1554`) to call
`stage_remove_direct_peer_endpoint`. The `TrustedPeerDescriptor` from the
bridge payload maps into a DSL `PeerEndpoint` (the three slug-string fields).

`BindMember` / `AuthorizeSupervisor` / `RevokeSupervisor` stay on their
existing supervisor-binding DSL transitions and their existing direct
`add_trusted_peer` / `remove_trusted_peer` calls — these are legitimately
(b) bootstrap paths: the supervisor itself is the trust anchor and cannot
reconcile against a peer set that doesn't yet include it. Document this
explicitly in the commit message at the bridge-handler call sites.

### Seam 2 — Consumer / observer wiring

The stager helpers are the natural observer seam: after applying the DSL
input, sample the emitted effects for `CommsTrustReconcileRequested`, read
the post-transition `direct_peer_endpoints ∪ mob_overlay_peer_endpoints`
from the same DSL lock, and invoke
`reconciler.reconcile(epoch, effective_peers).await` on the session-owned
`Arc<CommsTrustReconciler>`.

Single seam, placed inside the stager so every producer path (bridge + any
future mob-overlay driver) routes through the same terminator.

### Seam 3 — Reconciler lifetime

`CommsTrustReconciler::new(comms: Arc<dyn CommsRuntime>)` has zero
production callers today. `grep "CommsTrustReconciler::new" meerkat-runtime/src/`
returns only the test modules.

The reconciler must be constructed at session registration time (one per
session, lifetime-tied to the session's `CommsRuntime`) and stored in
`RuntimeSessionEntry` alongside the `dsl_authority`. Observer path:
stager helper → `entry.trust_reconciler.reconcile(...)` →
`entry.comms_runtime.add_trusted_peer` / `remove_trusted_peer`.

Alternative lifetime (also valid): runtime-level singleton keyed by
session_id, constructed lazily. The session-scoped shape is simpler and
matches the existing `RuntimeSessionEntry` → `CommsRuntime` ownership.

## Why this is deferred

Wave-c's scope was the shell rebuild + DSL state port driven by the C-T
track — land the reconciler module + tests (done, wave-c-c-t commit
`b0e881535` + `d2ec89c5e`) and inventory the missing producer-side
integration seams. Producer wiring crosses into the next architectural
integration layer that Track-B originally planned as its follow-up phase:
new stager helpers + bridge-handler re-wiring + per-session reconciler
lifetime. That's multi-seam structural work, not a single terminator placement.

The reconciler module and its 3 mock-backed tests ARE the wave-c C-T
obligation fulfillment — they pin the concurrency invariants (§6 #1/#2/#3
on the blocker list) against the ported API. The producer-side integration
(this doc) is the wave-d follow-up.

## Catching assertion for the eventual closure

When the wave-d follow-up task runs and closes the gap, these assertions
must both hold:

- `grep "CommsTrustReconciler::new" meerkat-runtime/src/` returns >0
  production hits (not just tests) — proving the reconciler has at least
  one production constructor site.
- `grep "add_trusted_peer\|remove_trusted_peer" meerkat-runtime/src/comms_drain.rs`
  returns zero results, OR only results that are the reconciler-invocation
  call site(s). Scattered direct calls on the direct-peer / overlay paths
  (WireMember / UnwireMember bridge handlers) must be gone; supervisor-bind
  / trust-edge paths that legitimately bootstrap the trust anchor are the
  named (b) exceptions and must be explicitly flagged in the commit message
  that closes the task.

Additionally, the existing C-T tests
(`meerkat-runtime/tests/trust_reconcile_concurrency.rs` and
`meerkat-runtime/tests/trust_reconcile_add_failure.rs`) must continue to
pass unchanged — they test the reconciler module against a mock
`CommsRuntime` and are independent of the production wiring path.

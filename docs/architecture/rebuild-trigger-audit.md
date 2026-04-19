# Rebuild-Trigger Audit

Status: Read-only audit seeding ITEM-6 sub-PRs
Scope: Every cached / projection-shaped field currently reachable on `main`
Dogma reference: `meerkat-runtime-dogma.md` #13 — *derived projections are rebuildable, never authoritative*
Roadmap reference: issue #264 (0.7 peer/comms authority drift), W1-D
Baseline commit: `97981e314` (post-prerequisite-patchwork merge)

## What this audit is

Dogma #13 says a projection must be rebuildable from canonical truth and must
never be load-bearing for semantic decisions. In practice, a field either:

- reads from one canonical owner (DSL authority, durable metadata, event
  ledger) and is rebuilt / projected on a well-defined trigger, or
- is shadow truth: its value can cause a behavioral divergence from canonical
  truth because no trigger guarantees freshness for the decision site
  that consults it.

This document walks every projection-shaped field on `main` today and classifies
it. Fields already slated for elimination in the Wave 1 parallel PRs are noted
explicitly — the verdict describes the field *as it exists on main at the
baseline commit*, independent of in-flight work.

### Four questions per field

For each field the audit answers:

1. **Canonical source.** What owns the truth?
2. **Rebuild trigger sites.** Every place that writes or invalidates the
   projection.
3. **Staleness policy.** How long stale values are tolerated, what triggers
   invalidation.
4. **Semantic impact of staleness.** Can a stale value change behavior? If
   yes, it is shadow truth, not a projection.

### Verdicts

- **Compliant projection** — all four questions answered cleanly; the
  field is safe.
- **Projection with soft policy (review)** — a projection whose staleness is
  tolerated in practice but not formally bounded; candidate for tightening.
- **Shadow truth (action required)** — a stale value can change behavior; must
  be promoted to machine-owned state or deleted.

Items are ordered by severity — shadow-truth hits appear first — so that the
ITEM-6 seed list at the end can be read top-down.

---

## Field inventory

### 1. `projection_refresh_dirty` + `projection_known_updated_at` (realtime WS loop)

Location: `meerkat-rpc/src/realtime_ws.rs` (locals at
`realtime_ws.rs:918`, `919`; channel `projection_refresh_rx` at
`realtime_ws.rs:926`; drain sites around `1240`, `1433`, `1580`, `1629`,
`1660`, `1678`, `1712`, `1856`).

- **Canonical source.** Canonical session state owned by `SessionService`
  (and, through it, the MeerkatMachine's session map + events). The realtime
  provider session is the derived projection.
- **Rebuild trigger sites.** A standalone projection-refresh task
  (`spawn_projection_refresh_task`) polls session `updated_at` and pushes
  notifications onto `projection_refresh_tx`. The main loop drains them
  *only* when `product_turn_in_flight` is false; otherwise it flips
  `projection_refresh_dirty = true` and defers the drain to input-chunk
  arrival sites, sometimes with a "was marked dirty but is now clean"
  comparison based on `projection_known_updated_at`.
- **Staleness policy.** Informal. The invariant is "when a product turn
  finishes and we return to idle, refresh if dirty." Drain is realized at
  many sites, each with its own combination of conditions. There is no
  typed transition.
- **Semantic impact of staleness.** YES. The static-analysis post-mortem
  that motivated issue #264 traced the s71 turn-8 regression to this gap:
  a peer-triggered turn arrives, completes, and does *not* fire the drain
  path that would refresh the provider session, so the provider session
  stays stale across the next turn boundary. Stale here means "assistant
  responds with incomplete context."

**Verdict: Shadow truth (action required).**

Seeded into **W2-E** ("Typed projection-refresh effect emission"). W2-E
promotes the flag to a typed `ProjectionFreshness` state with explicit
transitions and subscribes to a DSL `SessionContextAdvanced` effect.

No action in this PR; field described as it exists on main.

---

### 2. `ClassifiedInboxQueue.trusted_peers` (inbox-local trust cache)

Location: `meerkat-comms/src/inbox.rs:48` (`trusted_peers: BTreeSet<PeerId>`
on `ClassifiedInboxQueue`). Mutators: `sync_trusted_peer_added`
(`inbox.rs:64`), `sync_trusted_peer_removed` (`inbox.rs:68`). Initial fill:
`Inbox::new_classified` (`inbox.rs:273`). Read site: `admit_peer_receive`
(`inbox.rs:89`).

- **Canonical source.** `IngressClassificationContext.trusted_peers`
  (an `Arc<parking_lot::RwLock<TrustedPeers>>` shared with
  `classify.rs`). The authoritative trust set is owned by the comms
  runtime; this map on the queue is a local duplicate.
- **Rebuild trigger sites.**
  - Initial snapshot at `Inbox::new_classified`: iterates
    `context.trusted_peers.read().peers` and seeds the local set.
  - Mutation path: explicit `sync_trusted_peer_added` /
    `sync_trusted_peer_removed` calls fired by callers of add/remove
    trust operations (currently only internal sites within `inbox.rs`
    itself, `inbox.rs:351`, `inbox.rs:357`).
- **Staleness policy.** Push-based, not rebuilt — relies on every
  trust-set mutation being mirrored via `sync_*`. No drift detection, no
  periodic resync.
- **Semantic impact of staleness.** YES. `admit_peer_receive` decides
  whether to accept or silently drop an envelope based on this local set.
  If the canonical trust set is mutated without a matching
  `sync_trusted_peer_*` call, the queue either drops envelopes from
  newly-trusted peers or accepts envelopes from revoked peers. Silent
  security-relevant divergence.

**Verdict: Shadow truth (action required).**

Options:

- Read the canonical `Arc<RwLock<TrustedPeers>>` on every
  `admit_peer_receive` and delete the local `trusted_peers` set.
- Promote to a typed DSL field (`TrustedPeerSet`) with explicit
  `TrustPeerAdded` / `TrustPeerRevoked` transitions and project into the
  inbox on each mutation.

The first option is strictly simpler and matches the rest of the comms
shell treating the `Arc<RwLock<TrustedPeers>>` as canonical. Concrete
proposal: crate `meerkat-comms`, file `meerkat-comms/src/inbox.rs`,
replace `ClassifiedInboxQueue.trusted_peers` with a reference read through
`IngressClassificationContext`. Small follow-up PR.

Not affected by any in-flight Wave 1 PR.

---

### 3. `subscriber_registry` + `interaction_stream_registry` (comms-runtime maps)

Location: `meerkat-comms/src/runtime/comms_runtime.rs:952–953` (fields on
`CommsRuntime`); maintained on correlation insert/remove at
`comms_runtime.rs:451`, `476`, `575`, `598`, `654`, `657`, `1660`, `1672`,
`1679`, `1720`, `1743`, `1763`, `1766`. Shape: map from interaction id →
(subscriber channel, stream entry).

- **Canonical source.** Not clearly owned. Interaction lifecycle (outbound
  request-in-flight, inbound response-pending, reservation subscriber) is
  today scattered: partly in these maps, partly in in-flight
  `tokio::time::timeout` calls, partly in the supervisor bridge's own
  correlation book-keeping.
- **Rebuild trigger sites.** Each send/reserve fans out an insert; each
  terminal-response drain fans out a remove. No rebuild-from-canonical
  path exists; the map *is* the canonical truth for live subscriber
  channels. A process restart drops them.
- **Staleness policy.** None. Leaks on partial failure (send failed after
  insert but before the caller cancels), orphans on timer-only cleanup,
  no reconciliation.
- **Semantic impact of staleness.** YES. Missing an entry means a valid
  terminal response fires on a dropped channel (subscriber never sees it);
  leaked entries hold memory and can match future correlation ids if the
  id space isn't fully unique (pre-existing `PeerCorrelationId` newtype
  fix addresses the id side of this separately).

**Verdict: Shadow truth (action required).**

Explicitly scoped to **W1-A** ("Collapse peer-interaction lifecycle into
MeerkatMachine"). Per the issue, W1-A replaces these maps with either
DSL-generated read seams or explicit projections with rebuild triggers.
No action in this PR; W1-A is the active writer.

If W1-A lands as pure DSL authority (preferred), this entry closes. If
W1-A retains a projection, the projection rebuild trigger documentation
must be added to this audit in a follow-up pass.

---

### 4. `Roster.state` (`RosterEntry.state: MemberState`, phase 5G overlay)

Location: `meerkat-mob/src/roster.rs:85–95` (field), `meerkat-mob/src/roster.rs:603`
(`sync_retiring_projection`); `meerkat-mob/src/runtime/actor.rs:768`
(`sync_retiring_projection_into_roster`), called at `actor.rs:4220`,
`actor.rs:5673`.

- **Canonical source.** `mob_dsl::MobMachineAuthority.state.member_state_markers`
  — a map from DSL-runtime-id string → `MobMemberState::{Active, Retiring}`.
  DSL owns the authority; `MemberState` is explicitly documented as
  read-only projection (`roster.rs:87–94`).
- **Rebuild trigger sites.** `MobActor::sync_retiring_projection_into_roster`
  is called on exactly the transitions documented in the doc comment
  (Retire / RetireMember / ObserveRuntimeRetired / DestroyMob /
  ObserveRuntimeDestroyed). Callers are the actor itself.
- **Staleness policy.** Push-only. Staleness is bounded to the window
  between DSL transition and projection call within the same actor loop
  iteration — both under actor ownership.
- **Semantic impact of staleness.** NO for decisions; `MemberState` is
  consumed by serialization-and-read callers (clone/serialize into
  `RosterEntry` for read surfaces). The doc comment at `roster.rs:87–94`
  explicitly pins it as a convenience projection so cloned/serialized
  values don't need a back-channel to DSL. All gating decisions consult
  DSL directly.

**Verdict: Compliant projection.**

Caveat: the projection's correctness depends on every DSL-mutating
transition calling `sync_retiring_projection_into_roster`. Today that
set is small and documented; if new transitions touch
`member_state_markers` in the future, the contract is informal. A
typed-effect pattern (like W2-E proposes for session context) would
formalize it — noted as a future tightening, not action-required today.

---

### 5. `MobActor.phase_watch_tx` + `MobHandle.phase_watch_rx`

Location: `meerkat-mob/src/runtime/actor.rs:297–305` (tx field with
doc comment); `meerkat-mob/src/runtime/handle.rs:552–559` (rx field with
doc comment); writer: `apply_dsl_input` (`actor.rs:724`) and
`apply_dsl_signal` (`actor.rs:740`).

- **Canonical source.** `mob_dsl::MobMachineAuthority.state.lifecycle_phase`
  inside the actor task. The `watch` is a dogma-#13 projection explicitly
  documented as such (`actor.rs:297–304`).
- **Rebuild trigger sites.** Exactly two: the two DSL-input/signal
  helpers. Both are the sole seams through which the actor advances
  DSL phase (verified by `actor.rs:721`, `739`). The doc comment
  promises "once more right before the actor exits" — verify in
  shutdown path before declaring compliant.
- **Staleness policy.** Push on every phase-changing DSL transition;
  receiver reads the *current* cached value synchronously. After actor
  exit the last-published value is retained indefinitely (that is the
  intent — handle's `status()` falls back to the cached value once the
  command channel has closed).
- **Semantic impact of staleness.** NO. `MobHandle::status()` is async
  and fallible (`shadow_state_absent.rs:22–31` compile-time test locks
  this) and routes through the actor command channel *first*; the watch
  is only the fallback for post-actor-exit reads where the command
  channel is closed. At that point the DSL authority itself no longer
  exists, so the cached terminal value is the only signal that can be
  returned — by definition not stale relative to anything that is
  still mutating.

**Verdict: Compliant projection.**

Locked in by the existing `shadow_state_absent.rs` test, which
compile-time-asserts the sync-infallible `AtomicU8` shadow cannot
return.

---

### 6. `MobActor.lifecycle_phase_projection`

Location: confirmed *absent*. The Phase-5G cut removed
`lifecycle_phase_projection: Arc<AtomicU8>` from `MobActor` and
`state: Arc<AtomicU8>` from `MobHandle`. Lock-in test:
`meerkat-mob/tests/shadow_state_absent.rs`.

- **Canonical source.** DSL `lifecycle_phase` (see field 5 for the
  projection seam that replaced the atomic).
- **Rebuild trigger sites.** N/A — field does not exist.
- **Staleness policy.** N/A.
- **Semantic impact of staleness.** N/A.

**Verdict: Compliant projection (field was deleted; `phase_watch` is
its replacement, audited as field 5).**

---

### 7. `RealtimeAttachmentStatus` (runtime-facing projection of DSL realtime binding)

Location: `meerkat-runtime/src/meerkat_machine/dispatch_control.rs:435`
(pure function `project_realtime_attachment_status`), consumed through
`SessionServiceRuntimeExt::realtime_attachment_status` at
`meerkat-runtime/src/meerkat_machine/traits.rs:159`.

- **Canonical source.** Two DSL fields:
  `MeerkatMachineState.realtime_binding_state` and
  `MeerkatMachineState.realtime_intent_present` (plus the
  `realtime_reattach_required` marker). The projection joins them into
  the six-variant enum.
- **Rebuild trigger sites.** Not a stored projection — computed on every
  `realtime_attachment_status(&session_id)` call from the current DSL
  state under the DSL mutex. No cache.
- **Staleness policy.** Cannot go stale: read-through-mutex function, no
  storage.
- **Semantic impact of staleness.** N/A (no cached value).

**Verdict: Compliant projection.**

The gold-standard shape: canonical DSL fields, pure read-through
function, zero cache. Cross-surface consumers (REST, MCP, RPC) all call
through the same `SessionServiceRuntimeExt` seam.

---

### 8. `tool_visibility_state` (session metadata)

Location: store at `meerkat-core/src/session.rs:762–776`
(`set_tool_visibility_state` / `tool_visibility_state`); write sites in
`meerkat-core/src/agent/runner.rs` (261, 419, 618) and
`meerkat-core/src/agent/builder.rs` (353, 411); persistence rollback at
`meerkat-session/src/persistent.rs:75`
(`rollback_tool_visibility_state_snapshot`).

- **Canonical source.** Session metadata map (`metadata` on `Session`).
  Serialized as durable JSON under `SESSION_TOOL_VISIBILITY_STATE_KEY`.
  This *is* the canonical source, not a projection of anything else.
- **Rebuild trigger sites.** N/A — durable metadata.
- **Staleness policy.** N/A.
- **Semantic impact of staleness.** N/A; it is not a projection.

**Verdict: Not a projection** (canonical durable state). Included because
the audit brief listed it as projection-shaped; the name has "state" in
it but the owner is the session metadata map itself. DSL exposes it
through `SessionToolVisibilityState`
(`meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` catalog
entries at lines 18/28). No action.

---

### 9. `MemberVoiceIntent` (expected-gone-post-0.6)

Location: grep across the Rust tree returns no production-code
matches. Only mentions are in `docs/architecture/realtime-259-audit.md`,
`DELETE_ME_WHEN_DONE_meerkat_06_findings.md`, and
`.claude/skills/meerkat-architecture/references/realtime-attachment.md`
(all documentation).

- **Canonical source.** N/A — field does not exist in code.
- **Rebuild trigger sites.** N/A.
- **Staleness policy.** N/A.
- **Semantic impact of staleness.** N/A.

**Verdict: Compliant (deleted as planned).** No action.

---

### 10. MobActor runtime-only maps (`run_tasks`, `run_cancel_tokens`, `flow_streams`, `mcp_servers`, `autonomous_initial_turns`, `retired_event_index`, `pending_spawns`, `restore_diagnostics`, `edge_locks`)

Location: `meerkat-mob/src/runtime/actor.rs:268–292`.

These are the `BTreeMap` / `HashSet` / `HashMap`-shaped fields on
`MobActor`. The roadmap specifically called out "any `BTreeMap<K, V>` on
a runtime/actor struct that duplicates DSL state" — this entry confirms
the current ones do not.

- **Canonical source.** Each map owns shell-mechanical state that has no
  DSL counterpart:
  - `run_tasks`, `run_cancel_tokens`: tokio task handles and cancellation
    tokens for in-flight flow runs. Not semantic state, process-local
    concurrency plumbing.
  - `flow_streams`: live `mpsc::Sender` handles for run event streaming.
    Same category as above.
  - `mcp_servers`: cached MCP server connection entries. Actual MCP
    server state is DSL-owned (`mcp_server_states` map); this is the
    transport cache.
  - `autonomous_initial_turns`: per-member initial-turn join handles.
    Shell-mechanical.
  - `retired_event_index`: set of event ids already processed. Runtime
    dedupe cache over the event ledger. Rebuildable from the ledger
    itself, but used only within the same actor incarnation so rebuild
    is trivial (empty on start).
  - `pending_spawns`: wrapped by `PendingSpawnLineage`. Its semantic
    counterpart `MobOrchestratorAuthority.pending_spawn_count` is DSL-
    owned; the lineage struct carries the task/metadata coupling
    (see `xtask/src/ownership_ledger.rs:2118–2132`).
  - `restore_diagnostics`: error bookkeeping for restore failures;
    purely informational.
  - `edge_locks`: concurrency primitives, not state.
- **Rebuild trigger sites.** Not projections of DSL state; they are
  their own canonical owners for shell-mechanical truths (task handles,
  channels, timers).
- **Staleness policy.** N/A — canonical.
- **Semantic impact of staleness.** Bounded: none of these affect
  semantic decisions. DSL owns the *can we transition?* answer; these
  maps only affect *how* the shell executes the transition.

**Verdict: Not a projection** (shell-mechanical canonical state).
`pending_spawns` is coupled to the DSL `pending_spawn_count` via the
ownership-ledger contract and is the closest thing to a shadow among
these; noted as an audit target for any future tightening, but not
action-required today.

---

### 11. `RealtimePendingTurn` / `product_turn_in_flight` / `product_turn_committed` / `product_output_started` (realtime WS locals)

Location: `meerkat-rpc/src/realtime_ws.rs:904`, `915–917`. Co-located
with field 1 but tracking "is there a provider turn in flight" rather
than session freshness.

- **Canonical source.** Not clearly owned. The issue #264 analysis and
  W3-H ("Realtime turn lifecycle as explicit machine state") identify
  this as the next-wave target: these locals are re-derived from provider
  event kinds and `stop_reason` dispositions, with the `logical_turn_completed`
  helper serving as the local decision function.
- **Rebuild trigger sites.** Every provider event update in the main
  loop, with ad-hoc `match` blocks on event kind.
- **Staleness policy.** Tied to provider event ordering. The s72
  post-mortem identified cases where a test harness' assumed ordering
  didn't match product code's actual emission order — so "stale" here
  means "test harness asserted on a different state sequence than the
  product runs through."
- **Semantic impact of staleness.** YES in the test-harness sense (false
  negatives on test assertions). Potentially yes in product when the
  locals gate a later decision (projection-refresh deferral in field 1
  depends on `product_turn_in_flight`).

**Verdict: Shadow truth (action required).**

Scoped to **W3-H** ("Realtime turn lifecycle as explicit machine state").
W3-H promotes these to a `RealtimeTurnPhase` DSL enum with typed
transitions and a terminal effect. No action in this PR.

---

### 12. `session_runtime`-level ownership of peer ingress / comms drain (dispatch-time ownership)

Location: Not a single field; the ownership is implicit in how
`session_runtime.rs` configures comms drain vs. `MobActor` configures
its own mob-owned drain.

- **Canonical source.** Not owned by DSL on `main`. Ownership is inferred
  by call-site ordering.
- **Rebuild trigger sites.** Any call to `maybe_spawn_comms_drain` from
  session runtime; any mob spawn that configures its own.
- **Staleness policy.** None — the two paths can race. The s71 incident
  found a case where `start_turn_via_runtime` reconfigured a live
  mob-owned drain because the session runtime didn't know it was
  mob-owned.
- **Semantic impact of staleness.** YES. A reconfigured drain can
  silently swap the active peer ingress identity mid-turn.

**Verdict: Shadow truth (action required).**

Scoped to **W2-G** ("Typed ownership of transport capabilities"), which
promotes ownership to a typed DSL `peer_ingress_owner: PeerIngressOwner`
enum with explicit guards. No action in this PR.

---

## Summary table

| # | Field | Verdict | Seeded into |
|---|-------|---------|-------------|
| 1 | `projection_refresh_dirty` + `projection_known_updated_at` | Shadow truth | W2-E |
| 2 | `ClassifiedInboxQueue.trusted_peers` | Shadow truth | ITEM-6 (new sub-PR) |
| 3 | `subscriber_registry` + `interaction_stream_registry` | Shadow truth | W1-A |
| 4 | `Roster.state` (`MemberState`) | Compliant projection | — |
| 5 | `MobActor.phase_watch_tx` / `MobHandle.phase_watch_rx` | Compliant projection | — |
| 6 | `MobActor.lifecycle_phase_projection` | Compliant (deleted) | — |
| 7 | `RealtimeAttachmentStatus` | Compliant projection (reference quality) | — |
| 8 | `tool_visibility_state` | Not a projection | — |
| 9 | `MemberVoiceIntent` | Compliant (deleted) | — |
| 10 | MobActor runtime maps (bundle) | Not a projection | — |
| 11 | Realtime-WS turn-in-flight locals | Shadow truth | W3-H |
| 12 | `session_runtime` comms-drain ownership | Shadow truth | W2-G |

Five entries are shadow truth. Four of them are already scoped to named
Wave 2/3 PRs. One is a net-new finding.

---

## ITEM-6 sub-PR seeds

The continuous-fill ITEM-6 lane (issue #264) absorbs wrapper-migration work
that surfaces during audits. Only items not already scoped to a named wave
PR land here.

### Seed A — `ClassifiedInboxQueue.trusted_peers` → canonical read-through

- **Crate / file:** `meerkat-comms/src/inbox.rs`
- **Shape:** delete the local `BTreeSet<PeerId>` on `ClassifiedInboxQueue`;
  make `admit_peer_receive` consult
  `IngressClassificationContext.trusted_peers` directly.
- **DSL owner:** none required — canonical `Arc<RwLock<TrustedPeers>>`
  already exists in `IngressClassificationContext`.
- **Why:** today the local set can drift from canonical trust decisions,
  allowing envelopes from revoked peers or dropping envelopes from newly
  trusted peers. Security-relevant silent divergence.
- **Acceptance note:** `sync_trusted_peer_added` / `sync_trusted_peer_removed`
  helpers become dead and can be removed; the `new_classified` initial
  seeding loop at `inbox.rs:273–275` can be deleted in favor of the
  read-through. Test: add/remove trust after queue creation and verify
  `admit_peer_receive` picks up the change without re-initialization.

No other net-new seeds surfaced in this audit. All other shadow-truth
findings are covered by W1-A, W2-E, W2-G, or W3-H per the Wave 1–3
roadmap.

---

## Audit hygiene notes

- Audit reflects the tree at `97981e314`. If a Wave 1 sibling PR (W1-A)
  lands before this doc merges, the entry for `subscriber_registry` /
  `interaction_stream_registry` should be updated in a follow-up pass to
  reflect the post-W1-A reality (either "field deleted" or "now a
  compliant projection with rebuild trigger X").
- Future tightening opportunity noted on entry 4 (`Roster.state`): the
  informal "every DSL transition that touches `member_state_markers`
  calls `sync_retiring_projection_into_roster`" contract would benefit
  from the same typed-effect pattern W2-E introduces for session
  context. Not action-required today.
- Entry 10 (MobActor runtime maps) is included explicitly so that future
  auditors do not re-open the question for fields whose shape (`BTreeMap`
  on a runtime struct) superficially matches the audit brief but whose
  contents are shell-mechanical by design.

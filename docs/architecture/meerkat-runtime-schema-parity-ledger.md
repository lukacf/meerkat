# Meerkat Runtime/Schema Parity Ledger

## Purpose

This ledger is the checked-in burn-down surface for Meerkat runtime/schema
parity. It is derived from the ignored in-crate audit:

```bash
cargo test -p meerkat-runtime audit_meerkat_runtime_phase_parity_map \
  -- --ignored --nocapture
```

The goal is to drive the Meerkat parity loop in a controlled way:

- keep runtime as the provisional oracle only when real tests and the design
  docs support it
- avoid widening the schema blindly just to make the quotient look smaller
- separate already-verified rows from rows that still need direct probes

There are now two audit layers:

- the acceptance/surface map:

  ```bash
  cargo test -p meerkat-runtime audit_meerkat_runtime_phase_parity_map \
    -- --ignored --nocapture
  ```

- the stricter full-row map that also probes schema `same_surface` rows using a
  schema-aligned runtime field projection plus lower-authority carrier-derived
  control-report summaries where needed:

  ```bash
  cargo test -p meerkat-runtime audit_meerkat_runtime_phase_full_parity_map \
    -- --ignored --nocapture
  ```

## Current Snapshot (2026-04-15)

- Audited mixed-phase pairs: `Attached ↔ Idle`, `Attached ↔ Running`,
  `Running ↔ Stopped`, `Running ↔ Retired`, `Idle ↔ Retired`,
  `Idle ↔ Stopped`, `Attached ↔ Retired`, `Attached ↔ Stopped`,
  `Idle ↔ Running`, `Retired ↔ Stopped`
- Transition-bearing rows in scope: `243`
- Live-probed rows: `243`
- `fixed`: `243`
- `align_schema`: `0`
- `align_runtime`: `0`
- `needs_decision`: `0`

Current state:

- the Meerkat registration-helper tranche is aligned
- the durable tool-visibility tranche is aligned
- the helper/query tranche is aligned
- the formal Meerkat state no longer carries shadow publication/provenance
  visibility fields that do not affect transition legality:
  `committed_visibility_revision`, `requested_witnesses`,
  `filter_witnesses`
- the formal Meerkat state no longer carries the LLM/capability projection
  layer that is runtime-owned but does not affect transition legality or
  routed seam effects: `current_llm_identity`, `current_capability_surface`,
  `capability_surface_status`, `capability_base_filter`,
  `inherited_base_filter`
- the top-level Meerkat machine no longer carries the dead wake/process
  mirrors: `wake_pending` and `process_pending` were constant `FALSE` across
  the truthful reachable graph and their removal kept the exact parity surface
  unchanged
- the stale handwritten `RuntimeControlAuthority` wake/process branch is now
  gone too: `SubmitWork`, `AdmissionAccepted`, `AdmissionRejected`,
  `AdmissionDeduplicated`, `wake_pending`, and `process_pending` were dead
  helper-only control semantics that the live runtime no longer exercised
- the parity snapshot now carries the remaining live typed
  `post_admission_signal` carrier without the deleted control booleans, and
  exact full-row pair parity stayed green across the current 10-pair frontier
- `current_run_id` is now absorbed back into the top-level Meerkat formal
  state and clears on the same terminal/control paths as the live runtime
- the driver no longer carries a private `RunReturnPhase` shadow enum for
  coarse run return; the remaining `pre_run_phase` bookkeeping now uses the
  same real `RuntimeState` projection (`Idle` / `Attached` / `Retired`) that
  the checked-in machine and parity spine already use
- the generic `RuntimeDriver` trait no longer exposes the coarse lifecycle
  verbs `retire`, `reset`, or `destroy`; those nouns are now machine/control
  plane owned, with only concrete driver persistence helpers retaining the
  mechanics needed to realize the machine-owned transition and durable commit
- the generic `RuntimeDriver` trait no longer exposes coarse stop control
  either; `stop` now lives as a concrete driver realization helper while the
  machine/control-plane path remains the only layer that speaks the lifecycle
  noun
- runtime-loop batch start preparation no longer realizes coarse run truth
  inline; the checked-in machine module now owns the atomic
  `start_run + stage_batch + unwind-on-failure` sequence through a dedicated
  helper, while terminal return still remains the next separate tranche
- coarse run-start / run-return legality is no longer committed by
  `EphemeralRuntimeDriver` either: the checked-in machine module now validates
  active run identity, commits the `Running` projection, and clears
  `current_run_id` / `pre_run_phase` on terminal return, while the driver only
  realizes ingress/ledger mechanics for `BoundaryApplied`, `RunCompleted`, and
  `RunFailed`
- coarse stop legality no longer lives in the driver either: `MeerkatMachine`
  now owns the legal source phases for stop plus the `Stopped` projection,
  while driver stop paths only execute the already-decided cleanup mechanics
  (abandoning non-terminal inputs, clearing queue state, clearing silent
  intents)
- coarse destroy legality no longer lives in the driver either: the machine
  already owns the legal source states and binding guardrails for destroy, and
  the driver now only realizes the already-decided `Destroyed` cleanup
  mechanics
- coarse retire/reset legality no longer lives in the driver either: the
  checked-in machine now owns the legal source phases plus the resulting
  `Retired` / `Idle` projections, while the driver only computes retire
  reports and reset cleanup mechanics
- attached steered `AcceptWithCompletion` is no longer treated as a queue-only
  self-loop in the formal model: when `request_immediate_processing=true`, the
  checked-in Meerkat machine now models the runtime’s `Attached -> Running`
  jump and binds the fresh `current_run_id` / `pre_run_phase`
- running queued `AcceptWithCompletion` is no longer flattened to a passive
  self-loop either: the checked-in Meerkat machine now distinguishes the live
  `InterruptYielding` admission branch from the passive queued branch, and the
  targeted runtime/model regression for running peer-message admission is green
- the hidden ingress booleans `wake_requested` and `process_requested` are now
  gone as well: they had become write-only shadow state under
  `RuntimeIngressAuthority`, while the live runtime loop and exact audits were
  already driven by emitted wake/process effects plus the typed
  `post_admission_signal`
- the helper no longer owns the immediate post-admission control consequence
  either: consumed-on-accept vs queued routing now goes through explicit
  ingress transition variants, and the runtime computes
  `WakeLoop` / `InterruptYielding` / `RequestImmediateProcessing` directly from
  the resolved policy before executing the ingress transition
- Meerkat now formally owns `silent_intent_overrides` as checked-in ingress
  state instead of leaving it below the machine boundary; exact audited parity
  stayed green after lifting it, the targeted runtime/model regression for
  `SetSilentIntents` is green, and the remaining helper-side mirror is now
  gone too: reset / stop / destroy clear the driver-owned set directly and the
  diagnostic/formal projection no longer reads it from `RuntimeIngressAuthority`
- live recycle no longer detours through a handwritten control-side
  recovery hop: the runtime helper now realizes recycle as the same direct
  control projection the checked-in Meerkat machine already models
  (`Idle/Retired -> Idle`, `Attached -> Attached`), and exact parity stayed
  green after removing the extra helper-only recycle completion transition
- the top-level Meerkat machine no longer carries the filter mirror pair
  `active_filter` / `staged_filter`; the authoritative
  `MachineToolVisibilityOwner` still owns the real filter state, exact parity
  stayed green, and the truthful Meerkat reachable graph fell from `38,945`
  back to `11,858` states while the raw/phase quotient stayed at `385 / 390`
- the pure query/helper surface is now explicitly carried as
  `surface_only_inputs` instead of formal self-loops:
  `ContainsSession`, `SessionHasExecutor`, `SessionHasComms`,
  `OpsLifecycleRegistry`, `InputState`, `ListActiveInputs`,
  `RuntimeState`, `LoadBoundaryReceipt`
- the reducer/control tranche is aligned
- the cross-pair public-phase expansion tranche is aligned
- Meerkat acceptance parity is now green across the current public-phase
  frontier
- Fast-loop finding: `silent_intent_overrides` is **not** a safe collapse even
  though Hopcroft attributes `48.2%` collapse pressure to it. The live runtime
  still applies that set before ingress transitions to force wake/apply policy
  for matching peer intents, so we are treating it as an under-modeled
  behavior carrier rather than simplifying the wrong truth.
- Fast-loop landed: `drain_running` was removed from the checked-in Meerkat
  state after the runtime review confirmed it is only a projection of
  lower-authority drain state. TLC stayed green, the raw quotient stayed flat
  at `459`, and the truthful Meerkat graph fell from `19,459` to `14,536`
  reachable states.
- Fast-loop landed: `active_requested_deferred_names` was removed from the
  checked-in Meerkat state while the canonical `SessionToolVisibilityState`
  and visibility owner stayed intact below the machine boundary. TLC stayed
  green, the raw quotient stayed flat at `459`, and the truthful Meerkat graph
  fell again from `14,536` to `11,293` reachable states.
- Fast-loop landed: `peer_ingress_configured` was removed from the checked-in
  Meerkat state after the runtime review confirmed it is only a projection of
  lower-authority ingress/drain slot truth. TLC stayed green, the raw quotient
  dropped from `459` to `340`, and the truthful Meerkat graph fell again from
  `11,293` to `7,255` reachable states.
- Fast-loop landed: `staged_requested_deferred_names` was removed from the
  checked-in Meerkat state while the canonical visibility owner retained the
  staged deferred-name set below the machine boundary. The machine now enforces
  the active-vs-staged deferred-name invariants directly on
  `PublishCommittedVisibleSet` input bindings instead of storing a top-level
  mirror. TLC stayed green, the raw quotient stayed flat at `340`, and the
  truthful Meerkat graph fell again from `7,255` to `3,835` reachable states.
- Fast-loop landed: `active_visibility_revision` was removed from the
  checked-in Meerkat state while the machine collapsed boundary publication to
  a single staged revision token and publish-time legality. The canonical
  active revision still lives in the lower-authority visibility owner. TLC
  stayed green, the raw quotient dropped from `340` to `260`, and the truthful
  Meerkat graph fell again from `3,835` to `1,773` reachable states.
- Fast-loop landed: `staged_visibility_revision` was removed from the
  checked-in Meerkat state too. The machine now treats visibility publication
  as pure publish-time legality over owner-provided revision inputs rather than
  as top-level mirrored revision state. TLC stayed green, the raw quotient
  dropped again from `260` to `196`, and the truthful Meerkat graph fell from
  `1,773` to `742` reachable states.
- Fast-loop landed: `attachment_live` was removed from the checked-in
  Meerkat state after collapsing interrupt/cancel/stop legality onto phase
  rather than a second top-level "live" bit. TLC stayed green, the raw
  quotient dropped again from `196` to `133`, and the truthful Meerkat graph
  fell from `742` to `474` reachable states.
- Fast-loop landed: `active_generation` was removed from the checked-in
  Meerkat state and from the coarse `RuntimeBound` / `RuntimeRetired` /
  `RuntimeDestroyed` effect payload mirror. The live runtime never used the
  stored value for legality, the seam never consumed it on the return leg, and
  rerunning TLC/Hopcroft stayed exactly flat at `474` reachable with raw /
  phase / full quotients `133 / 138 / 455`, confirming it was a fully
  correlated binding-shadow field rather than behavior-bearing state.

## Full-row Snapshot (2026-04-15)

- Audited mixed-phase pairs: `Attached ↔ Idle`, `Attached ↔ Running`,
  `Running ↔ Stopped`, `Running ↔ Retired`, `Idle ↔ Retired`,
  `Idle ↔ Stopped`, `Attached ↔ Retired`, `Attached ↔ Stopped`,
  `Idle ↔ Running`, `Retired ↔ Stopped`
- Transition-bearing full rows in scope: `260`
- Live-probed rows: `260`
- aligned: `260`
- mismatched: `0`
- unprobed: `0`

Current exact-parity state:

- acceptance parity is still green
- modeled formal-state parity is green at `145 / 145`
- exact full-row parity is green at `260 / 260`
- the pair audit now compares runtime behavior against the simulated schema
  outcome from the same representative pre-state rather than against static
  transition topology
- the exact observable audit now also composes in lower-authority ledger
  carrier summaries for control-plane report counts such as
  `DestroyReport.inputs_abandoned`
- visibility publication/provenance facts that remain runtime-owned but do not
  change Meerkat transition legality have been pushed below the top-level
  formal machine boundary rather than kept as shadow state
- LLM/capability projection facts are now treated the same way: they remain
  runtime-owned and exact in the live runtime, but they are no longer modeled
  as top-level Meerkat machine state because they do not affect command
  legality, phase changes, or routed effect identity
- the old wake/process pending bits are also gone from both the top-level
  formal machine and the handwritten runtime-control helper; the truthful
  graph never drove those mirrors away from `FALSE`, and exact parity stayed
  green after both cuts
- that closes the stale handwritten wake/process branch, and the handwritten
  recover workflow is gone too: `RecoverRequested` / `RecoverySucceeded`,
  `ResumeRequested`, and the entire `RuntimeControlAuthority` reducer were
  removed, so the runtime now exposes only real machine phases and the live
  `recover()` path runs directly through the checked-in runtime/ingress seam
- coarse control truth now lives directly with the runtime driver in the same
  shape the checked-in machine models: `phase`, `current_run_id`, and the
  three legal run-return targets (`idle` / `attached` / `retired`)
- the dead top-level active-work slice is also gone: `active_work_id` never
  became `Some(...)` in the truthful graph, the old `has_active_work`-gated
  completion/operation slice had zero reachable edges, and exact parity stayed
  green after removing both
- the Meerkat verification `ToolFilter` domain is no longer singleton: CI/deep
  now both admit `{"All", "toolfilter_2"}`, which raises the truthful
  reachable state space sharply without changing the exact runtime/schema audit
  frontier
- after broadening that `ToolFilter` domain, we removed the top-level
  `active_filter` / `staged_filter` mirrors as well; exact parity stayed green
  and the truthful pre-absorption Meerkat readout sat at `11,858` reachable
  states with raw/phase/full quotients `385 / 390 / 11,469`
- after deleting `RuntimeControlAuthority` as a semantic reducer and absorbing
  coarse control truth into the runtime path that realizes `MeerkatMachine`,
  the truthful Meerkat readout now sits at `17,384` reachable states with
  raw/phase/full quotients `385 / 390 / 16,995`
- the first ingress-authority absorption slice is now landed too:
  `RuntimeIngressAuthority` no longer owns a coarse `Active` / `Retired` /
  `Destroyed` phase. Admission/drain/reset/stop/destroy legality is now owned
  by the checked-in Meerkat lifecycle, while the helper remains only a
  queue/ledger authority for admitted inputs and contributor bookkeeping
- the ingress helper also no longer keeps a second `admitted_inputs` set:
  tracked-input membership is derived directly from the canonical per-input
  lifecycle map, and exact Meerkat parity stayed green after removing that
  duplicate ledger index
- the ingress helper also no longer owns `silent_intent_overrides`; that
  canonical set now lives only on the driver/machine side, and exact Meerkat
  parity stayed green after removing the helper mirror
- the ingress helper no longer emits admission-side wake/process signals for
  newly accepted inputs; that control intent is now decided in the
  driver/machine path and the helper only executes the explicit admit
  transition plus lower-level queue/ledger bookkeeping
- the machine side now owns the admission signal classifier explicitly too:
  accept-time wake / interrupt / immediate-processing intent is derived from
  the resolved `AcceptOutcome` policy plus the steer/immediacy bit, while the
  driver helper no longer caches that admission signal locally
- the ingress helper also no longer derives a second authoritative
  `current_run` projection from contributor state; the live guard path now
  validates contributor `last_run` metadata directly against the control-owned
  run ID, and the diagnostic spine echoes `current_run_id` from control rather
  than from ingress
- the ingress helper now also no longer owns contributor-set / per-input
  run-boundary shadow truth:
  `current_run_contributors`, helper-side `last_run`, and helper-side
  `last_boundary_sequence` are gone, run-event calls carry explicit
  contributor IDs, and the diagnostic spine derives contributor membership plus
  run/boundary metadata from the canonical input lifecycle ledger instead
- terminalization now reconciles back into ingress queue visibility from the
  ledger owner too, so retire/reset/stop/destroy no longer leave ghost queued
  entries behind after the canonical input state has already moved terminal
- the formal machine now also models the `Running -> Retired` terminal return
  that live runtime already used during drain/retire flow, and the runtime-side
  return-phase classifier is now a machine-owned helper instead of a
  driver-local `finish_run_from_current_phase()` decision
- the formal machine now also models the internal retire-drain start path as a
  dedicated `DrainQueuedRunRetired` signal instead of pretending the public
  `Prepare` surface widened to `Retired`; the runtime-side start classifier is
  now a machine-owned helper too
- the pure query surface remains runtime-audited helper behavior, but it is no
  longer counted as formal transition coverage
- the next remaining dogma violation is no longer coarse control truth in a
  separate reducer; it is the remaining handwritten ingress queue/contributor
  owner in `RuntimeIngressAuthority`, which no longer derives a second active
  run identity but still owns contributor queues and reset / stop / destroy /
  recover bookkeeping below the two checked-in machines
- a second checked-in gap on the formal side is now closed too: the
  `meerkat_mob_seam` composition models both the Mob <- Meerkat return leg and
  the forward Mob -> Meerkat runtime-ingress request route. The checked-in seam
  still leaves `WorkRef -> InputId` translation below the machine boundary, but
  it no longer omits the behavior-bearing forward request itself

Interpretation:

- the Meerkat schema is no longer missing obvious acceptance guards on the
  current public-phase frontier
- the top-level modeled formal-state vector is green on the audited frontier
- exact observable parity is also green once the composed audit includes the
  lower-level ledger carrier that actually owns report counts
- the concrete answer to “is the machine under-modeled?” is now sharper:
  the remaining normalization work is about authority boundaries and field
  factoring, not about missing acceptance guards on the audited public surface
- rerunning Hopcroft after the control absorption was necessary: the reachable
  Meerkat graph changed materially (`11,858 -> 17,384`) even though the raw /
  phase quotient stayed flat at `385 / 390`, which means the old
  simplification map was stale but the core behavioral quotient remained
  stable
- rerunning Hopcroft again after modeling retire-drain start was necessary too:
  the truthful Meerkat graph moved again (`17,384 -> 19,467`) and the raw /
  phase quotient rose from `385 / 390` to `466 / 471`, which means the old
  simplification baseline had still been hiding a real lifecycle behavior slice
- removing the helper-owned ingress phase did not reopen any Meerkat parity
  gap: acceptance parity stayed `243 / 243`, exact full-row parity stayed
  `260 / 260`, and modeled-state parity returned to `145 / 145` after
  tightening `SetSilentIntents` from `Stopped` to remain a no-op at the helper
  layer too
- removing the duplicate ingress `admitted_inputs` index did not reopen any
  Meerkat parity gap either: the helper now derives tracked-input membership
  from lifecycle ownership instead of keeping a second set in sync
- removing the helper-owned ingress `terminal_outcome` cache did not reopen
  any Meerkat parity gap either: per-input terminal outcome truth already
  lived in the canonical input ledger, so ingress now emits terminal effects
  without storing a second copy
- removing the dead `request_immediate_processing` parameter from ingress
  admission did not reopen any Meerkat parity gap either: immediate-processing
  semantics are now plainly owned by the checked-in Meerkat accept /
  post-admission signal path, not by the queue helper boundary
- removing ingress-owned `Recover` semantics did not reopen any Meerkat parity
  gap either: recovery now normalizes canonical input lifecycle truth in the
  ledger first and then rebuilds ingress mechanically from that ledger, instead
  of asking ingress to classify recovery semantics on its own
- removing ingress `Retire` / `Reset` / `Stop` / `Destroy` did not reopen any
  Meerkat parity gap either: those coarse lifecycle transitions were already
  owned by driver + ledger abandonment semantics, so the helper surface was
  just shadow ceremony around queue reconciliation
- runtime-loop batch start is no longer realized inline either: the checked-in
  Meerkat module now owns `start_run + stage_batch + unwind` as one helper
  operation, so the loop no longer acts as a second semantic owner of run
  start legality
- runtime-loop terminal sequencing is also no longer realized inline: the
  checked-in Meerkat module now owns the `BoundaryApplied -> RunCompleted` and
  `RunFailed` realization helpers, so the loop no longer carries a second copy
  of terminal run semantics below the machine boundary
- ordinary attachment projection is no longer realized through direct driver
  lifecycle verbs either: `PrepareBindings`, stale loop republish, and session
  unregister now project `Idle <-> Attached` through machine-owned helpers
  rather than calling `attach()` / `detach()` as semantic reducers
- contributor-set legality for `StageDrainSnapshot`, `BoundaryApplied`, and
  `RunCompleted` is no longer helper-owned either: the checked-in Meerkat
  machine now validates queue-prefix, staged, and applied-pending-consumption
  preconditions before ingress applies the already-decided queue/lifecycle
  updates
- runtime-loop batch selection and boundary classification are no longer
  helper-owned either: the checked-in Meerkat machine now chooses the next
  steer/prompt batch and classifies its `RunStart` vs `RunCheckpoint`
  boundary from stored ingress metadata before the helper applies the
  already-decided queue/lifecycle updates
- failing contributor ownership is no longer rediscovered in the driver:
  the checked-in Meerkat machine now passes the explicit staged contributor
  set into `RunFailed`, instead of letting the runtime scan staged inputs and
  infer "which inputs were this run" locally on failure
- coarse `Stopped` / `Destroyed` projection is no longer written by driver
  cleanup helpers: the checked-in Meerkat machine now commits those phase
  changes first, and driver stop/destroy paths only perform already-decided
  cleanup/persistence mechanics
- coarse input-admission legality is no longer duplicated in the driver
  either: `MeerkatMachine::Ingest` remains the checked-in owner of phase-gated
  admission, while `EphemeralRuntimeDriver::accept_input()` now assumes the
  caller has already passed that coarse lifecycle gate and focuses on
  durability/policy/ledger mechanics
- accepted-input semantic classification is no longer helper-owned either:
  policy resolution, silent-intent override, handling-mode derivation, and
  admission-plan derivation now flow through the machine-owned
  `accept.rs::resolve_admission(...)` helper. The driver still performs the
  mechanical pieces it uniquely owns (durability/idempotency checks,
  supersession lookup, ingress effect application, and final ledger
  realization), but it no longer decides the semantic admission disposition on
  its own
- recover normalization/report semantics are no longer split across the
  drivers either: `machine_recover_ephemeral_driver(...)` and
  `machine_recover_persistent_driver(...)` now own recovery normalization,
  ingress rebuild, runtime-state realization, and report synthesis in the
  checked-in machine module. `EphemeralRuntimeDriver` and
  `PersistentRuntimeDriver` still perform the mechanics they uniquely own
  (store I/O and final durability commit for the persistent case), but they no
  longer each define their own recovery meaning below the machine boundary
- contributor staging / boundary / completion / replay are no longer routed
  through ingress-helper transition methods in production either:
  `EphemeralRuntimeDriver` now applies the already-decided queue/lifecycle
  mutations directly against lower-level ingress storage plus per-input
  lifecycle, while the old ingress transition methods remain only as focused
  table-test scaffolding. That removes the last obvious production transition
  surface below `MeerkatMachine` for run-contributor progression
- the runtime loop now enters that contributor mutation path only through
  explicit machine-owned realization helpers too:
  `machine_realize_stage_batch`, `machine_realize_boundary_applied`,
  `machine_realize_run_completed`, and `machine_realize_run_failed` sit in the
  checked-in machine layer and call the concrete drivers only for lower-level
  queue/ledger/persistence mechanics. The plain driver verbs remain available
  for direct contract tests, but production no longer uses them as semantic
  entry points
- the formal seam is no longer missing the forward Mob -> Meerkat ingress
  request: `SubmitWork` now emits `RequestRuntimeIngress`, the composition
  routes that effect into Meerkat `Ingest`, and that route now carries the
  opaque `work_id` plus `origin` as well as the member runtime binding. The
  truthful Mob quotient moved to raw/phase/full `207 / 209 / 1323`, which is
  the right signature for adding real routed behavior rather than more
  representative shadow state
- the checked-in transitions now actually carry those routed fields through
  execution too: `SubmitWorkRunning` binds `origin`, and Meerkat `Ingest`
  transitions bind `runtime_id`, `work_id`, and `origin` instead of widening
  the route shape without threading the new facts through the formal path
- coarse input-admission legality is now wholly machine-owned: the outer
  Meerkat ingress/prepare shell no longer pre-rejects phases on its own and
  `EphemeralRuntimeDriver::accept_input()` no longer repeats the same
  phase-gated lifecycle check. The checked-in `MeerkatMachine::Ingest` path is
  now the sole owner of that coarse legality gate, while the driver focuses on
  durability, policy, and ledger mechanics
- the generic `RuntimeDriver::on_run_event` hook is gone too: the checked-in
  machine and tests now target concrete `boundary_applied`, `run_completed`,
  and `run_failed` driver helpers instead of routing terminal lifecycle through
  an opaque catch-all run-event surface

## Resolution Rubric

| Label | Meaning |
| --- | --- |
| `fixed` | The live runtime probe agrees with the current schema classification for this pair/input row. |
| `align_schema` | Runtime behavior and the current architecture docs already point one way strongly enough that the schema should be widened or reshaped to match. There are no current Meerkat rows in this bucket. |
| `align_runtime` | The schema is carrying the intended contract and the runtime should be tightened to match it. There are no current Meerkat rows in this bucket. |
| `needs_decision` | We do not yet have enough evidence to align either side safely. There are no current Meerkat rows in this bucket. |

## Family Read

- `RegisterSession`, `StagePersistentFilter`, `RequestDeferredTools`, and
  `PublishCommittedVisibleSet` are now `fixed` across the audited frontier.
  The schema was widened to match the runtime’s extant-binding and durable
  visibility-owner behavior.
- The helper/query family (`EnsureSessionWithExecutor`, `SetSilentIntents`,
  `ContainsSession`, `SessionHasExecutor`, `SessionHasComms`,
  `OpsLifecycleRegistry`, `InputState`, `ListActiveInputs`) is also now
  `fixed` across the audited frontier.
- A first ingress-authority absorption slice is now landed too:
  `SetSilentIntents` no longer depends on lower-authority hidden state because
  `silent_intent_overrides` is part of the checked-in Meerkat model. A broader
  admitted-input ledger absorption was explored and deliberately deferred
  because `Ingest` / `Prepare` still need a wider payload/modeling tranche for
  exact parity.
- The reducer/control family is now also closed. Three sub-results matter:
  - direct runtime probes confirmed that `Recycle`, `Prepare`, `Commit`, and
    `Fail` already matched the current schema surface for the audited frontier
  - the schema was widened to match the runtime’s existing acceptance surface
    for `Abort*`, `Wait`, `Ingest`, `PublishEvent`, and `Accept*`
  - `RuntimeState` and `LoadBoundaryReceipt` are now carried with the other
    pure query helpers as `surface_only_inputs`
  - `InterruptCurrentRun` and `CancelAfterBoundary` are now modeled as
    attached-loop control commands: `Attached` accepts them as self-loops,
    while `Running` keeps the active-work surface and `Idle` / `Retired` /
    `Stopped` still reject them

## Pair Ledger

- `Attached ↔ Idle`: `25` interesting, `25` probed, `25` fixed, `0`
  mismatches, `0` unprobed
- `Attached ↔ Running`: `28` interesting, `28` probed, `28` fixed, `0`
  mismatches, `0` unprobed
- `Running ↔ Stopped`: `25` interesting, `25` probed, `25` fixed, `0`
  mismatches, `0` unprobed
- `Running ↔ Retired`: `27` interesting, `27` probed, `27` fixed, `0`
  mismatches, `0` unprobed
- `Idle ↔ Retired`: `19` interesting, `19` probed, `19` fixed, `0`
  mismatches, `0` unprobed
- `Idle ↔ Stopped`: `22` interesting, `22` probed, `22` fixed, `0`
  mismatches, `0` unprobed
- `Attached ↔ Retired`: `26` interesting, `26` probed, `26` fixed, `0`
  mismatches, `0` unprobed
- `Attached ↔ Stopped`: `26` interesting, `26` probed, `26` fixed, `0`
  mismatches, `0` unprobed
- `Idle ↔ Running`: `28` interesting, `28` probed, `28` fixed, `0`
  mismatches, `0` unprobed
- `Retired ↔ Stopped`: `17` interesting, `17` probed, `17` fixed, `0`
  mismatches, `0` unprobed

The last acceptance mismatch cluster was `Attached ↔ Running` on
`InterruptCurrentRun` and `CancelAfterBoundary`. Closing it required modeling
the existing attached-loop control-channel behavior rather than tightening the
runtime. The last exact observable mismatch after that was `Destroy`, and it is
now closed in the composed audit by projecting the lower-level ledger carrier
that owns abandoned-input counts.

## Batch Status

### Batch A: Registration and durable visibility

Status: complete

Closed rows:

- `RegisterSession`
- `StagePersistentFilter`
- `RequestDeferredTools`
- `PublishCommittedVisibleSet`

Outcome:

- the original Meerkat schema/runtime mismatch cluster is gone for the audited
  pairs
- the top-level formal machine now keeps the visibility fields that drive
  legality (`filter`, deferred-name sets, staged/active revisions) while
  treating publication/provenance details as lower-authority carrier state

### Batch B: Helper/query parity

Status: complete

Closed rows:

- `EnsureSessionWithExecutor`
- `SetSilentIntents`
- `ContainsSession`
- `SessionHasExecutor`
- `SessionHasComms`
- `OpsLifecycleRegistry`
- `InputState`
- `ListActiveInputs`
- `RuntimeState`
- `LoadBoundaryReceipt`

Outcome:

- the helper/query family is live-probed and aligned for the audited pairs
- the pure read-only Meerkat queries are now modeled as surfaced-only runtime
  inputs rather than formal self-loops, matching the existing Mob query
  boundary

### Batch C: Reducer/control probe expansion

Status: complete

Closed rows:

- `Abort`
- `AbortAll`
- `Wait`
- `Ingest`
- `PublishEvent`
- `AcceptWithCompletion`
- `AcceptWithoutWake`
- `Recycle`
- `Prepare`
- `Commit`
- `Fail`

Outcome:

- the runtime probe surface now covers the full currently targeted Meerkat
  reducer/control acceptance frontier
- the schema widening needed for this batch is landed and verified
- the first payload-sensitive accept gap is also closed inside this batch:
  attached steered `AcceptWithCompletion` is now modeled as an immediate
  `Attached -> Running` path rather than being collapsed into the queue-only
  accept surface
- there are no current Meerkat acceptance mismatches left in the initial
  three-pair tranche

### Batch D: Cross-pair public-phase expansion

Status: complete

Closed pairs:

- `Attached ↔ Running`
- `Running ↔ Retired`
- `Idle ↔ Stopped`
- `Attached ↔ Retired`
- `Attached ↔ Stopped`
- `Idle ↔ Running`
- `Retired ↔ Stopped`

Outcome:

- the Meerkat runtime parity map now covers the full public-phase frontier we
  care about for the current simplification pass
- the last live mismatch cluster was closed by adding `Attached` self-loops for
  `InterruptCurrentRun` and `CancelAfterBoundary`
- there are no current Meerkat acceptance mismatches left in the 10-pair
  audited frontier

## Next Loop

1. Keep using the acceptance map as the “green frontier” for command-surface
   parity.
2. Treat control-plane report counts such as `DestroyReport.inputs_abandoned`
   as lower-authority carrier facts in the exact observable audit unless and
   until the DSL work deliberately lifts them into the top-level machine.
3. Use the trustworthy post-parity Hopcroft rerun as the current Meerkat
   simplification baseline after the control/ingress owner-reduction cuts:
   raw `19,459 -> 459`, phase `19,459 -> 464`, full `19,459 -> 19,070`,
   TLC `1,814,665 generated / 19,459 distinct / depth 9`.
4. Read that baseline together with the largest-block field projection from
   [`docs/architecture/machine-simplification-proposal.md`](machine-simplification-proposal.md):
   the dominant Meerkat mixed block is now measured as `8,897` states over
   `4,560` extended-state tuples, with `3,096` tuples reused across multiple
   phases.
5. Read that baseline together with the now-green Mob lifecycle-triangle
   ledger in
   [`docs/architecture/mob-runtime-schema-parity-ledger.md`](mob-runtime-schema-parity-ledger.md).
6. The remaining Meerkat edge is now narrower and mostly terminal/batch
   mechanics. `RuntimeIngressAuthority` no longer treats contributor staging,
   boundary application, run completion, replay rollback, or admission
   disposition as helper-owned production transition classification; recovery
   normalization/report semantics are also machine-owned now.
7. Recovery semantics are also less split than before: persistent and
   ephemeral drivers now share the same machine-owned per-input recovery
   classifier, so Accepted/Staged/Applied recovery normalization no longer
   lives as duplicated handwritten logic in both drivers.
8. The next remaining formal seam question after that is Mob-side rather than
   composition-side: the forward Mob -> Meerkat route now carries opaque
   `work_id` plus `origin`, but `MobMachine` still treats `SubmitWork` as one
   origin-insensitive self-loop even though runtime gives `origin` real
   external-vs-internal turn semantics before ingress.
9. The refreshed fast-loop review rechecked the dominant remaining Meerkat
   splitters (`silent_intent_overrides`, `pre_run_phase`, `current_run_id`,
   `active_runtime_id`, `active_fence_token`, and internal `Attached`) and did
   not find another honest high-impact collapse on the current design. The
   remaining pressure is now largely real behavior pressure rather than stale
   shadow-state pressure.

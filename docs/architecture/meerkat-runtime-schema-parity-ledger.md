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
- Meerkat now formally owns `silent_intent_overrides` as checked-in ingress
  state instead of leaving it below the machine boundary; exact audited parity
  stayed green after lifting it, and the targeted runtime/model regression for
  `SetSilentIntents` is green
- live recycle no longer detours through a handwritten control-side
  `Recovering` hop: the runtime helper now realizes recycle as the same direct
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
  recover workflow is gone too: `RecoverRequested` / `RecoverySucceeded` were
  removed from `RuntimeControlAuthority`, the runtime tests now drive the real
  `recover()` path instead of a helper-only mini-machine, and `Recovering`
  remains only as a compatibility-facing public `RuntimeState`
- the remaining lower-authority control question is now narrower still:
  the helper still owns a compatibility `ResumeRequested` escape hatch for an
  explicitly restored `Recovering` state plus duplicated run-return
  bookkeeping, but the live recover/recycle flow no longer depends on a
  helper-side `Recovering` workflow
- that duplicated run-return bookkeeping is narrower than before too: the
  helper no longer stores a full `RuntimeState` as `pre_run_state`, only the
  three return targets the checked-in machine already models
  (`idle` / `attached` / `retired`)
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
  and the truthful Meerkat readout now sits at `11,858` reachable states with
  raw/phase/full quotients `385 / 390 / 11,469`
- the pure query surface remains runtime-audited helper behavior, but it is no
  longer counted as formal transition coverage

Interpretation:

- the Meerkat schema is no longer missing obvious acceptance guards on the
  current public-phase frontier
- the top-level modeled formal-state vector is green on the audited frontier
- exact observable parity is also green once the composed audit includes the
  lower-level ledger carrier that actually owns report counts
- the concrete answer to “is the machine under-modeled?” is now sharper:
  the remaining normalization work is about authority boundaries and field
  factoring, not about missing acceptance guards on the audited public surface

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
3. Use the trustworthy post-parity Hopcroft rerun as the Meerkat
   simplification baseline after the visibility-boundary,
   LLM/capability-boundary, dead active-work, `current_run_id`, and attached
   steered/interrupt-bearing accept cuts:
   raw `11,858 -> 385`, phase `11,858 -> 390`, full `11,858 -> 11,469`,
   TLC `1,068,719 generated / 11,858 distinct / depth 9`.
4. Read that baseline together with the largest-block field projection from
   [`docs/architecture/machine-simplification-proposal.md`](machine-simplification-proposal.md):
   the dominant Meerkat mixed block is now measured as `4,711` states over
   `2,169` extended-state tuples, with `1,644` tuples reused across multiple
   phases.
5. Read that baseline together with the now-green Mob lifecycle-triangle
   ledger in
   [`docs/architecture/mob-runtime-schema-parity-ledger.md`](mob-runtime-schema-parity-ledger.md).
6. Use the next loop to target payload-sensitive Meerkat admission semantics
   beyond the now-modeled attached-steer and running-interrupt cases,
   especially the question of which live ingress/post-admission carriers
   belong in the two-machine model versus which should stay below it as
   lower-authority mechanics.

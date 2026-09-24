# Mob Runtime/Schema Parity Ledger

## Purpose

This ledger is the checked-in burn-down surface for Mob runtime/schema parity.
It is derived from the ignored in-crate audit:

```bash
cargo test -p meerkat-mob audit_mob_runtime_phase_parity_map \
  -- --ignored --nocapture
```

and the stricter modeled-state follow-up:

```bash
cargo test -p meerkat-mob audit_mob_runtime_modeled_state_parity_map \
  -- --ignored --nocapture
```

The goal is to keep the Mob simplification loop honest:

- close runtime/schema gaps before using Hopcroft results as simplification
  evidence
- distinguish real schema/runtime disagreement from bad probe setup
- evaluate guard-sensitive rows against representative pre-states instead of
  treating every phase-local transition as always enabled

The live JSON report is written to the system temp directory as
`mob-runtime-phase-parity.json`.

## Current Snapshot (2026-04-15)

- Audited lifecycle pairs: `Running ↔ Stopped`, `Completed ↔ Running`,
  `Completed ↔ Stopped`
- Transition-bearing rows in scope: `62`
- Live-probed rows: `62`
- `fixed`: `62`
- `align_schema`: `0`
- `align_runtime`: `0`
- `needs_decision`: `0`
- Modeled-state rows in scope: `78`
- Modeled-state rows probed: `78`
- Modeled-state rows aligned: `78`

Current state:

- the Mob lifecycle triangle is fully probed and green on the current
  representative pre-state frontier
- the audit now evaluates guard-sensitive schema rows against the representative
  runtime pre-snapshots where it has enough field information to do so
- the transition-bearing parity surface is exact for the current triangle:
  `62 / 62 / 62 / 0 / 0`
- the runtime-backed modeled-state surface is also exact for the current
  triangle: `78 / 78 / 78 / 0`
- the exact full pair audit is also exact for the current transition-bearing
  triangle: `62 / 62 / 62 / 0 / 0`
- the formal machine no longer carries the pure shadow fields
  `inflight_work_id`, `task_count`, or `event_subscription_count`
- the formal machine also no longer carries the seam-shadow
  `current_generation` field; `RequestRuntimeBinding` now emits generation
  directly from the transition bindings instead of replaying a top-level
  stored copy
- `CancelWork` is now a surfaced-only input rather than a formal top-level
  transition, because the runtime behavior hangs on concrete `WorkRef` carrier
  lineage rather than on the lifecycle phase alone
- the formal machine still does not carry the old representative runtime-
  identity shadow trio `active_identity`, `active_runtime_id`, or
  `active_fence_token`, but it now does carry the real stale-binding truth as
  `live_runtime_ids` plus `runtime_fence_tokens`
- stale-fence-sensitive commands now validate against that formal binding
  table, so the checked-in machine owns the same current-binding truth the
  public handle uses to reject stale work
- the formal machine no longer carries the dead `retiring_member_count`
  counter; the retire path now relies directly on `active_member_count`, and
  the truthful Hopcroft/TLC readout stayed flat because the removed counter was
  never carrying independent behavior
- `StopRunning` now carries the same `no_active_runs` gate as the live actor
  path: a running Mob with active flows rejects `stop()` until those flows are
  canceled or settle, and the focused runtime/schema proof for that row is
  now checked in
- the formal machine now starts `Running + coordinator_bound=true`, matching
  the live runtime bootstrap path and the initial `debug_orchestrator_snapshot`
  seen from a freshly created Mob
- the parity harness now evaluates all remaining formal Mob core fields
  directly from representative runtime snapshots, including
  `wiring_edge_count`, so guard-sensitive rows like `Unwire` are no longer
  partially classified on missing-field tolerance
- the full pair audit now classifies the modeled kernel outcome plus coarse
  result summaries across all `62` transition-bearing rows of the lifecycle
  triangle, and that exact observable pass stays green too
- the formal machine no longer carries `cleanup_pending`; that flag remains
  lower-authority lifecycle bookkeeping inside `MobLifecycleAuthority`, and
  removing it from top-level `MobMachine` left the truthful Hopcroft/TLC
  baseline flat at raw/phase/full `207 / 209 / 2238`
- the formal machine also no longer carries `wiring_edge_count`; the live
  runtime only uses concrete roster adjacency and idempotent wire/unwire plans,
  not exact edge-count semantics, so removing the top-level count collapsed the
  truthful Mob baseline from raw/phase/full `207 / 209 / 2238` to
  `132 / 134 / 1181`
- the formal machine also no longer carries `active_member_count`; the live
  runtime already derives roster size from the roster itself, while top-level
  machine legality only needed member presence, which is already captured by
  `live_runtime_ids != {}`. Removing the counter collapsed the truthful Mob
  baseline again from raw/phase/full `132 / 134 / 1181` to `102 / 104 / 861`
- the next fast-loop reviewer pass rejected collapsing the remaining dominant
  Mob drivers `coordinator_bound`, `active_run_count`, `pending_spawn_count`,
  `live_runtime_ids`, `runtime_fence_tokens`, and
  `externally_addressable_runtime_ids`; on the current branch tip those fields
  are carrying real checked-in lifecycle, orchestration, or stale-binding
  legality rather than overlapping top-level summaries
- the generated `meerkat_mob_seam` composition now rejects inadmissible queued
  external entry packets instead of deadlocking after terminal Mob shutdown
- external-turn legality now honors the roster's
  `effective_profile_override` instead of always re-resolving by role name,
  so externally addressable override profiles are no longer silently ignored
- the checked-in Mob/Meerkat seam no longer claims a `WorkCompleted` /
  `WorkFailed` / `WorkCancelled` return lane. The live runtime still treats
  `SubmitWork` as a thin ingress request plus opaque `WorkRef` receipt, so the
  formal model now stops at `RequestRuntimeIngress` instead of overclaiming an
  unwired work-terminal protocol
- the production Mob actor now routes orchestrator legality and realization
  through actor-level `machine_*` wrappers instead of treating lower
  `MobOrchestratorAuthority` verbs as semantic entry points directly
- the production orchestrator surface also no longer treats helper-owned
  `active_flow_count` as canonical truth: snapshot/legality/application now
  take the machine-owned `active_run_count` explicitly, so flow-count gating
  is driven by the checked-in machine surface rather than by helper-local
  state

## Resolution Rubric

| Label | Meaning |
| --- | --- |
| `fixed` | The live runtime probe agrees with the current schema classification for this pair/input row. |
| `align_schema` | Runtime behavior and the current architecture notes already point strongly enough one way that the schema should be reshaped to match. There are no current Mob rows in this bucket. |
| `align_runtime` | The schema is carrying the intended contract and the runtime should be tightened to match it. There are no current Mob rows in this bucket. |
| `needs_decision` | We do not yet have enough evidence to align either side safely. There are no current Mob rows in this bucket. |

## What Changed

- The audit fixture now builds phase-appropriate representative states for the
  full `Running` / `Stopped` / `Completed` triangle instead of only the original
  high-signal mismatch rows.
- `TaskUpdate` and `CancelFlow` were closed as probe-shape issues:
  - `TaskUpdate` needed a real owner-bearing fixture
  - `CancelFlow` needed a synthetic run id on non-`Running` sides so phase
    rejection could be observed without trying to stop an active run first
- `SubscribeAgentEvents` was the last real schema gap in the triangle. The
  runtime requires a live member, so the schema now models that with
  `active_members_present`, and the audit evaluates that guard against the
  representative pre-state.
- The next simplification tranche removed three pure shadow fields from the
  formal machine:
  - `inflight_work_id`
  - `task_count`
  - `event_subscription_count`
- None of those fields participated in guards or emitted semantics, and the
  exact lifecycle-triangle parity audit stayed green after the cut.
- The following tranche removed `current_generation` from the formal state and
  switched `RequestRuntimeBinding` generation emission to come directly from the
  transition bindings.
- That cut also stayed green on the exact lifecycle triangle, and the truthful
  raw/phase/full Hopcroft readout remained unchanged at the time
  (`1,390 -> 202`, `1,390 -> 204`, `1,390 -> 1,390`), which means
  `current_generation` was fully correlated with the remaining state rather
  than carrying independent labeled behavior.
- The next tranche moved `CancelWork` out of the formal transition graph and
  into `surface_only_inputs`, then added a composition-side rejection step for
  queued external entry packets that are no longer admissible in the current
  machine state.
- That pair of fixes kept the exact lifecycle triangle green while also
  removing the last `meerkat_mob_seam` deadlock caused by impossible external
  calls against a destroyed Mob.
- The current tranche removed the remaining representative runtime-identity
  shadow from the top-level formal state:
  - `active_identity`
  - `active_runtime_id`
  - `active_fence_token`
- That cut also stayed green on the exact lifecycle triangle, and the truthful
  raw/phase/full Hopcroft readout tightened from `1,214 -> 187 -> 189 -> 1,214`
  to `770 -> 138 -> 140 -> 770`, which means the removed fields were still
  contributing artificial labeled distinctions rather than core lifecycle
  behavior.
- The next tranche removed the dead `retiring_member_count` counter from the
  top-level formal state and simplified the retire guards/updates to rely on
  `active_member_count`.
- That cut also stayed green on the exact lifecycle triangle, and the truthful
  raw/phase/full Hopcroft readout stayed exactly flat at
  `770 -> 138 -> 140 -> 770`, which means `retiring_member_count` was fully
  correlated with the remaining state and never carried independent labeled
  behavior.
- The next parity-hardening tranche tightened the public `StopRunning`
  transition with `no_active_runs` after a focused runtime probe showed that
  the actor rejects `stop()` while flows are still active.
- That fix kept the exact lifecycle triangle green and left the truthful raw /
  phase / full Hopcroft readout unchanged at `770 -> 138 -> 140 -> 770`,
  while TLC generated states fell slightly from `25,943` to `25,767`.
- The next parity-hardening tranche aligned the formal Mob init state with the
  live runtime bootstrap by switching `coordinator_bound` from `false` to
  `true` and adding a direct runtime/schema proof for the initial snapshot.
- That fix also kept the exact lifecycle triangle green, but it changed the
  truthful reachable state space from `770` to `813` because the old formal
  machine had been starting from the wrong bootstrap state. The raw / phase /
  full quotient stayed at `138 / 140 / 813`, which means this was a parity
  correction, not a new simplification.
- The current tranche restored the real stale-binding truth to the formal
  machine in a normalized shape:
  - `live_runtime_ids: Set<AgentRuntimeId>`
  - `runtime_fence_tokens: Map<AgentRuntimeId, FenceToken>`
- It also extended the formal update language with `MapRemove` so per-member
  teardown can invalidate one runtime binding without zeroing the whole map.
- The exact lifecycle triangle stayed green on all three parity layers after
  that change, and the truthful Hopcroft/TLC readout moved to
  `1323 -> 207 -> 209 -> 1323`, which means the restored binding table is real
  machine-owned behavior rather than a presentation-only shadow.
- The next tranche made `SubmitWork` origin-sensitive in the checked-in
  machine by adding `externally_addressable_runtime_ids` and splitting the
  running `SubmitWork` path into explicit external vs internal origin guards.
- That cut kept the core quotient flat at `207 / 209`, but the truthful Mob
  reachable graph rose from `1,323` to `2,238` with TLC
  `76,199 generated / 2,238 distinct / depth 7`, which is the right signature
  for lifting a real legality distinction into `MobMachine` instead of hiding
  it in the handle/actor branch.
- The latest runtime fix applies that same external-origin legality to the live
  actor path too: `handle_external_turn()` now prefers the roster entry's
  `effective_profile_override` when deciding whether a member is externally
  addressable.
- The latest simplification cut removes the unwired work-terminal seam from the
  formal graph entirely. `ObserveWorkCompleted`, `ObserveWorkFailed`, and
  `ObserveWorkCancelled` are gone from `MobMachine`, the return-leg routes are
  gone from `meerkat_mob_seam`, and the checked-in model now matches the live
  runtime's current "submit only" work semantics.

## Pair Ledger

- `Running ↔ Stopped`: `26` rows, `26` probed, `26` fixed, `0` mismatches,
  `0` unprobed
- `Completed ↔ Running`: `25` rows, `25` probed, `25` fixed, `0` mismatches,
  `0` unprobed
- `Completed ↔ Stopped`: `11` rows, `11` probed, `11` fixed, `0` mismatches,
  `0` unprobed

## Readout

- The lifecycle triangle is now exact on the audited transition surface.
- The lifecycle triangle is also exact on the runtime-backed modeled-state
  surface (`78 / 78 / 78 / 0`).
- The broad quotient result still stands after the parity cleanup, and the
  shadow-field cuts plus stale-binding/origin restoration made that read much
  cleaner: the truthful state space has now collapsed from `4,797` to `2,238`.
- The trustworthy raw/phase/full reread is now:
  raw `2238 -> 207`, phase `2238 -> 209`, full `2238 -> 2238`.
- The latest public-guard parity hardening did not change the remaining Mob
  core fields; it only removed one over-admitted `Stop` row from the formal
  graph, which is why the quotient stayed flat while TLC generated states
  dipped slightly.
- The latest binding-table restoration is intentionally different from the old
  representative identity trio: it gives `MobMachine` enough formal state to
  model stale-fence rejection exactly, without pretending the mob has a single
  current runtime/fence owner.
- The latest init-state parity hardening corrected the bootstrap truth without
  changing the remaining intrinsic quotient, which is why the reachable space
  rose while the raw quotient stayed flat.
- The dominant mixed block (`372` states) is now split primarily by
  `runtime_fence_tokens`, `pending_spawn_count`, `active_run_count`,
  `externally_addressable_runtime_ids`, `coordinator_bound`, and
  `live_runtime_ids`.
- The latest fast-loop tranche did not remove another remaining core field.
  Instead, it moved top-level Mob lifecycle legality for `Stop`, `Resume`,
  `Complete`, `Destroy`, `Reset`, and `Shutdown` into the checked-in machine
  boundary. `MobActor` now rejects those verbs using MobMachine-aligned
  phase/run predicates first, while the lifecycle and orchestrator authorities
  are left as realization checks after the machine-owned gate passes.
- The next tranche fixed a real schema/runtime gap on the coordinator path
  rather than shaving another field: `Spawn`, `Respawn`, and `RunFlow` now
  require `coordinator_bound=true` in the checked-in machine, matching the live
  runtime's existing `StageSpawn` / `StartFlow` rejections when the
  orchestrator is unbound. The actor shell now uses the same coordinator-bound
  gate directly instead of letting the lower orchestrator authority answer that
  validity question first.
- The next ownership cut is runtime-only but still important: the lower
  `MobOrchestratorAuthority` no longer writes the actor's shared phase
  observable at all. The top-level mob lifecycle projection remains actor /
  lifecycle-owned, and the orchestrator helper is reduced to field + effect
  realization instead of acting like a second hidden phase publisher.
- The follow-up ownership cut makes `MobActor` itself read coarse phase from
  that shared top-level state byte instead of asking `MobLifecycleAuthority`
  for the answer. That aligns the actor with the public handle surface and
  narrows lifecycle authority to transition realization rather than being the
  source of truth for top-level phase reads.
- The next follow-up cuts actor-side orchestrator legality and snapshot reads
  over to the canonical top-level mob phase as well: `MobActor` now calls
  `snapshot_in_phase(...)`, `can_accept_in_phase(...)`, and `apply_in_phase(...)`
  with `self.state()`, so the helper no longer gets to answer top-level legality
  from its own private phase copy during stop/resume/reset/destroy/flow paths.
  The helper still stored phase internally at that point, but that state was no
  longer consulted by the actor for top-level legality or diagnostics.
- The next ownership cut finishes that job on the production path: stored
  orchestrator phase is now test-only scaffolding inside
  `MobOrchestratorAuthority`, while production builder/actor code uses only the
  explicit `*_in_phase(...)` surface. This means live runtime orchestration no
  longer has any hidden second phase owner below the checked-in `MobMachine`.
- The next ownership cut applies the same pattern to `MobLifecycleAuthority`:
  actor startup, stop/resume/reset, run admission, and run completion now use
  `apply_in_phase(...)` / `can_accept_in_phase(...)`, and actor-local
  `require_state(...)` checks read the canonical shared `MobState` directly.
  Stored lifecycle phase now remains only as test scaffolding, while production
  lifecycle authority is reduced to field updates plus shared-state publication
  parameterized by the actor's canonical phase.
- The next cleanup-focused cut removes the last production use of the lifecycle
  helper's `cleanup_pending` mini-state: reset no longer drives
  `BeginCleanup` / `FinishCleanup`, so `cleanup_pending` and `RequestCleanup`
  are now effectively lower-authority test scaffolding rather than live runtime
  behavior. The top-level checked-in `MobMachine` had already dropped
  `cleanup_pending`, and production runtime behavior no longer pays rent for it
  below that boundary either.
- The next run-tracking cut does the same thing for `active_run_count`:
  production lifecycle legality now receives active-run count explicitly from
  the actor's canonical flow tracker set (`run_cancel_tokens` / `run_tasks`)
  through `apply_in_phase(..., active_run_count, ...)` and
  `can_accept_in_phase(..., active_run_count, ...)`. The helper no longer owns
  live production run-count truth; it just applies the machine-aligned legality
  table against actor-supplied state.

## Notes

- The audit loop also smoke-probes the surfaced query/inspection inputs while it
  walks the pair matrix, but the `62` counted rows above are the transition-
  bearing rows used for exact schema/runtime classification.
- This ledger is intentionally narrower than the Meerkat one: it is focused on
  the lifecycle triangle that dominates the current Mob mixed-phase quotient
  block.

## Next Loop

1. Treat the current Mob lifecycle triangle as parity-closed on its audited
   frontier: transition parity, modeled-state parity, and exact full-pair
   parity are all green.
2. Re-check whether any of the remaining five Mob counter/coordinator fields are
   still representational shadow, or whether the Mob machine is now at its real
   lifecycle/orchestration core.
3. Keep the same parity-plus-Hopcroft loop as the gate before making broader
   DSL-facing claims from the remaining mixed block.
4. The refreshed fast-loop review rechecked the dominant remaining Mob drivers
   (`pending_spawn_count`, `active_run_count`, `coordinator_bound`,
   `live_runtime_ids`, `runtime_fence_tokens`, and
   `externally_addressable_runtime_ids`). On the current design those drivers
   still read as real orchestration and stale-binding legality rather than
   leftover top-level projection state.
5. The next high-value Mob frontier is no longer helper-owned coarse phase in
   either lower authority. `MobOrchestratorAuthority` and
   `MobLifecycleAuthority` are both now parameterized by the actor's canonical
   `MobState` on the production path.
6. The next honest fast-loop candidate is whether any remaining machine-local
   legality in `MobLifecycleAuthority` still needs to exist as a separate lower
   authority at all, or whether the next largest win is to collapse more of its
   run-count / cleanup semantics directly under `MobMachine`.
7. After removing production cleanup choreography, the strongest remaining
   lifecycle-helper field was `active_run_count`.
8. After the actor-owned run-count projection cut, the remaining lifecycle
   helper state is no longer a second owner of either coarse phase or live run
   count. The next honest Mob frontier is whether the remaining
   `MobLifecycleAuthority` helper should persist as a separate realization table
   at all, or whether its residual legality can now be collapsed further under
   `MobMachine`.
9. The current stock-taking cut closes the last production call sites that were
   still using the helper-owned lifecycle mutator surface. Production actor
   paths for `MarkCompleted`, `Destroy`, and `Resume` now go through
   `apply_in_phase(..., machine_active_run_count, ...)`, which means both lower
   Mob helpers are parameterized by canonical top-level phase and run-count
   truth on the live path.
10. The remaining helper-only mutation/table APIs in `MobLifecycleAuthority`,
   `MobOrchestratorAuthority`, and `RuntimeIngressAuthority` are now explicitly
   test-only. They still exist for direct table verification, but they are no
   longer part of production semantic ownership.

# Machine Simplification: Prerequisite to the DSL

## Context

The two-kernel architecture is landed and the schema-runtime parity is verified
(48 mismatches found and fixed, TLC green). Before porting the machines to a DSL
macro, we should simplify them. The current machines were reverse-engineered from
an organic codebase that grew feature by feature across 20 separate machines, then
collapsed into two kernels. Nobody ever asked: "what's the minimal state machine
that produces these observable behaviors?"

The 1:1 alignment gives us the foundation to ask that question safely. Every
simplification can be verified by TLC — reduce, re-run, either it confirms
equivalence or gives a counterexample trace showing why the reduction is invalid.

Important precision: phase 1 itself used **TLC-gated simplification**, not a
checked-in old-vs-new refinement proof. The new `cargo xtask machine-hopcroft`
lane complements that by exporting the TLC state graph and computing a
Hopcroft-style behavioral quotient over the reachable graph.

## Phase 1 Baseline

### MeerkatMachine

- **8 phases**: Initializing, Idle, Attached, Running, Recovering, Retired, Stopped, Destroyed
- **6 dispatch command enums**: SessionCommand (17 variants), ControlCommand (9), DrainCommand (2), DrainLocalCommand (3), IngressCommand (2), LegacyRunCommand (3)
- **34 inputs**, **69 signals**, **162 transitions**, **0 surface-only inputs**
- **Extended state**: runtime/session identity, wake/process/drain flags, peer ingress flag, staged tool visibility state, committed visibility revisions
- **TLC state space (CI profile)**: 102,047 generated / 3,959 distinct / depth 9

### MobMachine

- **5 phases**: Creating, Running, Stopped, Completed, Destroyed
- **1 dispatch command enum**: MobMachineCommand (39 variants)
- **39 inputs**, **79 signals**, **210 transitions**, **0 surface-only inputs**
- **Extended state**: member counts, wiring counts, flow/frame/loop counters, kickoff flags
- **TLC state space (CI profile)**: 452,835 generated / 6,401 distinct / depth 7

## Phase 1 Progress (2026-04-14)

### Landed tranches

- Added `surface_only_inputs` to `MachineSchema`, with validation that the listed variants are real surfaced inputs and have no formal transitions.
- Fixed the Meerkat schema/runtime manifest drift by classifying `StagePersistentFilter` and `RequestDeferredTools` as inputs rather than signals.
- Accounted for the branch-head runtime command `ReconfigureSessionLlmIdentity` with explicit Meerkat input/phase coverage.
- Extracted Mob read-only queries from the formal transition graph while keeping them in the surfaced runtime command manifest via `surface_only_inputs`.
- Flattened the internal Meerkat dispatch surface into one `MeerkatMachineCommand` / `MeerkatMachineCommandResult` reducer while leaving the public runtime helpers and trait adapters unchanged.
- Pruned the Mob kickoff / runtime-run lifecycle formalism by removing `Kickoff*`, `RuntimeRun*`, and `RuntimeStopRequested`, along with the schema-only `kickoff_pending` bit and `AdmitKickoffTurn` effect.
- Pruned the Mob step-dispatch formalism by removing `RuntimeWorkAdmitted`, `DispatchStep`, `CompleteStep`, `RecordStepOutput`, `Condition*`, `FailStep`, `SkipStep`, `ProjectFrameStepStatus`, and `CancelStep`, along with the top-level `AdmitStepWork` effect.
- Pruned the absorbed Mob target / node / frame-terminal notice layer by removing `RegisterTargets`, `RecordTarget*`, `NodeExecutionReleased`, `Terminalize*`, `CompleteNode`, `RecordNodeOutput`, `FailNode`, `SkipNode`, `CancelNode`, and `UntilConditionMet`, plus their top-level effect-only projections.
- Pruned the remaining top-level Mob frame / loop mirror by removing `RegisterReadyFrame`, `RegisterPendingBodyFrame`, `FrameTerminated`, `StartRootFrame`, `StartBodyFrame`, `StartLoop`, `BodyFrameStarted`, `BodyFrame*`, `UntilConditionFailed`, and `CancelLoop`, along with `active_frame_count`, `active_loop_count`, and their orphan top-level effects/invariants.
- Collapsed the formal Mob `Creating` phase into `Running`, removing the dead top-level `Start` signal and all `Creating`-only formal transitions while leaving the public `MobState` surface unchanged.
- Removed the unreachable top-level Meerkat `Recovering` phase and its self-loops from the formal model.
- Extracted the Meerkat pure query/helper surface (`ContainsSession`,
  `SessionHasExecutor`, `SessionHasComms`, `OpsLifecycleRegistry`,
  `InputState`, `ListActiveInputs`, `RuntimeState`, `LoadBoundaryReceipt`) into
  `surface_only_inputs`, removing 40 formal self-loop transitions while keeping
  the runtime command manifest and helper behavior unchanged.
- Removed the Mob top-level `current_generation` field and switched
  `RequestRuntimeBinding` emission to consume the transition bindings directly,
  so the formal machine no longer stores a generation value purely to replay it
  into a routed seam effect.
- Moved Mob `CancelWork` into `surface_only_inputs`, because the runtime
  behavior is keyed to concrete `WorkRef` lineage rather than to a top-level
  lifecycle transition surface.
- Removed the Mob seam-shadow identity trio (`active_identity`,
  `active_runtime_id`, `active_fence_token`) and tightened the remaining
  `SubmitWork` / flow-start guards to the authoritative active-member counters
  instead of replaying representative runtime identity through top-level formal
  state.
- Restored the real Mob stale-binding truth in a normalized shape by adding
  `live_runtime_ids` plus `runtime_fence_tokens` to `MobMachine`, extending
  the formal update language with `MapRemove`, and modeling stale-fence
  invalidation directly in the checked-in machine instead of in a helper-only
  handle path or a fake single representative runtime binding.
- Removed the dead Mob `retiring_member_count` field and simplified the retire
  guard/update surface to rely on `active_member_count`, which preserved the
  truthful Hopcroft/TLC readout exactly because the counter never carried
  independent formal behavior.
- Tightened the public Mob `StopRunning` transition with `no_active_runs`
  after a focused runtime probe proved the live actor rejects `stop()` while
  flows are still active; the truthful Mob quotient stayed flat and TLC
  generated states fell slightly.
- Aligned the formal Mob init state to the live runtime bootstrap by switching
  `coordinator_bound` from `false` to `true`, then checked that against a
  fresh runtime snapshot directly; the truthful Mob quotient stayed flat while
  the reachable state count rose because the old bootstrap state had been
  wrong.
- Hardened the Mob parity harness so representative-state guard evaluation
  covered the remaining formal Mob core fields before the next reduction pass,
  then used that exact frontier to prove `wiring_edge_count` was only an
  overlapping top-level summary rather than real machine-owned behavior.
- Added a full Mob pair audit alongside the existing transition and
  modeled-state passes; the modeled kernel outcome plus coarse result
  summaries stayed exact across all `62` transition-bearing rows of the
  lifecycle triangle, so the remaining Mob normalization work is no longer
  blocked on parity instrumentation asymmetry.
- Removed the Meerkat LLM/capability projection layer
  (`current_llm_identity`, `current_capability_surface`,
  `capability_surface_status`, `capability_base_filter`,
  `inherited_base_filter`) from the formal state while keeping the live runtime
  ownership, exact runtime behavior, and routed seam effects unchanged.
- Removed the dead Meerkat wake/process pending bits from the formal state:
  `wake_pending` and `process_pending` were constant `FALSE` across the
  truthful reachable graph, did not participate in surviving formal guards,
  and kept exact parity plus the truthful TLC/Hopcroft readout unchanged after
  removal.
- Removed the dead Meerkat active-work slice from the formal state:
  `active_work_id` never became `Some(...)` in the truthful graph, and the
  old `has_active_work`-guarded top-level completion/operation signal slice
  had zero reachable edges. Removing both kept exact parity plus the truthful
  TLC/Hopcroft readout unchanged.
- Broadened the Meerkat verification `ToolFilter` domain from a singleton
  sample to a two-sample domain in both CI and deep cfg generation, which
  raised the truthful Meerkat state space from `11,814` to `38,945` reachable
  states while leaving the raw/phase quotient at `385 / 390`. That closes the
  old singleton-domain confound around the remaining filter-layer
  simplification questions.
- Removed the top-level Meerkat filter mirror pair `active_filter` /
  `staged_filter` after the stronger two-sample `ToolFilter` rerun showed the
  real filter behavior already lives below the Meerkat machine boundary in
  `MachineToolVisibilityOwner`; exact audited parity stayed green and the
  truthful Meerkat state space fell back from `38,945` to `11,814` distinct
  states while the raw/phase quotient held at `385 / 390`.
- Absorbed `current_run_id` back into the top-level Meerkat formal state,
  wiring it through `Prepare`, `Commit`, `Fail`, and the terminal/control
  clearing paths so the checked-in machine now owns active-run identity again
  instead of delegating it entirely to handwritten `RuntimeControlAuthority`
  state.
- Extended the exact Meerkat parity snapshot to include the stale handwritten
  control wake/process booleans plus the driver-owned typed
  `post_admission_signal`; the stricter full-row pair audit stayed green at
  `260 / 260`, which shows those carriers are not hiding new public-phase
  mismatches on the current 10-pair frontier.
- Modeled attached steered `AcceptWithCompletion` as a payload-sensitive
  immediate path: when `request_immediate_processing=true`, the checked-in
  Meerkat machine now transitions `Attached -> Running` and binds the fresh
  run identity instead of collapsing that runtime behavior into the queue-only
  attached self-loop.
- Modeled the running queued `InterruptYielding` accept path explicitly:
  queued `AcceptWithCompletion` while already `Running` is no longer flattened
  to a single passive self-loop, and the checked-in Meerkat machine now emits
  the same typed `PostAdmissionSignal("InterruptYielding")` branch that the
  runtime uses for running peer-message admission.
- Removed the dead hidden ingress wake/process flags
  (`wake_requested` / `process_requested`) from `RuntimeIngressAuthority` and
  the Meerkat diagnostic spine after verifying that the live runtime loop no
  longer consumes them. Exact audited parity stayed green, TLC/Hopcroft stayed
  flat, and the remaining wake/process carrier is now just emitted effects +
  `post_admission_signal`.
- Absorbed `silent_intent_overrides` into the checked-in Meerkat machine so
  `SetSilentIntents` no longer depends on lower-authority hidden ingress
  state. The targeted runtime/model regression is green, exact audited parity
  stayed flat, and a larger admitted-input ledger absorption was explicitly
  deferred because `Ingest` / `Prepare` still need a wider exact-modeling
  tranche.
- Removed the dead handwritten `RuntimeControlAuthority` admission/wake branch
  (`SubmitWork`, `AdmissionAccepted`, `AdmissionRejected`,
  `AdmissionDeduplicated`, `wake_pending`, `process_pending`) after verifying
  that the live runtime never exercised it. Exact audited parity stayed green,
  TLC/Hopcroft stayed flat, and the remaining handwritten control helper is
  now narrowed to coarse run-lifecycle / recycle bookkeeping instead of a
  shadow admission machine.
- Aligned live recycle with the checked-in Meerkat machine by collapsing the
  helper-only `Recovering` detour: ephemeral recycle now realizes the same
  direct control projection the formal model already uses
  (`Idle/Retired -> Idle`, `Attached -> Attached`), and exact audited parity
  plus TLC stayed flat after removing the extra helper-side recycle completion
  transition.
- Removed the dead handwritten recovery workflow from `RuntimeControlAuthority`
  by deleting `RecoverRequested` / `RecoverySucceeded` and retargeting the
  remaining runtime tests to the real `recover()` path.
- Removed the remaining handwritten control authority as a semantic reducer:
  `Recovering` and `Resume` are gone from the runtime surface, coarse control
  truth now lives directly with the runtime driver in the same shape the
  checked-in Meerkat machine models, and `RuntimeControlAuthority` no longer
  exists as a second lifecycle owner.
- Narrowed the remaining driver-side coarse run-return bookkeeping by removing
  the private `RunReturnPhase` shadow enum; `pre_run_phase` now uses the same
  checked-in `RuntimeState` projection (`Idle` / `Attached` / `Retired`) that
  the Meerkat machine and parity spine already use.
- Removed the coarse lifecycle verbs `retire`, `reset`, and `destroy` from the
  generic `RuntimeDriver` trait. Those lifecycle nouns are now spoken only by
  `MeerkatMachine` / `RuntimeControlPlane`, while the concrete drivers retain
  just the realization/persistence helpers needed to apply the machine-owned
  transition.
- Removed the last coarse lifecycle control verb from the generic
  `RuntimeDriver` trait as well: `stop` is now a concrete driver realization
  helper rather than a trait-level runtime lifecycle noun, while
  `MeerkatMachine` / the control-plane seam remain the only layers that speak
  the lifecycle command semantically.
- Moved runtime-loop batch start preparation out of `runtime_loop.rs` into a
  machine-module helper so the coarse `start_run + stage_batch + unwind`
  sequence is no longer realized inline by the loop body. This intentionally
  leaves run terminal return for the next separate tranche.
- Moved coarse run-start and run-return control writes out of the driver-owned
  `EphemeralRuntimeDriver` reducer path as well: the checked-in machine module
  now validates the active run binding, commits the `Running` projection, and
  clears `current_run_id` / `pre_run_phase` on terminal return, while the
  driver is reduced to ingress/ledger mechanics around `BoundaryApplied`,
  `RunCompleted`, and `RunFailed`.
- Moved coarse stop legality out of the driver too: `MeerkatMachine` now owns
  the legal source phases for stop and the `Stopped` projection, while the
  driver only performs already-decided cleanup mechanics (abandoning
  non-terminal inputs, clearing queue state, and clearing silent intents).
- Moved coarse destroy legality out of the driver as well: `MeerkatMachine`
  already owned the public/state guardrails, and the driver now only realizes
  the already-decided `Destroyed` cleanup instead of deciding whether destroy
  is legal.
- Moved coarse retire/reset legality out of the driver too: the control-plane
  already owned the legal source states, and the runtime now commits the
  `Retired` / `Idle` projections in the checked-in machine layer while the
  driver only computes reports and cleanup mechanics.
- Moved ordinary `Idle <-> Attached` projection out of direct driver lifecycle
  verbs too: `PrepareBindings`, stale loop republish, and unregister cleanup
  now use machine-owned attachment projection helpers instead of letting
  `attach()` / `detach()` act as semantic reducers below the checked-in
  machine.
- Taught the generated closed-world composition models to reject queued
  external entry packets that are no longer admissible for the current machine
  state, which removes seam deadlocks without widening the machine transition
  graphs.
- Added the missing forward Mob -> Meerkat work-ingress route to the checked-in
  `meerkat_mob_seam` composition. `SubmitWork` now emits
  `RequestRuntimeIngress`, the seam routes it into Meerkat `Ingest`, and the
  remaining `WorkRef -> InputId` translation is left explicitly below the
  machine boundary instead of being faked as a top-level identity correlation.
  The route now carries opaque `work_id` plus `origin`, so the checked-in seam
  preserves the behavior-bearing pre-admission facts without pretending to own
  minted Meerkat input identity or full content lowering.
- Moved contributor-set legality for `StageDrainSnapshot`,
  `BoundaryApplied`, and `RunCompleted` out of `RuntimeIngressAuthority` and
  into `MeerkatMachine`. The helper now applies only the already-decided
  queue/lifecycle updates for those run-batch transitions.
- Moved runtime-loop batch selection and batch-boundary classification out of
  `RuntimeIngressAuthority` too. The checked-in Meerkat machine now chooses the
  next steer/prompt batch and its `RunStart` vs `RunCheckpoint` boundary from
  stored ingress metadata, leaving the helper as queue storage plus mechanical
  lifecycle application.
- Moved failing-contributor ownership out of the driver too. `RunFailed` now
  takes the contributor set explicitly from `MeerkatMachine`, so rollback no
  longer depends on the runtime locally rediscovering "which staged inputs were
  this run".
- Moved coarse `Stopped` / `Destroyed` projection out of driver cleanup
  helpers. The checked-in machine now commits those phase changes before stop /
  destroy cleanup runs, leaving the driver responsible only for the already-
  decided cleanup and persistence mechanics.
- Removed the last duplicate coarse admission gate in
  `EphemeralRuntimeDriver::accept_input()`, so phase-gated input legality now
  lives only in the checked-in `MeerkatMachine::Ingest` path while the driver
  focuses on durability, policy, and ledger mechanics.
- Threaded the forward `Mob -> Meerkat` ingress seam fully through the
  checked-in transitions as well as the routed packet shape: `SubmitWork` now
  binds `origin`, and Meerkat `Ingest` transitions bind `runtime_id`,
  `work_id`, and `origin` instead of widening only the route schema.
- Corrected the remaining Mob external-addressability runtime drift by making
  `handle_external_turn()` honor `RosterEntry.effective_profile_override`
  rather than always re-resolving from the definition role. Override-backed
  members now follow the same externally addressable policy in runtime that the
  spawn path and formal machine already intended.
- Removed the paper-only Mob work-terminal seam from the checked-in machines and
  composition. `SubmitWork` still models `RequestRuntimeIngress`, but
  `ObserveWorkCompleted` / `ObserveWorkFailed` / `ObserveWorkCancelled` and the
  corresponding `Work*` return routes are gone until a real `WorkRef -> InputId`
  tracking protocol exists in the runtime. This keeps the two-machine model
  honest instead of publishing completion/cancellation semantics that the live
  runtime does not yet own.
- Cleared the full phase-exit workspace gates on the rebased branch tip: `./scripts/repo-cargo nextest run --workspace` finished with 4,305 passing tests / 149 skipped, and `./scripts/repo-cargo clippy --workspace -- -D warnings` finished clean after fixing branch-head regressions in tool-visibility boundary change detection, stale semantic-model expectations, and idle-session LLM hot-swap fallback handling.
- Regenerated machine/composition artifacts and re-ran schema parity, drift, TLC, render, runtime, and owner tests on top of the landed tranches.

### Current metrics delta

| Machine | Metric | Baseline | Current | Delta |
|---|---|---:|---:|---:|
| MeerkatMachine | Phases | 8 | 7 | -1 |
| MeerkatMachine | Inputs | 34 | 37 | +3 |
| MeerkatMachine | Surface-only inputs | 0 | 0 | 0 |
| MeerkatMachine | Signals | 69 | 67 | -2 |
| MeerkatMachine | Transitions | 162 | 159 | -3 |
| MeerkatMachine | TLC generated states | 102,047 | 1,814,665 | +1,712,618 |
| MeerkatMachine | TLC distinct states | 3,959 | 19,459 | +15,500 |
| MeerkatMachine | TLC depth | 9 | 9 | 0 |
| MobMachine | Phases | 5 | 4 | -1 |
| MobMachine | Inputs | 39 | 39 | 0 |
| MobMachine | Surface-only inputs | 0 | 12 | +12 |
| MobMachine | Signals | 79 | 31 | -48 |
| MobMachine | Transitions | 210 | 82 | -128 |
| MobMachine | TLC generated states | 452,835 | 26,484 | -426,351 |
| MobMachine | TLC distinct states | 6,401 | 1,323 | -5,078 |
| MobMachine | TLC depth | 7 | 7 | 0 |

### Hypothesis / verdict tracker

| Hypothesis | Verdict | Notes |
|---|---|---|
| Meerkat `StagePersistentFilter` / `RequestDeferredTools` are runtime inputs, not signals | passed / landed | Restored schema/runtime manifest parity without changing public runtime APIs. |
| Branch-head Meerkat runtime commands are fully surfaced in the schema manifest | passed / landed | Added `ReconfigureSessionLlmIdentity` after rebasing to the latest `fix/schema-runtime-audit-round3` tip. |
| Mob read-only queries should stay surfaced without formal transitions | passed / landed | Implemented via `surface_only_inputs`; owner tests and TLC stayed green. |
| Flatten Meerkat dispatch into one internal reducer | passed / landed | Unified the internal runtime reducer without changing the public helper or trait surfaces; targeted runtime and parity tests stayed green. |
| Prune Mob kickoff / run-lifecycle signals | passed / landed | Removed `Kickoff*`, `RuntimeRun*`, and `RuntimeStopRequested`, plus the schema-only kickoff bit; TLC distinct states fell from 6,401 to 5,411. |
| Prune Mob step-dispatch signals | passed / landed | Removed the top-level step-dispatch notice/persistence signals and `AdmitStepWork`; TLC generated states fell from 272,290 to 244,230 while distinct states held at 5,411. |
| Prune Mob target / node / frame-terminal notice signals | passed / landed | Removed the absorbed target/node/terminal notice layer; TLC generated states fell from 244,230 to 202,140 while distinct states held at 5,411. |
| Prune Mob frame / loop lifecycle signals | passed / landed | Removed the remaining top-level frame/loop mirror, including `active_frame_count` and `active_loop_count`; TLC generated states fell from 202,140 to 162,926 and distinct states fell from 5,411 to 4,859. |
| `Creating` vs `Running` can merge internally | passed / landed | Fresh and resumed mobs already surface `Running`; removed the dead formal `Creating` phase and `Start` signal without changing the public `MobState` enum. |
| Mob work/task/subscription shadow counters should not stay as top-level formal state | passed / landed | Removed `inflight_work_id`, `task_count`, and `event_subscription_count`; exact Mob parity stayed green and TLC distinct states fell from 4,797 to 1,390 while the raw/phase quotients stayed at 202 / 204. |
| Mob `current_generation` should not stay as top-level seam-shadow state | passed / landed | Removed `current_generation` and emitted `RequestRuntimeBinding.generation` directly from the transition bindings; exact Mob parity stayed green and the truthful Hopcroft/TLC readout stayed unchanged, proving the field was fully correlated rather than behavior-bearing. |
| Mob `CancelWork` should stay surfaced without formal transition coverage | passed / landed | Moved `CancelWork` to `surface_only_inputs`; exact Mob parity stayed green, the truthful TLC state space fell from 1,390 to 1,214, and the raw/phase quotients tightened from 202 / 204 to 187 / 189. |
| Mob representative runtime identity should not stay as top-level formal state | passed / landed | Removed `active_identity`, `active_runtime_id`, and `active_fence_token`; exact Mob parity stayed green, truthful TLC distinct states fell from 1,214 to 770, and the raw/phase quotients tightened from 187 / 189 to 138 / 140. |
| Mob stale-binding truth should stay formally owned in `MobMachine` | passed / landed | Restored `live_runtime_ids` plus `runtime_fence_tokens`, added formal `MapRemove`, and modeled stale-fence invalidation exactly where the public handle enforces it. Exact Mob parity stayed green on all three layers, and the truthful quotient moved to `1323 -> 207 / 209 / 1323`, showing that the restored binding table is real machine behavior rather than a representative shadow. |
| Mob `SubmitWork` origin can stay as handle/actor-only semantics | rejected / landed | Added `externally_addressable_runtime_ids` to `MobMachine` and split `SubmitWork` into explicit external vs internal origin guards. The truthful Mob state space rose from `1,323` to `2,238` while the quotient stayed flat at `207 / 209`, which is the expected signature for lifting a real legality distinction into the machine instead of hiding it below the boundary. |
| Mob top-level `cleanup_pending` should remain machine-owned lifecycle truth | rejected / landed | Removed `cleanup_pending` from the checked-in top-level `MobMachine` after confirming it is only lower-authority cleanup bookkeeping inside `MobLifecycleAuthority`. The truthful Mob baseline stayed flat at `2238 -> 207 / 209 / 2238`, which is the expected signature for deleting an overlapping top-level mirror rather than collapsing real behavior. |
| Mob top-level `wiring_edge_count` should remain machine-owned wiring truth | rejected / landed | Removed `wiring_edge_count` from the checked-in top-level `MobMachine` after confirming the live runtime only uses concrete roster adjacency and idempotent wire/unwire plans, not exact count semantics. The truthful Mob baseline collapsed from `2238 -> 207 / 209 / 2238` to `1181 -> 132 / 134 / 1181`, which showed the count had been an overlapping top-level summary rather than real machine-owned behavior. |
| Mob top-level `active_member_count` should remain machine-owned member truth | rejected / landed | Removed `active_member_count` from the checked-in top-level `MobMachine` after confirming it was only a manually maintained cardinality summary of `live_runtime_ids`, while the runtime already derives roster size from the roster itself. The truthful Mob baseline collapsed again from `1181 -> 132 / 134 / 1181` to `861 -> 102 / 104 / 861`, proving the counter was overlapping top-level summary state rather than real machine-owned behavior. |
| Refreshed fast-loop review should still collapse the remaining dominant Meerkat drivers | rejected / deferred | Re-ran the quotient-driven counterargument loop on the refreshed dominant Meerkat candidates (`silent_intent_overrides`, `pre_run_phase`, `current_run_id`, `active_runtime_id`, `active_fence_token`, and internal `Attached`). The review converged that the first four are still behavior-bearing machine truth, while `active_runtime_id` / `active_fence_token` only become reducible if the current binding seam is redesigned rather than simply trimmed. |
| Refreshed fast-loop review should still collapse the remaining dominant Mob drivers | rejected / deferred | Re-ran the quotient-driven counterargument loop on the refreshed dominant Mob candidates (`pending_spawn_count`, `active_run_count`, `coordinator_bound`, `live_runtime_ids`, `runtime_fence_tokens`, and `externally_addressable_runtime_ids`). The remaining Mob drivers still carry real orchestration and stale-binding legality on the current design; the only partially plausible further cut would require replacing `externally_addressable_runtime_ids` with another checked-in legality source rather than actually removing semantics. |
| Mob lifecycle verbs should be machine-owned before another field cut | passed / landed | Replaced `MobActor`'s handwritten top-level legality gate for `Stop`, `Resume`, `Complete`, `Destroy`, `Reset`, and `Shutdown` with MobMachine-aligned predicates (`Running && active_run_count == 0`, `Stopped`, `Running`, `Running|Stopped|Completed`, `Running|Stopped|Completed`, and non-`Destroyed`). The lower lifecycle/orchestrator authorities still realize the transition, but they no longer answer top-level validity first. This was an ownership correction rather than another field removal, and it clarifies that the remaining dominant Mob fields are still real while the actor shell was lagging behind the checked-in machine. |
| Mob coordinator-bound work admission should be explicit in the checked-in machine | passed / landed | Added `coordinator_bound=true` guards to the checked-in `Spawn`, `Respawn`, `RunFlow`, and absorbed `StartFlow` paths, matching the live runtime's existing `StageSpawn` / `StartFlow` rejections when the orchestrator is unbound. The actor shell now uses the same top-level coordinator-bound admission predicate before provisioning or flow admission, so this is both a parity fix and an ownership correction. |
| Lower Mob orchestrator helper should not publish top-level phase | passed / landed | Removed `MobOrchestratorAuthority`'s writes to the actor's shared phase observable and stopped passing that observable into the helper at construction time. The top-level mob lifecycle projection stays actor/lifecycle-owned, while the orchestrator helper is narrowed to field/effect realization. This is an ownership cleanup, not a formal graph change, but it removes a genuine second publisher of coarse mob phase below the checked-in machine. |
| Mob actor should read top-level phase from the shared mob state, not a helper | passed / landed | Switched `MobActor::state()` to read the shared top-level mob state byte directly instead of delegating coarse phase reads to `MobLifecycleAuthority`. This keeps the actor aligned with the public handle/read side and further narrows lifecycle authority toward transition realization rather than top-level state publication. |
| Mob actor should route orchestrator legality through the canonical top-level phase | passed / landed | Switched actor-side orchestrator snapshot, legality, and transition calls over to `snapshot_in_phase(...)`, `can_accept_in_phase(...)`, and `apply_in_phase(...)` using `self.state()`. This means stop/resume/reset/destroy/flow paths no longer consult the helper's private stored phase when answering top-level legality, even though the helper still retains an internal phase field for now. |
| Mob orchestrator helper should not store production phase at all | passed / landed | Removed production reliance on helper-owned orchestrator phase entirely: `MobOrchestratorAuthority` now keeps stored phase only for direct table tests, while runtime builder/actor code use the explicit `*_in_phase(...)` surface everywhere. That leaves the helper as field/effect realization parameterized by the actor's canonical `MobState`, instead of a second hidden phase machine below `MobMachine`. |
| Mob production should not let helper-owned `active_flow_count` decide legality | passed / landed | Production `MobOrchestratorAuthority` snapshot, legality, and transition calls now take the actor-owned `machine_active_run_count()` explicitly. That means flow-count gating is driven by the same top-level truth the checked-in `MobMachine` already models as `active_run_count`, while helper-local `active_flow_count` remains only for direct authority-table tests and compatibility snapshots. |
| Mob production should not treat lower orchestrator verbs as semantic entry points | passed / landed | Added actor-level `machine_orchestrator_snapshot(...)`, `machine_orchestrator_can_accept(...)`, and `machine_apply_orchestrator(...)` wrappers and moved the high-signal production paths (`stop`, `complete`, `destroy`, `reset`, flow admission/cleanup, and spawn bookkeeping) over to those wrappers. Production now speaks through a machine-owned helper surface, while the lower orchestrator authority remains the field/effect realization layer. |
| Mob lifecycle helper should not store production phase at all | passed / landed | Applied the same `*_in_phase(...)` pattern to `MobLifecycleAuthority`: actor startup, stop/resume/reset, run admission, and run completion now route legality through the actor's canonical `MobState`, while lifecycle snapshot reads in tests use `snapshot_in_phase(...)`. Stored lifecycle phase is now only test scaffolding, and production lifecycle authority is narrowed to field updates plus shared-state publication parameterized by the top-level mob phase. |
| Mob lifecycle cleanup mini-state should remain live production behavior | rejected / landed | Removed the last production `BeginCleanup` / `FinishCleanup` calls from the reset path after confirming `cleanup_pending` and `RequestCleanup` were only lower-authority bookkeeping with no live runtime consumers outside helper tests. This is a runtime ownership cleanup rather than a formal graph change, and it leaves `active_run_count` as the only clearly live lifecycle-helper field on the production path. |
| Mob lifecycle helper should own live production run count | rejected / landed | Routed lifecycle run-start / run-finish legality through actor-owned run tracking instead of helper-owned counters: production now passes `machine_active_run_count()` into `apply_in_phase(...)` / `can_accept_in_phase(...)`, so `MobLifecycleAuthority` no longer acts as the canonical owner of live run-count truth below `MobMachine`. This is a runtime ownership cleanup, not a formal graph change, and it leaves the helper as a legality/effect table parameterized by actor-owned state. |
| Mob `retiring_member_count` should not stay as top-level formal state | passed / landed | Removed the dead retire counter; exact Mob parity stayed green and the truthful Hopcroft/TLC readout stayed flat at `770 -> 138 / 140 / 770`, proving the counter was not carrying independent formal behavior. |
| Mob public `Stop` should reject active flows | passed / landed | Added `no_active_runs` to `StopRunning` after a focused runtime/schema probe showed `handle.stop()` rejects while flows are still active; the lifecycle-triangle parity audit stayed exact and truthful TLC generated states fell from `25,943` to `25,767` with the quotient unchanged. |
| Mob bootstrap should start with coordinator bound | passed / landed | Changed the formal init state from `Running + coordinator_bound=false` to `Running + coordinator_bound=true` to match the live runtime bootstrap snapshot; the lifecycle-triangle parity audit stayed exact and the truthful quotient held at `138 / 140` while reachable states rose from `770` to `813`, proving the old bootstrap state had been under-modeled rather than behavior-bearing. |
| Meerkat `Recovering` is a transient / no-op top-level phase | passed / landed | Removed the unreachable top-level `Recovering` phase from the formal model, then removed `Recovering` from the runtime and wire surfaces as well. The live recover/recycle flow no longer depends on any handwritten helper recovery workflow. |
| Driver-local run-start / run-return legality can stay below `MeerkatMachine` | rejected / landed | Moved active-run validation plus `current_run_id` / `pre_run_phase` writes/clears out of `EphemeralRuntimeDriver` and into the checked-in machine module. The driver now realizes ingress/ledger mechanics for `BoundaryApplied`, `RunCompleted`, and `RunFailed`, but it no longer decides whether a run may start or where it legally returns. |
| Driver-local stop legality can stay below `MeerkatMachine` | rejected / landed | Moved stop legality into `MeerkatMachine`: the machine now owns the legal source phases and the `Stopped` projection, while driver stop paths are reduced to cleanup mechanics only. This removes another coarse lifecycle decision from `EphemeralRuntimeDriver` without introducing a new machine. |
| Driver-local destroy legality can stay below `MeerkatMachine` | rejected / landed | Removed the coarse destroy legality check from `EphemeralRuntimeDriver`. The machine/control path already owns the legal source states, and the driver now only realizes the already-decided `Destroyed` cleanup mechanics. |
| Ingress contributor staging / boundary / completion / replay should remain encoded as ingress-machine transitions | rejected / landed | Removed `StageDrainSnapshot`, `BoundaryApplied`, `RunCompleted`, and `ReplayQueuedContributors` from `RuntimeIngressInput` and replaced them with direct lower-level mutator methods on `RuntimeIngressAuthority`. The checked-in `MeerkatMachine` already owns contributor legality and replay classification, so ingress now applies only the already-decided queue/ledger mutations instead of pretending to own another transition surface. |
| Recovery normalization/report semantics can stay split between persistent and ephemeral drivers | rejected / landed | Lifted per-input recovery normalization into machine-owned `machine_apply_recovered_input_normalization(...)` and made both persistent and ephemeral recovery flows call that one classifier. Store IO and ledger hydration still stay below the boundary, but the meaning of Accepted/Staged/Applied recovery outcomes is no longer duplicated across drivers. |
| Driver-local retire/reset legality can stay below `MeerkatMachine` | rejected / landed | Removed the coarse `Retired` / `Idle` projection decisions from the driver. The machine/control path now owns the legal source phases plus the resulting coarse phase projection, while the driver only computes retire reports and reset cleanup mechanics. |
| Meerkat pure queries should stay surfaced without formal transitions | passed / landed | Moved the read-only helper/query family into `surface_only_inputs`; runtime/schema audits stayed green and Meerkat TLC generated states dropped from 3,668,832 to 3,113,272 while the raw/phase quotients stayed at 385 / 390. |
| Meerkat committed visibility publication progress should not stay as top-level shadow state | passed / landed | Removed `committed_visibility_revision` from the formal state; exact audited parity stayed green and Meerkat TLC distinct states fell from 59,371 to 45,610 while the raw/phase quotients stayed at 385 / 390. |
| Meerkat visibility witness provenance should not stay as top-level shadow state | passed / landed | Removed `requested_witnesses` and `filter_witnesses` from the formal state; exact audited parity stayed green and Meerkat TLC distinct states fell from 45,610 to 15,809 while the raw/phase quotients still held at 385 / 390. |
| Meerkat LLM/capability projection should not stay as top-level shadow state | passed / landed | Removed `current_llm_identity`, `current_capability_surface`, `capability_surface_status`, `capability_base_filter`, and `inherited_base_filter` from the formal state; exact audited parity stayed green and Meerkat TLC distinct states fell from 15,809 to 11,814 while the raw/phase quotients still held at 385 / 390. |
| Meerkat top-level wake/process mirrors should not stay as formal state | passed / landed | Removed the top-level `wake_pending` and `process_pending` mirrors after the truthful graph showed they were constant `FALSE` across all reachable Meerkat states; exact audited parity stayed green and the truthful TLC/Hopcroft readout stayed flat at 11,814 reachable with raw/phase/full quotients 385 / 390 / 11,425. |
| Hidden ingress wake/process flags should stay as lower-authority state | rejected / landed | Removed the write-only `wake_requested` / `process_requested` flags from `RuntimeIngressAuthority` and the Meerkat diagnostic spine after verifying that the live runtime loop no longer consumes them. Exact audited parity stayed green, TLC/Hopcroft stayed flat, and the only remaining live wake/process admission carrier is emitted effects plus typed `post_admission_signal`. |
| Helper-owned silent intent overrides should remain below the checked-in machine | rejected / landed | Removed the remaining `silent_intent_overrides` mirror from `RuntimeIngressAuthority`. The canonical set now lives on the driver/machine side, the Meerkat diagnostic/formal projection reads that single owner directly, and reset / stop / destroy clear the real runtime copy instead of only clearing a helper shadow. All three Meerkat parity audits stayed green. |
| `silent_intent_overrides` should be collapsed out of the checked-in Meerkat machine | rejected / deferred | Hopcroft attributes strong collapse pressure to the field, but the live runtime still applies it before ingress transitions to force wake/apply policy for matching peer intents. That makes it an under-modeled behavior carrier today rather than a safe simplification target. |
| `drain_running` should remain checked-in Meerkat state | rejected / landed | Removed `drain_running` from the checked-in Meerkat state after confirming it is only a top-level projection of lower-authority drain lifecycle. TLC stayed green, the raw quotient stayed flat at `459`, and the truthful Meerkat graph fell from `19,459` to `14,536` reachable states. |
| `active_requested_deferred_names` should remain checked-in Meerkat state | rejected / landed | Removed `active_requested_deferred_names` from the checked-in Meerkat state while leaving the canonical `SessionToolVisibilityState` and visibility owner untouched below the machine boundary. TLC stayed green, the raw quotient stayed flat at `459`, and the truthful Meerkat graph fell again from `14,536` to `11,293` reachable states. |
| `peer_ingress_configured` should remain checked-in Meerkat state | rejected / landed | Removed `peer_ingress_configured` from the checked-in Meerkat state after confirming it is only a top-level projection of lower-authority ingress/drain slot truth. TLC stayed green, the raw quotient dropped from `459` to `340`, and the truthful Meerkat graph fell again from `11,293` to `7,255` reachable states. |
| `staged_requested_deferred_names` should remain checked-in Meerkat state | rejected / landed | Removed `staged_requested_deferred_names` from the checked-in Meerkat state while keeping the canonical staged deferred-name set in the lower-authority visibility owner. The machine now enforces active-vs-staged deferred-name constraints directly on `PublishCommittedVisibleSet` input bindings. TLC stayed green, the raw quotient stayed flat at `340`, and the truthful Meerkat graph fell again from `7,255` to `3,835` reachable states. |
| `active_visibility_revision` should remain checked-in Meerkat state | rejected / landed | Removed `active_visibility_revision` from the checked-in Meerkat state while collapsing visibility publication to a single staged revision token plus publish-time input legality. The canonical active revision remains in the lower-authority visibility owner. TLC stayed green, the raw quotient dropped from `340` to `260`, and the truthful Meerkat graph fell again from `3,835` to `1,773` reachable states. |
| `staged_visibility_revision` should remain checked-in Meerkat state | rejected / landed | Removed `staged_visibility_revision` from the checked-in Meerkat state as well. The machine now treats visibility publication as pure publish-time legality over owner-provided revision inputs instead of storing any top-level visibility revision mirrors. TLC stayed green, the raw quotient dropped from `260` to `196`, and the truthful Meerkat graph fell again from `1,773` to `742` reachable states. |
| `attachment_live` should remain checked-in Meerkat state | rejected / landed | Removed `attachment_live` from the checked-in Meerkat state and collapsed interrupt/cancel/stop legality onto phase rather than on a second top-level "live" bit. TLC stayed green, the raw quotient dropped from `196` to `133`, and the truthful Meerkat graph fell again from `742` to `474` reachable states. |
| `active_generation` should remain checked-in Meerkat state | rejected / landed | Removed `active_generation` from the checked-in Meerkat state and from the coarse runtime-identity effect payload mirror after confirming the live runtime never used it for legality and the seam never consumed it on the return leg. TLC/Hopcroft stayed exactly flat at `474` reachable with raw / phase / full `133 / 138 / 455`, proving it was fully correlated binding-shadow state. |
| Helper-owned admission wake/process signaling should stay below the checked-in machine | rejected / landed | Removed admission-side `WakeRuntime` / `InterruptYielding` / `RequestImmediateProcessing` ownership from `RuntimeIngressAuthority`, then lifted the final admission signal classifier into the machine-owned accept surface itself: the checked-in runtime now derives the post-admission signal from the resolved `AcceptOutcome` policy plus the steer/immediacy bit, and the driver helper no longer caches admission-time signal truth locally. All three Meerkat parity audits, the direct driver regression lanes, full runtime tests, and clippy stayed green. |
| Handwritten runtime-control admission/wake state should stay alive below the two-machine model | rejected / landed | Removed the dead handwritten `RuntimeControlAuthority` admission/wake branch (`SubmitWork`, `AdmissionAccepted`, `AdmissionRejected`, `AdmissionDeduplicated`, `wake_pending`, `process_pending`) after proving the live runtime never exercised it. Exact Meerkat parity stayed green, TLC/Hopcroft stayed flat, and the remaining handwritten helper is now narrowed to coarse run-lifecycle / recycle bookkeeping instead of a shadow admission machine. |
| Live recycle should keep the helper-only `Recovering` detour | rejected / landed | Collapsed the live helper recycle path to the same direct control projection the checked-in Meerkat machine already models (`Idle/Retired -> Idle`, `Attached -> Attached`). Exact Meerkat parity stayed green, TLC/Hopcroft stayed flat, and the extra helper-side recycle completion transition was removed without changing the truthful state-space readout. |
| Meerkat active-work slice should not stay as top-level formal state | passed / landed | Removed `active_work_id` plus the unreachable top-level `RunCompleted` / `RunFailed` / `RunCancelled` and old `has_active_work`-guarded operation-completion signal slice; exact audited parity stayed green and the truthful TLC/Hopcroft readout stayed flat at 11,814 reachable with raw/phase/full quotients 385 / 390 / 11,425. |
| Meerkat `ToolFilter` verification domain should not stay singleton | passed / landed | Broadened CI/deep cfg generation from `ToolFilterValues = {"All"}` to `{"All", "toolfilter_2"}`; the truthful Meerkat state space rose from 11,814 to 38,945 distinct states while the raw/phase quotient stayed at 385 / 390, proving the old filter simplification signal had been under-constrained rather than behavior-free. |
| Meerkat top-level filter mirrors should not stay as formal state | passed / landed | Removed top-level `active_filter` / `staged_filter` after the stronger two-sample `ToolFilter` rerun showed the authoritative filter state already lives in `MachineToolVisibilityOwner`; exact audited parity stayed green and the truthful Meerkat state space fell back from 38,945 to 11,814 distinct states while the raw/phase quotient stayed at 385 / 390. |
| Behavior-bearing runtime-control state should stay outside MeerkatMachine | rejected / landed | `current_run_id` is absorbed back into the checked-in Meerkat machine, the stale handwritten control booleans are gone, the dead handwritten recover workflow is gone, `Recovering` / `Resume` are gone from the runtime surface, and `RuntimeControlAuthority` has been deleted as a semantic reducer. Rerunning Hopcroft/TLC after that absorption changed the truthful Meerkat graph from `11,858` to `17,384` reachable states while leaving the raw/phase quotient flat at `385 / 390`, which is the right signature for lifting real control truth into the machine without changing its core behavioral quotient. |
| Helper-owned ingress phase can stay below the two-machine model | rejected / landed | Removed the handwritten `Active` / `Retired` / `Destroyed` phase machine from `RuntimeIngressAuthority`. Meerkat lifecycle now owns ingress legality, while the helper remains only a queue/ledger authority. Exact Meerkat parity stayed green at `243 / 243`, `260 / 260`, and `145 / 145`, so this cut removed a second coarse lifecycle owner without reopening the audited frontier. |
| Duplicate ingress admission membership should stay as a second helper-owned set | rejected / landed | Removed the helper-owned `admitted_inputs` set from `RuntimeIngressAuthority` and derived tracked-input membership directly from the canonical per-input lifecycle map. Runtime tests, clippy, and all three Meerkat parity audits stayed green, so this was a real shadow-ledger cut rather than a semantics change. |
| Ingress can keep deriving its own active run identity from contributor state | rejected / landed | Removed `RuntimeIngressAuthority::current_run()` as a second semantic owner of active run identity. The helper now validates contributor `last_run` bookkeeping directly against the control-owned run ID, the driver gate uses that contributor check instead of ingress-derived run identity, and the Meerkat diagnostic spine now echoes `current_run_id` from control rather than ingress. All three Meerkat parity audits, full runtime tests, TLC, drift, and clippy stayed green. |
| Ingress can keep owning contributor sets and per-input run/boundary shadow metadata | rejected / landed | Removed helper-owned `current_run_contributors`, helper-side `last_run`, and helper-side `last_boundary_sequence` from `RuntimeIngressAuthority`. Run-event calls now carry explicit contributor IDs, the driver validates run identity through the machine-owned control state, the diagnostic spine derives contributor membership and run/boundary metadata from the canonical input lifecycle ledger, and abandon paths reconcile terminal ledger truth back into ingress queues so retire/reset/stop/destroy no longer leave ghost queued entries behind. Exact Meerkat parity stayed green at `243 / 243`, `260 / 260`, and `145 / 145`; full runtime tests, drift, TLC, and clippy stayed green too. |
| Active run identity can stay outside the checked-in Meerkat machine | rejected / landed | Absorbed `current_run_id` into the top-level Meerkat state and wired the same prepare/commit/fail/control clears as the runtime. Exact audited parity stayed green, TLC moved to `1,068,719 generated / 11,858 distinct / depth 9`, and Hopcroft stayed at raw/phase/full `385 / 390 / 11,469`, which is the expected signature for lifting a real control truth into the model without changing the core quotient. |
| Run terminal return can stay as a driver-local helper decision | rejected / landed | Added the missing `Running -> Retired` terminal return to the checked-in Meerkat machine and replaced the driver-local run-return classifier with a machine-owned `pre_run_phase -> next_phase` helper. Exact Meerkat parity stayed green at `243 / 243`, `260 / 260`, and `145 / 145`; TLC stayed at `1,616,227 generated / 17,384 distinct / depth 9`; and Hopcroft stayed flat at raw/phase/full `385 / 390 / 16,995`, which is the right signature for moving return semantics upstairs without changing the core quotient. |
| Retire-drain run start can stay as an unmodeled helper-only path | rejected / landed | The live runtime can drain preserved queued work by re-entering `Running` from `Retired`, so the checked-in machine now models that behavior explicitly as an internal `DrainQueuedRunRetired` signal instead of widening the public `Prepare` surface. Exact Meerkat parity stayed green at `243 / 243`, `260 / 260`, and `145 / 145`; TLC moved to `1,827,886 generated / 19,467 distinct / depth 9`; and Hopcroft rose to raw/phase `466 / 471`, which is the right signature for exposing a previously omitted lifecycle behavior rather than papering it over. |
| Ordinary attachment projection can stay as a direct driver verb | rejected / landed | Replaced `PrepareBindings`, stale-loop republish, and unregister cleanup uses of `attach()` / `detach()` with machine-owned `Idle <-> Attached` projection helpers. This keeps attachment truth in the checked-in Meerkat machine even when the runtime is only repairing a published loop rather than taking a second lifecycle transition. |
| The formal seam can omit forward Mob -> Meerkat ingress requests | rejected / landed | Added `RequestRuntimeIngress` to `MobMachine`, routed it through `meerkat_mob_seam` into Meerkat `Ingest`, and regenerated the composition artifacts. The truthful Mob Hopcroft baseline moved to raw/phase/full `207 / 209 / 1323`, which is the expected signature for adding real routed behavior rather than a representative identity shadow. |
| Contributor-set legality can stay as a helper-owned ingress authority concern | rejected / landed | Moved the legality for `StageDrainSnapshot`, `BoundaryApplied`, and `RunCompleted` into `MeerkatMachine`. Queue-prefix and contributor-state preconditions are now validated by the checked-in machine before ingress applies the already-decided queue/lifecycle updates, so the helper no longer decides whether a concrete contributor set may legally become or complete a run. |
| Batch selection and boundary classification can stay as ingress-helper policy | rejected / landed | Moved steer/prompt batch selection and `RunStart` vs `RunCheckpoint` classification into `MeerkatMachine`. The runtime loop now asks the checked-in machine for the next batch and its boundary instead of delegating those behavior-bearing choices to `RuntimeIngressAuthority`. |
| Failing contributor ownership can stay as driver-local staged-input discovery | rejected / landed | `RunFailed` now receives the explicit contributor set from `MeerkatMachine`, so rollback no longer depends on the runtime scanning staged inputs and inferring "which inputs were this run" below the machine boundary. |
| Driver cleanup may keep writing `Stopped` / `Destroyed` coarse projection | rejected / landed | Stop and destroy cleanup helpers no longer decide the final coarse phase. `MeerkatMachine` now writes the `Stopped` / `Destroyed` projection first, and the driver only performs the already-decided cleanup/persistence mechanics. |
| Accepted-input semantic classification may remain helper-owned below `MeerkatMachine` | rejected / landed | Moved policy resolution, silent-intent override, handling-mode derivation, and admission-plan derivation into the machine-owned accept layer via `accept.rs::resolve_admission(...)`. `EphemeralRuntimeDriver::accept_input()` still performs durability/idempotency checks, supersession lookup, ingress effect application, and final ledger realization, but it no longer owns the semantic admission disposition itself. The same shared resolver now serves the direct-driver/runtime-loop path too, so the ownership fix does not leave a second shadow classifier behind. |
| Recovery normalization/report semantics may remain split across the drivers | rejected / landed | Moved recovery normalization, ingress rebuild, runtime-state realization, and `RecoveryReport` synthesis into machine-owned helpers (`machine_recover_ephemeral_driver(...)` and `machine_recover_persistent_driver(...)`). `EphemeralRuntimeDriver` and `PersistentRuntimeDriver` still perform the mechanics they uniquely own, but recovery meaning no longer lives as duplicated handwritten logic below the checked-in machine boundary. |
| Contributor stage/boundary/completion/replay may remain ingress-helper transitions in production | rejected / landed | Production no longer routes contributor staging, boundary application, run completion, or replay rollback through `RuntimeIngressAuthority` transition methods. `EphemeralRuntimeDriver` now applies the already-decided queue/lifecycle mutations directly against lower-level ingress storage plus per-input lifecycle, while the old ingress transition methods remain only as focused table-test scaffolding. That removes the last obvious production transition surface below `MeerkatMachine` for run-contributor progression. |
| Runtime loop may keep treating plain driver verbs as semantic entry points for contributor progression | rejected / landed | Production now enters contributor staging, boundary application, run completion, and failed-run replay only through explicit `machine_realize_*` helpers in the checked-in machine layer. The plain driver verbs remain available for focused contract tests, but the runtime loop no longer treats them as semantic entry points; concrete drivers only realize lower-level queue/ledger/persistence mechanics beneath the machine-owned helper surface. |
| Attached steered accept can stay collapsed into the queue-only accept surface | rejected / landed | The live runtime can synchronously jump `Attached -> Running` during `AcceptWithCompletion` when admission requests immediate processing. The Meerkat catalog now models that payload-sensitive path explicitly with a run binding, and the targeted runtime/model regression is green. |
| Running interrupt-bearing accept can stay collapsed into the passive queued accept surface | rejected / landed | The live runtime can request `InterruptYielding` during running queued `AcceptWithCompletion` even when it does not request immediate processing. The checked-in Meerkat machine now models that typed `PostAdmissionSignal("InterruptYielding")` branch explicitly, and both the targeted regression and the exact parity audits stayed green. |
| Meerkat `Stopped` vs `Retired` can merge internally | rejected | The top-level transition sets diverge in load-bearing ways: `Retired` still accepts `Reset`, `StopRuntimeExecutor`, and `Recycle`, while `Stopped` does not, and the retire path carries archival/drain semantics that phase 1 should keep explicit. |
| Meerkat `Idle` vs `Attached` can merge internally | rejected | In the current formal model, phase identity still carries load-bearing "executor attached" semantics that are not derivable from the existing Meerkat extended state. Several absorbed transitions are phase-gated without a field-level attachment witness, so phase-1 collapse would require introducing a replacement attachment bit plus a new public projection layer rather than actually simplifying the model. |

### Phase 1 exit status

- Workspace gates are green on top of `fix/schema-runtime-audit-round3`: `nextest` passed (`4305` run / `4305` passed / `149` skipped) and workspace `clippy` passed with `-D warnings`.
- The low-risk and medium-risk machine reductions are landed, generated artifacts are current, and the remaining Meerkat `Idle` vs `Attached` merge is explicitly closed as a phase-1 rejection rather than left as open debt.

## Hopcroft Stock-Taking (2026-04-14)

To get out of the hypothesis-only loop, we added an algorithmic minimization
lane:

```bash
cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine meerkat_machine --machine mob_machine --profile ci

cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine meerkat_machine --machine mob_machine \
  --profile ci --observation none --audit-map
```

The command asks TLC to `-dump dot,actionlabels` for the checked-in machine
spec, parses the reachable state graph, and runs a Hopcroft-style partition
refinement over the labeled transition system. Plain DOT already includes the
state labels we need for observation and field-projection work; the earlier
`snapshot` export mode turned out to stall after writing partial graphs on the
truthful Meerkat model. The `--audit-map` extension keeps the same TLC dump,
then layers two extra diagnostics on top:

- **field audit**: run `only:<field>` and `all_except:<field>` observation
  projections for every extended-state field
- **phase-pair audit**: for each mixed-phase quotient pair, emit the static
  schema input surface (`same` / `different` / `left-only` / `right-only`) plus
  a representative reachable witness diff

We ran three observation modes for each machine:

- `none` — pure behavior quotient; phase and fields are hidden unless they
  affect future labeled behavior
- `phase` — preserves the current `phase` label as an observation
- `full` — preserves the full snapshot except `model_step_count`

### Quotient results

| Machine | Observation | Reachable states | Quotient states | Reduction | Reading |
|---|---|---:|---:|---:|---|
| MobMachine | `none` | 861 | 102 | 88.2% | After removing `cleanup_pending`, `wiring_edge_count`, and then `active_member_count`, the truthful graph is much closer to the real Mob lifecycle/orchestration core. The remaining state is machine-owned binding/origin/run/coordinator truth rather than overlapping cleanup, wiring, or member-cardinality summaries. |
| MobMachine | `phase` | 861 | 104 | 87.9% | Preserving phase still adds only two quotient blocks; `Running` / `Stopped` / `Completed` remain mostly projection even after the stale-binding restoration, forward ingress route, origin-sensitive submit-work legality, and the summary-field collapses. |
| MobMachine | `full` | 861 | 861 | 0.0% | Once the remaining authoritative binding/origin/run/coordinator fields are preserved, every reachable Mob snapshot is still distinct. |
| MeerkatMachine | `none` | 474 | 133 | 71.9% | After collapsing the visibility stack and the top-level attachment bit, the truthful graph is smaller again and the quotient is now down to 133 blocks. The remaining largest mixed block is driven by `silent_intent_overrides`, `pre_run_phase`, run identity, and runtime binding. |
| MeerkatMachine | `phase` | 474 | 138 | 70.9% | Preserving phase still adds only five quotient blocks, so phase remains almost entirely projection even after the attachment collapse. |
| MeerkatMachine | `full` | 474 | 455 | 4.0% | Preserving the full snapshot still keeps most remaining Meerkat states distinct, but what survives is now almost entirely real machine-owned binding / run-return / interrupt policy truth. |

That rerun also clarified the remaining architectural blockers. The stale
`RuntimeControlAuthority` problem is gone, and `RuntimeIngressAuthority` no
longer owns coarse lifecycle, batch selection, boundary classification, or a
second derived active run identity. The remaining Meerkat edge is now narrower:
ingress no longer models contributor staging, boundary application, run
completion, or replay rollback as ingress-machine transitions. Those paths are
now direct lower-level mutators driven by machine-owned legality, leaving only
dedup, recovered-input seeding, and terminal reconciliation below the checked-in
machine boundary. On the Mob side, the forward route and `SubmitWork` origin
legality are now checked in; what remains below the machine is
delivery/lowering glue rather than an obvious missing machine/composition fact.

All six rows above have now been rerun after the exact runtime/schema parity
passes on the current branch tip.

### What the quotient is telling us

- The current machines are **well-defined transition systems**, but they are
  **poorly normalized phase machines**.
- Phase is almost entirely a projection layer in both kernels. The raw
  behavioral quotient and the phase-preserved quotient are nearly identical.
- The machine surface is therefore encoded primarily in the extended state
  fields and their guards, not in the phase labels themselves.
- Preserving the full snapshot eliminates all quotienting for Mob and nearly
  eliminates it for Meerkat, which means the next simplification wave should
  focus on the field structure and the guard set rather than on the phase enum
  alone.

### Mixed-phase blocks

The raw quotient exposes the highest-signal parity/simplification targets:

- **MobMachine** has one dominant mixed block of `705` states spanning
  `Running`, `Stopped`, and `Completed`.
- **MeerkatMachine** has one dominant mixed block of `4,711` states spanning
  `Initializing`, `Idle`, `Attached`, `Running`, `Retired`, and `Stopped`.

These are not automatic merge approvals. They are a **schema-side lower bound**
on the machine’s intrinsic complexity.

### Interpretation discipline

The quotient does **not** prove the runtime is only `195` / `201` states. It
proves that the **current schema** cannot behaviorally distinguish more than
those quotient blocks under the chosen observation mode.

That means:

- the quotient is a simplification roadmap
- the quotient is also a parity-audit prioritizer
- the remaining gap between the quotient and the current state space is where
  either:
  - real runtime constraints still need to be modeled, or
  - redundant schema distinctions remain

## Runtime Parity Map (2026-04-14)

The next step after the Hopcroft pair map is to ask whether the mixed-phase
blocks are showing **real simplification opportunities** or **missing
runtime/schema constraints**.

We added an ignored in-crate Meerkat audit for that:

```bash
cargo test -p meerkat-runtime audit_meerkat_runtime_phase_parity_map \
  -- --ignored --nocapture
```

The audit imports the live `meerkat-machine-schema` catalog, computes the
current schema-side row classifications for the highest-signal mixed-phase
pairs, constructs representative runtime fixtures for `Idle`, `Attached`,
`Running`, `Retired`, and `Stopped`, and then probes the **real
`execute_meerkat_machine_command()` reducer** for each non-`same_surface` row.
The report is written as `meerkat-runtime-phase-parity.json` in the system temp
directory.

The checked-in row ledger for that audit now lives in
[`docs/architecture/meerkat-runtime-schema-parity-ledger.md`](/architecture/meerkat-runtime-schema-parity-ledger).

### Current Meerkat coverage

- Audited pairs: `Attached <-> Idle`, `Attached <-> Running`,
  `Running <-> Stopped`, `Running <-> Retired`, `Idle <-> Retired`,
  `Idle <-> Stopped`, `Attached <-> Retired`, `Attached <-> Stopped`,
  `Idle <-> Running`, `Retired <-> Stopped`
- Transition-bearing schema rows in scope: `243`
- Runtime probes implemented: `243`
- Schema/runtime alignments: `243`
- Schema/runtime mismatches: `0`
- Remaining unprobed rows: `0`

The Meerkat acceptance frontier for the current public-phase matrix is fully
probed and green. The acceptance audit now classifies rows using the simulated
schema outcome from the same representative pre-state, rather than the older
static transition-signature comparison.

### Full-row Meerkat audit (2026-04-15)

Acceptance parity turned out to be necessary but not sufficient. We added a
second ignored in-crate audit that probes **all** transitioned rows for the
same 10-pair public-phase matrix and compares the runtime using a
schema-aligned field projection instead of the older trimmed snapshot:

```bash
cargo test -p meerkat-runtime audit_meerkat_runtime_phase_full_parity_map \
  -- --ignored --nocapture
```

That audit writes `meerkat-runtime-phase-parity-full.json` into the system temp
directory.

Current full-row numbers:

- transition-bearing rows in scope: `260`
- runtime probes implemented: `260`
- schema/runtime alignments: `260`
- schema/runtime mismatches: `0`
- remaining unprobed rows: `0`

That final exactness step did not come from another phase merge. It came from
making the audit boundary explicit: the pair report now composes the simulated
Meerkat machine outcome with lower-authority carrier-derived control-report
summaries for surfaces such as `DestroyReport.inputs_abandoned`.

The exact observable audit is green because it no longer pretends the top-level
phase machine alone owns report counts that are actually produced from the
input-ledger carrier.

The query extraction cut the audited transition-bearing surface further without
reopening parity gaps:

- modeled-state audit is now `145 / 145`
- the acceptance map is now `243 / 243`
- the exact full-row map is now `260 / 260`
- the removed rows are the eight pure read-only query helpers now carried as
  `surface_only_inputs`
- the formal Meerkat state no longer carries the runtime-owned
  LLM/capability projection layer (`current_llm_identity`,
  `current_capability_surface`, `capability_surface_status`,
  `capability_base_filter`, `inherited_base_filter`)

So the Meerkat story is now:

- the audited acceptance surface is green
- the modeled formal-state vector is green
- the exact observable audit is green on the audited 10-pair frontier
- the remaining normalization problem is no longer parity triage; it is
  deciding which lower-authority carrier facts should eventually be lifted into
  the DSL-facing model and which should remain composed beneath it

### What is now aligned

The Meerkat parity pass has now closed five concrete families:

- registration-helper surface
- durable tool-visibility mutation
- helper/query surface (`EnsureSessionWithExecutor`, `SetSilentIntents`,
  `ContainsSession`, `SessionHasExecutor`, `SessionHasComms`,
  `OpsLifecycleRegistry`, `InputState`, `ListActiveInputs`,
  `RuntimeState`, `LoadBoundaryReceipt`)
- reducer/control acceptance surface (`Abort*`, `Wait`, `Ingest`,
  `PublishEvent`, `Accept*`,
  `Recycle`, `Prepare`, `Commit`, `Fail`)
- LLM/capability projection shadow state

The probe also still confirms several genuinely load-bearing distinctions:

- `ReconfigureSessionLlmIdentity` remains phase-gated: `Attached` and
  `Running` accept it, while `Idle`, `Retired`, and `Stopped` reject it.
- `InterruptCurrentRun` and `CancelAfterBoundary` are now modeled as
  attached-loop control commands: `Attached` accepts them as self-loops,
  `Running` accepts them with the active-work surface, and `Idle`, `Retired`,
  and `Stopped` reject them.
- `Retire`, `Recover`, `SetPeerIngressContext`, and `NotifyDrainExited` all
  still line up with the current schema surface for the audited pairs.

### What remains open

There are no currently known Meerkat acceptance mismatches in the audited
public-phase frontier. The next open question is not acceptance parity; it is whether the
remaining large mixed-phase quotient blocks reflect real runtime structure or
other still-unaudited schema surfaces.

### Mob runtime parity map

We added the matching ignored in-crate Mob audit:

```bash
cargo test -p meerkat-mob audit_mob_runtime_phase_parity_map \
  -- --ignored --nocapture
```

The checked-in row ledger for that audit now lives in
[`docs/architecture/mob-runtime-schema-parity-ledger.md`](/architecture/mob-runtime-schema-parity-ledger).

### Current Mob coverage

- Audited lifecycle pairs: `Running <-> Stopped`, `Completed <-> Running`,
  `Completed <-> Stopped`
- Transition-bearing rows in scope: `62`
- Runtime probes implemented: `62`
- Schema/runtime alignments: `62`
- Schema/runtime mismatches: `0`
- Remaining unprobed rows: `0`
- Modeled-state rows in scope: `78`
- Modeled-state rows aligned: `78`

The Mob lifecycle triangle is now fully probed and green on the current
representative pre-state frontier.

### What the Mob pass changed

The Mob parity pass closed three concrete issues:

- `TaskUpdate` and `CancelFlow` were probe-shape issues, not real contract
  disagreement:
  - `TaskUpdate` needed an owner-bearing task fixture
  - `CancelFlow` needed a synthetic run id on the non-`Running` sides so
    phase rejection could be observed without illegally stopping an active run
- `SubscribeAgentEvents` was the real schema gap. The runtime requires a live
  member; the schema now models that with `active_members_present`, and the
  audit evaluates that guard against the representative pre-state instead of
  treating every phase-local transition as always enabled
- the full lifecycle triangle is now exact on the current transition-bearing
  frontier
- the stricter modeled-state triangle is also exact (`78 / 78 / 78 / 0`)
- the full pair audit is also exact on that same transition-bearing frontier,
  which means the modeled kernel outcome plus result summaries already match
  the runtime across the audited Mob lifecycle triangle
- `CancelWork` turned out not to belong in the formal top-level transition
  graph; it is now surfaced-only, and the generated seam rejects impossible
  queued external entry packets instead of deadlocking once Mob is terminal

### Post-parity Mob rerun

After closing the exact triangle parity pass, we reran the trustworthy Mob
Hopcroft lanes on the same plain-DOT export path used above:

```bash
cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine mob_machine --profile ci --observation none

cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine mob_machine --profile ci --observation phase

cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine mob_machine --profile ci --observation full
```

Current result:

- reachable states: `1,323`
- raw quotient states: `207`
- phase-observed quotient states: `209`
- full-observed quotient states: `1,323`
- TLC: `41,202 generated / 1,323 distinct / depth 7`
- dominant mixed block: `668` states spanning `Running`, `Stopped`, and
  `Completed`
- dominant block tuples: `236`
- tuples reused across multiple phases: `80`
- maximum phases sharing one tuple: `3`

The dominant Mob block is now being split primarily by the remaining
runtime-backed lifecycle/orchestration counters plus the restored binding
table:

- `pending_spawn_count`
- `active_run_count`
- `coordinator_bound`
- `runtime_fence_tokens`
- `live_runtime_ids`

The refreshed fast-loop review did not uncover another honest high-impact cut
on top of that core. At this point the remaining Mob split drivers look like
real orchestration / stale-binding behavior rather than leftover projection
state.

The important read is that the raw quotient has continued to move only when we
changed real machine-owned distinctions in the truthful state graph. The
reachable space is now down from `4,797` to `1,323`, while the raw quotient is
down to `207`. The most recent binding-table restoration increased the
reachable space because the old formal machine had been missing stale-fence
truth, not because the representative identity shadow was architecturally
correct. That means the remaining Mob state is now much closer to the real
lifecycle/counter-plus-binding core than to the earlier seam-shadow projection
layer.

We then removed the residual top-level `current_generation` seam-shadow field
and re-ran the same trustworthy Mob lanes. The readout stayed flat:

- reachable states: `1,390`
- raw quotient states: `202`
- phase-observed quotient states: `204`
- full-observed quotient states: `1,390`
- TLC: `45,831 generated / 1,390 distinct / depth 7`

That is a useful result in its own right: `current_generation` was not merely
low-signal, it was fully correlated with the remaining field tuple on the
current truthful state graph. Removing it simplified the formal contract
without reducing the modeled behavior.

The later `CancelWork` extraction plus composition-side external-entry
rejection pass tightened the truthful graph again:

- reachable states: `1,214`
- raw quotient states: `187`
- phase-observed quotient states: `189`
- full-observed quotient states: `1,214`
- TLC: `38,134 generated / 1,214 distinct / depth 7`

That cut is also a good architecture read. The top-level Mob machine should
own lifecycle and orchestration truth; concrete `WorkRef` cancellation remains a
carrier-level concern, and impossible external calls are now rejected at the
composition perimeter instead of being forced into the kernel state graph.

The next cut removed the remaining top-level representative runtime identity
shadow:

- `active_identity`
- `active_runtime_id`
- `active_fence_token`

and replaced the old `runtime_is_bound` guard with the remaining authoritative
active-member counter surface. That read tightened again:

- reachable states: `770`
- raw quotient states: `138`
- phase-observed quotient states: `140`
- full-observed quotient states: `770`
- TLC: `25,767 generated / 770 distinct / depth 7`

That was the strongest Mob simplification result so far. A later bootstrap
parity correction changed the truthful init state from
`Running + coordinator_bound=false` to `Running + coordinator_bound=true`. The
newer binding-table restoration then corrected the remaining stale-fence
omission without bringing back the old representative runtime-identity shadow,
so the current readout is:

- reachable states: `1,323`
- raw quotient states: `207`
- phase-observed quotient states: `209`
- full-observed quotient states: `1,323`
- TLC: `41,202 generated / 1,323 distinct / depth 7`

The key reading still did not change. The machine is no longer spending
state-space budget replaying a fake single current member/runtime identity at
the top level, but it is now honestly spending state-space budget on the real
binding table that the public stale-fence behavior depends on. The remaining
mixed block is now dominated by lifecycle, orchestration, and binding
orchestration counters rather than seam-shadow identity facts.

### Reading the current pass

The Meerkat audit has shifted from parity triage to post-parity stock-taking:

- the rows we have probed are green
- the current Meerkat acceptance surface is fully probed for the 10-pair
  public-phase matrix
- the exact observable audit is also green once control-plane report counts are
  composed with their lower-authority ledger carrier
- the pure read-only query surface is now intentionally outside formal
  transition coverage while remaining in the surfaced runtime command manifest
- the next step is to use the post-parity Hopcroft rerun to steer the DSL work

### Post-parity Meerkat rerun

After closing the reducer/control acceptance gap and fixing the broader
pair-audit methodology, we reran the trustworthy Meerkat Hopcroft lanes on the
plain-DOT export path:

```bash
cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine meerkat_machine --profile ci --observation none

cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine meerkat_machine --profile ci --observation phase

cargo run -p xtask --features machine-authority -- \
  machine-hopcroft --machine meerkat_machine --profile ci --observation full
```

Current result:

- reachable states: `19,467`
- raw quotient states: `466`
- phase-observed quotient states: `471`
- full-observed quotient states: `19,078`
- TLC: `1,827,886 generated / 19,467 distinct / depth 9`
- reachable edges: `957,838`
- dominant mixed block: `8,898` states spanning `Initializing`, `Idle`,
  `Attached`, `Running`, `Retired`, and `Stopped`
- dominant block tuples: `4,560`
- tuples reused across multiple phases: `3,096`
- maximum phases sharing one tuple: `5`

The important read is now sharper than the earlier partial dump story:

- the raw quotient is much smaller than the reachable state space
- phase preservation adds only `5` blocks (`466 -> 471`)
- phase still adds very little independent information on top of the field
  tuple (`466 -> 471`)
- removing ingress's duplicate `terminal_outcome` cache did not change the
  truthful Meerkat graph at all, which confirms that terminal-outcome truth was
  already canonical in the per-input ledger rather than the helper
- removing the dead ingress `request_immediate_processing` parameter did not
  change the truthful Meerkat graph or reopen parity, which confirms that
  immediate-processing semantics are already owned by the checked-in Meerkat
  accept / post-admission signal path rather than the queue helper
- removing ingress-owned `Recover` semantics did not change the audited
  Meerkat frontier either: recovery is now derived from canonical ledger truth
  and ingress is rebuilt mechanically from that truth instead of classifying
  recovery lifecycle itself
- removing ingress `Retire` / `Reset` / `Stop` / `Destroy` did not change the
  audited Meerkat frontier either: those coarse lifecycle transitions were
  already owned by driver + ledger abandonment semantics, so the helper surface
  was ceremonial rather than authoritative
- routing runtime-loop batch start through a checked-in Meerkat helper did not
  reopen parity either: the loop no longer realizes `start_run + stage_batch +
  unwind` inline, so run-start sequencing is owned by the machine module
  instead of by handwritten loop control flow
- routing runtime-loop terminal commit/failure sequencing through checked-in
  Meerkat helpers did not reopen parity either: the loop no longer realizes
  `BoundaryApplied -> RunCompleted` or `RunFailed` inline, so terminal run
  semantics moved another step closer to the two-machine boundary
- removing the outer ingress/prepare phase prechecks from `MeerkatMachine` did
  not reopen parity either: direct admission legality is now owned in one
  place by `EphemeralRuntimeDriver::accept_input`, while the machine boundary
  preserves only the outward `Destroyed` error normalization
- removing the generic `RuntimeDriver::on_run_event` hook did not reopen
  parity either: the checked-in machine now speaks only concrete
  `boundary_applied`, `run_completed`, and `run_failed` lifecycle helpers when
  realizing run terminals, instead of delegating through an opaque catch-all
  run-event trait surface
- preserving the full snapshot still keeps almost every remaining state
  distinct (`19,078`), which tells us the field tuple is still doing most of
  the real work even after the retire-drain correction
- the visibility-boundary and LLM/capability-boundary cuts removed a large
  amount of formal shadow state without changing the audited public-phase
  behavior
- the old active-work slice was also dead top-level state rather than hidden
  complexity: `active_work_id` never became `Some(...)`, the
  `has_active_work`-guarded run-terminal slice had zero reachable edges, and
  removing both left the truthful readout unchanged
- broadening `ToolFilter` was still necessary, because it exposed that the
  top-level `active_filter` / `staged_filter` pair was only a mirror of lower
  authority state; once those mirrors were removed, the truthful graph dropped
  back to the lower five-figure range without changing the quotient or exact
  parity surface
- absorbing `current_run_id` and modeling the live attached-steer plus
  running-interrupt acceptance branches raised the truthful graph only
  slightly while keeping the quotient flat, which is the expected signature
  for lifting real control truth into the checked-in machine instead of
  papering over it with shell state
- the next Meerkat gap is no longer "maybe filter mirrors are real"; it is the
  stronger authority-boundary and payload-sensitivity question of which live
  ingress/post-admission mechanics belong in the two-machine model

That means the remaining Meerkat complexity is carried overwhelmingly by the
field tuple, not by the phase label.

### Largest Meerkat mixed-block projection

We then extended the always-on Hopcroft summary to project the largest
mixed-phase block onto its extended-state fields. On the current truthful
Meerkat run, that yields:

- dominant mixed block: `4,711` states
- distinct extended-state tuples inside that block: `2,169`
- tuples reused across multiple phases: `1,644`
- maximum phases sharing one tuple: `5`

That is the strongest concrete evidence so far that the current phase surface
is layered on top of a smaller field-driven machine instead of acting as the
primary semantic axis. The most discriminating fields inside the block are now:

- `pre_run_phase` (`4` buckets; largest bucket `42`)
- `silent_intent_overrides` (`2` buckets; split `68 / 49`)
- `current_run_id` (`2` buckets; split `70 / 47`)
- `active_fence_token` (`2` buckets; split `78 / 39`)
- `session_id` (`2` buckets; split `80 / 37`)
- `active_runtime_id` (`2` buckets; split `87 / 30`)

The read is strikingly consistent with the earlier field-ablation pass: the
largest Meerkat block is now dominated by run-return state, runtime binding,
and the still-live silent-intent carrier rather than by visibility projection,
attachment shadow state, phase labels, or the removed ingress/deferred-name

The refreshed fast-loop review rechecked each of the dominant Meerkat
splitters above. The current conclusion is that the remaining pressure is
mostly real behavior pressure, not “forgotten shadow field” pressure:

- `silent_intent_overrides` still shapes live ingress wake/apply behavior
- `pre_run_phase` and `current_run_id` still carry exact run-return legality
- `Attached` still carries checked-in lifecycle semantics that are not yet
  reducible to simple boundness
- `active_runtime_id` / `active_fence_token` only become reducible if the
  current return-leg binding seam is redesigned rather than merely trimmed
mirrors.

A later fast-loop cut removed `active_generation` too. The truthful Meerkat
readout stayed perfectly flat at `474` reachable with raw / phase / full
`133 / 138 / 455`, which confirmed that the field was a fully correlated
binding-shadow rather than behavior-bearing machine state.

### Audit-map results

The trustworthy raw/phase/full reruns are now enough to set direction even
before the heavier field-ablation lane is re-optimized:

- Mob is clearly a field/counter-driven machine with a very thin phase overlay
- Meerkat is even more strongly field-driven than the earlier partial dump
  suggested
- the next simplification wave should be field-normalization-first and DSL-
  oriented, not phase-enum-first

The heavier `--audit-map` field-ablation lane is still valuable, but on the
truthful Meerkat graph it is now expensive enough that it should be optimized
before we rely on it as the primary stock-taking surface.

### Public-surface consequence

This result also sharpens the blast-radius story:

- the broad public API cleanup already happened in the 0.6 cutover
- the remaining public compatibility surface is mainly `RuntimeState` and
  `MobState`
- the quotient says those enums are mostly compatibility projections today
- phase simplification is therefore primarily an **internal normalization**
  problem, not a broad SDK/wire churn problem

## Findings from the 0.6 audit

### MeerkatMachine: suspected redundancies

**1. Idle vs Attached looked like a boolean pretending to be a phase.**

The original audit hypothesis was that `session_registered == true` already carried the
same information as the `Attached` phase. That does not hold on the rebased phase-1 tip.
After tracing the absorbed transitions, the formal Meerkat model still uses phase identity
itself to encode "executor attached" semantics across multiple transition families:
`Abort` / `Wait`, `EnsureDrainRunning`, `Ingest` / `PublishEvent` / `Accept*`,
`StartConversationRun*`, and the absorbed surface-boundary lifecycle inputs.

Two details made that load-bearing:
- Several of the affected transitions are gated only by the phase, not by a field-level
  attachment witness that could replace it.
- `active_runtime_id` is not a sufficient replacement witness, because some attached-path
  transitions clear the runtime binding fields while remaining formally `Attached`.

**Outcome**: phase-1 `Idle` / `Attached` collapse was rejected. To merge them safely we would
need to introduce a replacement attachment bit plus a new public projection layer, which is
not a net simplification ahead of the DSL port.

**2. Recovering may not be a runtime phase.**

The `Recover` command in the dispatch (meerkat_machine.rs:1130-1164) calls `drv.recover()`
which only touches the ingress authority — it never applies `RecoverRequested` to the
control authority. The control phase does NOT change. The schema had `to: Recovering`
which we fixed to self-loops.

This means `Recovering` as a phase is only reached through `Recycle` (which goes through
the authority). If Recycle is the only entry, and Recovering is just "we're replaying events
before re-entering a normal phase," it might be a pre-machine initialization step rather
than a phase the machine occupies during normal operation.

**Investigation**: trace all paths into and out of Recovering. If it's always
Recycle → Recovering → (replay) → Idle/Attached, consider modeling it as a transient
operation rather than a durable phase.

**3. Three terminal states: Stopped, Retired, Destroyed.**

| Phase | Semantics | Can resume? | Cleanup behavior |
|-------|-----------|-------------|-----------------|
| Stopped | Executor halted, session preserved | Yes (via resume) | No cleanup |
| Retired | Runtime deregistered, session archived | Yes (via recycle) | Drain queued inputs |
| Destroyed | Everything torn down | No | Full cleanup |

These are genuinely different — Stopped is resumable, Retired triggers archival, Destroyed
is terminal. But the question is whether the *machine* needs to distinguish them, or whether
the *shell* handles the distinction:
- The machine's job: decide which transitions are legal.
- The shell's job: execute the cleanup I/O.

If the set of legal transitions from Stopped and Retired is identical (both accept Reset,
Recycle, Destroy, and reject normal operations), they could merge into one phase with a
shell-side flag tracking whether archival happened.

**Investigation**: compare the exact transition sets from Stopped and Retired. If they
differ, the distinction is load-bearing. If they're identical, merge and move the
distinction to the shell.

**4. The 6 command enums are organizational scars.**

`SessionCommand`, `ControlCommand`, `DrainCommand`, `DrainLocalCommand`, `IngressCommand`,
`LegacyRunCommand` — these exist because there used to be 8 separate machines with separate
command surfaces. They were kept as organizational groupings when everything collapsed into
MeerkatMachine.

From the machine's perspective, they're all just inputs. The split creates artificial
indirection: 6 dispatch functions that could be one, 6 result enums that could be one.

**Fix**: flatten into a single `MeerkatMachineInput` enum. This is a refactor, not a
semantic change — the behavior is identical. It simplifies the dispatch path and makes
the machine's input surface explicit.

**5. The ops lifecycle is a semaphore wearing a state machine costume.**

"Wait for N pending operations to complete before advancing" is fundamentally a counter
reaching zero. The ops lifecycle was its own machine (`OpsLifecycleMachine`) with phases,
transitions, and effects. Now it's absorbed into MeerkatMachine as extended state and
signals, but the formalism is disproportionate to the semantics.

**Investigation**: can the barrier be expressed as a single `pending_barrier_ops: u32`
field with a guard `pending_barrier_ops == 0` on the advance transition? If yes, the
entire ops lifecycle signal alphabet collapses into increment/decrement/check.

### MobMachine: suspected redundancies

**6. Read-only queries shouldn't be transitions.**

18 commands (ListMembers, RosterSnapshot, TaskList, GetMember, etc.) are modeled as
self-loop transitions from every phase. They don't mutate state, don't emit effects,
don't check guards. They're observations, not state machine inputs.

These inflate the transition count and TLC state space for no semantic benefit. The
machine doesn't need to model reads — it needs to model mutations.

**Fix**: remove query transitions from the schema entirely. Queries bypass the machine
and read state directly (which is what the runtime already does — no `require_state`
guard, no actor command).

**7. Creating vs Running phase boundary is fuzzy.**

Many commands accept both Creating and Running (`Wire`, `ExternalTurn`, `InternalTurn`,
`TaskCreate`, `Unwire`, `ForceCancel`, `Retire`, `RetireAll`, `Respawn`). The difference
between Creating and Running is whether `Start` has been applied — similar to the
Idle/Attached pattern in MeerkatMachine.

**Investigation**: is there any command that accepts Creating but NOT Running, or vice
versa, other than `Start` itself and `Spawn` (which transitions Creating → Running)?
If not, they could merge with a `started: bool` field.

**8. Signal transitions for internal lifecycle steps.**

70+ signals model internal lifecycle steps: `KickoffStarted`, `KickoffCallbackPending`,
`RuntimeRunSubmitted`, `RuntimeRunCompleted`, `DispatchStep`, `CompleteStep`, etc. Many
of these were inter-machine effects when the mob had separate orchestrator/lifecycle/flow
machines. Now they're internal to MobMachine.

The question is: do TLC invariants actually reference these signals? If the invariants
only care about input-visible behavior (what happens when you call Spawn, Retire, RunFlow),
the internal signals are formalism that TLC doesn't need.

**Investigation**: remove signals one at a time and re-run TLC. If invariants still hold,
the signal was ceremony.

## Methodology

### Hypothesis-driven simplification

Each simplification is a hypothesis: "these two phases are equivalent" or "this field is
redundant." The verification is mechanical:

1. **State the hypothesis** — e.g., "Idle and Attached can merge"
2. **Modify the schema** — merge the phases, adjust guards
3. **Run TLC** — does the full invariant set still hold?
4. **If TLC passes** — the simplification is valid; apply it to the runtime
5. **If TLC fails** — TLC gives a counterexample trace showing exactly where the
   equivalence breaks; learn from it and refine

This is safe because TLC is exhaustive within its state space. If TLC says two phases
are equivalent, they're equivalent for all reachable states. No sampling, no heuristics.

### Formal minimization (if manual pass leaves suspected waste)

After the manual pass, formal minimization techniques can find non-obvious redundancies:

- **Hopcroft's algorithm**: O(n log n) partition refinement, gives the provably minimal
  DFA. Requires unfolding extended state into the phase space first.
- **Bisimulation quotient**: two states are bisimilar if they accept the same inputs,
  produce the same effects, and transition to bisimilar states. Computable automatically.
- **Krohn-Rhodes decomposition**: prime factorization of the state machine into groups
  (reversible) and flip-flops (resets). Reveals the inherent algebraic complexity.

For Extended FSMs (which is what we have — phase + boolean/counter fields), the practical
approach is:
1. Expand booleans into the phase space (8 phases × 2^6 booleans = 512 flat states)
2. Run Hopcroft on the flat FSM
3. Re-compress the minimal FSM back into phases + fields
4. Compare: did the minimal representation suggest fewer phases or fewer fields?

### Priority order

| # | Simplification | Expected impact | Risk |
|---|---------------|-----------------|------|
| 1 | Remove query transitions from MobMachine | -90 transitions, cleaner TLC | None — queries don't affect state |
| 2 | Flatten MeerkatMachine command enums | Simpler dispatch, no semantic change | Low — mechanical refactor |
| 3 | Merge Idle + Attached | -1 phase, ~20 transition merges | Medium — need to verify all guards |
| 4 | Evaluate Stopped vs Retired equivalence | Potentially -1 phase | Medium — need to compare transition sets |
| 5 | Evaluate Recovering as transient state | Potentially -1 phase | Medium — need to trace all paths |
| 6 | Evaluate Creating vs Running merge | Potentially -1 phase in MobMachine | Medium |
| 7 | Simplify ops lifecycle signal alphabet | -20 signals | Medium — need to verify TLC invariants |
| 8 | Prune MobMachine internal signals | -30+ signals | Low per signal — TLC verifies each |

Items 1-2 are safe mechanical wins. Items 3-6 are phase merges that TLC verifies.
Items 7-8 are signal pruning that reduces formal complexity without affecting runtime.

## Verification

After each simplification:

```bash
# Schema still consistent
cargo test -p meerkat-machine-schema --test schema_contracts --quiet

# Parity with runtime
cargo test -p meerkat-machine-codegen --test runtime_alphabet_parity

# TLC proves equivalence
make machine-verify

# Full test suite (behavioral regression)
./scripts/repo-cargo nextest run --workspace

# Clippy
./scripts/repo-cargo clippy --workspace -- -D warnings
```

The key gate is `make machine-verify` — TLC must stay green after every simplification.
If it breaks, the simplification is invalid and the counterexample trace tells you exactly
why.

## Current Stock-Take

- The latest Mob runtime cut keeps semantic ownership aligned with the checked-in
  two-machine model: production actor paths no longer use helper-owned lifecycle
  mutator entry points, and both lower Mob authorities now receive canonical
  top-level phase plus actor-owned run count on the live path.
- The remaining direct mutator/table entry points in
  `MobLifecycleAuthority`, `MobOrchestratorAuthority`, and the old
  table-oriented ingress replay helpers are now test-only scaffolding for direct
  table verification, not production semantic owners.
- That puts the branch at the point where the next step is no longer another
  obvious helper-authority cut; it is the real endgame sweep: full cargo/test,
  e2e, clippy, TLC, and CI verification on the simplified machine/runtime
  boundary.

## Relationship to the DSL

Simplification is a prerequisite, not a dependency. The DSL works with any machine
definition, simplified or not. But:

- Fewer transitions = smaller DSL definitions = easier to review and maintain
- Fewer phases = simpler dispatch = faster TLC verification
- Fewer signals = less formal overhead = clearer invariant structure

The ideal sequence:

```
1:1 alignment (done)
    → simplification (this proposal)
    → DSL port (machine-dsl-proposal.md)
    → code-first schema (no separate schema catalog)
```

Each step builds on the previous. Simplification makes the DSL port tractable.
The DSL makes drift impossible. Together they deliver the single-source architecture.

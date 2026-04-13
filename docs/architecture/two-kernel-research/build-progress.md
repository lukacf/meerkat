# Two-Kernel Build Progress

Status: active implementation log

This file tracks the slow, verified implementation path from the research docs
toward real code.

Historical note:

- slices 173 onward used an older internal `M1` / `M2` / `M3` numbering for
  Meerkat sub-freeze items
- those labels are now superseded by `K1` / `K2` / `K3` in
  `meerkat-cutover-checklist.md`
- top-level machine milestones are:
  - `M1` = full `MeerkatMachine`
  - `M2` = `MobMachine`
- read the old slice labels as historical shorthand only, not as the current
  milestone scheme

## 2026-04-12 — Slice 242: rebase probe isolates upstream drift to the Meerkat tools seam

Ran the first honest rebase probe against `origin/main` after the machine
freeze / proof work and then aborted it after the first conflict wave was
mapped.

The useful result is architectural, not mechanical:

- the first replayed machine commit collided immediately in:
  - `meerkat-core/src/tool_scope.rs`
  - `meerkat-core/src/gateway.rs`
  - `meerkat-mcp/src/adapter.rs`
  - `meerkat-session/src/ephemeral.rs`
  - plus export / example fallout in `meerkat-core/src/lib.rs` and
    `examples/035-mdm-tux-rs/src/bin/target.rs`
- `origin/main` has materially moved toward durable tool visibility state,
  exact catalog support, and explicit session-task visibility mutation seams
- the semantic drift is concentrated in `MeerkatMachine.tools`, not spread
  uniformly across Meerkat or Mob

That means the next rebase must be machine-led in the `tools` region rather
than conflict-led. The alignment note is now:

- `meerkat-upstream-tool-alignment.md`

Honest read after this slice:

- the current branch freeze still describes the current branch honestly
- the next full rebase should deliberately reopen the Meerkat `tools` region
  against upstream ownership drift
- the Mob freeze / proof package does not appear to be the primary rebase risk

## 2026-04-12 — Slice 243: tool-region realignment plan replaces generic rebase anxiety

Turned the rebase probe into a concrete machine-alignment plan:

- added `meerkat-tools-realignment-plan.md`

The plan makes the next work explicit:

- split the exact-current tools freeze into:
  - durable visibility ownership
  - external tool-surface lifecycle
- revisit the target `MeerkatMachine.tools` region because the current target
  schema is probably too small for the upstream visibility/catalog move
- update the Meerkat input/effect alphabet and target TLA scaffold only after
  the exact-current seam is re-understood

This is a useful step because it converts “the rebase is scary” into a
bounded ownership problem with a concrete order of attack.

## 2026-04-12 — Slice 244: upstream tool-visibility baseline is now explicit

Added a concrete upstream-baseline note:

- `meerkat-tool-visibility-upstream-baseline.md`

This closes an important gap in the earlier alignment work. Before this slice,
the plan said the tools seam had drifted, but a reviewer still had to infer the
actual new ownership shape from raw `origin/main` code. The new note makes the
machine-level read explicit:

- upstream now has durable visibility ownership (`SessionToolVisibilityState`)
- upstream `ToolScope` is already more projection-like than the current-branch
  freeze story
- exact catalog support is entering the gateway / adapter seam
- a session-task visibility mutation seam now exists

That means the next rebase no longer starts from “tool conflicts.” It starts
from a named upstream baseline for the `tool_visibility` subregion and a
separate `tool_surface` lifecycle subregion.

## 2026-04-12 — Slice 245: file-by-file tools merge strategy replaces generic conflict handling

Added:

- `meerkat-tools-merge-strategy.md`

This closes the remaining operational gap in the alignment work. Before this
slice, we knew the tools seam had drifted and we had a realignment plan, but
the next rebase still depended on ad hoc judgment inside the conflict wave.

The merge strategy now makes the intended source of truth explicit:

- `tool_scope.rs`: upstream ownership, branch projection helpers only
- `gateway.rs`: upstream exact-catalog semantics
- `adapter.rs`: upstream exact-catalog and pending-source projection
- `ephemeral.rs`: upstream session-task visibility mutation seam

That means the next rebase can now be evaluated against a machine-led merge
plan rather than just “did the code compile afterwards?”

## 2026-04-12 — Slice 246: target tools-region delta is now explicit

Added:

- `meerkat-tools-target-delta.md`

This closes the last conceptual gap in the alignment work so far. Before this
slice, we had:

- an upstream baseline for the richer tools seam
- an exact-current realignment plan
- a file-by-file merge strategy

But we still did not have an explicit target-state read of what that upstream
ownership move likely means for the frozen `MeerkatMachine.tools` region.

The new note makes that explicit:

- the target `tools` region should likely be treated as
  `tool_visibility + tool_surface`
- durable visibility owner state, exact-catalog capability, committed
  publication revision, dormant missing intent, and ownership witnesses are the
  main candidate additions

That means the next rebase can now be judged against both sides of the machine:

- the rebased exact-current baseline
- the re-frozen target-state `tools` region

## 2026-04-09 — Slice 172: TLC target scaffold caught destroyed-rebind ambiguity

The first executable TLC pass on the target-state `MeerkatMachine` scaffold
found a real freeze omission rather than a code issue: the target docs did not
say whether `DestroyRuntime` was terminal for the machine instance, so the
model allowed `PrepareBindings` to rebind from `Destroyed` and immediately
violated the destroyed-shape invariant.

The freeze package now closes that explicitly:

- `DestroyRuntime` is terminal for the machine instance
- `PrepareBindings` requires the pre-binding initializing state
- rebinding after destroy is outside this machine and belongs to a fresh
  `Init`
- `Destroyed` is now explicitly stutter-only for the machine instance

## 2026-04-09 — Slice 173: First executable target TLC scaffold

Added the first non-generated target-state TLA+ scaffold in:

- `tla/MeerkatMachineTarget.tla`
- `tla/MeerkatMachineTarget.cfg`
- `tla/README.md`

The scaffold encodes:

- the full frozen region schema
- canonical `Init`
- derived predicates
- a broad grouped `Next` touching every machine region
- a first safety invariant set derived from the proof obligations

This was the first direct executable pressure test of the target-state
`MeerkatMachine` freeze rather than of the exact-current baseline.

## 2026-04-09 — Slice 174: Bounded TLC passes on target machine scaffold

After closing the destroyed-state ambiguity, the target machine scaffold now
passes bounded TLC in both the base and widened stress configs:

- `tla/MeerkatMachineTarget.cfg`
- `tla/MeerkatMachineTargetStress.cfg`

The widened run explored 189,575 distinct states to depth 7 with no invariant
violations. This does not replace the full proof, but it is strong evidence
that the freeze package is now executable and internally consistent at the
target-machine level.

## 2026-04-09 — Slice 175: Strengthened target safety invariants survive TLC

Raised the executable target model to a stronger safety set derived more
directly from the frozen proof obligations:

- `Running => HasActiveRun`
- active-run phase restriction
- non-empty contributors for active runs
- ready-turn shape
- non-empty `wait_all` identity when active
- `active_count` equals the number of pending/running ops
- visible surfaces require machine base state
- snapshot alignment monotonicity
- non-inactive drain requires both a live binding and a non-disabled drain mode

Both bounded TLC configurations still passed after those invariants were added:

- base config: 25,129 distinct states, depth 6
- stress config: 189,575 distinct states, depth 7

At this point the target `MeerkatMachine` freeze is not just documented and
cross-linked; it has an executable bounded model that survives a nontrivial
safety suite.

## 2026-04-10 — Slice 220: Mob target freeze package closes

The Mob target-state package is now explicit enough to review as one frozen
asset instead of as a scattered set of passing TLC runs.

What closed in this slice:

- added `mob-cutover-checklist.md` as the explicit `M2 = MobMachine`
  target-state close-out artifact
- reran bounded TLC after the latest work/step and quorum-contribution
  strengthenings:
  - base config passed
  - stress config passed
- reran the target alphabet coverage audit against
  `mob-machine-coverage-matrix.md` and `tla/MobMachineTarget.tla`
- confirmed the audit is clean:
  - `MISSING_INPUTS []`
  - `EXTRA_INPUTS []`
  - `MISSING_EFFECTS []`
  - `EXTRA_EFFECTS []`
- confirmed hedge sweeps over active `mob-machine-*` freeze docs are clean

Why this matters:

- the Mob target machine now has the same kind of explicit close-out artifact
  that made the Meerkat freeze reviewable
- the target machine is no longer “probably complete if you read enough docs”
- alphabet coverage, executable TLC, and freeze-package completeness all now
  point at the same answer: `M2 = MobMachine` is frozen strongly enough to
  start TLA+ proof work

## 2026-04-10 — Slice 221: Mob handoff package gains alphabet, lowering map, and ownership decisions

The Mob freeze package was broad and executable, but it was still missing the
same handoff artifacts that made the Meerkat freeze review-proof.

What landed:

- `mob-input-effect-alphabet.md`
- `mob-lowering-map.md`
- `mob-ownership-decisions.md`

Why this matters:

- the target Mob alphabet is now explicit as a handoff artifact, not just
  recoverable from the transition catalog and coverage matrix
- implementation refinement no longer depends on remembering the current
  actor/handle paths from code spelunking alone

## 2026-04-12 — Slice 261: Mob-side Meerkat diagnostic bridge lands as best-effort surface

Extended the Mob runtime-facing diagnostic bridge so `MobHandle` can query
Meerkat-side diagnostic inputs for a live member session through the session
service when those surfaces are actually available.

What landed in code:

- `meerkat-mob/src/runtime/session_service.rs`
  - added best-effort diagnostic snapshot methods on `MobSessionService`:
    - `execution_snapshot(...)`
    - `tool_scope_snapshot(...)`
    - `external_tool_surface_snapshot(...)`
    - `peer_ingress_runtime_snapshot(...)`
  - wired the real `EphemeralSessionService` / `PersistentSessionService`
    implementations through to their canonical diagnostic paths
- `meerkat-mob/src/runtime/handle.rs`
  - added `MeerkatShadowInputsSnapshot`
  - added `diagnostic_meerkat_shadow_inputs(...)`
- `meerkat-mob/src/runtime/tests.rs`
  - added a live runtime-adapter-backed probe proving the bridge becomes
    queryable once a member session is live

The useful backtrack was semantic, not mechanical:

- the first test incorrectly assumed a mock-backed runtime-adapter session
  service must surface a full `AgentExecutionSnapshot`
- that is not honest: `MockSessionService` owns no canonical live agent-task
  diagnostic path
- the bridge is therefore frozen as a best-effort diagnostic surface:
  real session services may return execution/tool snapshots, while mock-backed
  runtime-adapter services may legitimately return `None`

Verification:

- focused bridge probe passed
- full `cargo test -p meerkat-mob --lib` passed

Why this matters:

- the Mob side now has an honest query surface for Meerkat diagnostic inputs
  when the underlying session service actually owns them
- we avoided baking a fake “all live members always have execution snapshots”
  rule into the pre-cutover alignment work

## 2026-04-12 — Slice 262: first cutover-facing shadow scenario matrix lands

Added:

- `two-kernel-shadow-scenario-matrix.md`

This closes the next gap in the alignment work. Before this slice, we had:

- frozen machines
- proven machines
- landed shadow lanes
- hook inventories
- aggregate suite helpers

But the next live step still depended on informal judgment about which real
scenario to run first and what each scenario was supposed to teach us.

The new matrix now makes that explicit:

- each first cutover-facing scenario names one primary suite helper
- each scenario maps to already-landed lanes
- each scenario predicts likely mismatch classes up front
- execution order now reflects current confidence instead of ad hoc preference

That means the next step is no longer “run something realistic.” It is to pick
the first scenario from the matrix and start collecting real mismatch taxonomy
against the rebased frozen/proven baseline.

## 2026-04-12 — Slice 263: first cutover-facing Meerkat shadow scenario lands

Landed the first real scenario-level shadow run from the matrix in:

- `meerkat/src/meerkat_machine.rs`

What now exists:

- `capture_all_meerkat_shadow_reports_stay_empty_across_plain_reset_with_pending_ops`

This scenario uses the already-landed aggregate Meerkat suite helper and
checks three points on the same real path:

- before reset
- after reset while `wait_all` is still live
- after the pending op settles

The important result is that the aggregate frozen Meerkat lanes remain clean
through the full reset split, not just in isolated lane-level unit tests.

This is the first genuine cutover-facing shadow scenario because it exercises
multiple lanes together against the live runtime:

- `LifecycleControl`
- `TurnOpsBarrier`
- `PeerDrain`

Verification:

- focused scenario test passed
- full `cargo test -p meerkat --lib` passed

Why this matters:

- the shadow program is now beyond helper surfaces and into scenario-level
  validation
- the next scenario should move to the first Mob lifecycle seam:
  Mob provision / kickoff / finalize
- the remaining target-state ownership choices are now frozen instead of being
  spread implicitly across older architecture notes

Package alignment updates:

- `mob-machine-freeze.md` now treats those notes as part of the canonical
  target proof handoff package
- `mob-cutover-checklist.md` now requires them for `C7`
- `README.md` and `tla/README.md` now list them in the active Mob package

## 2026-04-12 — Slice 264: Scenario 3 lands for rebased tools seam

What landed:

- a real cutover-facing scenario-level Meerkat shadow test in
  `meerkat/src/meerkat_machine.rs`:
  `capture_all_meerkat_shadow_reports_stay_empty_across_tool_visibility_mutation_and_mcp_apply`

What it covers:

- stages a real live session-plane visibility mutation through
  `SessionService::set_session_tool_filter(...)`
- drives a real MCP reload/apply/wait sequence through `McpRouterAdapter`
- samples the aggregate frozen Meerkat suite:
  - before mutation
  - after the staged visibility change
  - after MCP apply
  - after MCP settle

Useful backtracks:

- the first version incorrectly tried to filter the staged MCP server name
  itself; the rebased tools seam does not currently expose that as a filterable
  session-plane name
- the landed scenario uses a tiny local composite dispatcher so one stable
  filterable session-plane tool and one live MCP surface can coexist on the
  same session honestly

Verification:

- `cargo test -p meerkat --lib capture_all_meerkat_shadow_reports --features mcp`
- `cargo test -p meerkat --lib --features mcp`

## 2026-04-12 — Slice 265: Scenario 4 lands for rebased peer/trust seam

What landed:

- a real cutover-facing scenario-level Meerkat shadow test in
  `meerkat/src/meerkat_machine.rs`:
  `capture_all_meerkat_shadow_reports_stay_empty_across_peer_ingress_trust_and_drain`

What it covers:

- creates a real comms-enabled runtime-backed session
- performs real trust mutation through `CommsRuntime::add_trusted_peer(...)`
- performs real peer ingress through the live event injector
- drives the real drain lifecycle seam through
  `RuntimeSessionAdapter::update_peer_ingress_context(...)`
- samples the aggregate frozen Meerkat suite:
  - before ingress
  - after trust + ingress + live drain
  - after drain stop

Verification:

- `cargo test -p meerkat --lib capture_all_meerkat_shadow_reports --features comms`

## 2026-04-12 — Slice 266: Scenario 5 lands for Mob provision / kickoff / finalize

What landed:

- a real cutover-facing scenario-level Mob shadow test in
  `meerkat-mob/src/runtime/tests.rs`:
  `test_capture_mob_shadow_suite_report_stays_empty_across_provision_kickoff_finalize`

What it covers:

- creates a real runtime-adapter-backed mob with autonomous kickoff enabled
- samples the aggregate frozen Mob + seam suite:
  - before any member is provisioned
  - during staged pending spawn lineage
  - after spawn finalization while kickoff is still pending
  - after kickoff settles

Important shape choice:

- “finalize” here means the real lifecycle completion of spawn provisioning into
  a canonical roster/session-backed member, followed by kickoff-barrier
  settlement
- it does **not** smuggle in stop/destroy semantics under a nicer name

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_report_stays_empty_across_provision_kickoff_finalize`
- `cargo test -p meerkat-mob --lib`

## 2026-04-12 — Slice 267: Scenario 6 lands for single-step flow run

What landed:

- a real cutover-facing scenario-level Mob shadow test in
  `meerkat-mob/src/runtime/tests.rs`:
  `test_capture_mob_shadow_suite_report_stays_empty_across_single_step_flow_run`

What it covers:

- uses a runtime-adapter-backed mob with the canonical single-step `demo` flow
- samples the aggregate frozen Mob + seam suite:
  - before flow dispatch
  - while tracked run state and live Meerkat work posture are both observable
  - after terminal flow completion and cleanup

Why this matters:

- Scenario 5 proved provisioning/kickoff/finalization remains coherent
- this scenario proves the first real flow/work aggregate path remains coherent
  too, without relying on lane-local unit tests

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_report_stays_empty_across_single_step_flow_run`
- `cargo test -p meerkat-mob --lib`

## 2026-04-12 — Slice 268: Scenario 8 replacement-provisioning branch lands

What landed:

- a real cutover-facing Mob/seam supersession scenario test in
  `meerkat-mob/src/runtime/tests.rs`:
  `test_capture_mob_shadow_suite_report_stays_empty_across_respawn_supersession`

What it covers:

- uses a runtime-adapter-backed member with replacement provisioning on the
  real respawn path
- samples the aggregate frozen Mob + seam suite:
  - before respawn
  - after respawn returns with the old bridge archived and the new bridge current

Important honesty note:

- this lands the **settled** `retire with replacement provisioning` branch of
  Scenario 8
- it does **not** claim that the separate `destroy in flight` branch is already
  covered by the same scenario

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_report_stays_empty_across_respawn_supersession`
- `cargo test -p meerkat-mob --lib`

## 2026-04-12 — Slice 269: Scenario 8 destroy-in-flight branch lands

What landed:

- a real cutover-facing Mob/seam destroy supersession scenario test in
  `meerkat-mob/src/runtime/tests.rs`:
  `test_capture_mob_shadow_suite_report_stays_empty_across_destroy_inflight_flow`

What it covers:

- uses a runtime-adapter-backed member with a deliberately nonterminal
  single-step flow
- samples the aggregate frozen Mob + seam suite:
  - before flow dispatch
  - while tracked run state and live Meerkat work posture are both observable
  - then validates the stable post-destroy seam through:
    - `MobState::Destroyed`
    - archived live sessions
    - durable canceled run state

Important honesty note:

- this lands the `destroy in flight` branch of Scenario 8 against the stable
  surfaces the runtime actually owns:
  active tracked run visibility, live Meerkat work posture, durable canceled
  terminal run state, and destroyed mob/seam cleanup
- it does **not** pretend the aggregate shadow suite itself remains callable
  after the actor has fully dropped; post-destroy checks intentionally use the
  durable and externally visible surfaces that survive actor teardown

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_report_stays_empty_across_destroy_inflight_flow`
- `cargo test -p meerkat-mob --lib`

## 2026-04-12 — Slice 270: Scenario 7 attached recover replay branch lands

What landed:

- a real cutover-facing aggregate Meerkat shadow scenario test in
  `meerkat/src/meerkat_machine.rs`:
  `capture_all_meerkat_shadow_reports_stay_empty_across_attached_recover_replay`

What it covers:

- uses a runtime-backed session with an attached blocking executor
- drives the aggregate frozen Meerkat suite through:
  - pre-recover attached queued work + live `wait_all`
  - in-flight attached replay after `recover()`
  - post-replay state while `wait_all` still remains live
  - settled state after the waited background operation completes

Important honesty note:

- this lands the `attached recover replay` branch of Scenario 7
- it does **not** yet claim that the symmetric `attached recycle replay`
  branch is already covered

Verification:

- `cargo test -p meerkat --lib capture_all_meerkat_shadow_reports_stay_empty_across_attached_recover_replay`
- `cargo test -p meerkat --lib`

## 2026-04-12 — Slice 271: Scenario 7 attached recycle replay branch lands

What landed:

- a second real cutover-facing aggregate Meerkat shadow scenario test in
  `meerkat/src/meerkat_machine.rs`:
  `capture_all_meerkat_shadow_reports_stay_empty_across_attached_recycle_replay`

What it covers:

- uses a runtime-backed session with an attached blocking executor
- drives the aggregate frozen Meerkat suite through:
  - pre-recycle attached queued work + live `wait_all`
  - in-flight attached replay after `recycle()`
  - post-replay state while `wait_all` still remains live
  - settled state after the waited background operation completes

Important honesty note:

- this closes the symmetric `attached recycle replay` branch of Scenario 7
- Scenario 7 is now fully landed for Meerkat:
  - attached recover replay
  - attached recycle replay

Verification:

- `cargo test -p meerkat --lib capture_all_meerkat_shadow_reports_stay_empty_across_attached_recycle_replay`
- `cargo test -p meerkat --lib`

## 2026-04-12 — Slice 272: Scenario 7 branch fallback failure-history run lands

What landed:

- a real cutover-facing aggregate Mob + seam shadow scenario test in
  `meerkat-mob/src/runtime/tests.rs`:
  `test_capture_mob_shadow_suite_report_stays_empty_across_branch_fallback_failure_history`

What it covers:

- uses the real `branch_fallback` flow family with:
  - one failing branch candidate
  - one healthy fallback candidate
  - a live nonterminal failure history on the tracked run
- drives the aggregate frozen Mob + seam suite through:
  - pre-run state
  - active `Running` state while `failure_count > 0`
  - settled post-terminal cleanup

Important honesty note:

- this closes the final first-wave cutover-facing scenario from the scenario
  matrix
- the next step is no longer “land the next scenario”
- the next step is to use the landed aggregate suites to collect real mismatch
  taxonomy from broader shadow runs

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_report_stays_empty_across_branch_fallback_failure_history`
- `cargo test -p meerkat-mob --lib`

## 2026-04-12 — Slice 273: Broader shadow taxonomy smoke run starts

What landed:

- taxonomy summarizers over aggregate shadow suites:
  - `summarize_meerkat_shadow_taxonomy_reports(...)`
  - `summarize_mob_shadow_taxonomy_reports(...)`
- a broader Mob + seam smoke run in `meerkat-mob/src/runtime/tests.rs`:
  - `test_capture_mob_shadow_suite_taxonomy_stays_empty_across_broader_smoke_run`

What it covers:

- treats the landed aggregate suite helpers as a real broader-run collection
  surface instead of just a pointwise assertion helper
- samples the aggregate Mob + seam shadow suite across:
  - initial provisioned state
  - active single-step flow work
  - settled flow cleanup
  - settled respawn supersession
- collapses all collected suite reports into one mismatch taxonomy

Important honesty note:

- this is the first post-matrix broader shadow run
- it does not yet claim that broader live runs surface real mismatches
- it does prove that the rebased frozen/proven baseline still yields an empty
  mismatch taxonomy across a multi-scenario Mob + seam smoke path

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_taxonomy_stays_empty_across_broader_smoke_run`
- `cargo test -p meerkat-mob --lib`

## 2026-04-12 — Slice 274: Broader Meerkat shadow taxonomy smoke run lands

What landed:

- a Meerkat-side taxonomy summarizer over aggregate shadow suites:
  - `summarize_meerkat_shadow_taxonomy_reports(...)`
- a broader Meerkat smoke run in `meerkat/src/meerkat_machine.rs`:
  - `capture_meerkat_shadow_taxonomy_stays_empty_across_broader_smoke_run`

What it covers:

- treats the landed aggregate Meerkat suite helper as a real broader-run
  collection surface instead of just a pointwise assertion helper
- samples the aggregate Meerkat shadow suite across:
  - healthy baseline
  - plain reset with pending ops
  - settled post-reset wait-all cleanup
  - attached recover replay in-flight and settled
- collapses all collected suite reports into one mismatch taxonomy

Important honesty note:

- this is the first post-matrix broader Meerkat shadow run
- it does not yet claim that broader live runs surface real mismatches
- it does prove that the rebased frozen/proven baseline still yields an empty
  mismatch taxonomy across a multi-phase Meerkat smoke path

Verification:

- `cargo test -p meerkat --lib capture_meerkat_shadow_taxonomy_stays_empty_across_broader_smoke_run`

## 2026-04-12 — Slice 275: Seeded taxonomy validation lands on both kernels

Goal:

- prove the new taxonomy helpers are useful on real seeded drift, not only empty
  on the rebased happy path

What landed:

- added a seeded aggregate taxonomy test in `meerkat/src/meerkat_machine.rs`:
  - `summarize_meerkat_shadow_taxonomy_reports_collapses_seeded_lifecycle_control_drift`
- added a seeded aggregate taxonomy test in `meerkat-mob/src/mob_machine.rs`:
  - `summarize_mob_shadow_taxonomy_reports_collapses_seeded_mob_and_seam_drift`
- each test reuses already-landed seeded mismatch shapes instead of inventing a
  new drift harness:
  - Meerkat: lifecycle/control drift
  - Mob: provisioning drift plus seam work-bridge drift

Why this slice matters:

- it proves the aggregate taxonomy output is actually actionable when drift is
  present
- it keeps the next refinement step honest: we can now distinguish “taxonomy is
  empty on green paths” from “taxonomy can classify real mismatches cleanly”

Result:

- broader smoke runs are still green on the rebased baseline
- taxonomy usefulness is now validated on seeded drift for both machines
- the next refinement target is:
  - mismatch-producing live runs that surface the first real taxonomy classes

Validation:

- `cargo test -p meerkat --lib summarize_meerkat_shadow_taxonomy_reports_collapses_seeded_lifecycle_control_drift`
- `cargo test -p meerkat-mob --lib summarize_mob_shadow_taxonomy_reports_collapses_seeded_mob_and_seam_drift`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`

## 2026-04-09 — Slice 1: Meerkat runtime spine snapshot

Goal:

- start building `MeerkatMachine` without forcing a premature semantic collapse
- verify the architecture against the current codebase continuously
- create an explicit Meerkat-side internal surface in code before moving
  behavior between owners

What landed:

- new internal diagnostic module:
  - `meerkat-runtime/src/meerkat_machine.rs`
- new internal runtime adapter method:
  - `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)`
- new ingress getters needed to read canonical Meerkat admission truth:
  - content shape
  - request id
  - reservation key
- driver accessors needed to project runtime identity cleanly:
  - ephemeral `runtime_id()`
  - persistent `runtime_id()`
- first verification tests in `meerkat-runtime/src/session_adapter.rs`

Current scope of the snapshot:

- `binding`
  - session id
  - runtime id
  - driver kind
  - attachment liveness
  - detached wake presence
  - hidden epoch id
- `control`
  - runtime phase
  - current run id
  - pre-run phase
  - wake/process flags
- `inputs`
  - ingress phase
  - admission order
  - queue and steer queue
  - current run contributors
  - wake/process request flags
  - silent intent overrides
  - per-input lifecycle and metadata snapshot

Deliberate non-scope for slice 1:

- no semantic rewrite
- no new reducer
- no mob changes
- no attempt yet to fold ops, peer ingress, tool surface, or drain lifecycle
  into the same projected structure

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- passed

Backtracks encountered:

- the first compile exposed that the snapshot was mixing
  `runtime_control_authority::HandlingMode` with `meerkat_core::types::HandlingMode`
- `ContentShape` needed cloning rather than copying
- `InputLifecycleState` had to be imported from `input_state`, not from the
  authority wrapper

Why this slice matters:

- it gives the Meerkat refactor a real internal code anchor
- it verifies that the current runtime spine can already be observed as one
  grouped unit
- it keeps us honest about where current truth still lives

Next likely slice:

- extend the Meerkat internal projection to include `ops` and `drain`
- then decide whether to move `RuntimeSessionEntry` into an explicit
  Meerkat-side kernel struct or keep growing the projection first

## 2026-04-09 — Slice 2: `ops` and `drain` join the Meerkat spine

Goal:

- extend the existing Meerkat runtime projection with more of the real kernel
  coupling points
- keep the slice observational rather than rewriting runtime behavior
- verify the new projection against the authority-owned ops lifecycle and comms
  drain state

What landed:

- `MeerkatOpsSnapshot` in `meerkat-runtime/src/meerkat_machine.rs`
- `MeerkatDrainSnapshot` in `meerkat-runtime/src/meerkat_machine.rs`
- `RuntimeOpsLifecycleRegistry::diagnostic_snapshot()` in
  `meerkat-runtime/src/ops_lifecycle.rs`
- `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` now projects:
  - operation count
  - active operation count
  - authority-owned wait request id
  - authority-owned wait operation ids
  - operation lifecycle snapshots
  - detached wake pending/signaled flags
  - comms drain slot presence
  - comms drain phase
  - comms drain mode
  - comms drain handle presence
- new adapter tests covering:
  - live operation state
  - wait-all state
  - comms drain state

Why this slice matters:

- it brings two of the highest-bug-density Meerkat regions into the same
  explicit projected surface
- it proves that the current code already exposes authoritative wait/drain
  truth without needing a semantic rewrite first
- it keeps detached wake and comms drain visible as Meerkat internal facts
  rather than shell folklore

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --lib`

Result:

- passed

Backtracks encountered:

- the first wait-all test incorrectly minted its own `WaitRequestId`; the
  runtime authority owns that id, so the test now snapshots the real wait id
  and verifies it matches the eventual `WaitAllSatisfied` result
- the initial test also used a non-existent `OperationResult::Text(...)`
  helper; it had to be rewritten against the real struct shape used by
  `RuntimeOpsLifecycleRegistry`

## 2026-04-09 — Slice 3: strengthen hidden `binding`

Goal:

- move the Meerkat binding projection closer to the internal kernel shape
- expose hidden epoch cursor facts and kernel-handle presence without changing
  runtime behavior

What landed:

- `MeerkatCursorSnapshot` in `meerkat-runtime/src/meerkat_machine.rs`
- `MeerkatBindingSnapshot` now includes:
  - `driver_present`
  - `completions_present`
  - `ops_registry_present`
  - `cursor_state`
- `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` now projects
  `EpochCursorState` from the real `RuntimeSessionEntry`
- new adapter test covering:
  - epoch cursor projection from mutated live cursor state

Why this slice matters:

- it makes the hidden Meerkat binding more explicit as a kernel handle rather
  than just a runtime id plus attachment bit
- it confirms that epoch/cursor continuity is already a first-class Meerkat
  fact in the current codebase
- it gives us a firmer base before attempting a more structural Meerkat
  refactor

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --lib`

Result:

- passed

Current shape of the projected Meerkat spine:

- `binding`
- `control`
- `inputs`
- `ops`
- `drain`

Still deliberately out of scope:

- no turn/execution-region projection yet
- no peer ingress region yet
- no tool-surface region yet
- no structural replacement of `RuntimeSessionEntry`

Next likely slice:

- project the Meerkat turn/execution region if the current runtime exposes a
  stable enough truth surface
- otherwise, make `RuntimeSessionEntry` itself a more explicit Meerkat kernel
  handle before attempting turn/tools collapse

## 2026-04-09 — Slice 4: cross-crate execution snapshot for Meerkat turn truth

Goal:

- expose the real turn/execution authority without inventing a fake
  runtime-local owner
- verify the Meerkat turn region against the current codebase as it actually
  exists today: in `meerkat-core` and the live session task
- create a mapping path from core agent state to session service that later
  MeerkatMachine work can consume

What landed:

- `AgentExecutionSnapshot` in `meerkat-core/src/agent.rs`
- `Agent::execution_snapshot()` in `meerkat-core/src/agent/runner.rs`
- `SessionAgent::execution_snapshot()` default hook in
  `meerkat-session/src/ephemeral.rs`
- new session-task command path:
  - `SessionCommand::ExecutionSnapshot`
  - `EphemeralSessionService::execution_snapshot(...)`
  - `PersistentSessionService::execution_snapshot(...)`
- `FactoryAgent` now forwards the real core-agent execution snapshot in
  `meerkat/src/service_factory.rs`

Current scope of the execution snapshot:

- coarse loop state
- rich turn phase from `TurnExecutionAuthority`
- active run id
- primitive kind
- admitted content shape
- vision/image-tool flags
- tool-call pending count
- pending async operation ids
- barrier operation ids
- barrier flags
- boundary count
- cancel-after-boundary
- terminal outcome
- extraction retry state
- applied cursor

Why this slice matters:

- it confirms that the Meerkat turn region is not owned by
  `meerkat-runtime`; it lives in the core agent loop and has to be mapped
  across crates honestly
- it gives the MeerkatMachine work its first real read path into the turn
  authority instead of forcing runtime-local mirroring
- it exposes both `LoopState` and `TurnPhase` side by side, which is useful
  because they are related views, not identical truths

Verification:

- `cargo test -p meerkat-core --lib test_execution_snapshot_reflects_turn_authority`
- `cargo test -p meerkat-session --test regression_lifecycle execution_snapshot_returns_live_agent_execution_state`
- `cargo test -p meerkat --lib factory_agent_execution_snapshot_forwards_core_state`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- passed

Backtracks encountered:

- the planned runtime-only turn projection was wrong: `RuntimeSessionAdapter`
  does not own the core agent, so trying to add turn truth there first would
  have created another shadow seam
- adding the ops diagnostic slice made a few old `#[expect(dead_code)]`
  markers in `ops_lifecycle_authority` inaccurate; those were removed once the
  helpers became part of the live diagnostic path

Current MeerkatMachine picture after slice 4:

- runtime-owned projected regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- core/session-owned projected region:
  - `turn`

Still deliberately out of scope:

- no peer ingress region projection yet
- no tool-surface region projection yet
- no unified Meerkat cross-crate snapshot that joins runtime + turn in one
  place yet
- no structural replacement of `RuntimeSessionEntry`

Next likely slice:

- either build the first cross-crate Meerkat diagnostic join between runtime
  spine and session execution snapshot
- or, if that join is still too awkward, make the hidden runtime handle more
  explicit before attempting peer/tools projection

## 2026-04-09 — Slice 5: first joined Meerkat snapshot across real owners

Goal:

- build the first single read path that sees both halves of current
  `MeerkatMachine` truth
- keep the join at an honest integration point instead of pretending
  `meerkat-runtime` owns turn execution
- verify the resulting surface against real runtime-backed session creation

What landed:

- `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` is now
  hidden-public scaffolding instead of crate-private only
- `meerkat-runtime` now hidden-reexports the runtime-side Meerkat snapshot
  types so another crate can compose them without copying state
- new internal facade module:
  - `meerkat/src/meerkat_machine.rs`
- new internal helper:
  - `capture_meerkat_machine_snapshot(...)`
- new internal joined snapshot:
  - `MeerkatMachineSnapshot { spine, turn }`
- new internal execution-source trait covering both:
  - `EphemeralSessionService`
  - `PersistentSessionService`

Current scope of the joined snapshot:

- runtime-owned regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- session/core-owned region:
  - `turn`

Why this slice matters:

- it is the first place in real code where the current Meerkat split can be
  observed as one machine-shaped surface
- it proves that the correct join point is the facade/integration layer, not
  `meerkat-runtime` itself
- it makes an important current state explicit: runtime registration can exist
  before a live session task is attached, so `turn = None` is sometimes honest
  rather than erroneous

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat --lib --features session-store,memory-store capture_meerkat_machine_snapshot`
- `cargo test -p meerkat-session --test regression_lifecycle execution_snapshot_returns_live_agent_execution_state`

Result:

- passed

Backtracks encountered:

- the first instinct was to put the joined snapshot helper into
  `meerkat-runtime`; that would have pulled the turn/execution seam back under
  the runtime crate and repeated the ownership mistake from slice 4
- the session-side execution source needed a `'static` builder bound because
  the live session task keeps the builder behind async task ownership; the
  narrower bound compiled in the small but not in the joined helper

Current MeerkatMachine picture after slice 5:

- runtime-owned projected regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- core/session-owned projected region:
  - `turn`
- facade-level joined read path:
  - `MeerkatMachineSnapshot`

Still deliberately out of scope:

- no peer ingress region projection yet
- no tool-surface region projection yet
- no structural replacement of `RuntimeSessionEntry`
- no reducer or transition collapse; this is still observational scaffolding

Next likely slice:

- inspect `Peer Ingress` versus `External Tool Surface` and pick the smaller
  honest projection target for the next Meerkat region
- keep that slice runtime-first only if the real owner truly lives there;
  otherwise add another cross-crate join deliberately

## 2026-04-09 — Slice 6: tool-scope visibility joins the Meerkat view

Goal:

- extend the joined Meerkat snapshot with another real Meerkat region that
  already has explicit owner state
- avoid forcing a dishonest peer-ingress snapshot while the classified inbox
  still lives in transient channels
- keep the new slice grounded in real functions used by the live agent loop

What landed:

- `ToolScopeSnapshot` in `meerkat-core/src/tool_scope.rs`
- `ToolScope::snapshot()` in `meerkat-core/src/tool_scope.rs`
- `Agent::tool_scope_snapshot()` in `meerkat-core/src/agent/runner.rs`
- `SessionAgent::tool_scope_snapshot()` default hook plus session-task command
  path in `meerkat-session/src/ephemeral.rs`
- `PersistentSessionService::tool_scope_snapshot(...)` forwarding in
  `meerkat-session/src/persistent.rs`
- `FactoryAgent` forwarding in `meerkat/src/service_factory.rs`
- `MeerkatMachineSnapshot` now includes `tools` in
  `meerkat/src/meerkat_machine.rs`

Current scope of the tool-scope snapshot:

- known base tool names
- currently visible tool names
- base filter
- active external filter
- active turn overlay (allow/deny)
- active revision
- staged external filter
- staged revision

Why this slice matters:

- it covers a real subset of the Meerkat external-tool surface using state that
  already has one owner: `ToolScope`
- it maps directly to the live execution functions that matter:
  - `Agent::stage_external_tool_filter(...)`
  - `ToolScope::apply_staged(...)`
  - provider-visible tool selection at CallingLlm boundaries
- it expands the joined Meerkat read path without inventing another shell-side
  cache or projection

Verification:

- `cargo test -p meerkat-core --lib snapshot_reflects_active_and_staged_scope_state`
- `cargo test -p meerkat-session --test regression_lifecycle tool_scope_snapshot_returns_live_agent_tool_scope_state`
- `cargo test -p meerkat --lib factory_agent_tool_scope_snapshot_forwards_core_state`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib --features session-store,memory-store capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-session --test regression_lifecycle execution_snapshot_returns_live_agent_execution_state`

Result:

- passed

Backtracks encountered:

- `Peer Ingress` looked like the next semantic region, but the current
  classified inbox still keeps queued ingress truth inside transient channel
  buffers. A non-destructive snapshot there would have required either
  shadow state or an owner refactor first
- that made `External Tool Surface` the smaller honest slice, but only its
  current `ToolScope` subregion, not the full future router/surface kernel

Current MeerkatMachine picture after slice 6:

- runtime-owned projected regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- core/session-owned projected regions:
  - `turn`
  - `tools` (current tool-scope visibility subregion)
- facade-level joined read path:
  - `MeerkatMachineSnapshot { spine, turn, tools }`

Still deliberately out of scope:

- no explicit peer-ingress region projection yet
- no full external-tool router/surface state beyond `ToolScope`
- no structural replacement of `RuntimeSessionEntry`
- no reducer or transition collapse

Next likely slice:

- either refactor classified inbox ownership so peer-ingress queue truth becomes
  explicitly snapshotable
- or continue filling Meerkat from the core side by mapping another explicit
  owner region before attempting the peer refactor

## 2026-04-09 — Slice 7: peer ingress queue becomes explicit owner state

Goal:

- take the next honest Meerkat peer-ingress slice without fabricating a shadow
  queue model
- move classified ingress queue truth out of transient receiver state and into
  an explicit owned structure
- expose that queue through a hidden cross-crate diagnostic seam before trying
  to collapse broader peer semantics

What landed:

- `meerkat-comms/src/inbox.rs`
  - replaced the classified `mpsc::Receiver` storage with an explicit bounded
    `ClassifiedInboxQueue`
  - kept raw inbox channel behavior unchanged
  - added non-destructive `classified_snapshot()`
  - added explicit closed-state handling for classified senders
- `meerkat-core/src/interaction.rs`
  - added `PeerIngressKind`
  - added `PeerIngressEntrySnapshot`
  - added `PeerIngressQueueSnapshot`
- `meerkat-core/src/agent.rs`
  - added hidden `CommsRuntime::peer_ingress_queue_snapshot()` capability
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - implemented `peer_ingress_queue_snapshot()` over the real inbox owner
- `meerkat/src/meerkat_machine.rs`
  - `MeerkatMachineSnapshot` now includes optional `peer` state
  - the join path queries the live comms runtime via the trait seam rather than
    reaching into `meerkat-comms` directly

Why this slice matters:

- it is the first honest peer-ingress MeerkatMachine slice: queue truth now has
  a concrete owner that can be observed without draining it
- it avoids downcasting or concrete-crate reach-through by extending the core
  trait contract instead
- it exposes an important hidden semantic that the old channel-based shape had
  been carrying implicitly: classified ingress also needs explicit closed-state
  semantics once queue ownership becomes real

Verification:

- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`

Result:

- passed

Backtracks encountered:

- the first design nearly exposed peer state by downcasting concrete
  `meerkat_comms::CommsRuntime`; that would have violated the trait-owned
  architecture, so the snapshot became a hidden `meerkat-core` capability
- replacing the classified channel with an explicit queue removed the receiver's
  implicit closed semantics; an explicit `closed` flag had to be added to avoid
  regressing `send_classified()` behavior after `Inbox` drop
- this slice intentionally stopped at queued ingress state; it does not yet
  claim to model trusted-peer sets, peer-directory reachability, or the full
  `PeerCommsAuthority` surface

Current shape of the joined Meerkat snapshot:

- `spine`
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- `turn`
- `tools`
- `peer`
  - queued classified ingress only

Next likely slice:

- decide whether to grow peer ingress further through trusted-peer / routing
  truth, or switch to a different Meerkat region first if the next peer step
  would force too much semantic collapse
- if peer stays next, map the current `PeerCommsAuthority` fields against the
  new queue snapshot so we can see exactly what is still implicit versus now
  explicit

## 2026-04-09 — Slice 8: trust membership joins the peer snapshot

Goal:

- extend the new peer-ingress queue slice with the smallest additional facts
  that already have one clear owner in live code
- keep peer-directory reachability and transport health out of the kernel until
  we explicitly decide they belong there
- verify the joined Meerkat snapshot against a real per-session comms runtime

What landed:

- `meerkat-core/src/interaction.rs`
  - added `PeerIngressRuntimeSnapshot`
- `meerkat-core/src/agent.rs`
  - added hidden `CommsRuntime::peer_ingress_runtime_snapshot()` capability
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - implemented the runtime snapshot from:
    - local `public_key`
    - `require_peer_auth`
    - shared `trusted_peers`
    - existing classified queue snapshot
- `meerkat/src/meerkat_machine.rs`
  - `MeerkatMachineSnapshot.peer` now carries the runtime-level peer snapshot
    instead of queue data alone
  - added a positive integration test with a real runtime-backed session using
    `build.comms_name`
- updated Meerkat research docs to reflect:
  - `peer.self_peer_id`
  - `peer.auth_required`
  - `peer.trusted_peers`

Why this slice matters:

- it brings the first non-queue peer facts into MeerkatMachine using live owner
  state rather than projection folklore
- it proves the Meerkat facade can join runtime peer state through the existing
  `SessionService -> CommsRuntime` seam without reaching into concrete crates
- it draws a firmer line around what is still outside the kernel:
  peer-directory reachability and transport liveness remain perimeter concerns

Verification:

- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`

Result:

- passed

Backtracks encountered:

- the first version considered using `PeerDirectoryEntry` as the trust snapshot,
  but that would have silently imported reachability and directory synthesis
  into the Meerkat kernel. The final shape uses `TrustedPeerSpec` instead.
- this slice deliberately does not assert that queued peer work originates from
  trusted peers; plain-event ingress remains part of the same queue surface and
  must stay representable in the snapshot

Current Meerkat peer snapshot shape:

- `self_peer_id`
- `auth_required`
- `trusted_peers`
- `queue`
  - queued classified ingress only

Next likely slice:

- compare the new peer snapshot against `PeerCommsAuthority` and decide whether
  the next honest step is:
  - lifting more of raw-item lineage into an explicit owner snapshot, or
  - pausing peer work and moving to another Meerkat region before peer starts
    pulling too much transport semantics into the kernel

## 2026-04-09 — Slice 9: explicit completion-waiter carrier in the Meerkat spine

Goal:

- make the hidden completion-waiter carrier visible in the Meerkat runtime
  spine without promoting it into canonical machine truth
- verify tricky admission cases against the real runtime behavior:
  in-flight deduplication and hard runtime reset
- keep the joined facade snapshot aligned with the runtime-owned carrier

What landed:

- `meerkat-runtime/src/completion.rs`
  - added `CompletionRegistrySnapshot`
  - added `CompletionWaiterEntrySnapshot`
  - added `CompletionRegistry::diagnostic_snapshot()`
- `meerkat-runtime/src/meerkat_machine.rs`
  - added `MeerkatCompletionWaitersSnapshot`
  - added `MeerkatCompletionWaiterSnapshot`
  - `MeerkatMachineSpineSnapshot` now includes `completion_waiters`
- `meerkat-runtime/src/session_adapter.rs`
  - `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` now projects
    the live completion registry carrier
  - added tests covering:
    - idle runtime with no waiters
    - one queued prompt with one waiter
    - deduplicated in-flight admission joining a second waiter to one input id
    - `reset_runtime()` clearing the carrier and terminating the waiter
- `meerkat/src/meerkat_machine.rs`
  - joined facade snapshot test now asserts the projected waiter carrier
- `meerkat-owned-facts-ledger.md`
  - documented the completion registry snapshot explicitly as a supporting
    carrier rather than a new semantic owner

Why this slice matters:

- completion handles are not semantic runtime truth, but they sit on a
  historically bug-prone seam between admission, terminalization, and control
  actions like reset
- making the carrier explicit gives the Meerkat refactor a way to verify that
  this plumbing continues to track the real input lifecycle instead of becoming
  another folklore surface
- the tests deliberately exercise non-trivial behavior rather than just
  structural snapshot shape

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- passed

Backtracks encountered:

- the first test patch targeted the wrong context in `session_adapter.rs`; the
  fix was to patch smaller sections rather than forcing a broad edit
- this slice intentionally does not redefine completion waiters as a machine
  subregion. They remain supporting carrier state refined from canonical input
  terminalization

Next likely slice:

- choose the next Meerkat region that can be made explicit without inventing a
  new owner:
  - turn-side barrier and pending-op posture, if the current execution snapshot
    already exposes it honestly
  - or tool-surface mutation lineage, if that surface is currently better owned
    than the remaining peer authority path

## 2026-04-09 — Slice 10: external tool-surface lineage joins the Meerkat view

Goal:

- make the external tool-surface machine observable through the same Meerkat
  diagnostic path as runtime spine, turn, tools, and peer ingress
- keep the snapshot on the real dispatcher/router authority path instead of
  reaching around it with crate-specific downcasts
- verify the slice from producer authority through session, factory, and facade
  joins

What landed:

- new core diagnostic types in `meerkat-core/src/tool_scope.rs`:
  - `ExternalToolSurfaceGlobalPhase`
  - `ExternalToolSurfaceEntrySnapshot`
  - `ExternalToolSurfaceSnapshot`
- hidden dispatcher capability:
  - `AgentToolDispatcher::external_tool_surface_snapshot()`
- forwarding through the tool composition chain:
  - `FilteredToolDispatcher`
  - `ToolGateway`
  - `DynamicToolComposite`
  - `ToolDispatcher`
  - `FilteredDispatcher`
  - `CompositeDispatcher`
- live MCP authority snapshot:
  - `ExternalToolSurfaceAuthority::diagnostic_snapshot()`
  - `McpRouter::external_tool_surface_snapshot()`
  - `McpRouterAdapter` cache plus fallback support
- session, factory, and Meerkat joins:
  - `AgentRunner::external_tool_surface_snapshot()`
  - `SessionAgent::external_tool_surface_snapshot()`
  - session-service query path
  - `FactoryAgent` forwarding
  - `capture_meerkat_machine_snapshot(...)` now includes `tool_surface`

Why this slice matters:

- it pulls one of the higher-risk Meerkat mutation domains into the joined
  machine view without inventing a second owner
- it verifies that the production builder path preserves the same diagnostic
  truth as the lower-level session and MCP paths
- it makes external tool mutation lineage and snapshot-alignment state visible
  for later Meerkat kernel work and TLA mapping

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mcp --lib`
- `cargo test -p meerkat-session --test regression_lifecycle`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat --lib --features mcp`

Result:

- passed

Backtracks encountered:

- the first session-level expectation used `snapshot_epoch = 1` for a purely
  staged `StageAdd` surface; that was wrong against the current authority and
  had to be corrected back to `0`
- the first Meerkat integration idea tried to place the join inside
  `meerkat-runtime`; that would have recreated a false owner for tool-surface
  truth, so the final join stays in the facade path
- the MCP-backed Meerkat test had to be `mcp`-gated; the non-`mcp` build still
  needs to stay green because the abstract Meerkat surface exists even when no
  live MCP subsystem is compiled in

Current limitation:

- `ToolGateway::external_tool_surface_snapshot()` returns the first dispatcher
  that exposes surface state. That matches the current single live external
  surface authority assumption, but it would need redesign if Meerkat ever
  hosts multiple independent external-tool lifecycle authorities at once

Next likely step:

- return to the remaining Meerkat peer-ingress gap and decide whether the live
  peer authority can be made as explicit as the tool-surface authority
- if peer remains too entangled with transient transport buffers, pivot to a
  different Meerkat region before attempting more peer collapse

## 2026-04-09 — Slice 11: peer ingress gains stable raw identity and stored correlation

Goal:

- tighten the peer-ingress slice without forcing `PeerCommsAuthority` onto the
  live hot path before the owner mapping is honest
- carry stable ingress-time identity and correlation through the queued peer
  snapshot instead of dropping them at the queue boundary
- align ingress-prepared text projection with the live drain path so stored
  projection can become real runtime data rather than test-only scaffolding

What landed:

- `meerkat-comms/src/lib.rs`
  - `peer_comms_authority` is now explicitly compiled into the crate instead of
    existing as effectively dormant code
- `meerkat-comms/src/classify.rs`
  - `PreparedIngressItem` now provides the coarse `PeerIngressKind` used by the
    queued snapshot
  - message ingress projection now preserves the message body as the canonical
    prompt projection, even when multimodal blocks are present
  - legacy `classify()` scaffolding is test-only; live code now goes through
    `prepare(...)`
- `meerkat-comms/src/inbox.rs`
  - classified queue entries now retain:
    - `raw_item_id`
    - `kind`
    - `request_id`
    - stored `text_projection`
  - non-destructive peer queue snapshots now expose:
    - stable raw ingress identity
    - stored request/reply correlation
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - classified drain now uses the ingress-stored text projection instead of
    recomputing request/response/plain-event prompt text at drain time
  - added assertions that snapshot raw ids stay stable for plain events
- `meerkat-core/src/interaction.rs`
  - `PeerIngressEntrySnapshot` now includes `raw_item_id` and `request_id`

Why this slice matters:

- it makes the peer-ingress queue a better match for the Meerkat owned-facts
  ledger: stable raw item identity and request correlation now survive the
  queue boundary instead of being implicit in lower layers
- it strengthens the eventual authority handoff shape without pretending the
  authority already executes the live transitions
- it reduces one specific class of drift: ingress-classified request/response
  text no longer has to be recomputed later from the raw envelope

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo check -p meerkat-comms --lib`

Result:

- passed

Backtracks encountered:

- the first version changed multimodal message projection by blindly using the
  generic `CommsMessage::to_user_message_text()` result at drain time; that
  regressed the current runtime contract, which treats the message body as the
  canonical prompt projection for peer messages with blocks
- the fix was to move the semantic choice earlier: ingress preparation now
  computes message projection in the same shape the live drain already expects,
  and only then does the drain path consume the stored projection
- the first `InboxSender::send_classified()` patch also borrowed the prepared
  ingress descriptor after partially moving it into the queue entry; the fix
  was to compute the coarse ingress kind before consuming the prepared item

Current limitation:

- `PeerCommsAuthority` is now compiled, aligned, and tested, but it is still
  not the live owner of peer ingress transitions
- the queued peer snapshot now carries more of the right facts, but the hot
  path still updates the classified queue directly rather than driving the
  authority state first and projecting from it

Next likely step:

- decide whether the next honest peer slice is:
  - routing live queue admission through `PeerCommsAuthority`, or
  - stopping peer again and filling another Meerkat region before the peer
    integration step becomes too invasive for one slice

## 2026-04-09 — Slice 12: peer authority becomes reusable across session lifetime

Goal:

- critically review whether `PeerCommsAuthority` is actually fit to become a
  live Meerkat owner instead of assuming the existing model is already sound
- fix the authority's phase semantics so a long-lived session can accept more
  than one batch of peer work
- keep the fix narrow: improve the machine model first, without pretending the
  live runtime has already delegated ingress to it

What landed:

- `meerkat-comms/src/peer_comms_authority.rs`
  - `TrustPeer` now acts as a self-loop from any phase instead of resetting the
    machine to `Absent`
  - `ReceivePeerEnvelope` now accepts new ingress from `Dropped` and
    `Delivered`, so the authority can model repeated batches inside one live
    session
  - untrusted ingress no longer poisons a non-empty queue: if queued trusted
    work already exists, the machine stays in `Received` while still recording
    `trusted_snapshot = false` for the dropped raw item
  - added new tests covering:
    - trust mutation from `Dropped` and `Delivered`
    - resuming from `Dropped`
    - starting a new batch from `Delivered`
    - untrusted ingress while trusted queued work is still pending
    - accepting future work after a fully delivered batch

Why this slice matters:

- the earlier authority model was effectively single-use; once it reached
  `Delivered` or `Dropped`, it could not model further ingress for the same
  runtime
- that made it unfit as a serious candidate for the live Meerkat owner, even
  before any wiring concerns
- fixing the model first is the right backtrack: it lets the runtime
  integration step be about ownership and mapping, not about patching a machine
  that fundamentally could not represent a long-lived session

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib peer_comms_authority`
- `cargo test -p meerkat-comms --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- passed

Backtracks encountered:

- the original review target was live peer authority integration, but the code
  inspection showed that would have been premature because the authority could
  not represent a steady-state runtime after `Delivered` or `Dropped`
- this slice therefore intentionally stopped at the model level; it does not
  yet route live `send_classified()` or drain through the authority

Current limitation:

- `PeerCommsAuthority` is now session-reusable, but live trust membership and
  live queue ownership are still maintained outside it
- the next integration step must decide how trust changes and queued work stay
  synchronized without inventing a shadow authority

Next likely step:

- either add the smallest honest live synchronization path between trusted-peer
  mutations, ingress preparation, and `PeerCommsAuthority`
- or pause peer again and take another Meerkat region if that synchronization
  step is still too broad for one slice

## 2026-04-09 — Slice 13: peer authority learns auth-open admission and trust removal

Goal:

- align `PeerCommsAuthority` with real runtime comms behavior before giving it
  any more live ownership
- make the machine capable of expressing both supported peer-admission modes:
  - `require_peer_auth = true`
  - `require_peer_auth = false`
- add the missing trust-removal semantic so future live synchronization does
  not need to smuggle revocation around the authority

What landed:

- `meerkat-comms/src/peer_comms_authority.rs`
  - added machine-owned `auth_required`
  - added `PeerCommsAuthority::new_with_auth_required(...)`
  - added `RemoveTrustedPeer`
  - `ReceivePeerEnvelope` now admits untrusted peers when auth is open, while
    still recording `trusted_snapshot = false`
  - trust removal now revokes future trusted admission without disturbing
    already queued work
- `meerkat-comms/src/classify.rs`
  - added a round-trip test proving that auth-open runtime ingress preparation
    and auth-open authority admission agree on an unknown peer request

Why this slice matters:

- before this change, the authority could only model the strict-auth runtime.
  That meant it could not become a real Meerkat owner for sessions configured
  with `require_peer_auth = false`
- this is exactly the kind of mismatch that creates shadow logic: the live
  runtime would have to special-case admission outside the authority
- fixing the model first keeps the next live synchronization step honest

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib peer_comms_authority`
- `cargo test -p meerkat-comms --lib classify`
- `cargo test -p meerkat-comms --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- passed

Backtracks encountered:

- the original intent for this turn was to start live synchronization between
  trust state, queue state, and the authority
- code inspection showed that would still be premature because the authority
  could not express auth-open runtime semantics or trust revocation
- this slice therefore stayed at the model boundary on purpose

Current limitation:

- the authority now matches more of the live runtime semantics, but the runtime
  still owns trust mutation and classified queue state separately
- plain external events remain outside the authority model
- the Meerkat joined snapshot scaffolding still emits dead-code warnings in
  `meerkat/src/meerkat_machine.rs` because it is diagnostic infrastructure, not
  production wiring yet

Next likely step:

- wire one minimal live synchronization path for trusted-peer mutation into the
  peer authority, or
- step sideways and fill another Meerkat region if peer integration still
  needs one more model cleanup pass first

## 2026-04-09 — Slice 14: live peer trust and external ingress synchronize through one runtime seam

Goal:

- stop treating router-local trust mutation as if it were canonical Meerkat
  peer state
- wire one minimal live synchronization path between:
  - trusted-peer mutation
  - external peer envelope receive
  - typed peer dequeue/drain
  - `PeerCommsAuthority`
- keep the slice narrow enough that plain events and unrelated comms transport
  details stay outside the authority for now

What landed:

- `meerkat-comms/src/classify.rs`
  - `prepare(...)` now always prepares external peer envelopes and records
    `trusted_sender`, instead of dropping strict-auth unknown peers before the
    authority ever sees them
  - test-only `classify(...)` still preserves the old strict-auth drop
    semantics for focused classification tests
- `meerkat-comms/src/inbox.rs`
  - classified queue receive now asks `PeerCommsAuthority` whether an external
    envelope should be admitted
  - dequeue/drain now advances authority submission state in lockstep with the
    queue
  - trusted-peer add/remove notifications now synchronize into the
    inbox-owned authority
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - internal runtime tests now use the async
    `meerkat_core::agent::CommsRuntime::add_trusted_peer` seam instead of
    mutating `router.add_trusted_peer(...)` directly
  - added live tests for:
    - trust -> receive -> drain on auth-required runtimes
    - unknown-peer receive -> drain on auth-open runtimes
  - `upsert_trusted_peer(...)` is now documented as a legacy router-only helper,
    not an authoritative runtime seam
- `meerkat-comms/src/runtime/comms_bootstrap.rs`
  - child inproc bootstrap now uses the async trusted-peer registration seam
    rather than the router-only helper
- `meerkat/tests/factory_build_agent.rs`
  - shared-runtime sentinel trust setup now uses the same async registration
    seam as production runtime code

Why this slice matters:

- before this change, the peer authority could look correct in isolation while
  live runtime code still mutated trust through a second path
- that is exactly the kind of “owner plus bypass” shape the two-kernel work is
  trying to eliminate
- moving strict-auth unknown-peer drop into the authority path also fixed a
  deeper mismatch: the authority was previously unable to observe some real
  ingress attempts at all

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --test factory_build_agent shared_comms_runtime_skipped_when_comms_name_set --features comms`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first live runtime test failed because strict-auth unknown peers were
  still being dropped in classification before the authority saw them
- fixing that exposed a second bypass: several existing tests still mutated
  trust through `router.add_trusted_peer(...)`
- rather than teaching the authority about those bypasses, the fix was to move
  internal callers onto the async runtime registration seam

Current limitation:

- `upsert_trusted_peer(...)` still exists for legacy callers that only need
  router-visible trust and do not participate in classified ingress
- plain external events still bypass `PeerCommsAuthority`
- the Meerkat joined snapshot scaffolding in `meerkat/src/meerkat_machine.rs`
  still emits dead-code warnings because it is diagnostic infrastructure, not
  production wiring yet

Next likely step:

- decide whether the next honest Meerkat slice is:
  - promoting the async trusted-peer registration seam as the only supported
    runtime mutation path, or
  - stepping sideways into another Meerkat region before returning to peer
    ingress again

## 2026-04-09 — Slice 15: canonical runtime trust registration becomes a named Meerkat seam

Goal:

- stop making canonical peer-trust mutation depend on trait-call syntax or
  router writes
- give `MeerkatMachine` an explicit runtime-level trust registration seam that
  keeps peer discovery, peer authority, and classified ingress aligned
- move the real runtime-facing consumers onto that named seam

What landed:

- `meerkat-comms/src/runtime/comms_runtime.rs`
  - added `CommsRuntime::register_trusted_peer(...)`
  - added `CommsRuntime::unregister_trusted_peer(...)`
  - added `CommsRuntime::unregister_trusted_pubkey(...)`
  - the trait-level `add_trusted_peer` / `remove_trusted_peer` path now
    delegates to the same runtime implementation
  - added a focused test proving that `register_trusted_peer(...)`:
    - preserves `PeerMeta`
    - synchronizes inbox-owned peer authority
    - removes cleanly through the paired unregister seam
- `meerkat-comms/src/runtime/comms_bootstrap.rs`
  - child inproc bootstrap now uses the named runtime registration method
- `meerkat/tests/factory_build_agent.rs`
  - shared-runtime sentinel trust setup now uses the same named runtime seam
- `examples/035-mdm-tux-rs/src/bin/target.rs`
  - target runtime add/remove trust paths now use canonical runtime methods
    instead of `upsert_trusted_peer(...)` and raw router removal

Why this slice matters:

- the previous slice established that raw router mutation was a bypass
- this slice turns the fix into a stable API surface instead of leaving the
  “right path” implicit in trait calls
- it also closes the remove-path drift, not just the add-path drift

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --test factory_build_agent shared_comms_runtime_skipped_when_comms_name_set --features comms`
- `cargo check --manifest-path examples/035-mdm-tux-rs/Cargo.toml --bin mdm-target`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first peer-authority fix still left runtime callers choosing between the
  trait seam, the sync helper, and raw router mutation
- that was too easy to misuse, so this slice added explicit runtime methods and
  migrated the meaningful callers onto them

Current limitation:

- `upsert_trusted_peer(...)` still exists as a legacy router-only helper
- standalone example code in `examples/035-mdm-tux-rs/src/lib.rs` and
  `examples/035-mdm-tux-rs/src/bin/tux.rs` still mutates raw router state
  because it is not using `CommsRuntime`'s classified-ingress ownership path
- Meerkat snapshot scaffolding in `meerkat/src/meerkat_machine.rs` still emits
  dead-code warnings because it is diagnostic infrastructure, not production
  wiring yet

Next likely step:

- decide whether to retire or hard-deprecate `upsert_trusted_peer(...)`, or
- step sideways into the next Meerkat region instead of continuing to deepen
  peer ingress in one uninterrupted run

## 2026-04-09 — Slice 16: peer authority phase enters the live Meerkat snapshot

Goal:

- expose one more honest owner-side fact from the peer ingress region
- distinguish:
  - queued classified work
  - authority-owned peer submission backlog
  - dropped / delivered / absent authority states
- verify that the joined `MeerkatMachine` view can now see this difference
  without inventing a new injection seam

What landed:

- `meerkat-core/src/interaction.rs`
  - added `PeerIngressAuthorityPhase`
  - `PeerIngressRuntimeSnapshot` now carries:
    - `authority_phase`
    - `submission_queue_len`
- `meerkat-comms/src/inbox.rs`
  - added a non-destructive runtime peer snapshot path that captures:
    - classified queue projection
    - authority phase
    - authority submission queue length
  - this happens under one lock so the snapshot is not stitched from two
    different moments
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - `peer_ingress_runtime_snapshot()` now maps internal
    `PeerIngressState -> PeerIngressAuthorityPhase`
  - added focused tests proving:
    - plain events can leave `authority_phase = Absent` while the classified
      queue is non-empty
    - dropped peer ingress is visible through the runtime snapshot even when
      the queue remains empty
- `meerkat/src/meerkat_machine.rs`
  - joined Meerkat snapshot test now verifies that live peer runtime state
    carries the new authority-phase facts across the cross-crate join

Why this slice matters:

- the previous snapshot could show queue contents but could not explain whether
  the peer owner considered ingress absent, dropped, received, or delivered
- that meant one of the actual machine states was still hidden behind helper
  tests instead of being visible through the real runtime seam
- `submission_queue_len` also makes it explicit that the authority-owned peer
  backlog is not the same thing as the broader classified queue

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib test_peer_ingress_runtime_snapshot_reflects_trusted_peers_and_queue`
- `cargo test -p meerkat-comms --lib test_peer_ingress_runtime_snapshot_reflects_dropped_peer_authority_state`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_live_peer_runtime_state --features comms`
- `cargo check -p meerkat --lib --features comms`

Result:

- passed

Backtracks encountered:

- the first joined Meerkat test idea tried to manufacture a dropped peer
  ingress state through the session-level comms trait object
- that would have required a new helper seam or a downcast path, which was too
  much surface for this slice
- the fix was to keep dropped-state verification at the comms runtime owner
  level and only assert phase propagation in the joined Meerkat snapshot

Current limitation:

- plain events still bypass `PeerCommsAuthority`, so
  `authority_phase = Absent` with a non-empty classified queue remains a real
  and expected state
- the joined Meerkat snapshot still emits dead-code warnings because it is
  diagnostic infrastructure, not production wiring yet

Next likely step:

- either surface another owner-side peer fact, such as trusted/drop lineage for
  the last classified raw item, or
- step sideways into a different Meerkat region before continuing to deepen the
  peer ingress surface

## 2026-04-09 — Slice 17: executable joined-snapshot invariants

Goal:

- stop treating the joined `MeerkatMachine` snapshot as pure observation
- turn the first safe cross-region invariants into executable validation
- verify those invariants against the live runtime, turn, tool, and peer joins
  before adding more surface area

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added `MeerkatMachineInvariantViolation`
  - added `validate_meerkat_machine_snapshot(...)`
  - the first validator pass checks:
    - `Running => current_run_id # None`
    - `current_run_id # None => control.phase \in {Running, Retired}`
    - control/input current-run alignment
    - turn/control run alignment for non-ready, non-terminal turn phases
    - `WaitingForOps => pending_operation_ids # None`
    - peer authority phase vs submission queue coherence
    - tool-surface visibility/base-state coherence
    - `active_ops <= total_ops`
  - existing joined snapshot tests now assert that live captured snapshots pass
    validation
  - added a focused negative test that mutates a captured live snapshot and
    proves the validator reports the expected cross-region violations
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - updated the candidate invariants to match the real control authority:
    `Retired` may legitimately retain `current_run_id` during drain
  - recorded which invariants are now executable in code

Why this slice matters:

- it changes the Meerkat work from "we can see more state" to
  "we can check whether the joined state is coherent"
- this is the first place where the research docs now constrain the live code
  bidirectionally: the docs propose invariants, and the code proves or rejects
  them
- it also flushed out one important false assumption early: `Retired` is not a
  no-run state if a drain is still completing an in-flight run

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first validator draft used `control.phase = Running <=> current_run_id # None`
- live authority review showed that is wrong: `Retired` can retain
  `current_run_id` while a run drains after `RetireRequested`
- the validator and sketch were tightened to reflect the actual reducer rather
  than an over-simplified architecture story

Current limitation:

- the validator is still intentionally conservative; it does not yet check:
  - barrier satisfaction against concrete ops terminality
  - completion-waiter alignment against input terminalization
  - recovery-specific binding/cursor invariants
- it remains diagnostic infrastructure in `meerkat/src/meerkat_machine.rs`, so
  the joined snapshot module still carries dead-code warnings in non-test lib
  builds

Next likely step:

- deepen the validator with one more region that already has clear live owner
  truth, probably tool-surface visibility/base-state coherence or
  barrier/ops coupling, before opening `MobMachine`

## 2026-04-09 — Slice 18: runtime-spine input-ledger coherence

Goal:

- deepen the executable Meerkat validator without depending on a non-atomic
  cross-crate join
- validate the runtime-spine input ledger against queue membership, current-run
  contributors, and completion-waiter references
- keep the slice anchored in one authority-owned snapshot so failures represent
  real Meerkat mismatches instead of join timing noise

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added validator checks that:
    - every `queue` and `steer_queue` item exists in the admitted-input ledger
    - queued items have lifecycle `Queued`
    - every `current_run_contributors` item exists in the admitted-input ledger
    - contributor lifecycles stay within
      `{Staged, Applied, AppliedPendingConsumption}`
    - completion-waiter `input_count` matches `waiting_inputs.len()`
    - completion-waiter `waiter_count` matches the summed entry counts
    - every waiting completion input still exists in the admitted-input ledger
  - expanded the synthetic negative test so it now proves these runtime-spine
    violations are detected alongside the earlier cross-region ones
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded the new executable input-ledger invariants

Why this slice matters:

- it moves more of `MeerkatMachine` validation into the runtime spine, where
  the snapshot is substantially more coherent than the cross-crate turn/runtime
  join
- it verifies the current admitted-input ledger is strong enough to act as the
  anchor for queue truth, contributor truth, and waiter plumbing references
- it keeps the completion carrier explicit without promoting waiter counts into
  semantic truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first candidate follow-up was `turn.barrier_satisfied => AllTerminal(...)`
  against the ops snapshot
- that looks right in the abstract sketch, but the current joined snapshot is
  not atomic across crates: runtime ops are read before the later turn
  snapshot, so a legitimate barrier-satisfaction transition can race and
  manufacture a false mismatch
- the fix was to pivot to runtime-spine invariants that live inside one
  authority-owned snapshot and postpone barrier/ops validation until the join
  is tighter or the validator can account for the sampling boundary explicitly

Current limitation:

- barrier/ops coupling is still documented but not executable in the joined
  validator yet, for the non-atomic snapshot reason above
- completion waiters are only validated structurally; the validator still does
  not assert terminal/non-terminal lifecycle alignment because the waiter and
  ingress snapshots are captured under different locks

Next likely step:

- either tighten the joined Meerkat snapshot semantics enough to support safe
  barrier/ops validation, or keep extending the runtime-spine validator with
  another same-owner invariant before moving to `MobMachine`

## 2026-04-09 — Slice 19: ingress lifecycle and cohort coherence

Goal:

- keep strengthening `MeerkatMachine` inside the runtime-owned ingress spine
- validate that current-run/cohort shape and terminal-outcome shape remain
  coherent with the live ingress authority
- avoid widening the validator into a cross-crate race when the runtime spine
  still offers cleaner truth

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added validator checks that:
    - `inputs.current_run_id = None <=> current_run_contributors = << >>`
    - every terminal admitted input carries a terminal outcome
    - non-terminal admitted inputs do not carry terminal outcomes
  - added a focused negative test proving:
    - contributors without a current run are rejected
    - illegal contributor lifecycle is rejected
    - terminal lifecycle without terminal outcome is rejected
    - non-terminal lifecycle with terminal outcome is rejected
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded the new executable ingress-lifecycle invariants

Why this slice matters:

- it tightens the admitted-input ledger into a more trustworthy Meerkat anchor
- it proves that the current ingress authority already carries enough shape to
  validate both cohort structure and terminal bookkeeping without touching
  higher-level surfaces
- it keeps the work inside one owner domain, which is exactly the discipline we
  want before we start opening `MobMachine`

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_runtime_spine_lifecycle_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- I considered jumping from here into deeper tool-surface invariants or back to
  barrier/ops coupling
- the runtime ingress authority offered a cleaner next truth surface, so I kept
  the slice there instead of mixing in another cross-boundary concern too soon

Current limitation:

- `current_run_id Some => contributors non-empty` is now executable, but the
  validator still does not reason about the *contents* of `current_run_id`
  beyond matching control and ingress
- barrier/ops coupling remains documented but intentionally non-executable in
  the joined validator until the snapshot semantics are tighter

Next likely step:

- extend the tool-surface portion of the validator with one more same-owner MCP
  invariant, or
- start designing a tighter joined-snapshot coherence boundary if barrier/ops
  validation is the higher priority before `MobMachine`

## 2026-04-09 — Slice 20: richer tool-surface authority validation

Goal:

- expand the `MeerkatMachine` validator using same-owner MCP authority facts
- validate more of the external tool-surface machine than just
  visible/base-state membership
- keep the slice fully within the tool-surface snapshot so it remains free of
  cross-crate join races

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added validator checks that:
    - pending op is compatible with base state
    - inflight call counts only exist on `Active | Removing`
    - forced delta phase implies remove delta operation
    - staged intent sequence exists iff staged op is not `None`
    - pending task/lineage sequences exist iff pending op is not `None`
  - added a focused negative test proving each of those richer tool-surface
    invariants is detected
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded the richer tool-surface invariants in both the candidate list and
    the executable-subset list

Why this slice matters:

- it turns the tool-surface region into a substantially more trustworthy part
  of the Meerkat validator, not just a visibility check
- it is pulled directly from the live MCP authority’s own invariant checks, so
  we are validating real codebase truth rather than inventing a new layer
- it strengthens `MeerkatMachine` without widening the still-risky
  runtime/turn/ops sampling boundary

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- none in code, which is a useful signal by itself: the richer invariants held
  cleanly against both synthetic failures and the live staged-MCP snapshot
- that reinforces the choice to deepen same-owner regions before returning to
  more ambiguous cross-region joins

Current limitation:

- tool-surface validation is now strong, but the validator still does not
  account for removal-timing membership because that fact is not exposed
  through the current core snapshot
- barrier/ops coupling remains the main intentionally deferred Meerkat
  invariant family

Next likely step:

- either expose one more MCP-authority fact such as removal-timing membership
  through the snapshot, or
- stop deepening Meerkat’s same-owner regions and begin sketching the first
  narrow `MobMachine` slice once the remaining Meerkat risks are judged low
  enough

## 2026-04-09 — Slice 21: tool-surface removal-timing validation

Goal:

- expose the last obvious same-owner MCP authority fact still missing from the
  joined Meerkat snapshot
- validate that removal timing exists iff a tool surface is in `Removing`
  base state
- keep the slice fully inside the external tool-surface snapshot so it remains
  free of runtime/turn sampling races

What landed:

- `meerkat-core/src/tool_scope.rs`
  - added `has_removal_timing` to
    `ExternalToolSurfaceEntrySnapshot`
- `meerkat-mcp/src/external_tool_surface_authority.rs`
  - threaded live removal-timing membership into the diagnostic snapshot from
    the canonical authority state
- `meerkat/src/meerkat_machine.rs`
  - added `ToolSurfaceRemovalTimingMismatch`
  - validated `has_removal_timing <=> base_state = Removing`
  - expanded the focused negative tool-surface test with a
    `Removing`-without-timing failure case
- `meerkat-session/tests/regression_lifecycle.rs`
  - updated the session-side expected snapshot shape for the added field
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded removal-timing membership in both the candidate invariant list
    and the currently executable subset

Why this slice matters:

- it closes the last clear MCP same-owner gap in the current joined
  `MeerkatMachine` validator
- it reuses a fact the live MCP authority already enforces internally, so the
  validator remains grounded in real ownership rather than a parallel model
- it raises confidence that the external tool-surface region is genuinely
  ready, allowing the next decision to be about remaining Meerkat cross-region
  joins versus starting the first narrow `MobMachine` slice

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat-session --test regression_lifecycle tool_scope_snapshot_returns_live_agent_tool_scope_state`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first session test filter was wrong (`tool_surface_...` instead of the
  existing `tool_scope_...` test name); I corrected the verification lane
  rather than inventing a redundant test
- the slice otherwise held cleanly without code backtracks, which is useful
  signal that this was the right final same-owner MCP invariant to expose

Current limitation:

- the joined `MeerkatMachine` snapshot is still a diagnostic surface, not a
  production-owned API, so the existing dead-code warnings remain expected
- barrier/ops coupling remains intentionally deferred until the runtime/turn
  join semantics are tighter

Next likely step:

- start a first narrow `MobMachine` slice while keeping the deferred
  barrier/ops join explicitly tracked as remaining Meerkat work, or
- if Meerkat stays first for one more slice, focus on tightening runtime/turn
  snapshot coherence rather than adding more same-owner regions

## 2026-04-09 — Slice 22: first MobMachine roster/lifecycle snapshot

Goal:

- open the first real `MobMachine` implementation slice without jumping
  straight into flow/orchestrator complexity
- map live `MobHandle` read paths into one diagnostic snapshot
- validate only roster/lifecycle/member-projection invariants already justified
  by current code

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `MobMachineSnapshot { phase, roster, members }`
  - added `capture_mob_machine_snapshot(&MobHandle)`
  - added a conservative validator for:
    - roster member set vs projected member set
    - structural projection coherence between `RosterEntry` and
      `MobMemberListEntry`
    - `Active` vs `Retiring` status/state coherence
    - `Broken` requiring an error payload
    - member finality matching the current status classification
  - added a synthetic negative validator test
- `meerkat-mob/src/runtime/tests.rs`
  - added a live-handle test that spawns and wires members, captures the new
    snapshot from `MobHandle`, and proves the validator stays clean on the
    current runtime path
- `meerkat-mob/src/lib.rs`
  - wired in the new internal module

Why this slice matters:

- it starts `MobMachine` the same way `MeerkatMachine` started: by exposing a
  diagnostic read surface over real owners instead of inventing a second
  control plane
- it keeps the first Mob scope small enough to review honestly:
  lifecycle state, roster truth, and enriched member status projection
- it creates a concrete integration point for later identity-native migration
  work without pretending the current `MeerkatId`-centric API is already the
  target design

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first synthetic test tried to create a one-way wiring inconsistency
  through the public `Roster` API; that failed because the current projection
  mutators already preserve reciprocity
- that is good architectural signal: some topology invariants are protected
  before the new validator ever runs, so the validator should observe and
  corroborate those guarantees rather than bypassing them for the sake of a
  test fixture

Current limitation:

- the current `MobMachine` slice is still roster/lifecycle only
- flow runs, orchestrator state, kickoff barriers, pending spawns, and
  topology-intent vs realized-wiring gaps are not yet part of the diagnostic
  snapshot
- like `MeerkatMachine`, the new module is still diagnostic scaffolding, so
  the current dead-code warnings are expected

Next likely step:

- extend `MobMachine` with one more owner-backed region, most likely
  lifecycle/orchestrator counters or topology intent, while keeping flow
  execution out until the read surface is honest enough

## 2026-04-09 — Slice 23: Mob machine-state diagnostics

Goal:

- extend `MobMachine` beyond roster/member projection into actual
  machine-owned lifecycle and orchestrator state
- do that through real actor-backed diagnostic reads rather than by re-deriving
  counters from public surfaces
- keep the invariant set conservative and tied to authority semantics already
  encoded in `MobLifecycleAuthority` and `MobOrchestratorAuthority`

What landed:

- `meerkat-mob/src/runtime/mob_lifecycle_authority.rs`
  - added `MobLifecycleSnapshot { phase, active_run_count, cleanup_pending }`
  - added `snapshot()` for diagnostics while preserving canonical ownership
- `meerkat-mob/src/runtime/mob_orchestrator_authority.rs`
  - extended `MobOrchestratorSnapshot` to include `phase`
- `meerkat-mob/src/runtime/state.rs`
  - added a non-test `DiagnosticKernelSnapshot` command
- `meerkat-mob/src/runtime/actor.rs`
  - wired `DiagnosticKernelSnapshot` to return the live lifecycle snapshot and
    optional orchestrator snapshot from the actor-owned authorities
- `meerkat-mob/src/runtime/handle.rs`
  - added internal `diagnostic_kernel_snapshot()`
- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineSnapshot` with `kernel`
  - validated:
    - public phase matches lifecycle phase
    - terminal lifecycle phases have zero active runs
    - cleanup pending appears only in `Stopped | Completed`
    - public phase matches orchestrator phase when orchestrator exists
    - terminal orchestrator phases have zero pending spawns and active flows
    - destroyed orchestrator snapshots are no longer bound or supervisor-active
  - expanded the synthetic negative test with machine-state violations

Why this slice matters:

- it upgrades `MobMachine` from “roster plus projection” to “roster plus real
  canonical machine state”
- it creates a proper bridge between the public mob handle and the actual
  lifecycle/orchestrator authorities without making those authorities public
  control surfaces
- it gives later flow/topology work a grounded place to attach instead of
  forcing everything through `list_members*()` projections

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo test -p meerkat-mob --lib test_stop_clears_pending_spawn_count_and_failed_member_projection`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_pending_spawn_ownership_and_revision`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first compile pass broke existing lifecycle tests by removing the
  `active_run_count()` helper too aggressively; I restored the test-only helper
  instead of rewriting authority tests mid-slice
- adding `phase` to `MobOrchestratorSnapshot` also needed a manual `Default`
  impl because `MobState` does not implement `Default`
- the suspected fresh-create lifecycle/orchestrator/public-phase mismatch did
  not reproduce in the live handle capture lane, which is useful signal that
  the real startup path is currently coherent enough for this validator level

Current limitation:

- `MobMachine` still does not model flow kernel state, kickoff barriers,
  pending-spawn lineage contents, or topology intent vs realized wiring
- the new lifecycle/orchestrator snapshots are still diagnostic scaffolding, so
  their current dead-code warnings are expected just like the Meerkat side

Next likely step:

- extend `MobMachine` with one more honest owner-backed region:
  either topology intent/revision coherence or a narrow flow/orchestrator join
  around active-flow accounting

## Slice 24 - MobMachine topology coherence

Goal:

- add the first topology-owned lane to `MobMachine`
- verify that the live topology owner and the orchestrator authority are not
  silently drifting on spawn and lifecycle transitions
- fix the owner path if the new diagnostic join exposes a real mismatch

What landed:

- `meerkat-mob/src/runtime/topology.rs`
  - added `MobTopologySnapshot { coordinator_bound, revision }`
- `meerkat-mob/src/runtime/flow.rs`
  - exposed topology diagnostics and small owner-preserving topology mutation
    helpers on `FlowEngine`
- `meerkat-mob/src/runtime/mod.rs`
  - extended `MobKernelDiagnosticSnapshot` with topology state
- `meerkat-mob/src/runtime/actor.rs`
  - wired topology snapshot into `DiagnosticKernelSnapshot`
  - synchronized topology revision/binding updates with the same live
    orchestrator-driven transitions that already mutate:
    - `StageSpawn`
    - `CompleteSpawn`
    - `StopOrchestrator`
    - `ResumeOrchestrator`
  - mirrored the same unbind/rebind behavior in destroy/reset paths
- `meerkat-mob/src/mob_machine.rs`
  - added topology-aware validation:
    - topology `coordinator_bound` matches orchestrator `coordinator_bound`
    - topology `revision` matches orchestrator `topology_revision`
  - expanded the synthetic negative test to prove both mismatches are caught
- `meerkat-mob/src/runtime/tests.rs`
  - added a live test that captures `MobMachine` snapshots through:
    - initial running mob
    - staged spawn
    - settled spawn
    - stop
    - resume
  - each snapshot must satisfy the topology-aware validator

Why this slice matters:

- it exposed a real architectural pressure point rather than just another read
  model: topology revision already had a live owner (`MobTopologyService`), but
  the orchestrator authority was independently carrying its own revision counter
- instead of picking one by fiat mid-slice, the new diagnostic join makes the
  duplication visible and forces the two paths to stay coherent while the
  longer-term two-kernel collapse continues
- it also gives `MobMachine` its first owner-backed region beyond
  lifecycle/orchestrator/roster that is not just a projection of member status

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_pending_spawn_ownership_and_revision`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first review pass found a real ownership smell: `MobTopologyService`
  existed, but the staged/settled spawn path and stop/resume path were only
  mutating the orchestrator authority, not the topology owner
- I did not paper over that by weakening the validator; I added the topology
  snapshot first, then wired the missing owner-side updates onto the real actor
  transitions so the live test could verify them
- the only non-semantic cleanup after verification was lowering
  `MobTopologyService::snapshot()` to crate visibility so the diagnostic-only
  snapshot type did not leak a `private_interfaces` warning

Current limitation:

- topology intent/revision is now joined, but `MobMachine` still does not model
  flow kernel state itself, kickoff barriers, or the detailed contents of
  pending-spawn lineage
- the topology/orchestrator pairing is still two-owner reality kept coherent by
  live wiring; it is not yet the final collapsed kernel shape

Next likely step:

- extend `MobMachine` with one more owner-backed slice around active-flow
  accounting or flow/orchestrator coherence before touching the larger flow
  kernel surface

## Slice 25 - MobMachine flow-accounting coherence

Goal:

- join actor-owned flow tracker state into the diagnostic `MobMachine` view
- verify that three live carriers are actually aligned at stable actor
  boundaries:
  - `MobLifecycle.active_run_count`
  - `MobOrchestrator.active_flow_count`
  - actor-owned `run_tasks` / `run_cancel_tokens` tracker maps

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - added `MobFlowTrackerSnapshot { run_task_ids, cancel_token_ids, stream_ids }`
  - extended `MobKernelDiagnosticSnapshot` with `flow_trackers`
- `meerkat-mob/src/runtime/actor.rs`
  - added `flow_tracker_snapshot()`
  - changed `DiagnosticKernelSnapshot` to include the live flow tracker snapshot
  - reused the same snapshot for the existing test-only `FlowTrackerCounts`
    command so the diagnostic path and the older debug path stop drifting
- `meerkat-mob/src/mob_machine.rs`
  - added flow-aware validation:
    - lifecycle `active_run_count` must match tracked run-task count
    - orchestrator `active_flow_count` must match tracked run-task count
    - tracked run tasks and cancel tokens must name the same runs
    - scoped event streams must be a subset of tracked runs
  - expanded the synthetic negative test to prove each mismatch is caught
- `meerkat-mob/src/runtime/tests.rs`
  - added a live in-flight flow test that captures `MobMachine` snapshots:
    - while a delayed single-step flow is active
    - after terminalization and tracker drain
  - both snapshots must satisfy the new flow-aware validator

Why this slice matters:

- it gives `MobMachine` a real join across three previously separate carriers
  of the same semantic fact instead of trusting the old debug helper counts
- it also confirms something important for the future two-kernel collapse:
  at actor command boundaries, these flow-count carriers are already coherent
  enough to validate directly rather than only by convention

Verification:

- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_flow_activation_and_lifecycle_transitions`
- `cargo test -p meerkat-mob --lib test_flow_tracker_maps_remain_coherent_under_concurrent_run_and_cancel_commands`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first synthetic negative test used `RunId::from(\"...\")`, but `RunId`
  is UUID-backed, so the failure was in the harness rather than the model
- I fixed the test to use real `RunId::new()` values and re-ran the full slice
  instead of weakening the new validator

Current limitation:

- this slice only validates tracker/count coherence, not full flow-run kernel
  truth such as per-run terminal phase, frame state, or step-ledger projection
- `stream_ids` are only validated as a subset relation; there is still no
  stronger semantic check on scoped-event stream lifecycle

Next likely step:

- take the next Mob slice through a narrow flow-run kernel join:
  active run identity vs `MobRunStore` terminal/non-terminal truth, while
  keeping the validator atomic and actor-owned

## Slice 26 - MobMachine tracked-run store identity

Goal:

- join actor-owned tracked run identity with durable `MobRunStore` identity
- verify that tracked runs resolve in the store and carry the same `flow_id`
  the actor believes they do
- keep the invariant conservative enough to survive the real
  terminalization-before-cleanup window

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - extended `MobFlowTrackerSnapshot` with `tracked_flows: RunId -> FlowId`
- `meerkat-mob/src/runtime/actor.rs`
  - `flow_tracker_snapshot()` now exposes that actor-owned run-to-flow mapping
- `meerkat-mob/src/mob_machine.rs`
  - added `TrackedRunStoreSnapshot { flow_id, status }`
  - `capture_mob_machine_snapshot()` now resolves every tracked run ID through
    `MobHandle::flow_status()`
  - added validation that:
    - every tracked run ID exists in `MobRunStore`
    - actor-owned `RunId -> FlowId` agrees with persisted `MobRun.flow_id`
  - expanded the synthetic negative test to prove missing store entries and
    flow-id mismatches are caught
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened the live in-flight flow test so the tracked active run must
    resolve from the store with the expected `flow_id`

Why this slice matters:

- it is the first `MobMachine` join that crosses from actor-owned live control
  state into durable run aggregate truth
- it proves a real mapping to existing code paths rather than to a new
  abstraction: the join is grounded in `run_cancel_tokens` plus
  `MobHandle::flow_status()` / `MobRunStore::get_run()`
- it keeps us honest about what is not safe to prove yet: run existence and
  identity are stable enough, but status-phase equivalence is still too
  timing-sensitive at the current cleanup boundary

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_flow_activation_and_lifecycle_transitions`
- `cargo test -p meerkat-mob --lib test_flow_tracker_maps_remain_coherent_under_concurrent_run_and_cancel_commands`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first instinct was to validate tracked runs as non-terminal, but the
  current actor path can legitimately terminalize a run before the cleanup
  command drains local trackers
- instead of encoding a flaky stronger invariant, this slice stops at store
  existence and `flow_id` identity

Current limitation:

- `MobMachine` still does not validate whether the persisted `MobRun.status`
  phase is aligned with local tracker presence; that remains a known timing
  seam
- the run-store join is point-in-time and non-atomic with respect to actor
  cleanup, so only stable identity facts are validated

Next likely step:

- either collapse the cleanup seam enough to validate tracked-run terminal
  phase safely, or move sideways into a different owner-backed Mob region such
  as kickoff barriers or flow-run kernel projection

## Slice 27 - MobMachine pending-spawn lineage

Goal:

- expose pending-spawn lineage as a first-class Mob diagnostic lane instead of
  relying on actor-local debug assertions
- verify that staged pending spawns stay aligned with orchestrator-owned
  pending count and do not materialize into roster/member projection early
- keep the slice grounded in the real async provisioning path, not the older
  kickoff barrier shim

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - added `MobPendingSpawnLineageSnapshot` to the diagnostic kernel surface
- `meerkat-mob/src/runtime/pending_spawn_lineage.rs`
  - added a lineage snapshot that exposes:
    - metadata ticket IDs
    - task ticket IDs
    - ticket-to-member identity mapping
    - fully provision-bound tickets
    - partial progress tickets
- `meerkat-mob/src/runtime/actor.rs`
  - `DiagnosticKernelSnapshot` now includes the live pending-spawn lineage
- `meerkat-mob/src/mob_machine.rs`
  - added validation that:
    - orchestrator pending count matches lineage size
    - metadata and task ticket sets stay aligned
    - a pending member identity cannot appear twice
    - pending members do not appear in roster or projected member state
    - pending progress never exposes a partial session/operation binding
  - expanded the synthetic negative test to prove those violations are caught
- `meerkat-mob/src/runtime/tests.rs`
  - added a live delayed-spawn test that captures `MobMachine` during
    in-flight provisioning and after finalization settlement

Why this slice matters:

- pending spawn lineage is a real owner-backed Mob seam: it bridges actor task
  state, orchestrator counters, and eventual roster materialization
- the actor already depended on this alignment for cleanup and duplicate-ID
  prevention, but the joined `MobMachine` view could not see it before
- this keeps us on stable identity facts rather than forcing speculative
  stronger flow-status proof

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_pending_spawn_lineage`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- pending-spawn lineage is now part of the visible Mob kernel surface, and the
  live delayed-spawn path satisfies the new invariants

Backtracks encountered:

- I chose not to promote kickoff waiters as the next Mob seam because the code
  now treats kickoff as a compatibility barrier over synchronous runtime input
  admission, which makes it a weaker semantic center than pending spawn
  lineage
- I also stopped short of requiring every provision-bound pending ticket to
  have reached roster materialization, because finalization legitimately
  settles that after the async provisioning task completes

Current limitation:

- the validator still does not reason about the timing between
  `provision_bound_ticket_ids` and final roster admission; it only forbids
  premature materialization and partial binding

Next likely step:

- either validate one stable kickoff/initial-runtime readiness fact now that
  pending spawn lineage is visible, or move into a narrow flow-kernel join
  around persisted step/activation truth

## Slice 28 - MobMachine restore-failure ownership

Goal:

- stop treating `Broken` member status as a loose projection by joining it back
  to the canonical restore-failure map
- verify that restore diagnostics, roster membership, and projected broken
  member material stay aligned through real resume failures
- keep the seam honest by going through a named handle-level diagnostic read
  path rather than reaching into handle internals directly

What landed:

- `meerkat-mob/src/runtime/handle.rs`
  - added `diagnostic_restore_failures_snapshot()` that normalizes the
    internal `HashMap` into a deterministic `BTreeMap`
- `meerkat-mob/src/mob_machine.rs`
  - added `restore_failures` to `MobMachineSnapshot`
  - added restore-aware invariants:
    - every restore failure maps to a roster member
    - every restore failure maps to a projected member
    - projected member status must be `Broken`
    - projected session binding and error reason must match the restore
      diagnostic
    - every projected `Broken` member must have a restore diagnostic
  - added a focused negative unit test proving the validator rejects a
    projected broken member without canonical restore failure
- `meerkat-mob/src/runtime/tests.rs`
  - added a live resume-failure test that captures `MobMachine` after a
    missing-persisted-session restore and proves the joined snapshot is clean

Why this slice matters:

- `Broken` is not an independent lifecycle state today; it is restore-failure
  truth exposed through member projection
- before this slice, `MobMachine` could see broken members but not the owner
  that made them broken, which made the boundary fuzzier than it needed to be
- joining the restore map tightens an actual product-facing failure mode
  without introducing new timing assumptions

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_broken_projection_without_restore_failure`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_restore_failure_projection`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_pending_spawn_lineage`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- restore-failure ownership is now visible in the joined Mob snapshot, and the
  live broken-resume path satisfies the new invariants

Backtracks encountered:

- the first implementation tried to read `MobHandle.restore_diagnostics`
  directly from `mob_machine.rs`, which was a good failure: it crossed a
  private boundary instead of using a named diagnostic seam
- fixing that by adding a handle accessor also forced deterministic ordering at
  the seam, which is better for tests and future TLA-facing snapshots

Current limitation:

- this still models restore failure as a handle-side joined read path, not as
  actor-owned lifecycle truth; that is appropriate for today’s code, but it
  means `Broken` remains a projection over roster plus restore diagnostics

Next likely step:

- either pull one stable kickoff/readiness fact into the joined MobMachine
  view, or switch back to a narrow Meerkat-side slice if the next Mob seam is
  too timing-sensitive

## Slice 29 - MeerkatMachine peer authority and queue coherence

Goal:

- tighten the peer-ingress validator from "authority queue empty vs non-empty"
  to exact agreement between the authority-owned submission queue and the
  queued authority-tracked ingress entries
- keep the rule grounded in the live owner boundary: the inbox snapshot is
  captured atomically under one lock, so this count equality is stable enough
  to validate without cross-crate timing races
- avoid smuggling plain external events into peer authority semantics

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added `PeerAuthorityTrackedEntryCountMismatch`
  - added `push_peer_authority_mismatches(...)`
  - validator now checks that `submission_queue_len` equals the number of
    queued entries whose kind is not `PlainEvent`
  - added a focused negative test proving the validator rejects a snapshot
    where peer authority claims two queued submissions but only one
    authority-tracked ingress entry exists

Why this slice matters:

- the previous phase/queue-length rule could still miss a subtler owner drift:
  a `Received` authority with a non-zero queue length but the wrong number of
  actual peer entries visible at the ingress boundary
- exact count agreement is a stronger statement and still honest because both
  numbers come from the same inbox-owned snapshot path
- excluding `PlainEvent` from the comparison preserves the important
  architecture cut: plain external events are queued ingress, but not peer
  authority truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- MeerkatMachine now validates the exact size of the authority-tracked peer
  ingress surface, not just the authority phase

Backtracks encountered:

- the tempting but wrong invariant was "submission queue length matches total
  queue length"; the comms runtime tests make it clear that plain external
  events may coexist with an `Absent` or `Dropped` peer authority
- the right invariant therefore counts only authority-tracked entries and
  leaves plain events outside the peer authority ledger

Next likely step:

- either deepen Meerkat's peer side one more step with trust-membership
  surface checks, or return to a narrow MobMachine slice once this Meerkat
  seam feels solid enough

## Slice 30 - MeerkatMachine ingress-time trust snapshots

Goal:

- stop reconstructing peer-entry trust from the current trusted-peer set and
  carry the canonical ingress-time trust decision into the queued peer snapshot
- validate that strict-auth runtimes never expose an admitted untrusted peer
  entry, while plain external events remain outside peer authority truth
- tighten the classified inbox seam so the peer authority is seeded from the
  same initial trust context used by ingress classification

What landed:

- `meerkat-core/src/interaction.rs`
  - added `trusted_snapshot: Option<bool>` to `PeerIngressEntrySnapshot`
- `meerkat-comms/src/peer_comms_authority.rs`
  - added `trusted_snapshot_for(...)` accessor
- `meerkat-comms/src/inbox.rs`
  - `ClassifiedInboxEntry` now carries `trusted_snapshot`
  - `send_classified()` now threads the authority-owned trust decision into
    queued entries
  - `Inbox::new_classified()` now seeds the peer authority with the initial
    trusted peers from `IngressClassificationContext`
  - inbox snapshot tests now assert trusted peer entries carry
    `Some(true)` and plain events carry `None`
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - live runtime snapshot tests now assert plain events keep
    `trusted_snapshot = None`
  - auth-open unknown-peer admission now proves
    `trusted_snapshot = Some(false)` on the admitted peer entry
- `meerkat/src/meerkat_machine.rs`
  - validator now checks:
    - non-plain entries must carry a trust snapshot
    - plain events must not
    - auth-required runtimes may not expose admitted untrusted peer entries
  - authority-tracked entry counting now keys off the explicit trust snapshot
    rather than inferring from ingress kind alone
  - added a focused negative test for trust-shape violations

Why this slice matters:

- before this change, the Meerkat peer snapshot knew "who is trusted now" but
  not "was this queued peer item trusted when it was admitted"
- that made it impossible to distinguish a valid auth-open untrusted peer
  entry from a malformed strict-auth ingress state without re-deriving truth
- seeding the authority from the initial classification context closes a real
  split-owner gap: classified ingress and peer authority now start from the
  same trust basis

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib classified_snapshot`
- `cargo test -p meerkat-comms --lib peer_ingress_runtime_snapshot`
- `cargo test -p meerkat-comms --lib auth_open`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_trust_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo check -p meerkat-comms --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- MeerkatMachine now sees peer ingress with the same trust decision the peer
  authority used at admission time, and the classified inbox no longer starts
  with a silent authority/trust mismatch

Backtracks encountered:

- the first pass assumed the inbox tests should already report
  `trusted_snapshot = Some(true)` for seeded trusted peers; when they instead
  reported `Some(false)`, that exposed a real initialization gap rather than a
  bad test
- fixing the root cause in `Inbox::new_classified()` was the right move; it
  brought the authority into line with the classification context instead of
  weakening the new trust assertions

Next likely step:

- Meerkat's peer seam is now much firmer, so the next deliberate move can
  either be one more same-owner peer/trust invariant or a return to a narrow
  MobMachine slice without building on soft ground

## Slice 31 - MobMachine kickoff barrier ownership

Goal:

- make live autonomous kickoff-barrier state visible inside the joined
  `MobMachine` kernel snapshot instead of leaving it as a handle-only wait path
- validate the strongest same-owner seam first: a member cannot still be in
  pending spawn lineage and already be tracked by the kickoff barrier
- verify the live delayed-autonomous-start path against real actor state

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - added `MobKickoffBarrierSnapshot { pending_member_ids }`
  - added `kickoff_barrier` to `MobKernelDiagnosticSnapshot`
- `meerkat-mob/src/runtime/actor.rs`
  - added `kickoff_barrier_snapshot()` using the existing
    `autonomous_initial_turns` map and the same finished-handle pruning logic
    as `snapshot_kickoff_barrier_state()`
  - `DiagnosticKernelSnapshot` now includes the live kickoff barrier snapshot
- `meerkat-mob/src/mob_machine.rs`
  - added kickoff-aware invariants:
    - kickoff-pending member must not overlap pending spawn lineage
    - kickoff-pending member must still resolve in roster + projected member
    - kickoff-pending member must still have a session binding
    - kickoff-pending member must not already be `Broken`
  - added a focused synthetic validator test for kickoff-specific violations
  - extended the main synthetic projection test so kickoff/pending-spawn
    overlap is exercised alongside the earlier lineage assertions
- `meerkat-mob/src/runtime/tests.rs`
  - added a live test with delayed autonomous startup showing:
    - the spawned member appears in `kickoff_barrier.pending_member_ids`
    - the same member is already past pending spawn lineage
    - the settled snapshot drains the kickoff barrier after
      `wait_for_kickoff_complete()`

Why this slice matters:

- kickoff barrier state is real Mob runtime truth: callers use it to decide
  whether autonomous startup has actually settled
- before this slice, MobMachine could see pending spawn lineage and member
  projection, but not the intermediate autonomous-start barrier state between
  those phases
- the overlap check closes a useful ownership seam without relying on timing:
  pending spawn lineage and kickoff barrier are both actor-owned and captured in
  the same diagnostic kernel snapshot

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_kickoff_barrier_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_kickoff_barrier_state`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_pending_spawn_lineage`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_restore_failure_projection`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- MobMachine now sees and validates the live autonomous kickoff barrier, and
  the actor-owned handoff from pending spawn lineage to kickoff tracking is
  executable instead of implicit

Backtracks encountered:

- the first kickoff-specific unit test was shaped wrong: it expected a missing
  projected entry for a member that was still present in the projected member
  list
- splitting that into distinct fixture members produced a cleaner test and a
  better mapping from each violation to one semantic mistake

Next likely step:

- the next honest Mob slice is probably a flow-kernel join, now that the
  lifecycle/orchestrator/topology/pending-spawn/kickoff stack is visible in one
  place

## Slice 32 - MobMachine tracked flow-kernel shape

Goal:

- make `MobMachine` read typed flow-kernel run state instead of treating
  `flow_status()` as just `{ flow_id, status }`
- validate stable run-kernel invariants that survive cleanup races:
  completion timestamp shape, ordered-step membership, and terminal
  non-dispatched step state
- keep the slice honest by avoiding "tracked run must already be cleaned up"
  folklore while a terminal run is still draining through actor cleanup

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun` readers for:
    - `ordered_steps()`
    - `step_status_snapshot()`
    - `failure_count()`
    - `consecutive_failure_count()`
  - centralized `StepRunStatus` parsing with
    `StepRunStatus::from_flow_run_kernel_value(...)`
  - added focused unit coverage for the new typed readers and explicit unknown
    variant rejection
- `meerkat-mob/src/runtime/flow_run_kernel.rs`
  - now reuses the new `StepRunStatus` parser instead of carrying its own
    duplicate decoding logic
- `meerkat-mob/src/mob_machine.rs`
  - replaced the old tracked-run projection with a richer
    `TrackedRunSnapshot` model:
    - `Missing`
    - `Present(TrackedRunStoreSnapshot)`
    - `Malformed(TrackedRunMalformedSnapshot)`
  - `capture_mob_machine_snapshot()` now parses live `MobRun` kernel state into
    typed tracked-run snapshots and records parse failures explicitly instead of
    silently collapsing them into missing-store state
  - added tracked-run invariants for:
    - terminal status ↔ `completed_at` presence
    - duplicate ordered-step entries
    - step-status keys outside the ordered-step set
    - terminal runs still carrying `Dispatched` step status
    - `consecutive_failure_count <= failure_count`
  - added a focused synthetic validator test for tracked-run kernel-shape
    violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting MobMachine test to assert the richer
    tracked-run projection:
    - `completed_at_present` stays false while the run is active
    - ordered steps match the single-step flow definition
    - surfaced step statuses stay within the kernel-known step set

Why this slice matters:

- this is the first MobMachine slice that reads through typed flow-kernel state
  rather than only actor counters and run-store identity
- `MobRun.flow_state` already contains the canonical flow-run machine truth, but
  without typed readers the diagnostic path had to stop at shallow projection
- making malformed tracked runs explicit is important: a corrupt kernel state is
  not the same thing as a missing run, and collapsing those two would create
  another diagnostic blind spot

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_run_kernel_readers_surface`
- `cargo test -p meerkat-mob --lib step_status_snapshot_rejects_unknown_variant`
- `cargo test -p meerkat-mob --lib tracked_run_kernel_shape`
- `cargo test -p meerkat-mob --lib projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- MobMachine can now see and validate a meaningful slice of flow-kernel truth
  for tracked runs, not just actor-owned run counters
- the flow-run parser path is now centralized enough that future tracked-run
  diagnostics can build on `MobRun` helpers instead of duplicating `KernelValue`
  decoding logic

Backtracks encountered:

- the first compile pass exposed two self-inflicted issues:
  - partial move of `run.status` before reading the rest of the aggregate
  - a test pattern that moved the error string it still wanted to print
- fixing both early was useful signal that the richer tracked-run projection was
  still a modest, localized slice rather than a broader ownership problem

Next likely step:

- the next honest Mob slice is probably one of:
  - a deeper flow-kernel join around frame/run readiness state
  - a return to MeerkatMachine if the next Mob flow invariant starts depending
    on cleanup timing rather than stable owner truth

## Slice 33 - MeerkatMachine comms-drain shape

Goal:

- turn the runtime-owned comms-drain carrier from "visible but unvalidated"
  into an executable MeerkatMachine slice
- keep the slice strictly inside same-owner runtime truth so it satisfies the
  cutover gate discipline for observability work
- avoid waiter/ledger or barrier/ops joins that still depend on non-atomic
  multi-lock reads

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added drain-shape invariants to `validate_meerkat_machine_snapshot()`:
    - no phase/mode/handle may appear without a published drain slot
    - a published slot must carry a phase
    - `Inactive` drain cannot carry mode or handle
    - `Starting | Running | ExitedRespawnable | Stopped` must carry mode
    - `Inactive | ExitedRespawnable | Stopped` cannot still hold a task handle
  - added a focused synthetic validator test covering:
    - missing-slot metadata leakage
    - slot-without-phase
    - inactive-with-mode/handle
    - stopped-without-mode and stopped-with-handle
  - added a joined positive test proving a real stopped comms drain remains a
    valid `MeerkatMachine` snapshot
- `meerkat-runtime/src/session_adapter.rs`
  - tightened the idle spine test to assert the absent-drain shape explicitly
  - added a runtime-spine test for a live `Stopped` comms-drain slot:
    - slot remains published
    - phase is `Stopped`
    - mode is preserved
    - task handle is cleared

Why this slice matters:

- `drain` was one of the remaining Meerkat runtime-spine regions that had real
  owner data in the snapshot but no executable machine checks
- the comms-drain lifecycle is also a recurring seam hazard because it mixes
  keep-alive policy, background task ownership, and stop/exit cleanup
- this slice is a good fit for the cutover-gate discipline: it strengthens the
  observational machine without inventing a new reducer or depending on racy
  cross-crate joins

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_comms_drain_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_stopped_comms_drain_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_drain_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `git diff --check`

Result:

- MeerkatMachine now validates the runtime-owned comms-drain carrier in the
  same style as peer ingress, tool-surface, and ingress lifecycle shape
- the live stopped-drain state is now represented and tested explicitly, which
  matters for later cutover work because stop/cleanup states are where the old
  architecture usually lies

Backtracks encountered:

- the tempting next slice was completion-waiter vs admitted-input terminality,
  but that still crosses separate runtime locks and would have pushed us toward
  snapshot folklore instead of owner-backed machine truth
- staying with drain was the right call because its phase/mode/handle shape is
  captured from one owner lock and survives both positive and negative tests

Cutover gate read:

- this slice improves MeerkatMachine observability and executable coverage, but
  it does **not** move MeerkatMachine past the cutover gate yet
- we are still in the "observational model over real owners" stage:
  write authority has not moved, semantic freeze is not complete, and the
  stronger cross-region joins (especially timing-sensitive ones) are still
  intentionally deferred

Next likely step:

- return to `MobMachine` for another narrow owner-backed slice, or keep
  Meerkat-first if the next most valuable gap is still clearly same-owner and
  stable under the cutover-gate rules

## Slice 34 - MobMachine tracked run dependency-shape

Goal:

- deepen `MobMachine`'s tracked run view with one more proof-relevant slice of
  flow-kernel truth
- stay entirely inside the persisted run-store surface, avoiding actor cleanup
  timing and avoiding new shadow state
- use the cutover gate discipline to distinguish real live kernel truth from
  values that only exist on input/setup paths

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_dependencies()` reader for the persisted
    `step_dependencies` kernel field
  - extended the existing reader test to assert dependency map shape for a
    simple flow
  - added a focused negative test that rejects malformed dependency entries
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_dependencies`
  - added tracked-run invariants for:
    - every ordered step must have a dependency-map entry
    - dependency-map keys must refer to ordered steps
    - dependency targets must refer to ordered steps
  - extended the synthetic tracked-run validator test with dependency-shape
    violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface an empty dependency map for its only step

Why this slice matters:

- step/dependency metadata is proof-relevant flow truth, but unlike actor task
  trackers it lives in one durable owner surface
- this gives `MobMachine` another meaningful piece of kernel state without
  drifting into cleanup races or projecting behavior from event order
- it also sharpens the future TLA+ boundary by making more of the flow-kernel
  alphabet readable without spelunking raw `KernelValue` maps in validators

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_dependencies_reject_invalid_dependency_entry`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- MobMachine now sees and validates another stable slice of tracked flow-kernel
  structure from the live run store
- the live flow-accounting path still passes after the richer tracked-run
  parsing, so the slice is grounded in the actual runtime rather than a purely
  synthetic validator fixture

Backtracks encountered:

- the first version of this slice tried to treat `step_ids` as persisted live
  kernel truth because it is present in the `CreateRun` input surface
- that was wrong: the live tracked run failed immediately with
  `step_ids missing or invalid`, which means `step_ids` is not a stable stored
  field in the current run aggregate
- the correct recovery was to back out `step_ids` entirely and anchor the slice
  to fields that really survive in durable state:
  `ordered_steps` and `step_dependencies`

Cutover gate read:

- this slice improves MobMachine observability, but it does **not** move
  MobMachine past the cutover gate
- the important signal here is that the gate is still doing useful work:
  it caught a tempting but false "machine fact" before we promoted it into a
  larger semantic story

Next likely step:

- either keep deepening persisted tracked-run metadata in similarly small
  slices, or move sideways to another owner-backed Mob kernel region if the
  next flow-kernel fact starts depending on cleanup timing rather than stable
  stored truth

## Slice 35 - MeerkatMachine ops-shape

Goal:

- tighten `MeerkatMachine` around one more same-owner runtime seam without
  stepping into the still-racy cross-crate joins
- make the existing ops snapshot executable instead of just descriptive by
  validating count shape, wait-all shape, wait-target coverage, and
  detached-wake presence pairing
- keep the slice entirely on the current runtime-owned read path so the
  cutover gate can still distinguish owner truth from snapshot folklore

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added ops-shape invariant variants for:
    - `OpsOperationCountMismatch`
    - `WaitOperationIdsWithoutWaitRequest`
    - `WaitRequestWithoutTrackedOperations`
    - `WaitTargetsUnknownOperation`
    - `DetachedWakePresenceMismatch`
  - factored the runtime-owned checks into `push_ops_mismatches(...)`
  - added a focused synthetic validator test that mutates a live base snapshot
    instead of inventing a completely synthetic machine fixture

Why this slice matters:

- the ops snapshot already comes from one runtime-owned lock path, so it is a
  safer place to strengthen executable machine checks than the timing-sensitive
  barrier/turn joins
- the new invariants catch the exact kind of owner/shell drift we have seen in
  earlier backtracks: a wait request without a tracked set, tracked wait
  targets without a wait request, or snapshot operation counts that no longer
  line up with live records
- detached-wake presence pairing is small, but it is one of the cleanup/handoff
  carriers that historically tends to rot quietly if we do not make it explicit

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ops_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates another owner-backed region of runtime truth
  instead of only exposing it diagnostically
- the live runtime and Meerkat library lanes stayed green, which is a useful
  sign that these checks are anchored in real runtime authority rather than in
  aspirational machine shape

Backtracks encountered:

- I explicitly did **not** strengthen `active_count` into a full
  status-derived equality check yet, because the current operation snapshot is
  still built from authority plus shell records and I did not want to blur
  "authority mismatch" with "projection missing shell record" in the same step
- I also kept barrier/ops coupling out of this slice; that join still crosses
  timing-sensitive read paths and would not yet satisfy the cutover-gate bar

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- write authority is still in the old owners, semantic freeze is still not
  complete, and the stronger timing-sensitive joins are still intentionally
  deferred

Next likely step:

- either deepen one more same-owner ops invariant (if it can be justified from
  one runtime-owned snapshot path), or return to another Meerkat region before
  attempting the still-riskier barrier/turn coupling

## Slice 36 - MobMachine tracked run dependency-mode shape

Goal:

- continue `MobMachine` on the same persisted flow-kernel path without falling
  back into actor cleanup timing
- expose one more piece of durable run truth that is semantically meaningful to
  flow execution: the per-step dependency interpretation mode
- use the cutover gate to avoid promoting config/input-only facts that are not
  actually persisted in the live run aggregate

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_dependency_modes()` reader for the persisted
    `step_dependency_modes` kernel field
  - extended the kernel-reader test to assert live dependency-mode shape for a
    simple flow
  - added a focused negative parse test for unknown `DependencyMode` variants
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_dependency_modes`
  - added tracked-run invariants for:
    - every ordered step must have a dependency-mode entry
    - dependency-mode map keys must refer to ordered steps
  - extended the synthetic tracked-run validator test with dependency-mode
    violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface `DependencyMode::All` for its only step

Why this slice matters:

- dependency mode is real flow-kernel truth, not just config decoration: it is
  what makes `all` versus `any` dependency readiness semantically different at
  execution time
- unlike actor task trackers or cleanup counters, it lives in durable run state
  and survives the current read path cleanly
- it also reinforces the cutover-gate discipline after the previous
  `step_ids` backtrack: we are still only promoting facts that actually survive
  in the stored run aggregate

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_dependency_modes_reject_unknown_variant`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now sees and validates another durable slice of tracked
  flow-kernel truth without leaning on actor cleanup timing
- the live flow-accounting snapshot continues to satisfy the richer validator,
  so this slice stays grounded in the actual runtime rather than in a
  synthetic-only model

Backtracks encountered:

- there was no code-level backtrack in this slice, but the selection itself was
  a deliberate backtrack from "any next flow field" to "only fields that are
  clearly persisted kernel truth"
- I explicitly did **not** promote collection policy or other config-shaped
  surfaces in this pass because I had not yet re-established that they are the
  right stable read boundary for the live run aggregate

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- the tracked-run surface is getting firmer, but write authority is still in
  the old runtime/store/orchestrator components and the semantic boundary is
  still not frozen

Next likely step:

- either keep deepening durable tracked-run metadata in similarly small slices,
  or move sideways to another owner-backed Mob region if the next tracked-run
  fact starts to depend on actor cleanup timing instead of stable persisted
  truth

## Slice 37 - MeerkatMachine per-operation ops shape

Goal:

- deepen `MeerkatMachine` ops validation without crossing into the still-racy
  barrier/turn join
- promote one more owner-backed slice of runtime truth from "count-like shell
  shape" into explicit per-operation semantics
- keep the slice grounded in the existing runtime authority read path so it can
  be judged against the cutover gate rather than against an idealized machine

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with richer per-operation ops
    mismatch cases:
    - detached wake presence mismatch with the runtime binding
    - active-count versus computed nonterminal operation count mismatch
    - wait-all request already satisfied by the tracked operation set
    - peer-ready/handle inconsistencies
    - terminal outcome and completion timestamp mismatches
    - active operation missing start timestamp
    - terminal operation still carrying watchers
  - strengthened `push_ops_mismatches(...)` to validate those invariants from
    the joined runtime spine
  - extended the focused negative validator test so the richer per-operation
    mismatches are actually exercised
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the positive runtime-spine test to assert concrete per-op
    shape on a live operation snapshot, including start/completion timing and
    peer-handle absence

Why this slice matters:

- it moves ops validation closer to the real historical failure surface:
  operations are not just aggregate counts, they carry lifecycle, wait, wake,
  peer handoff, and completion truth
- it stays on one runtime-owned read path, which keeps it inside the
  cutover-gate discipline instead of overreaching into timing-sensitive joins
- it also gives us a more honest signal about whether the observational model
  is merely describing the shell or actually matching owner-backed runtime
  truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ops_shape_violations`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_runtime_ops_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`

Result:

- `MeerkatMachine` now validates richer per-operation ops truth rather than
  only aggregate runtime shape
- the focused runtime and joined Meerkat tests stayed green, which is a good
  sign that these checks are still anchored in the real runtime authority path

Backtracks encountered:

- the first pass of the richer negative ops test moved the base snapshot too
  early and then tried to clone it again; that was a test-harness mistake, not
  a machine-design issue, and it was fixed by cloning earlier
- I still deliberately kept barrier/ops coupling out of this slice because it
  would not yet satisfy the cutover-gate requirement for stable owner
  boundaries and non-racy reads

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- write authority still lives in the old runtime owners, semantic freeze is
  still incomplete, and the stronger timing-sensitive joins are still
  intentionally deferred

Next likely step:

- either take one more same-owner runtime slice that clearly satisfies the
  cutover gate, or move back to another Meerkat region before attempting the
  still-riskier barrier/turn coupling

## Slice 38 - MobMachine tracked run collection-policy and quorum shape

Goal:

- continue `MobMachine` on the durable tracked-run path rather than dropping
  back into actor cleanup timing
- validate that another piece of flow-kernel meaning survives in persisted run
  truth, not just in the create-run input surface
- use the cutover gate to reject any field that turns out to be setup-shaped
  rather than stable kernel truth

What landed:

- `meerkat-mob/src/run.rs`
  - added typed readers for:
    - `step_collection_policy_kinds()`
    - `step_quorum_thresholds()`
  - introduced `RunCollectionPolicyKind` so collection policy can be read as
    typed kernel truth instead of raw strings
  - extended the existing kernel-reader test to assert collection policy and
    quorum shape for a simple persisted run
  - added a focused negative parse test for unknown collection-policy variants
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with:
    - `step_collection_policy_kinds`
    - `step_quorum_thresholds`
  - added tracked-run invariants for:
    - every ordered step must have a collection policy entry
    - every ordered step must have a quorum-threshold entry
    - policy and quorum maps may only reference ordered steps
    - `Quorum` steps must have a nonzero threshold
    - `All` and `Any` steps must keep threshold `0`
  - extended the synthetic tracked-run validator test with the new violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface both `RunCollectionPolicyKind::All` and quorum threshold `0`

Why this slice matters:

- collection policy and quorum threshold are semantically active flow-kernel
  truth; they govern when a step may become ready, not just how the original
  flow happened to be authored
- the live flow-accounting snapshot proved they really survive in persisted run
  state, which is the exact question the cutover gate asks us to answer before
  promoting a field into the machine
- it is also a direct continuation of the earlier `step_ids` backtrack: we are
  still refusing to model fields as kernel truth until the live runtime proves
  they actually live there

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_collection_policy_kinds_reject_unknown_variant`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`

Result:

- `MobMachine` now validates another durable slice of tracked flow-kernel truth
  without leaning on actor cleanup timing
- the live flow-accounting snapshot stayed green, which is the useful proof
  that this is persisted kernel state rather than config echo

Backtracks encountered:

- there was no code-level backtrack in the final slice, but the selection
  itself was a deliberate backtrack from "read any nearby flow field" to "only
  promote facts that the live run aggregate actually preserves"
- that discipline is exactly the one the cutover gate is meant to enforce

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- write authority is still in the old runtime/store/orchestrator components,
  the machine boundary is still converging, and semantic freeze is still not
  complete

Next likely step:

- either keep deepening clearly persisted tracked-run metadata in similarly
  narrow slices, or move sideways to another owner-backed Mob region if the
  next candidate starts to depend on actor cleanup timing instead of durable
  truth

## Slice 39 - MeerkatMachine completion waiter coherence

Goal:

- strengthen the completion-waiter carrier without crossing into new owner
  boundaries
- make the joined `MeerkatMachine` validator reject waiter state that already
  points at resolved input truth
- keep the slice grounded entirely in the runtime-owned spine: admitted input
  lifecycle and terminal outcome plus the existing completion-waiter snapshot

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `CompletionWaiterZeroCount`
    - `CompletionWaiterResolvedInput`
  - strengthened completion-waiter validation so:
    - waiter entries may not have `waiter_count == 0`
    - waiter entries may not point at inputs that are already terminal or that
      already expose a terminal outcome
  - extended the focused negative joined-snapshot test to exercise:
    - zero-count waiter entries
    - waiter entries attached to an input already marked
      `Consumed`/`terminal_outcome = Consumed`

Why this slice matters:

- completion waiters are a supporting carrier, not canonical truth, so they
  are exactly the kind of thing that tends to drift unless the machine makes
  the carrier constraints explicit
- this slice stays fully on one runtime-owned read path and therefore fits the
  cutover-gate rule of "strengthen already-agreed ownership, do not invent a
  new boundary"
- it also tightens one of the historical handoff surfaces: callers waiting on
  input completion should never be represented as still pending after the input
  is already resolved

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_queued_prompt_input`

Result:

- `MeerkatMachine` now validates a stricter completion-waiter carrier shape
  without inventing new runtime semantics
- the live Meerkat and runtime-spine tests stayed green, which is the right
  signal that the new rule is aligned with current owner-backed runtime truth

Backtracks encountered:

- there was no code-level backtrack in the final slice, but the selection was
  deliberately narrower than several tempting alternatives
- I explicitly did **not** try to infer callback-boundary semantics or new
  completion classes here; this slice only uses truth already present in the
  runtime spine

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- completion waiter write authority still lives in the existing runtime
  component, and semantic freeze is still incomplete

Next likely step:

- either take another same-owner runtime slice around input/completion carrier
  coherence, or move sideways to another Meerkat region before revisiting the
  still-riskier timing-sensitive joins

## Slice 40 - MobMachine tracked run condition-flag shape

Goal:

- continue `MobMachine` on the persisted tracked-run path
- promote one more flow-kernel field only if the live stored run aggregate
  proves it really survives there
- validate step-level condition-presence shape without dragging in dispatch
  timing or actor cleanup state

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_has_conditions()` reader over the persisted
    `step_has_conditions` kernel map
  - extended the kernel-reader test to assert condition-presence shape for a
    simple persisted run
  - added a focused negative parse test for non-boolean condition flags
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_has_conditions`
  - extended tracked-run validation so:
    - every ordered step must have a condition-flag entry
    - condition-flag keys may only reference ordered steps
  - extended the synthetic tracked-run validator test with condition-flag
    missing/unknown-step violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface `step_has_conditions = { start -> false }`

Why this slice matters:

- condition presence is not just authoring metadata; it changes dispatch
  legality because condition results must be recorded before a guarded step can
  run
- the live flow-accounting snapshot proved that the field is actually preserved
  in the stored run aggregate, which is exactly the bar the cutover gate asks
  us to clear before promoting a field into the machine
- it also keeps us on the durable flow-kernel path rather than drifting back
  into actor timing or cleanup folklore

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_has_conditions_rejects_non_bool_entry`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`

Result:

- `MobMachine` now validates another persisted step-level flow-kernel map
  without relying on actor cleanup timing
- the live flow-accounting snapshot stayed green, which is the useful signal
  that `step_has_conditions` is real tracked-run truth rather than create-run
  config echo

Backtracks encountered:

- the only backtrack was the usual selection discipline: I promoted
  `step_has_conditions` only after the live run projection proved it survives
  as stored kernel state
- there was no code-level correction once that boundary was chosen

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- tracked-run write authority is still in the existing runtime/store/kernel
  stack, and semantic freeze is still incomplete

Next likely step:

- either keep deepening clearly persisted tracked-run shape in similarly narrow
  slices, or move sideways to another owner-backed Mob region if the next
  candidate starts depending on actor timing instead of durable truth

## Slice 41 - MeerkatMachine ingress routing and contributor linkage

Goal:

- strengthen `MeerkatMachine` around runtime-ingress facts that are already
  canonical in `RuntimeIngressAuthority`
- validate queue routing truth and active contributor linkage without touching
  the still-timing-sensitive barrier/turn seam
- stay entirely on one owner path: admitted input metadata, queue membership,
  current-run contributors, and stored run/boundary lineage

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `QueueContainsWrongHandlingMode`
    - `QueueAppearsInBothQueues`
    - `CurrentRunContributorRunMismatch`
    - `CurrentRunContributorPendingConsumptionMissingBoundary`
  - strengthened validation so:
    - `queue` entries must carry `HandlingMode::Queue`
    - `steer_queue` entries must carry `HandlingMode::Steer`
    - the same input may not appear in both queues
    - current-run contributors must carry `last_run_id == current_run_id`
    - contributors already in `AppliedPendingConsumption` must have a recorded
      `last_boundary_sequence`
  - extended the focused negative joined-snapshot test to exercise these cases
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the queued-prompt runtime-spine test to assert the live
    snapshot carries `handling_mode = Queue`

Why this slice matters:

- these are all real runtime-ingress facts already owned by the authority:
  queue routing, per-input handling mode, `last_run`, and boundary sequence are
  not shell folklore
- it tightens one of the recurring seam classes directly: "an input is in the
  queue but not semantically queued the same way everywhere"
- it also gives the joined machine a better model of run-linkage without
  reaching into the still-riskier turn/boundary timing seam

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_queued_prompt_input`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`

Result:

- `MeerkatMachine` now validates more of the ingress-routing and contributor
  lineage truth that the runtime already owns canonically
- the live runtime-spine and joined Meerkat tests stayed green, which is the
  right sign that these invariants are observing current owner-backed truth
  rather than inventing a replacement ahead of time

Backtracks encountered:

- the only code-level backtrack was an unqualified `InputLifecycleState`
  reference in the validator; that was a plain implementation miss, not a
  semantic retreat
- the slice itself was intentionally narrower than several tempting ingress
  ideas; I did **not** promote prompt/content-shape folklore or timing-sensitive
  boundary claims that are not yet justified by the live owner path

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- ingress write authority is still in the existing runtime authority and
  semantic freeze is still incomplete

Next likely step:

- either take another same-owner ingress/runtime slice, or move sideways to a
  different Meerkat region before revisiting any timing-sensitive cross-crate
  joins

## Slice 42 - MobMachine tracked run branch map shape

Goal:

- continue the durable tracked-run path in `MobMachine`
- promote one more step-level kernel map only if the persisted run aggregate
  proves it really survives there
- validate branch membership shape without dragging in actor cleanup timing or
  runtime execution details

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_branches()` reader over the persisted
    `step_branches` kernel map
  - extended the kernel-reader test to assert `None` branch shape for a simple
    persisted run
  - added a focused negative parse test for invalid non-string/non-`None`
    branch entries
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_branches`
  - extended tracked-run validation so:
    - every ordered step must have a branch-map entry
    - branch-map keys may only reference ordered steps
  - extended the synthetic tracked-run validator test with missing/unknown
    branch-map violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface `step_branches = { start -> None }`

Why this slice matters:

- branch membership is part of real flow semantics: it drives branch-winner and
  join behavior, so it is not just authoring metadata
- the live tracked-run snapshot proved that the field survives in persisted run
  truth, which is exactly the cutover-gate bar we want before promoting it into
  the machine
- it keeps us on the durable flow-kernel path instead of drifting into actor
  cleanup timing or setup-only configuration

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_branches_reject_invalid_entry`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`

Result:

- `MobMachine` now validates another persisted step-level flow-kernel map
  without relying on actor cleanup timing
- the live flow-accounting snapshot stayed green, which is the useful signal
  that `step_branches` is real tracked-run truth rather than config echo

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the meaningful backtrack was still the selection discipline itself: the field
  only got promoted after the stored run aggregate and live snapshot path made
  it clear that the branch map actually persists as kernel truth

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- tracked-run write authority is still in the existing runtime/store/kernel
  stack, and semantic freeze is still incomplete

Next likely step:

- either keep deepening clearly persisted tracked-run maps in the same style,
  or move sideways to another owner-backed Mob region if the next candidate no
  longer clears the durable-truth bar

## Slice 43 - MeerkatMachine ingress completeness and queue ownership

Goal:

- tighten the ingress side from the admitted-input perspective instead of only
  validating queue entries
- make sure admitted inputs still carry the authority-owned metadata that makes
  queue placement meaningful
- close the reverse seam where an input can be semantically `Queued` but no
  longer appear in its owning ingress lane

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `AdmittedInputMissingContentShape`
    - `AdmittedInputMissingHandlingMode`
    - `AdmittedInputMissingLifecycle`
    - `QueuedInputMissingOwningQueue`
  - strengthened validation so:
    - every admitted input must still carry `content_shape`
    - every admitted input must still carry `handling_mode`
    - every admitted input must still carry `lifecycle`
    - any input whose lifecycle is `Queued` must appear in its owning queue
      based on `handling_mode`
  - extended the focused negative joined-snapshot test to exercise missing
    metadata and missing queue membership
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the live queued-prompt runtime-spine test so it now asserts:
    - the input is present in `inputs.queue`
    - `inputs.steer_queue` stays empty
    - `content_shape` is populated

Why this slice matters:

- the first ingress slices only validated the queue-to-input direction; this
  slice closes the opposite direction
- `content_shape`, `handling_mode`, and `lifecycle` are not shell convenience
  fields; they are canonical ingress facts used to classify boundaries and
  queue ownership
- "queued but not actually owned by a queue anymore" is exactly the kind of
  silent drift the joined machine is meant to catch before cutover

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_queued_prompt_input`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates ingress completeness from both sides:
  queue entries must point to the right admitted input state, and queued inputs
  must remain owned by the right ingress lane
- the live runtime-backed snapshot path stayed green, which is the important
  sign that the slice is observing current authority truth rather than
  projecting future semantics

Backtracks encountered:

- the only backtrack was patch-shape drift while extending the negative test
  fixture; there was no semantic retreat
- I deliberately did **not** turn this into wake/process-flag validation,
  because current control/ingress signaling still carries timing asymmetry that
  has not cleared the same-owner stability bar

Cutover gate read:

- this still leaves `MeerkatMachine` on the observability side of the gate
- the slice strengthens owner-backed ingress truth, but it does not change
  write authority or freeze the remaining timing-sensitive lifecycle seams

Next likely step:

- another same-owner `MeerkatMachine` slice is still possible, but the best
  candidates are now narrower and more selective; anything timing-sensitive
  should continue to wait

## Slice 44 - MobMachine tracked run branch-condition coherence

Goal:

- promote the first real branch semantic in `MobMachine`, not just another map
  presence check
- use stored run truth that already survives into the tracked-run snapshot
- verify the branch semantics against both a synthetic malformed run and a live
  branch flow

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunBranchWithoutCondition`
  - strengthened tracked-run validation so:
    - any step carrying a non-`None` branch label must also carry
      `step_has_conditions[step_id] = true`
  - extended the synthetic tracked-run validator fixture to exercise a branch
    label on a step that claims `step_has_conditions = false`
- `meerkat-mob/src/runtime/tests.rs`
  - added a live branch-flow snapshot test:
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
  - the test runs the real `branching` flow, captures a joined `MobMachine`
    snapshot while the run is still tracked, and asserts:
    - the declared ordered step sequence survives
    - branch steps carry `step_has_conditions = true`
    - the `repair` branch label survives on both branch members

Why this slice matters:

- this is the first tracked-run invariant that ties two persisted kernel maps
  together to express real flow semantics
- it is grounded in current branch-spec rules, where branch-labelled steps are
  expected to be condition-bearing steps
- the live branch-flow snapshot is important evidence that this is not just a
  synthetic store-shape rule

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates a real persisted branch semantic rather than only
  independent per-step field presence
- the live branch-flow snapshot stayed green, which is the key signal that the
  branch/condition relationship survives into current tracked-run truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the main selection discipline was the real guardrail here: I chose a
  branch-condition rule that current spec/runtime behavior already enforces,
  instead of promoting more setup-shaped flow config just because it persists

Cutover gate read:

- `MobMachine` also remains firmly on the observability side of the gate
- the slice strengthens tracked-run semantics, but write authority still lives
  in the current flow kernel / store / actor stack

Next likely step:

- the next honest `MobMachine` slice should either relate another pair of
  persisted tracked-run maps semantically, or move sideways to a different
  owner-backed Mob region if the next flow field looks too config-shaped

## Slice 45 - MeerkatMachine ingress boundary metadata shape

Goal:

- strengthen `MeerkatMachine` with one more same-owner ingress invariant
- validate that boundary metadata still reflects the real `RuntimeIngress`
  transition shape instead of drifting into shell folklore
- keep the slice inside the runtime-owned input ledger and away from the
  still timing-sensitive multi-crate joins

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `InputBoundarySequenceWithoutRun`
    - `InputBoundarySequenceIllegalLifecycle`
  - strengthened ingress validation so:
    - `last_boundary_sequence` must imply `last_run_id`
    - queued or staged inputs must not already carry a boundary marker
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_ingress_boundary_shape_violations`

Why this slice matters:

- `BoundaryApplied` is not an annotation convenience; it is a specific ingress
  transition that only occurs after the run is staged and bound
- that means boundary metadata is a good same-owner truth surface for the
  observational machine: it should never float free of run ownership or appear
  on pre-boundary queued/staged work
- this is exactly the kind of low-noise invariant that helps us converge
  toward semantic freeze without accidentally encoding timing folklore

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ingress_boundary_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates another real ingress-owned semantic relation,
  not just field presence and queue membership
- the live runtime-backed snapshot path stayed green, which is the important
  sign that the rule matches current owner behavior rather than target-state
  wishful thinking

Backtracks encountered:

- there was no code-level semantic backtrack in the final slice
- the useful design backtrack was selection itself: I did **not** force a
  stronger boundary/current-run invariant that would have crossed into the
  known timing-sensitive join surface

Cutover gate read:

- this is still an observability-only strengthening slice
- `MeerkatMachine` is not ready to switch; the ownership model is converging,
  but the remaining multi-region lifecycle seams are still not frozen

Next likely step:

- another same-owner `MeerkatMachine` slice is still possible, but candidates
  are now increasingly selective; timing-sensitive joins should keep waiting

## Slice 46 - MobMachine branch-group dependency coherence

Goal:

- promote the next real branch semantic in `MobMachine`
- mirror the existing flow spec rule that branch-group members must share the
  same dependency set
- verify it both synthetically and against the live branch-flow snapshot path

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunBranchConflictingDependencies`
  - strengthened tracked-run validation so:
    - steps sharing the same non-`None` branch label must resolve to the same
      dependency set (set comparison, not insertion-order comparison)
  - added a focused negative test:
    `validate_mob_machine_snapshot_reports_branch_dependency_conflicts`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened the live branch-flow snapshot test so it now also asserts the
    two `repair` branch members preserve matching dependencies on `start`

Why this slice matters:

- this is a better branch semantic than simply adding more persisted fields;
  it checks a relationship the flow spec already treats as meaningful
- the rule is grounded in stored tracked-run truth, not just create-run input
  config
- using set comparison keeps the validator aligned with existing spec
  semantics rather than overfitting to field ordering

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_branch_dependency_conflicts`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates a second real branch-group semantic relation over
  persisted tracked-run state
- the live branch-flow path stayed green, which is the important sign that the
  dependency-coherence rule already survives into current runtime truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the meaningful guardrail was again selection discipline: I only promoted a
  branch-group rule that is already present in current spec validation, rather
  than inventing a new branch semantic for the tracked-run snapshot

Cutover gate read:

- `MobMachine` also remains on the observability side of the gate
- the tracked-run semantic surface is getting richer, but ownership is still
  clearly in the current flow kernel / actor / store stack

Next likely step:

- the next honest `MobMachine` slice should either extend branch-group truth
  one notch further, or pivot to another persisted flow semantic if the next
  branch rule looks too config-shaped

## Slice 47 - MeerkatMachine steer-processing intent coherence

Goal:

- promote one more same-owner ingress rule into `MeerkatMachine`
- verify that `process_requested` keeps its runtime-owned meaning rather than
  drifting into shell folklore
- prove the rule against a live steered admission path, not only a synthetic
  validator mutation

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `ProcessRequestedWithoutWake`
    - `ProcessRequestedWithoutSteerQueue`
  - strengthened ingress validation so:
    - `process_requested` must imply `wake_requested`
    - `process_requested` must imply non-empty `steer_queue`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_process_request_shape_violations`
- `meerkat-runtime/src/session_adapter.rs`
  - added a live adapter test:
    `meerkat_machine_spine_snapshot_tracks_steered_prompt_input`
  - the test drives a real prompt with `RuntimeTurnMetadata.handling_mode =
    Steer` through `accept_input_with_completion(...)` and asserts:
    - queue is empty
    - steer queue owns the input
    - `wake_requested = true`
    - `process_requested = true`
    - the admitted input snapshot preserves `HandlingMode::Steer`

Why this slice matters:

- `process_requested` is not just a convenience flag; the ingress authority
  sets it only for steer admissions that should be processed at the earliest
  admissible boundary
- that makes it a good same-owner truth surface for the observational machine:
  it should not survive without wake intent or without steer-owned work
- adding the live adapter test keeps the validator grounded in actual runtime
  behavior instead of synthetic snapshot expectations

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_steered_prompt_input`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_process_request_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates a real steer-processing semantic relation and
  has a live runtime-backed proof path for it
- this strengthens the ingress region without stepping into the known
  timing-sensitive control/runtime handoff seam

Backtracks encountered:

- there was no semantic backtrack in the final slice
- the important selection discipline was choosing `process_requested`, which is
  purely ingress-owned, instead of forcing a stronger wake/control join that
  still has timing asymmetry

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- the ownership model is still converging, but this slice is a clean example
  of owner-backed semantics becoming explicit without changing write authority

Next likely step:

- the next honest `MeerkatMachine` slice should again be same-owner and
  selective; if the next candidate crosses timing-sensitive joins, it should
  wait

## Slice 48 - MobMachine `DependencyMode::Any` branch-dependency semantics

Goal:

- promote another persisted flow semantic into `MobMachine`
- mirror the existing flow spec rule that `depends_on_mode = any` only makes
  sense when at least one dependency is branch-labelled
- verify it synthetically and against the live branch-flow tracked-run path

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunAnyDependencyModeWithoutBranchDependency`
  - strengthened tracked-run validation so:
    - any step with `DependencyMode::Any` must have at least one dependency
      whose tracked branch label is non-`None`
  - added a focused negative test:
    `validate_mob_machine_snapshot_reports_any_dependency_mode_without_branch_dependency`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened the live branch-flow snapshot test so it now also asserts:
    - `summarize` preserves `DependencyMode::Any`
    - the branch flow's dependency map still references the branch-labelled
      repair steps

Why this slice matters:

- it promotes a real semantic relation the current flow spec already enforces
- the rule is expressed entirely in persisted tracked-run truth:
  `step_dependency_modes`, `step_dependencies`, and `step_branches`
- this is exactly the sort of relation we want before cutover: not merely that
  fields exist, but that they still encode the semantics they are supposed to
  carry

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_any_dependency_mode_without_branch_dependency`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates another persisted branch/join semantic over
  tracked-run state
- the live branch-flow path stayed green, which is the key sign that the rule
  already survives into current runtime truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the useful guardrail was again selection discipline: I promoted a rule that
  already exists in spec validation, rather than inventing a new tracked-run
  branch meaning

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- the flow semantic surface is getting richer, but ownership is still clearly
  in the current flow kernel / actor / store stack

Next likely step:

- the next honest `MobMachine` slice should either deepen branch/join
  semantics one notch further or pivot to another persisted flow semantic if
  the next branch rule starts looking too config-shaped

## Slice 49 - MeerkatMachine ingress run-binding coherence

Goal:

- promote another same-owner ingress lifecycle relation into `MeerkatMachine`
- validate that active boundary-phase input states still carry the run/boundary
  metadata the ingress authority is supposed to own
- stay inside the runtime ingress ledger and avoid the timing-sensitive
  multi-crate joins

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `InputLifecycleMissingRunBinding`
    - `AppliedPendingConsumptionMissingBoundarySequence`
  - strengthened ingress validation so:
    - `Staged` and `AppliedPendingConsumption` inputs must carry `last_run_id`
    - `AppliedPendingConsumption` inputs must carry
      `last_boundary_sequence`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_ingress_run_binding_shape_violations`

Why this slice matters:

- the ingress authority does not invent `Staged` or
  `AppliedPendingConsumption` in isolation; those states come from specific
  run-bound transitions
- that makes run binding and boundary metadata part of the semantic truth for
  those lifecycle states, not optional annotations
- this is a good observational-kernel slice because it strengthens owner truth
  without reaching into the unstable control/runtime timing seam

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ingress_run_binding_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates another real ingress-owned lifecycle relation
  rather than only field presence or queue ownership
- the rule is grounded directly in the current ingress authority transitions:
  `StageDrainSnapshot` and `BoundaryApplied`

Backtracks encountered:

- there was no semantic backtrack in the final slice
- the useful guardrail was again slice selection: I avoided stronger
  current-run/coordinator joins that would have crossed into the known
  timing-sensitive boundary

Cutover gate read:

- `MeerkatMachine` remains in observability mode
- the ownership model is still converging, but this slice is another example
  of machine truth becoming explicit without changing write authority

Next likely step:

- the next honest `MeerkatMachine` slice should remain same-owner and
  selective; if it needs timing or cross-owner reconstruction, it should wait

## Slice 50 - MobMachine branch-group cardinality

Goal:

- promote the next persisted branch-group semantic into `MobMachine`
- mirror the current flow spec rule that a branch group must contain at least
  two members
- verify it synthetically while keeping the live branch-flow path green

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunBranchGroupTooSmall`
  - strengthened tracked-run validation so:
    - each non-`None` branch label must appear on at least two steps
  - added a focused negative test:
    `validate_mob_machine_snapshot_reports_branch_group_too_small`

Why this slice matters:

- branch groups are not just labels; the current spec already treats their
  cardinality as semantic truth
- this rule is expressed entirely in persisted tracked-run state through
  `step_branches`, which is exactly the kind of owner-backed relation we want
  before cutover
- it complements the earlier condition and dependency checks without inventing
  new branch meaning

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_branch_group_too_small`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates another branch-group semantic directly over
  tracked-run truth
- the broader mob test lane remained green, which is the important sign that
  the rule aligns with current runtime behavior

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the useful guardrail was again selection discipline: I promoted an existing
  spec rule instead of inventing a new tracked-run branch heuristic

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- the flow semantic surface is richer, but ownership is still clearly in the
  current flow kernel / actor / store stack

Next likely step:

- the next honest `MobMachine` slice should either continue branch/join
  semantics if the rule is already spec-backed, or pivot to a different
  persisted flow truth surface

## Slice 51 - MeerkatMachine wake-request / queued-work coherence

Goal:

- promote one more same-owner ingress rule into `MeerkatMachine`
- validate that `wake_requested` is not carried without any queued work
- stay inside `RuntimeIngressAuthority` semantics rather than crossing into
  the timing-sensitive control/runtime wake seam

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `WakeRequestedWithoutQueuedWork`
  - strengthened ingress validation so:
    - `wake_requested` requires at least one queued input in either `queue` or
      `steer_queue`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_wake_without_queued_work`

Why this slice matters:

- in the current ingress authority, `wake_requested` is raised only when queued
  work exists and is cleared when the current drain snapshot starts
- that makes `wake_requested` part of the ingress-owned "work is pending"
  carrier, not a free-floating hint
- this is exactly the kind of conservative observational-kernel rule we want:
  it strengthens machine truth without trying to prove the larger wake/control
  handoff too early

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_wake_without_queued_work`

Result:

- `MeerkatMachine` now validates another explicit ingress-owned relation
- the new rule is grounded directly in the current `RuntimeIngressAuthority`
  transitions rather than target-state cleanup

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the main restraint was not reaching for the stronger but still timing-shaped
  reverse rule (`queued work implies wake_requested`)

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this slice strengthens owner-backed truth but does not change write authority

Next likely step:

- the next honest `MeerkatMachine` slice should again stay same-owner and avoid
  crossing into the unstable multi-crate barrier/turn join

## Slice 52 - MobMachine live collection-policy durability

Goal:

- strengthen `MobMachine` against the live codebase without inventing another
  tracked-run rule too early
- prove that collection-policy kind and quorum threshold really survive into the
  tracked-run snapshot for a runtime-backed flow

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added a live tracked-run test:
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
  - the new test runs a real quorum collection flow and asserts the joined
    `MobMachine` snapshot preserves:
    - ordered steps
    - dependency map
    - dependency mode
    - branch/condition flags
    - collection policy kind
    - quorum threshold

Why this slice matters:

- the validator already checks collection-policy / quorum-threshold shape, but
  the honest question was whether those facts are truly durable in the current
  run aggregate
- this slice answers that with a live runtime-backed proof instead of guessing
- it keeps the Mob side moving while avoiding a speculative new semantic rule
  that might still be config folklore

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`

Result:

- `MobMachine` now has live evidence that quorum collection-policy truth
  survives into tracked-run snapshots, not just synthetic validator coverage

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the deliberate choice here was methodological: use a live durability proof
  rather than promote a new tracked-run invariant before the persistence shape
  had proved itself

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice improves confidence in existing tracked-run facts, but it is still
  validation scaffolding rather than switched write authority

Next likely step:

- the next honest `MobMachine` slice should either promote another rule only if
  it is clearly persisted and spec-backed, or continue strengthening live
  runtime-backed proofs for the facts already in the validator

## Slice 53 - MeerkatMachine reverse contributor coverage

Goal:

- strengthen the ingress run-binding surface in `MeerkatMachine`
- validate the reverse direction of current-run contributor ownership:
  run-bound contributor-compatible inputs must still appear in the current run
  contributor set
- stay entirely inside the ingress authority's own state instead of crossing
  into the timing-sensitive turn/runtime boundary

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `CurrentRunBoundInputMissingContributor`
  - strengthened ingress validation so:
    - any admitted input whose `last_run_id` matches `current_run_id` and whose
      lifecycle is contributor-compatible (`Staged`, `Applied`, or
      `AppliedPendingConsumption`) must appear in
      `current_run_contributors`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_missing_run_bound_contributor`

Why this slice matters:

- we were already validating `contributors -> inputs`, but not the reverse
  relation
- in the current ingress authority, contributor-compatible run-bound inputs are
  the canonical membership set for the active drain snapshot
- that makes this a real ownership rule, not just a cleanup preference

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_missing_run_bound_contributor`

Result:

- `MeerkatMachine` now validates a fuller ingress-owned picture of run-bound
  contributor truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the useful guardrail was keeping the rule purely ingress-owned rather than
  overreaching into turn-execution state

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- the rule strengthens owner-backed truth without moving write authority

Next likely step:

- the next honest `MeerkatMachine` slice should keep targeting same-owner
  ingress/ops/drain facts until the remaining gaps are mostly cross-crate joins

## Slice 54 - MobMachine live `Any` collection-policy durability

Goal:

- keep strengthening `MobMachine` against the live codebase without promoting a
  speculative new tracked-run invariant
- prove that the `CollectionPolicy::Any` variant survives into the joined
  tracked-run snapshot with the expected kind/threshold shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added a live tracked-run test:
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
  - the new test runs a real `CollectionPolicy::Any` flow and asserts the
    joined `MobMachine` snapshot preserves:
    - `RunCollectionPolicyKind::Any`
    - zero quorum threshold

Why this slice matters:

- the validator already checks the generic shape relation for collection policy
  kinds and quorum thresholds
- this slice adds live evidence for the `Any` variant specifically, complementing
  the existing quorum-backed durability proof
- that is the cautious way to move the Mob side forward while the next semantic
  promotion is still under evaluation

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`

Result:

- `MobMachine` now has live runtime-backed evidence for both quorum and any
  collection-policy durability in tracked-run snapshots

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the deliberate choice here was again methodological: extend live durability
  evidence before promoting another tracked-run law

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice improves confidence in existing tracked-run facts, but it is still
  diagnostic scaffolding rather than switched write authority

Next likely step:

- the next honest `MobMachine` slice should either promote another clearly
  persisted and spec-backed rule, or continue widening live proof coverage for
  the tracked-run facts already modeled

## Slice 55 - MeerkatMachine wait-all carrier coherence

Goal:

- expose one more runtime-owned fact inside the Meerkat ops slice without
  crossing a timing-sensitive join
- make the hidden `wait_all` carrier visible so the joined validator can catch
  authority/carrier drift instead of only watching aggregate wait metadata

What landed:

- `meerkat-runtime/src/ops_lifecycle.rs`
  - extended `RuntimeOpsDiagnosticSnapshot` with `pending_wait_present`
  - sourced it directly from `ShellState.pending_wait.is_some()`
- `meerkat-runtime/src/meerkat_machine.rs`
  - extended `MeerkatOpsSnapshot` with `pending_wait_present`
- `meerkat-runtime/src/session_adapter.rs`
  - threaded the new field into the runtime spine snapshot
  - strengthened the live `wait_all` snapshot test to require the carrier while
    the wait future is active
- `meerkat/src/meerkat_machine.rs`
  - added two new invariants:
    - `PendingWaitCarrierWithoutWaitRequest`
    - `WaitRequestMissingPendingWaitCarrier`
  - extended the focused negative ops test to prove both sides of the pairing

Why this slice matters:

- `wait_all` installs the authority-owned wait request and the shell-owned
  pending wait sender under the same ops registry write lock
- that makes this a good same-owner slice for the cutover gate: stronger than a
  pure aggregate count, but still grounded in stable current ownership
- it also makes one more hidden carrier explicit before we start talking about
  switch readiness

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_runtime_ops_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_wait_all_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`

Result:

- `MeerkatMachine` now validates the exact pairing between active `wait_all`
  authority and the pending wait carrier

Backtracks encountered:

- the useful backtrack happened before code landed: I rejected a stronger
  ingress lifecycle/current-run rule because recovery can legitimately clear
  `current_run` while keeping applied inputs
- the wait-all carrier slice was chosen specifically because it avoids that
  timing/recovery trap

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger owner-backed truth, not a write-authority move

Next likely step:

- keep targeting same-owner Meerkat regions until the remaining gaps are mostly
  cross-crate joins or explicit switch preparation work

## Slice 56 - MobMachine live `All` collection-policy durability

Goal:

- finish live durability coverage for the full `{All, Any, Quorum}` tracked
  collection-policy set
- improve confidence in existing tracked-run policy/quorum rules without
  promoting a speculative new invariant

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added a live tracked-run test:
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
  - the new test runs a real `CollectionPolicy::All` flow and asserts the
    joined `MobMachine` snapshot preserves:
    - `RunCollectionPolicyKind::All`
    - zero quorum threshold

Why this slice matters:

- the validator already enforces the generic shape relation between collection
  policy kind and quorum threshold
- this closes the remaining live proof gap for the three concrete policy
  variants actually stored in tracked run truth
- that is still the safer Mob move while the next semantic promotion is under
  evaluation

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`

Result:

- `MobMachine` now has live runtime-backed durability evidence for
  `CollectionPolicy::{All, Any, Quorum}`

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the deliberate methodological choice was to prefer a live proof over another
  new tracked-run law until the next persisted relation is clearly worth
  promoting

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice raises confidence in already-modeled tracked-run truth without
  changing ownership or write authority

Next likely step:

- the next honest `MobMachine` slice should either promote another clearly
  persisted and spec-backed relation, or continue widening live proof coverage
  where the semantic promotion bar is not met yet

## Slice 57 - MeerkatMachine wait-all request ID pairing

Goal:

- strengthen the `wait_all` carrier slice without leaving the same-owner ops
  boundary
- prove that the shell-owned pending wait carrier not only exists, but tracks
  the same `WaitRequestId` as the authority-owned wait request

What landed:

- `meerkat-runtime/src/ops_lifecycle.rs`
  - extended `RuntimeOpsDiagnosticSnapshot` with `pending_wait_request_id`
  - sourced it directly from `ShellState.pending_wait.wait_request_id`
- `meerkat-runtime/src/meerkat_machine.rs`
  - extended `MeerkatOpsSnapshot` with `pending_wait_request_id`
- `meerkat-runtime/src/session_adapter.rs`
  - threaded the new field into the runtime spine snapshot
  - strengthened the live `wait_all` snapshot test to require exact ID
    agreement between the carrier and authority
- `meerkat/src/meerkat_machine.rs`
  - added new invariants:
    - `PendingWaitCarrierShapeMismatch`
    - `PendingWaitCarrierRequestMismatch`
  - extended the focused negative ops test to prove request-ID drift is caught

Why this slice matters:

- this is the same semantic seam as Slice 55, but one level stronger
- the carrier and authority live under the same registry write lock, so exact
  ID agreement is real current truth rather than a speculative cross-crate join
- it turns the hidden wait sender from a boolean presence marker into a typed
  obligation we can reason about during cutover

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_wait_all_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now validates exact agreement between the active `wait_all`
  request and the pending wait carrier request ID

Backtracks encountered:

- there was no code-level backtrack in the final Meerkat slice
- the guardrail was methodological: stay on the same-owner ops path instead of
  reaching for a stronger but timing-sensitive ingress or barrier invariant

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is still convergence work over current owner truth, not a switch to
  write authority

Next likely step:

- keep mining same-owner Meerkat slices until the remaining unknowns are mostly
  explicit cross-crate joins or cutover-preparation work

## Slice 58 - MobMachine healthy flow failure-counter durability

Goal:

- move the Mob side one step forward on tracked failure-counter truth without
  promoting a speculative new invariant
- verify that the joined tracked-run snapshot carries stable zeroed failure
  counters on a healthy live flow

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
  - the active tracked-run assertion now requires:
    - `failure_count == 0`
    - `consecutive_failure_count == 0`

Why this slice matters:

- the tracked-run snapshot already carries both counters, and the validator
  already knows their basic ordering relation
- this adds live joined-snapshot evidence on the stable non-failing path rather
  than inventing a new law or overreaching into cleanup timing
- it keeps the Mob side moving while respecting the cutover gate’s “current
  owner truth first” rule

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live tracked-run durability evidence that a healthy
  active flow keeps both failure counters cleared

Backtracks encountered:

- I first tried a stronger live proof on a retrying failing flow
- that timed out waiting for failure counters to remain visible on the joined
  tracked-run path, which is a good signal that the current tracker/store join
  is not yet stable enough for that proof
- I backed that out and replaced it with the safer healthy-flow durability
  assertion instead of weakening the timing model or forcing a flaky test

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice improves confidence in an already-modeled fact while explicitly
  recording where the failure-counter proof surface is still too timing-shaped

Next likely step:

- the next honest `MobMachine` slice should either prove another stable tracked
  fact on the healthy path, or revisit failure counters only after the
  tracker/store lifetime is modeled more explicitly

## Slice 59 - MeerkatMachine legacy detached-wake latch consistency

Goal:

- tighten one real Meerkat carrier seam instead of only observing it
- make the legacy detached-wake latch match its documented meaning:
  `signaled` means “wake sent and not yet consumed”

What landed:

- `meerkat-runtime/src/runtime_loop.rs`
  - introduced `clear_legacy_detached_wake_signal(...)`
  - the legacy idle wake arm now clears both `pending` and `signaled` after
    successfully injecting the continuation input
  - added a focused unit test proving the helper resets both flags
- `meerkat/src/meerkat_machine.rs`
  - added `DetachedWakeSignaledWithoutPending`
  - the joined validator now rejects the stale latch state
    `detached_wake_pending == false && detached_wake_signaled == true`
  - extended the focused negative ops test to prove that stale legacy state is
    now treated as invalid

Why this slice matters:

- this is not just a prettier observer rule; it corrects a real implementation
  inconsistency in the legacy detached-wake path
- before this slice, the runtime loop could consume a legacy detached wake and
  leave `signaled = true`, which no longer matched the documented meaning of
  the latch
- the validator tightening is only justified because the implementation now
  excludes that stale state

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib clear_legacy_detached_wake_signal_resets_pending_and_signaled`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ops_shape_violations`

Result:

- the legacy detached-wake carrier is now internally self-consistent
- `MeerkatMachine` can safely reject a stale signaled-without-pending snapshot

Backtracks encountered:

- the interesting backtrack here was architectural, not syntactic
- while tracing the carrier I noticed the feed-backed runtime does not actually
  use the legacy `signaled` latch on the active path, which is why I kept this
  slice narrowly scoped to legacy self-consistency instead of inventing a
  broader live invariant too early

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this slice improves a real carrier seam, but it does not yet move write
  authority under the joined machine

Next likely step:

- either refine another same-owner Meerkat seam such as completion-waiter
  carrier shape, or explicitly map how detached wake should behave under the
  feed-backed path before promoting stronger live invariants there

## Slice 60 - MobMachine branch-flow collection-policy durability

Goal:

- keep the Mob side moving without repeating the retry/cleanup timing mistake
- prove that branch flows preserve full tracked collection-policy state, not
  just branch/dependency shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
  - the live branch-flow snapshot must now also preserve:
    - default `RunCollectionPolicyKind::All` for every branch-flow step
    - zero quorum thresholds for every non-quorum branch-flow step

Why this slice matters:

- we already had live proofs for collection-policy durability on single-step
  `{All, Any, Quorum}` flows
- this extends that evidence to a multi-step branch flow, which is a better
  proxy for real flow-kernel shape than another isolated policy demo
- it keeps the Mob side on persisted tracked-run truth rather than retry or
  cleanup timing

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`

Result:

- `MobMachine` now has live durability evidence that branch flows preserve both
  branch semantics and collection-policy kernel state together

Backtracks encountered:

- there was no new code-level backtrack in the final Mob slice
- the restraint was methodological: use branch-flow durability proof rather
  than force a new tracked-run invariant on a timing-sensitive path

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice adds breadth to persisted tracked-run evidence, but it does not
  change the underlying authority boundary yet

Next likely step:

- keep the Mob side on durable tracked-run facts until the remaining gaps are
  clearly semantic rather than cleanup-shaped

## Slice 61 - MeerkatMachine detached-wake owner proof at the ops registry seam

Goal:

- complement the legacy detached-wake runtime-loop fix with a same-owner proof
  at the registry seam
- verify that a terminal `BackgroundToolOp` arms pending wake state without
  mutating the legacy `signaled` latch directly

What landed:

- `meerkat-runtime/src/ops_lifecycle.rs`
  - added `background_terminal_sets_detached_wake_pending_without_signaled_latch`
  - the test wires a real `DetachedWakeState` into the registry, completes a
    `BackgroundToolOp`, and asserts:
    - `pending == true`
    - `signaled == false`

Why this slice matters:

- Slice 59 fixed consumption semantics in the legacy runtime loop
- this slice proves the producer side of the same carrier
- together they make the detached-wake latch understandable at both ends:
  ops lifecycle arms pending, runtime loop consumes and clears the full latch

Verification:

- `cargo test -p meerkat-runtime --lib background_terminal_sets_detached_wake_pending_without_signaled_latch`

Result:

- `MeerkatMachine` now has direct owner-side evidence for how the detached-wake
  carrier is produced before the joined validator reasons about it

Backtracks encountered:

- there was no code-level backtrack in the final Meerkat slice
- the discipline was choosing a same-owner registry proof instead of reaching
  for a broader live feed-path invariant that the current runtime does not yet
  justify

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger owner-backed evidence, not a switch of write authority

Next likely step:

- continue only on Meerkat seams whose producer and consumer meanings can both
  be grounded in current owner code

## Slice 62 - MobMachine completed-run step-status contract invariant

Goal:

- mirror one more existing `FlowRunMachine` contract invariant inside the joined
  `MobMachine`
- use only fields already projected from the durable tracked-run kernel state

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `TrackedRunCompletedRunHasNonCompletedStep`
  - `MobMachine` now rejects `MobRunStatus::Completed` runs that still report a
    step status other than `Completed` or `Skipped`
  - extended the focused synthetic tracked-run validator harness to prove the
    new violation is surfaced

Why this slice matters:

- the generated flow-run contract already says completed runs must contain only
  completed or skipped steps
- this slice promotes a genuine machine invariant, not just another persisted
  field
- it uses stable tracked-run state we already project (`status` +
  `step_statuses`) without reaching into cleanup timing

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`

Result:

- `MobMachine` now mirrors one more formal flow-run invariant using current
  durable run truth

Backtracks encountered:

- no code-level backtrack in the final Mob slice
- the methodological choice was to add a contract-derived invariant rather than
  another policy or retry-shaped live proof

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens semantic convergence, but the current tracked-run boundary
  is still read/validate only

Next likely step:

- keep pulling contract-derived tracked-run invariants into `MobMachine` so
  long as they only depend on already-projected durable state

## Slice 63 - MeerkatMachine recycle preserves live completion waiters and drops stale ones

Goal:

- make the recycle/recovery seam explicit in the Meerkat spine instead of
  leaving it only in detached control-plane tests
- prove both sides of completion-waiter reconciliation:
  - active queued waiters survive recycle
  - stale waiter entries are terminated and removed

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle`
    to prove the joined Meerkat spine still shows the active queued waiter
    after `recycle()`
  - added
    `meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters`
    to seed one stale internal waiter, recycle the runtime, and prove the
    stale waiter is terminated with `"recycled input no longer pending"` while
    the live queued waiter survives

Why this slice matters:

- recycle/recovery is a high-value cutover seam because it is where carrier
  truth and ledger truth can silently diverge
- the adapter already had the reconciliation logic; this slice promotes that
  logic into the observable MeerkatMachine build instead of trusting it
  implicitly

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that recycle preserves
  live completion-waiter linkage and reconciles stale waiter carrier state

Backtracks encountered:

- no machine-level backtrack was needed after the slice direction was chosen
- the only deliberate compromise was seeding the stale waiter directly through
  the completion registry because the public API does not currently expose a
  way to manufacture that recovery-only state

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens a recovery seam but does not move write authority

Next likely step:

- keep choosing Meerkat slices at lifecycle handoff seams where producer and
  consumer truth are already explicit in current owner code

## Slice 64 - MobMachine live proof of branch-group durability

Goal:

- strengthen the live branch-flow tracked-run proof without inventing a new
  invariant
- confirm that branch-group facts already enforced by the validator survive the
  real tracked-run projection path

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    assert:
    - the `repair` branch group still contains both branch members
    - both members preserve the same dependency set in tracked-run state

Why this slice matters:

- the branch-group invariants already exist in `MobMachine`, but they were
  still mostly justified by synthetic harnesses
- this adds a live durability proof that the real tracked-run snapshot carries
  the same branch membership and dependency coherence the validator expects

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that branch-group structure
  survives the real flow-run projection path

Backtracks encountered:

- no code-level backtrack was needed in the final Mob slice
- the restraint was intentionally keeping this as a live proof instead of
  promoting another new invariant before the tracked-run durability bar clearly
  justified it

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this increases confidence in current tracked-run durability without shifting
  write authority

Next likely step:

- continue with durable tracked-run facts or lifecycle seams that can be proved
  against live owner state without leaning on cleanup timing

## Slice 65 - MeerkatMachine recover reconciles completion waiters

Goal:

- test whether `recover()` handles completion waiters with the same carrier
  discipline as `recycle()`
- use the Meerkat spine to distinguish “current behavior we should preserve”
  from “hidden lifecycle bug we need to fix”

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover`
    to prove active queued waiters survive a recover handoff
  - added
    `meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters`
    to seed one stale waiter and check that recover drops it
  - fixed `RuntimeSessionAdapter::recover()` so it now reconciles completion
    waiters against `active_input_ids()` and terminates stale entries with
    `"recovered input no longer pending"`

Why this slice matters:

- `CompletionRegistry::resolve_not_pending()` already documented recovery as a
  valid reconciliation site, but the adapter only used it for recycle
- the new stale-waiter test failed red first, which means this slice closed a
  real lifecycle bug instead of merely adding another observer

Verification:

- red proof before the fix:
  - `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters`
- green after the fix:
  - `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters`
  - `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover`
  - `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that recover preserves live
  completion waiters and clears stale carrier entries instead of letting them
  hang indefinitely

Backtracks encountered:

- this slice intentionally started test-first and failed red
- the failure was useful and precise: recover left a stale waiter visible in
  the spine (`input_count == 2` instead of `1`)
- the final fix reused the recycle reconciliation pattern rather than inventing
  a new recovery-only path

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is exactly the kind of lifecycle seam bug we want to burn down before
  considering a switch

Next likely step:

- keep selecting Meerkat slices where lifecycle handoff and carrier cleanup can
  be expressed through current owner code and tested red-to-green

## Slice 66 - MobMachine live proof of any-join branch durability

Goal:

- strengthen the live branch-flow proof around the `depends_on_mode=any` join
  step without promoting a new invariant prematurely

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    prove that the `summarize` join step:
    - still has at least one dependency in tracked-run state
    - depends only on branch-labelled steps

Why this slice matters:

- `MobMachine` already validates the synthetic rule that `DependencyMode::Any`
  requires at least one branch-backed dependency
- this slice shows that the real tracked-run projection keeps enough durable
  branch information for that rule to be meaningful in live state

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that the any-join branch rule
  survives the real tracked-run projection path

Backtracks encountered:

- no code-level backtrack in the final Mob slice
- the deliberate restraint was keeping this as a live durability proof instead
  of promoting another invariant before the tracked-run boundary justified it

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in current tracked-run durability without shifting
  write authority

Next likely step:

- keep `MobMachine` focused on persisted tracked-run facts or live durability
  proofs that do not depend on cleanup timing

## Slice 67 - MeerkatMachine preserves active wait_all across recover

Goal:

- prove that `recover()` preserves the active `wait_all` carrier coherently
  when the same ops registry remains the canonical owner

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recover`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives recover
    - the shell-owned pending wait carrier survives recover
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target operation
      completes

Why this slice matters:

- this is the ops-lifecycle analogue of the completion-waiter recovery work
- it verifies a real lifecycle handoff at the Meerkat seam without inventing a
  new owner or a new reducer path
- the current code path says recover replays the runtime driver but keeps the
  same live ops registry, so the observer needs to prove that the wait carrier
  remains coherent under that ownership model

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recover`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that active `wait_all` state
  survives recover coherently instead of drifting across the authority/shell
  seam

Backtracks encountered:

- the detached-wake reset/destroy idea was intentionally not promoted into a
  rule because it was not yet clear that the current attachment lifecycle made
  that a single-owner semantic fact
- switching to `wait_all` was the better call because the ownership story was
  already explicit in the ops authority

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this slice strengthens lifecycle-handoff confidence, but it does not change
  write authority

Next likely step:

- keep selecting Meerkat seams where lifecycle handoff can be expressed
  directly through current owner code and where a green result actually reduces
  cutover risk

## Slice 68 - MobMachine live proof of active tracked-run step status coverage

Goal:

- strengthen the live tracked-run proof around step-status durability without
  depending on the cleanup-sensitive terminal visibility window

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    prove that the active tracked branch run:
    - surfaces at least one materialized step status
    - never exposes a step-status entry outside the ordered-step universe

Why this slice matters:

- the branch-flow tracked run already exposes durable ordered-step truth
- this slice proves that the live status projection stays inside kernel-known
  step identity without assuming full coverage or terminal visibility

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active tracked runs preserve
  materialized step-status identity across the joined snapshot

Backtracks encountered:

- the first attempt tried to prove completed-run terminal status shape through
  the joined snapshot
- that went red for a good reason: terminal tracked runs do not remain visible
  long enough to be a stable proof surface before local cleanup drains them
- the second attempt tried to prove full step-status coverage for all ordered
  steps while the run was still active
- that also went red: active tracked runs only materialize status entries for
  the steps that have actually entered the ledger so far
- the final slice backed off to the active-run window and to the smaller fact
  the current code really owns: materialized status keys stay inside the
  ordered-step universe

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run status durability without moving
  write authority

Next likely step:

- continue selecting Mob slices that either prove existing invariants against
  live tracked-run state or surface new durable facts only after that live
  proof exists

## Slice 69 - MeerkatMachine preserves active wait_all across recycle

Goal:

- prove that `recycle()` preserves the active `wait_all` carrier coherently
  when the same ops registry remains the canonical owner

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives recycle
    - the shell-owned pending wait carrier survives recycle
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target
      operation completes

Why this slice matters:

- this is the recycle-side counterpart to the recover `wait_all` handoff proof
- it verifies another Meerkat lifecycle transition where driver state is
  rebuilt while the ops registry remains the owner of the active wait request

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that active `wait_all` state
  survives both recover and recycle coherently instead of drifting across the
  authority/shell seam

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was not promoting a reset/destroy rule before the current
  ownership semantics made that boundary explicit

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens lifecycle-handoff confidence, but it does not change write
  authority

Next likely step:

- keep selecting Meerkat seams where lifecycle handoff is explicit in current
  owner code and where a green result materially reduces cutover risk

## Slice 70 - MobMachine live proof of healthy collection-policy run durability

Goal:

- strengthen the live tracked-run proof for collection-policy flows without
  assuming terminal visibility or full active-step coverage

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape` to
    prove that the active tracked collection-policy run:
    - keeps `failure_count == 0`
    - keeps `consecutive_failure_count == 0`
    - only exposes step-status entries whose keys stay inside the ordered-step
      universe

Why this slice matters:

- the healthy single-step tracked flow already showed zeroed failure counters;
  this extends the same durability proof to a tracked run that also carries
  collection-policy metadata
- it stays on currently materialized tracked-run truth instead of assuming more
  status coverage than the live projection actually guarantees

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy collection-policy tracked
  runs preserve both zeroed failure counters and kernel-bounded materialized
  step-status identity

Backtracks encountered:

- no new code-level backtrack in the final slice
- this slice deliberately reused the smaller active-run status identity fact
  learned from the previous branch-flow backtracks

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in collection-policy tracked-run durability without
  moving write authority

Next likely step:

- keep extending live durability proofs only where the joined snapshot has
  already demonstrated stable owner-backed truth

## Slice 71 - MeerkatMachine preserves active wait_all after reset

Goal:

- prove that `reset()` preserves the active `wait_all` carrier coherently
  when ops lifecycle remains the canonical owner

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_reset`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives reset
    - the shell-owned pending wait carrier survives reset
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target
      operation completes

Why this slice matters:

- this completes the current Meerkat lifecycle trio for active `wait_all`
  carrier survival across recover, recycle, and reset
- it stays grounded in present owner code: reset clears queued input work, but
  it does not replace the ops lifecycle registry

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_reset`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that active `wait_all` state
  survives the current reset path coherently instead of drifting across the
  authority/shell seam

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was intentional: this stays observational about current reset
  semantics rather than asserting a stronger desired policy for destroy

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this materially improves lifecycle-handoff coverage, but it does not change
  write authority

Next likely step:

- keep selecting Meerkat lifecycle seams where current owner behavior is clear
  enough to prove without inventing target-state semantics

## Slice 72 - MobMachine live proof of healthy any-policy run durability

Goal:

- strengthen the live tracked-run proof for `CollectionPolicy::Any` without
  assuming more status coverage than the current projection materializes

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
    to prove that the active tracked any-policy run:
    - keeps `failure_count == 0`
    - keeps `consecutive_failure_count == 0`
    - surfaces at least one materialized step status
    - only exposes step-status entries whose keys stay inside the ordered-step
      universe

Why this slice matters:

- it extends the same healthy tracked-run durability proof to another
  collection-policy variant without widening the semantic claims
- it reuses the smaller active-run status identity fact that the previous
  branch-flow backtracks established as stable

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy any-policy tracked runs
  preserve zeroed failure counters and kernel-bounded materialized step-status
  identity

Backtracks encountered:

- no new code-level backtrack in the final slice
- the deliberate restraint was keeping this on healthy active-run durability
  instead of reopening terminal or full-coverage assumptions

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run durability without moving write
  authority

Next likely step:

- continue extending live durability proofs only where the joined snapshot has
  already demonstrated stable owner-backed truth

## Slice 159 - MeerkatMachine proves plain reset clears the steer lane but preserves wait_all

Goal:

- extend the plain registered steer lane into `reset()`
- verify whether reset follows the plain destroy steer behavior or the plain
  retire backtrack on current owner code

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_reset_clears_steered_waiter_and_queue_but_preserves_wait_all`
  - the new test proves that plain `reset()`:
    - starts from a pending steered input in `inputs.steer_queue`
    - clears the steered completion waiter immediately
    - clears the steer queue immediately
    - preserves the ops-owned `wait_all` carrier and `WaitRequestId`
      agreement until the background operation settles

Why this slice matters:

- it closes the remaining ambiguity between plain `reset()` and the already
  divergent plain `retire()` / plain `destroy()` steer paths
- it strengthens the lifecycle map with another direct owner-backed steer-lane
  teardown case instead of assuming all teardown families behave alike

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_clears_steered_waiter_and_queue_but_preserves_wait_all`

Result:

- `MeerkatMachine` now has direct evidence that plain reset clears the steer
  lane immediately while still preserving the separate ops wait carrier

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was to keep this on the plain registered path instead of
  speculating about the attached steer-lane reset family before the owner path
  proves it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-side switch signal

Next likely step:

- keep selecting distinct steer-lane lifecycle seams where current owner code
  exposes a real semantic answer rather than extrapolating from nearby
  teardown families

## Slice 160 - MobMachine adds a live branch-fallback durability proof

Goal:

- extend active multi-step tracked-run coverage beyond the plain branch and
  shared-path shapes, but only if the live aggregate sustains the same durable
  bar

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_branch_fallback_shape`
  - the new live proof shows an active `branch_fallback` run preserves:
    - durable `flow_id == branch_fallback` and `Running` status
    - schema version `4` and non-terminal shape
    - zero frame/loop structure
    - the full ordered-step and dependency maps for the fallback branch family
    - `DependencyMode::Any` on the `join` step
    - branch labels for both repair candidates
    - default collection policy kinds and zero quorum thresholds
    - zeroed retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it adds a distinct multi-step conditional family where one branch is allowed
  to fail later, without forcing failure timing into the machine proof surface
- it keeps `MobMachine` on the durable tracked-run aggregate instead of
  elevating runtime failure mechanics into new machine-owned fields

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_fallback_shape`

Result:

- `MobMachine` now has owner-backed evidence that the active branch-fallback
  family collapses onto the same durable tracked-run surface used for the
  other active multi-step families

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to stop at the active durable shape and avoid
  promoting failure-counter or terminal-visibility claims that still depend on
  timing

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-authority switch signal

Next likely step:

- keep extending active multi-step coverage only where the live tracked-run
  aggregate sustains the same durable bar without leaning on cleanup or
  failure timing

## Slice 161 - MeerkatMachine proves plain stop clears the steer lane but preserves wait_all

Goal:

- extend the plain registered steer lane into `stop_runtime_executor()`
- verify whether stop follows the plain reset/destroy steer behavior or the
  plain retire backtrack on current owner code

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_stop_runtime_executor_clears_steered_waiter_and_queue_but_preserves_wait_all`
  - the new test proves that plain `stop_runtime_executor()`:
    - starts from a pending steered input in `inputs.steer_queue`
    - clears the steered completion waiter immediately
    - clears the steer queue immediately
    - preserves the ops-owned `wait_all` carrier and `WaitRequestId`
      agreement until the background operation settles

Why this slice matters:

- it closes the remaining ambiguity between plain `stop_runtime_executor()`
  and the already divergent plain `retire()` steer path
- it strengthens the lifecycle map with another direct owner-backed
  steer-lane teardown case rather than assuming all teardown families behave
  alike

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_clears_steered_waiter_and_queue_but_preserves_wait_all`

Result:

- `MeerkatMachine` now has direct evidence that plain stop clears the steer
  lane immediately while still preserving the separate ops wait carrier

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was to keep this on the plain registered path instead of
  speculating about the attached steer-lane stop family before the owner path
  proves it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-side switch signal

Next likely step:

- keep selecting distinct steer-lane lifecycle seams where current owner code
  exposes a real semantic answer rather than extrapolating from nearby
  teardown families

## Slice 162 - MobMachine backtracks active root-frame status visibility

Goal:

- test whether active root-frame runs now surface non-empty materialized
  `step_statuses` strongly enough to promote that fact into the durable
  tracked-run proof bar

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - tried strengthening
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
    to require non-empty active `step_statuses`
  - the live runtime rejected that claim, so the assertion was removed and the
    test returned to the smaller stable fact:
    - if `step_statuses` are present, their keys stay inside the ordered-step
      universe

Why this slice matters:

- it gives us a direct answer about another tempting active-run surface:
  root-frame status visibility is still too timing-shaped to promote
- that is exactly the kind of claim we want to reject now instead of carrying
  a cleaner parallel architecture into cutover work

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`

Result:

- `MobMachine` keeps the smaller bounded-status identity fact for active
  root-frame runs, and does not yet promote non-empty active status
  visibility into machine-owned truth

Backtracks encountered:

- yes: the stronger hypothesis failed immediately on the live runtime
- the fix was to back out the claim rather than weaken the runtime-facing
  family into a timing-dependent story

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is useful convergence because it removes a false promotion before any
  write-side switch

Next likely step:

- keep extending active family coverage only where the live tracked-run
  aggregate sustains the stronger fact without leaning on materialization
  timing

## Slice 155 - MeerkatMachine backtracks invalid attached steer no-wake assumptions

Goal:

- correct the observational model after discovering that attached steered
  inputs are not valid queue-only no-wake admissions on the current runtime

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_attached_steered_prompt_requests_immediate_processing`
  - removed the older attached-loop steered destroy proof that relied on
    `accept_input_without_wake(...)` for a steered prompt
  - the new test proves that attached steered admission:
    - requests immediate processing through the live attached executor path
    - routes one control command through the executor seam
    - moves the runtime into `Running`
    - binds control and ingress to the same active run
    - does not remain in `inputs.steer_queue`
    - completes normally through the attached loop and returns to `Attached`

Why this slice matters:

- it fixes an invalid observational assumption before cutover work hardens
  around it
- the old tests were staging an impossible state by treating a steered prompt
  as queue-only work on an attached runtime
- the corrected proof now reflects the current owner boundary: attached
  steered inputs become active immediately rather than staying in a dormant
  steer queue

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_attached_steered_prompt_requests_immediate_processing`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached steered
  admission is an immediate-processing seam, not a valid queue-only no-wake
  seam

Backtracks encountered:

- the focused run showed the new attached steered retire proof was invalid for
  the same reason as an older attached steered destroy proof: both depended on
  `accept_input_without_wake(...)` even though steered admission requests
  immediate processing
- the first replacement proof still assumed the cleaner story that steered
  admission would not touch the executor control seam; the live runtime showed
  one control command, so the proof was tightened to match current behavior
- the correct response was to remove the impossible story rather than weaken
  the helper or fake the state

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a healthy convergence backtrack, not a write-authority switch signal

Next likely step:

- keep selecting lifecycle seams only where the live admission path can be
  observed without inventing impossible preconditions

## Slice 156 - MobMachine adds a live two-step tracked-run shape proof

Goal:

- extend `MobMachine` beyond single-step and dispatch-family shapes by proving
  that a linear two-step flow also survives as durable tracked-run truth in the
  joined snapshot

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added `test_capture_mob_machine_snapshot_tracks_live_two_step_shape`
  - the new test proves that an active `two_step` run preserves:
    - durable `flow_id == two_step`
    - `Running` status and non-terminal shape
    - schema version `4`
    - no persisted frame or loop structure
    - ordered steps `first -> second`
    - the declared dependency map where `second` depends on `first`
    - durable dependency modes, condition flags, branch map, collection policy
      kinds, and quorum thresholds for both steps
    - zeroed failure/retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it moves `MobMachine` beyond the single-step/dispatch family and shows the
  tracked-run aggregate can preserve a basic multi-step dependency ledger
  without timing-sensitive cleanup assumptions

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_two_step_shape`

Result:

- `MobMachine` now has owner-backed evidence that a simple linear two-step flow
  survives as durable tracked-run kernel truth in the joined snapshot

Backtracks encountered:

- none in the final slice
- the key restraint was sticking to durable maps and bounded status identity
  instead of assuming richer active-step materialization than the live runtime
  guarantees

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this broadens tracked-run coverage, but it is still not a write-side switch
  signal

Next likely step:

- keep moving one small step outward from the dispatch matrix into other durable
  tracked-run families, only when the live aggregate sustains them cleanly

## Slice 157 - MeerkatMachine promotes the attached-steer backtrack into a validator rule

Goal:

- turn the attached steer admission correction into an executable
  `MeerkatMachine` rule so the joined machine rejects the impossible idle
  `Attached + steer_queue` state directly

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added `AttachedRuntimeStillHasSteerQueuedWork`
  - `validate_meerkat_machine_snapshot(...)` now rejects snapshots where
    `control.phase == Attached` while `inputs.steer_queue` is non-empty
  - added the focused negative proof
    `validate_meerkat_machine_snapshot_reports_attached_steer_queue`

Why this slice matters:

- the previous Meerkat backtrack showed that attached steered admission goes
  active immediately and routes through the executor seam instead of remaining
  as dormant steer-queued work
- promoting that into the validator keeps the executable machine honest and
  prevents the old impossible state from quietly re-entering later slices

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_attached_steer_queue`

Result:

- `MeerkatMachine` now rejects the impossible attached idle steer-queue state
  as an explicit invariant violation

Backtracks encountered:

- none in the final slice
- this slice is the codified follow-through from the earlier attached-steer
  backtrack rather than a new semantic dispute

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens the joined machine model but does not yet imply any
  write-side switch

Next likely step:

- continue promoting only those Meerkat invariants that are now stable across
  multiple owner-backed observations

## Slice 158 - MobMachine adds a live shared-path conditional two-step proof

Goal:

- extend `MobMachine` from linear two-step tracked-run truth into a conditional
  two-step flow while staying on durable stored maps rather than runtime
  timing folklore

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added `test_capture_mob_machine_snapshot_tracks_live_shared_path_shape`
  - the new test proves that an active `shared_paths` run preserves:
    - durable `flow_id == shared_paths`
    - `Running` status and non-terminal shape
    - schema version `4`
    - no persisted frame or loop structure
    - ordered steps `start -> follow`
    - dependency map where `follow` depends on `start`
    - dependency modes, branch map, and zero quorum thresholds for both steps
    - collection policy kinds `Any` for `start` and `All` for `follow`
    - condition flags showing `follow` is conditional while `start` is not
    - zeroed failure/retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it broadens tracked-run coverage beyond the dispatch-family matrix and the
  plain linear `two_step` flow
- it shows the durable aggregate can preserve a conditional follower shape
  without us depending on when the condition actually becomes true at runtime

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_shared_path_shape`

Result:

- `MobMachine` now has owner-backed evidence that a conditional two-step flow
  survives as durable tracked-run kernel truth in the joined snapshot

Backtracks encountered:

- none in the final slice
- the key restraint was proving only durable tracked-run maps and bounded
  status identity, not richer active-step semantics

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this broadens the tracked-run surface, but it is still not a write-side
  switch signal

Next likely step:

- keep moving outward from simple tracked-run families only where the live
  aggregate clearly sustains the next shape

## Slice 143 - MeerkatMachine proves reset abandons queued work on both plain and attached paths

Goal:

- tighten the current reset lifecycle map by checking whether queued input
  teardown is really owned behavior on both the plain and attached-loop paths

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
    so the plain reset path now proves:
    - `inputs.queue` is empty immediately after reset
    - `inputs.steer_queue` is empty immediately after reset
    - both queues remain empty after the preserved `wait_all` settles
  - strengthened
    `meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the attached reset path now proves:
    - `inputs.queue` is empty immediately after reset
    - `inputs.steer_queue` is empty immediately after reset
    - both queues remain empty after the preserved `wait_all` settles

Why this slice matters:

- it upgrades reset from "completion waiters clear" to the stronger current
  owner truth we actually need for cutover work: reset abandons queued input on
  both plain and attached paths
- it is exactly the sort of path-specific lifecycle fact that is easy to
  overgeneralize later if we do not pin it down now

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that current reset
  semantics clear both ordinary and steered queued work on plain and attached
  paths, while still preserving ops-owned `wait_all` until the operation
  settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to test this only on reset, where the live runtime
  already strongly implied queue abandonment, rather than promoting a broader
  phase rule

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another clean convergence slice, but not a write-authority switch
  signal

Next likely step:

- keep extending only lifecycle facts that the live owner path clearly sustains
  without broadening them into phase-wide folklore

## Slice 144 - MobMachine adds a live fan-out dispatch durability proof

Goal:

- cover the remaining distinct dispatch family at the same durable single-step
  tracked-run bar already proven for one-to-one and fan-in

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_fan_out_dispatch_shape`
  - the new live proof shows an active fan-out dispatch run preserves:
    - durable `flow_id == dispatch`
    - `status == Running`
    - schema version `4`
    - non-terminal shape
    - zero frame/loop structure
    - the full single-step kernel maps for `dispatch`
    - `CollectionPolicyKind::Any` with zero quorum threshold
    - zeroed failure counters, retry budget, and escalation threshold
    - bounded materialized step-status identity

Why this slice matters:

- it completes the dispatch-family coverage across one-to-one, fan-in, and
  fan-out without promoting `dispatch_mode` itself into a tracked-run fact
- it keeps `MobMachine` on the durable aggregate surface instead of reaching
  into timing-sensitive runtime behavior

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_fan_out_dispatch_shape`

Result:

- `MobMachine` now has live evidence that active fan-out dispatch runs collapse
  onto the same durable single-step tracked-run shape already trusted in the
  other dispatch families

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to keep the proof on persisted tracked-run fields and not
  invent a new `dispatch_mode` machine fact

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens family coverage, but it is still not a write-authority
  switch signal

Next likely step:

- continue selecting only distinct active families whose durable tracked-run
  aggregates already sustain the same proof bar

## Slice 147 - MeerkatMachine proves plain recover preserves steered input and wait_all

Goal:

- cover the steered ingress lane explicitly on the plain registered recover
  path instead of continuing to reuse only ordinary queued prompts

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_recover_preserves_steered_input_and_wait_all`
  - the new test proves that plain `recover()`:
    - preserves `inputs.steer_queue == [input_id]`
    - keeps `inputs.queue` empty
    - preserves `wake_requested` and `process_requested`
    - preserves the input-owned completion waiter
    - preserves the ops-owned `wait_all` carrier and request-id agreement
    - still leaves the steered input pending after `wait_all` settles

Why this slice matters:

- it upgrades the lifecycle map from "queued work survives recover" to the more
  precise fact that the dedicated steer lane also survives recover with its own
  wake/process semantics intact
- it broadens the observational model without inventing any new owner

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_preserves_steered_input_and_wait_all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_steered_prompt_input`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that plain recover
  preserves steered queued work and its wake/process semantics while also
  preserving active `wait_all`

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to keep this on the plain recover path instead of
  assuming the whole lifecycle matrix behaves identically for steer inputs

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens a distinct ingress lane, but it is not a write-authority
  switch signal

Next likely step:

- continue selecting genuinely distinct owner lanes, especially where steer,
  lifecycle, and queued-work semantics intersect

## Slice 148 - MobMachine adds a live fan-in all-policy dispatch proof

Goal:

- extend dispatch-family coverage to the fan-in + `CollectionPolicy::All`
  combination on the same durable single-step tracked-run surface already used
  for other dispatch families

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_fan_in_all_dispatch_shape`
  - the new live proof shows an active fan-in all-policy dispatch run
    preserves:
    - durable `flow_id == dispatch`
    - `status == Running`
    - schema version `4`
    - non-terminal shape
    - zero frame/loop structure
    - the full single-step kernel maps for `dispatch`
    - `CollectionPolicyKind::All` with zero quorum threshold
    - zeroed failure counters, retry budget, and escalation threshold
    - bounded materialized step-status identity

Why this slice matters:

- it expands the active dispatch-family matrix with another distinct routing +
  aggregation combination while staying entirely on persisted tracked-run state
- it gives `MobMachine` better coverage without promoting dispatch mechanics
  themselves into machine-owned truth

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_fan_in_all_dispatch_shape`

Result:

- `MobMachine` now has live evidence that active fan-in all-policy dispatch
  runs collapse onto the same durable single-step tracked-run shape already
  trusted elsewhere

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to keep the proof on durable tracked-run fields rather than
  inventing a new `dispatch_mode` machine fact

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens family coverage, but it is still not a write-authority
  switch signal

Next likely step:

- continue selecting only distinct active families whose durable tracked-run
  aggregates already sustain the same proof bar

## Slice 145 - MeerkatMachine proves stop abandons queued work on both plain and attached paths

Goal:

- tighten the current stop lifecycle map by checking whether queued input
  teardown is really owner-backed behavior on both the plain and attached-loop
  paths

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
    so the plain stop path now proves:
    - `inputs.queue` is empty immediately after stop
    - `inputs.steer_queue` is empty immediately after stop
    - both queues remain empty after the preserved `wait_all` settles
  - strengthened
    `meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the attached stop path now proves:
    - `inputs.queue` is empty immediately after stop
    - `inputs.steer_queue` is empty immediately after stop
    - both queues remain empty after the preserved `wait_all` settles

Why this slice matters:

- it upgrades stop from "completion waiters clear" to the stronger current
  owner truth we need for cutover work: stop abandons queued input on both
  plain and attached paths
- it complements the earlier reset slice and gives us a cleaner teardown matrix
  for the non-terminal control path that still preserves ops-owned `wait_all`

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that current stop
  semantics clear both ordinary and steered queued work on plain and attached
  paths, while still preserving ops-owned `wait_all` until the operation
  settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to keep this on the stop family rather than promoting a
  broader phase-wide queued-work rule

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another clean convergence slice, but not a write-authority switch
  signal

Next likely step:

- keep extending lifecycle facts only where the live owner path strongly
  sustains the narrower claim before any broader phase generalization

## Slice 146 - MobMachine adds a live fan-out all-policy dispatch proof

Goal:

- cover the next distinct dispatch family by combining fan-out routing with the
  non-default `CollectionPolicy::All` on the same durable single-step tracked-run
  surface

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_fan_out_all_dispatch_shape`
  - the new live proof shows an active fan-out all-policy dispatch run
    preserves:
    - durable `flow_id == dispatch`
    - `status == Running`
    - schema version `4`
    - non-terminal shape
    - zero frame/loop structure
    - the full single-step kernel maps for `dispatch`
    - `CollectionPolicyKind::All` with zero quorum threshold
    - zeroed failure counters, retry budget, and escalation threshold
    - bounded materialized step-status identity

Why this slice matters:

- it extends the dispatch-family coverage beyond the default `Any` aggregation
  path without inventing a new tracked-run fact for `dispatch_mode`
- it keeps `MobMachine` on the durable aggregate surface instead of reaching
  into timing-sensitive dispatch/runtime behavior

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_fan_out_all_dispatch_shape`

Result:

- `MobMachine` now has live evidence that active fan-out all-policy dispatch
  runs still collapse onto the same durable single-step tracked-run shape
  already trusted in the other dispatch families

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to keep the proof on persisted tracked-run fields and not
  promote dispatch mechanics themselves into machine truth

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens family coverage, but it is still not a write-authority
  switch signal

Next likely step:

- continue selecting only distinct active families whose durable tracked-run
  aggregates already sustain the same proof bar

## Slice 139 - MeerkatMachine narrows retire queue cleanup to the attached-loop path

Goal:

- probe whether settled `Retired` snapshots can finally support a broad
  no-queued-work rule, and keep only the smaller truth the live owner path
  actually sustains

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the attached-loop `Retired` snapshots now also prove:
    - `inputs.queue.is_empty()`
    - `inputs.steer_queue.is_empty()`

Why this slice matters:

- it keeps a useful Meerkat fact without lying about the broader phase: once an
  attached loop has actually drained preserved work and returned to `Retired`,
  the queued-work residue is gone
- it also reconfirms that the plain registered retire path is still not ready
  for a global `Retired => no queued work` validator rule

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` gained a narrower attached-loop retire queue-cleanup proof
- no broad `RetiredRuntimeStillHasQueuedWork` validator rule was promoted

Backtracks encountered:

- the plain retire split lane failed immediately when asked to prove settled
  queue emptiness, so those assertions were backed out
- that keeps the machine aligned to current owner truth instead of a nicer
  phase story

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is convergence through narrowing, not a write-side switch

Next likely step:

- keep probing Meerkat settled-phase cleanup only where focused lanes can tell
  us whether the fact is truly global or still path-dependent

## Slice 140 - MobMachine adds a live one-to-one dispatch durability proof

Goal:

- extend active tracked-run coverage to a dispatch-mode flow family without
  inventing `dispatch_mode` as a durable MobMachine field

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_one_to_one_dispatch_shape`
    to prove the active one-to-one dispatch run preserves:
    - `flow_id == dispatch`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - durable single-step kernel maps for the lone `dispatch` step
    - `CollectionPolicyKind::Any`
    - zeroed failure counters, retry budget, and escalation threshold
    - non-empty, kernel-bounded materialized `step_status` identity

Why this slice matters:

- it broadens active-run family coverage into a flow whose orchestration
  behavior differs, while still staying on the durable tracked-run fields the
  aggregate already owns today
- it explicitly avoids promoting `dispatch_mode` itself, because that is still
  config truth rather than persisted kernel truth

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_one_to_one_dispatch_shape`

Result:

- `MobMachine` now has live active-run durability coverage for the one-to-one
  dispatch family at the same structural bar as the other step-only active
  paths

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was choosing family coverage rather than a new tracked-run
  field the aggregate does not actually persist

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens durable family coverage without moving write authority

Next likely step:

- keep adding active flow families only when the joined tracked-run snapshot
  clearly sustains the same durable structure without timing folklore

## Slice 141 - MeerkatMachine proves attached recover/recycle replay drains queued work

Goal:

- strengthen the attached-loop `recover()` / `recycle()` lifecycle map with one
- more path-specific truth that does not over-promote a global phase rule

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the post-replay and settled attached snapshots now also prove:
    - `inputs.queue.is_empty()`
    - `inputs.steer_queue.is_empty()`
  - strengthened
    `meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the post-replay and settled attached snapshots now also prove:
    - `inputs.queue.is_empty()`
    - `inputs.steer_queue.is_empty()`

Why this slice matters:

- it captures a real attached-loop fact: once replay has actually finished and
  the runtime returns to `Attached`, preserved queued work has been fully
  drained
- it stays intentionally path-specific after the plain retire probe showed that
  broader phase-level queue cleanup rules can still be false

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has stronger attached-path evidence that replay-driven
  lifecycle handoffs drain queued work before the runtime settles back into
  `Attached`

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was keeping this as attached-path truth instead of trying to
  generalize it into a new global validator rule

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger lifecycle convergence, not a write-side switch

Next likely step:

- continue extending attached/plain lifecycle maps only where the focused owner
  lanes clearly sustain the additional fact

## Slice 142 - MobMachine adds a live fan-in dispatch durability proof

Goal:

- broaden active tracked-run family coverage to the `DispatchMode::FanIn`
  orchestration path without inventing `dispatch_mode` as a durable machine
  field

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_fan_in_dispatch_shape`
    to prove the active fan-in dispatch run preserves:
    - `flow_id == dispatch`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - durable single-step kernel maps for the lone `dispatch` step
    - `CollectionPolicyKind::Any`
    - zeroed failure counters, retry budget, and escalation threshold
    - non-empty, kernel-bounded materialized `step_status` identity

Why this slice matters:

- it adds a second dispatch-mode family to the active durability set, so we are
  no longer leaning on only the one-to-one dispatch path
- it deliberately avoids turning `dispatch_mode` itself into tracked-run truth,
  since that still lives in config rather than the durable run aggregate

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_fan_in_dispatch_shape`

Result:

- `MobMachine` now has live active-run durability coverage for both one-to-one
  and fan-in dispatch families at the same single-step structural bar

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was family coverage rather than a new tracked-run field the
  durable aggregate does not currently own

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens tracked-run family coverage without moving write authority

Next likely step:

- keep adding flow families only when the joined tracked-run snapshot clearly
  sustains the same durable structure without timing folklore

## Slice 133 - MeerkatMachine backs settled recover teardown rules on plain and attached paths

Goal:

- extend the settled current-run teardown rule into the recover family without
  flattening the replay or queued-input semantics that recover intentionally
  preserves

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes`
    so the settled post-`wait_all` snapshot now also proves:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`
  - strengthened
    `meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the settled post-`wait_all` attached snapshot now also proves:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`

Why this slice matters:

- it aligns recover with the settled teardown rule already established for
  retire, stop, destroy, and the corresponding attached-loop paths
- it keeps the current implementation truth intact: recover may still preserve
  queued work or ops-owned wait carriers, but once the handoff has settled it
  must not continue to claim an active current run

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has direct plain and attached recover coverage for the
  settled active-run teardown rule

Backtracks encountered:

- none in code; this slice confirmed the existing owner boundary rather than
  moving it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger lifecycle convergence, not a write-side switch

Next likely step:

- keep extending settled teardown rules only into lifecycle families where the
  owner-backed semantics are already stable enough to prove directly

## Slice 134 - MobMachine lifts retry-limited active runs to the durable single-step shape bar

Goal:

- raise the active retry-limited path to the same durable single-step tracked
  run shape already proven for stable collection-policy and root-frame paths

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_retry_limit_shape`
    so active retry-limited runs now also prove:
    - `flow_id == demo`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - ordered step / dependency / dependency mode / condition / branch /
      collection-policy / quorum maps for the lone `start` step

Why this slice matters:

- it promotes the retry-limited path only after the live tracked-run aggregate
  showed it follows the same durable single-step pattern as the already-stable
  step-only paths
- it still avoids timing-sensitive retry behavior or stronger step-status
  materialization claims

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_retry_limit_shape`

Result:

- `MobMachine` now has the retry-limited active path at the same structural
  single-step durability bar as the other stable step-only flow families

Backtracks encountered:

- none in the final slice; this was deliberately scoped to facts the current
  durable aggregate already owns

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens durable tracked-run shape without moving write authority

Next likely step:

- continue raising active flow families to the same structural bar only when
  the live aggregate clearly sustains them

## Slice 137 - MeerkatMachine backs settled reset teardown rules on plain and attached paths

Goal:

- extend the settled current-run teardown rule into the reset family without
  flattening the split lifetime between input waiters and ops-owned `wait_all`

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
    so the settled post-`wait_all` snapshot now also proves:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`
  - strengthened
    `meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the settled post-`wait_all` attached snapshot now also proves:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`

Why this slice matters:

- it brings reset into the same settled teardown rule already proven for
  retire, stop, destroy, recover, and recycle, across both plain and
  attached-loop paths
- it keeps the current implementation truth intact: reset may preserve the
  ops-owned wait carrier even after input waiters are cleared, but once the
  handoff has settled it must not continue to claim an active current run

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has direct plain and attached reset coverage for the
  settled active-run teardown rule

Backtracks encountered:

- none in code; this slice confirmed the existing owner boundary rather than
  moving it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger lifecycle convergence, not a write-side switch

Next likely step:

- keep extending settled teardown rules only where the owner-backed lifecycle
  semantics are already stable enough to prove directly

## Slice 138 - MobMachine adds a baseline active single-step durability proof

Goal:

- prove the plain single-step `demo` flow on the same durable tracked-run
  structural bar already used by retry-limited, supervisor, and collection
  variants

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_single_step_shape`
    to prove active single-step runs preserve:
    - `flow_id == demo`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - ordered step / dependency / dependency mode / condition / branch /
      collection-policy / quorum maps for the lone `start` step
    - zeroed failure counters, retry budget, and escalation threshold
    - bounded materialized step-status identity

Why this slice matters:

- it gives `MobMachine` a plain baseline active-run proof, not just decorated
  variants like retry-limited or supervisor-threshold flows
- it still avoids timing-sensitive claims beyond the bounded step-status fact
  already accepted on healthy active single-step paths

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_single_step_shape`

Result:

- `MobMachine` now has a baseline active single-step tracked-run durability
  proof at the same structural bar as the already-stable single-step families

Backtracks encountered:

- none in the final slice; the new path sustained the same durable structure as
  the other healthy single-step flow families

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens durable tracked-run shape without moving write authority

Next likely step:

- continue selecting active run families only when the live aggregate clearly
  sustains the same durable structural bar

## Slice 135 - MeerkatMachine backs settled recycle teardown rules on plain and attached paths

Goal:

- extend the settled current-run teardown rule into the recycle family without
  flattening the preserved-work semantics that recycle intentionally keeps

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes`
    so the settled post-`wait_all` snapshot now also proves:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`
  - strengthened
    `meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so the settled post-`wait_all` attached snapshot now also proves:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`

Why this slice matters:

- it aligns recycle with the settled teardown rule already established for
  retire, stop, destroy, recover, and the corresponding attached-loop paths
- it keeps the current implementation truth intact: recycle may still preserve
  queued work or ops-owned wait carriers, but once the handoff has settled it
  must not continue to claim an active current run

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has direct plain and attached recycle coverage for the
  settled active-run teardown rule

Backtracks encountered:

- none in code; this slice confirmed the existing owner boundary rather than
  moving it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger lifecycle convergence, not a write-side switch

Next likely step:

- keep extending settled teardown rules only into lifecycle families where the
  owner-backed semantics are already stable enough to prove directly

## Slice 136 - MobMachine lifts supervisor-threshold active runs to the durable single-step shape bar

Goal:

- raise the active supervisor-threshold path to the same durable single-step
  tracked-run shape already proven for retry-limited and stable collection
  paths

What landed:

- `/Users/luka/.codex/worktrees/c5c6/meerkat/meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_supervisor_threshold_shape`
    so active supervisor-threshold runs now also prove:
    - `flow_id == demo`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - ordered step / dependency / dependency mode / condition / branch /
      collection-policy / quorum maps for the lone `start` step

Why this slice matters:

- it promotes the supervisor-threshold path only after confirming it follows
  the same durable single-step pattern as the already-stable step-only paths
- it still avoids timing-sensitive escalation behavior or stronger step-status
  materialization claims

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_supervisor_threshold_shape`

Result:

- `MobMachine` now has the supervisor-threshold active path at the same
  structural single-step durability bar as the retry-limited and
  collection-policy families

Backtracks encountered:

- none in the final slice; this was deliberately scoped to facts the current
  durable aggregate already owns

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens durable tracked-run shape without moving write authority

Next likely step:

- continue raising active flow families to the same structural bar only when
  the live aggregate clearly sustains them

## Slice 129 - MeerkatMachine rejects settled retired active-run bindings

Goal:

- tighten the executable Meerkat validator only where the lifecycle matrix has
  already converged enough to support it: once a runtime is actually settled in
  `Retired`, it should no longer advertise an active run binding

What landed:

- `meerkat/src/meerkat_machine.rs`
  - tightened `CurrentRunInIllegalControlPhase` so `current_run_id` is now only
    legal while `control.phase == Running`
  - added focused negative proof:
    - `validate_meerkat_machine_snapshot_reports_retired_active_run`
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened:
    - `meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
    - `meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - both settled `Retired` snapshots must now prove:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`

Why this slice matters:

- earlier validator logic still allowed `Retired` to carry `current_run_id`
  even after the retire lifecycle matrix had converged on a different story
- this promotes a real settled-phase ownership fact without flattening the
  temporary attached-loop drain behavior, which still legitimately re-enters
  `Running` before returning to `Retired`

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_retired_active_run`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now rejects a stale settled-retire shape that still claimed
  an active run binding after retire had fully settled

Backtracks encountered:

- no code-level backtrack in the final slice
- the useful restraint was proving only the settled `Retired` state, not the
  temporary attached-loop `Running` drain phase

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another converged validator rule, not a write-side switch

Next likely step:

- continue promoting settled-phase Meerkat invariants only where both the plain
  and attached lifecycle paths now agree on the same owner truth

## Slice 130 - MobMachine bounds active root-frame step-status identity without assuming materialization

Goal:

- strengthen the active root-frame tracked-run proof without reintroducing the
  timing-sensitive assumption that frame-aware runs must already materialize a
  non-empty `step_status` map

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
    so the active tracked root-frame run now also proves:
    - any surfaced `step_statuses` keys stay inside the ordered-step universe

Why this slice matters:

- it keeps the root-frame path moving forward on a real bounded-status fact
  while respecting the current live aggregate, which does not yet sustain
  non-empty status materialization strongly enough to treat that as durable
  machine truth
- it matches the broader methodology here: keep the smaller owned fact, reject
  the nicer imagined one

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`

Result:

- `MobMachine` now has a slightly stronger active root-frame proof without
  overclaiming the current status-materialization surface

Backtracks encountered:

- the initial attempt required
  `!tracked_run.step_statuses.is_empty()`
- the live root-frame lane failed immediately, so that stronger claim was
  removed and replaced with the smaller bounded-status assertion

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is convergence work, not a write-side switch

Next likely step:

- keep preferring active Mob facts that are clearly durable or validator-grade,
  and keep backing off whenever a frame-aware path fails to sustain
  materialized status coverage

## Slice 131 - MeerkatMachine backs settled active-run teardown rules across stop and destroy

Goal:

- strengthen the newly converged Meerkat rule that active run bindings only
  exist while `Running` by proving it across the other settled teardown paths,
  not just retire

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the settled snapshots in:
    - `meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes`
    - `meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    - `meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
    - `meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - each settled snapshot must now also prove:
    - `control.current_run_id == None`
    - `inputs.current_run_id == None`

Why this slice matters:

- the validator now rejects any non-`Running` control phase that still claims a
- current run, but that rule is only useful if the real teardown paths sustain it
- this slice gives that live owner-backed evidence on both plain and attached
  stop/destroy paths, not just settled retire

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now has live teardown-path evidence that once stop or
  destroy has actually settled, neither the control plane nor ingress ledger
  still advertises a current run binding

Backtracks encountered:

- none in the final slice
- the useful restraint was staying on the settled snapshots rather than trying
  to assert anything stronger about intermediate stop/destroy publication timing

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another converged lifecycle invariant family, not a write-side switch

Next likely step:

- continue harvesting settled-phase Meerkat invariants where multiple lifecycle
  paths now agree on the same owner truth

## Slice 132 - MobMachine bounds active loop step-status identity without assuming materialization

Goal:

- raise the active loop path to the same bounded-status bar as the root-frame
  path without assuming the live loop aggregate must already materialize a
  non-empty step-status map

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active tracked loop run now also proves:
    - any surfaced `step_statuses` keys stay inside the ordered-step universe

Why this slice matters:

- it continues the pattern of promoting smaller, clearly owned active-run
  status facts instead of assuming stronger materialization than the live loop
  path really sustains
- it brings the loop path in line with the bounded status identity we already
  kept on the root-frame path after the earlier backtrack

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`

Result:

- `MobMachine` now has a slightly stronger loop-path proof while still staying
  on durable or validator-grade truth

Backtracks encountered:

- none in the final slice
- the useful restraint was again not requiring non-empty loop `step_status`
  materialization, only bounded status identity when entries do exist

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is convergence work, not a write-side switch

Next likely step:

- continue preferring bounded active-run status facts or durable structure
  facts over stronger status-coverage claims that still depend on timing

## Slice 125 - MeerkatMachine rejects settled stopped/destroyed queue residue

Goal:

- promote a control-side queue invariant only after confirming the live runtime
  really clears queued work once `Stopped` or `Destroyed` has settled

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added:
    - `DestroyedRuntimeStillHasQueuedWork`
    - `StoppedRuntimeStillHasQueuedWork`
  - `validate_meerkat_machine_snapshot(...)` now rejects settled `Destroyed`
    and `Stopped` snapshots that still carry either ordinary queued work or
    steer-queued work
  - added focused negative proofs:
    - `validate_meerkat_machine_snapshot_reports_destroyed_queued_work`
    - `validate_meerkat_machine_snapshot_reports_stopped_queued_work`
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the existing live destroy/stop completion-waiter tests on
    both plain and attached-loop paths so the settled snapshots must also prove:
    - `inputs.queue.is_empty()`
    - `inputs.steer_queue.is_empty()`

Why this slice matters:

- it upgrades the Meerkat validator from “settled stop/destroy clears waiters”
  to the stronger executable statement that settled stop/destroy also leaves no
  queued input residue behind
- it stays on current owner truth instead of assuming anything stronger about
  `Retired`, which still has process-capable semantics in the live control
  authority

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_destroyed_queued_work`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_stopped_queued_work`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor_with_runtime_loop`

Result:

- `MeerkatMachine` now executable-checks that once `Stopped` or `Destroyed`
  has actually settled, queued work cannot still be present in either ingress
  queue

Backtracks encountered:

- the key backtrack happened before editing: the tempting `Retired =>
  no current_run` rule was wrong, so this slice stayed on the safer settled
  stop/destroy queue seam instead

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger settled-state validator, not a write-side switch

Next likely step:

- keep selecting Meerkat invariants that are both current-owner-backed and
  phase-stable, especially where lifecycle handoff has already converged

## Slice 126 - MobMachine raises active any-policy runs to single-step kernel maps

Goal:

- bring the active `CollectionPolicy::Any` tracked-run proof up to the same
  durable single-step kernel-map bar already proven for `CollectionPolicy::All`

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
    so the active tracked any-policy run must now also preserve:
    - `ordered_steps == ["collect"]`
    - `step_dependencies == { collect: [] }`
    - `step_dependency_modes == { collect: All }`
    - `step_has_conditions == { collect: false }`
    - `step_branches == { collect: None }`

Why this slice matters:

- it keeps the Mob work on durable tracked-run truth instead of timing-shaped
  step-status coverage
- it aligns the active `Any` path with the same single-step kernel-map surface
  already proven on the `All` path

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that the active any-policy collection path
  preserves the same durable single-step kernel maps as the active all-policy
  path

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to stay on durable tracked-run maps and not re-promote the
  timing-sensitive step-status coverage that earlier slices had already ruled
  out

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is durability convergence, not a write-side switch

Next likely step:

- continue extending active tracked-run proofs only where the live aggregate
  keeps the relevant fields durably present without cleanup races

## Slice 127 - MeerkatMachine rejects a too-strong retired-queue candidate

Goal:

- test whether settled `Retired` snapshots can be promoted to the same
  no-queued-work bar already proven for settled `Stopped` and `Destroyed`

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - strengthened
    `meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
    so that once the attached runtime loop has fully drained preserved work and
    the runtime returns to `Retired`, the snapshot must also prove:
    - `inputs.queue.is_empty()`
    - `inputs.steer_queue.is_empty()`

Why this slice matters:

- it gives us one smaller, trustworthy Meerkat fact: the attached-loop retire
  path does finish draining queued work before the runtime settles back into
  `Retired`
- it also exposed that the broader plain-registered `Retired => no queued work`
  rule is not yet safe to promote

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` gained a stronger attached-loop retire proof, but no new
  retired-phase validator rule landed

Backtracks encountered:

- the candidate `RetiredRuntimeStillHasQueuedWork` validator rule was rejected
  by the focused plain retire lane
- the plain no-loop retire path currently leaves queued-work residue visible in
  the snapshot even after input-owned completion waiters clear
- a quick `EphemeralRuntimeDriver::retire()` projection rebuild did not fix
  that, which points to deeper ingress-authority truth rather than a simple
  projection cache bug
- I backed out the validator rule and the plain-path queue assertions rather
  than forcing the machine to lie

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this was productive because it found a real authority/shell mismatch, not
  because it advanced write authority

Next likely step:

- keep probing Meerkat lifecycle seams where a focused lane can tell us whether
  a candidate rule is real machine truth or still hidden shell residue

## Slice 128 - MobMachine proves active quorum runs surface materialized step status

Goal:

- raise the active quorum collection path to the same smaller healthy-run
  status bar already sustained by the active any-policy and all-policy paths

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
    so the active tracked quorum run must now also prove:
    - `!tracked_run.step_statuses.is_empty()`

Why this slice matters:

- it stays on a live tracked-run fact the current aggregate already owns
  durably for healthy active collection runs
- it brings the quorum path up to the same “materialized but not full-coverage”
  status surface that already held for any/all

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy active quorum runs surface at
  least one materialized step status while keeping those status keys inside the
  ordered-step universe

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to stop at non-empty status presence instead of trying to
  prove broader active-run step-status coverage

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is durability convergence, not a write-side switch

Next likely step:

- keep extending active tracked-run proofs only where the live aggregate keeps
  the relevant fields durably present without cleanup races

## Slice 127 - MeerkatMachine rejects settled retired queue residue

Goal:

- promote the next phase-stable Meerkat queue invariant only after confirming
  the live runtime really leaves `Retired` with no queued work once the phase
  has settled

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added `RetiredRuntimeStillHasQueuedWork`
  - `validate_meerkat_machine_snapshot(...)` now rejects settled `Retired`
    snapshots that still carry ordinary queued work or steer-queued work
  - added focused negative proof:
    - `validate_meerkat_machine_snapshot_reports_retired_queued_work`
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the existing plain and attached-loop retire split tests so the
    settled `Retired` snapshots must also prove:
    - `inputs.queue.is_empty()`
    - `inputs.steer_queue.is_empty()`

Why this slice matters:

- it extends the settled-phase queue rules from `Stopped` and `Destroyed` to
  `Retired`, but only at the point where current owner behavior is already
  phase-stable
- it deliberately avoids the stronger and wrong rule that `Retired` can never
  carry `current_run_id`; the live control authority still allows that during
  drain handoff

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_retired_queued_work`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`

Result:

- `MeerkatMachine` now executable-checks that once `Retired` has actually
  settled, queued work cannot still be present in either ingress queue

Backtracks encountered:

- no code-level backtrack in the final slice
- the important restraint was keeping this on the settled `Retired` snapshot
  instead of trying to outlaw all queued work during the live attached-loop
  drain handoff

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger settled-state validator, not a write-side switch

Next likely step:

- keep selecting phase-stable Meerkat invariants whose live owner behavior has
  already converged across plain and attached lifecycle paths

## Slice 128 - MobMachine proves active quorum runs surface materialized step status

Goal:

- raise the active quorum collection path to the same healthy-run status bar
  already sustained by the active any-policy and all-policy paths

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
    so the active tracked quorum run must now also prove:
    - `!tracked_run.step_statuses.is_empty()`

Why this slice matters:

- it stays on a live tracked-run fact that the current aggregate already owns
  durably for healthy active collection runs
- it brings the quorum path up to the same smaller “materialized but not
  full-coverage” status surface that already worked for any/all

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy active quorum runs surface at
  least one materialized step status while staying inside the ordered-step
  universe

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to stop at non-empty status presence instead of trying to
  prove broader active-run step-status coverage

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is durability convergence, not a write-side switch

Next likely step:

- keep extending active tracked-run proofs only where the live aggregate keeps
  the relevant fields durably present without cleanup races

## Slice 89 - MeerkatMachine proves attached-loop reset preserves wait_all while abandoning queued work

Goal:

- cover the remaining attached-loop lifecycle seam where `reset()` abandons
  queued work but should still preserve the ops-owned `wait_all` carrier

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop`
  - proved that on an attached runtime with queued work and a live background
    operation:
    - `reset()` abandons the queued input (`inputs_abandoned == 1`)
    - runtime phase returns to `Idle`
    - the authority-owned `wait_request_id` survives
    - the shell-owned pending wait carrier survives with the same
      `WaitRequestId`
    - the tracked wait target list stays intact until the operation settles
    - queued attached-loop work is bypassed without calling executor `apply()`
      or `control()`

Why this slice matters:

- it closes the last obvious attached-loop `wait_all` gap across the current
  Meerkat lifecycle handoff matrix
- it sharpens current semantics instead of guessing target-state behavior:
  attached-loop reset behaves like idle reset for ops waiters, even while it
  eagerly abandons queued input work

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop
  `wait_all` survives `recover`, `recycle`, `reset`, `stop`, `retire`, and
  `destroy`, even though input-completion waiters do not survive all of those
  same seams

Backtracks encountered:

- no code backtrack in the final slice
- the restraint was to preserve only the current owner truth and not assume
  anything stronger about executor control delivery on reset

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens lifecycle-handoff coverage, but it is still observational
  rather than write-authoritative

Next likely step:

- keep selecting lifecycle seams where input waiters and ops waiters diverge,
  because those continue to surface real current-owner semantics

## Slice 90 - MobMachine strengthens live root-frame tracked-run durability

Goal:

- extend the active root-frame proof from coarse structure (`frame_count > 0`)
  to the durable tracked-run step maps that should already survive on the
  frame-aware single-step path

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
    so the live root-frame tracked run must now preserve:
    - `ordered_steps == [start]`
    - `step_dependencies == { start: [] }`
    - `step_dependency_modes == { start: All }`
    - `step_has_conditions == { start: false }`
    - `step_branches == { start: None }`
    - `step_collection_policy_kinds == { start: All }`
    - `step_quorum_thresholds == { start: 0 }`
  - retained the existing healthy-run structural checks:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - zeroed failure counters
    - no terminal completion timestamp

Why this slice matters:

- it proves the frame-aware path carries the same kernel-owned step maps as the
  simpler non-frame single-step flow, rather than only surfacing frame counts
- it stays on durable tracked-run truth and avoids the timing-sensitive output
  or status surfaces that previously failed the live-truth bar

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active root-frame tracked runs
  preserve their durable step/dependency/kernel-policy maps, not just their
  frame presence

Backtracks encountered:

- no new code backtrack in the final slice
- the deliberate restraint was to strengthen only clearly durable tracked-run
  structure, not step-status timing or output projection

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in the tracked-run kernel shape without changing
  canonical write authority

Next likely step:

- continue promoting only tracked-run facts that the live frame-aware path can
  sustain across active snapshots without timing folklore

## Slice 91 - MeerkatMachine proves attached-loop recycle preserves wait_all through replay

Goal:

- cover the attached-loop `recycle()` seam where the adapter preserves queued
  work, restores `Attached`, and wakes the loop again to replay that work

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop`
  - proved that on an attached runtime with queued work and a live background
    operation:
    - `recycle()` transfers the queued input (`inputs_transferred == 1`)
    - the pending wait carrier and authority-owned `wait_request_id` survive
    - queued work is replayed by the attached executor exactly once
    - runtime phase temporarily re-enters `Running` while replay is in flight
    - runtime returns to `Attached` once replay finishes
    - the wait target list remains intact until the operation itself settles
    - no executor `control()` call is used on the recycle path

Why this slice matters:

- it fills in the attached-loop recycle seam, which is one of the last major
  lifecycle handoffs not yet represented in the Meerkat observation matrix
- it captures current replay semantics instead of flattening recycle into a
  simpler idle-only story

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop
  `wait_all` survives recycle and remains coherent across the replay phase and
  the return to `Attached`

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to assert only the current replay-phase transitions, not a
  broader target-state story about how recycle should behave after cutover

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this materially improves lifecycle-handoff coverage, but it is still an
  observational proof over current owner code

Next likely step:

- keep filling the remaining attached-loop lifecycle matrix only where the
  current runtime actually owns a distinct seam

## Slice 92 - MobMachine backs off root-frame step-status timing and keeps durable root-frame truth

Goal:

- try to strengthen the active root-frame proof with materialized step-status
  visibility, but keep only what the live tracked-run path actually sustains

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - attempted to require non-empty `step_statuses` for the active root-frame
    snapshot
  - the live path failed that requirement immediately
  - backed the slice down to a smaller durable proof on the same root-frame
    run:
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
    - plus the already-proven frame-aware step maps and healthy-run counters

Why this slice matters:

- it gives a clean negative signal: active root-frame runs still do not own
  materialized step-status strongly enough to treat it as cutover-grade truth
- it keeps progress honest by strengthening only the durable scalars that the
  tracked-run store actually preserves on the root-frame path

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that active root-frame tracked
  runs preserve default retry/escalation kernel fields, while step-status
  timing remains explicitly non-authoritative on this path

Backtracks encountered:

- the first version of the slice failed because the active root-frame run did
  not surface any materialized step status
- the correct response was to remove that claim instead of forcing the runtime
  into a cleaner observer model

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is exactly the kind of backtrack the gate is meant to surface:
  boundaries stay stable, but not every representable fact is owner-backed yet

Next likely step:

- continue promoting only root-frame facts that survive the live tracked-run
  path without depending on timing-sensitive step-status materialization

## Slice 93 - MeerkatMachine proves attached-loop recover preserves wait_all and returns to Attached

Goal:

- cover the remaining attached-loop `recover()` seam where queued work is
  preserved and replayed through a still-live executor loop

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop`
  - proved that on an attached runtime with queued work and a live background
    operation:
    - `recover()` reports `inputs_recovered == 1`
    - the authority-owned `wait_request_id` survives
    - the shell-owned pending wait carrier survives with the same request id
    - the attached executor replays recovered queued work exactly once
    - runtime phase temporarily re-enters `Running` while replay is in flight
    - runtime returns to `Attached` after replay completes
    - the tracked wait target list remains intact until the operation settles
    - no executor `control()` call is used on the recover path

Why this slice matters:

- it closes another major attached-loop lifecycle seam in the Meerkat
  observation matrix
- more importantly, it exposed a real semantic correction:
  the first hypothesis was that recover would land in `Idle`, but the live
  owner path proved that attached-loop recover returns to `Attached`

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop
  `wait_all` survives recover, replay, and the return to `Attached`

Backtracks encountered:

- the first version of the slice expected recover replay to land in `Idle`
- the live runtime disproved that; the correct stable phase after replay is
  `Attached`
- this was a good semantic backtrack, not a flaky test failure

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is strong convergence signal because the boundary did not move, but we
  were still learning real lifecycle semantics from current code

Next likely step:

- continue selecting attached-loop lifecycle seams only where the live runtime
  still has distinct current behavior we have not yet frozen

## Slice 94 - MobMachine extends healthy loop durability with default retry/escalation scalars

Goal:

- strengthen the active loop-path proof with durable scalar fields that should
  survive even though active loop step-status timing still does not

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active loop tracked run must now preserve:
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
  - kept the existing durable loop-path truths:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count > 0`
    - `loop_iteration_count > 0`
    - zeroed failure counters
    - no terminal completion timestamp

Why this slice matters:

- it mirrors the successful root-frame scalar proof onto the loop path
- it keeps Mob progress on durable tracked-run truth and avoids reintroducing
  timing-shaped claims about active loop status materialization

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that healthy active loop runs
  preserve default retry/escalation kernel fields as well as loop/frame
  structure

Backtracks encountered:

- none in the final loop slice
- the important restraint was choosing durable loop scalars instead of trying
  to promote active loop step-status timing again

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run durability without changing canonical
  write authority

Next likely step:

- continue promoting only loop-path facts that survive the live tracked-run
  path without depending on materialized active step-status timing

## Slice 95 - MeerkatMachine proves attached-loop recover preserves completion waiters through replay

Goal:

- cover the attached-loop `recover()` seam for input-owned completion waiters,
  not just ops-owned `wait_all`, so the input-vs-ops split stays explicit in
  the lifecycle matrix

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop`
  - proved that on an attached runtime with queued progress work and a live
    completion waiter:
    - `recover()` reports `inputs_recovered == 1`
    - the completion-waiter carrier survives while the attached loop replays
      recovered work
    - runtime phase temporarily re-enters `Running` during replay
    - the attached executor replays the recovered queued work exactly once
    - no executor `control()` call is used on the recover path
    - once replayed work completes, the completion waiter resolves and the
      carrier clears
    - runtime returns to `Attached` after replay completes

Why this slice matters:

- it fills the completion-waiter side of the attached-loop recover seam, which
  was still missing even after the `wait_all` side was covered
- it keeps the lifecycle map honest by showing that recover preserves
  input-owned waiters long enough to drain recovered work, rather than
  flattening them into stop/reset/destroy behavior

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop recover
  preserves completion waiters during replay and clears them only after the
  recovered work actually completes

Backtracks encountered:

- none in the final slice
- the restraint was to mirror only currently owned replay semantics, not to
  invent a broader target-state story for recover

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another strong convergence slice because it extended current-owner
  lifecycle truth without moving any owner boundary

Next likely step:

- continue filling only the remaining attached-loop lifecycle seams where
  completion waiters and ops waiters still diverge in current code

## Slice 96 - MobMachine proves live loop runs preserve durable step maps

Goal:

- strengthen the active loop-path proof with durable tracked-run step maps,
  but only if the live flow path sustains them without depending on active
  step-status timing

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active loop tracked run must now preserve:
    - `ordered_steps == [body]`
    - empty dependency map for `body`
    - `DependencyMode::All` for `body`
    - `step_has_conditions[body] == false`
    - no branch label for `body`
    - `RunCollectionPolicyKind::All` for `body`
    - zero quorum threshold for `body`
  - kept the already-proven loop-path truths:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count > 0`
    - `loop_iteration_count > 0`
    - zeroed failure counters
    - default retry/escalation scalars
    - no terminal completion timestamp

Why this slice matters:

- it upgrades the loop path from “structure exists” to “durable step kernel
  maps survive on the live loop path,” matching what the root-frame path was
  already proving
- it still stays on durable tracked-run truth and avoids reintroducing the
  active step-status timing claims that previously failed

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy active loop runs preserve
  their durable step/dependency/kernel-policy maps, not just loop/frame
  counters

Backtracks encountered:

- none in the final slice
- the key restraint was to strengthen only the durable step maps and leave
  active loop step-status timing out of scope

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is a good convergence slice because it strengthens durable tracked-run
  truth without expanding the boundary or inventing a cleaner runtime than the
  live code actually owns

Next likely step:

- continue promoting only active loop facts that the tracked-run store
  actually preserves across live snapshots without timing folklore

## Slice 97 - MeerkatMachine proves attached-loop recycle preserves completion waiters through replay

Goal:

- cover the attached-loop `recycle()` seam for input-owned completion waiters,
  not just ops-owned `wait_all`, so recycle is represented on both sides of
  the input-vs-ops split

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop`
  - proved that on an attached runtime with queued progress work and a live
    completion waiter:
    - `recycle()` reports `inputs_transferred == 1`
    - the completion-waiter carrier survives while the attached loop replays
      preserved work
    - runtime phase temporarily re-enters `Running` during replay
    - the attached executor replays preserved work exactly once
    - no executor `control()` call is used on the recycle path
    - once replayed work completes, the completion waiter resolves and the
      carrier clears
    - runtime returns to `Attached` after replay completes

Why this slice matters:

- it fills the completion-waiter side of the attached-loop recycle seam, which
  was still missing even after the `wait_all` path was covered
- it confirms that recycle behaves like recover here: input-owned waiters stay
  live long enough to drain preserved work instead of being flattened into the
  stop/reset/destroy class

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop recycle
  preserves completion waiters during replay and clears them only once the
  preserved work actually completes

Backtracks encountered:

- none in the final slice
- the key discipline was to mirror only the current replay behavior already
  owned by the adapter and executor seam

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens the lifecycle matrix again, but it is still observational
  coverage over current owner code rather than a switch to new write authority

Next likely step:

- continue filling only the remaining attached-loop lifecycle seams where
  completion waiters and ops waiters still diverge in current code

## Slice 98 - MobMachine proves active loop runs preserve durable identity and running status

Goal:

- take one small active-loop tracked-run step that stays durable and avoids the
  step-status timing surfaces that have already failed us

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active loop tracked run must now preserve:
    - `flow_id == demo`
    - `status == Running`
  - kept the already-proven loop-path truths:
    - `schema_version == 4`
    - frame/loop/iteration structure
    - durable step/dependency/kernel-policy maps
    - zeroed failure counters
    - default retry/escalation scalars
    - no terminal completion timestamp

Why this slice matters:

- it adds a first small live proof that the active loop path preserves durable
  run identity and current non-terminal status, not just structural counters
- it stays well clear of the timing-shaped active step-status materialization
  that previously forced backtracks

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active loop runs preserve durable
  `flow_id` identity and stay in `Running` while the never-terminal body keeps
  the run alive

Backtracks encountered:

- none in the final slice
- the important restraint was choosing durable run identity/state rather than
  another materialized step-status claim

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in active tracked-run truth without changing the
  canonical write boundary

Next likely step:

- continue promoting only active-loop facts that survive live snapshots
  without depending on cleanup timing or step-status folklore

## Slice 99 - MeerkatMachine proves attached-loop retire splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `retire()` seam with both an input-owned
  completion waiter and an ops-owned `wait_all` live at the same time, so the
  split is proved directly instead of inferred from separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with queued progress work, a live
    completion waiter, and an active background operation under `wait_all`:
    - `retire()` reports `inputs_pending_drain == 1`
    - runtime phase temporarily re-enters `Running` while the attached loop
      drains preserved work
    - the completion-waiter carrier survives through that replay/drain phase
    - the authority-owned `wait_request_id` and pending wait carrier also
      survive through that same phase
    - no executor `control()` call is used on the retire path
    - once the preserved queued work finishes, completion waiters clear and
      runtime returns to `Retired`
    - the ops-owned `wait_all` carrier remains live after that point and only
      clears once the background operation itself settles

Why this slice matters:

- it is the clearest direct proof so far that current Meerkat lifecycle
  really does split input waiters from ops waiters on the same attached-loop
  retire path
- it moves us from “separate tests imply a split” to “the joined model sees
  the split happen in one live scenario”

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop retire
  clears input-owned completion waiters when drained work completes, while
  preserving ops-owned `wait_all` until the underlying operation settles

Backtracks encountered:

- none in the final slice
- the important discipline was keeping the test entirely on current owner
  seams rather than using it to “normalize” retire into a simpler lifecycle

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is strong convergence signal because it deepens the lifecycle matrix
  without changing any owner boundary

Next likely step:

- continue selecting only lifecycle seams where current input-owned and
  ops-owned behavior still diverge in a way the joined machine needs to freeze

## Slice 100 - MobMachine proves active branch runs preserve durable identity and running status

Goal:

- take the same small durable run-identity/state step on the active branch
  path that already held on the active loop path

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
    so the active branch tracked run must now preserve:
    - `flow_id == branching`
    - `status == Running`
  - kept the already-proven branch-path truths:
    - ordered steps
    - condition flags
    - branch labels and group membership
    - dependency/dependency-mode maps
    - collection policy kind and quorum threshold maps
    - materialized step-status keys remain inside the ordered-step universe

Why this slice matters:

- it upgrades the branch path from pure structural durability to a small but
  useful active-run identity/state proof
- it stays on durable tracked-run truth and avoids the timing-sensitive branch
  completion or cleanup surfaces

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active branch runs preserve durable
  `flow_id` identity and remain in `Running` while delayed branch work is
  still in flight

Backtracks encountered:

- none in the final slice
- the key restraint was choosing durable active-run identity/state, not adding
  another timing-shaped branch status claim

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in active tracked-run truth without moving the
  canonical write boundary

Next likely step:

- continue promoting only active-branch facts that survive live snapshots
  without depending on cleanup timing or terminal visibility

## Slice 101 - MeerkatMachine proves attached-loop reset splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `reset()` seam with both an input-owned
  completion waiter and an ops-owned `wait_all` live at the same time, so the
  split is proved directly instead of inferred from separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with queued progress work, a live
    completion waiter, and an active background operation under `wait_all`:
    - `reset_runtime()` abandons the queued input immediately
    - the completion-waiter carrier clears immediately and resolves
      `RuntimeTerminated("runtime reset")`
    - runtime control phase returns to `Idle`
    - the authority-owned `wait_request_id` and pending wait carrier both
      survive reset
    - no executor `apply()` or `control()` call is used on the reset path
    - the ops-owned `wait_all` carrier only clears once the background
      operation itself settles

Why this slice matters:

- it proves directly that attached-loop reset already has the same split
  between input-owned and ops-owned waiting semantics that retire exposed
- it strengthens the Meerkat lifecycle matrix without changing any owner
  boundary

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-mob --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop reset
  clears input-owned completion waiters immediately while preserving the
  ops-owned `wait_all` carrier until the operation settles

Backtracks encountered:

- none in the final slice
- the important discipline was keeping the proof on current owner seams
  rather than simplifying reset into a hypothetical unified teardown

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this deepens lifecycle convergence, but it does not yet justify a write-side
  switch

Next likely step:

- continue selecting Meerkat lifecycle seams where current input-owned and
  ops-owned behavior still diverge and need to be frozen explicitly

## Slice 102 - MobMachine proves active collection-policy runs preserve durable identity and running status

Goal:

- take the same small durable run-identity/state step on the active
  collection-policy path that already held on the active branch and loop paths

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
    so the active collection tracked run must now preserve:
    - `flow_id == collect`
    - `status == Running`
  - kept the already-proven collection-path truths:
    - ordered steps
    - dependency/dependency-mode maps
    - condition flags
    - branch map
    - collection policy kind and quorum threshold
    - zeroed failure counters
    - materialized step-status keys remain inside the ordered-step universe

Why this slice matters:

- it upgrades the collection path from pure structural durability to a small
  active-run identity/state proof
- it stays on durable tracked-run truth and avoids timing-sensitive terminal
  or cleanup surfaces

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MobMachine` now has live evidence that active collection-policy runs
  preserve durable `flow_id` identity and remain in `Running` while delayed
  collection targets are still in flight

Backtracks encountered:

- none in the final slice
- the useful restraint was staying on active-run identity/state rather than
  promoting another timing-shaped collection status claim

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in active tracked-run truth without moving the
  canonical write boundary

Next likely step:

- continue promoting only collection-path facts that survive live snapshots
  without depending on cleanup timing or terminal visibility

## Slice 103 - MeerkatMachine proves attached-loop stop splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `stop_runtime_executor()` seam with both
  an input-owned completion waiter and an ops-owned `wait_all` live at the
  same time, so the split is proved directly instead of inferred from
  separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with queued progress work, a live
    completion waiter, and an active background operation under `wait_all`:
    - `stop_runtime_executor()` clears the completion-waiter carrier
      immediately and resolves it as `RuntimeTerminated("runtime stopped")`
    - runtime phase eventually publishes `Stopped`
    - the authority-owned `wait_request_id` and pending wait carrier both
      survive stop
    - queued ordinary work is preempted before attached-loop `apply()` runs
    - the attached executor observes exactly one stop control command
    - the ops-owned `wait_all` carrier only clears once the background
      operation itself settles

Why this slice matters:

- it completes the same direct split proof for stop that we already had for
  retire and reset
- it keeps the Meerkat lifecycle story grounded in current attached-loop owner
  behavior instead of a simplified teardown model

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop stop
  clears input-owned completion waiters immediately while preserving the
  ops-owned `wait_all` carrier until the operation settles

Backtracks encountered:

- the first attempt used a non-existent completion helper on the attached-loop
  path
- the final proof switched back to the real carrier seam by registering the
  completion waiter directly through the session completion registry

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting attached-loop lifecycle seams that expose surprising
  current owner behavior instead of assuming the lifecycle should already be
  uniform

## Slice 104 - MobMachine proves active any-policy runs preserve durable identity and defaults

Goal:

- extend the active `CollectionPolicy::Any` path from pure collection-policy
  shape into durable active-run identity and default scalar truth

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
    so the active any-policy tracked run must now preserve:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
  - kept the already-proven any-policy truths:
    - collection policy kind `Any`
    - zero quorum threshold
    - zeroed failure counters
    - at least one materialized step status
    - surfaced step-status keys remain inside the ordered-step universe

Why this slice matters:

- it upgrades the active any-policy path from policy-only durability to a
  cleaner active-run identity/defaults proof
- it still stays on durable tracked-run truth and avoids timing-sensitive
  terminal or cleanup surfaces

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active any-policy runs preserve
  durable run identity, running status, modern schema, and zeroed default
  retry/escalation limits while delayed targets are still in flight

Backtracks encountered:

- none in the final slice
- the useful restraint was adding only defaults already proven durable by the
  live tracked-run aggregate

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not yet justify a switch to
  write authority

Next likely step:

- continue promoting only active-path facts whose durability is proven by the
  live tracked-run aggregate rather than by timing-sensitive observer effects

## Slice 105 - MeerkatMachine proves attached-loop destroy splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `destroy()` seam with both an input-owned
  completion waiter and an ops-owned `wait_all` live at the same time, so the
  split is proved directly instead of inferred from separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with queued progress work, a live
    completion waiter, and an active background operation under `wait_all`:
    - `destroy()` abandons queued input immediately
    - the completion-waiter carrier clears immediately and resolves
      `RuntimeTerminated("runtime destroyed")`
    - runtime control phase becomes `Destroyed`
    - the authority-owned `wait_request_id` and pending wait carrier both
      survive destroy
    - queued ordinary work is bypassed before attached-loop `apply()` runs
    - destroy still bypasses the executor control seam on this path
    - the ops-owned `wait_all` carrier only clears once the background
      operation itself settles

Why this slice matters:

- it completes the same direct split proof for destroy that we already had for
  retire, reset, and stop
- it keeps the Meerkat lifecycle story grounded in current attached-loop owner
  behavior instead of a simplified teardown model

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop destroy
  clears input-owned completion waiters immediately while preserving the
  ops-owned `wait_all` carrier until the operation settles

Backtracks encountered:

- none in the final slice
- the important discipline was keeping the proof on the current destroy seam,
  including the fact that destroy still bypasses executor control here

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting attached-loop lifecycle seams that expose surprising
  current owner behavior instead of assuming the lifecycle should already be
  uniform

## Slice 106 - MobMachine proves active all-policy runs preserve durable identity and defaults

Goal:

- extend the active `CollectionPolicy::All` path from policy-only durability
  into durable active-run identity and default scalar truth

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
    so the active all-policy tracked run must now preserve:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
  - kept the already-proven all-policy truths:
    - collection policy kind `All`
    - zero quorum threshold
    - zeroed failure counters
    - at least one materialized step status
    - surfaced step-status keys remain inside the ordered-step universe

Why this slice matters:

- it upgrades the active all-policy path from policy-only durability to a
  cleaner active-run identity/defaults proof
- it still stays on durable tracked-run truth and avoids timing-sensitive
  terminal or cleanup surfaces

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active all-policy runs preserve
  durable run identity, running status, modern schema, and zeroed default
  retry/escalation limits while delayed targets are still in flight

Backtracks encountered:

- none in the final slice
- the useful restraint was adding only defaults already proven durable by the
  live tracked-run aggregate

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not yet justify a switch to
  write authority

Next likely step:

- continue promoting only active-path facts whose durability is proven by the
  live tracked-run aggregate rather than by timing-sensitive observer effects

## Slice 107 - MeerkatMachine proves attached-loop recover splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `recover()` seam with both an input-owned
  completion waiter and an ops-owned `wait_all` live at the same time, so the
  split is proved directly instead of inferred from separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with recoverable queued progress work,
    a live completion waiter, and an active background operation under
    `wait_all`:
    - `recover()` reports `inputs_recovered == 1`
    - the attached loop wakes exactly once and re-enters `Running` while
      replaying recovered work
    - the completion-waiter carrier survives through that replay phase
    - the authority-owned `wait_request_id` and pending wait carrier also
      survive through that same phase
    - no executor control command is routed through the attached executor seam
    - once the recovered queued work finishes replaying, completion waiters
      clear and runtime returns to `Attached`
    - the ops-owned `wait_all` carrier remains live after that point and only
      clears once the background operation itself settles

Why this slice matters:

- it completes the same direct split proof for recover that we already had for
  reset, stop, retire, and destroy
- it keeps the Meerkat lifecycle story grounded in current attached-loop owner
  behavior instead of a simplified replay model

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop recover
  clears input-owned completion waiters only after recovered work finishes
  replaying, while preserving the ops-owned `wait_all` carrier until the
  operation settles

Backtracks encountered:

- none in the final slice
- the important discipline was keeping the proof on the current recover seam,
  including the fact that recover returns to `Attached`, not `Idle`

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting attached-loop lifecycle seams that expose surprising
  current owner behavior instead of assuming the lifecycle should already be
  uniform

## Slice 108 - MobMachine proves active quorum-policy runs preserve durable identity and defaults

Goal:

- extend the active quorum collection path from policy-only durability into
  durable active-run identity and default scalar truth

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
    so the active quorum tracked run must now preserve:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
  - kept the already-proven quorum-policy truths:
    - ordered steps
    - dependency/dependency-mode maps
    - condition flags
    - branch map
    - collection policy kind `Quorum`
    - quorum threshold `2`
    - zeroed failure counters
    - surfaced step-status keys remain inside the ordered-step universe

Why this slice matters:

- it brings the active quorum path up to the same active-run identity/defaults
  bar that already held for the `Any` and `All` paths
- it still stays on durable tracked-run truth and avoids timing-sensitive
  terminal or cleanup surfaces

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active quorum-policy runs preserve
  durable run identity, running status, modern schema, and zeroed default
  retry/escalation limits while delayed targets are still in flight

Backtracks encountered:

- none in the final slice
- the useful restraint was adding only defaults already proven durable by the
  live tracked-run aggregate

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not yet justify a switch to
  write authority

Next likely step:

- continue promoting only active-path facts whose durability is proven by the
  live tracked-run aggregate rather than by timing-sensitive observer effects

## Slice 109 - MeerkatMachine proves attached-loop recycle splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `recycle()` seam with both an input-owned
  completion waiter and an ops-owned `wait_all` live at the same time, so the
  split is proved directly instead of inferred from separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with queued progress work, a live
    completion waiter, and an active background operation under `wait_all`:
    - `recycle()` reports `inputs_transferred == 1`
    - the attached loop wakes exactly once and re-enters `Running` while
      replaying preserved work
    - the completion-waiter carrier survives through that replay phase
    - the authority-owned `wait_request_id` and pending wait carrier also
      survive through that same phase
    - no executor control command is routed through the attached executor seam
    - once the preserved queued work finishes replaying, completion waiters
      clear and runtime returns to `Attached`
    - the ops-owned `wait_all` carrier remains live after that point and only
      clears once the background operation itself settles

Why this slice matters:

- it completes the same direct split proof for recycle that we already had for
  recover, reset, stop, retire, and destroy
- it keeps the Meerkat lifecycle story grounded in current attached-loop owner
  behavior instead of a simplified replay model

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop recycle
  clears input-owned completion waiters only after preserved work finishes
  replaying, while preserving the ops-owned `wait_all` carrier until the
  operation settles

Backtracks encountered:

- none in the final slice
- the important discipline was keeping the proof on the current recycle seam,
  including the fact that recycle returns to `Attached`, not `Idle`

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting attached-loop lifecycle seams that expose surprising
  current owner behavior instead of assuming the lifecycle should already be
  uniform

## Slice 110 - MobMachine proves active root-frame runs preserve durable identity and running status

Goal:

- extend the active root-frame path from structural durability into a small
  active-run identity/state proof

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
    so the active root-frame tracked run must now preserve:
    - `flow_id == demo`
    - `status == Running`
  - kept the already-proven root-frame truths:
    - `schema_version == 4`
    - `frame_count > 0`
    - no loop or loop-iteration state
    - durable step/dependency/kernel maps for the lone root-frame step
    - zeroed failure counters
    - zero retry/escalation defaults
    - `completed_at_present == false`

Why this slice matters:

- it upgrades the root-frame path from pure structure/defaults durability to a
  small but useful active-run identity/state proof
- it still stays on durable tracked-run truth and avoids timing-sensitive
  terminal or cleanup surfaces

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active root-frame runs preserve
  durable run identity and remain in `Running` while the never-terminal root
  frame keeps the run alive

Backtracks encountered:

- none in the final slice
- the useful restraint was adding only the small active-run identity/state
  facts already supported by the live tracked-run aggregate

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not yet justify a switch to
  write authority

Next likely step:

- continue promoting only active-path facts whose durability is proven by the
  live tracked-run aggregate rather than by timing-sensitive observer effects

## Slice 111 - MeerkatMachine proves registered destroy splits completion and wait-all lifetimes

Goal:

- pin down the non-attached destroy path with both input-owned completion
  waiters and ops-owned `wait_all` active at the same time

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes`
  - the new test proves that on the registered idle path:
    - queued prompt work can still carry a live completion waiter when
      destroy begins
    - an active background op can still own a live `wait_all` request at the
      same time
    - `destroy()` abandons the queued input immediately
    - the input-owned completion waiter clears immediately and resolves with
      `RuntimeTerminated("runtime destroyed")`
    - runtime control phase becomes `Destroyed`
    - the ops-owned `wait_all` carrier and `WaitRequestId` remain live until
      the background op itself settles
    - the runtime stays `Destroyed` after that settlement

Why this slice matters:

- it completes the split-lifetime proof on the non-attached destroy path,
  not just the attached-loop path
- it strengthens the Meerkat lifecycle map by showing that destroy is
  consistent across both runtime topologies: input-owned waiters are torn
  down immediately while ops-owned waits remain tied to op truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that registered
  destroy splits completion-waiter and `wait_all` lifetimes the same way the
  attached-loop destroy path already did

Backtracks encountered:

- none in the final slice
- the useful read was that the previously proven attached-loop split was not
  a special-case anomaly; the registered path exhibits the same ownership
  boundary

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting Meerkat lifecycle seams that compare attached and
  non-attached behavior without assuming they should already be uniform

## Slice 112 - MobMachine proves active branch runs preserve durable schema and defaults

Goal:

- raise the active branch path to the same durability/defaults bar already
  proven for the collection-policy paths

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
    so the active branch tracked run must now also preserve:
    - `schema_version == 4`
    - `completed_at_present == false`
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
  - kept the already-proven branch truths:
    - `flow_id == branching`
    - `status == Running`
    - branch/dependency/kernel map durability for the active branch run

Why this slice matters:

- it brings the active branch path up to the same modern-schema/defaults
  durability bar as the active `All` / `Any` / `Quorum` collection paths
- it still stays on durable tracked-run truth and avoids timing-sensitive
  step-status or output projection surfaces

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active branch runs preserve the
  same durable schema/default scalar truths as the active collection paths,
  alongside the branch/dependency facts already frozen

Backtracks encountered:

- none in the final slice
- the useful restraint was keeping the slice on durable tracked-run scalars
  instead of retrying a timing-sensitive branch step-status promotion

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active branch facts that survive the live tracked-run
  aggregate as cleanly as the collection paths did

## Slice 113 - MeerkatMachine proves registered reset splits completion and wait-all lifetimes

Goal:

- close the non-attached reset gap by proving that registered `reset()`
  enforces the same input/ops lifetime split already observed on attached
  runtimes

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
  - the new test proves that on the registered idle path:
    - queued prompt work can still carry a live completion waiter when reset
      begins
    - an active background op can still own a live `wait_all` request at the
      same time
    - `reset()` abandons the queued input immediately
    - the input-owned completion waiter clears immediately and resolves with
      `RuntimeTerminated("runtime reset")`
    - runtime control phase returns to `Idle`
    - the ops-owned `wait_all` carrier and `WaitRequestId` remain live until
      the background op itself settles
    - the runtime stays `Idle` after that settlement

Why this slice matters:

- it extends the registered lifecycle map beyond destroy and shows the same
  ownership split holds for reset too
- it strengthens the evidence that the input/ops lifetime boundary is a real
  Meerkat semantic seam, not an attached-runtime quirk

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that registered reset
  splits completion-waiter and `wait_all` lifetimes the same way registered
  destroy and attached-loop reset already did

Backtracks encountered:

- none in the final slice
- the useful read was that registered reset followed the expected owner split
  cleanly, so the boundary stayed stable rather than forcing another
  lifecycle-specific exception

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting registered lifecycle seams whose combined split proof is
  still missing, rather than assuming the separate carrier tests are already
  enough

## Slice 114 - MobMachine proves active branch runs preserve healthy failure counters

Goal:

- raise the active branch path to the same healthy-counter bar already proven
  for the active collection-policy and frame/loop paths

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
    so the active branch tracked run must now also preserve:
    - `failure_count == 0`
    - `consecutive_failure_count == 0`
  - kept the already-proven branch truths:
    - `flow_id == branching`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - zero retry/escalation defaults
    - branch/dependency/kernel map durability for the active branch run

Why this slice matters:

- it brings the active branch path up to the same healthy-run counter bar as
  the collection and frame/loop paths
- it still stays on durable tracked-run truth and avoids timing-sensitive
  terminal or cleanup surfaces

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active branch runs preserve the same
  healthy failure-counter defaults as the other active tracked-run shapes we
  already trust

Backtracks encountered:

- none in the final slice
- the useful restraint was promoting only the healthy counters the live branch
  path already owned durably, instead of reaching again for timing-sensitive
  branch step-status coverage

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active branch facts that survive the live tracked-run
  aggregate as cleanly as the collection, frame, and loop paths did

## Slice 115 - MeerkatMachine proves registered stop splits completion and wait-all lifetimes

Goal:

- close the non-attached stop gap by proving that registered
  `stop_runtime_executor()` enforces the same input/ops lifetime split
  already observed on attached runtimes

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
  - the new test proves that on the registered idle path:
    - queued prompt work can still carry a live completion waiter when stop
      begins
    - an active background op can still own a live `wait_all` request at the
      same time
    - `stop_runtime_executor()` tears down the queued input immediately
    - the input-owned completion waiter clears immediately and resolves with
      `RuntimeTerminated("runtime stopped")`
    - runtime control phase becomes `Stopped`
    - the ops-owned `wait_all` carrier and `WaitRequestId` remain live until
      the background op itself settles
    - the runtime stays `Stopped` after that settlement

Why this slice matters:

- it extends the registered lifecycle split map beyond destroy and reset and
  shows the same ownership boundary holds for stop too
- it gives us the direct combined proof for stop instead of relying on two
  separate carrier-level observations

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that registered stop
  splits completion-waiter and `wait_all` lifetimes the same way registered
  destroy/reset and attached-loop stop already did

Backtracks encountered:

- none in the final slice
- the useful read was that registered stop followed the same owner split
  cleanly, so the boundary stayed stable rather than forcing a stop-specific
  exception

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue selecting registered lifecycle seams whose combined split proof is
  still missing, especially where separate completion and wait-all tests are
  already green but not yet unified

## Slice 116 - MobMachine proves active branch runs preserve structural absence of frames and loops

Goal:

- raise the active branch path to the same structural clarity bar as the
  root-frame and loop paths by proving that plain branch flows stay free of
  frame/loop state

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
    so the active branch tracked run must now also preserve:
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
  - kept the already-proven branch truths:
    - `flow_id == branching`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - zero retry/escalation defaults
    - zero healthy failure counters
    - branch/dependency/kernel map durability for the active branch run

Why this slice matters:

- it establishes that the active branch path is not just healthy and
  schema-modern, but also structurally plain: no persisted frame or loop
  machinery leaks into this tracked-run shape
- it still stays on durable tracked-run truth and avoids timing-sensitive
  step-status or output surfaces

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active branch runs preserve the
  expected absence of frame and loop structure, not just branch-specific
  kernel maps and healthy scalar defaults

Backtracks encountered:

- none in the final slice
- the useful restraint was again structural: only facts the live tracked-run
  aggregate clearly owned were promoted

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active branch facts that survive the live tracked-run
  aggregate as cleanly as the collection, frame, and loop paths did

## Slice 117 - MeerkatMachine proves registered retire splits completion and wait-all lifetimes

Goal:

- complete the registered lifecycle split matrix by proving that plain
  `retire()` without a live runtime loop follows the same ownership boundary
  as the other registered teardown paths

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
  - the new test proves that:
    - a registered runtime can carry queued work plus a live completion waiter
      before retire
    - a background op can simultaneously keep an ops-owned `wait_all` carrier
      live
    - `retire()` abandons queued work immediately
    - the completion waiter resolves with
      `RuntimeTerminated("retired without runtime loop")`
    - runtime control phase moves to `Retired`
    - input-owned completion waiters clear immediately
    - ops-owned `wait_all` and its `WaitRequestId` remain live until the
      background op itself settles

Why this slice matters:

- it closes the remaining plain registered teardown path in the
  completion-vs-wait-all split matrix
- it confirms that current registered retire semantics match the ownership
  pattern already observed for registered reset, destroy, and stop

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has direct registered-lifecycle evidence that `retire()`
  tears down input-owned completion waiters immediately while preserving the
  ops-owned `wait_all` carrier until the underlying op settles

Backtracks encountered:

- none in the final slice
- the useful restraint was keeping the proof on current registered owner truth
  instead of trying to fold in attached-loop behavior again

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue closing the remaining registered-vs-attached lifecycle seams only
  where current owner behavior is still not directly proven

## Slice 118 - MobMachine proves active collection runs preserve structural absence of frames and loops

Goal:

- raise the active quorum collection path to the same structural clarity bar as
  the branch path by proving that plain collection flows stay free of frame and
  loop structure

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
    so the active tracked quorum collection run must now also preserve:
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
  - kept the already-proven collection truths:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - zero retry/escalation defaults
    - zero healthy failure counters
    - durable quorum collection-policy shape

Why this slice matters:

- it establishes that the active quorum collection path is not just policy
  shaped and healthy, but also structurally plain: no persisted frame or loop
  machinery leaks into this tracked-run shape
- it continues to stay on durable tracked-run truth and avoids timing-sensitive
  step-status or output surfaces

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active quorum collection runs
  preserve the expected absence of frame and loop structure, not just policy
  shape and healthy scalar defaults

Backtracks encountered:

- none in the final slice
- the useful restraint was again structural: only facts the live tracked-run
  aggregate clearly owned were promoted

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active collection facts that survive the live
  tracked-run aggregate as cleanly as the branch, frame, and loop paths did

## Slice 119 - MeerkatMachine proves registered recover splits completion and wait-all lifetimes

Goal:

- add the first direct combined registered recovery proof so plain `recover()`
  is covered as one owner-backed lifecycle seam instead of only through
  separate completion-waiter and wait-all tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes`
  - the new test proves that:
    - a registered idle runtime can carry queued work plus an input-owned
      completion waiter before recover
    - the same registered runtime can simultaneously carry an ops-owned
      `wait_all` carrier
    - `recover()` preserves both carriers and the queued input immediately
      after recovery
    - once the background operation settles, `wait_all` clears first
    - the queued input and its completion waiter remain live afterward
      because the recovered work is still queued and unexecuted

Why this slice matters:

- it shows that plain registered recover is not merely “preserve everything”
  folklore; the runtime actually has a visible lifetime split:
  `wait_all` can settle while the recovered queued input is still pending
- it closes a meaningful gap between the teardown family, which now has direct
  split proofs, and the plain recovery family, which had only piecemeal
  coverage before this slice

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has direct registered recover evidence that queued-input
  completion waiters can legitimately outlive the ops-owned `wait_all` carrier
  on the same recovered runtime

Backtracks encountered:

- none in the final slice
- the useful restraint was keeping the proof on a plain registered path rather
  than mixing it with the already-proven attached-loop replay behavior

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue closing the remaining plain recovery lifecycle seams only where the
  current owner behavior is still covered piecemeal rather than directly

## Slice 120 - MobMachine proves active any-policy runs preserve structural absence of frames and loops

Goal:

- raise the active `CollectionPolicy::Any` path to the same structural clarity
  bar as the quorum collection path by proving that any-policy collection flows
  stay free of frame and loop structure

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
    so the active tracked any-policy run must now also preserve:
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
  - kept the already-proven any-policy truths:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - zero retry/escalation defaults
    - zero healthy failure counters
    - at least one materialized step status
    - durable any-policy kind and zero quorum threshold

Why this slice matters:

- it establishes that the active any-policy collection path is not just
  policy-shaped and healthy, but also structurally plain: no persisted frame
  or loop machinery leaks into this tracked-run shape
- it keeps the Mob side on durable tracked-run truth and avoids the
  timing-sensitive active-step-status surfaces that have forced backtracks
  elsewhere

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active any-policy collection runs
  preserve the expected absence of frame and loop structure, not just policy
  shape and healthy scalar defaults

Backtracks encountered:

- none in the final slice
- the useful restraint was again structural: only facts the live tracked-run
  aggregate clearly owned were promoted

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active collection facts that survive the live
  tracked-run aggregate as cleanly as the branch, frame, and loop paths did

## Slice 121 - MeerkatMachine proves registered recycle splits completion and wait-all lifetimes

Goal:

- add the direct combined registered recycle proof so plain `recycle()` is
  covered as one owner-backed lifecycle seam instead of only through separate
  completion-waiter and wait-all tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes`
  - the new test proves that:
    - a registered idle runtime can carry queued work plus an input-owned
      completion waiter before recycle
    - the same registered runtime can simultaneously carry an ops-owned
      `wait_all` carrier
    - `recycle()` preserves both carriers and the queued input immediately
      after recycle
    - once the background operation settles, `wait_all` clears first
    - the transferred queued input and its completion waiter remain live
      afterward because the recycled work is still pending and unexecuted

Why this slice matters:

- it gives plain registered recycle the same direct split-lifetime treatment we
  already established for registered recover and the teardown family
- it makes current recycle semantics explicit: recycle is not a teardown path;
  it preserves queued input ownership while allowing the ops-owned wait carrier
  to settle independently

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now has direct registered recycle evidence that queued-input
  completion waiters can legitimately outlive the ops-owned `wait_all` carrier
  on the same recycled runtime

Backtracks encountered:

- none in the final slice
- the useful restraint was keeping the proof on a plain registered path rather
  than mixing it with the already-proven attached-loop replay behavior

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, but not a write-side switch point yet

Next likely step:

- continue closing the remaining plain recovery/lifecycle seams only where the
  current owner behavior is still covered piecemeal rather than directly

## Slice 122 - MobMachine proves active all-policy runs preserve structural absence of frames and loops

Goal:

- raise the active `CollectionPolicy::All` path to the same structural clarity
  bar as the quorum and any-policy collection paths by proving that all-policy
  collection flows stay free of frame and loop structure

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
    so the active tracked all-policy run must now also preserve:
    - `frame_count == 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
  - kept the already-proven all-policy truths:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - zero retry/escalation defaults
    - zero healthy failure counters
    - at least one materialized step status
    - durable all-policy kind and zero quorum threshold

Why this slice matters:

- it brings the last active collection-policy path up to the same plain
  structural bar as quorum and any-policy
- it continues to stay on durable tracked-run truth and avoids the
  timing-sensitive active-step-status or output surfaces that still do not
  survive the live aggregate cleanly enough

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active all-policy collection runs
  preserve the expected absence of frame and loop structure, not just policy
  shape and healthy scalar defaults

Backtracks encountered:

- none in the final slice
- the useful restraint was again structural: only facts the live tracked-run
  aggregate clearly owned were promoted

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is good convergence evidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active collection facts that survive the live
  tracked-run aggregate as cleanly as the branch, frame, and loop paths did

## Slice 123 - MeerkatMachine rejects retired snapshots that still carry completion waiters

Goal:

- lift a lifecycle conclusion we have now proven repeatedly into the executable
  Meerkat validator: once the runtime is actually `Retired`, input-owned
  completion waiters should already be gone

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added
    `MeerkatMachineInvariantViolation::RetiredRuntimeStillHasCompletionWaiters`
  - extended `validate_meerkat_machine_snapshot(...)` to reject snapshots where:
    - `control.phase == RuntimeState::Retired`
    - `completion_waiters.waiter_count > 0`
  - added
    `validate_meerkat_machine_snapshot_reports_retired_completion_waiters`
    to prove the validator reports this shape directly

Why this slice matters:

- the direct lifecycle proofs now show a stable pattern:
  registered retire tears completion waiters down immediately, and attached
  retire only carries them while the runtime is temporarily back in `Running`
  for drain replay
- encoding that into the validator turns an observed lifecycle truth into a
  machine-level invariant instead of leaving it implicit in test folklore

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_retired_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now rejects a stale retired snapshot shape that the current
  owner-backed lifecycle proofs say should never survive into steady-state

Backtracks encountered:

- none in the final slice
- the useful restraint was grounding the new validator rule in the already
  converged retire proofs rather than guessing from analogy alone

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger executable invariant, but not a write-side switch point

Next likely step:

- continue harvesting validator-grade invariants only where the lifecycle
  matrix has already converged enough to support them cleanly

## Slice 124 - MobMachine proves active all-policy runs preserve durable single-step kernel maps

Goal:

- bring the active `CollectionPolicy::All` path up another notch by proving it
  preserves the same durable single-step kernel maps we already trust on the
  quorum collection and root-frame paths

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
    so the active tracked all-policy run must now also preserve:
    - `ordered_steps == ["collect"]`
    - `step_dependencies == { collect: [] }`
    - `step_dependency_modes == { collect: All }`
    - `step_has_conditions == { collect: false }`
    - `step_branches == { collect: None }`
  - kept the already-proven all-policy truths:
    - `flow_id == collect`
    - `status == Running`
    - `schema_version == 4`
    - `completed_at_present == false`
    - structural absence of frames/loops
    - zero retry/escalation defaults
    - zero healthy failure counters
    - at least one materialized step status
    - durable all-policy kind and zero quorum threshold

Why this slice matters:

- it extends the active all-policy path from “plain healthy scalar shape” to
  “durable single-step kernel map shape,” which is a more useful convergence
  signal for future flow-kernel cutover work
- it still stays on durable tracked-run truth and avoids timing-sensitive
  output or full-status-coverage claims

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_retired_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MobMachine` now has live evidence that active all-policy collection runs
  preserve not just policy/default scalars and structural plainness, but also
  the durable single-step kernel maps underneath them

Backtracks encountered:

- none in the final slice
- the useful restraint was promoting only the single-step kernel maps the live
  tracked-run aggregate already carries durably

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves convergence confidence, but it does not justify a switch to
  write authority yet

Next likely step:

- continue promoting only active tracked-run facts that survive the live
  aggregate as clearly as the collection, branch, frame, and loop paths do

## Slice 87 - MeerkatMachine makes attached-loop reset semantics explicit

Goal:

- pin down what current `reset()` does when an attached runtime still has
  queued work and completion waiters, without assuming it behaves like stop,
  destroy, or retire

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can carry queued work plus a live completion waiter
      before reset
    - `reset()` abandons that queued work immediately
    - completion waiters are cleared immediately
    - the waiter resolves with `RuntimeTerminated("runtime reset")`
    - runtime control phase returns to `Idle`
    - current reset does not call the attached executor's `apply()` or
      `control()` hooks on this path

Why this slice matters:

- it closes the last obvious missing attached-loop completion-waiter lifecycle
  seam alongside stop, retire, and destroy
- it makes current reset semantics explicit instead of inferred:
  reset is a hard queued-work teardown, but it returns to `Idle` rather than
  `Stopped` or `Destroyed`

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop reset
  bypasses executor control and clears queued-input completion waiters
  immediately while returning the runtime to `Idle`

Backtracks encountered:

- no semantic backtrack was required in the final slice
- the useful restraint was checking the control authority before asserting the
  post-reset phase, rather than guessing from the other lifecycle paths

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another concrete lifecycle read, not a write-authority switch signal

Next likely step:

- keep selecting attached-loop lifecycle seams where current owner behavior is
  still easy to misread from analogy alone

## Slice 88 - MobMachine strengthens the live root-frame proof with healthy structured state

Goal:

- extend the existing active root-frame tracked-run proof using only facts the
  live aggregate already sustains cleanly

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
  - the live proof now requires that an active root-frame tracked run preserves:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - `failure_count == 0`
    - `consecutive_failure_count == 0`
    - `completed_at_present == false`

Why this slice matters:

- it broadens the healthy active-run durability proof outside collection and
  loop flows, without introducing any new tracked-run field
- it confirms that root-frame structured state and zeroed failure counters are
  stable owner-backed truth for `MobMachine`

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has a stronger active root-frame proof while still staying
  on non-timing-sensitive tracked-run facts

Backtracks encountered:

- no new red run in the final slice
- this was chosen specifically as a smaller structural proof after earlier loop
  and branch work showed where active step-status timing is still too soft

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run structural truth without moving write
  authority

Next likely step:

- continue preferring active-run structure and healthy counters over timing- or
  cleanup-shaped facts until the live path proves otherwise

## Slice 83 - MeerkatMachine makes attached-loop destroy semantics explicit

Goal:

- observe what current `destroy()` does to queued work and completion waiters
  when a runtime still has a live attached executor, without projecting target
  semantics onto the path

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can carry queued input plus a live completion waiter
      before destroy
    - `destroy()` abandons that queued input immediately
    - completion waiters are cleared immediately
    - the waiter future resolves with
      `RuntimeTerminated(\"runtime destroyed\")`
    - the runtime enters `RuntimeState::Destroyed`
    - current destroy does not call the attached executor's `apply()` or
      `control()` hooks on this path

Why this slice matters:

- it sharpens another lifecycle seam where attached-loop behavior could easily
  have been guessed wrong
- it gives the observational model a concrete answer for destroy with a live
  executor: current code bypasses executor control and tears down queued-input
  completion state synchronously

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop destroy is
  a hard teardown for queued input waiters, even though ops-owned `wait_all`
  remains a distinct lifecycle story

Backtracks encountered:

- no code-level backtrack was required in the final slice
- the important restraint was not assuming destroy would coordinate through the
  executor just because stop/retire sometimes do

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a better picture of current destroy semantics, not a write-authority
  cutover signal

Next likely step:

- keep selecting Meerkat lifecycle seams where attached-loop and no-loop paths
  genuinely diverge in current owner behavior

## Slice 84 - MobMachine backs out root-output count and stays on stable branch truth

Goal:

- test whether durable root output count belongs in the tracked-run MobMachine
  snapshot today, and back out cleanly if the live owner path does not support
  it

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - removed `root_output_count` from `TrackedRunStoreSnapshot`
  - removed the synthetic validator branch that treated root-output count as a
    tracked-run invariant
- `meerkat-mob/src/runtime/tests.rs`
  - restored
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    wait for the smaller active-run truth the runtime actually surfaces today:
    materialized step status
  - removed the attempted durable-root-output proof after the live code showed
    it was not an owner-backed fact on this path

Why this slice matters:

- it is a clean example of the method working as intended: a synthetic fact
  parsed cleanly, but the real system did not actually own it strongly enough
  to promote into `MobMachine`
- backing it out avoids building a second, cleaner architecture on top of
  fields the live runtime does not yet sustain

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_branch_dependency_conflicts`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` keeps the stable active-branch proof surface
- root-output count is not treated as current tracked-run kernel truth

Backtracks encountered:

- the first live branch proof timed out waiting for active tracked runs to
  surface durable root outputs
- a direct durable-store proof then also failed: the terminal branch run did
  not preserve root outputs on this live path either
- that combination was the decision point to remove the slice instead of
  rationalizing it

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is a convergence improvement because it removes a false-positive fact
  from the observational model

Next likely step:

- continue preferring frame/loop structure and other clearly durable tracked-run
  facts over output durability until the live owner path proves otherwise

## Slice 85 - MeerkatMachine makes attached-loop destroy plus wait_all semantics explicit

Goal:

- pin down what current `destroy()` does when an attached runtime has both
  queued work and an active ops-owned `wait_all`, without assuming it behaves
  like stop or retire

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can hold queued input plus an active authority-owned
      wait request at the same time
    - `destroy()` abandons the queued input immediately
    - `destroy()` preserves the ops-owned `wait_request_id`
    - the pending wait carrier remains present and keeps the same
      `WaitRequestId`
    - the tracked wait target remains visible until the operation settles
    - runtime control phase becomes `Destroyed`
    - current destroy bypasses both executor `apply()` and executor `control()`
      on this path

Why this slice matters:

- it closes a real lifecycle gap between the attached-loop completion-waiter
  destroy proof and the previously idle-only `wait_all` destroy proof
- it makes the current asymmetry explicit:
  - queued-input completion waiters are torn down immediately
  - ops-owned `wait_all` state survives destroy until the operation itself
    resolves

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop destroy
  behaves like idle destroy for `wait_all`, even though it hard-tears down
  queued-input completion waiters on the same path

Backtracks encountered:

- the first red run was only a harness/API mismatch:
  - wrong `OperationResult` fields
  - wrong `wait_all` assertion shape
- after aligning to the real API, the semantic claim held without further
  changes

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger picture of current destroy semantics, not a write-authority
  cutover signal

Next likely step:

- keep selecting Meerkat lifecycle seams where current behavior is surprising
  enough that we should name it explicitly before any cutover decision

## Slice 86 - MobMachine keeps loop structure and backs off active step-status timing

Goal:

- strengthen the live loop-flow proof only as far as the current tracked-run
  path genuinely supports it

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the test now proves that an active loop-aware tracked run preserves:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count > 0`
    - `loop_iteration_count > 0`
    - `failure_count == 0`
    - `consecutive_failure_count == 0`
    - `completed_at_present == false`

Why this slice matters:

- it extends the healthy active-run durability proofs into the loop-aware flow
  path without introducing another new tracked-run field
- it confirms that loop structure and healthy failure counters are stable owner
  truth for `MobMachine`

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has a stronger live proof for active loop-aware tracked runs
  while staying on facts the aggregate actually sustains today

Backtracks encountered:

- the first version required active loop runs to surface materialized
  `step_statuses`
- that timed out on the live path, which told us active loop step-status timing
  is not a stable proof surface yet
- the final slice backed off cleanly to loop structure plus healthy failure
  counters instead of encoding timing folklore

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is convergence toward stable tracked-run truth, not a switch signal

Next likely step:

- continue preferring loop/frame structure and healthy-run counters over active
  step-status timing until the loop path proves otherwise

## Slice 75 - MeerkatMachine makes attached-loop retire plus wait_all semantics explicit

Goal:

- pin down the current `retire()` behavior when two different carriers are live
  at once:
  - attached-loop queued work that must drain
  - an ops-owned `wait_all` request that is still waiting on a background
    operation

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop`
  - the new test proves that:
    - retire preserves the authority-owned `wait_request_id`
    - retire preserves the shell-owned pending wait carrier and their shared
      `WaitRequestId`
    - retire reports `inputs_pending_drain == 1` when queued work is handed to
      the attached loop
    - control phase temporarily re-enters `Running` while that queued work is
      draining
    - once the attached loop finishes draining, control returns to `Retired`
      even though the ops-owned `wait_all` is still active
    - the wait carrier only clears after the tracked operation itself settles

Why this slice matters:

- it exposes a real lifecycle asymmetry instead of assuming retire is a single
  phase transition
- it shows that the current runtime already treats attached-loop drain and
  ops-owned waits as distinct semantic surfaces during retire

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that current retire semantics
  can pass through:
  - `Attached`
  - temporary `Running` drain
  - back to `Retired`
  while preserving active `wait_all` state until the tracked operation itself
  completes

Backtracks encountered:

- no code-level backtrack in the final slice
- the key design choice was to avoid inventing a target retire policy and only
  record the exact handoff the current owners implement

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a better map of current retire semantics, not a switch-ready write
  authority

Next likely step:

- keep selecting Meerkat lifecycle seams where attached-loop and non-loop
  behavior diverge, because that is where current semantic truth is still most
  likely to surprise us

## Slice 76 - MobMachine makes loop presence imply frame structure

Goal:

- promote one more durable tracked-run structural fact without stepping into
  timing-sensitive flow behavior

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `TrackedRunLoopWithoutFrameStructure`
  - `validate_mob_machine_snapshot(...)` now rejects tracked runs that expose
    persisted loop state without any persisted frame structure
  - added
    `validate_mob_machine_snapshot_reports_loop_without_frame_structure`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the live loop-path proof now waits for and asserts:
    - `loop_count > 0`
    - `frame_count > 0`
    - `loop_iteration_count > 0`

Why this slice matters:

- it turns loop presence from an isolated counter into a structural relation
  over the durable run aggregate
- it keeps the Mob side moving through persisted frame/loop truth rather than
  timing-shaped active execution behavior

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_loop_without_frame_structure`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has explicit evidence that persisted loop state does not
  float on its own in the current durable aggregate; it remains anchored to
  persisted frame structure

Backtracks encountered:

- no model backtrack in the final slice
- the important restraint was keeping this purely structural and not promoting
  broader loop execution semantics before the durable owner path is tighter

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is still diagnostic read-side convergence, not a write-authority cutover

Next likely step:

- continue promoting only those Mob tracked-run facts that stay durable and
  structurally visible through the live aggregate path

## Slice 77 - MeerkatMachine makes attached-loop stop plus wait_all semantics explicit

Goal:

- pin down current `stop_runtime_executor()` behavior when an attached loop has:
  - queued work that has not started applying yet
  - an active ops-owned `wait_all` request over a background operation

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop`
  - the new test proves that:
    - attached-loop stop preserves the authority-owned `wait_request_id`
    - attached-loop stop preserves the shell-owned pending wait carrier and
      request-id agreement
    - stop preempts queued attached-loop work before `apply()` runs
    - the attached executor observes exactly one
      `StopRuntimeExecutor` control command
    - active `wait_all` still resolves normally once the tracked operation
      settles

Why this slice matters:

- it fills the gap between:
  - existing non-loop `wait_all` stop coverage
  - existing attached-loop completion-waiter stop coverage
- it gives us a clearer Meerkat lifecycle map for attached stop semantics:
  queued work and ops waits do not behave the same way

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop stop:
  - can preempt queued work before apply
  - does not cancel active `wait_all`
  - eventually publishes `RuntimeState::Stopped`
    while preserving the wait carrier until the target operation completes

Backtracks encountered:

- the first version of the test assumed `stop_runtime_executor()` would publish
  `Stopped` synchronously on the attached-loop path
- the red run showed that this was wrong: when a live control sender exists,
  stop queues the control command and returns before the phase necessarily
  flips
- I backtracked the test to wait for the control-phase publication instead of
  weakening the semantic claim about preserved `wait_all`

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a better map of current attached-loop stop semantics, not a
  switch-ready write authority

Next likely step:

- keep selecting Meerkat lifecycle seams where loop-attached behavior differs
  from no-loop behavior, because those continue to reveal the highest-value
  current semantics

## Slice 78 - MobMachine surfaces schema version for structured tracked-run state

Goal:

- promote one more durable tracked-run fact that helps distinguish legacy run
  shape from modern frame/loop-aware run shape without stepping into cleanup or
  active-execution timing

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - `TrackedRunStoreSnapshot` now carries `schema_version`
  - `tracked_run_snapshot_from_run(...)` now projects `MobRun.schema_version`
  - added
    `TrackedRunStructuredStateWithLegacySchemaVersion`
  - validator now rejects tracked runs that expose frame/loop/loop-iteration
    structure while still claiming legacy schema state
  - added
    `validate_mob_machine_snapshot_reports_structured_state_with_legacy_schema_version`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the live loop-path proof now also asserts `schema_version == 4`

Why this slice matters:

- it turns schema version from passive storage metadata into an explicit
  structural fact in the joined MobMachine view
- it gives us one more durable guardrail against accidentally treating modern
  frame/loop state as if it belonged to a legacy aggregate shape

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_structured_state_with_legacy_schema_version`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has explicit evidence that structured loop-aware tracked
  runs surface both:
  - persisted frame/loop structure
  - the modern schema version that owns that structure

Backtracks encountered:

- no semantic backtrack in the final slice
- the main care point was mechanical: every synthetic tracked-run fixture had
  to be updated so the new field stayed explicit rather than silently defaulted

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves durable tracked-run shape checking, but it is still not a
  write-authority cutover signal

Next likely step:

- continue preferring durable tracked-run structure or durable output shape over
  flow behavior that still depends on timing or cleanup visibility

## Slice 75 - MeerkatMachine makes retire-with-live-loop drain semantics explicit

Goal:

- observe the current completion-waiter handoff when `retire()` preserves
  queued work for an attached runtime loop to drain

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can hold a queued completion waiter before retire
    - `retire()` preserves that queued work instead of abandoning it
    - the completion waiter carrier remains present across the handoff
    - the preserved work drains exactly once through the attached runtime loop
    - the completion waiter carrier clears only after the drained work settles

Why this slice matters:

- it closes the gap between the existing no-loop retire proof and the real
  attached-loop retire path
- it captures a subtle but important current behavior: retire on an attached
  runtime is not a steady `Retired` plateau while preserved work drains

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that current retire semantics
  hand preserved queued work to the live loop and keep the completion-waiter
  carrier intact until that work finishes

Backtracks encountered:

- the first version of the test assumed the snapshot would remain in
  `RuntimeState::Retired` immediately after retire
- that was false: once retire wakes the attached loop, current code
  temporarily re-enters `RuntimeState::Running` while draining preserved work,
  then returns to `Retired` after completion
- the test was updated to record that current semantic instead of forcing a
  cleaner but incorrect story

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger lifecycle-handoff proof, but it is still describing
  current owner behavior rather than freezing target-state retire semantics

Next likely step:

- keep selecting Meerkat lifecycle seams where live-loop and no-loop behavior
  diverge, because those handoffs are where cutover risk is highest

## Slice 76 - MobMachine surfaces live loop-iteration ledger presence

Goal:

- promote one more durable structural fact from the flow-run aggregate without
  inventing a new behavioral invariant

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - `TrackedRunStoreSnapshot` now carries `loop_iteration_count`
  - tracked-run capture now maps `MobRun.loop_iteration_ledger.len()`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so it waits for and asserts both:
    - `loop_count > 0`
    - `loop_iteration_count > 0`

Why this slice matters:

- it moves `MobMachine` one step past “a loop exists” toward “the loop’s
  durable execution ledger is visible in the joined machine view”
- it stays structural and durable; it does not assume terminal loop outcomes or
  cleanup timing

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that the joined tracked-run view can see
  persisted loop-iteration ledger rows, not just loop snapshot presence

Backtracks encountered:

- no code-level backtrack in the final slice
- the useful restraint was not adding a new validator rule until the live loop
  path first proved the ledger is stably observable

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves structural coverage of tracked runs, but it does not yet imply
  write-authority readiness

Next likely step:

- continue preferring durable flow-run structure over cleanup-shaped behavior,
  especially for frames, loops, and iteration ledgers

## Slice 81 - MeerkatMachine maps retire-side carrier asymmetry explicitly

Goal:

- pin down current `retire()` semantics for input completion waiters and
  ops-owned `wait_all` without prematurely normalizing them into one policy

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop`
  - added `meerkat_machine_spine_snapshot_preserves_wait_all_after_retire`
  - the new tests prove that:
    - retiring a queued input with no live runtime loop abandons the input
      immediately
    - that same no-loop retire path clears completion waiters immediately
    - an active ops-owned `wait_all` survives retire
    - the authority-owned `wait_request_id` and shell-owned pending wait
      carrier stay aligned across retire
    - the surviving `wait_all` future still resolves once the tracked
      operation completes

Why this slice matters:

- it closes the remaining lifecycle gap in the current carrier map
- it makes the current asymmetry explicit:
  - input waiters are tied to queue-drain availability
  - ops waiters are tied to operation lifecycle authority instead

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_retire`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that current retire semantics
  split completion waiters and ops waiters when no runtime loop is present:
  queued input waiters terminate immediately, while active `wait_all` remains
  live until its operation settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was not trying to “fix” the asymmetry in the observer;
  this slice records current truth rather than target policy

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this materially improves the lifecycle map, but it still does not freeze the
  target retire policy for cutover

Next likely step:

- continue selecting lifecycle seams where two different carriers expose
  meaningfully different current-owner behavior

## Slice 82 - MobMachine surfaces tracked loop presence

Goal:

- take one small step from tracked run truth toward the eventual loop-machine
  boundary using a durable structural fact rather than another scalar setting

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `loop_count` to `TrackedRunStoreSnapshot`
  - populated it from durable `MobRun.loops.len()`
- `meerkat-mob/src/runtime/tests.rs`
  - added `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the new test builds an active root-loop flow and proves that:
    - the tracked run becomes visible through the joined `MobMachine` snapshot
    - `loop_count > 0` once loop state is materialized
    - the tracked run is still non-terminal while that loop structure is
      visible

Why this slice matters:

- it is the loop counterpart to the earlier `frame_count` slice
- it gives the joined `MobMachine` view its first durable loop-structure fact
  without overclaiming full loop semantics

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `git diff --check`

Result:

- `MobMachine` now exposes one durable loop-structure fact through the joined
  tracked-run view, with live proof that an active root-loop flow actually
  carries it

Backtracks encountered:

- no code-level backtrack in the final slice
- the key choice was to prefer loop structure over another config-shaped field

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- keep preferring small structural frame/loop facts when they can be observed
  cleanly from current durable state

## Slice 79 - MeerkatMachine preserves active wait_all after stop

Goal:

- pin down current stop semantics for ops-owned `wait_all` without smoothing
  them into the completion-waiter behavior

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor`
  - the new test proves that:
    - `stop_runtime_executor()` moves the runtime control phase to `Stopped`
    - the authority-owned `wait_request_id` survives stop
    - the shell-owned pending wait carrier survives stop
    - both sides still agree on the same `WaitRequestId`
    - the active `wait_all` future still resolves normally after the tracked
      operation completes

Why this slice matters:

- it sharpens a real lifecycle asymmetry in current code:
  - stop clears completion waiters immediately
  - stop does not currently clear the ops lifecycle `wait_all` carrier
- that is exactly the sort of current-semantics fact we need frozen before
  cutover and TLA+ work

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that current stop semantics
  preserve active `wait_all` state until the tracked operation itself settles,
  even though completion waiters terminate immediately

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was not inventing a new stop policy just because the
  completion-waiter path looks tidier

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this improves the lifecycle map, but it does not yet imply the target stop
  policy is settled

Next likely step:

- continue selecting lifecycle seams where the producer and consumer sides of a
  carrier can both be stated from real current-owner behavior

## Slice 80 - MobMachine surfaces tracked root-frame presence

Goal:

- take one small step from flow-run scalar truth toward frame-owned structure
  without pretending `MobMachine` already owns full frame semantics

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `frame_count` to `TrackedRunStoreSnapshot`
  - populated it from durable `MobRun.frames.len()`
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
  - the new test builds a root-frame flow, captures the joined `MobMachine`
    snapshot while the run is active, and proves:
    - the tracked run becomes visible through the joined machine view
    - `frame_count > 0` once the root frame is materialized
    - the tracked run is still non-terminal while that frame state is visible

Why this slice matters:

- it is the first small bridge from the current tracked run aggregate toward
  the eventual frame-machine boundary
- unlike another stored scalar, `frame_count` gives us a structural foothold in
  the current durable run shape

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `git diff --check`

Result:

- `MobMachine` now exposes one durable frame-structure fact through the joined
  tracked-run view, with live proof that a root-frame flow actually carries it

Backtracks encountered:

- no code-level backtrack in the final slice
- the key choice was to prefer a small structural frame fact over another
  config-shaped scalar

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- keep preferring structural tracked-run or frame-owned facts when they can be
  observed cleanly from current durable state

## Slice 75 - MeerkatMachine destroys completion waiters while abandoning queued work

Goal:

- close the missing destroy-side observational gap for completion waiters
- promote only the piece of destroy semantics the current control plane already
  owns explicitly

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy`
  - the new test proves that:
    - a queued completion waiter is visible before destroy
    - `destroy()` transitions runtime control to `Destroyed`
    - completion waiters are cleared immediately after destroy
    - the waiter resolves as `RuntimeTerminated("runtime destroyed")`
    - destroy abandons the queued input (`inputs_abandoned == 1`)
- `meerkat/src/meerkat_machine.rs`
  - added `DestroyedRuntimeStillHasCompletionWaiters`
  - the validator now rejects destroyed runtime snapshots that still carry
    completion waiters
  - added
    `validate_meerkat_machine_snapshot_reports_destroyed_completion_waiters`

Why this slice matters:

- it complements the earlier destroy-side `wait_all` observation with the
  opposite carrier: input completion waiters terminate immediately, while
  ops-owned `wait_all` currently survives until the tracked operation settles
- it strengthens the Meerkat lifecycle map using current owner truth rather
  than target-state preference

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_destroyed_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed destroy semantics for both waiter
  carriers:
  - completion waiters terminate immediately on destroy
  - active `wait_all` currently survives destroy until the tracked op settles

Backtracks encountered:

- the first red run was useful: I initially asserted
  `DestroyReport.inputs_abandoned == 0`
- the real current behavior is `inputs_abandoned == 1` for the queued input,
  which matches driver semantics; the waiter-clearing part of the proof still
  held

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens lifecycle truth, but it still does not mean destroy policy
  is frozen for the eventual cutover

Next likely step:

- keep selecting Meerkat lifecycle seams where the concrete owner behavior is
  stable enough to distinguish “current truth” from “future desired policy”

## Slice 76 - MobMachine surfaces persisted max_step_retries in tracked runs

Goal:

- extend the tracked-run snapshot with one more durable kernel field without
  inventing a new cleanup-sensitive invariant

What landed:

- `meerkat-mob/src/run.rs`
  - added typed reader `MobRun::max_step_retries()`
  - extended the kernel reader unit test to prove the field parses correctly
- `meerkat-mob/src/mob_machine.rs`
  - added `max_step_retries` to `TrackedRunStoreSnapshot`
  - populated it from the persisted `MobRun`
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_retry_limit_shape`
  - the new live proof shows that a healthy retry-limited run preserves:
    - `max_step_retries == 2`
    - zeroed failure counters
    - at least one materialized step status
    - step-status keys bounded by `ordered_steps`

Why this slice matters:

- it moves `MobMachine` a bit further from pure status projection toward a
  fuller durable flow-kernel map
- it stays honest about the current proof surface by projecting a persisted
  limit rather than forcing retry/cleanup timing into the validator

Verification:

- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_retry_limit_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now carries the persisted retry budget through the joined
  tracked-run snapshot, and the live codebase confirms it survives the current
  runtime path

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was methodological: do not add a retry-behavior invariant
  yet, only the durable retry-budget fact

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- keep preferring durable tracked-run facts or live durability proofs over
  retry/cleanup semantics that still depend on timing

## Slice 77 - MeerkatMachine stop clears completion waiters immediately

Goal:

- extend the lifecycle map for completion waiters from destroy into the
  stop-runtime-executor path
- keep the slice grounded in the current direct owner path, not in hypothetical
  future stop semantics

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor`
  - the new test proves that:
    - a queued completion waiter is visible before stop
    - `stop_runtime_executor()` transitions runtime control to `Stopped`
    - completion waiters are cleared immediately after stop
    - the waiter resolves as `RuntimeTerminated("runtime stopped")`
- `meerkat/src/meerkat_machine.rs`
  - added `StoppedRuntimeStillHasCompletionWaiters`
  - the validator now rejects stopped runtime snapshots that still carry
    completion waiters
  - added
    `validate_meerkat_machine_snapshot_reports_stopped_completion_waiters`

Why this slice matters:

- it makes the current completion-waiter lifecycle map more explicit:
  - stop clears completion waiters immediately
  - destroy clears completion waiters immediately
  - recover/recycle preserve live completion waiters
  - reset terminates completion waiters
- that gives us another owner-backed lifecycle cut we can eventually compare
  against the target Meerkat policy during cutover

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_stopped_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed stop-side completion-waiter semantics
  in addition to the existing destroy/recover/recycle/reset map

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to stay on completion waiters only and not force a
  stronger claim yet about the ops-owned `wait_all` carrier under stop

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this sharpens current lifecycle truth, but it is still not a write-authority
  cutover signal

Next likely step:

- keep selecting Meerkat lifecycle seams where current owner behavior is
  explicit enough to separate “observed current semantics” from “desired final
  semantics”

## Slice 78 - MobMachine surfaces persisted escalation_threshold in tracked runs

Goal:

- extend the tracked-run map with one more durable supervisor-related kernel
  fact without promoting new escalation behavior invariants too early

What landed:

- `meerkat-mob/src/run.rs`
  - added typed reader `MobRun::escalation_threshold()`
  - extended the kernel reader unit test to prove the field parses correctly
- `meerkat-mob/src/mob_machine.rs`
  - added `escalation_threshold` to `TrackedRunStoreSnapshot`
  - populated it from the persisted `MobRun`
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_supervisor_threshold_shape`
  - the new live proof shows that a healthy supervisor-threshold run preserves:
    - `escalation_threshold == 3`
    - zeroed failure counters
    - at least one materialized step status
    - step-status keys bounded by `ordered_steps`

Why this slice matters:

- it keeps broadening the durable Mob tracked-run map using facts the current
  run aggregate actually stores
- it gives the future `MobMachine` a cleaner bridge from supervisor config into
  persisted kernel state without pretending the escalation behavior is already
  formalized

Verification:

- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_supervisor_threshold_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now carries persisted supervisor escalation threshold through the
  joined tracked-run snapshot, and the live code path confirms it survives the
  current runtime projection

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was methodological: add the durable threshold fact first, not a
  new claim about when or how escalation must happen

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- continue preferring durable tracked-run facts or live durability proofs over
  escalation/retry cleanup behavior that still depends on timing

## Slice 149 - MeerkatMachine proves plain recycle preserves steered input and wait_all

Goal:

- extend the plain registered steer lane from `recover()` into `recycle()`
- prove current owner truth for steered queued work without inventing a nicer
  synthetic ingress model

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_recycle_preserves_steered_input_and_wait_all`
  - the new test proves that plain `recycle()`:
    - keeps `inputs.queue` empty and preserves the steered input in
      `inputs.steer_queue`
    - preserves `wake_requested` and `process_requested`
    - preserves the input-owned completion waiter
    - preserves the ops-owned `wait_all` carrier and `WaitRequestId` agreement
    - allows `wait_all` to settle first while the steered input remains pending

Why this slice matters:

- it closes the remaining plain steer-lane gap between `recover()` and
  `recycle()`
- it gives `MeerkatMachine` one more owner-backed ingress/lifecycle seam that
  is genuinely distinct from ordinary queued prompt recovery

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_preserves_steered_input_and_wait_all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_steered_prompt_input`

Result:

- `MeerkatMachine` now has direct evidence that plain recycle preserves the
  steer lane and the wait-all carrier together, with the steered input still
  pending after `wait_all` settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was to keep this on the plain registered path rather than
  speculating about attached-loop steer replay before the owner path proves it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens the ingress/lifecycle map, but it is still not a write-side
  switch signal

Next likely step:

- keep selecting distinct owner-backed lifecycle seams where queued and steered
  inputs can diverge in current code

## Slice 150 - MobMachine adds a live one-to-one all-policy dispatch proof

Goal:

- extend the active dispatch-family durability map with a new combination only
  if the live tracked-run aggregate sustains the same durable single-step shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_one_to_one_all_dispatch_shape`
  - the new test proves that active `DispatchMode::OneToOne` plus
    `CollectionPolicy::All` runs preserve:
    - durable `flow_id` and `Running` status
    - schema `4` and non-terminal shape
    - zero frame/loop structure
    - the single-step kernel maps for `dispatch`
    - `RunCollectionPolicyKind::All` and zero quorum threshold
    - zeroed failure/retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it gives `MobMachine` another distinct active dispatch family without
  promoting a new semantic field
- it keeps the dispatch-family matrix converging at the same durable
  single-step tracked-run bar

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_one_to_one_all_dispatch_shape`

Result:

- `MobMachine` now has owner-backed evidence that one-to-one all-policy
  dispatch collapses onto the same durable single-step tracked-run surface as
  the other active dispatch families

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to avoid treating dispatch mode itself as a machine-owned
  field and instead prove only the durable tracked-run shape

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-authority switch signal

Next likely step:

- continue adding dispatch-family or lifecycle coverage only when the live
  owner path sustains it at the same durable bar

## Slice 151 - MeerkatMachine backtracks plain retire steer teardown and records the real split

Goal:

- extend the plain registered steer lane into `retire()`
- observe current owner truth for the split between input-owned steered
  completion waiters and ops-owned `wait_all`

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_retire_clears_steered_waiter_but_leaves_steer_queue_visible`
  - the new test proves that plain `retire()`:
    - starts from a pending steered input in `inputs.steer_queue`
    - terminates the input-owned steered completion waiter immediately
    - still leaves the steered input visible in `inputs.steer_queue`
    - preserves the ops-owned `wait_all` carrier and `WaitRequestId` agreement
      until the background operation actually settles

Why this slice matters:

- it exposes a real seam mismatch between steered completion-waiter teardown and
  steer-queue visibility on the plain retire path
- it strengthens the lifecycle map with a distinct owner-backed retire case
  rather than another ordinary queued-input proof

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_clears_steered_waiter_but_leaves_steer_queue_visible`

Result:

- `MeerkatMachine` now has direct evidence that plain retire clears the
  steered completion waiter but does not currently clear the steer-queue
  projection at the same time

Backtracks encountered:

- yes: the first hypothesis was wrong
- the live runtime rejected the nicer story that plain retire clears steered
  queued work immediately, so the test was rewritten to capture the real owner
  truth instead of preserving the hypothesis

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a convergence slice, not a write-side switch signal

Next likely step:

- keep selecting distinct steer-lane lifecycle seams only where current owner
  code exposes stable truth directly

## Slice 152 - MobMachine adds a live fan-in quorum dispatch proof

Goal:

- extend the active dispatch-family durability map with a quorum variant only
  if the live tracked-run aggregate sustains the same durable single-step shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_fan_in_quorum_dispatch_shape`
  - the new test proves that active `DispatchMode::FanIn` plus
    `CollectionPolicy::Quorum { n: 2 }` runs preserve:
    - durable `flow_id` and `Running` status
    - schema `4` and non-terminal shape
    - zero frame/loop structure
    - the single-step kernel maps for `dispatch`
    - `RunCollectionPolicyKind::Quorum` and threshold `2`
    - zeroed failure/retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it adds the first dispatch-family proof that carries a non-zero quorum
  threshold through the joined `MobMachine` snapshot
- it keeps expanding the dispatch-family matrix without promoting a new
  machine-owned field

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_fan_in_quorum_dispatch_shape`

Result:

- `MobMachine` now has owner-backed evidence that the fan-in quorum dispatch
  family collapses onto the same durable single-step tracked-run surface as
  the other active dispatch families

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to stop at durable tracked-run shape and avoid asserting
  dispatch-mode-specific runtime behavior beyond what the aggregate preserves

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-authority switch signal

Next likely step:

- continue extending either steer-lane lifecycle coverage or dispatch-family
  coverage only where the live owner path sustains the same durable bar

## Slice 153 - MeerkatMachine proves plain destroy clears the steer lane but preserves wait_all

Goal:

- extend the plain registered steer lane into `destroy()`
- verify whether destroy follows the plain retire backtrack or behaves like the
  generic queue-teardown rules already visible in current owner code

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_destroy_clears_steered_waiter_and_queue_but_preserves_wait_all`
  - the new test proves that plain `destroy()`:
    - starts from a pending steered input in `inputs.steer_queue`
    - clears the steered completion waiter immediately
    - clears the steer queue immediately
    - preserves the ops-owned `wait_all` carrier and `WaitRequestId` agreement
      until the background operation settles

Why this slice matters:

- it distinguishes the plain destroy steer path from the plain retire steer
  backtrack
- it strengthens the lifecycle map with a second direct owner-backed steer-lane
  teardown case

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_clears_steered_waiter_and_queue_but_preserves_wait_all`

Result:

- `MeerkatMachine` now has direct evidence that plain destroy clears the steer
  lane immediately while still preserving the separate ops wait carrier

Backtracks encountered:

- no code-level backtrack in the final slice
- the key value of this slice is precisely that it did *not* match the plain
  retire steer behavior

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-side switch signal

Next likely step:

- keep probing only distinct steer-lane lifecycle seams where current owner
  code can answer a real semantic question

## Slice 154 - MobMachine adds a live fan-out quorum dispatch proof

Goal:

- extend the active dispatch-family durability map with the fan-out quorum
  variant, but only if the live tracked-run aggregate sustains the same durable
  single-step shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_fan_out_quorum_dispatch_shape`
  - the new test proves that active `DispatchMode::FanOut` plus
    `CollectionPolicy::Quorum { n: 2 }` runs preserve:
    - durable `flow_id` and `Running` status
    - schema `4` and non-terminal shape
    - zero frame/loop structure
    - the single-step kernel maps for `dispatch`
    - `RunCollectionPolicyKind::Quorum` and threshold `2`
    - zeroed failure/retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it complements the fan-in quorum proof and strengthens the dispatch-family
  matrix without inventing a new machine-owned field
- it shows the durable tracked-run surface survives a second quorum dispatch
  mode

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_fan_out_quorum_dispatch_shape`

Result:

- `MobMachine` now has owner-backed evidence that the fan-out quorum dispatch
  family collapses onto the same durable single-step tracked-run surface as
  the other active dispatch families

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to stay on the durable aggregate and not elevate
  dispatch-mode-specific runtime mechanics into new machine fields

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-authority switch signal

Next likely step:

- continue extending steer-lane lifecycle coverage or dispatch-family coverage
  only where the live owner path sustains the same durable bar

## Slice 155 - MeerkatMachine proves attached destroy clears the steer lane but preserves wait_all

Goal:

- extend the steer-lane destroy observation from the plain registered path into
  the attached-loop path
- verify whether a live executor changes the owner truth or whether destroy
  still bypasses queued steered work completely

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_destroy_with_runtime_loop_clears_steered_waiter_and_queue_but_preserves_wait_all`
  - the new test proves that attached `destroy()`:
    - starts from a pending steered input in `inputs.steer_queue`
    - clears the steered completion waiter immediately
    - clears the steer queue immediately
    - preserves the ops-owned `wait_all` carrier and `WaitRequestId`
      agreement until the background operation settles
    - still bypasses both `apply()` and `control()` on the executor seam

Why this slice matters:

- it confirms the plain destroy steer behavior survives even when an attached
  executor exists
- it narrows the real distinction to `retire()` vs `destroy()`, not
  plain-vs-attached destroy behavior

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_destroy_with_runtime_loop_clears_steered_waiter_and_queue_but_preserves_wait_all`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached destroy clears
  the steer lane immediately while preserving the separate ops wait carrier

Backtracks encountered:

- no code-level backtrack in the final slice
- the main question was whether the attached executor would alter destroy
  semantics, and current owner truth says it does not

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-side switch signal

Next likely step:

- keep probing steer-lane lifecycle seams only where the current owner path can
  answer a real semantic question we have not already covered

## Slice 156 - MobMachine adds a live one-to-one quorum dispatch proof

Goal:

- extend the active dispatch-family durability map with the final obvious
  quorum variant, but only if the live tracked-run aggregate sustains the same
  durable single-step shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_one_to_one_quorum_dispatch_shape`
  - the new test proves that active `DispatchMode::OneToOne` plus
    `CollectionPolicy::Quorum { n: 2 }` runs preserve:
    - durable `flow_id` and `Running` status
    - schema `4` and non-terminal shape
    - zero frame/loop structure
    - the single-step kernel maps for `dispatch`
    - `RunCollectionPolicyKind::Quorum` and threshold `2`
    - zeroed failure/retry/escalation defaults
    - bounded materialized step-status identity

Why this slice matters:

- it closes the obvious dispatch-family quorum matrix on the observed
  tracked-run surface
- it stays disciplined by proving only the durable aggregate shape, not a new
  dispatch-mode semantic field

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_one_to_one_quorum_dispatch_shape`

Result:

- `MobMachine` now has owner-backed evidence that the one-to-one quorum
  dispatch family also collapses onto the same durable single-step tracked-run
  surface as the rest of the active dispatch matrix

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to treat this as an aggregate durability proof rather
  than concluding anything yet about the higher-level semantic desirability of
  one-to-one quorum

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-authority switch signal

Next likely step:

- continue only with distinct lifecycle or tracked-run seams that still answer
  genuinely new owner-boundary questions

## Slice 73 - MeerkatMachine preserves active wait_all after destroy

Goal:

- observe what happens to an active `wait_all` carrier across the current
  `destroy()` path without inventing target-state semantics

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives destroy
    - the shell-owned pending wait carrier survives destroy
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target
      operation completes
    - runtime control phase is `Destroyed` while that ops-owned wait state is
      still visible

Why this slice matters:

- it extends the Meerkat lifecycle map from recover/recycle/reset into destroy
- it gives us an explicit observational answer to a non-obvious question in
  current code: destroy terminates input waiters, but it does not currently
  replace or cancel the ops lifecycle registry

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that current destroy semantics
  preserve active `wait_all` state until the tracked operation itself settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was treating this as an observation of current semantics,
  not as proof that destroy should keep this behavior after cutover

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this improves our picture of current lifecycle semantics, but it does not yet
  imply the target destroy policy is decided

Next likely step:

- keep selecting Meerkat lifecycle seams where the current owner behavior can
  be stated precisely, even when that behavior may later be judged undesirable

## Slice 74 - MobMachine live proof of healthy all-policy run durability

Goal:

- strengthen the live tracked-run proof for `CollectionPolicy::All` without
  assuming more status coverage than the current projection materializes

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
    to prove that the active tracked all-policy run:
    - keeps `failure_count == 0`
    - keeps `consecutive_failure_count == 0`
    - surfaces at least one materialized step status
    - only exposes step-status entries whose keys stay inside the ordered-step
      universe

Why this slice matters:

- it completes the healthy tracked-run durability trio across quorum, any, and
  all collection-policy variants
- it stays on the same smaller active-run status identity fact established by
  the earlier backtracks

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy all-policy tracked runs
  preserve zeroed failure counters and kernel-bounded materialized step-status
  identity

Backtracks encountered:

- no new code-level backtrack in the final slice
- the restraint was again to avoid terminal or full-coverage claims and stay on
  the durable active-run surface

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run durability without moving write
  authority

Next likely step:

- continue extending live durability proofs only where the joined snapshot has
  already demonstrated stable owner-backed truth

## Slice 163 - MeerkatMachine proves attached steered completion can clear before wait_all

Goal:

- strengthen the attached steer lane without guessing about interrupt semantics
- verify whether an attached steered prompt can complete normally while an
  independent ops-owned `wait_all` carrier remains live

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_attached_steered_prompt_splits_completion_and_wait_all_lifetimes`
  - the new test proves that on an attached runtime:
    - a steered prompt enters `Running` immediately and leaves both `queue` and
      `steer_queue`
    - the prompt's completion waiter and an independent `wait_all` carrier can
      coexist while the executor is blocked in `apply()`
    - once `apply()` is released, the steered prompt completes and the runtime
      returns to `Attached`
    - the input-owned completion waiter clears at that point
    - the ops-owned `wait_all` carrier and `WaitRequestId` agreement remain
      live until the background operation itself settles

Why this slice matters:

- it extends the attached steer lane past admission and into the first real
  split-lifetime proof
- it stays honest by observing the current normal completion path instead of
  assuming how attached steer should behave under reset/stop/destroy interrupts

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_attached_steered_prompt_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached steered work can
  clear the input-owned completion carrier while preserving a separate
  authority-owned ops wait carrier

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was not jumping straight to attached steer interrupt
  semantics before we had a stable completion-vs-wait split on the current
  owner path

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another convergence slice, not a write-side switch signal

Next likely step:

- keep attached steer work on owner-backed seams only, especially where a live
  executor is already proving distinct completion, queue, or control behavior

## Slice 164 - MobMachine rejects tracked runs without ordered steps

Goal:

- encode one generic tracked-run integrity rule that already falls out of the
  durable surface across all live families we have validated
- improve `MobMachine` without forcing another family-specific active-status
  claim

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `MobMachineInvariantViolation::TrackedRunMissingOrderedSteps`
  - taught `validate_mob_machine_snapshot(...)` to reject present tracked runs
    whose durable `ordered_steps` projection is empty
  - added
    `validate_mob_machine_snapshot_reports_tracked_run_without_ordered_steps`
    as the focused negative proof

Why this slice matters:

- every active family we currently trust already proves a non-empty ordered step
  universe, so this promotes a real generic truth rather than another
  family-specific convenience assertion
- it improves the executable model at the validator layer instead of leaning on
  timing-sensitive `step_status` materialization

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_without_ordered_steps`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now rejects an obviously malformed durable tracked-run shape
  before any family-specific map or status reasoning starts

Backtracks encountered:

- no code-level backtrack in the final slice
- the key decision was to prefer a generic durable-surface invariant over
  another speculative active-family status promotion

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is useful convergence work, but still not a write-side switch signal

Next likely step:

- continue raising generic durable-surface invariants where they are already
  supported across the live tracked-run families, and avoid status-materialization
  claims unless the runtime sustains them directly

## Slice 165 - MeerkatMachine isolates attached steered completion from detached-wake reentry

Goal:

- verify whether attached steered completion can still clear after `wait_all`
  settles first when detached-wake continuation reentry is not in scope
- correct the observational model if the earlier background-op proof was mixing
  two different seams together

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - updated
    `meerkat_machine_spine_snapshot_attached_steered_prompt_preserves_completion_after_wait_all_settles`
  - switched the waited operation from `BackgroundToolOp` to `MobMemberChild`
    so the test no longer arms detached-wake continuation injection
  - removed the temporary debug panic used to inspect the false failing shape
  - the corrected proof now shows that when detached-wake reentry is isolated
    out of the scenario:
    - attached steered work remains `Running` while `apply()` is blocked
    - `wait_all` can settle first and clear independently
    - the input-owned completion waiter still survives until `apply()` finishes
    - once `apply()` completes, the runtime returns to `Attached`
    - no follow-on continuation run starts on this path

Why this slice matters:

- it confirms the earlier red probe was a mixed-seam problem, not a broken
  attached steered completion path
- it keeps the Meerkat model honest by separating:
  - the completion-vs-`wait_all` split
  - detached-wake continuation reentry for background ops

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_attached_steered_prompt_preserves_completion_after_wait_all_settles`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has a clean attached steered proof for
  completion-after-`wait_all` ordering without detached-wake interference

Backtracks encountered:

- yes: the first version used `BackgroundToolOp`, which let detached wake inject
  a continuation immediately after the steered completion cleared
- the correct response was to isolate the seam instead of baking detached-wake
  reentry into the attached steered completion proof

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is useful convergence because it removes a mixed-seam false failure
  before switch work

Next likely step:

- keep the steer lane and detached-wake continuation work separate unless the
  explicit goal is to prove their interaction on the live path

## Slice 166 - MobMachine rejects tracked runs with self-dependencies

Goal:

- add another generic durable-surface integrity rule that holds across tracked
  runs without depending on any one active family
- strengthen `MobMachine` with structure the durable aggregate already owns

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `MobMachineInvariantViolation::TrackedRunSelfDependency`
  - taught `validate_mob_machine_snapshot(...)` to reject tracked runs whose
    dependency map points a step at itself
  - added the focused negative proof
    `validate_mob_machine_snapshot_reports_tracked_run_self_dependency`

Why this slice matters:

- self-dependency is invalid durable flow-run truth regardless of which live
- family produced the tracked run
- it strengthens the executable validator without leaning on timing-sensitive
  status materialization or active-family quirks

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_self_dependency`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now rejects another malformed tracked-run shape before higher
  level branch, collection, or status reasoning starts

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was choosing a generic durable rule instead of another
  family-specific active-status promotion

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is clean convergence work, but still not a write-side switch signal

Next likely step:

- keep promoting generic tracked-run integrity rules that the durable surface
  already supports across families

## Slice 167 - MeerkatMachine splits attached steered destroy between completion and wait_all

Goal:

- extend the attached steer lane with a real destroy seam
- verify whether plain `destroy()` can terminate an in-flight attached steered
  completion waiter while leaving an independent ops-owned `wait_all` carrier
  alive
- keep detached wake out of scope so the proof only exercises steer completion
  vs destroy vs `wait_all`

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_attached_steered_prompt_destroy_splits_completion_and_wait_all_lifetimes`
  - the test uses an attached blocking executor plus `OperationKind::MobMemberChild`
    so the runtime stays on the live attached steer path without arming detached
    wake continuation injection
  - the new proof shows that:
    - the attached steered prompt enters `Running` and owns a completion waiter
    - `destroy()` clears that input-owned completion waiter immediately even
      while `apply()` is still blocked
    - `destroy()` preserves the authority-owned `wait_all` carrier and request-id
      agreement until the waited operation itself settles
    - once `apply()` is released and the waited operation completes, the settled
      snapshot lands in `Destroyed` with no current-run binding and no remaining
      wait carrier

Why this slice matters:

- it gives the steer lane a direct attached destroy split proof instead of only
  plain registered destroy behavior
- it confirms the current owner boundary:
  - input-owned completion waiters terminate at destroy
  - ops-owned `wait_all` survives until the waited operation settles
- it does so without re-mixing detached-wake continuation behavior into the
  steer model

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_attached_steered_prompt_destroy_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has an attached steer destroy split-lifetime proof
  alongside the existing attached steer completion ordering proofs

Backtracks encountered:

- no owner-boundary backtrack in the final slice
- the main restraint was keeping the waited operation on `MobMemberChild`
  specifically to avoid detached-wake continuation reentry

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is clean convergence work, but still not a write-side switch signal

Next likely step:

- continue filling attached steer lifecycle seams only where the live runtime
  owns them directly, and keep detached wake isolated unless the explicit goal
  is to model that interaction

## Slice 168 - MobMachine rejects duplicate tracked-run dependencies

Goal:

- strengthen the generic durable tracked-run validator with another integrity
  rule that does not depend on any one active family
- reject a malformed dependency shape before branch, collection, or step-status
  reasoning starts

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `MobMachineInvariantViolation::TrackedRunDuplicateDependency`
  - taught `validate_mob_machine_snapshot(...)` to reject dependency vectors that
    repeat the same dependency step for a given owner step
  - added the focused negative proof
    `validate_mob_machine_snapshot_reports_tracked_run_duplicate_dependency`

Why this slice matters:

- duplicate dependencies are invalid durable flow-run truth regardless of which
  live family produced the tracked run
- the rule is purely structural and generic, so it strengthens `MobMachine`
  without leaning on timing-shaped active-status behavior

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_duplicate_dependency`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now rejects another malformed tracked-run shape at the durable
  surface boundary

Backtracks encountered:

- no code-level backtrack in the final slice
- the main discipline was choosing another generic durable rule instead of
  another active-family promotion

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is clean convergence work, but still not a write-side switch signal

Next likely step:

- keep preferring generic tracked-run integrity rules when they are clearly
  supported across families, and only promote new live family facts when the
  durable aggregate sustains them directly

## Slice 169 - MeerkatMachine proves attached steer stop is deferred until apply finishes

Goal:

- probe the first attached steer seam that genuinely crosses the live control
  plane instead of a direct driver mutation
- verify whether `stop_runtime_executor()` interrupts an in-flight attached
  steered `apply()` or waits behind it
- keep detached wake out of scope so the proof only covers attached steer
  completion vs queued stop vs independent `wait_all`

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_attached_steered_prompt_defers_stop_until_apply_finishes`
  - the test uses an attached blocking executor plus `OperationKind::MobMemberChild`
    so the runtime stays on the live attached steer path without arming
    detached-wake continuation injection
  - the new proof shows that:
    - the attached steered prompt enters `Running` and owns a completion waiter
    - `stop_runtime_executor()` stays queued while `apply()` is still blocked
      and does not immediately reach the executor control seam
    - `wait_all` can settle first while the steered completion waiter and
      current-run binding both stay live
    - once `apply()` finishes, the steered completion resolves normally,
      then the queued stop drains and moves the runtime to `Stopped`

Why this slice matters:

- it keeps the attached steer model honest about the real control-plane shape:
  stop is deferred, not an in-flight interrupt
- it sharpens the ownership split:
  - input-owned steered completion still belongs to the active run until
    `apply()` returns
  - ops-owned `wait_all` can settle independently before the queued stop drains
- it does so without re-mixing detached-wake behavior into the steer lane

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_attached_steered_prompt_defers_stop_until_apply_finishes`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has an explicit attached steer proof for deferred stop
  ordering on the live loop path

Backtracks encountered:

- no owner-boundary backtrack in the final slice
- the main discipline was keeping the waited operation on `MobMemberChild`
  specifically so stop ordering was not obscured by detached-wake continuation

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is strong convergence work, but it is still not a write-side switch
  signal

Next likely step:

- keep extending attached steer lifecycle seams only where the live loop owns
  the behavior directly, and keep out-of-band continuation behavior isolated
  unless it becomes the explicit target

## Slice 170 - MobMachine proves branch fallback preserves durable failure_count

Goal:

- raise one active multi-step family to include a non-zero durable failure fact
  instead of only healthy zeroed counters
- verify that the branch-fallback family really sustains failed-branch history
  while the overall run is still `Running`

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_branch_fallback_shape`
  - the polling gate now waits for the joined tracked-run surface to show
    `failure_count > 0` while the run is still `Running`
  - the live proof now asserts that the active branch-fallback run preserves
    durable `failure_count == 1` alongside the existing ordered-step,
    dependency, branch, policy, and bounded step-status facts

Why this slice matters:

- it is the first active multi-step family proof that carries a positive
  persisted failure count through the joined `MobMachine` surface
- that gives us a stronger signal that failure history is durable machine truth
  here, not just a terminalization artifact

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_fallback_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now proves that a running fallback flow can preserve one durable
  failed branch while the successful fallback path is still live

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was only promoting `failure_count`, not
  `consecutive_failure_count`, because the latter is still more timing-shaped
  under mixed fail-then-success execution

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is clean convergence work, but still not a write-side switch signal

Next likely step:

- continue preferring live multi-step family proofs when they can promote a
  clearly durable fact without leaning on cleanup timing or transient status

## Slice 171 - MeerkatMachine starts cutover checklist with interrupt_current_run semantics

Goal:

- turn the Meerkat cutover gate into a concrete execution checklist
- start burning down the first explicit freeze blocker instead of only
  documenting it
- freeze the current adapter semantics for `interrupt_current_run`

What landed:

- `docs/architecture/two-kernel-research/meerkat-cutover-checklist.md`
  - added a Meerkat-only cutover checklist derived from the gate
  - marked `M1` (`interrupt` / cancel semantics) as the first in-progress
    freeze blocker
- `meerkat-runtime/src/session_adapter.rs`
  - added `interrupt_current_run_returns_not_ready_without_attached_loop`
  - added
    `interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
  - the attached proof uses a blocking executor on an attached steered prompt
    path, which reaches the live `apply()` seam without mixing in
    detached-wake behavior, so the result still isolates the actual
    adapter/control-loop semantics:
    - `interrupt_current_run()` succeeds only when a live control channel exists
    - without an attached loop it returns `NotReady { state: Idle }`
    - with an attached loop it does not interrupt in-flight `apply()`
    - the run, current-run binding, and completion waiter all stay live until
      `apply()` returns
    - once `apply()` returns, the queued cancel drains through the executor
      control seam and the runtime returns to `Attached`

Why this slice matters:

- it converts the cutover conversation into an executable Meerkat freeze
  program instead of a general research direction
- it freezes the first named shell action in the Meerkat kernel verbs list
  through the real adapter path
- it removes a real ambiguity from the cutover gate: `interrupt_current_run`
  is currently a deferred executor-side control request, not a runtime-side
  state transition and not an in-flight interrupt

Verification:

- `cargo test -p meerkat-runtime --lib interrupt_current_run_returns_not_ready_without_attached_loop`
- `cargo test -p meerkat-runtime --lib interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has the first direct proofs for the current
  `interrupt_current_run` boundary, and the cutover work has a concrete
  checklist to execute

Backtracks encountered:

- no owner-boundary backtrack in the final slice
- the main discipline was keeping the proof off the detached-wake path, even
  though attached steer metadata was required to hit the real live `apply()`
  seam

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is the start of the accelerated freeze push, but `M1` is still only
  partially complete because cooperative-yield interrupt behavior and
  cancel-after-boundary are still uncovered

Next likely step:

- continue `M1` by freezing the cooperative-yield / `InterruptYielding` path,
  then tie that back to live turn cancellation posture instead of keeping it
  only at policy-table level

## Slice 172 - MeerkatMachine freezes adapter InterruptYielding semantics and validates session-layer cooperative interrupt

Goal:

- finish the non-speculative part of `M1` by distinguishing hard cancel from
  cooperative interrupt on the live Meerkat path
- freeze the runtime-adapter `InterruptYielding` seam instead of leaving it as
  lower-level driver folklore
- confirm the session-layer cooperative interrupt proof still holds on the
  current codebase

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - corrected
    `interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
    to use an attached steered prompt, which is the real live `apply()` seam
  - added
    `running_peer_message_interrupt_yielding_drains_before_next_apply`
  - the new proof freezes current runtime-adapter behavior for a running peer
    message:
    - the peer input is accepted while the first apply is still running
    - the peer input remains queued while the first apply is blocked
    - `InterruptYielding` does not hard-cancel the current run
    - the queued `InterruptYielding` control drains after the current apply
      returns and before the next queued input starts
- `meerkat-session/tests/ephemeral_contract.rs`
  - revalidated
    `test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`
    against the current codebase
- `docs/architecture/two-kernel-research/meerkat-cutover-checklist.md`
  - updated `M1` wording to reflect that adapter/runtime interrupt proofs and
    the session-layer cooperative interrupt proof now both exist

Why this slice matters:

- it freezes the exact distinction we need before cutover:
  `CancelCurrentRun` is a deferred hard-cancel request, while
  `InterruptYielding` is a deferred cooperative interrupt request that the
  runtime adapter delivers before the next queued input starts
- it also makes the ownership split explicit instead of smearing it:
  adapter/runtime owns delivery semantics, while the actual cooperative yield
  behavior still lives at the session task boundary today
- that narrows `M1` from “generic interrupt uncertainty” down to the remaining
  live `cancel_after_boundary` posture

Verification:

- `cargo test -p meerkat-runtime --lib interrupt_current_run_returns_not_ready_without_attached_loop`
- `cargo test -p meerkat-runtime --lib interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
- `cargo test -p meerkat-runtime --lib running_peer_message_interrupt_yielding_drains_before_next_apply`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-session --test ephemeral_contract test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`

Result:

- `MeerkatMachine` now has direct proofs for:
  - plain `interrupt_current_run` rejection without an attached loop
  - attached deferred hard-cancel semantics
  - adapter/runtime `InterruptYielding` delivery ordering
  - session-layer cooperative interrupt without hard cancel
- the remaining explicit `M1` blocker is the live `cancel_after_boundary`
  posture

Backtracks encountered:

- the first attached interrupt harness used a peer-progress helper that never
  entered the live `apply()` seam
- the first `InterruptYielding` harness mixed two different sources of the
  signal: the initial attached steered prompt and the running peer message
- correcting both sharpened the machine story instead of weakening it

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- but `M1` is now materially narrower: interrupt delivery is mostly frozen,
  while `cancel_after_boundary` remains the clear next blocker

Next likely step:

- continue `M1` by tying live Meerkat effects back to `cancel_after_boundary`,
  or document explicitly if that posture still lives one layer below the
  current adapter seam

## Slice 173 - MeerkatMachine narrows M1 to cancel_after_boundary lowering gap

Goal:

- decide whether `M1` still lacks interrupt proofs, or whether the remaining
  blocker is specifically `cancel_after_boundary`
- avoid spending more slices rediscovering already-frozen interrupt semantics

What landed:

- `docs/architecture/two-kernel-research/meerkat-cutover-checklist.md`
  - added an explicit `M1` read:
    - `interrupt_current_run` is frozen well enough at the adapter/runtime
      boundary for plain and attached runtimes
    - runtime-adapter `InterruptYielding` delivery is frozen as a queued
      control seam that drains before the next queued apply starts
    - session-layer cooperative interrupt is still anchored by a live
      `meerkat-session` proof and does not hard-cancel the turn
    - the remaining `M1` blocker is `cancel_after_boundary`
- repo trace:
  - searched for live `TurnExecutionInput::CancelAfterBoundary` callsites
  - current results show only authority-local uses in
    `meerkat-core/src/turn_execution_authority.rs`
  - no live Meerkat lowering path was found yet in the adapter, runtime loop,
    session layer, or runner code

Why this slice matters:

- it converts `M1` from a vague freeze item into a precise remaining problem
- the blocker is no longer “interrupt semantics are unclear”
- the blocker is “`cancel_after_boundary` does not yet have a discovered live
  Meerkat lowering outside the authority itself”
- that is a much more actionable cutover fact: the next move is either to
  surface the lowering or to classify the gap explicitly before cutover

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-session --test ephemeral_contract test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`
- `rg -n "TurnExecutionInput::CancelAfterBoundary|CancelAfterBoundary \\{" /Users/luka/.codex/worktrees/c5c6/meerkat`

Result:

- `M1` is now mostly frozen on the interrupt side
- the remaining blocker is the missing discovered live lowering for
  `cancel_after_boundary`

Backtracks encountered:

- no owner-boundary backtrack in the final read
- the useful negative result was the repo trace itself: no live lowering path
  was found, so this cannot honestly be marked frozen yet

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- but the gate is now failing `M1` for one narrow reason instead of a broad
  interrupt/cancel cloud

Next likely step:

- either surface a concrete live `cancel_after_boundary` lowering path, or
  move to `M2` while carrying `cancel_after_boundary` as an explicit Meerkat
  freeze blocker that still needs a write-side landing

## Slice 174 - MeerkatMachine freezes M1 as an exact-current-state asset

Goal:

- close `M1` honestly enough to stop rediscovering interrupt semantics
- freeze the exact current Meerkat boundary without inventing a live
  `cancel_after_boundary` lowering

What landed:

- `docs/architecture/two-kernel-research/meerkat-interrupt-freeze.md`
  - added a dedicated exact-current-state freeze note for Meerkat `M1`
  - freezes:
    - plain and attached `interrupt_current_run`
    - runtime-adapter `InterruptYielding`
    - session-layer cooperative interrupt
  - classifies `cancel_after_boundary` as authority-local or target-state
    until a real lowering exists
- `docs/architecture/two-kernel-research/meerkat-cutover-checklist.md`
  - flipped `M1` to `Done`
  - made the exact current freeze decision explicit
  - moved immediate execution order on to `M2`
- `docs/architecture/two-kernel-research/README.md`
  - added the Meerkat `M1` freeze note to the active artifact index

Why this slice matters:

- it turns `M1` from an open research item into a freezable cutover asset
- it also prevents a false freeze by separating:
  - live interrupt semantics that are now proven
  - `cancel_after_boundary`, which is still not a discovered live Meerkat
    lowering
- that is the exact kind of exact-vs-target-state classification the cutover
  gate requires before we move into the remaining Meerkat freeze blockers

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-session --test ephemeral_contract test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`
- `git diff --check`

Result:

- `M1` is resolved for the exact current Meerkat cutover boundary
- the next Meerkat freeze blocker is now `M2`: detached-wake and
  continuation interaction

Backtracks encountered:

- none in the final freeze landing itself
- the important backtrack was already incorporated into the freeze asset:
  `cancel_after_boundary` was removed from the frozen live boundary instead of
  being promoted without a discovered lowering

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate overall
- but `M1` no longer blocks semantic freeze

Next likely step:

- start `M2` by classifying exact current detached-wake and continuation
  behavior across legacy and feed-backed paths

## Slice 175 - MeerkatMachine fixes feed-backed detached-wake startup race

Goal:

- make the feed-backed detached-wake path honest enough to freeze
- verify that a background completion can wake a freshly registered idle
  runtime without relying on a manual prompt trigger

What landed:

- `meerkat-runtime/src/runtime_loop.rs`
  - changed feed-backed loop seeding so runtime-backed cursor state always wins
    over the current feed watermark, even when the cursor is all zeros
  - added direct runtime-loop proofs:
    - `maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent`
    - `maybe_inject_feed_wake_legacy_path_sets_signaled_without_inline_injection`
- `meerkat-runtime/tests/detached_wake_contract.rs`
  - added
    `choke_004_feed_backed_idle_runtime_injects_continuation_without_manual_trigger`
  - updated the stale detached-wake comments so they describe the current
    runtime-loop-owned model instead of a retired waker-task story

Why this slice matters:

- the first direct idle feed-backed proof started red and exposed a real race:
  a newly spawned runtime loop could seed from the current feed watermark and
  silently skip a background completion that landed before its first select
  iteration
- fixing that race turns detached wake into a freezable exact-current seam
  instead of a timing-sensitive maybe
- it also cleanly separates:
  - canonical feed-backed inline injection
  - legacy `DetachedWakeState` signaled-notify fallback

Verification:

- `cargo test -p meerkat-runtime --test detached_wake_contract choke_004_feed_backed_idle_runtime_injects_continuation_without_manual_trigger`
- `cargo test -p meerkat-runtime --lib maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent`
- `cargo test -p meerkat-runtime --lib maybe_inject_feed_wake_legacy_path_sets_signaled_without_inline_injection`
- `cargo test -p meerkat-runtime --test detached_wake_contract`

Result:

- fresh registered runtimes no longer skip early background completions on the
  feed-backed detached-wake path
- feed-backed and legacy detached-wake behavior now both have direct
  runtime-loop proofs

Backtracks encountered:

- yes: the first idle feed-backed proof failed
- that failure was a real startup race in the loop watermark seeding, not a
  bad test
- fixing the code was the honest move; freezing around the race would have
  been architecture debt

Cutover gate read:

- `MeerkatMachine` remains on the observability side overall
- but `M2` is now ready to freeze as an exact-current-state asset

Next likely step:

- land the `M2` freeze note and move the Meerkat checklist on to `M3`

## Slice 176 - MeerkatMachine freezes M2 as an exact-current-state asset

Goal:

- close `M2` honestly enough to stop rediscovering detached-wake semantics
- classify canonical feed-backed behavior separately from the legacy
  compatibility fallback

What landed:

- `docs/architecture/two-kernel-research/meerkat-detached-wake-freeze.md`
  - added a dedicated exact-current-state freeze note for detached wake and
    continuation interaction
  - freezes:
    - canonical feed-backed idle and post-drain continuation injection
    - deferred non-quiescent behavior
    - completion-kind filtering
    - continuation shape
    - legacy `DetachedWakeState` compatibility behavior
- `docs/architecture/two-kernel-research/meerkat-cutover-checklist.md`
  - flipped `M2` to `Done`
  - removed detached wake from the remaining Meerkat freeze blockers
  - moved immediate execution order on to `M3`
- `docs/architecture/two-kernel-research/README.md`
  - added the Meerkat `M2` freeze note to the active artifact index

Why this slice matters:

- it converts `M2` from a lingering historical seam into a frozen exact-current
  cutover asset
- it also makes the compatibility boundary explicit instead of letting the
  legacy latch path blur together with the registered-runtime feed path
- that is the exact sort of exact-vs-compatibility classification the cutover
  gate requires before we move into the remaining Meerkat blockers

Verification:

- `cargo test -p meerkat-runtime --test detached_wake_contract`
- `cargo test -p meerkat-runtime --lib maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent`
- `cargo test -p meerkat-runtime --lib maybe_inject_feed_wake_legacy_path_sets_signaled_without_inline_injection`

Result:

- `M2` is resolved for the exact current Meerkat cutover boundary
- the next Meerkat freeze blocker is now `M3`: turn / ops / barrier coupling

Backtracks encountered:

- none in the final freeze landing itself
- the key backtrack was already folded into the freeze asset: the startup race
  was fixed instead of being classified as an acceptable current behavior

Cutover gate read:

- `MeerkatMachine` remains on the observability side overall
- but `M2` no longer blocks semantic freeze

Next likely step:

- start `M3` by mapping exact current turn / ops / barrier coupling on the
  live runtime path

## Slice 177 - MeerkatMachine adversarial M1 re-audit confirms freeze asset

Goal:

- challenge `M1` after the later Meerkat convergence work instead of assuming
  the older freeze note is still accurate
- verify that no newly surfaced live code path quietly invalidates the frozen
  exact-current interrupt/cancel story

What landed:

- re-audited `docs/architecture/two-kernel-research/meerkat-interrupt-freeze.md`
  against the current codebase
- re-traced live interrupt and cancel-adjacent callsites across:
  - `meerkat-runtime`
  - `meerkat-session`
  - `meerkat-core`
  - `meerkat`
- confirmed that the current live boundary still matches the frozen split:
  - plain and attached `interrupt_current_run`
  - runtime-adapter `InterruptYielding`
  - session-layer cooperative `interrupt_yielding`
- confirmed again that `cancel_after_boundary` still has authority and
  snapshot presence, but no discovered live runtime/session lowering

Why this slice matters:

- later Meerkat work touched adjacent runtime behavior, so `M1` needed an
  explicit re-audit before we kept treating it as closed
- this slice converts “we froze it once” into “we rechecked it after more
  runtime work and it still holds”
- that is the right bar for keeping `M1` out of the remaining freeze blockers

Verification:

- `rg -n "CancelAfterBoundary|cancel_after_boundary|interrupt_current_run|InterruptYielding|interrupt_yielding" ...`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-session --test ephemeral_contract test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`

Result:

- `M1` remains frozen as an exact-current-state asset
- no new live lowering for `cancel_after_boundary` was discovered
- no post-freeze drift was found in the interrupt/cooperative-interrupt split

Backtracks encountered:

- none
- the useful result was negative: the audit did not uncover any hidden live
  path that would force `M1` back open

Cutover gate read:

- `MeerkatMachine` remains on the observability side overall
- `M1` remains done and does not block Meerkat freeze

Next likely step:

- continue on `M3` rather than reopening `M1`

## Slice 178 - MeerkatMachine hardens M1 into a review-ready freeze asset

Goal:

- make `M1` easier to review without reopening it for target-state design work
- turn the existing freeze note into a self-contained asset with an explicit
  verification lane and explicit reopen conditions

What landed:

- `docs/architecture/two-kernel-research/meerkat-interrupt-freeze.md`
  - added a dedicated reviewer verification lane
  - added explicit reopen conditions
  - added explicit non-reopen conditions so adjacent Meerkat work does not
    accidentally drag `M1` back open
  - marked the note as a review-ready exact-current-state asset
- `docs/architecture/two-kernel-research/meerkat-cutover-checklist.md`
  - updated the current `M1` read to reflect that the freeze note is now
    review-ready, not merely present

Why this slice matters:

- the blocker was no longer semantic uncertainty; it was review clarity
- this slice makes it obvious what evidence reviewers should rerun and what
  kinds of future changes really would require reopening `M1`
- that is the right final packaging step before we move full attention to the
  remaining Meerkat freeze blockers

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-session --test ephemeral_contract test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`
- `git diff --check`

Result:

- `M1` remains frozen as an exact-current-state asset
- the asset is now review-ready and self-contained
- no new live `cancel_after_boundary` lowering was discovered

Backtracks encountered:

- none
- this slice intentionally avoided inventing new semantics; it only hardened
  the freeze boundary and reviewer guidance

Cutover gate read:

- `MeerkatMachine` remains on the observability side overall
- `M1` is fully packaged and no longer needs more work before review

Next likely step:

- move to `M3` and keep `M1` closed unless one of the explicit reopen
  conditions is triggered

## Slice 179 - Post-rebase repair restores Meerkat freeze lanes and widened green surface

Goal:

- untangle the semantic fallout from rebasing `codex/machines-redux` onto
  `origin/main`
- restore the exact-current interrupt/cooperative-interrupt boundary instead of
  allowing `main`'s flatter comms/runtime surface to silently redefine it
- prove that the rebased branch is back to a reviewable green baseline before
  continuing Meerkat freeze work

What landed:

- restored the missing core wait-interrupt compatibility surface in
  `meerkat-core/src/wait_interrupt.rs`, plus the builder/agent wiring needed
  by the rebased branch
- restored the live `InterruptYielding` seam across:
  - `meerkat-core/src/lifecycle/run_control.rs`
  - `meerkat-runtime/src/policy.rs`
  - `meerkat-runtime/src/policy_table.rs`
  - `meerkat-runtime/src/runtime_ingress_authority.rs`
  - `meerkat-runtime/src/driver/ephemeral.rs`
- repaired the rebased comms command/envelope contract so peer
  `handling_mode` survives from parsed commands into internal `MessageKind`
  envelopes without regressing the restored `stream` contract
- updated downstream consumers (`meerkat-tools`, `meerkat-mob`,
  `meerkat-mob-mcp`, `meerkat`) to the rebased `DispatcherCapabilities`,
  `CommsCommand`, kickoff, and event surfaces
- corrected one stale mob persistence test so it asserts append-only event-log
  preservation instead of exact event-count equality in the presence of
  legitimate concurrent kickoff projections

Why this slice matters:

- the original narrow Meerkat failure after rebase was real: `main` had
  flattened away `InterruptYielding` in policy/ingress, which broke the frozen
  `M1` boundary
- the repair keeps the current-machine interpretation honest by restoring the
  live cooperative interrupt delivery path rather than silently redefining
  Meerkat semantics to fit the rebased compile surface
- the widened green lanes mean the rebased branch is no longer just "narrow
  runtime tests pass"; the downstream mob and top-level Meerkat consumers are
  back in agreement too

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --test detached_wake_contract`
- `cargo check -p meerkat-comms --lib`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- the rebased branch is back to a clean green baseline for the Meerkat freeze
  lanes plus the widened mob/top-level consumers
- `M1` and `M2` remain frozen exact-current assets after rebase
- the remaining Meerkat freeze blockers are still `M3+`, not rebase fallout

Backtracks encountered:

- a stale `DispatcherCapabilities` initializer in `meerkat-tools` that dropped
  new capability fields
- a stale top-level `service_factory` test constructor missing rebased fields
- a mob persistence test that assumed exact event-count stability instead of
  append-only event-log preservation
- a false attached-steer mixed-seam hypothesis from earlier work stayed fixed;
  no new boundary movement was introduced during rebase repair

Cutover gate read:

- `MeerkatMachine` remains on the observability side overall, but the rebase
  repair no longer blocks freeze work
- the branch is healthy enough to resume accelerated work on `M3` rather than
  spending more time on rebase untangling

## Slice 180 - Widened comms lane exposes one stale trust assertion, not new fallout

Goal:

- finish the post-rebase widening pass across directly touched crates
- determine whether `meerkat-comms --lib` still held hidden semantic fallout
  after the constructor/field repair work

What landed:

- patched the remaining rebased `handling_mode` constructors across:
  - `meerkat-comms/src/inbox.rs`
  - `meerkat-comms/src/agent/types.rs`
  - `meerkat-comms/src/classify.rs`
  - `meerkat-comms/src/runtime/comms_runtime.rs`
- reran the widened `meerkat-comms --lib` lane and reduced the remaining issue
  from compile fallout to one live test mismatch
- corrected `test_live_peer_authority_syncs_trust_receive_and_drain` to assert
  the current truth: once trust is explicitly registered before the second
  ingress, the queued peer entry records `trusted_snapshot = Some(true)`

Why this slice matters:

- it shows the remaining rebase damage was localized to stale constructor
  shapes and one stale test expectation, not to a deeper semantic break in the
  current comms authority model
- the final mismatch was healthy to fix because it aligned the test with the
  already-restored trust-sync behavior instead of reverting the runtime toward
  older semantics

Verification:

- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat-core --lib`
- `cargo test -p meerkat-tools --lib`
- `cargo test -p meerkat-session --test ephemeral_contract`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --test detached_wake_contract`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- the widened post-rebase verification surface is green again
- rebase fallout is no longer the thing blocking machine freeze work
- the next real Meerkat blocker remains `M3`, not merge damage

## Slice 181 - Restored wait-interrupt builder surface is removed, not preserved

Goal:

- resolve the last suspicious restored API surface before treating the rebased
  machine baseline as trustworthy for freeze work
- determine whether resurrected wait-interrupt builder/binder state was a
  missing live wire or dead historical surface

What landed:

- re-audited all live reads of the restored wait-interrupt builder/binder
  surface
- confirmed there is no remaining local wait-tool binding on the current live
  Meerkat path and no real consumer of the resurrected API
- removed the dead restored surface instead of preserving it:
  - removed the extra `AgentBuilder` wait-interrupt setters
  - removed the extra `AgentToolDispatcher` wait-interrupt / completion-feed
    binding hooks
  - removed the extracted `wait_interrupt.rs` module restoration
- updated:
  - `meerkat-interrupt-freeze.md`
  - `meerkat-cutover-checklist.md`
  so the freeze story now says this explicitly

Why this slice matters:

- it resolves the last ambiguous restored API without inventing a target-state
  consumer
- the rebased baseline is now clearer: the current product shape does not
  include a local wait-tool builder/binder seam
- this removes the last suspicious “API came back from history” story from the
  freeze baseline

Verification:

- `cargo check -p meerkat-core --lib`
- `cargo test -p meerkat-core --lib`
- `cargo test -p meerkat-session --test ephemeral_contract`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --test detached_wake_contract`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- the rebased branch no longer has a credible “maybe a deleted wait-interrupt
  API should really still exist” story
- the remaining blockers to Meerkat freeze are once again semantic (`M3+`),
  not rebase untangling

## Slice 182 - Workspace-wide rebase audit restores missing peer-ingress surface seam

Goal:

- widen the post-rebase verification surface from the curated machine lanes to
  workspace-level tests
- determine whether any stale caller drift remained hidden outside the
  Meerkat/Mob-focused lanes

What landed:

- widened to workspace test lanes, which exposed one real stale surface:
  `RuntimeSessionAdapter::update_peer_ingress_context(...)` had been dropped
  while the lower-level drain-lifecycle owner method
  `maybe_spawn_comms_drain(...)` remained alive
- restored `update_peer_ingress_context(...)` in
  `meerkat-runtime/src/session_adapter.rs` as the live surface-facing seam
  that delegates to the canonical drain-lifecycle owner method
- kept the state model unchanged: no old per-session ingress context storage
  was reintroduced; the restored surface just forwards to the current owner
  seam

Why this slice matters:

- it proves the wider workspace audit was worthwhile: the narrower freeze lanes
  were green, but one stale top-level surface/test seam still existed
- it corrects the ownership story: `update_peer_ingress_context(...)` is not
  just historical compatibility; it is still the live surface seam used across
  CLI/REST/RPC/MCP and tests, while `maybe_spawn_comms_drain(...)` is the
  lower-level owner method behind it

Verification:

- `cargo test --workspace --tests --quiet` exposed the missing method
- follow-up narrowed lanes were rerun after the wrapper restore

Result:

- the last hidden caller-drift pocket exposed by the workspace audit is now
  repaired
- rebase fallout remains a non-blocker; the remaining Meerkat freeze blockers
  are still semantic (`M3+`)

## Slice 183 - Workspace-wide lib and test lanes confirm rebase fallout is closed

Goal:

- stop treating rebase cleanup as active machine work unless a fresh wide audit
  shows otherwise
- confirm the rebased branch is trustworthy enough that the remaining blockers
  are semantic freeze blockers, not merge fallout

What landed:

- reran the widest practical verification lanes on the rebased tree:
  - `cargo test --workspace --tests --quiet`
  - `cargo test --workspace --lib --quiet`
- kept `git diff --check` green after the final compatibility and expectation
  fixes
- rechecked the current Meerkat freeze posture against the widened green lanes

Why this slice matters:

- it closes the rebase-repair chapter with evidence instead of intuition
- it means `M1` and `M2` are now standing on a rebased baseline that has been
  validated outside the curated machine lanes
- it lets the accelerated push move cleanly into `M3` without keeping one eye
  on merge fallout

Verification:

- `cargo test --workspace --tests --quiet`
- `cargo test --workspace --lib --quiet`
- `git diff --check`

Result:

- the rebased baseline is now wide-lane green
- rebase fallout is no longer an active Meerkat freeze blocker
- the next real Meerkat blocker is `M3`, not merge damage

## Slice 184 - All-targets audit confirms rebase fallout is fully flushed

Goal:

- make sure no last compile-only drift remains in bins/examples/test harnesses
- stop spending machine-freeze time on merge fallout once the widest practical
  audit surface is green

What landed:

- widened again to:
  - `cargo check --workspace --tests --quiet`
  - `cargo check --workspace --all-targets --quiet`
- at that point, the restored `wait_interrupt` module still compiled cleanly;
  Slice 185 later proved it should be removed, so this slice should be read as
  the all-target audit only, not as the final classification of that surface
- confirmed both lanes are green and only emit the expected observational
  `dead_code` warnings from the diagnostic machine scaffolding
- updated freeze docs so the branch state is explicit: rebase fallout is fully
  closed, and the remaining blockers are semantic (`M3+`)

Why this slice matters:

- it closes the last plausible “maybe there is still hidden rebase drift in an
  untested target” objection
- it means the rebased machine baseline is now trustworthy across libs, tests,
  and all-target compile surfaces
- it lets the next push move directly into Meerkat semantic freeze work

Verification:

- `cargo test --workspace --tests --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo check --workspace --tests --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

Result:

- rebase fallout is fully flushed from the Meerkat freeze baseline
- the remaining Meerkat blockers are semantic freeze blockers, not merge
  cleanup
- the next active work should start at `M3`

## Slice 185 - Restored wait-interrupt API is collapsed out of the baseline

Goal:

- remove the last suspicious resurrected API surface from the rebased machine
  baseline
- confirm that the real Meerkat interrupt boundary lives in runtime/session
  semantics, not in a dead core builder/binder compatibility seam

What landed:

- traced the restored `wait_interrupt` surface and confirmed it had no live
  consumers outside core-local tests and capability plumbing
- removed the resurrected core surface:
  - deleted the extracted `meerkat-core/src/wait_interrupt.rs` module
  - removed the extra `AgentToolDispatcher` wait-interrupt / completion-feed
    binding hooks
  - collapsed `DispatcherCapabilities` back down to the live field
    `ops_lifecycle`
  - removed the extra `AgentBuilder` wait-interrupt compatibility setters
- tightened the freeze docs so they no longer describe those removed seams as
  compatibility no-ops; they are now simply absent from the current boundary

Why this slice matters:

- it removes the last “this API came back from history during rebase” smell
  from the Meerkat freeze baseline
- it makes the rebased branch smaller and more honest: only the real
  runtime/session interrupt semantics remain
- it improves freeze confidence because the surviving delta now tracks
  semantic machine work, not a resurrected dead surface

Verification:

- `cargo test -p meerkat-core --lib --quiet`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `cargo test -p meerkat-session --test ephemeral_contract --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

Result:

- the last suspicious resurrected rebase surface has been removed
- the remaining rebase audit surface is the expected observational machine
  scaffolding warnings only
- the next real Meerkat blocker remains `M3`

## Slice 186 - Final adversarial rebase audit closes the machine baseline

Goal:

- do one last exact-current-state pass for leftover merge artifacts before
  declaring the rebase chapter closed
- make the freeze record internally consistent so we do not carry forward
  conflicting stories about what survived the repair

What landed:

- reran the widest practical audit lanes on the repaired branch:
  - `cargo test --workspace --tests --quiet`
  - `cargo test --workspace --lib --quiet`
  - `cargo check --workspace --tests --quiet`
  - `cargo check --workspace --all-targets --quiet`
- rechecked for conflict markers outside `specs/**` and confirmed none remain
- confirmed the restored `wait_interrupt` surface is now gone from code and
  only remains in historical documentation / changelog references
- aligned the freeze docs so the final story is singular:
  the rebased baseline is trusted again, and the remaining blocker is `M3`,
  not merge fallout

Why this slice matters:

- it changes the rebase status from “probably clean now” to “adversarially
  rechecked and documented as closed”
- it gives reviewers one final stop/go point for the repaired baseline before
  semantic freeze work continues
- it makes it safe to stop spending time on merge damage and spend that time on
  real Meerkat cutover blockers

Verification:

- `cargo test --workspace --tests --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo check --workspace --tests --quiet`
- `cargo check --workspace --all-targets --quiet`
- `rg -n '^(<<<<<<< |=======$|>>>>>>> )' --glob '!specs/**' .`
- `git diff --check`

Result:

- rebase fallout is now fully untangled enough to freeze the machine baseline
- no credible merge-artifact blocker remains in the Meerkat freeze posture
- the next real Meerkat blocker is `M3`

## Slice 187 - Peer-ingress surface seam is reclassified as live, not compatibility-only

Goal:

- make sure the repaired baseline is frozen against the *actual* public seam
  shape, not against an overly internalized story
- remove one last narrative mismatch from the rebase-repair record

What landed:

- re-audited `RuntimeSessionAdapter::update_peer_ingress_context(...)` usage
  across CLI, REST, RPC, MCP, examples, and tests
- confirmed it is still the live surface-facing seam for peer-ingress/drain
  updates, not merely a historical compatibility wrapper
- updated the code comment in `meerkat-runtime/src/session_adapter.rs`
- corrected the earlier slice narrative so the freeze story now matches the
  live call graph: `update_peer_ingress_context(...)` is the public seam, and
  `maybe_spawn_comms_drain(...)` is the lower-level owner method behind it

Why this slice matters:

- it removes one more place where the repaired branch could have frozen the
  wrong story even though behavior was correct
- it raises confidence that “rebase fallout is closed” now means both code and
  documentation match the same exact-current seam map
- it keeps the next freeze work focused on semantic blockers instead of
  lingering baseline narration errors

Verification:

- `rg -n "update_peer_ingress_context\\(" .`
- `git diff --check`

Result:

- the repaired baseline no longer treats a live public seam as compatibility
  debt
- the remaining Meerkat freeze blocker is still `M3`

## Slice 188 - Remaining compatibility markers are classified as product seams, not rebase fallout

Goal:

- separate surviving compatibility/fallback wording from actual rebase damage
- make sure we do not keep auditing intentional current-state seams as if they
  were merge leftovers

What landed:

- re-audited the remaining compatibility/fallback markers in the repaired code
- confirmed `drain_peer_input_candidates(...)` is still a live runtime drain
  bridge over the newer classified-ingress noun, not rebase fallout
- confirmed the raw `tx`/`rx` retained by `Inbox::new_classified(...)` are a
  pre-existing structural vestige tied to `InboxSender::send()`, not merge
  damage
- confirmed the remaining detached-wake legacy fallback is already frozen as
  exact-current compatibility behavior in `meerkat-detached-wake-freeze.md`

Why this slice matters:

- it removes the last ambient suspicion that any surviving compatibility note
  might still be hiding rebase fallout
- it lets the branch freeze posture say something stronger than “tests are
  green”: the remaining transitional seams are understood and intentionally
  classified
- it keeps the next push focused on semantic freeze blockers instead of more
  branch archaeology

Verification:

- `rg -n "drain_peer_input_candidates\\(|drain_classified_inbox_interactions\\(" .`
- `rg -n "InboxSender::send\\(|\\.send\\(" meerkat-comms/src/inbox.rs meerkat-comms/src/runtime/comms_runtime.rs`
- `git diff --check`

Result:

- the remaining compatibility/fallback markers are now classified as
  intentional current-state product seams
- no credible rebase-fallout seam remains in the Meerkat baseline

## Slice 189 - Runtime peer-input drain bridge is documented as a live seam

Goal:

- align the code comments with the final repaired freeze story
- remove the last suggestion that the peer-input drain bridge is merely
  historical compatibility debt

What landed:

- reclassified `AgentToolDispatcher::drain_peer_input_candidates(...)` in
  `meerkat-core/src/agent.rs` as the live runtime drain bridge for callers
  that still consume `PeerInputCandidate` directly
- kept its implementation unchanged: the bridge still forwards to the
  classified ingress drain path

Why this slice matters:

- it removes one more narrative mismatch between the repaired code and the
  freeze record
- it reinforces the current exact-state story: the remaining bridges are
  intentional product seams, not rebase leftovers

Verification:

- `cargo test -p meerkat-comms --lib --quiet`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `git diff --check`

Result:

- the peer-input drain bridge is now documented as a live seam
- no credible rebase-fallout narrative mismatch remains in the Meerkat baseline

## Slice 190 - K3 exact-current turn / ops / barrier freeze lands in code

Goal:

- stop treating turn / ops / barrier coupling as the last undefined Meerkat
  red zone
- freeze only the exact joined invariants the current code can honestly
  support

What landed:

- added exact-current K3 validator rules in `meerkat/src/meerkat_machine.rs`:
  - `WaitingForOps` requires `tool_calls_pending > 0`
  - pending ops outside `WaitingForOps` are invalid
  - barrier flag / barrier id-set coherence is explicit
  - unsatisfied barrier without barrier ops is invalid
  - pending op ids must resolve in the ops registry snapshot
  - barrier op ids must resolve in the ops registry snapshot
  - barrier op ids must be a subset of pending op ids
- added focused negative proof:
  - `validate_meerkat_machine_snapshot_reports_turn_ops_barrier_violations`

Why this slice matters:

- it turns `K3` from “known hard area” into a bounded exact-current freeze
  surface
- it freezes the live runner lowering without pretending the cross-crate
  snapshot is fully atomic

Verification:

- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_turn_ops_barrier_violations`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- the exact-current K3 validator holds on the widened Meerkat lane
- no owner-boundary movement was needed

## Slice 191 - Full MeerkatMachine freeze set is assembled

Goal:

- stop treating Meerkat freeze as a checklist with floating blockers
- produce the actual exact-current freeze set we can hand to TLA+

What landed:

- closed `K3` with `meerkat-turn-ops-barrier-freeze.md`
- closed `K4` with `meerkat-peer-ingress-freeze.md`
- closed `K5` with `meerkat-tool-surface-freeze.md`
- closed `K6` with `meerkat-drain-freeze.md`
- closed `K7` with `meerkat-input-effect-alphabet.md`
- closed `K8` with `meerkat-lowering-map.md`
- closed `K9` with `meerkat-ownership-decisions.md`
- closed `K10` and top-level `M1` with `meerkat-machine-freeze.md`
- updated `meerkat-cutover-checklist.md` from active blocker list to closed
  exact-current freeze checklist

Why this slice matters:

- this is the point where `MeerkatMachine` becomes a reviewable exact-current
  machine asset rather than a collection of good partial notes
- it is the first honest point at which the next step can be “TLA+ proof of
  frozen Meerkat,” not “keep discovering Meerkat boundaries”

Verification:

- focused K1-K6 lanes as listed in the freeze notes
- workspace-wide lib / test / compile lanes

Result:

- top-level `M1 = MeerkatMachine` is now frozen as an exact-current asset
- the next active blocker is no longer Meerkat freeze; it is TLA+ proof of the
  frozen machine

## Slice 192 - K1 final audit removes stale session-layer interrupt claim

Goal:

- make the full `MeerkatMachine` freeze honest by removing the one remaining
  stale proof claim in `K1`
- verify that the exact-current interrupt boundary is the runtime-adapter seam,
  not a separate `meerkat-session` action that no longer exists

What landed:

- re-audited `meerkat-session` and confirmed there is no current top-level
  `interrupt_yielding` entrypoint, control verb, or contract test there
- narrowed `meerkat-interrupt-freeze.md` to the actual exact-current boundary:
  - `interrupt_current_run`
  - runtime-adapter `InterruptYielding`
  - explicit absence of a second session-layer interrupt lowering
- updated `meerkat-machine-freeze.md`, `meerkat-cutover-checklist.md`, and
  `meerkat-lowering-map.md` to match that exact-current classification

Why this slice matters:

- without it, the full Meerkat freeze still cited a proof that no longer
  existed
- with it, the freeze set stops overclaiming and becomes review-safe enough
  for exact-current TLA+ work

Verification:

- `rg -n "interrupt_yielding|InterruptYielding|CancelCurrentRun" meerkat-session meerkat-runtime meerkat-core`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `K1` is now frozen as the actual exact-current runtime-adapter interrupt
  boundary
- the top-level `MeerkatMachine` freeze no longer depends on a nonexistent
  session-layer proof seam

## Slice 193 - Full MeerkatMachine freeze reruns clean after final K1 audit

Goal:

- prove that the final `M1 = MeerkatMachine` freeze still holds after the K1
  narrowing
- rerun the full documented freeze surface instead of relying on older green
  runs

What landed:

- reran the focused freeze lanes from `meerkat-machine-freeze.md`
- reran the widened workspace test and compile lanes
- confirmed the only remaining broad warnings are the expected observational
  `dead_code` warnings from the diagnostic `MeerkatMachine` / `MobMachine`
  scaffolding

Why this slice matters:

- this is the point where the top-level Meerkat freeze stops being “assembled”
  and becomes “reverified after the last honest-boundary correction”
- it closes the final gap between “the docs now say the right thing” and “the
  machine is actually ready for exact-current TLA+ work”

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
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
- `cargo test --workspace --tests --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo check --workspace --tests --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

Result:

- top-level `M1 = MeerkatMachine` is now an honest exact-current freeze asset
- the next blocking milestone is TLA+ proof of frozen Meerkat, not more freeze
  closure work

## Slice 194 - Top-level M1 is promoted from exact-current baseline to target-state machine freeze

Goal:

- stop conflating the exact-current Meerkat baseline with the actual target
  machine we intend to prove and cut over to
- freeze one explicit target-state `MeerkatMachine` instead of forcing TLA+ to
  infer target cleanup from scattered drafts

What landed:

- split the freeze story into two top-level assets:
  - `meerkat-machine-exact-current-freeze.md` for the implementation baseline
  - `meerkat-machine-freeze.md` for the target-state single-machine design
- resolved the previously open target questions directly in the target freeze:
  - explicit completion-waiter subregion
  - `cancel_after_boundary` in the target alphabet
  - replayable peer-ingress backlog
  - tool-surface recovery as machine truth
  - all drain modes as machine-owned state
  - reset-driven epoch rotation
  - removal of the legacy detached-wake fallback from the target machine
- updated `meerkat-cutover-checklist.md` so it is clearly the audit trail for
  the exact-current baseline, not the target freeze itself

Why this slice matters:

- without it, the top-level freeze would still be “what the code does today”
  rather than “the single machine we actually intend to prove”
- with it, `M1` is finally the target-state machine the user originally asked
  for, and the exact-current baseline remains available as comparison evidence

Verification:

- `git diff --check`
- architecture-doc consistency audit across:
  - `meerkat-kernel-shape.md`
  - `meerkat-state-machine-sketch.md`
  - `meerkat-owned-facts-ledger.md`
  - exact-current freeze notes

Result:

- top-level `M1 = MeerkatMachine` is now frozen as the target-state single
  machine
- the exact-current freeze remains preserved separately as implementation
  baseline for TLA+ and review

## Slice 195 - Target freeze removes the last implementation-colored lifecycle hedges

Goal:

- make the target `MeerkatMachine` read like architecture, not like a
  half-translated implementation note

What landed:

- `InterruptCurrentRun` is now stated as a machine-owned cancellation request
  observed at machine-visible cancellation boundaries
- `InterruptYielding` is now stated as a machine input/effect regardless of one
  implementation lowering using executor control
- `RetireRuntime` is now described in target machine terms instead of
  “exact-current drain” language

Why this slice matters:

- the target freeze should not preserve implementation-colored hedges once the
  target design choice is already made

Verification:

- `git diff --check`

Result:

- the target freeze now reads as one closed target machine rather than a
  near-final implementation note

## Slice 196 - Target freeze gets a proof-obligations handoff package

Goal:

- turn the target `MeerkatMachine` freeze into a proof-ready package rather
  than a standalone architecture note

What landed:

- added `meerkat-machine-proof-obligations.md`
- made the target TLA+ boundary, finite abstraction rules, action families,
  safety obligations, liveness obligations, and implementation-delta review
  obligations explicit
- updated `README.md` and `meerkat-machine-freeze.md` to point to the proof
  handoff directly

Why this slice matters:

- the user asked for a full freeze that we can actually take into TLA+
- this closes the gap between “frozen architecture” and “proof-ready machine”

Verification:

- `git diff --check`
- doc consistency pass across:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-exact-current-freeze.md`
  - `meerkat-cutover-checklist.md`
  - `meerkat-machine-proof-obligations.md`

Result:

- `M1 = MeerkatMachine` is now packaged as a proof-ready target-state freeze
  plus exact-current comparison baseline

## Slice 197 - Supporting Meerkat docs stop talking like the target is still open

Goal:

- remove the last support-doc wording that made the target freeze sound
  provisional or half-formalized

What landed:

- `meerkat-kernel-shape.md` now treats the target decisions as closed design
  decisions rather than “open questions”
- `meerkat-state-machine-sketch.md` now states the reset/recover formalization
  as complete target commitments, not future work
- `meerkat-cutover-checklist.md` now points directly at
  `meerkat-machine-proof-obligations.md` as part of the post-freeze handoff

Why this slice matters:

- a freeze is only believable if the surrounding docs also stop hedging

Verification:

- `git diff --check`
- doc wording audit across:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-proof-obligations.md`
  - `meerkat-kernel-shape.md`
  - `meerkat-state-machine-sketch.md`
  - `meerkat-cutover-checklist.md`

Result:

- the active Meerkat support docs now all treat `M1` as a closed target-state
  machine and proof handoff, not as an open design sketch

## Slice 198 - Final broad validation after proof-package hardening

Goal:

- ensure the final proof-ready Meerkat freeze package is backed by a fresh
  workspace-wide validation run, not only by earlier green lanes

What landed:

- reran the broad workspace test and compile lanes after the target freeze,
  proof-obligations handoff, and support-doc consistency cleanup

Why this slice matters:

- this is the last honesty check before calling the Meerkat freeze package
  ready for TLA+

Verification:

- `cargo test --workspace --tests --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

Result:

- the full Meerkat freeze package is backed by a fresh green workspace run
- the only broad warnings remaining are the expected observational
  `dead_code` warnings from the diagnostic machine scaffolding

## Slice 199 - Target freeze gets a canonical transition catalog

Goal:

- close the remaining gap between “frozen machine state” and “proof-ready
  machine dynamics”

What landed:

- added `meerkat-machine-transition-catalog.md`
- cataloged the target transition families across binding, admission, turn,
  ops, detached wake, peer ingress, tool surface, drain, and lifecycle
- made cross-region transition requirements explicit instead of leaving them
  implicit in the freeze prose
- linked the transition catalog from the target freeze and proof-obligations
  handoff

Why this slice matters:

- a full single-machine freeze should include canonical transition semantics,
  not only state, alphabet, and invariants

Verification:

- `git diff --check`
- target-doc consistency pass across:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-proof-obligations.md`
  - `meerkat-machine-transition-catalog.md`

Result:

- the Meerkat freeze package now includes state, alphabet, invariants, proof
  obligations, and canonical transitions for the target machine

## Slice 200 - Target freeze gets frozen terminology and fairness assumptions

Goal:

- remove the last interpretive gaps that would otherwise be filled in ad hoc
  during TLA+ work

What landed:

- added `meerkat-machine-glossary.md`
- added `meerkat-machine-fairness-assumptions.md`
- linked both into the target freeze and proof-obligations handoff

Why this slice matters:

- terms like `settled`, `quiescent`, `live binding`, and `ordinary admitted
  input` should not be left to modeler interpretation
- liveness claims should not rely on hidden fairness folklore

Verification:

- `git diff --check`
- terminology/fairness consistency audit across:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-proof-obligations.md`
  - `meerkat-machine-transition-catalog.md`
  - `meerkat-machine-glossary.md`
  - `meerkat-machine-fairness-assumptions.md`

Result:

- the Meerkat freeze package now includes frozen state, transitions,
  obligations, vocabulary, and fairness assumptions for TLA+ work

## Slice 201 - Target freeze gets explicit alphabet and region coverage

Goal:

- prove that every target input, effect, and machine region now has an explicit
  home in the freeze package

What landed:

- added `meerkat-machine-coverage-matrix.md`
- fixed the transition catalog gap for:
  - `WaitAllSatisfied`
  - `ExecutorControl(StopRuntimeExecutor)`
  - `PersistRecoverySnapshot`
- linked the coverage matrix from the target freeze and proof handoff

Why this slice matters:

- this closes the exact sort of “looks complete but silently forgot an action”
  gap that would otherwise show up only during TLA+ modeling

Verification:

- target alphabet coverage audit against:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-transition-catalog.md`
  - `meerkat-machine-coverage-matrix.md`
- `git diff --check`

Result:

- every target input, every target effect, and every machine region now has an
  explicit home in the Meerkat freeze package

## Slice 202 - Target freeze gets a canonical state schema and `Init`

Goal:

- close the remaining gap between “proof-ready transitions” and “proof-ready
  state space”

What landed:

- added `meerkat-machine-state-schema.md`
- defined canonical region schemas, phase/status domains, and the target
  machine `Init`
- added explicit region typing obligations so `TypeOK` does not have to be
  reconstructed ad hoc during TLA+ work
- linked the state schema from the target freeze and proof handoff

Why this slice matters:

- a full single-machine freeze is still incomplete if the modeler has to invent
  `Init`, region domains, or phase lattices while writing the proof

Verification:

- `git diff --check`
- target-package consistency pass across:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-proof-obligations.md`
  - `meerkat-machine-transition-catalog.md`
  - `meerkat-machine-state-schema.md`

Result:

- the Meerkat freeze package now includes canonical target state, `Init`,
  transitions, obligations, vocabulary, fairness, and coverage

## Slice 203 - Target freeze gets explicit abstract domains and derived predicates

Goal:

- remove the remaining need for a modeler to invent abstract domains or key
  machine predicates while writing TLA+

What landed:

- extended `meerkat-machine-state-schema.md` with explicit abstract scalar and
  classification domains
- added `meerkat-machine-derived-predicates.md`
- linked both into the target freeze and proof handoff

Why this slice matters:

- a single-machine freeze is still softer than it should be if terms like
  `Quiescent`, `SettledRetired`, `BarrierPending`, or `TurnPrimitiveKind`
  remain implicit

Verification:

- `git diff --check`
- target-package consistency pass across:
  - `meerkat-machine-freeze.md`
  - `meerkat-machine-state-schema.md`
  - `meerkat-machine-derived-predicates.md`
  - `meerkat-machine-proof-obligations.md`

Result:

- the Meerkat freeze package now includes explicit target domains and derived
  predicates, not just raw fields and prose terminology

## Slice 204 - Target TLC forced live-binding lifecycle preconditions

Goal:

- tighten the target machine so lifecycle verbs cannot act on a pre-binding
  `Initializing` shell

What landed:

- strengthened target TLC preconditions so:
  - `RetireRuntime` requires a live binding
  - `StopRuntime` requires a live binding
  - `DestroyRuntime` requires a live binding
- aligned the transition catalog with those requirements

Why this slice matters:

- the stronger TLC invariant set immediately exposed that the earlier target
  model allowed impossible lifecycle commands before a binding existed

Verification:

- bounded TLC base and stress runs after the lifecycle-precondition fix

Result:

- the target machine no longer permits “retire/stop/destroy before bind”
  traces

## Slice 205 - Target peer replay gets canonical terminalized lineage

Goal:

- make peer replay uniqueness provable from machine state instead of from
  transient backlog shape

What landed:

- promoted peer lineage into canonical target state:
  - `peer.terminalized_items`
  - `peer.admitted_items`
- updated the target freeze, state schema, glossary, proof obligations, and
  transition catalog to treat that lineage as machine-owned truth
- updated the TLC model so:
  - available peer items exclude terminalized lineage
  - peer admission terminalizes and records admitted lineage
  - non-admitted peer drain terminalizes lineage

Why this slice matters:

- without terminalized peer lineage, the target machine could not honestly
  prove “peer replay never duplicates already admitted peer work”

Verification:

- bounded TLC base and stress runs with the stronger peer lineage state
- `git diff --check`

Result:

- peer replay uniqueness is now expressible and checked from canonical machine
  state

## Slice 206 - Stronger tool invariants exposed applied-state gaps

Goal:

- raise the tool-surface part of the target machine from loose visibility
  subset checks to real applied-state consistency

What landed:

- added stronger target invariants for:
  - visible surfaces matching applied base state
  - staged surfaces remaining machine-known
  - pending removal implying `Removing`
  - inflight calls requiring applied surface state
- TLC exposed two real target underspecs:
  - `StageReload` was legal on known-but-never-applied surfaces
  - final removal left stale staged remove lineage behind
- fixed the target model so:
  - `StageReload` requires an already applied surface
  - both removal finalization paths clear staged remove lineage

Why this slice matters:

- these are exactly the sorts of cross-boundary tool-surface ambiguities that
  become expensive if they are left to informal interpretation before proof

Verification:

- bounded TLC base and stress runs with the stronger tool-surface invariants

Result:

- the target machine now has a tighter and more honest applied-surface story

## Slice 207 - Stronger target TLC safety set survives base and stress runs

Goal:

- confirm that the stricter peer, input, barrier, and tool invariants are
  coherent as target-state commitments

What landed:

- expanded the TLC safety set with invariants for:
  - live-binding lifecycle legality
  - resolved waiters being truly cleared
  - queue/steer disjointness and handling-mode legality
  - contributor lifecycle legality
  - terminal inputs excluded from queue/steer
  - barrier satisfaction legality
  - replayable peer-class correctness
  - terminalized peer backlog exclusion
  - admitted peer lineage inclusion
  - applied-surface equality and staged/pending removal legality

Verification:

- base TLC:
  - `56,757` states generated
  - `14,954` distinct states
  - depth `6`
- stress TLC:
  - `432,922` states generated
  - `110,144` distinct states
  - depth `7`
- no invariant violations

Result:

- the target single-machine freeze is now materially stronger than the first
  TLC pass, and the added obligations have already forced honest corrections
  into the target model

## Slice 208 - Mob target TLC exposed dispatch-capacity as a terminalization obligation

Goal:

- push the `MobMachine` target scaffold past the first bounded TLC pass without
  baking in a false running-flow theorem

What landed:

- widened the target model so work submission and step dispatch choose only
  dispatch-capable members
- added `FailRunNoDispatchCapacity` as an explicit target transition
- promoted dispatch-capable-member vocabulary into the Mob freeze, derived
  predicates, proof obligations, and transition catalog
- backed out the false state invariant that required dispatch capacity to hold
  in every running-undispatched state

Why this slice matters:

- TLC found a real target mismatch: a run can still be `Running` while the
  last dispatch-capable member transitions to `Retiring`, and the honest model
  response is an explicit failure transition, not a false safety theorem

Verification:

- bounded TLC base after the dispatch-capacity correction
- bounded TLC stress after the dispatch-capacity correction

Result:

- the Mob target machine now treats dispatch capacity as a terminalization
  obligation instead of an always-true state invariant

## Slice 209 - Mob target TLC base and stress are green

Goal:

- confirm the corrected `MobMachine` target state is coherent under bounded TLC

What landed:

- base TLC passed on `MobMachineTarget.cfg`
- stress TLC passed on `MobMachineTargetStress.cfg`
- the target scaffold now covers the full Mob input alphabet with no missing
  transition definitions or `Next` entries

Verification:

- base TLC:
  - `9,385` states generated
  - `3,834` distinct states
  - depth `7`
- stress TLC:
  - `45,139` states generated
  - `16,221` distinct states
  - depth `8`
- coverage audit:
  - `Missing defs: []`
  - `Missing next: []`

Result:

- the `MobMachine` target scaffold is now executable, bounded, and complete
  enough to act as a real proof handoff asset

## Slice 210 - Mob freeze package is now a full proof handoff

Goal:

- make the Mob freeze package internally closed, not just TLC-green

What landed:

- updated `tla/README.md` so it accurately reports both Meerkat and Mob target
  passes and the target corrections TLC forced on Mob
- added an explicit proof-handoff section to `mob-machine-freeze.md`
- updated the top-level research README to mark both `M1` and `M2` as frozen
  target packages with bounded TLC base/stress passes

Why this slice matters:

- without the handoff packaging, the Mob freeze would still depend on local
  context about which files matter and why the model changed

Verification:

- doc consistency pass across active `mob-machine-*` freeze files
- `git diff --check`

Result:

- `M2 = MobMachine` now has an honest, self-contained target freeze package
  that is ready for deeper TLA+ proof work

## Slice 211 - Mob target dispatch now respects dependency readiness

Goal:

- close the biggest remaining flow gap in the target model by making step
  dispatch depend on actual dependency satisfaction instead of any `None`
  status slot

What landed:

- added explicit target predicates for:
  - `AllDependenciesSatisfied`
  - `AnyDependencySatisfied`
  - `ReadyDirectDispatchStep`
  - `ReadyResolvedStep`
  - `HasDispatchReadyStep`
- changed `ResolveAnyJoin` so it moves an `Any` join into explicit
  machine-owned `Pending` state instead of pretending dispatch happened
- changed `DispatchStep` so it only chooses dependency-ready steps
- changed `FailRunNoDispatchCapacity` so it only fires when a run has
  dispatch-ready undispatched work and no dispatch-capable member remains

Why this slice matters:

- before this slice, the target machine could dispatch blocked steps and fail
  runs for mere blockedness instead of actual dispatch starvation
- after this slice, the target flow story is materially closer to a real
  orchestration kernel rather than a loose state bag

Verification:

- bounded TLC base passed after the dependency-ready dispatch correction:
  - `9,479` states generated
  - `3,914` distinct states
  - depth `7`
- bounded TLC stress passed after the dependency-ready dispatch correction:
  - `217,271` states generated
  - `71,736` distinct states
  - depth `9`

Result:

- the target Mob flow machine now distinguishes blocked work from
  dispatch-ready work honestly

## Slice 212 - Mob target terminal runs now clear live task bindings

Goal:

- tighten the task-board surface so it behaves like one machine, not a stale
  projection over terminal runs

What landed:

- added `LiveTaskRunBindingInvariant`
- changed `CompleteRun`, `FailRun`, `CancelRun`, `FailRunNoDispatchCapacity`,
  and terminalizing `MarkStepCompleted` / `MarkStepFailed` paths so they clear
  any live task binding to the terminalized run
- changed `UpdateTask` so new task attachments only target non-terminal runs

Why this slice matters:

- before this slice, the target model could leave active task bindings pointing
  at already terminal runs
- that would have made the task-board region weaker than the frozen machine
  claims about owning coordination truth

Verification:

- bounded TLC base passed after task/run cleanup tightening:
  - `9,479` states generated
  - `3,914` distinct states
  - depth `7`
- bounded TLC stress passed after task/run cleanup tightening:
  - `217,256` states generated
  - `71,696` distinct states
  - depth `9`

Result:

- live task/run binding semantics are now part of the honest Mob target

## Slice 213 - TLC rejected a false global history-frontier theorem

Goal:

- strengthen the history surface without baking in the wrong interpretation of
  per-identity history state

What landed:

- attempted `HistoryFrontierInvariant`
- TLC immediately falsified it
- backed it out and replaced it with
  `HistoryIdentitySequenceInvariant`, which matches current target semantics:
  `last_seq_by_identity[id] = event_count_by_identity[id]`

Why this slice matters:

- this was an honest backtrack the freeze package needed
- the model makes clear that `last_seq_by_identity` is per-identity local
  sequence truth, not a copy of the global frontier

Verification:

- bounded TLC base passed after the history backtrack:
  - `9,479` states generated
  - `3,914` distinct states
  - depth `7`
- bounded TLC stress passed after the history backtrack:
  - `217,256` states generated
  - `71,696` distinct states
  - depth `9`

Result:

- the Mob history freeze is now stronger and more honest at the same time

## Slice 214 - Mob freeze docs are realigned to the stronger target model

Goal:

- bring the freeze prose back into exact alignment with the target model after
  the flow/task/history strengthening pass

What landed:

- updated:
  - `mob-machine-derived-predicates.md`
  - `mob-machine-proof-obligations.md`
  - `mob-machine-transition-catalog.md`
  - `mob-machine-freeze.md`
  - `mob-machine-state-schema.md`
  - `tla/README.md`
  - top-level `README.md`
- the freeze docs now explicitly say that:
  - `Any` joins enter explicit ready/pending state before dispatch
  - only dependency-ready work may dispatch
  - terminal runs clear live task bindings
  - per-identity history sequence/count truth stays aligned

Verification:

- doc consistency pass across active `mob-machine-*` freeze files
- `git diff --check`

Result:

- the Mob freeze package is again honest about what the target model actually
  proves today

## Slice 215 - Target Mob steps now drain live bound work when they terminalize

Goal:

- stop treating flow-step terminalization and bound-work terminalization as
  parallel sidecars inside the target machine

What landed:

- added `BoundLiveWork(r, step)` to the target model
- strengthened `WorkStepStateInvariant` so terminal step states cannot retain
  live bound work
- changed `MarkStepCompleted`, `MarkStepFailed`, `MarkStepSkipped`, and
  `MarkStepCanceled` so they drain any live work bound to that step
- changed terminalizing `MarkStepFailed` paths to drain residual live work
  across the run when the run fails

Why this slice matters:

- before this slice, TLC could drive the target machine into a state where a
  step was already `Canceled` while its bound work still remained live
- that is exactly the kind of split-brain flow/work story the target Mob
  kernel is supposed to eliminate

Verification:

- bounded TLC base passed after step/work tightening:
  - `9,479` states generated
  - `3,894` distinct states
  - depth `7`
- bounded TLC stress passed after step/work tightening:
  - `208,853` states generated
  - `67,559` distinct states
  - depth `9`

Result:

- terminal step transitions now own their bound live work cleanup

## Slice 216 - Target Mob terminal runs now drain residual live bound work

Goal:

- extend the same one-machine cleanup rule from terminal steps to terminal
  runs

What landed:

- added `BoundLiveWorkForRun(r)` and `TerminalRunWorkInvariant`
- changed `CompleteRun`, `FailRun`, `CancelRun`, and
  `FailRunNoDispatchCapacity` so they drain any residual live work bound to
  the terminalized run
- changed terminalizing `MarkStepCompleted` / `MarkStepFailed` paths so
  residual run-bound live work cannot leak past run terminalization

Why this slice matters:

- the previous target model could still have left run-bound live work attached
  to a completed, failed, or canceled run
- that would have meant the target kernel still had two partially independent
  terminalization stories

Verification:

- bounded TLC base passed after run/work tightening:
  - `9,479` states generated
  - `3,894` distinct states
  - depth `7`
- bounded TLC stress passed after run/work tightening:
  - `208,853` states generated
  - `67,559` distinct states
  - depth `9`

Result:

- terminal runs now own cleanup of residual live bound work too

## Slice 217 - Dispatch lifecycle is now explicit in the target Mob machine

Goal:

- tighten the handoff between dependency resolution, dispatch readiness, and
  work allocation

What landed:

- added `PendingStepHasNoBoundWorkInvariant`
- added `DispatchedStepHasBoundWorkInvariant`
- confirmed they hold under both TLC configs without additional transition
  changes

Why this slice matters:

- `Pending` now means “machine-owned ready state before dispatch,” not “ready
  and maybe already carrying work”
- `Dispatched` now means “there is a bound work record,” not just a label in
  the flow region

Verification:

- bounded TLC base passed after dispatch lifecycle tightening:
  - `9,479` states generated
  - `3,894` distinct states
  - depth `7`
- bounded TLC stress passed after dispatch lifecycle tightening:
  - `208,853` states generated
  - `67,559` distinct states
  - depth `9`

Result:

- the target Mob machine now has a materially stronger one-machine story for
  flow-step dispatch, work allocation, and terminal cleanup

## Slice 218 - Branch groups now prove their shared dependency sets

Goal:

- align the target model with the freeze claim that multi-member branch groups
  must not silently diverge in dependency shape

What landed:

- added `BranchGroupDependencyInvariant`
- proved under TLC that steps sharing a non-`None` branch label keep identical
  dependency sets

Why this slice matters:

- the freeze docs already claimed branch-group dependency agreement was part of
  the target machine
- this slice makes the executable model enforce that claim instead of leaving
  it as prose

Verification:

- bounded TLC base passed:
  - `9,479` states generated
  - `3,894` distinct states
  - depth `7`
- bounded TLC stress passed:
  - `208,853` states generated
  - `67,559` distinct states
  - depth `9`

Result:

- branch-group structural consistency is now part of the proved target model

## Slice 219 - Quorum collection now has explicit observed-contribution state

Goal:

- stop treating quorum collection as a free-floating step resolution and make
  contribution progress explicit machine state

What landed:

- added `flows.step_collection_observed`
- added `RecordCollectionContribution`
- added `QuorumReady(r, step)`
- tightened `ResolveCollection` so quorum completion now requires observed
  contribution count to meet the stored threshold
- excluded quorum steps from the direct `MarkStepCompleted` path
- added `QuorumCompletedObservedInvariant`

Why this slice matters:

- before this slice, quorum completion was under-modeled and could resolve
  without any explicit collected contributions
- that was too weak for a “fully featured including flows” Mob freeze

Verification:

- bounded TLC base passed after quorum collection strengthening:
  - `9,479` states generated
  - `3,894` distinct states
  - depth `7`
- bounded TLC stress passed after quorum collection strengthening:
  - `208,853` states generated
  - `67,559` distinct states
  - depth `9`

Result:

- the target Mob machine now models quorum collection progress explicitly
  instead of treating it as an ungrounded step-resolution shortcut

## Slice 220 - Mob target freeze package closes

Goal:

- close `M2 = MobMachine` as a proof-start-ready target package rather than a
  pile of partial notes

What landed:

- added the missing Mob close-out artifact: `mob-cutover-checklist.md`
- verified the bounded TLC base and stress runs on `MobMachineTarget`
- verified the target alphabet coverage audit is clean
- verified the active Mob freeze docs no longer carry hedge phrases

Why this slice matters:

- without an explicit Mob close-out asset, the package still looked less
  settled than `MeerkatMachine`
- this slice moves `MobMachine` from “strong notes” to “reviewable target
  freeze package”

Result:

- `M2 = MobMachine` now has a closed target-state cutover checklist and a
  green executable scaffold

## Slice 221 - Mob handoff package gains alphabet, lowering map, and ownership decisions

Goal:

- make the Mob proof handoff package explicit enough for real TLA+ and later
  refinement review

What landed:

- added `mob-input-effect-alphabet.md`
- added `mob-lowering-map.md`
- added `mob-ownership-decisions.md`
- wired those artifacts into the Mob freeze, research index, and TLA scaffold
  README

Why this slice matters:

- the Mob freeze was missing the same handoff surfaces that made the Meerkat
  package honestly reviewable
- closing those gaps makes the target machine auditable instead of relying on
  implicit interpretation

Result:

- the Mob package now has an explicit target alphabet, lowering map, and
  ownership-decision record

## Slice 222 - Mob derived vocabulary is now fully executable

Goal:

- remove the silent gap between the documented derived Mob vocabulary and the
  executable TLA model

What landed:

- added the missing derived predicates directly to `MobMachineTarget.tla`,
  including:
  - identity/authority helpers
  - topology helpers
  - work/run helpers
  - structural flow helpers
  - task/history helpers
- verified that every predicate listed in
  `mob-machine-derived-predicates.md` now has a direct definition in the TLA
  model

Why this slice matters:

- before this slice, the docs advertised a larger proof vocabulary than the
  executable model actually exposed
- that would have made the freeze package look more complete than it really
  was

Result:

- the documented Mob derived vocabulary now matches the executable proof
  surface exactly

## Slice 223 - Mob proof obligations now map explicitly to invariants and fairness

Goal:

- stop implying proof coverage and state it directly

What landed:

- added `mob-machine-proof-coverage.md`
- added named fairness bundles to `MobMachineTarget.tla` and exposed
  `TargetFairness` / `FairSpec`
- aligned `mob-machine-fairness-assumptions.md`,
  `mob-machine-proof-obligations.md`, `mob-machine-coverage-matrix.md`, the
  Mob freeze, and the TLA README with that coverage story

Why this slice matters:

- a no-excuses target freeze should make it obvious how each obligation lands:
  invariant, transition, fairness bundle, or future refinement item
- this slice closes that last silent handoff gap

Verification:

- derived-predicate coverage audit passed
- transition-in-`Next` audit remained clean
- bounded TLC base remained green
- bounded TLC stress remained green

Result:

- the Mob target package now has explicit proof coverage, not just green TLC
  and implied intent

## Slice 224 - Mob effect mapping is now explicit

Goal:

- stop relying on implied mapping between named target effects and the
  executable `effects` region in the TLA model

What landed:

- added `mob-machine-effect-coverage.md`
- mapped every named target effect to:
  - one concrete `effects.*` field in `MobMachineTarget.tla`
  - at least one emitting transition

Why this slice matters:

- before this slice, the freeze package still required readers to infer how
  semantic effects such as `SubmitWorkToMeerkat` or `PersistMobRun` showed up
  in the executable model
- that was too soft for a no-excuses proof handoff

Result:

- the Mob effect surface is now explicit and reviewable

## Slice 225 - Mob target delta is now explicit

Goal:

- make the exact-current vs target-state Mob delta reviewable as a first-class
  handoff artifact

What landed:

- added `mob-machine-refinement-delta.md`
- wired it into the freeze, checklist, proof obligations, research index, and
  TLA scaffold README

Why this slice matters:

- the target machine can now be stronger than the rebased implementation
  without relying on memory or oral history to explain why
- that makes later proof and cutover review much safer

Verification:

- bounded TLC base remained green
- bounded TLC stress remained green
- transition-in-`Next` audit remained clean
- derived-predicate coverage audit remained clean
- hedge sweep over active Mob freeze docs remained clean

Result:

- the Mob target package now has explicit effect coverage and explicit
  refinement delta, not just target-state notes

## Slice 226 - Mob exact-current abstraction is now explicit

Goal:

- stop making reviewers reconstruct the mapping from today's joined
  `MobMachineSnapshot` into the frozen target regions

What landed:

- added `mob-machine-refinement-map.md`
- wired it into the freeze, checklist, proof obligations, research index, and
  TLA scaffold README

Why this slice matters:

- the package already had an exact-current baseline and a target delta, but it
  still lacked the explicit abstraction map between them
- without that map, later proof and cutover review would still depend on
  interpretation

Result:

- the Mob freeze package now states, region by region, whether each target
  region is:
  - strongly mapped from exact-current state
  - partially mapped
  - or an explicit target-state promotion

## Slice 227 - Mob target seeds dispatch family as real run-start state

Goal:

- stop treating dispatch family as a documented field without executable
  run-start semantics
- close the last flow-family gap between the target freeze and the TLC model

What landed:

- `StartFlowRun` in `tla/MobMachineTarget.tla` now seeds an explicit per-step
  `step_dispatch_mode` assignment from the ordered-step set of the new run
- added `AggregateShapePartitionInvariant` so aggregate shape class is total
  and mutually exclusive for every ordered step
- strengthened the Mob docs to say the same thing:
  - `mob-machine-freeze.md`
  - `mob-machine-state-schema.md`
  - `mob-machine-transition-catalog.md`
  - `mob-machine-proof-obligations.md`
  - `mob-machine-proof-coverage.md`
- added `mob-machine-flow-family-coverage.md` so the freeze package is explicit
  about how flow archetypes and dispatch families compose

Why this slice matters:

- before this slice, the target package claimed explicit dispatch-family truth
  but the executable model still left `step_dispatch_mode` effectively
  unseeded
- that was too soft for a no-excuses flow freeze

Verification:

- bounded TLC base passed with explicit dispatch-mode seeding:
  - `11,051` generated
  - `4,890` distinct
  - depth `7`
- bounded TLC stress passed with explicit dispatch-mode seeding:
  - `452,920` generated
  - `156,041` distinct
  - depth `9`
- `git diff --check` stayed clean

Result:

- the target `MobMachine` now models dispatch family as real machine state at
  run start rather than as a purely documentary promise
- the flow freeze package is explicit about the coverage relationship between:
  - flow archetypes
  - dispatch families
  - aggregate shape class

## Slice 228 - Mob target now records dispatch width as execution-time truth

Goal:

- stop implying that dispatch multiplicity is known at run start
- make flow-step fan-out / fan-in / one-to-one multiplicity explicit machine
  state only when dispatch actually occurs

What landed:

- `StartFlowRun` now seeds `step_dispatch_width` to `0` for every ordered step
- `DispatchStep` now chooses actual dispatch multiplicity at dispatch time and
  stores it in `step_dispatch_width`
- direct `SubmitWork` no longer masquerades as implicit flow-step dispatch
- the target docs now say the same thing across:
  - `mob-machine-freeze.md`
  - `mob-machine-state-schema.md`
  - `mob-machine-transition-catalog.md`
  - `mob-input-effect-alphabet.md`
  - `mob-machine-proof-obligations.md`
  - `mob-machine-proof-coverage.md`
  - `mob-machine-effect-coverage.md`
  - `tla/README.md`

Why this slice matters:

- the previous package had explicit dispatch family but still blurred planned
  shape and executed multiplicity
- that would have let the proof package overclaim step/work coupling without
  actually carrying multiplicity as machine truth

Result:

- dispatch width is now honest target machine state
- bound work count and stored dispatch width are tied together by the target
  invariants and TLC scaffold

## Slice 229 - Mob target top-level lifecycle is now fully documented

Goal:

- stop leaving the top-level mob lifecycle implicit in the TLA model while the
  prose package focused only on member and flow lifecycle

What landed:

- added `StopMob`, `CompleteMob`, and `DestroyMob` to the transition catalog
- added top-level lifecycle obligations and proof coverage
- added top-level lifecycle semantics to `mob-machine-freeze.md`
- tightened target-facing wording so target docs now describe machine
  permissions and guarantees directly instead of relying on loose “may/should”
  phrasing

Why this slice matters:

- the target package is not honestly single-machine if the machine-level phase
  transitions only exist in the executable model
- reviewers should not have to infer terminal mob lifecycle from `Next`

Result:

- top-level mob lifecycle is now first-class in the target package, not just in
  the scaffold

## Slice 230 - Mob freeze package passed final mechanical honesty audit

Goal:

- verify that the target package is actually closed, not merely broad

What landed:

- transition catalog and TLA `Next` arms were checked mechanically and now
  match exactly
- target-facing hedge sweep is clean; the only remaining hedge hit is the TLC
  README note about bounded deadlock-check configuration
- `git diff --check` remains clean

Why this slice matters:

- a freeze package can still hide a silent mismatch between prose and model
  even when TLC passes
- this closes the last silent-gap risk for the target `MobMachine`

Result:

- `MobMachine` now has a fully featured target freeze package that is honest
  about flows, dispatch family, dispatch width, top-level lifecycle, effects,
  obligations, and refinement framing
- the next step is real TLA+ proof work, not more freeze repair

## Slice 231 - Mob freeze gets an explicit proof handoff and wider audit envelope

Goal:

- stop leaving the Mob freeze handoff implicit in a pile of package docs
- distinguish canonical bounded proof gates from wider exploratory search

What landed:

- added `mob-machine-proof-handoff.md`
- linked the handoff into the target freeze and research index
- added `MobMachineTargetAudit.cfg` as an explicitly exploratory widened TLC
  config
- recorded the wider search envelope in the handoff and TLA README

Why this slice matters:

- a freeze can still be misleading if it collapses “passed bounded gate” and
  “explored more widely without failure” into one claim
- this makes the proof posture honest for reviewers before the full proof pass

Verification:

- canonical bounded base and stress TLC remain the passing freeze gates
- exploratory audit reached:
  - `8,042,572` generated
  - `2,379,112` distinct
  - depth `11`
  - with no invariant violations observed before deliberate termination
- `git diff --check` stayed clean

Result:

- `MobMachine` now has the same style of explicit proof handoff as
  `MeerkatMachine`
- the freeze package cleanly separates:
  - canonical bounded passes
  - exploratory widened search
  - next-step proof work

## Slice 232 - Bounded liveness probe stays a proof target, not a freeze gate

Goal:

- pressure-test the frozen Mob target with real temporal properties
- avoid treating bounded-liveness artifacts as freeze-complete proof

What landed:

- added named temporal properties to `MobMachineTarget.tla` for:
  - destroyed-state absorption
  - pending-spawn eventual clearance
  - kickoff eventual clearance
- ran an exploratory bounded `FairSpec` TLC pass against those properties
- accepted TLC's warning that liveness with `StateConstraint` is not an honest
  freeze-complete gate
- updated the proof handoff and fairness docs to classify those temporal claims
  as the next proof phase rather than as completed freeze checks

Why this slice matters:

- this prevents us from quietly collapsing “we have fairness assumptions” into
  “we have already proved liveness”
- the failed bounded temporal run is useful learning, not a reason to weaken
  the target freeze

What TLC taught us:

- the counterexample was shaped by the bounded `step_count` cap rather than a
  credible target-machine semantic hole
- bounded safety remains the right canonical freeze gate

Result:

- the Mob freeze package is now more honest about the boundary between:
  - completed bounded safety work
  - exploratory liveness probing
  - the upcoming real temporal proof pass

## Slice 233 - Mob freeze gets an explicit closeout artifact

Goal:

- stop leaving freeze completion implied by the package shape
- make it explicit when freeze authoring ends and proof work begins

What landed:

- added `mob-machine-freeze-closeout.md`
- linked that closeout from the top-level target freeze and research index
- made the reopen criteria explicit instead of leaving them implicit in the
  proof handoff

Why this slice matters:

- a large package can still feel “maybe still open” even when the target
  machine is actually frozen
- this gives reviewers one canonical note for why freeze authoring is complete
  and what would legitimately reopen it

Result:

- `MobMachine` now has the same kind of explicit freeze closeout posture as the
  Meerkat side
- the next honest step is proof, not more freeze authoring

## Slice 234 - Mob freeze gets an explicit package-level audit

Goal:

- close the last reviewer ambiguity between semantic `Pending` vocabulary and
  unresolved authoring markers
- make package completeness explicit instead of inferred

What landed:

- added `mob-machine-package-audit.md`
- recorded presence of the full target package
- recorded the canonical bounded mechanical checks
- recorded that the remaining open-marker sweep hits are semantic status words,
  not unresolved TODOs

Why this slice matters:

- a large freeze package can still feel “not really closed” if grep-style
  marker sweeps produce hits without context
- this closes that ambiguity directly

Result:

- `MobMachine` now has an explicit package-level audit in addition to the proof
  handoff and closeout
- the next honest step is proof, not more freeze assembly

## Slice 235 - Mob freeze gets an explicit self-containment audit

Goal:

- verify that the target package stands on its own rather than relying on the
  exact-current baseline for machine meaning

What landed:

- added `mob-machine-self-containment-audit.md`
- recorded that the exact-current Mob docs remain comparison/refinement aids,
  not hidden semantic dependencies of the target machine

Why this slice matters:

- a freeze package is not fully honest if reviewers must reconstruct target
  semantics from the implementation baseline
- this closes the last target/current entanglement concern directly

Result:

- `MobMachine` target semantics are now explicitly self-contained
- the next honest step remains proof, not more freeze authoring

## Slice 236 - Mob freeze gets an explicit traceability audit

Goal:

- prove that the named target package vocabulary resolves to the executable
  model rather than relying on reviewer trust

What landed:

- added `mob-machine-traceability-audit.md`
- recorded that target-facing file references resolve
- recorded that all documented transition names resolve to executable TLA
  definitions
- recorded that all documented derived predicate names resolve to executable TLA
  definitions

Why this slice matters:

- a freeze package is stronger when named docs, named transitions, and named
  predicates are mechanically traceable to the model
- this closes the last “is this only narrative?” concern directly

Result:

- the target-facing `MobMachine` package is now document-complete, self-
  contained, package-audited, and traceable to the executable model
- the next honest step is proof, not more freeze authoring

## Slice 237 - Mob freeze closure artifacts become first-class freeze inputs

Goal:

- remove the last gap between “the audits exist” and “the official freeze path
  actually treats them as part of the closed package”

What landed:

- updated `mob-cutover-checklist.md` so the current posture and final review
  criteria explicitly include:
  - `mob-machine-proof-handoff.md`
  - `mob-machine-self-containment-audit.md`
  - `mob-machine-traceability-audit.md`
  - `tla/MobMachineTargetAudit.cfg`
- updated `mob-machine-proof-handoff.md` so the package-level reviewer audits
  are part of the official handoff set
- updated `mob-machine-package-audit.md` so package presence now explicitly
  includes `mob-machine-self-containment-audit.md` and
  `mob-machine-traceability-audit.md`
- reran the closure checks:
  - target-facing Mob docs sweep clean with `NO_HEDGES`
  - package-presence audit resolves with `MISSING []`
  - `git diff --check` is clean

Why this slice matters:

- before this pass, the Mob package was substantively closed, but the newest
  closure artifacts were still slightly “adjacent” to the main freeze path
- this removes that ambiguity and makes the package closure explicit in the
  documents reviewers will actually read first

Result:

- the Mob freeze package now closes as one coherent reviewer-facing story
  rather than as a pile of separately good documents
- there is no remaining honest freeze-authoring work left; the next step is
  the real TLA+ proof phase

## Slice 238 - Mob proof phase adds a focused lifecycle liveness harness

Goal:

- stop treating top-level lifecycle temporal properties as a full-`FairSpec`
  concern when they really belong to a smaller canonical proof lane

What landed:

- added `LifecycleOperationalNext`, `LifecycleHarnessNext`,
  `LifecycleFairness`, `LifecycleSpec`, and `LifecycleFairSpec` to
  `tla/MobMachineTarget.tla`
- added `tla/MobMachineTargetLifecycleLiveness.cfg`
- rerouted the canonical proof story so:
  - `DestroyedAbsorbingProp`
  - `PendingSpawnEventuallyClearsProp`
  - `KickoffEventuallyClearsProp`
  are covered by the focused lifecycle harness
- kept `MobMachineTargetLiveness.cfg` as a wider exploratory full-`FairSpec`
  run rather than a hidden canonical proof gate
- updated:
  - `tla/README.md`
  - `mob-machine-proof-handoff.md`
  - `mob-machine-freeze-closeout.md`
  - `mob-cutover-checklist.md`

Why this slice matters:

- the earlier proof package had an asymmetry: work and flow had compact focused
  liveness harnesses, but lifecycle properties still rode the much larger full
  fair spec
- that made the proof posture less honest than it looked, because the
  top-level liveness run was growing much faster than the canonical lanes
- this pass restores symmetry and makes every named temporal property part of a
  bounded canonical proof lane

Result:

- `MobMachine` now has four canonical focused proof lanes:
  - lifecycle
  - provisioning/kickoff
  - work ledger
  - flow-step progress
- full-`FairSpec` liveness remains useful exploratory search, but no longer
  masquerades as unfinished canonical proof

## Slice 239 - Mob proof phase adds focused recovery and task liveness harnesses

Goal:

- extend the proof surface beyond lifecycle/work/flow only where the target
  semantics already justify it

What landed:

- added `RecoveryHarnessNext`, `RecoverySpec`, `RecoveryFairness`, and
  `RecoveryFairSpec` to `tla/MobMachineTarget.tla`
- added `TaskHarnessNext`, `TaskSpec`, `TaskFairness`, and `TaskFairSpec` to
  `tla/MobMachineTarget.tla`
- added:
  - `RestoreFailureEventuallyClearsProp`
  - `LiveTaskEventuallyClosesProp`
- added focused configs:
  - `tla/MobMachineTargetRecoveryLiveness.cfg`
  - `tla/MobMachineTargetTaskLiveness.cfg`
- updated the proof handoff/docs so recovery and task closure are now part of
  the canonical proof surface instead of living only in prose fairness notes

Why this slice matters:

- before this pass, recovery fairness and task fairness were documented, but
  they were not exercised by their own executable proof lanes
- that left an asymmetry between what the package claimed and what TLC had
  actually checked
- both new properties were kept only because the model sustained them cleanly;
  if either had failed, they would have been narrowed or removed instead of
  papered over

Result:

- `MobMachine` now has six canonical focused temporal lanes:
  - lifecycle
  - provisioning/kickoff
  - recovery
  - task lifecycle
  - work ledger
  - flow-step progress
- wider full-`FairSpec` liveness remains exploratory and is no longer carrying
  canonical proof weight by accident

## Slice 240 - Mob/Meerkat seam composition model proves the small bridge contract

Goal:

- move from isolated machine proofs to the first honest "together" proof
  surface

What landed:

- added the executable seam model:
  - `tla/MobMeerkatCompositionTarget.tla`
- added canonical configs:
  - `tla/MobMeerkatCompositionTarget.cfg`
  - `tla/MobMeerkatCompositionTargetStress.cfg`
  - `tla/MobMeerkatCompositionTargetLifecycleLiveness.cfg`
  - `tla/MobMeerkatCompositionTargetFlowWorkLiveness.cfg`
- added handoff docs:
  - `mob-meerkat-composition-freeze.md`
  - `mob-meerkat-composition-proof-handoff.md`
- updated:
  - `README.md`
  - `tla/README.md`

Why this slice matters:

- the single-machine proofs establish that `MeerkatMachine` and `MobMachine`
  are individually coherent, but not yet that `MobMachine` depends only on the
  small seam contract
- this composition model proves exactly that reduced claim instead of trying to
  brute-force the full product of both detailed machines
- TLC forced two real seam corrections:
  - pending submit may legally remain targeted at a superseded retiring
    runtime/fence during reprovision, so the invariant had to be weakened to
    "matches bound work target" rather than "targets current ready runtime"
  - `DestroyMember` had to be tightened so destroy cannot be reissued after the
    bridge has already destroyed the runtime and is waiting on the terminal
    event

Result:

- base safety passes:
  - `173` generated / `47` distinct / depth `8`
- widened safety passes:
  - `589` generated / `152` distinct / depth `10`
- lifecycle liveness passes:
  - `407` generated / `197` distinct / depth `15`
- flow/work liveness passes:
  - `25` generated / `19` distinct / depth `13`
- we now have a first honest proof surface for "machines together" at the seam
  abstraction, not just in isolation

## Slice 241 - Seam composition package gets review-ready closeout

Goal:

- make the new composition proof surface reviewable as a closed package instead
  of a bare model plus TLC outputs

What landed:

- added:
  - `mob-meerkat-composition-closeout.md`
  - `mob-meerkat-composition-package-audit.md`
- updated:
  - `README.md`

Why this slice matters:

- the single-machine proof packages already had explicit closeout and audit
  surfaces
- without the same treatment, the composition proof could still look
  second-class even though it is now the key “machines together” artifact

Result:

- the seam composition proof is now packaged as a closed review surface, not
  just a passing TLA model

## Slice 242 - Rebased machine baseline restored after upstream tool-visibility drift

Goal:

- bring the frozen `MeerkatMachine` / `MobMachine` / seam proof baseline
  forward onto `origin/main` without losing machine honesty

What happened:

- rebased `codex/machines-redux` onto `origin/main`
- the only real semantic conflict cluster was the expected tools seam:
  - `meerkat-core/src/tool_scope.rs`
  - `meerkat-core/src/gateway.rs`
  - `meerkat-mcp/src/adapter.rs`
  - `meerkat-session/src/ephemeral.rs`
- resolved those files machine-first:
  - upstream exact-catalog and durable visibility ownership won
  - branch-side machine diagnostic/projection surfaces were retained only where
    they are still genuinely consumed
- two post-rebase code fixes were required:
  - `ToolScopeSnapshot` now projects from `SessionToolVisibilityState`
    instead of pre-upstream `ToolScopeState` filter fields
  - `RuntimeSessionAdapter::session_has_comms(...)` now reads the
    adapter-owned comms-drain slot presence instead of a removed
    `RuntimeSessionEntry.comms_runtime` field
  - `McpRouterAdapter::new(...)` duplicate cache initialization from the merge
    was removed

Verification:

- `git diff --check` clean
- no conflict markers remain outside `specs/**`
- `cargo check --workspace --all-targets --quiet` passes
- `cargo test --workspace --lib --quiet` passes
- `cargo test --workspace --tests --quiet` passes
- canonical Meerkat TLC safety lanes pass:
  - base: `56,757` generated / `14,954` distinct / depth `6`
  - stress: `432,922` generated / `110,144` distinct / depth `7`
- canonical Mob TLC lanes pass:
  - base safety: `10,821` / `1,110` / depth `6`
  - stress safety: `388,822` / `25,481` / depth `8`
  - lifecycle liveness: `922` / `437` / depth `15`
  - provisioning liveness: `207,110` / `48,862` / depth `26`
  - recovery liveness: `44` / `18` / depth `7`
  - task liveness: `9` / `7` / depth `5`
  - work liveness: `63` / `46` / depth `14`
  - flow liveness: `1,911` / `1,010` / depth `17`
- canonical seam composition lanes pass:
  - base safety: `173` / `47` / depth `8`
  - stress safety: `589` / `152` / depth `10`
  - lifecycle liveness: `407` / `197` / depth `15`
  - flow/work liveness: `25` / `19` / depth `13`

Result:

- the rebased branch is back to a real frozen/proven pre-cutover baseline
- upstream drift materially advanced `MeerkatMachine.tools`, but did not
  invalidate `MobMachine` or the seam proof story
- the active blocker is again cutover/refinement work, not rebase fallout

## Slice 243 - Seam contract docs aligned to the proven composition package

Goal:

- stop leaving the Mob-Meerkat seam prose behind the actual composition proof

What landed:

- upgraded:
  - `abstract-member-contract.md`
  - `bridge-alphabet.md`
- updated:
  - `mob-meerkat-composition-freeze.md`
  - `README.md`

Why this slice matters:

- the seam composition model and proof package were already frozen and green,
  but the underlying seam docs still described themselves as working drafts
- that made the architecture story weaker than the proof surface actually is

Result:

- the abstract member contract is now explicitly the frozen composition-facing
  seam contract
- the bridge alphabet is now explicitly the frozen seam alphabet
- the composition freeze now states that those seam docs are frozen supporting
  artifacts rather than open drafts

## Slice 244 - Refinement/cutover program replaces ad hoc "next steps"

Goal:

- turn the now-frozen machine and seam packages into one explicit
  pre-cutover execution plan

What landed:

- added:
  - `two-kernel-refinement-program.md`
- updated:
  - `README.md`

Why this slice matters:

- the repo already had the frozen machine packages, the frozen seam package,
  and the cutover gate, but the next action after proof completion was still
  spread across multiple notes
- that made it too easy to drift back into ad hoc repair instead of deliberate
  refinement and cutover preparation

Result:

- the active program is now explicit:
  - re-freeze the rebased exact-current `MeerkatMachine.tools` seam
  - write the exact-current seam refinement delta
  - build lowering maps for `MeerkatMachine` and `MobMachine`
  - shadow-validate
  - hard-cut `MeerkatMachine`, then `MobMachine`

## Slice 245 - Rebased exact-current tools seam re-frozen and seam refinement delta opened

Goal:

- bring the rebased exact-current `MeerkatMachine.tools` region and the proven
  Mob-Meerkat seam into one aligned pre-cutover story

What landed:

- added:
  - `meerkat-tool-visibility-freeze.md`
  - `mob-meerkat-composition-refinement-delta.md`
- updated:
  - `meerkat-machine-exact-current-freeze.md`
  - `meerkat-cutover-checklist.md`
  - `mob-meerkat-composition-proof-handoff.md`
  - `README.md`

Why this slice matters:

- after the successful rebase, the exact-current branch no longer honestly
  supports the older tools story that only `tool_surface` is frozen while
  richer visibility ownership remains purely upstream
- at the same time, the composition proof package was still leaving the
  implementation refinement gap implicit

Result:

- the rebased exact-current `MeerkatMachine.tools` region is now explicitly
  frozen as:
  - `tool_visibility`
  - `tool_surface`
- the Mob-Meerkat seam now has an explicit refinement-delta note, so the
  remaining gap is clearly classified as implementation lowering work rather
  than target-machine uncertainty

## Slice 246 - Meerkat implementation delta made explicit for cutover work

Goal:

- stop leaving the Meerkat implementation-vs-target gap spread across multiple
  freeze notes and machine commitments

What landed:

- added:
  - `meerkat-machine-refinement-delta.md`
- updated:
  - `two-kernel-refinement-program.md`
  - `README.md`

Why this slice matters:

- the branch now has:
  - a frozen exact-current `MeerkatMachine`
  - a frozen target `MeerkatMachine`
  - a rebased exact-current tools split
- but the remaining implementation-side gap was still implicit across the
  target freeze, exact-current freeze, lowering map, and ownership decisions

Result:

- the remaining Meerkat cutover work is now classified explicitly as:
  - accepted target cleanup
  - required implementation work
  - target promotions not yet lowered
- the refinement program no longer describes the exact-current tools/seam work
  as future tense; it now points to the concrete alignment artifacts already in
  the tree

## Slice 247 - Cutover lowering inventories added for both kernels

Goal:

- stop leaving the next pre-cutover work as “lowering, somehow” and turn it
  into explicit machine-facing inventories for both kernels

What landed:

- added:
  - `meerkat-cutover-lowering-inventory.md`
  - `mob-cutover-lowering-inventory.md`
- updated:
  - `two-kernel-refinement-program.md`
  - `README.md`

Why this slice matters:

- both machines already had frozen lowering maps, but those were still
  comparison handoffs rather than active cutover execution documents
- the next real work needs canonical-ready vs helper-rich vs target-only
  classification, not just a list of code paths

Result:

- `MeerkatMachine` and `MobMachine` now have parallel pre-cutover lowering
  inventories
- the refinement program can now move from freeze alignment into shadow
  validation planning with one explicit source of truth per kernel

## Slice 248 - Shadow-validation plans added for both kernels and the seam

Goal:

- turn the new cutover-lowering inventories into the first concrete validation
  program against the real implementation

What landed:

- added:
  - `meerkat-shadow-validation-plan.md`
  - `mob-shadow-validation-plan.md`
  - `mob-meerkat-seam-shadow-checks.md`
- updated:
  - `two-kernel-refinement-program.md`
  - `README.md`

Why this slice matters:

- the branch already had frozen machines, frozen seam, refinement deltas, and
  cutover lowering inventories
- but there was still no explicit plan for the first short-lived
  shadow-read/shadow-validate pass against the real system

Result:

- `MeerkatMachine`, `MobMachine`, and the Mob↔Meerkat seam now each have an
  explicit pre-cutover shadow-validation plan
- the next active work is implementation of those hooks/checks, not more
  freeze/proof alignment

## Slice 249 - Shadow hook inventories mapped to live code

Goal:

- stop leaving the new shadow-validation plans at the level of “compare these
  concepts somehow” and identify the exact live hook points we can wire first

What landed:

- added:
  - `meerkat-shadow-hook-inventory.md`
  - `mob-shadow-hook-inventory.md`
  - `mob-meerkat-seam-hook-inventory.md`
- updated:
  - `two-kernel-refinement-program.md`
  - `README.md`

Why this slice matters:

- we already had the machine plans
- the next blocker was practical: which live snapshots, diagnostics, and bridge
  helpers actually anchor those plans

Result:

- the next pre-cutover work is now concrete:
  - Meerkat shadow hooks
  - Mob shadow hooks
  - seam shadow hooks
- both machine tracks remain aligned in structure: freeze -> proof -> lowering
  inventory -> shadow plan -> hook inventory

## Slice 250 - First shadow implementation plan fixes host files and output shape

Goal:

- stop leaving the transition from hook inventories to code as an implied step

What landed:

- added:
  - `two-kernel-shadow-implementation-plan.md`
- updated:
  - `two-kernel-refinement-program.md`
  - `README.md`

Why this slice matters:

- we already knew what to validate and which live hooks existed
- the remaining ambiguity was practical: which files should the first patches
  actually touch and what should they emit

Result:

- the first shadow implementation wave now has:
  - concrete host files
  - concrete helper names
  - concrete scenarios
  - one shared mismatch output shape
- the next step can be the first code patch rather than more alignment prose

## Slice 251 - First Meerkat lifecycle/control shadow lane lands in code

Goal:

- move the shadow program from plans/hook inventories into the first actual
  machine helper without inventing a second validator stack

What landed:

- added typed shadow output in `meerkat/src/meerkat_machine.rs`:
  - `MeerkatShadowLane`
  - `ShadowMismatchTriage`
  - `MeerkatShadowMismatch`
  - `MeerkatShadowReport`
- added:
  - `capture_meerkat_shadow_report(...)`
  - lifecycle/control mismatch lifting built directly on
    `validate_meerkat_machine_snapshot(...)`
- added focused tests proving:
  - healthy live runtime-backed sessions report no lifecycle/control mismatches
  - seeded lifecycle/control drift is surfaced with structured mismatch output

Why this slice matters:

- it proves the shadow-validation program can reuse the frozen machine
  validator instead of forking a second interpretation layer
- it turns the first step of the pre-cutover program into a real code path
- it gives the rest of the shadow work one concrete typed output shape to copy

Result:

- the Meerkat lifecycle/control lane is now implemented, not just planned
- the next shadow target is the Meerkat tools lane, followed by the first Mob
  provisioning/lifecycle lane

## Slice 252 - Meerkat tools shadow lane lands in code

Goal:

- extend the first shadow implementation from lifecycle/control into the
  rebased tools seam without inventing tool-specific shadow semantics

What landed:

- extended `meerkat/src/meerkat_machine.rs` with:
  - `MeerkatShadowLane::Tools`
  - tool-surface mismatch lifting built directly on
    `validate_meerkat_machine_snapshot(...)`
- added focused tests proving:
  - healthy live runtime-backed sessions report no tool mismatches
  - seeded tool-surface drift is surfaced as structured shadow mismatches

Why this slice matters:

- it proves the rebased `tool_visibility + tool_surface` seam can use the same
  shadow-report pattern as lifecycle/control
- it turns the first Meerkat shadow wave into two real implemented lanes
- it makes the next pre-cutover implementation target unambiguous: Mob
  provisioning/lifecycle

Result:

- the Meerkat shadow implementation now has:
  - lifecycle/control shadow reporting
  - tools shadow reporting
- the next shadow target is the first Mob provisioning/lifecycle lane

## Slice 253 - First Mob provisioning/lifecycle shadow lane lands in code

Goal:

- move `MobMachine` shadow validation from plans into the first live lane
  without inventing a second Mob interpretation layer

What landed:

- extended `meerkat-mob/src/mob_machine.rs` with:
  - `MobShadowLane`
  - `MobShadowMismatch`
  - `MobShadowReport`
  - `capture_mob_shadow_report(...)`
- added provisioning/lifecycle mismatch lifting built directly on
  `validate_mob_machine_snapshot(...)`
- added focused tests proving:
  - seeded provisioning/lifecycle drift is surfaced as structured mismatch
    output
  - healthy live Mob state reports no provisioning/lifecycle mismatches

Why this slice matters:

- it proves the Mob side can reuse the frozen machine validator the same way
  Meerkat already does
- it turns the first Mob shadow lane into real code instead of a plan
- it fixes the output shape for all later Mob lanes

Result:

- the Mob shadow implementation now has:
  - provisioning/lifecycle shadow reporting
- the next shadow target is the seam lifecycle/supersession lane

## Slice 254 - Seam lifecycle/supersession shadow lane lands in code

Goal:

- land the first bridge-level shadow checks between frozen `MobMachine` and
  frozen `MeerkatMachine` without leaking internal Meerkat mechanics into Mob

What landed:

- extended `meerkat-mob/src/mob_machine.rs` with:
  - `CompositionShadowLane`
  - `CompositionShadowMismatch`
  - `CompositionShadowReport`
  - `capture_composition_shadow_report(...)`
- added lifecycle/supersession mismatch lifting that compares:
  - projected Mob member ownership
  - live session liveness
  - live Meerkat runtime phase
  - current projected bridge session identity
- added focused tests proving:
  - duplicate bridge ownership and terminal Meerkat binding drift are surfaced
    as structured mismatches
  - healthy live Mob↔Meerkat seam state reports no lifecycle/supersession
    mismatches

Why this slice matters:

- it turns the first seam validation lane into real code
- it keeps the seam checks on the frozen bridge abstraction instead of letting
  Mob reason about hidden Meerkat internals
- it makes the next Mob shadow target unambiguous: flow/frame/loop

Result:

- the shadow implementation now includes:
  - Meerkat lifecycle/control
  - Meerkat tools
  - Mob provisioning/lifecycle
  - seam lifecycle/supersession
- the next shadow target is the Mob flow/frame/loop lane

## Slice 255 - Mob flow/frame/loop shadow lane lands in code

Goal:

- extend `MobMachine` shadow validation from provisioning into the tracked
  run/frame/loop surface without creating flow-specific shadow semantics

What landed:

- extended `meerkat-mob/src/mob_machine.rs` with:
  - `MobShadowLane::FlowFrameLoop`
  - tracked-run / flow-tracker / frame-loop mismatch lifting built directly on
    `validate_mob_machine_snapshot(...)`
- added focused tests proving:
  - seeded malformed tracked-run state is surfaced as structured
    flow/frame/loop shadow mismatches
  - healthy live single-step flow state reports no flow/frame/loop shadow
    mismatches
- reran the full `meerkat-mob --lib` lane successfully after landing the new
  shadow lane

Why this slice matters:

- it proves the Mob flow surface can use the same frozen-machine shadow pattern
  as provisioning/lifecycle
- it lands the first live Mob flow-aware shadow lane
- it leaves the next pre-cutover target clear: the first Meerkat
  turn/ops/barrier shadow lane, followed by peer/drain and seam work bridge

Result:

- the implemented shadow lanes are now:
  - Meerkat lifecycle/control
  - Meerkat tools
  - Mob provisioning/lifecycle
  - Mob flow/frame/loop
  - seam lifecycle/supersession
- the next shadow target is the Meerkat turn/ops/barrier lane

## Slice 256 - Meerkat turn/ops/barrier shadow lane lands in code

Goal:

- extend `MeerkatMachine` shadow validation from lifecycle/tools into the
  frozen turn / ops / barrier seam without inventing a second turn model

What landed:

- extended `meerkat/src/meerkat_machine.rs` with:
  - `MeerkatShadowLane::TurnOpsBarrier`
  - turn / ops / wait-all / barrier mismatch lifting built directly on
    `validate_meerkat_machine_snapshot(...)`
- added focused tests proving:
  - healthy live runtime-backed sessions emit no turn/ops/barrier shadow
    mismatches
  - seeded malformed turn/ops/barrier state is surfaced as structured shadow
    mismatches
- reran the full `meerkat --lib` lane successfully after landing the new
  shadow lane

Why this slice matters:

- it lands the first live Meerkat execution-aware shadow lane beyond lifecycle
  and tools
- it keeps shadow reporting machine-led by reusing the frozen
  `MeerkatMachine` validator rather than inventing parallel turn semantics
- it leaves the next pre-cutover targets clear: Meerkat peer/drain, Mob
  task/history/recovery, and the seam work-bridge lane

Result:

- the implemented shadow lanes are now:
  - Meerkat lifecycle/control
  - Meerkat tools
  - Meerkat turn/ops/barrier
  - Mob provisioning/lifecycle
  - Mob flow/frame/loop
  - seam lifecycle/supersession
- the next shadow targets are:
  - Meerkat peer/drain
  - Mob task/history/recovery
  - seam work bridge

## Slice 257 - Meerkat peer/drain shadow lane lands in code

Goal:

- extend `MeerkatMachine` shadow validation into peer-ingress and comms-drain
  state without inventing a second transport/drain model

What landed:

- extended `meerkat/src/meerkat_machine.rs` with:
  - `MeerkatShadowLane::PeerDrain`
  - peer authority / peer queue / drain mismatch lifting built directly on
    `validate_meerkat_machine_snapshot(...)`
- added focused tests proving:
  - healthy live runtime-backed sessions emit no peer/drain shadow mismatches
  - seeded malformed peer/drain state is surfaced as structured shadow
    mismatches
- reran the full `meerkat --lib` lane successfully after landing the new
  shadow lane

Why this slice matters:

- it completes the first Meerkat shadow sweep across lifecycle, tools,
  execution, peer ingress, and drain
- it keeps the peer/drain lane machine-led by reusing frozen invariant truth
  instead of inventing transport-specific shadow semantics
- it leaves the next pre-cutover targets clear: Mob task/history/recovery and
  the seam work-bridge lane

Result:

- the implemented shadow lanes are now:
  - Meerkat lifecycle/control
  - Meerkat tools
  - Meerkat turn/ops/barrier
  - Meerkat peer/drain
  - Mob provisioning/lifecycle
  - Mob flow/frame/loop
  - seam lifecycle/supersession
- the next shadow targets are:
  - Mob task/history/recovery
  - seam work bridge

## Slice 258 - Mob task/history/recovery shadow lane lands in code

Goal:

- extend `MobMachine` shadow validation into task-ledger, restore-failure, and
  history/finality state without inventing a second recovery model

What landed:

- extended `meerkat-mob/src/mob_machine.rs` with:
  - `MobShadowLane::TaskHistoryRecovery`
  - task/history/recovery mismatch lifting built directly on frozen
    `MobMachine` projection truth
- added focused tests proving:
  - healthy live mob state emits no task/history/recovery shadow mismatches
  - seeded malformed restore/task/history state is surfaced as structured
    shadow mismatches
- reran the full `meerkat-mob --lib` lane successfully after landing the new
  shadow lane

Why this slice matters:

- it completes the first Mob shadow sweep across provisioning/lifecycle,
  flow/frame/loop, and task/history/recovery
- it keeps the lane machine-led by reusing existing durable projection truth
  instead of inventing actor-local recovery semantics
- it leaves the next pre-cutover target clear: the seam work-bridge lane

Result:

- the implemented shadow lanes are now:
  - Meerkat lifecycle/control
  - Meerkat tools
  - Meerkat turn/ops/barrier
  - Meerkat peer/drain
  - Mob provisioning/lifecycle
  - Mob flow/frame/loop
  - Mob task/history/recovery
  - seam lifecycle/supersession
- the next shadow target is:
  - seam work bridge

## Slice 259 - Seam work-bridge shadow lane lands in code

Goal:

- extend the frozen Mob↔Meerkat seam shadowing beyond lifecycle/supersession
  into a narrow, honest bridge-work check

What landed:

- extended `meerkat-mob/src/mob_machine.rs` with:
  - `CompositionShadowLane::WorkBridge`
  - work-bridge mismatch lifting based on observable Meerkat work posture
    rather than invented run-to-member coupling
- added focused tests proving:
  - healthy live seam state emits no work-bridge mismatches
  - seeded nonterminal Mob work without any observable Meerkat work posture is
    surfaced as a structured seam mismatch
- reran the full `meerkat-mob --lib` lane successfully after landing the new
  seam lane

Why this slice matters:

- it completes the first seam shadow sweep across:
  - lifecycle/supersession
  - work bridge
- it keeps the seam lane honest by checking only data the rebased branch
  already projects cleanly
- it moves the next pre-cutover step from “author more lanes” to “collect and
  triage shadow reports from real scenarios”

Result:

- the implemented shadow lanes are now:
  - Meerkat lifecycle/control
  - Meerkat tools
  - Meerkat turn/ops/barrier
  - Meerkat peer/drain
  - Mob provisioning/lifecycle
  - Mob flow/frame/loop
  - Mob task/history/recovery
  - seam lifecycle/supersession
  - seam work bridge
- the next shadow target is:
  - a shared shadow-report sink / scenario runner

## Slice 260 - Aggregate shadow suite helpers land in code

Goal:

- stop treating the shadow lanes as isolated helper surfaces and make them
  usable as one real pre-cutover scenario report

What landed:

- extended `meerkat/src/meerkat_machine.rs` with:
  - `MeerkatShadowSuiteReport`
  - `capture_all_meerkat_shadow_reports(...)`
- extended `meerkat-mob/src/mob_machine.rs` with:
  - `MobShadowSuiteReport`
  - `capture_mob_shadow_suite_report(...)`
- added focused tests proving:
  - healthy live `MeerkatMachine` sessions emit no aggregate mismatches across
    all landed Meerkat lanes
  - healthy live `MobMachine + Meerkat seam` scenarios emit no aggregate
    mismatches across all landed Mob and seam lanes
- reran full `meerkat --lib` and `meerkat-mob --lib` lanes successfully after
  landing the aggregate helpers

Why this slice matters:

- it turns the shadow plan from “many lane helpers” into a real scenario-level
  collection path
- it keeps the first aggregate step honest by avoiding a fake all-in-one
  backdoor from `MobHandle` into joined `MeerkatMachine` snapshots
- it leaves the next refinement target clear: use the aggregate helpers in real
  pre-cutover shadow runs, then add a shared sink/export path only if the live
  mismatch volume justifies it

Result:

- the implemented shadow surfaces are now:
  - `capture_all_meerkat_shadow_reports(...)`
  - `capture_mob_shadow_suite_report(...)`
  - all previously landed lane-level helpers
- the next shadow target is:
  - first real end-to-end shadow scenario run using the aggregate suite helpers

## Slice 276 - First live seam mismatch taxonomy lands

Goal:

- move beyond seeded synthetic taxonomy drift and prove that the aggregate
  shadow taxonomy surfaces a real mismatch class from live implementation
  contact

What landed:

- added `test_capture_mob_shadow_suite_taxonomy_reports_live_bridge_loss_drift`
  in `meerkat-mob/src/runtime/tests.rs`
- the scenario:
  - provisions a real runtime-adapter-backed member
  - starts a real single-step flow and waits for observable Meerkat work
    posture
  - archives the live bridge session through the mock session service while the
    flow is still nonterminal
  - captures the aggregate Mob + seam shadow suite and collapses it into
    taxonomy buckets
- the live taxonomy now proves two real seam mismatch classes:
  - `composition / LifecycleSupersession / lifecycle`
  - `composition / WorkBridge / work`
- with the currently observed live triage:
  - `composition / LifecycleSupersession / lifecycle / implementation_detail`
  - `composition / WorkBridge / work / semantic_gap`

Why this slice matters:

- it is the first mismatch-producing **live** shadow run instead of a seeded
  synthetic mutation
- it proves the aggregate taxonomy is useful on real implementation drift, not
  just empty on green paths or shaped correctly on seeded snapshots
- it moves the branch from “shadow taxonomy is validated” to “shadow taxonomy
  is now collecting real refinement signals”

Validation:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_taxonomy_reports_live_bridge_loss_drift`
- `git diff --check`

## Slice 277 - Shared shadow sink schema lands after first live mismatch

Goal:

- stop treating mismatch export as a vague future step once the first live
  taxonomy class has appeared

What landed:

- added `two-kernel-shadow-sink-schema.md`
- the schema defines one shared pre-cutover export unit for:
  - `MeerkatMachine` taxonomy buckets
  - `MobMachine` taxonomy buckets
  - seam taxonomy buckets
- the first required live sample is now frozen explicitly:
  - `seam.live_bridge_loss`
  - `composition / LifecycleSupersession / lifecycle`
  - `composition / WorkBridge / work`
- and the currently observed live triage is now recorded explicitly in the
  schema:
  - `LifecycleSupersession / lifecycle / implementation_detail`
  - `WorkBridge / work / semantic_gap`

Why this slice matters:

- it turns the “shared sink/export path” from a deferred idea into an explicit
  next implementation target
- it keeps the sink taxonomy-oriented instead of prematurely exporting raw
  machine snapshots
- it gives the cutover program one stable shape to build against now that live
  mismatch classes exist

Validation:

- `git diff --check`

## Slice 278 - Both kernels can emit the first sink sample shape

Goal:

- stop treating the shared sink schema as Mob-only once the shape exists

What landed:

- added `export_meerkat_shadow_scenario_sample(...)` in
  `meerkat/src/meerkat_machine.rs`
- validated it on:
  - the broader green Meerkat smoke taxonomy (empty sample)
  - the seeded lifecycle/control drift taxonomy (three explicit Meerkat
    buckets)
- kept the existing Mob-side export consumer as the first live seam-mismatch
  sample path

Why this slice matters:

- both kernels can now emit the same scenario-sample export shape
- the branch is no longer blocked on “make the sink symmetric first” before
  writing a shared cutover-facing runner
- the next honest step is the first combined scenario collector, not more
  export-shape work

Validation:

- `cargo test -p meerkat --lib summarize_meerkat_shadow_taxonomy_reports_collapses_seeded_lifecycle_control_drift`
- `cargo test -p meerkat --lib capture_meerkat_shadow_taxonomy_stays_empty_across_broader_smoke_run`
- `git diff --check`

## Slice 279 - First combined cutover-facing shadow runner lands

Goal:

- move from separate Meerkat and Mob exporters to the first paired scenario
  run that emits both sides of the same cutover-facing scenario

What landed:

- added a narrow public Meerkat diagnostic export helper in
  `meerkat/src/meerkat_machine.rs` that works from:
  - `MeerkatMachineSpineSnapshot`
  - best-effort live execution/tool/peer projections
- re-exported that helper from `meerkat/src/lib.rs`
- extended
  `test_capture_mob_shadow_suite_taxonomy_reports_live_bridge_loss_drift`
  in `meerkat-mob/src/runtime/tests.rs` so the same live scenario now emits:
  - active Meerkat sample
  - active Mob sample
  - post-archive Meerkat sample
  - post-archive Mob sample

Important backtrack:

- the first version assumed the post-archive Meerkat sample would disappear
- that was false on the rebased baseline
- the honest paired result is now:
  - active phase: empty Meerkat sample + empty Mob sample
  - `post_archive` phase:
    - empty Meerkat sample
    - non-empty Mob/seam sample carrying the real bridge-loss taxonomy

Why this slice matters:

- it is the first actual combined cutover-facing runner, not just two
  exporters that happen to share a shape
- it proves the shared sink/export path can represent asymmetric machine
  behavior honestly without inventing a stronger seam story
- it gives the pre-cutover program one real paired sample to build durable sink
  consumers around

Validation:

- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 280 - Reusable two-kernel shadow batch consumer lands

Goal:

- move the first combined cutover-facing runner from "paired ad hoc exports"
  to a reusable batch/sink shape

What landed:

- added `merge_two_kernel_shadow_scenario_samples(...)` in
  `meerkat-mob/src/mob_machine.rs`
- added `export_two_kernel_shadow_batch(...)` in
  `meerkat-mob/src/mob_machine.rs`
- updated the live `seam.live_bridge_loss` scenario in
  `meerkat-mob/src/runtime/tests.rs` so it now exports:
  - one normalized `active` sample
  - one normalized `post_archive` sample
  - one reusable batch with:
    - `run_id = "seam.live_bridge_loss"`
    - ordered samples `[active, post_archive]`

Important backtrack:

- the merge helper could not reuse the Meerkat sink bucket type directly
- the honest fix was an explicit field-by-field conversion from the Meerkat
  bucket shape into the Mob-side sink bucket shape

Why this slice matters:

- the sink/export path is now reusable across more than one scenario phase
- the branch has moved from "first combined runner exists" to
  "first reusable cutover-facing batch consumer exists"
- this is the right base for a future runner that collects multiple scenarios
  in one shadow session

Validation:

- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_taxonomy_reports_live_bridge_loss_drift`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 281 - Multi-scenario cutover-facing shadow run batch lands

Goal:

- move the shadow sink from one reusable scenario batch to the first reusable
  cutover-facing run that carries both a green scenario and a
  mismatch-producing scenario under one run id

What landed:

- added `TwoKernelShadowRunBatch` in `meerkat-mob/src/mob_machine.rs`
- added `export_two_kernel_shadow_run_batch(...)` in
  `meerkat-mob/src/mob_machine.rs`
- added
  `test_export_two_kernel_shadow_run_batch_collects_green_and_drift_scenarios`
  in `meerkat-mob/src/runtime/tests.rs`
- the new run batch now exports:
  - `run_id = "shadow.cutover.smoke"`
  - scenario batch `0`:
    - `mob.flow.single_step.green`
    - one empty `active` sample
  - scenario batch `1`:
    - `seam.live_bridge_loss`
    - one non-empty `post_archive` sample

Important backtrack:

- the first version considered reusing the same scenario id for both the green
  and drift phases
- the landed version keeps them as distinct scenario batches so the sink stays
  faithful to the scenario matrix instead of flattening unrelated meanings into
  one synthetic scenario

Why this slice matters:

- the cutover-facing sink can now represent one run session with:
  - healthy machine/seam posture
  - real live drift
- this is the first shape that starts looking like a real shadow-run artifact
  instead of a one-test export helper

Validation:

- `cargo test -p meerkat-mob --lib test_export_two_kernel_shadow_run_batch_collects_green_and_drift_scenarios`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 313 - Owner-bridge semantics reach the deeper Mob spawn/provision path

Goal:

- carry the identity-first naming cleanup from the public/runtime bridge into
  the deeper Mob spawn/provision/finalize path so internal ownership carriers
  stop calling bridge bindings `owner_session_id`

What landed:

- renamed the internal canonical owner carrier in:
  - `meerkat-mob/src/runtime/handle.rs`
    - `CanonicalOpsOwnerContext.owner_bridge_session_id`
  - `meerkat-mob/src/runtime/state.rs`
    - `MobCommand::Spawn.owner_bridge_session_id`
  - `meerkat-mob/src/runtime/provisioner.rs`
    - `ProvisionMemberRequest.owner_bridge_session_id`
    - `bind_member_owner_context(..., owner_bridge_session_id, ...)`
  - `meerkat-mob/src/runtime/actor.rs`
    - `PendingSpawn.owner_bridge_session_id`
    - `enqueue_spawn(..., owner_bridge_session_id, ...)`
    - attached/auto-wire/finalize locals and receipts
- updated the remaining `CanonicalOpsOwnerContext` construction sites in:
  - `meerkat-mob/src/runtime/tools.rs`
  - `meerkat-mob/src/runtime/tests.rs`
- cleaned the last runtime-local mock naming straggler in
  `meerkat-mob/src/runtime/provision_guard.rs`

Important backtrack:

- I did not rename durable/public compatibility surfaces like
  `OperationSpec.owner_session_id` or mob definition metadata in this slice
- those remain compatibility fields because they cross older persistence /
  protocol boundaries; this slice was intentionally limited to the internal
  runtime carrier path

Why this slice matters:

- the clean-cut regime is now more internally honest: the deeper Mob runtime
  path names the bridge binding for what it is
- this reduces the remaining semantic confusion between identity ownership and
  bridge-session ownership before the next behavior adaptations

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test -p meerkat-rpc --lib --quiet`

## Slice 311 - Bridge-session semantics reach Mob lifecycle projection and spawn surfaces

Goal:

- keep pushing the clean-cut / identity-first adaptation by making the Mob
  runtime projection speak bridge-session semantics internally instead of
  treating `current_session_id` as the canonical machine vocabulary

What landed:

- renamed the internal lifecycle-authority material/input fields from
  `current_session_id` to `current_bridge_session_id` in
  `meerkat-mob/src/runtime/mob_member_lifecycle_authority.rs`
- updated `MobHandle` lifecycle materialization in
  `meerkat-mob/src/runtime/handle.rs` to feed and consume the renamed
  bridge-session fields
- added `MobMemberListEntry::current_bridge_session_id()` in
  `meerkat-mob/src/runtime/handle.rs`
- rewired internal `MobMachine` validation/composition reads in
  `meerkat-mob/src/mob_machine.rs` to use bridge-session accessors where that
  is the actual meaning
- updated the MCP and RPC spawn surfaces in:
  - `meerkat-mob-mcp/src/public_mcp.rs`
  - `meerkat-rpc/src/handlers/mob.rs`
  so stable JSON field `session_id` is now explicitly sourced from
  `bridge_session_id()`
- updated the Mob operator tool surface in `meerkat-mob/src/runtime/tools.rs`
  so stable JSON fields `session_id` and `current_session_id` are populated
  from `bridge_session_id()` / `current_bridge_session_id()`

Important backtrack:

- the first rename pass missed the respawn helper path in `MobHandle`
- the honest fix was to update that runtime convenience layer too rather than
  sneaking the old field name back into the canonical lifecycle authority

Why this slice matters:

- the branch now treats session IDs more consistently as bridge bindings in
  internal runtime and machine code
- public/wire field names remain stable for compatibility, but the canonical
  internal meaning is less muddled
- the first broad cutover failure was already behind us; this slice proves we
  can keep adapting identity-first semantics without destabilizing the clean-cut
  regime

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test -p meerkat-rpc --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 312 - Owner-bridge semantics reach the Mob runtime tool and ops bridge

Goal:

- keep the identity-first cut moving by removing one more place where the Mob
- runtime still used `owner_session_id` as the canonical internal name for a
- bridge binding

What landed:

- renamed the internal ops-adapter binding seam in
  `meerkat-mob/src/runtime/ops_adapter.rs` from `owner_session_id` to
  `owner_bridge_session_id`
- renamed the internal Mob operator tool dispatcher seam in
  `meerkat-mob/src/runtime/tools.rs` from `owner_session_id` to
  `owner_bridge_session_id`
- kept the downstream `OperationSpec.owner_session_id` field and existing
  external wire names stable, so this is a semantic/runtime cleanup rather than
  a protocol migration
- updated the Mob operator tool JSON projection in
  `meerkat-mob/src/runtime/tools.rs` to source stable `session_id` and
  `current_session_id` fields from the canonical bridge-session accessors

Important backtrack:

- this slice only renames the canonical internal runtime seam
- it does not pretend the broader ops/protocol surface has already been renamed;
  those fields remain compatibility carriers for now

Why this slice matters:

- the Mob runtime tool/ops bridge now matches the bridge-session semantics we
  already pushed into the lifecycle material, machine validator, and RPC/MCP
  spawn surfaces
- that reduces one more source of “session as identity” confusion while keeping
  the broad cut stable

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## 2026-04-13 — Slice 283: Mob MCP bridge-session helpers replace session-as-identity internals

Goal:

- keep the clean-cut / identity-first push moving by fixing the Mob MCP layer
  where internal helper code still treated bridge session bindings as if they
  were semantic member identity

What landed:

- `meerkat-mob-mcp/src/agent_tools.rs`
  - `AgentMobToolSurface` now stores `owner_bridge_session_id`
  - delegate wiring and spawn results now read `MemberRef::bridge_session_id()`
    internally while keeping external JSON fields named `session_id`
- `meerkat-mob-mcp/src/lib.rs`
  - added canonical helpers:
    - `retire_member_by_bridge_session_id(...)`
    - `owns_live_bridge_session(...)`
    - `owns_persisted_bridge_session(...)`
  - kept `*_session_*` entrypoints as compatibility wrappers
  - switched live roster and persisted-membership checks to
    `MemberRef::bridge_session_id()`
  - switched `mob_append_system_context(...)` to resolve bridge bindings
    explicitly
- `meerkat-rpc/src/router.rs`
  - session-owner resolution and archive delegation now use the canonical
    bridge-session helpers

Important backtrack:

- I did not rename the external payload field `session_id` or the durable
  `owner_session_id` metadata key in this slice
- the honest cut is to fix runtime semantics first and preserve compatibility
  at the outer API boundary until we deliberately choose a protocol/storage
  migration

Why this slice matters:

- the Mob MCP runtime/control path is now aligned with the identity-first story
  we already established in Mob runtime hot paths
- the first broad cutover failure (`e2e_system_rest_resume_metadata`) was a
  direct stale-field regression from this adaptation and is now fixed

Validation:

- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test -p meerkat-rest --features integration-real-tests --test system_rest_resume integration_real_rest_resume_metadata -- --ignored --nocapture`
- `cargo test -p meerkat-rpc --lib --quiet`
- `cargo check --workspace --all-targets --quiet`

## 2026-04-13 — Slice 284: bridge-session vocabulary reaches Mob handle snapshots and the broad cut stays green

Goal:

- keep moving the identity-first cut from hot runtime seams into the public-ish
  Mob handle/snapshot layer without forcing an external wire break

What landed:

- `meerkat-mob/src/runtime/handle.rs`
  - added bridge-session accessors on:
    - `MobMemberSnapshot`
    - `MemberRespawnReceipt`
    - `MemberDeliveryReceipt`
    - `MemberSessionRef`
  - added `MobMemberHandle::current_bridge_session_id(...)`
  - kept existing `current_session_id(...)` / public fields as compatibility
    surfaces

Important backtrack:

- I did not rename the serialized/public fields like `current_session_id`,
  `old_session_id`, `new_session_id`, or `session_id` in this slice
- the honest cut is to move the runtime/control vocabulary first and preserve
  the outer compatibility shape until we deliberately choose a protocol break

Why this slice matters:

- the canonical meaning is now explicit all the way from:
  - `MemberRef`
  - roster projection
  - Mob MCP state
  - RPC owner resolution
  - Mob handle snapshots / receipts
- the full clean-cut regime now survives the broad workspace test lane after
  the identity-first bridge-session adaptation, not just the focused crates

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 308 - Meerkat session query and binding reads join the session command seam

Goal:

- remove the remaining obvious public `RuntimeSessionAdapter` read helpers that
  still reached directly into shared session maps

What landed:

- added richer `MeerkatMachineSessionCommand` query variants in
  `meerkat-runtime/src/meerkat_machine.rs`:
  - `ContainsSession`
  - `SessionHasExecutor`
  - `SessionHasComms`
  - `OpsLifecycleRegistry`
  - `PrepareBindings`
- added `MeerkatMachineSessionCommandResult` in the same file so those commands
  can return typed data instead of only `()`
- rewired the corresponding public adapter helpers in
  `meerkat-runtime/src/session_adapter.rs` so they now route through
  `execute_meerkat_machine_session_command(...)`

Important backtrack:

- `PrepareBindings` still needs to return the canonical binding payload after
  registration, so the honest cut was to centralize the registration + binding
  lookup seam inside the machine command executor rather than pretending the
  payload itself lives in the enum

Why this slice matters:

- the last obvious public Meerkat session/binding reads no longer freehand
  shared-map access in `RuntimeSessionAdapter`
- the public adapter surface is now more consistently machine-routed for both
  mutation and read-side lifecycle/session queries

Validation:

- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 309 - Canonical Mob member projection remains a lock-free read seam

Goal:

- continue the full cut without sacrificing the `Retiring` visibility window
  that the cutover tests depend on

What landed:

- attempted actor-side routing for `ListMembers`, `ListMembersIncludingRetiring`,
  and `MemberStatus`
- backed that port out after it proved too strong for current Mob lifecycle
  semantics
- restored canonical member material projection on `MobHandle` for:
  - list projections
  - deep member status
  - helper completion snapshots
  - terminal wait classification

Important backtrack:

- putting canonical member projection behind the actor queue caused
  `Retiring` members to disappear behind in-flight disposal work
- the honest shape for the clean-cut regime is:
  - machine-routed event / diagnostic / mutation surfaces
  - lock-free canonical member projection from shared roster + live session
    observation

Why this slice matters:

- the brutal cut stays green without lying about lifecycle observability
- we now have a clearer boundary between:
  - public old-regime bypasses that should die
  - canonical read seams that must stay immediate to preserve truth during
    retirement and supersession windows

Validation:

- `cargo test -p meerkat-mob --lib test_structural_roster_reads_round_trips_through_machine_command_surface --quiet`
- `cargo test -p meerkat-mob --lib test_member_status_round_trips_through_machine_command_surface --quiet`
- `cargo test -p meerkat-mob --lib test_retiring_member_is_not_routable_before_disposal_completes --quiet`
- `cargo test -p meerkat-mob --lib test_wait_one_observes_retiring_member_as_non_terminal_until_archive --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 310 - Meerkat input-state reads join the session command seam

Goal:

- remove the remaining obvious public `RuntimeSessionAdapter` input-state reads
  that still reached directly into runtime drivers

What landed:

- added `InputState` and `ListActiveInputs` to
  `MeerkatMachineSessionCommand` in
  `meerkat-runtime/src/meerkat_machine.rs`
- added matching typed results to
  `MeerkatMachineSessionCommandResult`
- rewired `SessionServiceRuntimeExt::input_state(...)` and
  `SessionServiceRuntimeExt::list_active_inputs(...)` in
  `meerkat-runtime/src/session_adapter.rs` so they now route through
  `execute_meerkat_machine_session_command(...)`
- widened the existing session-service-runtime-ext cutover test so it now
  proves queued input visibility still works through the machine-routed
  read seam

Important backtrack:

- the machine seam returns cloned `InputState` snapshots rather than exposing a
  borrowed driver view; that keeps the public read path typed and honest
  without pretending the runtime driver itself belongs in the command result

Why this slice matters:

- the public session-runtime extension surface is now more consistently
  machine-routed for lifecycle writes and input-state reads together
- this is a better stopping point than forcing another doubtful cut, because
  most of the remaining read seams are now either intentional lock-free truth
  boundaries or runtime-adaptation work for the identity-first regime

Validation:

- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 311 - Mob runtime hot path switches to explicit bridge-session vocabulary

Goal:

- make the clean-cut Mob runtime speak honestly about `SessionId` as bridge
  binding metadata instead of quietly treating it like semantic member
  identity

What landed:

- added `MemberRef::from_bridge_session_id(...)` and
  `MemberRef::bridge_session_id(...)` in `meerkat-mob/src/event.rs`
- added `Roster::set_bridge_session_id(...)`,
  `Roster::bridge_session_id(...)`, and
  `RosterEntry::bridge_session_id(...)` in
  `meerkat-mob/src/roster.rs`
- added `RosterAuthority::set_bridge_session_id(...)` in
  `meerkat-mob/src/runtime/roster_authority.rs`
- rewired the hot runtime callers to use the bridge-session helpers in:
  - `meerkat-mob/src/runtime/builder.rs`
  - `meerkat-mob/src/runtime/actor.rs`
  - `meerkat-mob/src/runtime/provisioner.rs`
  - `meerkat-mob/src/runtime/handle.rs`
  - `meerkat-mob/src/mob_machine.rs`
- tightened the roster tests so the explicit bridge-session helpers are now
  asserted directly
- updated `MemberLaunchMode::Resume` docs in
  `meerkat-mob/src/launch.rs` to describe resuming a bridge session rather
  than implying session identity is the semantic member key

Important backtrack:

- I did not try to rename the wire format or flatten every public
  `session_id`-named API in one pass
- the landed slice keeps compatibility wrappers where they still help, but
  moves the canonical runtime path onto bridge-session naming so the
  identity-first regime is explicit where the actor, builder, provisioner,
  and diagnostics actually reason about live members

Why this slice matters:

- it shifts the clean-cut runtime toward the identity-first Mob model without
  destabilizing the post-cut surface
- it also gives us a sharper line between:
  - semantic member identity (`MeerkatId` / roster member)
  - local runtime bridge binding (`SessionId`)

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 312 - Mob owner-session indexing helpers join the identity-first cleanup story

Goal:

- stop the internal mob-definition layer from talking as if
  `owner_session_id` were semantic member ownership when it is really
  owner-session indexing plus cleanup policy

What landed:

- added explicit owner-session indexing helpers in
  `meerkat-mob/src/definition.rs`:
  - `has_owner_session_index(...)`
  - `is_indexed_to_owner_session(...)`
  - `is_cleanup_scoped_to_owner_session(...)`
  - `mark_owner_session_indexed(...)`
- kept the older helper names as compatibility wrappers instead of forcing a
  wire-format or broad API break in one pass
- rewired the internal creation / cleanup call sites to use the new
  indexing-scoped names in:
  - `meerkat-mob/src/definition.rs`
  - `meerkat-mob-mcp/src/agent_tools.rs`
  - `meerkat-mob-mcp/src/lib.rs`

Important backtrack:

- I did not rename the durable field itself or every public `owner_session_id`
  reference in one shot
- the landed slice keeps storage and compatibility stable while moving the
  canonical runtime/control paths onto the more honest “owner-session index”
  language

Why this slice matters:

- it complements Slice 311 by making the two identity-first distinctions
  explicit in code:
  - member identity is roster/member based
  - session ids are bridge or indexing metadata
- it also makes cleanup eligibility read more honestly, which matters now that
  we are in the clean-cut regime and runtime adaptation is the active frontier

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 313 - Mob ops/tool/turn helpers finish the bridge-session vocabulary move

Goal:

- finish the low-risk identity-first adaptation in the helper paths that still
  steer live work but were reading `MemberRef::session_id()` directly

What landed:

- rewired the remaining helper-level session bridge reads to use
  `MemberRef::bridge_session_id(...)` in:
  - `meerkat-mob/src/runtime/ops_adapter.rs`
  - `meerkat-mob/src/runtime/actor_turn_executor.rs`
  - `meerkat-mob/src/runtime/tools.rs`
- this covers:
  - ops lifecycle binding lookups
  - autonomous flow dispatch bridge resolution
  - tool-call JSON result/reporting surfaces that expose current bridge
    session ids

Important backtrack:

- I did not try to rename every public `current_session_id` field or the
  `Resume { session_id }` wire shape here
- the landed slice is intentionally about the canonical helper/runtime paths,
  not a repo-wide naming churn pass

Why this slice matters:

- the identity-first move now reaches the paths that actually launch turns,
  bind ops lifecycle, and surface bridge metadata back to tools
- that makes the runtime behavior read more honestly without destabilizing the
  broader public API and persisted formats in the middle of the clean-cut push

Validation:

- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 305 - Mob event ledger joins the actor seam with destroyed-state replay fallback

Goal:

- finish culling the remaining raw `MobEventStore` touches from `MobHandle`
  without breaking the terminal destroy contract

What landed:

- moved `PollEvents`, `ReplayAllEvents`, and
  `RecordOperatorActionProvenance` through `MobCommand` in:
  - `meerkat-mob/src/runtime/state.rs`
  - `meerkat-mob/src/runtime/actor.rs`
  - `meerkat-mob/src/runtime/handle.rs`
- removed the live handle-side event-store mutation/read path
- kept an explicit destroyed-state-only replay/poll fallback in `MobHandle`
  so post-destroy reads still resolve against the canonical persisted ledger
- updated `MobHandle` docs to match the real post-cut contract

Important backtrack:

- the first full actor-only cut broke `test_destroy_deletes_storage`
- that was a real contract regression, not a test bug: after `Destroy`, the
  actor exits by design but terminal event replay still needs to work
- the landed shape keeps live event authority in the actor while using the
  persisted event store only as a terminal read-only fallback

Why this slice matters:

- public and internal event operations no longer split authority during the
  live regime
- the remaining handle-side event ledger access is now explicit, narrow, and
  terminal-only instead of being an accidental old-regime bypass

Validation:

- `cargo test -p meerkat-mob --lib test_mob_events_view_round_trips_through_machine_command_surface --quiet`
- `cargo test -p meerkat-mob --lib test_record_operator_action_provenance_round_trips_through_machine_command_surface --quiet`
- `cargo test -p meerkat-mob --lib test_destroy_deletes_storage --quiet`
- `cargo test -p meerkat-mob --lib --quiet`

## Slice 306 - Mob agent event subscriptions join the actor seam

Goal:

- remove the remaining direct `session_service` subscription construction from
  `MobHandle`

What landed:

- added `SubscribeAgentEvents` / `SubscribeAllAgentEvents` to `MobCommand` in
  `meerkat-mob/src/runtime/state.rs`
- moved agent event stream construction into `MobActor` in
  `meerkat-mob/src/runtime/actor.rs`
- rewired `MobMachineCommand::SubscribeAgentEvents` and
  `MobMachineCommand::SubscribeAllAgentEvents` in
  `meerkat-mob/src/runtime/handle.rs` to use `send_actor_command(...)`

Important backtrack:

- the first actor-side port tried to reuse richer handle-only projection
  helpers that do not exist on `MobActor`
- the landed version uses the honest actor-owned surface:
  canonical `member_ref.session_id()` from roster state

Why this slice matters:

- agent event subscription creation now follows the same authority path as the
  rest of the public Mob event surface
- `MobHandle` no longer constructs live agent streams directly from the
  session service

Validation:

- `cargo test -p meerkat-mob --lib test_agent_event_subscriptions_resolve_through_machine_routed_member_reads --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 307 - Mob diagnostic session-service reads join the actor seam

Goal:

- remove the remaining direct live-session diagnostic reach-throughs from
  `MobHandle`

What landed:

- added actor commands for:
  - `DiagnosticRuntimeAdapter`
  - `DiagnosticHasLiveSession`
  - `DiagnosticMeerkatShadowInputs`
  in `meerkat-mob/src/runtime/state.rs`
- handled those commands inside `MobActor` in
  `meerkat-mob/src/runtime/actor.rs`
- rewired the corresponding `MobMachineCommand` cases in
  `meerkat-mob/src/runtime/handle.rs` to use `send_actor_command(...)`

Important backtrack:

- the first attempt to move agent-event subscriptions through the actor tried
  to reuse handle-only canonical projection helpers that do not exist on
  `MobActor`
- the landed diagnostic slice keeps the actor side honest: it uses the actor's
  own `runtime_adapter` and `session_service`, not a second hidden handle-level
  fallback

Why this slice matters:

- `MobHandle` is no longer the quiet place that knows how to reach directly
  into live session internals for diagnostics
- the machine/actor seam now owns event, subscription, and diagnostic session
  reads together

Validation:

- `cargo test -p meerkat-mob --lib test_diagnostic_meerkat_shadow_inputs_returns_best_effort_live_member_state --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `git diff --check`

## Slice 302 - Mob-wide event router subscription joins the machine seam

Goal:

- remove the last obvious public Mob event-surface bypass by routing
  `subscribe_mob_events(...)` and `subscribe_mob_events_with_config(...)`
  through `MobMachineCommand`

What landed:

- added `SubscribeMobEvents { config }` to `MobMachineCommand` in
  `meerkat-mob/src/mob_machine.rs`
- added `MobEventRouter(...)` to `MobMachineCommandResult` in
  `meerkat-mob/src/mob_machine.rs`
- rewired `MobHandle::subscribe_mob_events(...)` and
  `MobHandle::subscribe_mob_events_with_config(...)` in
  `meerkat-mob/src/runtime/handle.rs` so they now round-trip through
  `execute_machine_command(...)`
- updated the direct Rust call sites that consume the public `MobHandle`
  surface:
  - `meerkat-rpc/src/router.rs`
  - `meerkat-mob-mcp/src/lib.rs`
  - `meerkat-mob/src/runtime/tests.rs`

Important backtrack:

- this cut required an honest API change: the public mob-wide event router
  subscription helpers are now `async`
- I checked the repo-wide Rust call sites before doing that; the in-repo users
  were already in async contexts, so paying the API break was cleaner than
  preserving the last public bypass behind another local helper

Why this slice matters:

- the public Mob event surface is now machine-routed for:
  - per-member subscriptions
  - mob-wide router subscriptions
  - polling/replay
  - operator-action audit append
- the remaining raw event-store handle is now an internal router/runtime
  implementation detail rather than a public escape hatch

Validation:

- `cargo test -p meerkat-mob --lib test_mob_event_router_stays_alive_across_machine_routed_spawn_tracking --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 304 - Dead Mob diagnostic runtime fallback is culled

Goal:

- remove the remaining dead old-regime diagnostic fallback in
  `MobHandle::diagnostic_runtime_adapter(...)`

What landed:

- removed the `Err(_) => self.session_service.runtime_adapter()` fallback from
  `meerkat-mob/src/runtime/handle.rs`
- the helper now returns `None` if the machine command fails, which is the
  honest post-cut behavior
- updated the stale runtime module header in
  `meerkat-mob/src/runtime/mod.rs` so it no longer claims read-only operations
  broadly bypass the actor; it now describes the actual machine-seam
  consolidation posture

Important backtrack:

- I did not cut the broader internal canonical snapshot helpers in this slice
- the only thing removed here is the dead diagnostic jump-around because the
  machine command path already returned the same runtime-adapter source

Why this slice matters:

- it removes one more hidden escape hatch from the cutover regime
- it also makes the runtime module docs stop lying about the current
  architecture

Validation:

- `cargo test -p meerkat-mob --lib test_diagnostic_meerkat_shadow_inputs_returns_best_effort_live_member_state --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 303 - Internal Mob event router relay joins the machine seam

Goal:

- remove the remaining internal old-regime relay in the mob-wide event router,
  which still polled the raw `MobEventStore` directly even after the public
  router subscription APIs had joined `MobMachineCommand`

What landed:

- rewired `spawn_event_router(...)` in
  `meerkat-mob/src/runtime/event_router.rs` so it no longer receives an
  `Arc<dyn MobEventStore>`
- rewired `run_event_router(...)` in the same file so bootstrap cursor seeding
  and roster-change tracking now go through `MobHandle::poll_events(...)`
- simplified the `SubscribeMobEvents { config }` command case in
  `meerkat-mob/src/runtime/handle.rs` so it now passes only the `MobHandle`
  clone and router config into the router

Important backtrack:

- I did not try to force the router into the actor itself
- the honest cut here is to keep the router as an independent runtime task,
  but make its only event-log input the machine-routed `MobHandle` event
  surface instead of the raw store handle

Why this slice matters:

- the public Mob event surface was already machine-routed in Slice 302
- after this slice, the mob-wide router itself also lives on that same event
  surface for:
  - bootstrap cursor seeding
  - `MeerkatSpawned` / `MeerkatRetired` tracking
- the raw event store is now even more firmly an internal persistence detail
  rather than router truth

Validation:

- `cargo test -p meerkat-mob --lib test_mob_event_router_stays_alive_across_machine_routed_spawn_tracking --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 296 - Mob event subscriptions and restore diagnostics stop bypassing shared state

Goal:

- remove the next public/helper-era Mob read bypasses that still touched shared
  roster or restore-diagnostic state directly

What landed:

- added `RestoreFailuresSnapshot` to the top-level Mob machine command/result
  surface in `meerkat-mob/src/mob_machine.rs`
- rewired `diagnostic_restore_failures_snapshot(...)` in
  `meerkat-mob/src/runtime/handle.rs` to use that machine command instead of
  reading `restore_diagnostics` directly
- rewired `subscribe_agent_events(...)` and
  `subscribe_all_agent_events(...)` in
  `meerkat-mob/src/runtime/handle.rs` so they:
  - resolve member/session identity through machine-routed projection surfaces
  - subscribe through the canonical `MobSessionService::subscribe_session_events`
    seam instead of the base `SessionService` default path
- added
  `test_agent_event_subscriptions_resolve_through_machine_routed_member_reads`
  in `meerkat-mob/src/runtime/tests.rs`
- removed the now-dead `RosterAuthority::session_id(...)` helper in
  `meerkat-mob/src/runtime/roster_authority.rs`

Important backtrack:

- the first version of the subscription cut used the base
  `SessionService::subscribe_session_events(...)` trait method, which produced
  false `stream not found` failures on the live Mob session-service contract
- the honest fix was not more roster tweaking; it was to route through the
  Mob-owned session-service seam that actually exposes session-wide event
  streams

Why this slice matters:

- public event subscriptions no longer bypass the machine/projection seam to
  read shared roster state directly
- restore-failure diagnostics now ride the same top-level command surface as
  the other read-side cutover work
- this removes another visible chunk of the old regime without inventing a new
  privileged bridge

Validation:

- `cargo test -p meerkat-mob --lib test_agent_event_subscriptions_resolve_through_machine_routed_member_reads`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 297 - Member session helpers stop peeking through the roster

Goal:

- remove the next small public read bypass on `MemberHandle`, where
  `current_session_id()` and `session_ref()` still read the shared roster
  directly instead of following the machine-routed member projection seam

What landed:

- rewired `MemberHandle::current_session_id()` in
  `meerkat-mob/src/runtime/handle.rs` to use `status()` /
  `MobHandle::member_status(...)`
- rewired `MemberHandle::session_ref()` in the same file to build on
  `current_session_id()` instead of touching the roster directly
- added
  `test_member_handle_session_helpers_round_trip_through_machine_projection_surface`
  in `meerkat-mob/src/runtime/tests.rs`

Why this slice matters:

- the public member helper surface now follows the same machine-routed
  lifecycle/session projection as the other read-side cutover work
- after this slice, the obvious public Mob member/session reads are no longer
  split between machine-routed status and raw roster peeks

Validation:

- `cargo test -p meerkat-mob --lib test_member_handle_session_helpers_round_trip_through_machine_projection_surface`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 298 - Mob diagnostic Meerkat bridge stops bypassing the machine seam

Goal:

- remove the next internal cutover helper bypasses on `MobHandle`, where the
  diagnostic bridge for live Meerkat shadow inputs still talked to
  `session_service` directly instead of riding the top-level machine command
  surface

What landed:

- added `DiagnosticRuntimeAdapter`, `DiagnosticHasLiveSession`, and
  `DiagnosticMeerkatShadowInputs` to the top-level command/result surface in
  `meerkat-mob/src/mob_machine.rs`
- re-exported `MeerkatShadowInputsSnapshot` from `meerkat-mob/src/runtime/mod.rs`
- rewired:
  - `diagnostic_runtime_adapter(...)`
  - `diagnostic_has_live_session(...)`
  - `diagnostic_meerkat_shadow_inputs(...)`
  in `meerkat-mob/src/runtime/handle.rs` to use
  `execute_machine_command(...)`
- updated the live-member-state proof in
  `meerkat-mob/src/runtime/tests.rs` so it still validates the best-effort
  diagnostic bridge after the cut

Important backtrack:

- the first version tried to keep `diagnostic_runtime_adapter(...)` as a sync
  helper while still riding the machine seam
- the honest fix was to make that helper async instead of inventing a hidden
  synchronous bypass around the command surface

Why this slice matters:

- the Mob-side shadow/cutover bridge no longer has a privileged direct path
  into `session_service`
- this keeps the diagnostic story aligned with the same machine-routed command
  surface used by the rest of the cutover work

Validation:

- `cargo test -p meerkat-mob --lib test_diagnostic_meerkat_shadow_inputs_returns_best_effort_live_member_state --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 299 - Mob event router bootstrap stops depending on raw shared state

Goal:

- remove the next public Mob bypass, where `subscribe_mob_events_with_config`
  still built the router directly from shared `session_service` and `roster`
  instead of the machine-routed member projection/subscription surface

What landed:

- rewired `meerkat-mob/src/runtime/event_router.rs` so the router now accepts a
  cloned `MobHandle` plus the Mob event store
- bootstrap and spawn tracking now subscribe members through:
  - `handle.list_members()`
  - `handle.subscribe_agent_events(...)`
- rewired `subscribe_mob_events_with_config(...)` in
  `meerkat-mob/src/runtime/handle.rs` to spawn the router from `self.clone()`
- added
  `test_mob_event_router_stays_alive_across_machine_routed_spawn_tracking`
  in `meerkat-mob/src/runtime/tests.rs`

Important backtrack:

- the first version tried to prove full mob-wide attributed delivery in the
  generic test harness
- that overclaimed what the current test services can guarantee
- the landed proof keeps the event router test honest:
  - the per-member session-event seam remains proved separately
  - the router test now proves the router stays live while tracking a spawned
    member through the machine-routed bootstrap path

Why this slice matters:

- the public mob-wide event router no longer depends on raw shared
  `session_service` / `roster` surfaces at construction time
- this removes another visible old-regime public seam without inventing a new
  privileged bridge

Validation:

- `cargo test -p meerkat-mob --lib test_mob_event_router_stays_alive_across_machine_routed_spawn_tracking --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 300 - Mob agent event subscriptions join the machine seam

Goal:

- remove the next public Mob bypass, where `subscribe_agent_events(...)` and
  `subscribe_all_agent_events(...)` still jumped straight from `MobHandle` to
  `session_service` instead of riding the top-level machine command surface

What landed:

- added `SubscribeAgentEvents` / `SubscribeAllAgentEvents` to
  `MobMachineCommand`
- added `EventStream` / `AllAgentEventStreams` to
  `MobMachineCommandResult`
- taught `execute_machine_command(...)` in
  `meerkat-mob/src/runtime/handle.rs` to resolve:
  - canonical member session binding
  - single-member session event subscription
  - point-in-time all-member event subscription
- rewired:
  - `subscribe_agent_events(...)`
  - `subscribe_all_agent_events(...)`
  in `meerkat-mob/src/runtime/handle.rs`
- kept the existing subscription regression and let the event router inherit
  this cut automatically because it already bootstraps through
  `handle.subscribe_agent_events(...)`

Why this matters:

- the public member/session event subscription surface no longer bypasses the
  machine seam
- after this slice, the remaining raw event-store handle is an internal
  router/runtime detail rather than a public subscription shortcut

Validation:

- `cargo test -p meerkat-mob --lib test_agent_event_subscriptions_resolve_through_machine_routed_member_reads --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 301 - Mob public event view and operator provenance append join the machine seam

Goal:

- remove the remaining public event-surface bypasses, where:
  - `handle.events().poll()/replay_all()` cloned the raw store directly
  - `record_operator_action_provenance(...)` appended directly to the raw store

What landed:

- added `PollEvents`, `ReplayAllEvents`, and
  `RecordOperatorActionProvenance` to `MobMachineCommand`
- added `MobEvents(Vec<MobEvent>)` to `MobMachineCommandResult`
- rewired `MobEventsView` in `meerkat-mob/src/runtime/handle.rs` so it now
  carries a `MobHandle` and routes:
  - `poll(...)`
  - `replay_all(...)`
  through `execute_machine_command(...)`
- rewired `record_operator_action_provenance(...)` through the same top-level
  machine command surface
- added focused regressions:
  - `test_mob_events_view_round_trips_through_machine_command_surface`
  - `test_record_operator_action_provenance_round_trips_through_machine_command_surface`

Why this matters:

- the public Mob event surface is now machine-routed end to end for:
  - subscription
  - polling/replay
  - operator-action audit append
- this leaves the raw event-store handle as an internal implementation detail
  instead of a public old-regime escape hatch

Validation:

- `cargo test -p meerkat-mob --lib test_mob_events_view_round_trips_through_machine_command_surface --quiet`
- `cargo test -p meerkat-mob --lib test_record_operator_action_provenance_round_trips_through_machine_command_surface --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 291 - SessionServiceRuntimeExt write-side joins the Meerkat machine seam

Goal:

- remove the last obvious duplicated write-side Meerkat authority path below
  the public control plane

What landed:

- added `logical_runtime_id(...)` and
  `driver_error_from_control_plane_error(...)` helpers in
  `meerkat-runtime/src/session_adapter.rs`
- rewired `SessionServiceRuntimeExt for RuntimeSessionAdapter` so the write-side
  methods now ride the same machine command surface as the public control plane:
  - `accept_input(...)`
  - `runtime_state(...)`
  - `retire_runtime(...)`
  - `reset_runtime(...)`
- left `input_state(...)` and `list_active_inputs(...)` as direct read
  projections for now, since they are read-only and not a duplicate authority
  seam
- added `session_service_runtime_ext_write_side_follows_machine_control_surface`
  in `meerkat-runtime/src/session_adapter.rs`

Important backtrack:

- I did not force the read-only projection helpers through the machine command
  surface in this slice; that would be churn rather than authority cleanup
- the honest cut here is removing duplicate write-side lifecycle/input logic,
  not pretending every read must become a command

Why this slice matters:

- `RuntimeControlPlane` and `SessionServiceRuntimeExt` no longer carry two
  separate implementations of the same Meerkat write-side semantics
- one more old-regime path is gone before we push deeper into actor/runtime
  relay cleanup

Validation:

- `cargo test -p meerkat-runtime --lib session_service_runtime_ext_write_side_follows_machine_control_surface`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `cargo test --workspace --tests --quiet` (long e2e lane still draining without
  early failures at slice capture time)
- `git diff --check`

## Slice 292 - Mob task-board reads join the machine seam

Goal:
- remove one more old-regime projection exception from the public `MobHandle`
  surface by routing task-board reads through `MobMachineCommand` instead of
  direct handle-side `task_board` access

What landed:
- added `TaskList` / `TaskGet` to:
  - `MobMachineCommand`
  - `MobMachineCommandResult`
  - `MobCommand`
- taught the actor to serve those read queries from canonical task-board state
- rewired `MobHandle::task_list(...)` / `task_get(...)` through
  `execute_machine_command(...)`
- removed the now-dead `MobHandle.task_board` field and its constructor wiring
- added a focused regression proving `task_get(...)` still works across reset

Why this matters:
- this trims another special-case public bypass before the harder cutover work
- task-board reads now follow the same top-level machine command seam as the
  rest of the public Mob API instead of quietly reading shared projection state

Validation:
- `cargo test -p meerkat-mob --lib test_task_get_round_trips_through_machine_command_surface`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 293 - Mob MCP lifecycle reads join the machine seam

Goal:
- remove another public handle-side shared-state bypass by routing MCP server
  lifecycle reads through `MobMachineCommand`

What landed:
- added `McpServerStates` to:
  - `MobMachineCommand`
  - `MobMachineCommandResult`
  - `MobCommand`
- taught the actor to answer MCP lifecycle projection reads from canonical
  `mcp_servers` state
- rewired `MobHandle::mcp_server_states(...)` through
  `execute_machine_command(...)`
- removed the now-dead `MobHandle.mcp_servers` field and its constructor wiring

Why this matters:
- task-board and MCP lifecycle reads were the two clearest remaining public
  projection exceptions after the main public cut
- this keeps the public Mob surface moving toward one top-level machine seam
  instead of a mix of machine commands plus direct shared-map reads

Validation:
- `cargo test -p meerkat-mob --lib test_lifecycle_updates_mcp_server_states`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 294 - Mob structural roster reads stay machine-routed without losing retiring visibility

Goal:
- keep `roster()`, `list_all_members()`, and `get_member()` on the top-level
  `MobMachineCommand` surface without losing the in-flight `Retiring` window
  that direct roster reads previously exposed

What landed:
- kept the public structural roster methods routed through
  `execute_machine_command(...)`
- changed `MobMachineCommand::{RosterSnapshot, ListAllMembers, GetMember}` to
  resolve directly from the shared canonical `RosterAuthority` instead of
  waiting behind long actor commands like `Retire`
- removed the now-unnecessary lower actor relay variants for those structural
  roster reads
- extended the focused structural roster regression and restored the retiring
  visibility regression

Important backtrack:
- the first actor-queued version was a real regression: `list_all_members()`
  could miss a member in the `Retiring` window because the read waited behind
  the long-running disposal command
- the honest fix was to preserve the machine command surface while resolving
  structural roster reads from the shared canonical authority immediately

Why this matters:
- public structural roster reads remain on the machine-owned surface
- we no longer pay for that cut with worse observability during in-flight
  retirement/disposal

Validation:
- `cargo test -p meerkat-mob --lib test_structural_roster_reads_round_trip_through_machine_command_surface`
- `cargo test -p meerkat-mob --lib test_retiring_member_is_not_routable_before_disposal_completes`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 295 - Mob member projection status joins the machine seam

Goal:
- move the remaining rich public member projection read, `member_status()`,
  onto the top-level machine command surface

What landed:
- added `MemberStatus` to:
  - `MobMachineCommand`
  - `MobMachineCommandResult`
- rewired `MobHandle::member_status(...)` through
  `execute_machine_command(...)`
- removed the dead `MobMachineCommandResult::MemberRef` variant while touching
  that surface
- added a focused regression proving `member_status()` still round-trips the
  active session binding through the machine-owned read path

Why this matters:
- public Mob reads are no longer split between:
  - machine-routed structural reads
  - direct handle-side rich lifecycle projection
- richer member lifecycle inspection now follows the same top-level machine
  command seam as the rest of the public cutover surface

Validation:
- `cargo test -p meerkat-mob --lib test_member_status_round_trips_through_machine_command_surface`
- `cargo test -p meerkat-mob --lib`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 288 - Remaining public drain helpers move behind the Meerkat machine seam

Goal:

- finish the last meaningful public Meerkat bypass family after the control,
  ingress, legacy-run, and silent-intent cuts

What landed:

- added `MeerkatMachineDrainLocalCommand` in
  `meerkat-runtime/src/meerkat_machine.rs`
- added `execute_meerkat_machine_drain_local_command(...)` in
  `meerkat-runtime/src/session_adapter.rs`
- rewired the remaining public drain helpers to go through that typed local
  machine seam:
  - `abort_comms_drains(...)`
  - `abort_comms_drain(...)`
  - `wait_comms_drain(...)`
- rewired the explicit disable branch in
  `update_peer_ingress_context_inner(...)` to use the same local drain command
  path

Important backtrack:

- I did not try to force `SetPeerIngressContext` into the same `&self` helper,
  because that seam still honestly needs `Arc<Self>` task-spawn authority
- the landed split is:
  - `MeerkatMachineDrainCommand` for the `Arc<Self>` spawn / notify path
  - `MeerkatMachineDrainLocalCommand` for abort / wait lifecycle helpers that
    do not need task-spawn authority

Why this slice matters:

- the remaining public comms-drain lifecycle helpers no longer mutate adapter
  state directly
- the public Meerkat surface is now consistently machine-routed even in the
  drain family, without introducing a fake unified abstraction

Validation:

- `cargo check -p meerkat-runtime --all-targets --quiet`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `git diff --check`

## Slice 290 - First real post-cutover failure is a stale RPC contract assertion

Goal:

- fix the first genuine runtime-adaptation failure surfaced by the fresh
  workspace `--tests` lane

What landed:

- updated `meerkat-rpc/tests/regression_rpc.rs` so
  `initialize_methods_list_complete` now compares
  `contract_version` against `meerkat_contracts::ContractVersion::CURRENT`
  instead of a stale hardcoded literal

Why this slice matters:

- the first real red after the public cut was not a machine/seam regression;
  it was a stale surface assertion
- tying the regression test to the canonical contracts constant prevents the
  same low-value failure from resurfacing on the next version bump

Validation:

- `cargo test -p meerkat-rpc --test regression_rpc initialize_methods_list_complete -- --exact --nocapture`
- `git diff --check`

## Slice 289 - Session unregister joins the Meerkat machine session seam

Goal:

- remove the last obvious old-style public session teardown mutation from the
  Meerkat adapter surface

What landed:

- added `UnregisterSession` to `MeerkatMachineSessionCommand` in
  `meerkat-runtime/src/meerkat_machine.rs`
- added `unregister_session_inner(...)` in
  `meerkat-runtime/src/session_adapter.rs`
- rewired public `unregister_session(...)` to route through
  `execute_meerkat_machine_session_command(...)`

Why this slice matters:

- the public Meerkat session mutation surface is now consistently routed
  through typed session/control/ingress/drain/legacy-run machine seams
- unregister is no longer a one-off direct teardown path beside the machine

Validation:

- `cargo check -p meerkat-runtime --all-targets --quiet`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`
- `git diff --check`

## Slice 283 - Public machine command cutover lands for Meerkat and Mob

Goal:

- remove the remaining public write/control seams that still bypassed the frozen
  machine layers before the brutal cutover

What landed:

- added `MeerkatMachineControlCommand` /
  `MeerkatMachineControlCommandResult` in
  `meerkat-runtime/src/meerkat_machine.rs`
- routed the full `RuntimeControlPlane for RuntimeSessionAdapter` impl through
  `execute_meerkat_machine_control_command(...)` in
  `meerkat-runtime/src/session_adapter.rs`
- kept the already-landed Meerkat session/drain command routing as the public
  session mutation seam
- extended `MobMachineCommand` /
  `MobMachineCommandResult` in `meerkat-mob/src/mob_machine.rs` so the
  remaining public diagnostic and kickoff snapshot paths also route through the
  machine command layer
- rewired the corresponding `MobHandle` helpers in
  `meerkat-mob/src/runtime/handle.rs`

Important backtracks:

- the new Meerkat control-command helper surfaced a hidden tuple bug in the
  `Ingest` arm that had been masked only because the trait impl was still
  bypassing the helper; that bug was fixed instead of preserving the bypass
- the Mob diagnostic snapshotters needed explicit command/result variants rather
  than pretending the two crates already shared one diagnostic result type

Why this slice matters:

- the brutal cut is now real on the public mutation/control surface:
  - Meerkat public control/session/drain mutation routes through machine
    command layers
  - Mob public flow/member/task/diagnostic mutation routes through the
    machine command layer
- the old regime is no longer propped up by public helper bypasses

Validation:

- `cargo check -p meerkat-runtime -p meerkat-mob -p meerkat --all-targets --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `git diff --check`

## Slice 286 - Final cutover-readiness handoff lands

Goal:

- close the preparation phase with one explicit note that says what is frozen,
  what is proven, what is shadowed, and what we already expect to break first

What landed:

- added `two-kernel-cutover-readiness.md`
- removed the stray unused `LogicalRuntimeId` import from
  `meerkat-mob/src/mob_machine.rs` so the final prep patch set does not add
  fresh local warning noise

Why this slice matters:

- the branch no longer relies on piecing together the final cutover posture
  from many machine, proof, and shadow docs
- the first brutalist pass now has one canonical handoff note

Validation:

- `cargo test --workspace --lib --quiet`
- `git diff --check`

## Slice 283 - First genuinely multi-run live report session lands

Goal:

- prove the shared report/session layer on more than one real runtime-backed
  run instead of one mixed test assembly

What landed:

- extended `meerkat-mob/src/runtime/tests.rs` with
  `test_export_two_kernel_shadow_report_session_collects_multiple_live_runs`
- the new live report session now exports:
  - `session_id = "shadow.cutover.multi-run"`
  - run batch `0`:
    - `run_id = "shadow.cutover.green-run"`
    - one healthy runtime-backed green scenario batch
  - run batch `1`:
    - `run_id = "shadow.cutover.drift-run"`
    - one mismatch-producing runtime-backed drift scenario batch

Important backtrack:

- the first tempting approach was to keep reusing one live run and just split
  it into more phases
- the landed version uses two distinct runtime-backed runs so the session layer
  now proves a real cutover collection story, not just a more nested one-run
  fixture

Why this slice matters:

- the sink now has a first honest "collection session" artifact over multiple
  live runs
- this is the first point where the shadow export path starts resembling a
  real cutover rehearsal harness instead of a sequence of unit-shaped exports

Validation:

- `cargo test -p meerkat-mob --lib test_export_two_kernel_shadow_report_session_collects_multiple_live_runs`
- `cargo test -p meerkat-mob --lib test_export_two_kernel_shadow_run_batch_collects_green_and_drift_scenarios`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

## Slice 284 - Session-level triage summary lands for shared shadow reports

Goal:

- add the first compact top-level triage view over a multi-run cutover shadow
  collection session

What landed:

- added `TwoKernelShadowReportSessionSummary` in
  `meerkat-mob/src/mob_machine.rs`
- added `summarize_two_kernel_shadow_report_session(...)` in
  `meerkat-mob/src/mob_machine.rs`
- extended
  `test_export_two_kernel_shadow_report_session_collects_multiple_live_runs`
  in `meerkat-mob/src/runtime/tests.rs` so it now proves:
  - `run_count = 2`
  - `scenario_count = 2`
  - `sample_count = 2`
  - `green_sample_count = 1`
  - `mismatch_sample_count = 1`
  - `seam_bucket_count >= 2`

Why this slice matters:

- the sink is no longer just a nested export tree
- it now has the first compact session-level triage artifact a real cutover
  rehearsal can inspect before drilling into raw scenario batches

Validation:

- `cargo test -p meerkat-mob --lib test_export_two_kernel_shadow_report_session_collects_multiple_live_runs`
- `git diff --check`

## Slice 285 - Machine-readable shadow report export lands

Goal:

- turn the shared shadow report-session shape into a real cutover artifact
  instead of keeping it purely in-memory and test-local

What landed:

- added `export_two_kernel_shadow_report_session_pretty_json(...)` in
  `meerkat-mob/src/mob_machine.rs`
- added
  `export_two_kernel_shadow_report_session_pretty_json_roundtrips_structure`
  in `meerkat-mob/src/mob_machine.rs`
- the JSON proof now round-trips:
  - `session_id`
  - run ids
  - scenario batch ids
  - sample ids
  - seam bucket labels

Why this slice matters:

- the first brutal cutover can emit a single machine-readable artifact without
  inventing a second export format later
- the sink hierarchy now has:
  - structural export
  - compact session summary
  - machine-readable serialization

Validation:

- `cargo test -p meerkat-mob --lib export_two_kernel_shadow_report_session_pretty_json_roundtrips_structure`
- `git diff --check`

## Slice 287 - Legacy sync run helper moves behind the Meerkat machine seam

Goal:

- remove the last substantial public Meerkat compatibility helper that still
  hand-drove admission, run start, and run terminalization outside the machine
  command layer

What landed:

- added `MeerkatMachineLegacyRunCommand` /
  `MeerkatMachineLegacyRunCommandResult` in
  `meerkat-runtime/src/meerkat_machine.rs`
- added `execute_meerkat_machine_legacy_run_command(...)` in
  `meerkat-runtime/src/session_adapter.rs`
- rewired `accept_input_and_run(...)` to:
  - `Prepare`
  - execute the existing caller-supplied operation
  - `Commit` or `Fail`

Important backtrack:

- I did not try to hide the generic operation closure inside the machine
  command enum; the honest cut is to centralize the Meerkat-owned prepare /
  commit / fail seams and leave the caller-provided apply closure outside that
  boundary for now

Why this slice matters:

- the public Meerkat session surface no longer freehands legacy sync run setup
  and teardown against the driver
- after this slice, the old-regime public bypasses are basically gone; the
  remaining direct command sends are internal actor/runtime relays rather than
  public control seams

Validation:

- `cargo fmt --all`
- `cargo check -p meerkat-runtime --all-targets --quiet`
- `cargo test -p meerkat-runtime --lib session_adapter --quiet`

## Slice 282 - Shared two-kernel shadow report session lands

Goal:

- move the sink from one reusable run batch to the first reusable shared
  collection session that can accumulate multiple cutover-facing runs

What landed:

- added `TwoKernelShadowReportSession` in `meerkat-mob/src/mob_machine.rs`
- added `export_two_kernel_shadow_report_session(...)` in
  `meerkat-mob/src/mob_machine.rs`
- extended
  `test_export_two_kernel_shadow_run_batch_collects_green_and_drift_scenarios`
  in `meerkat-mob/src/runtime/tests.rs` so it now exports:
  - `session_id = "shadow.cutover.session"`
  - run batch `0`:
    - `run_id = "shadow.cutover.smoke"`
    - one green scenario batch
    - one mismatch-producing scenario batch
  - run batch `1`:
    - `run_id = "shadow.cutover.green-only"`
    - one green-only scenario batch

Important backtrack:

- the first tempting shape was a flat session-level list of scenario samples
- the landed shape keeps the run batch boundary intact, because one cutover
  collection session may include multiple runs and each run may still include
  multiple scenario batches

Why this slice matters:

- the sink now has a stable hierarchy:
  - sample
  - scenario batch
  - run batch
  - report session
- this is the first shape that can honestly model an actual cutover shadow
  collection session instead of a single test fixture

Validation:

- `cargo test -p meerkat-mob --lib test_export_two_kernel_shadow_run_batch_collects_green_and_drift_scenarios`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`
## Slice 314 - Mob bridge-session wire outputs become additive across public and helper surfaces

Goal: keep the clean-cut / identity-first runtime semantics moving outward so Mob-facing wire outputs stop speaking bridge bindings only through legacy `session_id` names.

What landed:
- Added additive `bridge_session_id` / `current_bridge_session_id` outputs alongside compatibility `session_id` / `current_session_id` keys on the major Mob-facing result builders in:
  - `meerkat-mob/src/runtime/tools.rs`
  - `meerkat-mob-mcp/src/agent_tools.rs`
  - `meerkat-mob-mcp/src/public_mcp.rs`
  - `meerkat-rpc/src/handlers/mob.rs`
  - `meerkat-mob-mcp/src/lib.rs`
- Extended `meerkat_contracts::MobMemberSendResult` additively with `bridge_session_id`, and updated the RPC handler to populate both fields from the same bridge binding.

Backtrack:
- The first additive pass tried to move the same owned `SessionId` twice in several builders. The honest fix was to clone at the wire edge where we intentionally duplicate compatibility and bridge-session fields, instead of pretending the result types were already alias-aware.

Why this matters:
- The runtime is already identity-first internally, but public Mob surfaces were still speaking almost entirely in old session-centric vocabulary.
- This slice starts making the bridge-binding meaning explicit for consumers without breaking existing `session_id` readers.

Validation:
- `cargo test -p meerkat-contracts -p meerkat-mob -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 315 - Orphan cleanup now follows the canonical bridge-owner index

Goal: stop treating compatibility `owner_session_id` as the cleanup/scavenge
truth now that the identity-first owner bridge binding is explicit.

What landed:
- `meerkat-mob-mcp/src/lib.rs`
  - `scavenge_orphaned_session_scoped_mobs_inner()` now collects candidates via
    `definition.owner_bridge_session_index()` instead of reading
    `definition.owner_session_id` directly
  - doc comments on implicit lookup / cleanup / scavenging now speak in terms of
    the owner bridge-session index, with compatibility fallback explicitly
    called out
- added
  `test_scavenge_orphaned_session_scoped_mobs_honors_bridge_owner_index`
  proving a session-scoped mob with only `owner_bridge_session_id` populated is
  still scavenged once the owning bridge session disappears
- `examples/035-mdm-tux-rs/src/bin/kennel.rs`
  - hive-mob creation now sets additive `owner_bridge_session_id` alongside the
    compatibility owner field

Why this matters:
- cleanup and orphan scavenging are part of the identity-first semantic shift,
  not just naming
- after this slice, the canonical cleanup path no longer depends on the old
  compatibility owner field being present

Validation:
- `cargo test -p meerkat-mob-mcp --lib test_scavenge_orphaned_session_scoped_mobs_honors_bridge_owner_index --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 316 - Bridge-session semantics become canonical in public Mob outputs

Goal: make the public MCP/RPC result builders source bridge-session outputs from
the canonical bridge-binding accessors instead of compatibility aliases.

What landed:
- `meerkat-mob-mcp/src/public_mcp.rs`
  - `meerkat_mob_member_send` now sources `bridge_session_id` from
    `receipt.bridge_session_id()`
  - `meerkat_mob_append_system_context` now uses a bridge-native local binding
    and emits both `session_id` and `bridge_session_id` from that canonical
    value
- `meerkat-rpc/src/handlers/mob.rs`
  - `MobMemberSendResult.bridge_session_id` now comes from
    `receipt.bridge_session_id().clone()`
  - append-system-context response now uses a bridge-native local binding too

Why this matters:
- additive bridge-session fields are much less useful if we still populate them
  through the old compatibility alias
- after this slice, the public wire shape and the runtime meaning line up

Validation:
- `cargo test -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 317 - Mob-MCP state API gets canonical bridge-session entry points

Goal: stop exposing bridge-bound Mob-MCP state operations only through generic
`*_session_*` helper names.

What landed:
- `meerkat-mob-mcp/src/lib.rs`
  - added canonical bridge-native entry points:
    - `find_implicit_mob_for_bridge_session(...)`
    - `find_mobs_for_bridge_session(...)`
    - `get_or_create_implicit_mob_for_bridge_session(...)`
    - `destroy_bridge_session_mobs(...)`
  - kept the older session-centric names as thin compatibility wrappers
- updated internal callers to prefer the new canonical entry points:
  - `meerkat-mob-mcp/src/agent_tools.rs`
  - `meerkat-rest/src/lib.rs`
  - `meerkat-rpc/src/router.rs`

Why this matters:
- the state API itself now reflects the bridge-binding truth instead of forcing
  identity-first logic through generic session naming
- this keeps the internal cutover story aligned with the runtime semantics

Validation:
- `cargo test -p meerkat-mob-mcp -p meerkat-rpc -p meerkat-rest --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 318 - Mob machine mismatch labels align to canonical bridge binding

Goal: stop reporting bridge-binding drift under the old
`current_session_id` label inside `MobMachine` mismatches once the canonical
projection field is `current_bridge_session_id`.

What landed:
- `meerkat-mob/src/mob_machine.rs`
  - composition mismatch text now says
    `projected current_bridge_session_id`
  - structural projected mismatch for bridge-binding drift now emits
    `field: "current_bridge_session_id"`
  - focused validator expectations were updated to match the canonical field

Why this matters:
- the machine output itself now matches the identity-first/runtime cutover
  story instead of leaking compatibility names in the main drift taxonomy

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 344 - CancelAfterBoundary and PublishCommittedVisibleSet are now live seams

Goal: close the last two explicitly deferred Meerkat cutover seams instead of
leaving them as target-only machine verbs.

What landed:
- `meerkat-runtime/src/meerkat_machine_types.rs`
  - `CancelAfterBoundary` now exists on the Meerkat session command surface
- `meerkat-runtime/src/meerkat_machine.rs`
  - `MeerkatMachine::cancel_after_boundary(...)` now routes the request through
    the live attached-loop control seam
- `meerkat-session/src/ephemeral.rs`
  - boundary-cancel requests now set a shared live flag and wake the running
    session task
- `meerkat/src/service_factory.rs`
  - real runtime-backed agents now expose the shared boundary-cancel handle
- `meerkat-core/src/agent/runner.rs`
  - committed visibility publication is now one explicit helper:
    `publish_committed_visible_set()`
- `meerkat-core/src/agent/state.rs`
  - boundary visibility tests now prove the committed state is persisted into
    session metadata

Why this matters:
- `CancelAfterBoundary` is no longer just target-machine truth; it now has a
  real top-level lowering through the runtime/session authority seam
- `PublishCommittedVisibleSet` is no longer only an implicit side effect in the
  runner; it is now a named committed-visibility publication seam

Validation:
- `cargo test -p meerkat-session --test ephemeral_contract test_cancel_after_boundary -- --nocapture`
- `cargo test -p meerkat-runtime --lib cancel_after_boundary_ -- --nocapture`
- `cargo test -p meerkat-core --lib run_loop_boundary_applies_filter_and_emits_tool_config_changed_and_notice -- --nocapture`
- `git diff --check`

## Slice 344 - Meerkat authority rename is complete and the full cut is green

Goal: finish the accepted v1 authority rename so the runtime control plane is
implemented in `meerkat_machine.rs` rather than still living behind the old
`session_adapter.rs` filename, while keeping compatibility imports stable
during the cut.

What landed:
- `meerkat-runtime/src/meerkat_machine.rs`
  - now holds the real `MeerkatMachine` authority implementation
- `meerkat-runtime/src/session_adapter.rs`
  - reduced to a thin compatibility re-export shim
- `meerkat-runtime/src/lib.rs`
  - exports `meerkat_machine` directly as the canonical runtime authority
    module
- `xtask/src/ownership_ledger.rs`
- `xtask/src/rmat_policy.rs`
- `meerkat-machine-schema/src/catalog/coverage.rs`
  - updated code-facing path metadata so ownership/rmat/schema checks now point
    at the renamed authoritative file
- `meerkat-mob/src/mob_machine.rs`
- `meerkat-mob/src/runtime/state.rs`
- `meerkat-mob/src/runtime/actor.rs`
- `meerkat-mob/src/runtime/handle.rs`
  - test-only orchestrator snapshot command path is now explicitly `#[cfg(test)]`
    so the clean-cut regime no longer leaves dead library-code warnings behind

Why this matters:
- it closes the last obvious naming mismatch between what Meerkat is called
  architecturally and where the runtime authority actually lives
- it leaves `session_adapter` as compatibility only, instead of a half-renamed
  primary implementation surface
- it proves the cut is not just locally green: the full rebased workspace test
  lane still passes with the renamed authority file in place

Validation:
- `cargo check --workspace --all-targets --quiet`
- `cargo test --workspace --lib --quiet`
- `cargo nextest run --workspace`
- `git diff --check`

## Slice 351 - In-repo callers now use bridge-session Mob MCP helpers directly

Goal: stop teaching the compatibility `*_session_*` Mob MCP wrappers as the
primary in-repo interface now that the bridge-session canon is stable.

What landed:
- `tests/integration/tests/live_mob_tools.rs`
  - live integration coverage now calls
    `find_implicit_mob_for_bridge_session(...)` and
    `find_mobs_for_bridge_session(...)` directly
- `examples/035-mdm-tux-rs/src/bin/target.rs`
  - target cleanup and implicit-mob checks now use
    `destroy_bridge_session_mobs(...)` and
    `find_implicit_mob_for_bridge_session(...)`
- `meerkat-mcp-server/src/lib.rs`
  - archive cleanup now uses `destroy_bridge_session_mobs(...)`
- `meerkat-mob-mcp/src/lib.rs`
  - bridge-native restore labels are now used in the canonical lookup paths
  - in-crate tests now exercise
    `owns_persisted_bridge_session(...)` and
    `retire_member_by_bridge_session_id(...)` directly
- `meerkat-mob-mcp/src/agent_tools.rs`
  - cleanup docs now name the bridge-session canonical helper rather than the
    compatibility alias

Why this matters:
- it pushes the codebase itself to teach the identity-first bridge-binding
  model instead of keeping the old session-centric wrappers as the normal
  authoring surface
- it leaves the compatibility wrappers intact for outside callers and legacy
  payloads, but makes them clearly additive rather than canonical

Validation:
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test -p meerkat-integration-tests --test live_mob_tools --features integration-real-tests --no-run`
- `git diff --check`

## Slice 349 - TypeScript SDK now types bridge-native member refs explicitly

Goal: stop leaving the typed Node SDK with an unstructured `memberRef` blob
once the client already normalizes bridge-session binding at that surface.

What landed:
- `sdks/typescript/src/types.ts`
  - added `MobMemberRef`
  - `MobMember.memberRef` now uses `MobMemberRef`
  - `MobSpawnManyResultEntry.memberRef` now uses `MobMemberRef`
- `sdks/typescript/src/client.ts`
  - added `normalizeMobMemberRef(...)`
  - `listMobMembers(...)` now emits typed bridge-native `memberRef`
  - `spawnMobMembers(...)` now emits typed bridge-native `memberRef`
- `sdks/typescript/tests/types.test.js`
  - now asserts `memberRef.sessionId` / `memberRef.bridgeSessionId`
    on both spawn-many and member-listing paths

Why this matters:
- it turns the bridge-session alias from a duplicated outer convenience field
  into a real typed part of the public SDK surface
- it keeps the TypeScript SDK aligned with the browser SDK and the Python
  client instead of leaving `memberRef` as the last raw old-regime island

Validation:
- `npm run build && node --test tests/types.test.js` (in `sdks/typescript`)
- `git diff --check`

## Slice 350 - Python mob wrappers now name bridge-native member projections

Goal: stop having the Python client normalize bridge aliases while the public
mob module still advertises anonymous `dict[str, Any]` payloads for member
refs and member listings.

What landed:
- `sdks/python/meerkat/mob.py`
  - added `MobMemberRef`
  - added `MobSpawnResult`
  - added `MobMember`
  - `Mob.members()`, `Mob.spawn()`, and `Mob.spawn_many()` now use those
    named bridge-aware shapes
- `sdks/python/meerkat/client.py`
  - `list_mob_members(...)` now returns `list[MobMember]`
  - `spawn_mob_member(...)` now returns `MobSpawnResult`
  - `spawn_mob_members(...)` now returns `list[MobSpawnResult]`

Why this matters:
- it makes the Python mob surface describe the bridge-native member binding
  explicitly instead of silently normalizing it under anonymous dicts
- it keeps the three SDKs converging on the same named identity-first shapes

Validation:
- `python -m py_compile sdks/python/meerkat/client.py sdks/python/meerkat/mob.py`
- `python -m pytest sdks/python/tests/test_types.py -q`
- `git diff --check`

## Slice 348 - Web SDK now types bridge-native member refs explicitly

Goal: stop teaching browser-side Mob member identity as an untyped
`Record<string, unknown>` once the wrapper has already normalized the
bridge-session aliases.

What landed:
- `sdks/web/src/types.ts`
  - added `MobMemberRef`
  - `SpawnResult.member_ref` now uses `MobMemberRef | null`
  - `MobMember.member_ref` now uses `MobMemberRef`
- `sdks/web/src/mob.ts`
  - bridge-alias normalization helpers now work against the typed
    `MobMemberRef` shape instead of a generic record
- `sdks/web/tests/e2e_wasm_runtime.test.mjs`
  - the mocked parity lane still proves the wrapper fills
    `bridge_session_id` when only `session_id` is present

Why this matters:
- it turns the browser SDK boundary into an honest identity-first surface
  instead of relying on runtime normalization hidden behind raw records
- it keeps the typed Web facade aligned with the richer Python and TypeScript
  client behavior we already landed

Validation:
- `npm test && node --test --test-name-pattern "forwards canonical mob status/helper methods through the wasm binding surface" tests/e2e_wasm_runtime.test.mjs` (in `sdks/web`)
- `git diff --check`

## Slice 347 - Web mob wrapper now normalizes bridge-session aliases on raw member projections

Goal: stop leaving the lightweight WASM/web Mob facade behind the richer SDK
clients now that bridge-session binding is the canonical identity-first story.

What landed:
- `sdks/web/src/mob.ts`
  - `spawn(...)` now fills `member_ref.bridge_session_id` from
    `member_ref.session_id` when the runtime only returns the compatibility
    field
  - `listMembers(...)` now normalizes:
    - `member_ref.bridge_session_id` from `member_ref.session_id`
    - `current_bridge_session_id` from `current_session_id`
- `sdks/web/tests/e2e_wasm_runtime.test.mjs`
  - the shipped WASM parity lane now intentionally feeds old-style spawn/list
    payloads with only `session_id`
  - the wrapper is required to surface the additive bridge-session aliases
    itself rather than inheriting them from the mocked payload

Why this matters:
- it keeps the lightweight browser runtime facade aligned with the Python and
  TypeScript clients instead of making Web the last place that still teaches
  raw session-centric member projections
- it proves the bridge alias is a client guarantee at the wrapper surface, not
  just something that happens to be present when the backend is already updated

Validation:
- `npm run build && node --test --test-name-pattern "forwards canonical mob status/helper methods through the wasm binding surface" tests/e2e_wasm_runtime.test.mjs` (in `sdks/web`)
- `git diff --check`

## Slice 344 - Web SDK mob surfaces now carry additive bridge-session aliases

Goal: keep the browser/WASM client aligned with the identity-first Mob regime
 instead of leaving bridge bindings hidden behind compatibility-only
 `session_id` reads.

What landed:
- `sdks/web/src/types.ts`
  - additive bridge-session fields now exist on:
    - member delivery receipts
    - respawn receipts
    - helper results
    - member snapshots
- `sdks/web/src/mob.ts`
  - public mob helpers now fill additive bridge-session aliases from the
    canonical wire payload, falling back to compatibility `session_id` only
    when needed
- `sdks/web/tests/e2e_wasm_runtime.test.mjs`
  - wasm binding tests now assert additive bridge-session fields on mob helper,
    send, respawn, and status paths
- `sdks/web/tests/live_smoke.test.mjs`
  - live smoke scenarios now check bridge-session visibility on real Mob
    spawn/status/respawn paths

Why this matters:
- it extends the clean-cut identity-first story into the browser client
  instead of limiting it to Rust surfaces
- it proves that the new bridge-native payload shape survives both mocked wasm
  bindings and live packaged-runtime usage

Validation:
- `npm test` in `sdks/web`
- `npm run test:e2e` in `sdks/web`
- `git diff --check`

## Slice 345 - SDK mob clients now normalize bridge-session aliases consistently

Goal: stop leaving Python and TypeScript mob helpers/status paths as the odd
 compatibility pockets where bridge bindings only existed if callers manually
 inspected raw wire payloads.

What landed:
- `sdks/python/meerkat/client.py`
  - `spawn_mob_member(...)`, `list_mob_members(...)`,
    `mob_member_status(...)`, `wait_mob_kickoff(...)`,
    `spawn_mob_helper(...)`, `fork_mob_helper(...)`,
    `append_mob_system_context(...)`, and `respawn_mob_member(...)`
    now fill additive bridge-session aliases from compatibility fields when
    the bridge-native key is absent
- `sdks/python/tests/test_types.py`
  - mob client tests now assert the additive bridge-session aliases on those
    normalized paths
- `sdks/typescript/src/client.ts`
  - raw-record mob helpers `spawnMobMember(...)` and
    `appendMobSystemContext(...)` now add `bridge_session_id` when only
    `session_id` is present
- `sdks/typescript/tests/types.test.js`
  - parity tests now prove the additive bridge alias shows up on spawn,
    append, helper, and kickoff wrapper paths

Why this matters:
- it keeps the old wire keys compatible without teaching them as the primary
  semantic meaning in the SDKs
- it makes the identity-first bridge-binding story consistent across Rust,
  Python, TypeScript, and web instead of stopping at the runtime boundary

Validation:
- `python -m pytest sdks/python/tests/test_types.py -q`
- `npm run build && node --test tests/types.test.js` in `sdks/typescript`
- `cargo test -p meerkat-mob -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `git diff --check`

## Slice 346 - Python and TypeScript mob clients now treat bridge binding as first-class

Goal: stop leaving the public SDK mob helpers half in the old regime where
 additive `bridge_session_id` support existed only on some structured helpers
 and live smoke still mostly asserted legacy session-centric keys.

What landed:
- `sdks/python/meerkat/client.py`
  - `spawn_mob_member(...)`, `list_mob_members(...)`,
    `mob_member_status(...)`, `wait_mob_kickoff(...)`,
    `spawn_mob_helper(...)`, `fork_mob_helper(...)`,
    `append_mob_system_context(...)`, and `respawn_mob_member(...)`
    now normalize additive bridge-session aliases from compatibility fields
    when the canonical key is absent
- `sdks/python/tests/test_types.py`
  - client tests now assert the additive bridge aliases on those mob paths
- `sdks/python/tests/test_e2e_smoke.py`
  - live smoke now expects bridge-session fields on spawn, append, send, and
    respawned-member observation
- `sdks/typescript/src/client.ts`
  - `spawnMobMember(...)` and `appendMobSystemContext(...)` now add
    `bridge_session_id` when only `session_id` is present
- `sdks/typescript/tests/types.test.js`
  - parity tests now assert additive bridge aliases on spawn, append, helper,
    and kickoff paths
- `sdks/typescript/tests/e2e_smoke.test.mjs`
  - live smoke now expects bridge-session aliases on spawn, append, and
    respawned reviewer observation

Why this matters:
- it extends the identity-first cutover beyond Rust internals and into the
  public clients people actually touch
- it keeps the old wire keys compatible without teaching them as the primary
  meaning at the client layer

Validation:
- `python -m pytest sdks/python/tests/test_types.py -q`
- `python -m py_compile sdks/python/tests/test_e2e_smoke.py`
- `npm run build && node --test tests/types.test.js` in `sdks/typescript`
- `git diff --check`

## Slice 319 - Helper results now expose canonical bridge-session identity

Goal: stop treating helper/fork results as bridge-backed semantically while only
carrying that identity through the compatibility `session_id` field.

What landed:
- `meerkat-mob/src/runtime/handle.rs`
  - `HelperResult` now carries additive `bridge_session_id`
  - added canonical accessors:
    - `bridge_session_id()`
    - `session_id()`
- `meerkat-mob/src/runtime/mob_member_lifecycle_authority.rs`
  - helper materialization now populates both compatibility and canonical
    bridge-session fields from the same live bridge binding
- `meerkat-rpc/src/handlers/mob.rs`
  - `mob/spawn_helper` and `mob/fork_helper` now emit `bridge_session_id`
    from the canonical accessor instead of mirroring the compatibility field
- `meerkat-mob-mcp/src/lib.rs`
  - restore/orphan tests now assert live member binding through
    `current_bridge_session_id()`
  - helper-related cleanup tests no longer overclaim bridge accessors on raw
    `RunResult` values

Why this matters:
- helper/fork flows are one of the last places where bridge-backed runtime
  truth was still traveling under an old session-only result shape
- after this slice, the helper path is aligned with the rest of the
  identity-first additive bridge-session rollout

Validation:
- `cargo test -p meerkat-mob -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 320 - Diagnostic command paths now speak bridge-session internally

Goal: stop carrying bridge bindings under raw `session_id` field names in the
Mob actor/machine diagnostic command path once the rest of the runtime has
already moved to canonical bridge-session semantics.

What landed:
- `meerkat-mob/src/mob_machine.rs`
  - `MobMachineCommand::DiagnosticHasLiveSession`
  - `MobMachineCommand::DiagnosticMeerkatShadowInputs`
  - both now carry `bridge_session_id`
- `meerkat-mob/src/runtime/state.rs`
  - `MobCommand::DiagnosticHasLiveSession`
  - `MobCommand::DiagnosticMeerkatShadowInputs`
  - both now carry `bridge_session_id`
- `meerkat-mob/src/runtime/handle.rs`
  - crate-private diagnostic helpers now accept `bridge_session_id`
  - machine command dispatch forwards that canonical binding name end to end
- `meerkat-mob/src/runtime/actor.rs`
  - actor dispatch now queries session-service diagnostics via
    `bridge_session_id`

Why this matters:
- the shadow/diagnostic seam was still one of the last actor-internal places
  where identity-first bridge bindings were traveling under the old
  `session_id` label
- after this slice, the diagnostic command path matches the canonical bridge
  language already used by restore-failure, pending-spawn, and respawn
  internals

Validation:
- `cargo test -p meerkat-mob --lib test_diagnostic_meerkat_shadow_inputs_returns_best_effort_live_member_state --quiet`
- `cargo test -p meerkat-mob --lib test_capture_mob_shadow_suite_report_returns_empty_for_live_system --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `git diff --check`

## Slice 321 - Provisioner internals now treat runtime bindings as bridge sessions

Goal: stop teaching the clean-cut runtime the old `session_id` vocabulary from
inside the Mob provisioner, where the binding is really a bridge session
managed by the Meerkat runtime seam.

What landed:
- `meerkat-mob/src/runtime/provisioner.rs`
  - `MobSessionRuntimeExecutor` now stores `bridge_session_id`
  - executor `apply(...)` / `control(...)` use `bridge_session_id`
  - `session_turn_error_to_mob_error(...)` now treats its input as a bridge
    session binding
  - the direct `SessionBackend::provision_member(...)` pre-registration path
    now uses `pre_registered_bridge_session_id` /
    `member_bridge_session_id`
  - `abort_member_provision(...)` now uses `bridge_session_id` internally
    through ops lookup, archive, unregister, and runtime-state cleanup

Why this matters:
- the public/session-service result shapes still keep compatibility names, but
  the Mob runtime internals no longer need to think in those terms
- this keeps the clean-cut / identity-first adaptation moving in the runtime
  path instead of only at the wire and diagnostic layers

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 322 - Deeper Mob turn execution paths now name bridge bindings honestly

Goal: keep the clean-cut / identity-first adaptation moving in the deeper Mob
runtime path by removing another layer of internal `session_id` vocabulary
where the live value is actually a bridge-session binding.

What landed:
- `meerkat-mob/src/runtime/actor_turn_executor.rs`
  - autonomous flow dispatch now uses `bridge_session_id` locally
  - `StreamScopeFrame::MobMember.session_id` is explicitly documented as the
    stable event field name that carries the canonical bridge-session binding
- `meerkat-mob/src/runtime/actor.rs`
  - autonomous lifecycle kickoff injection now uses `bridge_session_id`
  - autonomous dispatch locals now use `bridge_session_id`
  - pending spawn progress recording now stores `bridge_session_id` under a
    correctly named local binding instead of immediately reverting to
    `session_id`
- `meerkat-mob/src/runtime/provisioner.rs`
  - `interaction_event_injector(...)` now speaks in terms of
    `bridge_session_id`
  - member-derived helpers (`is_member_active`, `comms_runtime`,
    `active_operation_id_for_member`, `bind_member_owner_context`) now use
    bridge-native local naming consistently

Why this matters:
- the deeper runtime execution path now matches the bridge-session semantics
  already established in the machine surface, diagnostics, and provisioner
  executor path
- this reduces the remaining semantic noise where identity-first behavior was
  already correct but the internal vocabulary still taught the old regime

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test -p meerkat-rpc --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 323 - Mob runtime rebuild/resume paths now use bridge-session vocabulary

Goal: keep the identity-first cut moving through the remaining runtime glue so
the rebuild/resume path no longer hides bridge bindings under plain
`session_id` locals.

What landed:
- `meerkat-mob/src/runtime/builder.rs`
  - missing-session recreation for roster-backed members now uses
    `bridge_session_id`
  - restore-failure diagnostics are recorded from bridge-native locals
  - orchestrator resume notification now uses `bridge_session_id` when talking
    to the live injector

Why this matters:
- the deeper runtime rebuild/resume path now matches the bridge-session
  semantics already used in the actor, provisioner, diagnostics, and public
  machine command surfaces
- this removes another pocket where the new regime behaved correctly but was
  still explained in the old vocabulary

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 324 - Canonical Mob member projection now names bridge bindings honestly

Goal: finish the current identity-first vocabulary cleanup by making the
canonical Mob member projection seam describe bridge bindings as bridge
bindings, even though it still feeds compatibility `current_session_id`
surfaces outward.

What landed:
- `meerkat-mob/src/runtime/handle.rs`
  - `resolve_peer_connectivity(...)` now takes `bridge_session_id`
  - canonical member materialization now carries `bridge_session_id` through
    live session observation and connectivity projection instead of reusing a
    plain `session_id` local

Why this matters:
- the canonical read seam is an intentional truth boundary, so it should not
  keep teaching the old vocabulary internally
- after this slice, the builder, actor, provisioner, turn executor, and
  canonical member projection paths all agree on the bridge-session meaning of
  the live binding

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 325 - Roster authority now names bridge-session setters honestly

Goal: keep the identity-first cut consistent through the canonical Mob roster
authority so bridge-binding setters stop taking plain `session_id` locals in the
internal machine/runtime path.

What landed:
- `meerkat-mob/src/roster.rs`
  - `set_bridge_session_id(...)` now takes `bridge_session_id`
- `meerkat-mob/src/runtime/roster_authority.rs`
  - `set_bridge_session_id(...)` now forwards `bridge_session_id` explicitly

Why this matters:
- the roster/authority layer is canonical internal machine state, so it should
- use the same bridge-session vocabulary as the rest of the clean-cut runtime
- this removes another place where the new regime still looked like the old one

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 326 - Mob operator tools now assemble bridge bindings from one canonical seam

Goal: keep the clean-cut / identity-first adaptation moving through the Mob
operator tool surface by removing another pocket of hand-built compatibility
payload assembly. The tool layer should speak bridge-session truth internally
even while it still emits compatibility `session_id` keys for callers.

What landed:
- `meerkat-mob/src/runtime/tools.rs`
  - added `member_ref_result_payload(...)`
  - added `member_list_entry_result_payload(...)`
  - `spawn_meerkat`, `spawn_many_meerkats`, and `list_meerkats` now all build
    their compatibility `session_id` / `current_session_id` fields from one
    bridge-native payload seam instead of duplicating ad hoc JSON assembly
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_visible_mob_operator_tools_emit_bridge_session_fields_consistently`
    to prove an in-scope operator dispatcher still emits stable compatibility
    keys while sourcing them from canonical bridge bindings

Why this matters:
- the operator tool layer no longer has to hand-assemble duplicated bridge
  binding aliases in multiple code paths
- this keeps the internal runtime truth bridge-native while preserving the
  outward compatibility contract for existing callers

Validation:
- `cargo test -p meerkat-mob --lib test_visible_mob_operator_tools_emit_bridge_session_fields_consistently --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `git diff --check`

## Slice 327 - Mob helper and public MCP outputs now source compatibility fields from bridge-native helpers

Goal: keep the identity-first cut moving all the way to the outward helper and
public MCP edges so those surfaces stop hand-building duplicated
`session_id` / `bridge_session_id` payloads.

What landed:
- `meerkat-mob-mcp/src/agent_tools.rs`
  - added `member_ref_result_payload(...)`
  - `delegate` and `mob_spawn_member` now emit compatibility session fields
    from a bridge-native helper instead of repeating inline JSON assembly
  - extended
    `test_successful_in_scope_mutation_persists_provenance_and_denied_calls_do_not`
    to assert the compatibility field stays aligned with the canonical bridge
    binding
- `meerkat-mob-mcp/src/public_mcp.rs`
  - added `insert_bridge_session_aliases(...)`
  - added `member_ref_result_payload(...)`
  - `meerkat_mob_spawn`, `meerkat_mob_spawn_many`,
    `meerkat_mob_member_send`, and `meerkat_mob_append_system_context` now all
    source compatibility session fields from one bridge-native helper path

Why this matters:
- the runtime, helper, and public MCP layers now agree on the same bridge
  binding truth instead of each one hand-assembling compatibility aliases
- this reduces the remaining identity-first work to deeper compatibility naming
  cleanup instead of semantic drift

Validation:
- `cargo test -p meerkat-mob-mcp --lib test_successful_in_scope_mutation_persists_provenance_and_denied_calls_do_not --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `git diff --check`

## Slice 328 - Mob definition ownership is now canonical on bridge-session indexing

Goal: move the identity-first meaning down into `MobDefinition` itself so the
definition layer stops treating `owner_session_id` as the primary concept and
the public create wire contract explicitly rejects both reserved owner fields.

What landed:
- `meerkat-mob/src/definition.rs`
  - added canonical bridge-owner helpers:
    - `has_owner_bridge_session_index(...)`
    - `is_indexed_to_owner_bridge_session(...)`
    - `is_cleanup_scoped_to_owner_bridge_session(...)`
    - `mark_owner_bridge_session_indexed(...)`
    - `is_owned_by_bridge_session(...)`
    - `is_bridge_session_scoped_to(...)`
    - `mark_bridge_session_scoped(...)`
  - kept the old session-centric methods as compatibility wrappers
  - `MobDefinition::implicit(...)` now uses the bridge-native canonical helper
  - extended the additive/back-compat test so both new and old predicates are
    proven against legacy `owner_session_id` fallback
- `meerkat-mob-mcp/src/agent_tools.rs`
  - explicit mob creation now rebinds ownership through
    `mark_owner_bridge_session_indexed(...)`
- `meerkat-mob-mcp/src/lib.rs`
  - implicit lookup and cleanup/scavenge logic now use the bridge-native
    definition predicates
- `meerkat-contracts/src/wire/mob.rs`
  - the public `mob/create` contract comment now names both reserved owner
    fields explicitly
  - added
    `mob_create_params_reject_reserved_runtime_bridge_owner_field`
    so callers cannot sneak `owner_bridge_session_id` through the public schema

Why this matters:
- the runtime was already bridge-native, but the definition layer still told a
  half-old story
- now the canonical owner-binding vocabulary is bridge-native all the way down
  to the definition/helpers layer, while older session-centric method names
  remain as additive compatibility wrappers

Validation:
- `cargo test -p meerkat-mob --lib test_owner_bridge_session_index_is_additive_and_back_compatible --quiet`
- `cargo test -p meerkat-contracts --lib mob_create_params_reject_reserved_runtime_bridge_owner_field --quiet`
- `cargo test -p meerkat-mob -p meerkat-mob-mcp -p meerkat-contracts --lib --quiet`
- `git diff --check`

## Slice 329 - Runtime bridge bindings stop using compatibility MemberRef constructors

Goal: keep the full-cut identity-first story honest in production/runtime code
instead of only at helper and wire edges by switching the real bridge-binding
call sites to `MemberRef::from_bridge_session_id(...)`.

What landed:
- `meerkat-mob/src/runtime/actor.rs`
  - pending-spawn abort and resume fast-path now construct member refs from
    bridge session ids explicitly
- `meerkat-mob/src/runtime/provisioner.rs`
  - `MemberSpawnReceipt.member_ref` now comes from the canonical
    bridge-session constructor
- `meerkat-mob/src/runtime/disposal.rs`
- `meerkat-mob/src/runtime/provision_guard.rs`
- `meerkat-mob/src/runtime/ops_adapter.rs`
- `meerkat-mob/src/mob_machine.rs`
  - internal runtime/test helpers now use the same canonical constructor so
    local runtime semantics and production semantics stop drifting

Why this matters:
- the compatibility `from_session_id(...)` alias still exists, but the runtime
  path is no longer telling the old story by default
- this keeps the clean-cut/identity-first regime centered on bridge bindings
  all the way through spawn, resume, provision abort, and provision receipts

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `git diff --check`

## Slice 330 - Resume semantics become bridge-native across runtime, RPC, MCP, tools, and REST

Goal: finish the first honest identity-first resume pass by making bridge
session binding the canonical resume concept everywhere the live system accepts
resume input, while keeping additive compatibility `resume_session_id` fields
at the outward edges.

What landed:
- `meerkat-mob/src/runtime/handle.rs`
  - added canonical bridge-native resume helpers on `SpawnMemberSpec`:
    `with_resume_bridge_session_id(...)` and
    `resume_bridge_session_id()`
  - kept `with_resume_session_id(...)` and `resume_session_id()` as
    compatibility wrappers over the bridge-native helpers
  - added
    `spawn_member_spec_resume_bridge_session_accessors_stay_additive`
- `meerkat-rpc/src/handlers/mob.rs`
  - `MobSpawnParams` and `MobSpawnSpecParams` now accept additive
    `resume_bridge_session_id`
  - parsing now prefers `resume_bridge_session_id` and falls back to
    `resume_session_id`
- `meerkat-mob-mcp/src/public_mcp.rs`
  - public MCP spawn inputs/specs now accept additive
    `resume_bridge_session_id`
  - spawn-spec construction now prefers the bridge-native field
- `meerkat-mob/src/runtime/tools.rs`
  - operator tool spawn args now accept additive
    `resume_bridge_session_id`
  - schema/docs now explicitly mark `resume_bridge_session_id` as preferred
- `meerkat-mob-mcp/src/lib.rs`
  - mob MCP tool schema/help now expose additive bridge-native resume input
- `meerkat-rest/src/lib.rs`
  - helper-route test coverage now honestly reflects that helper spawn requires
    a comms-capable worker profile
  - added
    `test_mob_spawn_helper_route_returns_additive_bridge_session_id`

Why this matters:
- bridge binding is now the canonical resume identity end-to-end across the
  live runtime and the public control surfaces
- outward compatibility remains additive instead of forcing a flag day on RPC,
  MCP, REST, and operator-tool callers
- the helper-route test now matches the post-cutover truth rather than hiding
  a real comms requirement behind a minimal definition that could never spawn

Validation:
- `cargo test -p meerkat-mob --lib spawn_member_spec_resume_bridge_session_accessors_stay_additive --quiet`
- `cargo test -p meerkat-rest --lib test_mob_spawn_helper_route_returns_additive_bridge_session_id --quiet`
- `cargo test -p meerkat-mob -p meerkat-mob-mcp -p meerkat-rpc -p meerkat-rest --lib --quiet`
- `git diff --check`

## Slice 331 - Deeper resume fast-paths and validation text now tell the bridge-session story

Goal: finish the first bridge-native resume cleanup by making the live actor
fast-path and outward validation text say what the runtime already means:
resume targets bridge bindings, while `resume_session_id` remains only a
compatibility alias.

What landed:
- `meerkat-mob/src/runtime/actor.rs`
  - the internal resume fast-path now uses `resume_bridge_session_id`
    terminology end-to-end
  - the fast-path comment and failure text now explicitly describe bridge
    session validation
- `meerkat-rpc/src/handlers/mob.rs`
  - invalid resume errors now name both accepted additive fields:
    `resume_bridge_session_id/resume_session_id`
- `meerkat-mob-mcp/src/lib.rs`
  - mob MCP invalid-argument errors now name the same additive pair

Why this matters:
- the runtime and the validation/reporting layer now tell the same identity-first story
- callers still get additive compatibility, but the error text no longer teaches
  the old regime by default
- this keeps the post-cutover adaptation honest deeper than the wire/schema edge

Validation:
- `cargo test -p meerkat-mob -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `git diff --check`

## Slice 332 - Canonical bridge-binding projection is centralized for Mob member views

Goal: remove one more quiet source of compatibility drift by centralizing how
Mob member snapshots and list entries project `current_session_id` from the
canonical `current_bridge_session_id`.

What landed:
- `meerkat-mob/src/runtime/handle.rs`
  - added `MobMemberSnapshot::with_current_bridge_session_id(...)`
  - added `MobMemberListEntry::with_current_bridge_session_id(...)`
  - `project_member_list(...)` now uses the centralized helper instead of
    hand-populating both fields
  - additive serialization tests now build through the centralized helper
- `meerkat-mob/src/runtime/mob_member_lifecycle_authority.rs`
  - canonical member material now projects through the same helper
- `meerkat-mob/src/mob_machine.rs`
  - local projected-entry helpers now use the same centralized bridge-binding
    projection instead of duplicating field wiring

Why this matters:
- bridge-native binding remains the only real source of truth
- the compatibility `current_session_id` alias can no longer drift from the
  canonical field in one projection path while staying correct in another
- this shrinks the remaining identity-first cleanup surface from repeated field
  wiring to deliberate outward-compatibility choices

Validation:
- `cargo test -p meerkat-mob --lib canonical_member_material_populates_compatibility_session_alias_from_bridge_binding --quiet`
- `cargo test -p meerkat-mob --lib member_projection_types_serialize_additive_bridge_session_fields --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `git diff --check`

## Slice 333 - Launch-mode resume helpers now speak bridge-session internally

Goal: push the identity-first resume cleanup one level deeper by teaching
`MemberLaunchMode` and the actor fast-path to expose bridge-native resume
helpers instead of open-coding `Resume { session_id }` matches.

What landed:
- `meerkat-mob/src/launch.rs`
  - added
    `MemberLaunchMode::resume_bridge_session_id()`
    and additive compatibility
    `MemberLaunchMode::resume_session_id()`
  - added
    `resume_launch_mode_bridge_session_accessors_stay_additive`
- `meerkat-mob/src/runtime/handle.rs`
  - `SpawnMemberSpec::resume_bridge_session_id()` now delegates to the
    launch-mode helper instead of pattern matching directly
- `meerkat-mob/src/runtime/actor.rs`
  - spawn preparation now derives the resume fast-path from the launch-mode
    helper, keeping the actor aligned with the bridge-native launch surface

Why this matters:
- the runtime now has one canonical way to ask “is this a resume bridge
  binding?” instead of duplicating that logic in each layer
- compatibility `resume_session_id` remains additive, but the launch/runtime
  seam no longer teaches the old story internally

Validation:
- `cargo test -p meerkat-mob --lib resume_launch_mode_bridge_session_accessors_stay_additive --quiet`
- `cargo test -p meerkat-mob --lib spawn_member_spec_resume_bridge_session_accessors_stay_additive --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `git diff --check`

## Slice 334 - Direct launch-mode payloads and MCP validation now accept bridge-native resume vocabulary

Goal: make the serialized `launch_mode.resume` path and the public MCP resume
path as bridge-native as the RPC/tool layers, without breaking older
`session_id` payloads.

What landed:
- `meerkat-mob/src/launch.rs`
  - `MemberLaunchMode::Resume { session_id }` now deserializes additive
    `bridge_session_id` aliases
  - added
    `resume_launch_mode_deserializes_bridge_session_id_alias`
- `meerkat-mob-mcp/src/public_mcp.rs`
  - public MCP spawn validation now reports
    `invalid resume_bridge_session_id/resume_session_id`
    instead of the older generic session-only wording
  - added
    `build_spawn_spec_rejects_invalid_bridge_resume_aliases_with_bridge_native_message`

Why this matters:
- callers that serialize `launch_mode` directly can now speak the identity-first
  bridge vocabulary without going through the higher-level RPC/MCP wrapper fields
- the last public resume-validation surface now matches the additive bridge-native
  contract we already teach everywhere else

Validation:
- `cargo test -p meerkat-mob --lib resume_launch_mode_deserializes_bridge_session_id_alias --quiet`
- `cargo test -p meerkat-mob-mcp --lib build_spawn_spec_rejects_invalid_bridge_resume_aliases_with_bridge_native_message --quiet`
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `git diff --check`

## Slice 335 - SDK and web-runtime mob spawn ingress now accept bridge-native resume bindings

Goal: finish the identity-first resume vocabulary at the remaining caller
ingress surfaces so bridge-native resume bindings are not only true in core
runtime/RPC/MCP layers, but also in SDK and wasm-facing client entry points.

What landed:
- `sdks/typescript/src/types.ts`
  - `SpawnSpec` now carries additive `resumeBridgeSessionId`
- `sdks/typescript/src/client.ts`
  - `spawnMobMember(...)` and `spawnMobMembers(...)` now emit
    `resume_bridge_session_id` additively, while still preserving
    `resume_session_id`
- `sdks/python/meerkat/mob.py`
  - `MobSpawnSpec` and `Mob.spawn(...)` now accept additive
    `resume_bridge_session_id`
- `sdks/python/meerkat/client.py`
  - `spawn_mob_member(...)` now emits additive
    `resume_bridge_session_id`
- `meerkat-web-runtime/src/lib.rs`
  - `mob_spawn(...)` now prefers `resume_bridge_session_id` over
    `resume_session_id`
  - bridge-native resume IDs now use
    `SpawnMemberSpec::with_resume_bridge_session_id(...)`
  - added
    `spawn_spec_input_deserializes_resume_bridge_session_id_alias`

Why this matters:
- the old session-centric resume name is now consistently an additive
  compatibility field rather than the canonical story, all the way out to the
  user-facing SDKs and wasm runtime shim
- client/runtime ingress now agrees with the cutover regime instead of forcing
  callers back into old vocabulary at the edge

Validation:
- `cargo test -p meerkat-web-runtime --lib spawn_spec_input_deserializes_resume_bridge_session_id_alias --quiet`
- `cargo check -p meerkat-web-runtime --quiet`
- `git diff --check`

## Slice 336 - Mob launch mode and web runtime member helpers now treat bridge binding as canonical

Goal: keep shrinking the remaining session-centric compatibility pockets in the
identity-first regime by making the Mob launch surface and wasm mob-member
helpers internally bridge-native instead of storing bridge semantics under
legacy field names.

What landed:
- `meerkat-mob/src/launch.rs`
  - `MemberLaunchMode::Resume` now stores `bridge_session_id` canonically
  - additive compatibility is preserved with `#[serde(alias = "session_id")]`
- `meerkat-mob/src/runtime/handle.rs`
  - `with_resume_bridge_session_id(...)` and `attach_existing_session(...)`
    now construct the canonical `bridge_session_id` form
  - compatibility `with_resume_session_id(...)` remains a wrapper
- `meerkat-web-runtime/src/lib.rs`
  - `resolve_mob_member_bridge_session_id(...)` now resolves bridge bindings
    explicitly from `MemberRef::bridge_session_id()`
  - mob member append/subscribe/cross-mob helpers now use bridge-native locals
    internally while preserving outward additive `session_id` +
    `bridge_session_id` JSON
  - helper/spawn payload tests now cover additive bridge aliases honestly on the
    host-target lane

Why this matters:
- the bridge binding is now canonical in the mob launch surface itself, not
  just in higher-level helper accessors
- the remaining wasm mob-member control helpers no longer quietly translate
  through legacy `session_id()` calls internally
- compatibility names still exist at the wire edge, but they now clearly wrap
  bridge-native truth instead of owning it

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-web-runtime --lib --quiet`
- `cargo test -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `git diff --check`

## Slice 337 - Ops lifecycle bind seams now speak in bridge-owner terms

Goal: keep shrinking the remaining session-centric compatibility pockets in the
clean-cut, identity-first regime by making the shared ops lifecycle dispatcher
contract explicitly bridge-owner oriented instead of silently reusing old
`owner_session_id` locals everywhere.

What landed:
- `meerkat-core/src/ops_lifecycle.rs`
  - `OperationSpec` now explicitly documents that `owner_session_id` is the
    compatibility carrier for the canonical bridge binding
  - additive accessors `owner_bridge_session_id()` and `owner_session_id()`
    now make that meaning explicit
- `meerkat-core/src/agent.rs`
  - `AgentToolDispatcher::bind_ops_lifecycle(...)` docs and parameter names now
    speak in terms of `owner_bridge_session_id`
- `meerkat-core/src/gateway.rs`
- `meerkat-tools/src/builtin/composite.rs`
- `meerkat/src/service_factory.rs`
  - the shared dispatcher/gateway/factory bind seam now consistently threads
    `owner_bridge_session_id` instead of semantically overloaded
    `owner_session_id` locals

Why this matters:
- the identity-first Mob runtime was already feeding bridge bindings into the
  ops lifecycle registry; the trait and spec layer now say that truth out loud
- this reduces one of the last low-level semantic mismatches between the new
  bridge-session regime and the older session-centric vocabulary without
  forcing a risky ledger-shape migration in the middle of a green cutover
  branch

Validation:
- `cargo test -p meerkat-core -p meerkat-tools -p meerkat --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 338 - Shell job manager now treats ops owner binding as bridge-native

Goal: keep pushing the identity-first regime down into the execution edge by
making the background shell job manager speak in terms of canonical bridge
ownership instead of quietly carrying that meaning under old `owner_session_id`
 locals.

What landed:
- `meerkat-tools/src/builtin/shell/job_manager.rs`
  - internal owner binding is now `owner_bridge_session_id`
  - `bind_canonical_async_ops(...)` now accepts `owner_bridge_session_id`
  - background shell job registration now records that bridge-native owner
    binding into the shared ops lifecycle ledger
  - `with_owner_session_id(...)` remains only as a compatibility wrapper
  - in-file coverage now exercises the bridge-native helper directly

Why this matters:
- the shell job manager is one of the deepest live async-op producers in the
  system, so carrying bridge-native semantics there removes another meaningful
  place where the new regime was still being translated through old names
- it aligns the shell detached-op path with the earlier dispatcher bind-seam
  cleanup instead of stopping at the trait boundary

Validation:
- `cargo test -p meerkat-tools --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 339 - Implicit mob owner-index coverage now exercises bridge-session canon

Goal: stop proving the Mob MCP owner-index story mainly through compatibility
wrapper names and make the bridge-session APIs the primary exercised path.

What landed:
- `meerkat-mob-mcp/src/lib.rs`
  - implicit-mob tests now call:
    - `find_implicit_mob_for_bridge_session(...)`
    - `get_or_create_implicit_mob_for_bridge_session(...)`
    - `destroy_bridge_session_mobs(...)`
  - bridge-scoped destroy warnings now log `bridge_session_id`, not
    `session_id`
  - compatibility wrappers remain, but the canonical bridge-session API is now
    the path that in-crate coverage primarily exercises

Why this matters:
- it makes the owner-indexed implicit-mob lifecycle read honestly as
  bridge-session-scoped in the crate that owns that behavior
- it reduces the chance that future changes accidentally keep the compatibility
  wrappers alive as the de facto primary API

Validation:
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 340 - MemberRef is now bridge-native at the event wire boundary

Goal: stop teaching the old session-centric meaning at the canonical Mob event
boundary by making `MemberRef` serialize the bridge binding additively and
prefer `bridge_session_id` on read.

What landed:
- `meerkat-mob/src/event.rs`
  - `MemberRef` now serializes additive `bridge_session_id` alongside
    compatibility `session_id`
  - deserialization now accepts canonical payloads that provide either
    `bridge_session_id` or `session_id`
  - backend-peer member refs now carry the same additive bridge-session shape
  - focused event coverage now proves deterministic additive serialization,
    bridge-session-only deserialization, and backend-peer roundtrip stability

Why this matters:
- it moves the identity-first truth into the canonical Mob event/store boundary
  instead of only patching it back in at outward helper layers
- it gives the runtime one less place where bridge binding is silently
  represented under old vocabulary at the source of replay truth

Validation:
- `cargo test -p meerkat-mob --lib test_member_ref_ --quiet`
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test -p meerkat-mob-mcp -p meerkat-rpc --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 341 - Mob provisioner and restore paths now narrate bridge binding honestly

Goal: carry the identity-first bridge-session vocabulary through the live
provisioner and restore paths instead of only at the wire/helpers edge.

What landed:
- `meerkat-mob/src/runtime/provisioner.rs`
  - session-backend and external-backend provisioning now bind
    `created.session_id` into explicit `created_bridge_session_id` locals
  - runtime-registration reconciliation, registry binding, lifecycle logging,
    and `MemberSpawnReceipt` construction now speak in terms of bridge
    sessions
  - external bridge-backed members still keep compatibility `session_id` in
    the `MemberRef::BackendPeer` payload, but the runtime path now treats that
    field explicitly as bridge-session binding
- `meerkat-mob/src/runtime/builder.rs`
  - restore/rebuild paths now set roster bridge bindings through explicit
    `created_bridge_session_id` locals instead of smuggling the same meaning
    through unnamed compatibility vocabulary

Why this matters:
- it aligns the deepest live runtime provisioning path with the new
  bridge-native `MemberRef` wire semantics
- it reduces one of the last meaningful places where the clean-cut regime was
  still internally narrating bridge binding as generic session identity

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo check --workspace --all-targets --quiet`
- `git diff --check`

## Slice 342 - Mob MCP scavenging and cleanup now exercise the bridge-session canon

Goal: stop letting internal Mob MCP cleanup/scavenge coverage teach the old
session-scoped vocabulary when the runtime already owns this seam as
bridge-session-scoped cleanup.

What landed:
- `meerkat-mob-mcp/src/lib.rs`
  - internal cleanup/scavenge doc strings now point at
    `destroy_bridge_session_mobs(...)`
  - the canonical bridge-session scavenger path is what the in-crate tests now
    exercise directly
  - test names and assertions now say `bridge-session-scoped` instead of
    `session-scoped` where that is the real meaning

Why this matters:
- it makes the cleanup/scavenge coverage read like the runtime we actually
  have, instead of like a compatibility alias that accidentally became the
  primary story
- it reduces the chance that future bridge-owner cleanup changes get wired to
  the old wrapper names first

Validation:
- `cargo test -p meerkat-mob-mcp --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

## Slice 343 - Respawn receipts are now bridge-native inside the runtime

Goal: stop building replacement lifecycle receipts through compatibility
`old_session_id` / `new_session_id` naming inside the Mob actor/handle seam.

What landed:
- `meerkat-mob/src/runtime/handle.rs`
  - `MemberRespawnReceipt::from_bridge_session_ids(...)` is now the canonical
    constructor
  - bridge-native setter helpers keep the compatibility fields mirrored at the
    edge without making them the internal authoring surface
- `meerkat-mob/src/runtime/actor.rs`
  - respawn success and topology-restore-failure paths now construct receipts
    with bridge-session-native helpers instead of hand-filling both the old
    and canonical fields
- focused receipt coverage now uses the bridge-native constructor too

Why this matters:
- respawn/replacement is exactly where identity-first mobs should read most
  clearly as bridge-binding continuity plus semantic replacement
- it removes another deep runtime path where the clean-cut regime still taught
  the compatibility fields as the primary lifecycle truth

Validation:
- `cargo test -p meerkat-mob --lib --quiet`
- `cargo test --workspace --tests --quiet`
- `git diff --check`

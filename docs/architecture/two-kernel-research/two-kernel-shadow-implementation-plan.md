# Two-Kernel Shadow Implementation Plan

Status: active pre-cutover implementation plan

This note turns the shadow-validation plans and hook inventories into the first
concrete implementation plan.

The question here is no longer “what should we compare?” It is:

- which files do we patch first
- what helper surfaces do we add
- what output should those helpers produce

## Output shape

The first shadow implementation should emit structured mismatch records with:

- `lane`
- `region`
- `entity_id` when applicable
- `expected_summary`
- `observed_summary`
- `triage`

Where `triage` is one of:

- `implementation_detail`
- `semantic_gap`
- `dogma_violation`

## Meerkat first wave

### Host files

- `meerkat/src/meerkat_machine.rs`
- `meerkat-runtime/src/meerkat_machine.rs`
- `meerkat-core/src/agent/runner.rs`

### First helpers

1. `capture_meerkat_shadow_report(...)`
   - wraps `capture_meerkat_machine_snapshot(...)`
   - runs lane-specific comparisons
   - returns typed mismatch records

2. lifecycle/control comparator
   - built from `meerkat_machine_spine_snapshot(...)`
   - checks teardown/current-run/queue/completion invariants at shadow time

3. tools comparator
   - built from `tool_scope_snapshot()` +
     `external_tool_surface_snapshot()`
   - checks visibility/surface revision coherence

### First scenarios

- plain `reset`
- attached `recover`
- tool visibility mutation
- MCP staged apply/finalize

## Mob first wave

### Host files

- `meerkat-mob/src/mob_machine.rs`
- `meerkat-mob/src/runtime/handle.rs`
- `meerkat-mob/src/runtime/actor.rs`

### First helpers

1. `capture_mob_shadow_report(...)`
   - wraps `capture_mob_machine_snapshot(...)`
   - runs lane-specific comparisons

2. provisioning/lifecycle comparator
   - compares pending spawn, kickoff, restore failure, roster presence

3. flow/frame/loop comparator
   - compares tracked run shape against active actor/run tracker truth

### First scenarios

- spawn/finalize
- retire
- destroy
- single-step flow
- branch-fallback flow

## Seam first wave

### Host files

- `meerkat-mob/src/runtime/actor.rs`
- `meerkat-runtime/src/meerkat_machine.rs`
- `meerkat/src/meerkat_machine.rs`
- `meerkat-mob/src/mob_machine.rs`

### First helpers

1. `capture_composition_shadow_report(...)`
   - compares current Mob-visible member/runtime state against current Meerkat
     lifecycle/work state through the frozen seam abstraction

2. lifecycle bridge comparator
   - provision / ready / retiring / destroyed / failed

3. supersession/fencing comparator
   - runtime identity and stale-event handling

### First scenarios

- fresh provision
- retire with replacement provisioning
- destroy in flight
- flow-step work completion

## Recommended implementation order

1. Meerkat lifecycle/control shadow report
   - status: landed in `meerkat/src/meerkat_machine.rs`
2. Meerkat tools shadow report
   - status: landed in `meerkat/src/meerkat_machine.rs`
3. Mob provisioning/lifecycle shadow report
   - status: landed in `meerkat-mob/src/mob_machine.rs`
4. Seam lifecycle/supersession shadow report
   - status: landed in `meerkat-mob/src/mob_machine.rs`
5. Mob flow/frame/loop shadow report
   - status: landed in `meerkat-mob/src/mob_machine.rs`
6. Meerkat turn/ops/barrier shadow report
   - status: landed in `meerkat/src/meerkat_machine.rs`
7. Meerkat peer/drain shadow report
   - status: landed in `meerkat/src/meerkat_machine.rs`
8. Mob task/history/recovery shadow report
   - status: landed in `meerkat-mob/src/mob_machine.rs`
9. Seam work bridge shadow report
   - status: landed in `meerkat-mob/src/mob_machine.rs`
10. Aggregate scenario suite helpers
   - status: landed in:
     - `meerkat/src/meerkat_machine.rs`
     - `meerkat-mob/src/mob_machine.rs`
11. Mob-side best-effort Meerkat diagnostic bridge
   - status: landed in:
     - `meerkat-mob/src/runtime/session_service.rs`
     - `meerkat-mob/src/runtime/handle.rs`
     - `meerkat-mob/src/runtime/tests.rs`
   - note:
     - this is intentionally best-effort
     - real session services may surface execution/tool snapshots
     - mock-backed runtime-adapter services may legitimately return `None`

## Exit criteria

This plan is complete enough to start code once:

- each first-wave host file has one named helper target
- each first-wave scenario is tied to one machine/seam lane
- mismatch output shape is stable enough to reuse across both kernels

The next implementation step after first-wave lanes is:

- use the aggregate suite helpers to run one real scenario end to end
- use `two-kernel-shadow-scenario-matrix.md` as the scenario source of truth
- Scenario 1 is now landed for Meerkat (`plain reset with pending ops`)
- Scenario 3 is now landed for Meerkat (`tool visibility mutation + MCP apply`)
- Scenario 4 is now landed for Meerkat (`peer ingress receive / trust mutation / drain`)
- Scenario 5 is now landed for Mob (`provision / kickoff / finalize`)
- Scenario 6 is now landed for Mob (`single-step flow run`)
- the settled replacement-provisioning branch of Scenario 8 is now landed for
  Mob (`respawn supersession`)
- the destroy-in-flight branch of Scenario 8 is now landed for Mob
  (`destroy in-flight flow`)
- both attached replay branches of Scenario 7 are now landed for Meerkat:
  - recover replay
  - recycle replay
- the branch-fallback failure-history run is now landed for Mob
- the next step is no longer to land the first-wave matrix
- taxonomy summarizers over aggregate suite reports are now landed
- the first broader Meerkat smoke run is now landed and currently produces an
  empty mismatch taxonomy on the rebased baseline
- the first broader Mob + seam smoke run is now landed and currently produces
  an empty mismatch taxonomy on the rebased baseline
- seeded taxonomy validation is now landed for:
  - `MeerkatMachine` lifecycle/control drift
  - `MobMachine` provisioning drift plus seam work-bridge drift
- the first mismatch-producing live taxonomy run is now landed for the seam:
  - archive the live bridge session under active single-step flow
  - expect real taxonomy buckets in:
    - `composition / LifecycleSupersession / lifecycle`
    - `composition / WorkBridge / work`
- the shared first sink/export shape is now frozen in:
  - `two-kernel-shadow-sink-schema.md`
- both kernels can now emit that shared sink shape
- the first combined cutover-facing runner is now landed for:
  - `seam.live_bridge_loss`
  - active phase: empty Meerkat sample + empty Mob sample
  - `post_archive` phase: empty Meerkat sample + non-empty Mob/seam sample
- the first reusable batch consumer is now landed on the Mob side:
  - pair optional Meerkat sample + Mob sample into one normalized sink sample
  - collect multiple normalized samples under one `run_id`
- the first reusable multi-scenario run batch is now landed on the Mob side:
  - `run_id = "shadow.cutover.smoke"`
  - scenario batch `0`: one green `mob.flow.single_step.green` sample
  - scenario batch `1`: one mismatch-producing `seam.live_bridge_loss` sample
- the first shared report/session layer is now landed on the Mob side:
  - `session_id = "shadow.cutover.session"`
  - run batch `0`: `shadow.cutover.smoke`
  - run batch `1`: `shadow.cutover.green-only`
- the first genuinely multi-run live report session is now landed:
  - `session_id = "shadow.cutover.multi-run"`
  - run batch `0`: one healthy runtime-backed run
  - run batch `1`: one drift-producing runtime-backed run
- the first session-level triage summary is now landed:
  - top-level counts over the multi-run live report session
  - separates green samples from mismatch-producing samples
  - preserves per-scope bucket totals (`meerkat`, `mob`, `composition`)
- only now add a longer-lived sink/export path if broader shadow runs start
  surfacing real mismatch taxonomy that needs durable storage or external
  surfacing

## Read with

- `meerkat-shadow-validation-plan.md`
- `mob-shadow-validation-plan.md`
- `mob-meerkat-seam-shadow-checks.md`
- `meerkat-shadow-hook-inventory.md`
- `mob-shadow-hook-inventory.md`
- `mob-meerkat-seam-hook-inventory.md`
- `two-kernel-shadow-scenario-matrix.md`

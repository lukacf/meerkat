# Two-Kernel Refinement Program

Status: active pre-cutover program

This note turns the current machine/proof posture into one explicit execution
plan:

- `MeerkatMachine` is frozen and mechanically checked
- `MobMachine` is frozen and mechanically checked
- the `MobMachine <-> MeerkatMachine` seam is frozen and mechanically checked

The remaining work is no longer semantic freeze authoring. It is refinement,
lowering, and staged cutover preparation against the real implementation.

## Current Proven Baseline

The current rebased branch has:

- a frozen target `MeerkatMachine`
- a frozen target `MobMachine`
- a frozen target seam composition package
- green workspace compile and test lanes
- green canonical TLC safety/liveness lanes for:
  - `MeerkatMachine`
  - `MobMachine`
  - `MobMachine <-> MeerkatMachine` seam

That means the question is no longer "what is the machine?" The question is
"how does the implementation refine to the machine, and how do we switch
authority without hidden shell truth surviving?"

## Remaining Work Classes

There are four remaining work classes.

### R1. Exact-current refinement

Show how the current implementation lowers into the frozen target machines and
the frozen seam.

Needed outputs:

- exact-current -> target delta review for `MeerkatMachine`
- exact-current -> target delta review for `MobMachine`
- exact-current -> target seam delta review
- implementation obligations classified as:
  - accepted target cleanup
  - required implementation work
  - rejected target overreach

### R2. Input/effect lowering

Turn named machine inputs/effects into real implementation seams.

Needed outputs:

- function-to-input map for every cutover-relevant runtime/orchestration action
- effect-to-code-path map for every cutover-relevant effect
- no remaining major behavior that exists only as helper convention or shell
  folklore

### R3. Shadow authority preparation

Prepare the system to validate the machines against the real implementation
before write-side switch.

Needed outputs:

- short-lived shadow-read/shadow-validate hooks
- mismatch classification against machine state and seam state
- explicit failure triage using:
  - implementation detail
  - semantic gap
  - dogma violation

### R4. Kernel cutover program

Prepare the actual switch from old shell authority to machine authority.

Needed outputs:

- `MeerkatMachine` cutover plan
- `MobMachine` cutover plan
- old-authority removal checklist
- rollback conditions

## Recommended Execution Order

### Step 1. Re-freeze the hot exact-current seam after upstream drift

The current upstream drift is concentrated in `MeerkatMachine.tools`.

Use:

- `meerkat-upstream-tool-alignment.md`
- `meerkat-tool-visibility-upstream-baseline.md`
- `meerkat-tools-realignment-plan.md`
- `meerkat-tools-merge-strategy.md`
- `meerkat-tools-target-delta.md`

Goal:

- make the exact-current `MeerkatMachine.tools` story match the rebased branch
- keep target-state `tool_visibility + tool_surface` growth explicit

This is now recorded in:

- `meerkat-tool-visibility-freeze.md`
- `meerkat-tool-surface-freeze.md`

### Step 2. Write the seam refinement delta

The seam is now frozen and proven. The next step is to compare the current code
against it, not to reopen the seam.

Goal:

- enumerate all exact-current places where:
  - `SessionId` still leaks
  - raw turn-delivery paths still stand in for `WorkSpec`
  - lifecycle/work lowering is implicit rather than explicit

This is now recorded in:

- `mob-meerkat-composition-refinement-delta.md`

### Step 3. Build Meerkat cutover lowering map

For `MeerkatMachine`, move from proof package to cutover-ready lowering.

Goal:

- identify the runtime/session/service functions that become:
  - machine inputs
  - machine-owned effects
  - compatibility shims to be deleted later

This is now grounded by:

- `meerkat-cutover-lowering-inventory.md`

### Step 4. Build Mob cutover lowering map

For `MobMachine`, do the same for:

- roster and lifecycle
- provisioning and kickoff
- work and flow-step dispatch
- recovery and task cleanup

This is now grounded by:

- `mob-cutover-lowering-inventory.md`

### Step 5. Shadow-validate both kernels and the seam

Goal:

- run the frozen machines as validators over the live system
- collect mismatches as machine/seam deltas, not ad hoc runtime bugs

This is now grounded by:

- `meerkat-shadow-validation-plan.md`
- `mob-shadow-validation-plan.md`
- `mob-meerkat-seam-shadow-checks.md`
- `meerkat-shadow-hook-inventory.md`
- `mob-shadow-hook-inventory.md`
- `mob-meerkat-seam-hook-inventory.md`
- `two-kernel-shadow-implementation-plan.md`

### Step 6. Hard-cut `MeerkatMachine`, then `MobMachine`

Order:

1. hard-cut `MeerkatMachine`
2. stabilize and classify failures
3. hard-cut `MobMachine`

This keeps attribution tractable and preserves the design rule that Mob should
see only the seam, not hidden Meerkat internals.

## Non-goals

This program does not reopen:

- the `MeerkatMachine` target freeze
- the `MobMachine` target freeze
- the frozen seam contract

Those are reopened only if proof or implementation contact uncovers a real
semantic contradiction.

## Immediate Next Step

The immediate next step is:

1. use the landed aggregate suite helpers as the shared shadow-report
   collection path for one real scenario:
   - `capture_all_meerkat_shadow_reports(...)`
   - `capture_mob_shadow_suite_report(...)`
   - optionally enrich Mob-side scenario probes with
     `diagnostic_meerkat_shadow_inputs(...)` when the underlying session
     service owns canonical live diagnostic surfaces
2. wire the first cutover-facing shadow run around:
   - Meerkat lifecycle/control + tools + turn/ops/barrier + peer/drain
   - Mob provisioning/lifecycle + flow/frame/loop + task/history/recovery
   - seam lifecycle/supersession + work bridge
3. keep `two-kernel-shadow-implementation-plan.md` as the active patch-order
   plan for any remaining lane additions
4. add a shared sink/export path only after the aggregate scenario helpers start
   surfacing real mismatch classes during live runs
5. treat newly surfaced mismatches as refinement work, not as proof/freeze
   uncertainty, unless they force a real semantic backtrack

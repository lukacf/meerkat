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
- Removed the unreachable top-level Meerkat `Recovering` phase and its self-loops from the formal model while leaving the internal runtime-control `Recovering` phase and public `RuntimeState` enum unchanged.
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
- Hardened the Mob parity harness so representative-state guard evaluation now
  covers all five remaining formal Mob core fields, including
  `wiring_edge_count`; the stricter audit rerun stayed exact, which means the
  remaining core split is not an artifact of a missing guard input in the
  checker.
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
- Taught the generated closed-world composition models to reject queued
  external entry packets that are no longer admissible for the current machine
  state, which removes seam deadlocks without widening the machine transition
  graphs.
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
| MeerkatMachine | TLC generated states | 102,047 | 105,855 | +3,808 |
| MeerkatMachine | TLC distinct states | 3,959 | 3,959 | 0 |
| MeerkatMachine | TLC depth | 9 | 9 | 0 |
| MobMachine | Phases | 5 | 4 | -1 |
| MobMachine | Inputs | 39 | 39 | 0 |
| MobMachine | Surface-only inputs | 0 | 12 | +12 |
| MobMachine | Signals | 79 | 31 | -48 |
| MobMachine | Transitions | 210 | 82 | -128 |
| MobMachine | TLC generated states | 452,835 | 26,484 | -426,351 |
| MobMachine | TLC distinct states | 6,401 | 813 | -5,588 |
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
| Mob `retiring_member_count` should not stay as top-level formal state | passed / landed | Removed the dead retire counter; exact Mob parity stayed green and the truthful Hopcroft/TLC readout stayed flat at `770 -> 138 / 140 / 770`, proving the counter was not carrying independent formal behavior. |
| Mob public `Stop` should reject active flows | passed / landed | Added `no_active_runs` to `StopRunning` after a focused runtime/schema probe showed `handle.stop()` rejects while flows are still active; the lifecycle-triangle parity audit stayed exact and truthful TLC generated states fell from `25,943` to `25,767` with the quotient unchanged. |
| Mob bootstrap should start with coordinator bound | passed / landed | Changed the formal init state from `Running + coordinator_bound=false` to `Running + coordinator_bound=true` to match the live runtime bootstrap snapshot; the lifecycle-triangle parity audit stayed exact and the truthful quotient held at `138 / 140` while reachable states rose from `770` to `813`, proving the old bootstrap state had been under-modeled rather than behavior-bearing. |
| Meerkat `Recovering` is a transient / no-op top-level phase | passed / landed | Removed the unreachable top-level `Recovering` phase from the formal model; the internal `RuntimeControlAuthority` still owns recovery/recycle transitions, and TLC state space stayed unchanged. |
| Meerkat pure queries should stay surfaced without formal transitions | passed / landed | Moved the read-only helper/query family into `surface_only_inputs`; runtime/schema audits stayed green and Meerkat TLC generated states dropped from 3,668,832 to 3,113,272 while the raw/phase quotients stayed at 385 / 390. |
| Meerkat committed visibility publication progress should not stay as top-level shadow state | passed / landed | Removed `committed_visibility_revision` from the formal state; exact audited parity stayed green and Meerkat TLC distinct states fell from 59,371 to 45,610 while the raw/phase quotients stayed at 385 / 390. |
| Meerkat visibility witness provenance should not stay as top-level shadow state | passed / landed | Removed `requested_witnesses` and `filter_witnesses` from the formal state; exact audited parity stayed green and Meerkat TLC distinct states fell from 45,610 to 15,809 while the raw/phase quotients still held at 385 / 390. |
| Meerkat LLM/capability projection should not stay as top-level shadow state | passed / landed | Removed `current_llm_identity`, `current_capability_surface`, `capability_surface_status`, `capability_base_filter`, and `inherited_base_filter` from the formal state; exact audited parity stayed green and Meerkat TLC distinct states fell from 15,809 to 11,814 while the raw/phase quotients still held at 385 / 390. |
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
| MobMachine | `none` | 813 | 138 | 83.0% | After the bootstrap parity correction, the truthful graph grew slightly while the raw quotient stayed flat, confirming the old `coordinator_bound=false` init was under-modeled bootstrap truth rather than behavior-bearing structure. |
| MobMachine | `phase` | 813 | 140 | 82.8% | Preserving phase still adds only two quotient blocks; `Running` / `Stopped` / `Completed` remain mostly projection. |
| MobMachine | `full` | 813 | 813 | 0.0% | Once the remaining authoritative counters are preserved, every reachable Mob snapshot is still distinct. |
| MeerkatMachine | `none` | 11,814 | 385 | 96.7% | Raw behavior still collapses to a much smaller machine after the exact parity pass and the visibility plus LLM/capability boundary simplifications. |
| MeerkatMachine | `phase` | 11,814 | 390 | 96.7% | Preserving phase still adds only five quotient blocks; phase remains almost entirely projection here too. |
| MeerkatMachine | `full` | 11,814 | 11,425 | 3.3% | Preserving the full snapshot still keeps almost every remaining state distinct, but the remaining shadow projection layers are materially smaller. |

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
- **MeerkatMachine** has one dominant mixed block of `4,669` states spanning
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
[`docs/architecture/meerkat-runtime-schema-parity-ledger.md`](meerkat-runtime-schema-parity-ledger.md).

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
[`docs/architecture/mob-runtime-schema-parity-ledger.md`](mob-runtime-schema-parity-ledger.md).

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

- reachable states: `813`
- raw quotient states: `138`
- phase-observed quotient states: `140`
- full-observed quotient states: `813`
- TLC: `26,484 generated / 813 distinct / depth 7`
- dominant mixed block: `362` states spanning `Running`, `Stopped`, and
  `Completed`
- dominant block tuples: `205`
- tuples reused across multiple phases: `86`
- maximum phases sharing one tuple: `3`

The dominant Mob block is now being split primarily by the remaining
runtime-backed lifecycle/orchestration counters:

- `pending_spawn_count`
- `wiring_edge_count`
- `active_run_count`
- `active_member_count`
- `coordinator_bound`

The important read is that the raw quotient has continued to fall only when we
removed formal distinctions that were still present in the truthful state
graph. The reachable space is now down from `4,797` to `813`, while the raw
quotient is down to `138`. The most recent init-state correction increased the
reachable space because the old bootstrap truth was wrong, but it did not
increase the intrinsic quotient. That means the remaining Mob state is now much
closer to the real lifecycle/counter core than to the earlier seam-shadow
projection layer.

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
`Running + coordinator_bound=false` to `Running + coordinator_bound=true`, so
the current readout is:

- reachable states: `813`
- raw quotient states: `138`
- phase-observed quotient states: `140`
- full-observed quotient states: `813`
- TLC: `26,484 generated / 813 distinct / depth 7`

The key reading did not change. The machine is no longer spending state-space
budget replaying a representative member/runtime identity at the top level,
and the remaining mixed block is now dominated by lifecycle and
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

- reachable states: `11,814`
- raw quotient states: `385`
- phase-observed quotient states: `390`
- full-observed quotient states: `11,425`
- TLC: `598,901 generated / 11,814 distinct / depth 9`
- reachable edges: `346,958`
- dominant mixed block: `4,669` states spanning `Initializing`, `Idle`,
  `Attached`, `Running`, `Retired`, and `Stopped`
- dominant block tuples: `1,971`
- tuples reused across multiple phases: `1,566`
- maximum phases sharing one tuple: `5`

The important read is now sharper than the earlier partial dump story:

- the raw quotient is much smaller than the reachable state space
- phase preservation adds only `5` blocks (`385 -> 390`)
- preserving the full snapshot still keeps almost every remaining state distinct
  (`11,425`)
- the visibility-boundary and LLM/capability-boundary cuts removed a large
  amount of formal shadow state without changing the audited public-phase
  behavior

That means the remaining Meerkat complexity is carried overwhelmingly by the
field tuple, not by the phase label.

### Largest Meerkat mixed-block projection

We then extended the always-on Hopcroft summary to project the largest
mixed-phase block onto its extended-state fields. On the current Meerkat run,
that yields:

- dominant mixed block: `4,669` states
- distinct extended-state tuples inside that block: `1,971`
- tuples reused across multiple phases: `1,566`
- maximum phases sharing one tuple: `5`

That is the strongest concrete evidence so far that the current phase surface
is layered on top of a smaller field-driven machine instead of acting as the
primary semantic axis. The most discriminating fields inside the block are now:

- `staged_visibility_revision` (`8` value buckets; largest bucket `1,141`)
- `active_visibility_revision` (`3` buckets; largest bucket `1,866`)
- `pre_run_phase` (`3` buckets; largest bucket `2,436`)
- `attachment_live` (`2` buckets; split `2,675 / 1,994`)
- `peer_ingress_configured` (`2` buckets; split `2,768 / 1,901`)
- `staged_requested_deferred_names` (`2` buckets; split `2,904 / 1,765`)
- `active_requested_deferred_names` (`2` buckets; split `3,079 / 1,590`)
- `active_fence_token` (`2` buckets; split `3,128 / 1,541`)

The read is strikingly consistent with the earlier field-ablation pass: the
largest Meerkat block is now dominated by visibility-staging, runtime binding,
ingress, and pre-run restoration dimensions rather than by phase labels.

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

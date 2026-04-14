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
| MobMachine | TLC generated states | 452,835 | 169,282 | -283,553 |
| MobMachine | TLC distinct states | 6,401 | 4,797 | -1,604 |
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
| Meerkat `Recovering` is a transient / no-op top-level phase | passed / landed | Removed the unreachable top-level `Recovering` phase from the formal model; the internal `RuntimeControlAuthority` still owns recovery/recycle transitions, and TLC state space stayed unchanged. |
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

The command asks TLC to `-dump dot,actionlabels,snapshot` for the checked-in
machine spec, parses the reachable state graph, and runs a Hopcroft-style
partition refinement over the labeled transition system. The `--audit-map`
extension keeps the same TLC dump, then layers two extra diagnostics on top:

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
| MobMachine | `none` | 4,797 | 195 | 95.9% | Raw behavior is dramatically smaller than the current formal state space. |
| MobMachine | `phase` | 4,797 | 197 | 95.9% | Preserving phase barely changes the quotient; `Running` / `Stopped` / `Completed` are almost entirely projection. |
| MobMachine | `full` | 4,797 | 4,797 | 0.0% | Once the full extended state is preserved, every reachable snapshot is distinct. |
| MeerkatMachine | `none` | 4,144 | 199 | 95.2% | Raw behavior again collapses to a machine of roughly the same order as Mob. |
| MeerkatMachine | `phase` | 4,144 | 204 | 95.1% | Preserving phase barely changes the quotient; phase is not the main source of complexity. |
| MeerkatMachine | `full` | 4,144 | 4,144 | 0.0% | As with Mob, the entire present surface lives in the extended state tuple. |

### What the quotient is telling us

- The current machines are **well-defined transition systems**, but they are
  **poorly normalized phase machines**.
- Phase is almost entirely a projection layer in both kernels. The raw
  behavioral quotient and the phase-preserved quotient are nearly identical.
- The machine surface is therefore encoded primarily in the extended state
  fields and their guards, not in the phase labels themselves.
- Preserving the full snapshot eliminates all quotienting, which means the next
  simplification wave should focus on the field structure and the guard set
  rather than on the phase enum alone.

### Mixed-phase blocks

The raw quotient exposes the highest-signal parity/simplification targets:

- **MobMachine** has one dominant mixed block of `2,819` states spanning
  `Running`, `Stopped`, and `Completed`.
- **MeerkatMachine** has one dominant mixed block of `2,094` states spanning
  `Initializing`, `Idle`, `Attached`, `Running`, `Retired`, and `Stopped`.

These are not automatic merge approvals. They are a **schema-side lower bound**
on the machine’s intrinsic complexity.

### Interpretation discipline

The quotient does **not** prove the runtime is only `195` / `199` states. It
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

- Audited pairs: `Attached <-> Idle`, `Running <-> Stopped`, `Idle <-> Retired`
- Interesting schema rows in scope: `76`
- Runtime probes implemented: `53`
- Schema/runtime alignments: `53`
- Schema/runtime mismatches: `0`
- Remaining unprobed rows: `23`

The open remainder is now concentrated in the reducer/control family:
`Abort*`, `Accept*`, `LoadBoundaryReceipt`, `Prepare/Commit/Fail`,
`PublishEvent`, `RuntimeState`, `Recycle`, and similar reducer-only carriers.

### What is now aligned

The Meerkat parity pass has already closed three concrete mismatch families:

- registration-helper surface
- durable tool-visibility mutation
- helper/query surface (`EnsureSessionWithExecutor`, `SetSilentIntents`,
  `ContainsSession`, `SessionHasExecutor`, `SessionHasComms`,
  `OpsLifecycleRegistry`, `InputState`, `ListActiveInputs`)

The probe also still confirms several genuinely load-bearing distinctions:

- `ReconfigureSessionLlmIdentity` remains phase-gated: `Attached` and
  `Running` accept it, while `Idle`, `Retired`, and `Stopped` reject it.
- `InterruptCurrentRun` and `CancelAfterBoundary` remain meaningfully
  `Running`-only relative to `Stopped`.
- `Retire`, `Recover`, `SetPeerIngressContext`, and `NotifyDrainExited` all
  still line up with the current schema surface for the audited pairs.

### What remains open

The Meerkat audit front is no longer “resolve known mismatches.” It is now
“finish probe coverage.”

The current open rows all sit in the reducer/control family, where genuine
phase distinctions are still most likely to be load-bearing:

- `Abort`
- `AbortAll`
- `Wait`
- `Ingest`
- `PublishEvent`
- `RuntimeState`
- `LoadBoundaryReceipt`
- `AcceptWithCompletion`
- `AcceptWithoutWake`
- `Recycle`
- `Prepare`
- `Commit`
- `Fail`

### Reading the current pass

The Meerkat audit has shifted from parity triage to parity completion:

- the rows we have probed are currently green
- the remaining uncertainty is probe coverage, not a known schema/runtime split
- once the reducer/control family is probed, we can rerun Hopcroft against a
  materially cleaner acceptance surface and use that result to steer the DSL
  work

### Audit-map results

The first `--audit-map` pass answers two more concrete questions:

1. Which extended-state fields move the quotient the most?
2. Which mixed-phase pairs already have obvious schema-surface deltas before we
   even look at runtime?

#### Field-ablation leaders

- **MeerkatMachine** most quotient-bearing fields today are
  `requested_witnesses` (`all_except` collapses `2,647` states),
  `filter_witnesses` (`2,493`), `staged_visibility_revision` (`2,350`),
  `staged_requested_deferred_names` (`2,111`), `drain_running` (`2,111`), and
  `peer_ingress_configured` (`1,885`).
- **MobMachine** most quotient-bearing fields today are
  `event_subscription_count` (`2,596`), `task_count` (`2,581`),
  `wiring_edge_count` (`2,219`), `coordinator_bound` (`2,182`),
  `pending_spawn_count` (`2,113`), and then the active-work / run counters.

This is the first concrete sign that the next simplification wave should be
field-normalization-first, not phase-first.

#### Pair-audit highlights

- **Meerkat `Attached` ↔ `Idle`**:
  `same=6`, `different=7`, `left-only=10`, `right-only=10`.
  The first reachable witness differs on `active_visibility_revision`,
  `committed_visibility_revision`, `drain_running`, and `requested_witnesses`.
  The first non-`same` schema rows are already useful parity leads:
  `RegisterSession` / `UnregisterSession` are `right-only`, while
  `ReconfigureSessionLlmIdentity` is `left-only`, and
  `SetPeerIngressContext`, `NotifyDrainExited`, `StagePersistentFilter`,
  `RequestDeferredTools`, and `PublishCommittedVisibleSet` are all
  `different_surface`.
- **Meerkat `Running` ↔ `Stopped`**:
  `same=1`, `different=4`, `left-only=18`, `right-only=3`.
  The witness differs on deferred-tool state, visibility revisions,
  `drain_running`, `filter_witnesses`, and `peer_ingress_configured`.
  The first non-`same` rows put the likely runtime probes in plain view:
  `PrepareBindings`, `SetPeerIngressContext`, `NotifyDrainExited`,
  `InterruptCurrentRun`, and `CancelAfterBoundary`.
- **Mob `Running` ↔ `Stopped`**:
  `same=3`, `different=8`, `left-only=15`, `right-only=1`.
  The witness differs only on `event_subscription_count`, but the schema rows
  show the lifecycle split very clearly: `Spawn`, `SubmitWork`, `RunFlow`,
  `Respawn`, and `Wire` are `left-only`, while `Retire` / `RetireAll` are
  `different_surface`.
- **Mob `Completed` ↔ `Running`** and **`Completed` ↔ `Stopped`**:
  `Completed` is behaviorally merged into the same raw quotient block, but the
  static schema rows show it is mostly the query/subscription/provenance shell
  while `Running` / `Stopped` still own the lifecycle-mutating inputs.

These counts are **not** runtime truth yet. They are the new audit checklist.
Every non-`same` row is now a concrete place to ask: "does runtime agree with
the schema here?" Every `same` row inside a mixed-phase block is a genuine
simplification candidate if runtime also agrees.

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

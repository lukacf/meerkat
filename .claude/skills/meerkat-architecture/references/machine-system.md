# Machine System — DSL, Schemas, Kernels, TLA+, Runtime

Load this reference when working on DSL definitions, schema catalog, generated kernels, TLC verification, or authority cutover.

## The 4-machine target

The final system has exactly four canonical machines:

- **MeerkatMachine** — session-scoped execution kernel (absorbs input lifecycle, runtime ingress, ops lifecycle, turn execution, comms drain, peer comms, external tool surface, session turn admission)
- **MobMachine** — mob-scoped orchestration (absorbs mob lifecycle, member bootstrap, member lifecycle, wiring, roster, orchestrator, flow, loop iteration)
- **ScheduleLifecycleMachine** — perimeter scheduler
- **OccurrenceLifecycleMachine** — perimeter occurrence lifecycle

Plus four composition protocols at the seams: `meerkat_mob_seam`, `schedule_bundle`, `schedule_runtime_bundle`, `schedule_mob_bundle`.

Catalog authoritative file: `meerkat-machine-schema/src/catalog/dsl/` — contains exactly these four machine DSLs.

Previously-standalone machines absorbed into MeerkatMachine/MobMachine state become fields, inputs, and transitions inside the host machine. Post-absorption, no `*_authority.rs` files should remain for absorbed domains — shell code routes through the host machine's DSL via handle traits (see "Cross-crate DSL access" below).

## The two-compilation model

The DSL is a single source that produces two artifacts:

```
┌────────────────────────────────────────────────────────────────────┐
│  DSL SOURCE (single source of truth, 4 files)                      │
│  meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs         │
│  meerkat-machine-schema/src/catalog/dsl/mob_machine.rs             │
│  meerkat-machine-schema/src/catalog/dsl/schedule_lifecycle.rs      │
│  meerkat-machine-schema/src/catalog/dsl/occurrence_lifecycle.rs    │
│                                                                    │
│  machine_dsl! {                                                    │
│    state { <fields> }                                              │
│    phase <Name> { <variants> }                                     │
│    input <Name> { <parameterized variants> }                       │
│    effect <Name> { <parameterized variants> }                      │
│    signal <Name> { <parameterless variants> }                      │
│    transition <Name> {                                             │
│      on input <Variant> { <params> }                               │
│      guard "<name>" { <predicate over self.<field>> }              │
│      update { self.<field> = <expr>; ... }                         │
│      emit <Effect>                                                 │
│    }                                                               │
│  }                                                                 │
└──────────────────┬────────────────────────────┬────────────────────┘
                   │                            │
                   │ rust proc macro            │ schema extraction at
                   │ expansion at compile time  │ codegen time (xtask)
                   ▼                            ▼
┌──────────────────────────────┐  ┌──────────────────────────────────┐
│  RUNTIME KERNEL (Rust)       │  │  MachineSchema (descriptive)     │
│  (generated in-place into    │  │  Vec<MachineSchema> registered   │
│   each catalog file)         │  │  via canonical_machine_schemas() │
│                              │  │  in catalog/mod.rs               │
│  mm_dsl::MeerkatMachineState │  │                                  │
│  mm_dsl::MeerkatMachineInput │  │  xtask machine-codegen --all     │
│  mm_dsl::MeerkatMachine      │  │    → .tla spec files             │
│    Authority                 │  │  xtask machine-check-drift --all │
│  mm_dsl::MeerkatMachine      │  │    → compare on-disk vs current  │
│    Mutator                   │  │  xtask machine-verify --all      │
│    .apply(input)             │  │    → run TLC on the specs        │
│                              │  │                                  │
│  Enforces guards/updates     │  │  Also emits generated protocol   │
│  at runtime. Transition      │  │  adapter .rs files for cross-    │
│  table IS the generated code.│  │  composition seams (stored in    │
│                              │  │  each crate's src/generated/)    │
└──────────────┬───────────────┘  └──────────────────────────────────┘
               │
               │ called by runtime shell via dsl_apply()
               ▼
┌────────────────────────────────────────────────────────────────────┐
│  RUNTIME SHELL (handwritten, mechanics only)                       │
│  meerkat-runtime/src/meerkat_machine/*, driver/*, ops_lifecycle.rs │
│                                                                    │
│  Holds per-session Arc<Mutex<MeerkatMachineAuthority>>.            │
│  dsl_apply(input) locks, calls kernel.apply, realizes effects.     │
│  Owns IO, tokio lock topology, shell-only metadata (history,       │
│  wall-clock timestamps, correlation IDs, persistence channels).    │
└────────────────────────────────────────────────────────────────────┘
```

Trust axiom: the codegen (macro expansion + schema extraction) is correct. The drift-check + machine-verify together prove that the on-disk generated artifacts match the current DSL AND that the abstract model satisfies invariants. No separate equivalence proof between "runtime Rust" and "TLA+ model" — both come from the same DSL source.

## The three verification passes

Run these in order after any DSL edit:

1. `cargo xtask machine-codegen --all` — re-emit TLA+ specs + generated protocol adapters from the current DSL. Must run after any DSL change.
2. `cargo xtask machine-check-drift --all` — compare freshly generated artifacts against committed on-disk versions. Fails if DSL was edited without re-running codegen.
3. `cargo xtask machine-verify --all` — run TLC model-checker on the TLA+ specs. Proves invariants across the bounded state space (CI uses 2-3 concurrent entities per domain; can be expanded).

All three pass = DSL + runtime are provably in sync, and the machine cannot enter an invalid state given typed inputs.

## DSL is field-driven, not phase-driven

This is a non-obvious insight that must inform any authority absorption or DSL extension:

The pre-DSL model tracked lifecycle via enum phases (`TurnPhase` with 11 variants, `SurfacePhase`, `PeerIngressState`, `MobMemberKickoffPhase`). Each domain had its own phase enum.

The DSL model stores lifecycle via **fields** (scalar values, Maps, Sets) on `MeerkatMachineState`. The top-level `MeerkatPhase` has only 5 session-lifecycle values (Idle/Attached/Running/Retired/Stopped). Per-instance sub-states (per-input phase, per-op status, per-surface state) live in Maps keyed on domain ID:

```
input_phases: Map<String, String>
op_statuses: Map<String, String>
surface_base_state: Map<String, String>   (post F3 extension)
member_kickoff: Map<String, String>       (post F6 extension)
```

Sub-phases like `TurnPhase::CallingLlm` or `SurfacePhase::Active` are PROJECTIONS derivable from DSL field combinations. They are not independent state variables.

**Implication for absorbing an authority:** do NOT try to map the authority's phase enum onto the DSL's top-level Phase enum. Instead, add fields to MeerkatMachineState (or a domain-keyed Map) that hold the same information as the authority's phase + auxiliary fields combined. Parameters that the authority's inputs carry (run_id, surface_id, boundary_sequence) either map to existing DSL state fields (e.g., `current_run_id`) or need new fields added during absorption.

## Signals vs Inputs

The DSL distinguishes two transition triggers:

- **Inputs** — parameterized variants carrying data (`MeerkatMachineInput::QueueAccepted { input_id }`). Drive state mutations; usually called from runtime shell code via `dsl_apply(input)`.
- **Signals** — parameterless variants (`MeerkatMachineSignal::CallStarted`). Used at composition seams for cross-machine handoffs; typically fired via `apply_signal(signal)`. The machine may transition on a signal without any data payload.

Signals exist because compositions route traffic across machine boundaries at owner handoffs. The receiver doesn't need payload — the payload was stored in an upstream Input on the sending side. Example: `Prepare { session_id, run_id }` (Input with params) populates `current_run_id` field. Later, `StartConversationRun` signal fires parameterless — the run_id is already in state.

When writing a trait or handle method for a domain, match the DSL shape:
- If the domain uses Inputs with params, the trait method takes those params.
- If the domain uses Signals, the trait method is parameterless.

Mapping an old phase-driven authority's input onto a parameterless DSL signal loses the parameters. Either (a) an upstream DSL Input is responsible for storing the params into state fields, OR (b) the DSL needs to be extended with a new Input variant carrying those params.

## Concrete trace — user sends "Hello"

1. **Surface** (CLI/RPC/REST): `rkat run "Hello"` → `SessionService::create_session()` → `AgentFactory::build_agent()`.
2. **Runtime-backed surface**: calls `MeerkatMachine::prepare_bindings(session_id)` to obtain `SessionRuntimeBindings` (bundle of handles including DSL handles per F1/F3/F4/F5 cutover).
3. **Accept input**: `MeerkatMachine::accept_input_with_completion(session_id, Input::Prompt { content: "Hello" })` routes through dispatch → driver.
4. **Driver** generates `input_id`, resolves policy, calls `self.dsl_apply(mm_dsl::MeerkatMachineInput::QueueAccepted { input_id })`.
5. **`dsl_apply`** locks the per-session `Arc<Mutex<MeerkatMachineAuthority>>`, calls `authority.apply(input)` on the generated kernel.
6. **Generated kernel** runs the `QueueAccepted` transition: guards (`!self.input_phases.contains_key(input_id)`), updates (`self.input_phases.insert(input_id, "Queued")`, `self.queue_lane.insert(input_id)`, `self.input_admission_seq.insert(input_id, self.next_admission_seq)`, `self.next_admission_seq += 1`), emits effects (`EmitQueuedEvent`).
7. **Kernel returns** `Result<MeerkatMachineTransition, MeerkatMachineError>`. Transition carries `effects: Vec<MeerkatMachineEffect>`.
8. **Shell realizes effects**: sends events over observer channel, wakes runtime loop, notifies completion handles. Updates shell-only metadata on `InputState` (history log, wall-clock `updated_at`).
9. **Runtime loop** wakes (via WakeRuntime effect), reads DSL for Queued inputs in `queue_lane`, picks next by `input_admission_seq` order, applies `StageForRun { input_id, run_id }` → transitions to "Staged".
10. **Agent turn** executes: LLM calls, tool execution, boundaries. Each significant event is a DSL apply.
11. **Run completes**: `MarkApplied → MarkAppliedPendingConsumption → ConsumeInput` sequence. `ConsumeInput` removes `input_id` from `queue_lane`/`steer_lane` and marks `input_phases[input_id] = "Consumed"` (terminal).
12. **Completion handle** (oneshot) fires back to surface; surface returns response.

Every semantic state change is one `dsl_apply`. Shell code does not decide what phase an input is in; the DSL does. Shell reads DSL fields when it needs to know the phase.

## Cross-crate DSL access (handle trait pattern)

`meerkat-runtime` holds the MeerkatMachine DSL authority per session. Other crates (meerkat-core, meerkat-mcp, meerkat-comms, meerkat-session) need to route transitions through it without importing meerkat-runtime (dependency direction forbids).

Pattern: trait in meerkat-core, impl in meerkat-runtime, `Arc<dyn Trait>` on `SessionRuntimeBindings` (also in meerkat-core).

Current handle traits (all in `meerkat-core/src/handles.rs`):
- `TurnStateHandle` — for turn execution transitions
- `CommsDrainHandle` — for comms drain lifecycle
- `ExternalToolSurfaceHandle` — for MCP tool surface transitions
- `PeerCommsHandle` — for peer envelope classification
- `SessionAdmissionHandle` — for session turn admission

Each trait has methods 1:1 with the DSL's Inputs/Signals for that domain. Each method takes primitive-or-core-type parameters (not domain types from downstream crates, which meerkat-core can't import). Return type is `Result<(), DslTransitionError>` for write methods; read accessors return plain snapshot structs.

Impls in `meerkat-runtime/src/handles/` hold `Arc<HandleDslAuthority>` where `HandleDslAuthority` wraps `Arc<Mutex<mm_dsl::MeerkatMachineAuthority>>` — **the session's real authority**, not a private per-handle copy. `prepare_bindings()` constructs one `HandleDslAuthority` per session pointing at the session's authority, passes Arc clones to all 5 handle impls. All handles for a given session route to the same state.

MobMachine in-crate access: `MobActor.dsl_authority: MobMachineAuthority` is direct (meerkat-mob depends on meerkat-runtime). No cross-crate trait needed for mob-internal callers.

## Compositions

Compositions live in `meerkat-machine-schema/src/catalog/compositions.rs`. Four canonical ones:

- `meerkat_mob_seam_composition` — MeerkatMachine ↔ MobMachine handoff
- `schedule_bundle_composition` — schedule + occurrence + delivery
- `schedule_runtime_bundle_composition` — schedule + runtime boundary
- `schedule_mob_bundle_composition` — schedule + mob boundary

Compositions express effect-disposition rules: which effects emitted by one machine are consumed as inputs by another, which obligations must be realized before a terminal, and which protocol helpers get codegen'd.

Generated protocol adapters land in each crate's `src/generated/` (e.g., `meerkat-core/src/generated/protocol_ops_barrier_satisfaction.rs`). These are not hand-edited; regenerate via `xtask machine-codegen --all` when the catalog changes.

## Dogma for machine work

Per `docs/architecture/meerkat-runtime-dogma.md`:

1. **One semantic fact, one owner.** If something can mutate machine state, the machine owns it. Shell copies of DSL-owned fields are shadow truth and must be eliminated (see Phase 5G).
2. **Machines own semantics, shell owns mechanics.** Transition tables belong in the DSL. Handwritten match tables on `(phase, input)` tuples in shell code are authority-reimplementation in disguise.
3. **Derived projections are rebuildable, never authoritative.** If a cache or projection affects a semantic decision, it's shadow truth.

## What the workspace looks like in the target state

- Exactly 4 `*.rs` files in `meerkat-machine-schema/src/catalog/dsl/` — one per machine. No other machine DSL sources.
- Exactly 4 compositions in `meerkat-machine-schema/src/catalog/compositions.rs`.
- Zero `*_authority.rs` files containing handwritten match-table state machines. The only file named `dsl_authority.rs` is `meerkat-runtime/src/meerkat_machine/dsl_authority.rs`, which is the runtime adapter plumbing (not a state machine).
- Runtime shell holds only: per-session `Arc<Mutex<MeerkatMachineAuthority>>` + `Arc<Mutex<MobMachineAuthority>>`, handle trait impls that route through the shared authorities, IO mechanics (channels, handles, wall-clock timestamps), and observability projections (history logs, diagnostic snapshots).
- Handle traits in `meerkat-core/src/handles.rs` (`TurnStateHandle`, `CommsDrainHandle`, `ExternalToolSurfaceHandle`, `PeerCommsHandle`, `SessionAdmissionHandle`) give cross-crate access to MeerkatMachine transitions. Mob-internal callers use `MobActor.dsl_authority` directly.
- `InputState` and equivalent per-instance shell structs carry only non-DSL data: caller-provided metadata (policy, durability, idempotency keys), wall-clock timestamps, and observability history. Semantic state (phase, terminal outcome, attempt count, run association, boundary sequence) is read from the DSL through handle accessors.

## Key files to read when touching the machine system

- `meerkat-machine-schema/src/catalog/dsl/<machine>.rs` — DSL source (truth)
- `meerkat-machine-schema/src/catalog/mod.rs` — `canonical_machine_schemas()` registry
- `meerkat-machine-schema/src/catalog/compositions.rs` — composition definitions
- `meerkat-machine-kernels/src/runtime.rs` — `GeneratedMachineKernel` interpreter
- `meerkat-runtime/src/meerkat_machine/dsl.rs` — MeerkatMachine DSL + runtime-local re-exports
- `meerkat-runtime/src/meerkat_machine/dsl_authority.rs` — DSL adapter plumbing (NOT a handwritten authority)
- `meerkat-runtime/src/handles/` — runtime impls of handle traits; the `HandleDslAuthority` shared wrapper
- `meerkat-core/src/handles.rs` — handle trait definitions
- `meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings` (the cross-crate seam)
- `docs/architecture/meerkat-runtime-dogma.md` — dogma rules
- `xtask/src/machines.rs` — codegen/drift/verify xtask command implementations

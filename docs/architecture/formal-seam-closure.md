# ADR: Formal Seam Closure

**Status**: Accepted
**Date**: 2026-03-20

## Problem

Meerkat's RMAT model proves individual machines and declared machine-to-machine
routes, but does not prove that machine effects requiring owner feedback are
fully realized, or that shell-observed outcomes match machine truth. Three
specific gaps exist:

1. **Comms drain seam**: Shell code directly mutates `comms_drain_active` outside
   the machine authority path. The machine emits `SpawnDrainTask` /
   `AbortDrainTask` effects, but their realization and feedback flow is not
   protocol-constrained.

2. **Turn barrier seam**: `TurnExecutionMachine` delegates operation completion
   to `OpsLifecycleMachine`, but the barrier-satisfaction signal (all pending ops
   complete) flows through ad hoc channels rather than through authority truth.
   The classification of ops as barrier-blocking vs detached is shell logic, not
   machine-owned.

3. **Terminal outcome alignment**: `ExtractionExhausted` transitions to
   `Completed` phase in `TurnExecutionMachine` but semantically represents a
   validation failure. Shell code independently decides how to surface outcomes,
   creating divergence risk between machine truth and API responses.

## Decision

Extend the existing composition ontology with typed handoff protocols, obligation
tracking, and generated protocol helpers. No second actor system, no second
effect transport, no monolithic composition machine.

### Locked Decisions

- **No second actor system.** Owner participants extend existing composition
  actors with `ActorKind`.
- **No second effect transport.** "Owned handoff" is a realization facet
  (`handoff_protocol: Option<String>`) on existing `EffectDispositionRule`, not a
  fourth disposition variant.
- **Step 10 is real codegen**, scoped to seam protocol helpers only. Authorities
  stay handwritten.
- **`ExtractionExhausted`** transitions to existing `Failed` phase with new
  `TurnTerminalOutcome::StructuredOutputValidationFailed`. No new terminal phase.
- **Turn-boundary suppression hot path** stays lock-free (atomic-read cost).

## Proof Model

Three claim types classify every acceptance criterion:

### Structural Coverage

Protocol exists, realization sites are declared, CI enforces no missing path.

Structural claims assert completeness of the model: every effect that crosses an
ownership boundary has a declared protocol, every protocol has realization sites,
and the closed-world composition validation rejects unprotocolized handoffs.

Structural claims are verified by:
- Schema validation (`CompositionSchema::validate()`)
- RMAT audit hard-error rules (`xtask rmat-audit`)
- CI gate (`make ci`)

### Safety

No invalid state or transition is reachable.

Safety claims assert that the system cannot enter a state where machine truth
and runtime behavior diverge. Safety is verified by:
- Machine-level TLC model checking (per-machine invariants)
- Composition-level TLC model checking (cross-machine invariants)
- Obligation closure invariants: no open obligations on terminal phases, no
  feedback without a prior obligation

### Liveness

Under explicit fairness assumptions, required feedback eventually occurs.

Liveness claims are scoped and conditional. Each liveness claim states its
fairness assumption explicitly. Liveness is verified by:
- TLC weak/strong fairness annotations on owner-actor processes
- Explicit witness scenarios that exercise the feedback path

## Tranche Boundaries

### Tranche 1 Proves

**Structural coverage** for all handoff-annotated effects:
- Every effect with `handoff_protocol` has a matching `EffectHandoffProtocol` in
  its composition
- Every protocol has at least one realization site
- Protocol feedback enters only through generated helpers
- Terminal mapping uses generated classification helpers

**Safety** for three seams:
- Comms drain: obligation closure (`NoOpenObligationsOnTerminal`,
  `NoFeedbackWithoutObligation`), no direct shell mutation of
  `comms_drain_active` outside generated protocol path
- Barrier: `ToolCallsResolved` requires `barrier_satisfied == true`, only
  `Barrier`-classified ops enter the wait set, `Detached` ops do not block
- Terminal alignment: `ExtractionExhausted` -> `Failed` phase with `RunFailed`
  effect, exhaustive generated terminal classifier with no default arm

**One explicit liveness claim**:
- Comms drain spawn protocol: "Under task-scheduling fairness, every
  `SpawnDrainTask` obligation eventually receives `TaskSpawned` or `TaskExited`
  feedback."

### Tranche 1 Does NOT Prove

- Cross-machine op-barrier correctness/liveness (OpsLifecycle ->
  TurnExecution barrier satisfaction is wired but not composition-proven)
- Liveness of barrier satisfaction (only safety: barrier cannot be bypassed)
- Liveness of abort acknowledgment (only safety: no feedback without obligation)

### Tranche 2 Proves

**Structural + safety + scoped liveness** for TurnExecution <-> OpsLifecycle
barrier composition:
- OpsLifecycle `WaitAllSatisfied` effect routes to TurnExecution
  `OpsBarrierSatisfied` input through a declared handoff protocol
- Authority-backed barrier satisfaction: `wait_all()` routes through authority
  `WaitAll` input, authority emits `WaitAllSatisfied`, shell waits for the
  authority-emitted effect
- Composition-level obligation closure for the barrier seam
- Decision gate: if criteria met, extract `TurnOpBarrierMachine` as a separate
  machine; otherwise keep barrier as enriched TurnExecution input wired to
  authority-backed OpsLifecycle satisfaction

## Execution Order

The dependency line is strict:

- **Formal layer first**: lock the proof model, inventory seam candidates,
  extend the current actor/effect ontology, add protocol-aware validation and
  TLA generation, add generated Rust seam helpers, and upgrade RMAT/CI
- **Runtime seam conversion second**: protocolize comms drain, replace raw
  operation IDs with typed async-op references, move ops waiting onto the
  authority-owned wait lifecycle, align terminal outcomes with surfaced failure
  classes, and preserve producer-derived feedback fields across live handoffs
- **Closeout last**: backfill the seam model onto the remaining async
  boundaries, remove legacy realization paths, tighten release gates, and run
  the final repo-wide seam audit

## Ontology Extension — Type Sketches

These are the exact type-level additions to `meerkat-machine-schema`, ready for
implementation in steps 5-8. All additions extend existing types — nothing is
forked or duplicated.

### Step 5: `handoff_protocol` on `EffectDispositionRule`

**File**: `meerkat-machine-schema/src/machine.rs`

**Current shape**:

```rust
pub struct EffectDispositionRule {
    pub effect_variant: String,
    pub disposition: EffectDisposition,  // Local | External | Routed { .. }
}
```

**Extended shape**:

```rust
pub struct EffectDispositionRule {
    pub effect_variant: String,
    pub disposition: EffectDisposition,
    /// When set, this effect participates in an owner-handoff protocol.
    /// The named protocol must be declared as an `EffectHandoffProtocol`
    /// in every composition that includes this machine.
    ///
    /// Only meaningful for `Local` or `External` dispositions — `Routed`
    /// effects are already constrained by composition routes.
    pub handoff_protocol: Option<String>,
}
```

**Validation** (in `MachineSchema::validate()`):
- If `handoff_protocol.is_some()` and `disposition` is `Routed { .. }`, emit
  error — routed effects don't need handoff protocols, they have routes.
- No other validation at the machine level; the composition validates protocol
  existence.

**Migration**: All existing `EffectDispositionRule` constructors in catalog files
get `handoff_protocol: None`. The local `disposition()` helper in each catalog
file is updated to pass `None`. No behavioral change.

### Step 6: `ActorSchema` and `ActorKind` on `CompositionSchema`

**File**: `meerkat-machine-schema/src/composition.rs`

**New types**:

```rust
/// Declares a named actor participating in this composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorSchema {
    pub name: String,
    pub kind: ActorKind,
}

/// Distinguishes machine actors from owner actors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorKind {
    /// Driven by machine transitions — deterministic given inputs.
    Machine,
    /// Represents a host/runtime that realizes effects and provides
    /// protocol-constrained feedback. Nondeterministic within protocol
    /// bounds: chooses *when* and *which* allowed feedback to submit,
    /// but cannot submit feedback outside the declared protocol.
    Owner,
}
```

**Integration** — new field on `CompositionSchema`:

```rust
pub struct CompositionSchema {
    // ... all existing fields unchanged ...

    /// Explicit actor declarations. Every `MachineInstance.actor` must
    /// reference an `ActorSchema.name` where `kind == Machine`.
    /// Owner actors are referenced only by `EffectHandoffProtocol.realizing_actor`.
    pub actors: Vec<ActorSchema>,
}
```

**Validation** (in `CompositionSchema::validate()`):
1. `ActorSchema` names are unique (reuse the existing `unique_names()` helper).
2. Every `MachineInstance.actor` references an `ActorSchema` with
   `kind == Machine`.
3. Every actor referenced by `actor_priorities` or `scheduler_rules` must exist
   in `actors` (this replaces the current implicit actor-id deduction from
   `MachineInstance.actor` fields).
4. `ActorKind::Owner` actors may participate in `actor_priorities` and
   `scheduler_rules` (they need scheduling semantics for fairness modeling).

**Breaking change to actor validation**: Currently, `validate()` deduces the
actor set from `machines.iter().map(|m| m.actor)`. After this change, it deduces
actors from `actors.iter().map(|a| a.name)`. The `MachineInstance.actor` field
still exists and must reference a `Machine`-kind actor. This means existing
compositions must add `actors: vec![...]` declarations.

**Migration**: All 8 canonical compositions get `actors` declarations matching
their current implicit actor sets, all with `ActorKind::Machine`.

### Step 7: `EffectHandoffProtocol` on `CompositionSchema`

**File**: `meerkat-machine-schema/src/composition.rs`

**New types**:

```rust
/// Declares the contract between a machine that emits an effect and an
/// owner actor that realizes it and provides feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectHandoffProtocol {
    /// Protocol name — must match the `handoff_protocol` value on the
    /// producing machine's `EffectDispositionRule`.
    pub name: String,

    /// The machine instance that produces the effect.
    pub producer_instance: String,

    /// The effect variant that triggers this protocol.
    pub effect_variant: String,

    /// The owner actor that realizes the effect. Must reference an
    /// `ActorSchema` with `kind == Owner`.
    pub realizing_actor: String,

    /// Fields from the effect variant that correlate the obligation to
    /// the feedback. These become the typed obligation record in
    /// generated code.
    pub correlation_fields: Vec<String>,

    /// Machine inputs that the owner actor may submit as feedback.
    /// The owner may submit any of these, but nothing outside this set.
    pub allowed_feedback_inputs: Vec<FeedbackInputRef>,

    /// When and how the obligation must be closed.
    pub closure_policy: ClosurePolicy,

    /// Optional fairness annotation for TLA+ liveness claims.
    /// e.g., "eventual feedback under task-scheduling fairness"
    pub liveness_annotation: Option<String>,
}

/// References a specific machine input that the owner may submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackInputRef {
    /// The machine instance receiving the feedback.
    pub machine_instance: String,
    /// The input variant on that machine.
    pub input_variant: String,
}

/// Determines when a handoff obligation is considered closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosurePolicy {
    /// Owner must acknowledge (provide at least one feedback input).
    /// Obligation remains open until feedback is received.
    AckRequired,
    /// Owner must either acknowledge or the machine must reach a
    /// terminal/abort phase. Allows cleanup without feedback.
    AckOrAbort,
    /// Obligation is closed when the machine reaches a terminal phase,
    /// regardless of whether feedback was received. Fire-and-observe.
    TerminalClosure,
}
```

**Integration** — new field on `CompositionSchema`:

```rust
pub struct CompositionSchema {
    // ... all existing fields unchanged ...
    pub actors: Vec<ActorSchema>,              // from step 6
    pub handoff_protocols: Vec<EffectHandoffProtocol>,  // new
}
```

**Validation** (in `validate()` and `validate_against()`):
1. Protocol names are unique within the composition.
2. `producer_instance` references a valid `MachineInstance.instance_id`.
3. `effect_variant` exists on the producer machine's `effects` enum
   (in `validate_against()` where schemas are available).
4. The producer machine's `EffectDispositionRule` for that `effect_variant` has
   `handoff_protocol == Some(protocol.name)` (in `validate_against()`).
5. `realizing_actor` references an `ActorSchema` with `kind == Owner`.
6. `correlation_fields` are valid field names on the effect variant
   (in `validate_against()`).
7. Each `FeedbackInputRef.machine_instance` references a valid `MachineInstance`.
8. Each `FeedbackInputRef.input_variant` exists on the target machine's `inputs`
   enum (in `validate_against()`).
9. `TerminalClosure` requires the producer machine to have at least one terminal
   phase (in `validate_against()`).

**Migration**: All 8 canonical compositions get `handoff_protocols: vec![]`
initially (no protocols until step 12).

### Step 8: Closed-World Obligation Validation

**File**: `meerkat-machine-schema/src/composition.rs`

**New invariant kind**:

```rust
pub enum CompositionInvariantKind {
    // ... existing variants unchanged ...

    /// Asserts that a handoff-annotated effect has a matching protocol.
    HandoffProtocolCovered {
        producer_instance: String,
        effect_variant: String,
        protocol_name: String,
    },
}
```

**Closed-world extension** (in `validate_against()`, within the
`if self.closed_world { .. }` block):

For every `MachineInstance` in the composition, for every
`EffectDispositionRule` on that machine's schema where
`handoff_protocol.is_some()`:
- There must exist an `EffectHandoffProtocol` in
  `self.handoff_protocols` where:
  - `protocol.producer_instance == machine_instance.instance_id`
  - `protocol.effect_variant == rule.effect_variant`
  - `protocol.name == rule.handoff_protocol.unwrap()`
- If missing, emit `CompositionSchemaError::MissingHandoffProtocol { .. }`.

**Additional feedback-input consistency checks**:
- Each `FeedbackInputRef.input_variant` field types must be compatible with the
  correlation fields (checked in `validate_against()`).

### Concrete Example: Comms Drain (Step 12)

After steps 5-8 are implemented, step 12 applies the ontology to the comms
drain seam:

**In `comms_drain_lifecycle.rs`** — disposition changes:

```rust
disposition_with_handoff("SpawnDrainTask", EffectDisposition::Local,
    Some("comms_drain_spawn")),
disposition("SetTurnBoundaryDrainSuppressed", EffectDisposition::Local),
    // handoff_protocol: None — local projection, no owner feedback needed
disposition_with_handoff("AbortDrainTask", EffectDisposition::Local,
    Some("comms_drain_abort")),
```

**In `compositions.rs`** — for every composition containing CommsDrainLifecycle:

```rust
actors: vec![
    // ... existing Machine actors ...
    ActorSchema { name: "session_host".into(), kind: ActorKind::Owner },
],
handoff_protocols: vec![
    EffectHandoffProtocol {
        name: "comms_drain_spawn".into(),
        producer_instance: "comms_drain".into(),
        effect_variant: "SpawnDrainTask".into(),
        realizing_actor: "session_host".into(),
        correlation_fields: vec!["mode".into()],
        allowed_feedback_inputs: vec![
            FeedbackInputRef {
                machine_instance: "comms_drain".into(),
                input_variant: "TaskSpawned".into(),
            },
            FeedbackInputRef {
                machine_instance: "comms_drain".into(),
                input_variant: "TaskExited".into(),
            },
        ],
        closure_policy: ClosurePolicy::AckRequired,
        liveness_annotation: Some(
            "eventual feedback under task-scheduling fairness".into(),
        ),
    },
    EffectHandoffProtocol {
        name: "comms_drain_abort".into(),
        producer_instance: "comms_drain".into(),
        effect_variant: "AbortDrainTask".into(),
        realizing_actor: "session_host".into(),
        correlation_fields: vec![],
        allowed_feedback_inputs: vec![
            FeedbackInputRef {
                machine_instance: "comms_drain".into(),
                input_variant: "AbortObserved".into(),
            },
        ],
        closure_policy: ClosurePolicy::TerminalClosure,
        liveness_annotation: None,
    },
],
```

### Re-exports

**File**: `meerkat-machine-schema/src/lib.rs`

New types to re-export after steps 5-7:

```rust
pub use composition::{
    // ... existing re-exports ...
    ActorKind, ActorSchema,                    // step 6
    ClosurePolicy, EffectHandoffProtocol,      // step 7
    FeedbackInputRef,                          // step 7
};
```

### New Error Variants

**`MachineSchemaError`** (step 5):

```rust
HandoffProtocolOnRoutedEffect {
    variant: String,
}
```

**`CompositionSchemaError`** (steps 6-8):

```rust
ActorKindMismatch {
    actor: String,
    expected: ActorKind,
    actual: ActorKind,
},
UnknownHandoffProducer {
    protocol: String,
    instance: String,
},
UnknownHandoffEffect {
    protocol: String,
    effect: String,
},
UnknownHandoffActor {
    protocol: String,
    actor: String,
},
HandoffActorNotOwner {
    protocol: String,
    actor: String,
},
UnknownHandoffFeedbackMachine {
    protocol: String,
    machine: String,
},
UnknownHandoffFeedbackInput {
    protocol: String,
    machine: String,
    input: String,
},
HandoffProtocolMismatch {
    protocol: String,
    effect_variant: String,
    expected_protocol: String,
},
MissingHandoffProtocol {
    from_instance: String,
    effect_variant: String,
    expected_protocol: String,
},
TerminalClosureRequiresTerminalPhases {
    protocol: String,
    producer_instance: String,
},
UnknownHandoffCorrelationField {
    protocol: String,
    field: String,
},
```

### Generated Protocol Helpers

`xtask protocol-codegen` generates typed helpers per protocol:
- Obligation record struct (captures correlation fields)
- Executor function (calls `authority.apply()`, returns effects + obligation
  token)
- Feedback submitter (consumes obligation token, calls `authority.apply()` with
  feedback input)
- Terminal classification helper (exhaustive match, no default arm)

Generated code carries `// @generated` markers. RMAT audit enforces that
protocol feedback enters only through generated helpers.

### Barrier Schema Changes — Type Sketches (Steps 17-19)

These are the schema-level additions to `TurnExecutionMachine` for barrier-aware
ops. The design replaces raw `OperationId` sequences with typed async-op
references that carry a wait policy, adds a barrier-satisfaction input, and gates
`ToolCallsResolved` on barrier clearance.

#### Step 17: Replace raw op IDs with `AsyncOpRef`

**File**: `meerkat-machine-schema/src/catalog/turn_execution.rs`

**Current state field** (line ~48):

```rust
field("pending_op_ids", TypeRef::Option(Box::new(
    TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into())))
))),
```

**Replaced with**:

```rust
field("pending_op_refs", TypeRef::Option(Box::new(
    TypeRef::Seq(Box::new(TypeRef::Named("AsyncOpRef".into())))
))),
```

Where `AsyncOpRef` is a named type carrying:
- `operation_id: OperationId` — the raw op ID
- `wait_policy: WaitPolicy` — `Barrier` or `Detached`

`WaitPolicy` is a two-variant enum:
- `Barrier` — op blocks `ToolCallsResolved` until satisfied
- `Detached` — op runs independently, does not block the turn

**Input change**: `RegisterPendingOps` field changes from
`operation_ids: Seq<OperationId>` to `op_refs: Seq<AsyncOpRef>`.

**Derived field**: `barrier_op_ids` — a computed projection of
`pending_op_refs.filter(|r| r.wait_policy == Barrier).map(|r| r.operation_id)`.
This is a read-only view used by guards, not a mutable state field.

**Init**: `pending_op_refs` initializes to `None`.

#### Step 18: Formalize barrier-classification authority

The classification of an operation as `Barrier` vs `Detached` is determined at
`RegisterPendingOps` time by the shell, but it is represented explicitly in the
typed `AsyncOpRef` data crossing the dispatcher boundary and enforced by the
turn machine. Classification is immutable once registered; the machine owns the
barrier gate and the runtime waits against authority-owned `BeginWaitAll` /
`WaitAllSatisfied` state rather than raw shell watcher timing.

**RMAT audit rule**: `RegisterPendingOps` must be the sole entry point for
`pending_op_refs` mutation. No shell code may directly modify `pending_op_refs`
outside the machine authority.

#### Step 19: Extend TurnExecution for barrier membership

**New state field**:

```rust
field("barrier_satisfied", TypeRef::Bool),
```

Initialized to `true` (no barrier = satisfied). Set to `false` when
`RegisterPendingOps` contains any `Barrier`-policy ops. Set back to `true`
when `OpsBarrierSatisfied` is received.

**New input**:

```rust
VariantSchema {
    name: "OpsBarrierSatisfied".into(),
    fields: vec![
        field("run_id", TypeRef::Named("RunId".into())),
        field("barrier_token", TypeRef::Named("BarrierToken".into())),
    ],
},
```

The `barrier_token` is a correlation field that ties the satisfaction signal to
the specific barrier set. This prevents stale satisfaction from a previous
barrier from clearing a new one.

**New guard on `ToolCallsResolved`**:

```rust
Guard {
    name: "barrier_satisfied".into(),
    expr: Expr::Eq(
        Box::new(Expr::Field("barrier_satisfied".into())),
        Box::new(Expr::Bool(true)),
    ),
},
```

This is the critical safety property: `ToolCallsResolved` cannot fire while
`barrier_satisfied == false`. The machine enforces this — the shell cannot
bypass it.

**New transition**: `OpsBarrierSatisfied` from `WaitingForOps`:

```rust
TransitionSchema {
    name: "OpsBarrierSatisfied".into(),
    from: vec!["WaitingForOps".into()],
    on: InputMatch {
        variant: "OpsBarrierSatisfied".into(),
        bindings: vec!["run_id".into(), "barrier_token".into()],
    },
    guards: vec![
        Guard {
            name: "matches_active_run".into(),
            expr: Expr::Eq(
                Box::new(Expr::Field("active_run".into())),
                Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
            ),
        },
    ],
    updates: vec![Update::Assign {
        field: "barrier_satisfied".into(),
        expr: Expr::Bool(true),
    }],
    to: "WaitingForOps".into(),
    emit: vec![],
}
```

**Modified `RegisterPendingOps` transition**: Updates must set
`barrier_satisfied` to `false` when any op has `wait_policy == Barrier`:

```rust
Update::Assign {
    field: "barrier_satisfied".into(),
    expr: Expr::ContainsWhere {
        seq: Box::new(Expr::Binding("op_refs".into())),
        predicate: "wait_policy_is_barrier".into(),
        then: Expr::Bool(false),
        otherwise: Expr::Bool(true),
    },
},
```

Note: `Expr::ContainsWhere` may require extending the `Expr` enum if the
existing expression system cannot represent this. If so, a separate boolean
field `has_barrier_ops` may be carried alongside `op_refs`, but the machine
must still enforce the gate and RMAT must verify consistency with the typed
`AsyncOpRef` content.

#### Composition wiring (Step 28)

`OpsBarrierSatisfied` is not a direct routed effect. `OpsLifecycle` emits the
authority-owned `WaitAllSatisfied` effect, the handoff protocol captures the
obligation, and owner feedback submits `TurnExecution.OpsBarrierSatisfied`
using explicit field bindings (`operation_ids` from the obligation, `run_id`
from owner context).

## Shell Boundary Rule

Shell code may:
- Capture evidence (observe runtime lifecycle facts)
- Execute mechanics (spawn tasks, manage channels, route messages)
- Classify raw inputs into typed machine inputs

Shell code must not:
- Perform observation-to-feedback mapping outside protocol constraints
- Directly mutate protected state fields outside generated protocol helpers
- Implement handwritten terminal-outcome classification that duplicates machine
  truth

The boundary rule is enforced by:
- RMAT audit structural seam rules (hard errors in CI)
- Protected field rules in `xtask/src/rmat_policy.rs`
- Generated protocol helpers that are the only legal path for feedback submission

## Verification Strategy

After each phase gate:

```bash
# Schema validation
./scripts/repo-cargo nextest run -p meerkat-machine-schema

# Machine TLC verification
cargo run -p xtask -- machine-verify --all

# Drift check
cargo run -p xtask -- machine-check-drift --all

# RMAT audit (includes new structural seam rules)
cargo run -p xtask -- rmat-audit

# Full test suite
./scripts/repo-cargo nextest run --workspace --show-progress none \
    --status-level none --final-status-level fail
```

## Consequences

- Every async ownership boundary gains formal protocol coverage
- Shell code is mechanically constrained from inventing parallel semantic state
- Terminal outcome classification is generated and exhaustive — no divergence
  between machine truth and surface responses
- Owner actors are first-class composition participants, not implicit runtime
  behavior
- The proof model is closed end-to-end for the current seam inventory rather
  than staged behind later phases
- CI enforces structural coverage — new handoff-annotated effects without
  protocols fail the build

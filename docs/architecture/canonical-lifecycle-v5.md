# Canonical Lifecycle and Execution Specification

Status: Final normative proposal, ontology-aligned v5  
Scope: Meerkat and Meerkat Mob architecture  
Applies to: `meerkat-core`, runtime/control-plane implementations, and operational surfaces

## 1. Purpose

This specification defines the canonical lifecycle, execution, durability, recovery, and control-plane model for the Meerkat ecosystem.

Its goals are:

- keep `meerkat-core` small, reusable, and orchestration-agnostic
- align internal runtime concepts with the public Meerkat/Mob concept model
- make durability, recovery, and lifecycle behavior explicit and testable
- avoid fake semantic distinctions at the execution layer
- prevent hidden control-flow semantics from reappearing in implementation-specific paths

This document is normative. Where it uses MUST, MUST NOT, SHOULD, or MAY, those words are intentional.

## 2. Shared Ontology

The canonical shared ontology is:

- `Runtime`
- `Conversation`
- `Input`
- `Run`
- `Peer`
- `Tool`
- `Sub-agent`
- `Mob`
- `Flow`
- `Step`

These are the concepts that should be visible, directly or indirectly, across the ecosystem.

The following are **not** foundational runtime concepts:

- `Message`
- `Request`
- `Response`
- `Assignment`
- `Delegation`
- `Role`

Those may exist as communication or orchestration conventions above the foundational runtime model, but they are not the base execution ontology.

## 3. Layer Model

### 3.1 `meerkat-core`

`meerkat-core` owns:

- run execution
- conversation mutation boundaries
- tool loop execution
- transcript and system-context mutation primitives
- run lifecycle events
- out-of-band control commands

`meerkat-core` does **not** own:

- routing
- topology
- wiring
- peer lifecycle semantics
- scheduling semantics
- delivery semantics
- flow semantics
- input queue policy
- coalescing, supersession, suppression, or priority
- crash recovery policy above run-boundary reconciliation

### 3.2 Runtime / Control Plane

The runtime/control-plane layer owns:

- runtime lifecycle
- input acceptance and queueing
- input state ledger
- explicit projection of control-plane facts into runtime-visible input
- peer topology and wiring
- flow dispatch and recovery
- canonical default policy decisions
- crash recovery
- retire, respawn, reset, and destroy semantics

### 3.3 Operational / Product Surfaces

Operational/product surfaces own:

- CLI, REST, RPC, console, admin, and SDK packaging
- schedules, connectors, routing adapters, delivery adapters, memory integrations
- operator-facing views and reports
- profile configuration

Operational/product surfaces MUST consume the semantics defined here. They MUST NOT invent incompatible lifecycle or recovery behavior.

## 4. Core Invariants

The following are hard invariants:

- Core MUST know only about `Conversation`, `Run`, `Tool`, and conversation mutation primitives.
- Core MUST NOT classify flow, topology, peer lifecycle, delivery, routing, or scheduling semantics.
- Core MUST NOT infer control-plane state from conversation content.
- Runtime/control-plane MUST be the source of truth for input lifecycle and recovery.
- No durable accepted input may be silently dropped.
- Runtime/control-plane facts become runtime-visible input only by explicit projection.
- Public surfaces and internal runtime contracts MUST share this ontology, even if they expose different levels of detail.

## 5. Canonical Runtime Concepts

The canonical internal contract consists of:

1. `RuntimeEvent`
2. `Input`
3. `InputState`
4. `RunPrimitive`
5. `RunControlCommand`

`PolicyDecision` exists as internal runtime machinery but is not a foundational ontology noun.

## 6. Canonical Identifiers

```rust
pub type RuntimeEventId = String;
pub type RuntimeId = String;
pub type LogicalRuntimeId = String;
pub type ConversationId = String;
pub type InputId = String;
pub type RunId = String;
pub type FlowId = String;
pub type StepId = String;
pub type CausationId = String;
pub type CorrelationId = String;
pub type IdempotencyKey = String;
pub type SupersessionKey = String;
pub type PolicyVersion = String;
```

## 7. RuntimeEvent

`RuntimeEvent` is a control-plane fact. It is not a prompt and is never implicitly consumed as runtime input.

```rust
pub struct RuntimeEventHeader {
    pub event_id: RuntimeEventId,
    pub emitted_at_unix_ms: u64,
    pub operator_eligible: bool,
    pub causation_id: Option<CausationId>,
    pub correlation_id: Option<CorrelationId>,
}

pub enum RuntimeEvent {
    RuntimeLifecycle(RuntimeLifecycleEvent),
    RuntimeTopology(RuntimeTopologyEvent),
    FlowLifecycle(FlowLifecycleEvent),
    InputLifecycle(InputLifecycleEvent),
    Health(HealthEvent),
}

pub struct RuntimeLifecycleEvent {
    pub header: RuntimeEventHeader,
    pub runtime_id: LogicalRuntimeId,
    pub code: RuntimeLifecycleCode,
}

pub enum RuntimeLifecycleCode {
    Started,
    StopRequested,
    Stopped,
    RecoveryStarted,
    RecoveryCompleted,
    ResetRequested,
    ResetCompleted,
    DestroyRequested,
    Destroyed,
    Crashed,
    Recovered,
}

pub struct RuntimeTopologyEvent {
    pub header: RuntimeEventHeader,
    pub subject_runtime_id: LogicalRuntimeId,
    pub related_runtime_id: Option<LogicalRuntimeId>,
    pub code: RuntimeTopologyCode,
}

pub enum RuntimeTopologyCode {
    Wired,
    Unwired,
    PeerAdded,
    PeerRemoved,
    RouteRegistered,
    RouteRemoved,
}

pub struct FlowLifecycleEvent {
    pub header: RuntimeEventHeader,
    pub flow_id: FlowId,
    pub step_id: Option<StepId>,
    pub code: FlowLifecycleCode,
}

pub enum FlowLifecycleCode {
    FlowStarted,
    StepDispatched,
    StepCompleted,
    StepFailed,
    FlowCompleted,
    FlowCancelled,
}

pub struct InputLifecycleEvent {
    pub header: RuntimeEventHeader,
    pub input_id: InputId,
    pub runtime_id: LogicalRuntimeId,
    pub code: InputLifecycleCode,
}

pub enum InputLifecycleCode {
    Accepted,
    Deduplicated { existing_input_id: InputId },
    Queued,
    Staged,
    Applied,
    Consumed,
    Coalesced { aggregate_input_id: InputId },
    Superseded { by_input_id: InputId },
    Abandoned { reason: InputAbandonReason },
    Recovered,
}

pub struct HealthEvent {
    pub header: RuntimeEventHeader,
    pub runtime_id: Option<LogicalRuntimeId>,
    pub code: HealthCode,
}

pub enum HealthCode {
    HostLoopCrashed,
    DependencyUnavailable,
    CallbackTimeout,
    ResourceLimitExceeded,
}
```

## 8. Input

`Input` is the foundational externally originated thing that becomes available to a runtime for future execution.

It is the shared concept behind:

- user prompt
- peer-originated content
- flow step delivery
- external connector/webhook/schedule event
- tool callback result
- explicit projection from control-plane facts

```rust
pub enum InputSource {
    User,
    App,
    Peer,
    Connector,
    Schedule,
    Webhook,
    Flow,
    RuntimeProjection,
    ToolCallback,
}

pub enum InputDurability {
    Ephemeral,
    Durable,
    Derived,
}

pub struct InputVisibility {
    pub transcript_eligible: bool,
    pub operator_eligible: bool,
}

pub struct InputHeader {
    pub input_id: InputId,
    pub created_at_unix_ms: u64,
    pub source: InputSource,
    pub durability: InputDurability,
    pub visibility: InputVisibility,
    pub causation_id: Option<CausationId>,
    pub correlation_id: Option<CorrelationId>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub supersession_key: Option<SupersessionKey>,
}

pub enum Input {
    Prompt(PromptInput),
    ExternalEvent(ExternalEventInput),
    PeerInput(PeerInput),
    FlowStepInput(FlowStepInput),
    ProjectedInput(ProjectedInput),
    ToolCallbackInput(ToolCallbackInput),
}

pub struct PromptInput {
    pub header: InputHeader,
    pub prompt: String,
}

pub struct ExternalEventInput {
    pub header: InputHeader,
    pub event_kind: String,
    pub summary: String,
    pub body_json: serde_json::Value,
}

pub struct PeerInput {
    pub header: InputHeader,
    pub from_peer: LogicalRuntimeId,
    pub body_json: serde_json::Value,
    pub convention: Option<PeerConvention>,
}

pub enum PeerConvention {
    Message,
    Request {
        request_id: String,
        intent: Option<String>,
    },
    ResponseProgress {
        in_reply_to_request_id: String,
        phase: ResponseProgressPhase,
    },
    ResponseTerminal {
        in_reply_to_request_id: String,
        status: ResponseTerminalStatus,
    },
}

pub enum ResponseProgressPhase {
    Accepted,
    Progress,
    Checkpoint,
}

pub enum ResponseTerminalStatus {
    Completed,
    Failed,
    Cancelled,
}

pub struct FlowStepInput {
    pub header: InputHeader,
    pub flow_id: FlowId,
    pub step_id: StepId,
    pub instruction: String,
    pub overlay_json: Option<serde_json::Value>,
}

pub struct ProjectedInput {
    pub header: InputHeader,
    pub origin_event_id: RuntimeEventId,
    pub origin_kind: ProjectedEventKind,
    pub summary: String,
    pub body_json: serde_json::Value,
}

pub enum ProjectedEventKind {
    RuntimeLifecycle,
    RuntimeTopology,
    FlowLifecycle,
    InputLifecycle,
    Health,
}

pub struct ToolCallbackInput {
    pub header: InputHeader,
    pub callback_kind: String,
    pub body_json: serde_json::Value,
}
```

### 8.1 Communication Conventions

`Message`, `Request`, and `Response` are not separate foundational runtime concepts. They are optional communication conventions expressed through `PeerConvention`.

This means:

- they are available for comms, observability, tracing, and higher-level coordination
- they do not define the deepest runtime execution ontology
- the runtime can remain truthful about execution while still carrying those conventions where useful

## 9. Projection Rules

A `RuntimeEvent` MAY become an `Input` only through explicit projection.

Projection is valid only if:

- there is a declared projection rule
- there is a concrete target `LogicalRuntimeId`
- the result is a valid `Input` variant
- durability and visibility are assigned explicitly
- recovery semantics are valid for the chosen durability

Canonical defaults:

- `PeerInput` with `ResponseTerminal` is runtime-visible by default
- `PeerInput` with `ResponseProgress` is not runtime-visible by default, but is a sanctioned typed path when explicitly produced
- topology, health, and runtime lifecycle events are not runtime-visible by default
- `ProjectedInput` is the generic path for non-peer-runtime-event projection

`ResponseProgress` MUST NOT be tunneled through `ProjectedInput` when it is reasoning-relevant.

## 10. Input Durability Rules

### 10.1 Durable

- accepted durable input MUST get `InputState`
- `InputState` MUST be persisted before acceptance acknowledgement
- `InputState` MUST survive crash/restart

### 10.2 Ephemeral

- accepted ephemeral input MUST get `InputState`
- `InputState` MAY be memory-only
- it MAY be lost on crash/restart

### 10.3 Derived

- accepted derived input MUST get `InputState`
- the `InputState` is not the authoritative persisted source of truth
- the runtime MUST maintain or already possess a canonical reconstruction source
- on recovery, a fresh `InputState` MUST be rebuilt from that source instead of replaying the old derived state as authoritative work

External/public ingress MUST NOT directly submit `Input` with `durability = Derived`.

`Derived` MAY only be used when:

- a canonical durable source of truth exists
- reconstruction is deterministic enough to avoid duplicate correctness effects
- loss affects convenience or latency only, not correctness
- the runtime can rebuild the input before autonomous execution resumes

`Derived` MUST NOT be used for:

- `PromptInput`
- `PeerInput` with `Message`
- `PeerInput` with `Request`
- `PeerInput` with `ResponseTerminal`
- `FlowStepInput`

## 11. Input Scope

All accepted inputs have explicit scope.

```rust
pub enum InputScope {
    Runtime {
        logical_runtime_id: LogicalRuntimeId,
    },
    FlowStep {
        logical_runtime_id: LogicalRuntimeId,
        flow_id: FlowId,
        step_id: StepId,
    },
    ConversationContinuation {
        logical_runtime_id: LogicalRuntimeId,
        conversation_id: ConversationId,
    },
}
```

Defaults:

- `PromptInput`, `ExternalEventInput`, `PeerInput`, `ToolCallbackInput` -> `InputScope::Runtime`
- `FlowStepInput` -> `InputScope::FlowStep`
- strictly local runtime continuation work -> `InputScope::ConversationContinuation`

Ownership is by `LogicalRuntimeId`, not by transient runtime instance identity.

## 12. PolicyDecision

`PolicyDecision` is internal runtime machinery. It is not a foundational public concept, but it is part of the runtime contract.

```rust
pub enum ApplyMode {
    Ignore,
    InjectNow,
    StageRunStart,
    StageRunBoundary,
}

pub enum WakeMode {
    None,
    WakeIfIdle,
}

pub enum QueueMode {
    None,
    Fifo,
    Coalesce,
    Supersede,
}

pub enum ConsumePoint {
    OnAccept,
    OnApply,
    OnRunStart,
    OnRunComplete,
    ExplicitAck,
}

pub struct PolicyDecision {
    pub apply: ApplyMode,
    pub wake: WakeMode,
    pub queue: QueueMode,
    pub consume_at: ConsumePoint,
    pub record_transcript: bool,
    pub emit_operator_content: bool,
}
```

Ordinary input handling MUST NOT use interrupt semantics. Interruptions are represented only by `RunControlCommand`.

## 13. InputState

`InputState` is the canonical lifecycle ledger for every accepted input.

Only durable inputs are required to have persisted `InputState`.

```rust
pub struct InputState {
    pub input: Input,
    pub scope: InputScope,
    pub state: InputLifecycleState,
    pub accepted_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub attempt_count: u32,
    pub recovery_count: u32,
    pub last_policy_snapshot: Option<PolicySnapshot>,
    pub reconstruction_source: Option<ReconstructionSource>,
    pub terminal_outcome: Option<InputTerminalOutcome>,
    pub history: Vec<InputStateEvent>,
}

pub struct PolicySnapshot {
    pub decision: PolicyDecision,
    pub policy_version: PolicyVersion,
    pub computed_at_unix_ms: u64,
}

pub enum ReconstructionSource {
    RuntimeEvent { event_id: RuntimeEventId },
    Input { input_id: InputId },
    ConversationState { conversation_id: ConversationId, key: String },
    FlowState { flow_id: FlowId, step_id: Option<StepId>, key: String },
}

pub enum InputLifecycleState {
    Accepted,
    Queued,
    Staged,
    AppliedPendingConsumption,
    Consumed,
    Coalesced,
    Superseded,
    Abandoned,
}

pub enum InputTerminalOutcome {
    Consumed { run_id: Option<RunId> },
    Coalesced { aggregate_input_id: InputId },
    Superseded { by_input_id: InputId },
    Abandoned { reason: InputAbandonReason },
}

pub enum InputAbandonReason {
    RuntimeRetired,
    RuntimeReset,
    RuntimeDestroyed,
    RecoveryFailed,
    RetryBudgetExceeded,
    ExplicitCancelled,
}

pub struct InputStateEvent {
    pub at_unix_ms: u64,
    pub kind: InputStateEventKind,
    pub from_state: Option<InputLifecycleState>,
    pub to_state: InputLifecycleState,
    pub reason: Option<String>,
}

pub enum InputStateEventKind {
    Accepted,
    Deduplicated { existing_input_id: InputId },
    Queued,
    Staged { boundary: RunBoundary },
    Applied { boundary: RunBoundary },
    ContinuationScheduled,
    Consumed { run_id: Option<RunId> },
    Coalesced { aggregate_input_id: InputId },
    Superseded { by_input_id: InputId },
    Abandoned { reason: InputAbandonReason },
    Recovered,
}
```

Terminal states:

- `Consumed`
- `Coalesced`
- `Superseded`
- `Abandoned`

## 14. Acceptance Rules

```rust
pub enum AcceptOutcome {
    Accepted { input_id: InputId },
    Deduplicated { existing_input_id: InputId },
    Rejected { reason: String },
}
```

Rules:

- every accepted input gets `InputState`
- `Deduplicated` creates no new `InputState`
- exact duplicate deliveries by `idempotency_key` MUST return the existing `input_id`
- `Deduplicated` MUST emit `RuntimeEvent::InputLifecycle(Deduplicated)`
- rebuilt derived inputs MAY receive new `input_id`s after recovery
- durable persisted inputs retain stable `input_id`s across recovery

## 15. Input State Machine

These transitions are mandatory.

| From | Event | To | Notes |
|---|---|---|---|
| none | unique input accepted | `Accepted` | all accepted inputs get state |
| `Accepted` | policy computed and queued | `Queued` | default path |
| `Queued` | selected for run-boundary application | `Staged` | boundary recorded |
| `Staged` | core confirms boundary applied | `AppliedPendingConsumption` | side effects are present |
| `AppliedPendingConsumption` | consume point reached | `Consumed` | terminal |
| `Queued` | superseded | `Superseded` | terminal |
| `Queued` | coalesced into aggregate | `Coalesced` | terminal |
| `Accepted`, `Queued`, `Staged`, `AppliedPendingConsumption` | retire/reset/destroy/recovery failure | `Abandoned` | terminal |
| `AppliedPendingConsumption` | run failed before consume point | `AppliedPendingConsumption` | continuation only |

Hard rules:

- `AppliedPendingConsumption` MUST NOT return to `Queued`
- `AppliedPendingConsumption` MUST NOT re-emit conversation or system-context side effects
- `AppliedPendingConsumption` MAY only:
  - schedule continuation
  - become `Consumed`
  - become `Abandoned`

## 16. Coalescing and Supersession

Supersession scope is:

`(logical_runtime_id, input_variant, supersession_key)`

Cross-kind supersession is forbidden.

Coalescing is allowed only for:

- `ExternalEventInput`
- `PeerInput` with `ResponseProgress`
- `ProjectedInput`

Coalescing rules:

- a new aggregate `InputState` MUST be created
- source inputs MUST transition to `Coalesced { aggregate_input_id }`
- the aggregate input MUST retain lineage to source input IDs
- in-place mutation of queued input is forbidden

## 17. Canonical Default Policy

These defaults are normative.

| Input | Runtime idle | Runtime running |
|---|---|---|
| `PromptInput` | `StageRunStart`, `WakeIfIdle`, `Fifo`, `OnRunComplete`, transcript=true | `StageRunStart`, `None`, `Fifo`, `OnRunComplete`, transcript=true |
| `ExternalEventInput` | `StageRunStart`, `WakeIfIdle`, `Fifo`, `OnRunComplete`, transcript=true | `StageRunStart`, `None`, `Fifo`, `OnRunComplete`, transcript=true |
| `PeerInput` with no convention, `Message`, or `Request` | `StageRunStart`, `WakeIfIdle`, `Fifo`, `OnRunComplete`, transcript=true | `StageRunStart`, `None`, `Fifo`, `OnRunComplete`, transcript=true |
| `PeerInput` with `ResponseProgress` | `StageRunBoundary`, `None`, `Coalesce`, `OnRunComplete`, transcript=true | `StageRunBoundary`, `None`, `Coalesce`, `OnRunComplete`, transcript=true |
| `PeerInput` with `ResponseTerminal` | `StageRunStart`, `WakeIfIdle`, `Fifo`, `OnRunComplete`, transcript=true | `StageRunBoundary`, `None`, `Fifo`, `OnRunComplete`, transcript=true |
| `FlowStepInput` | `StageRunStart`, `WakeIfIdle`, `Fifo`, `OnRunComplete`, transcript=true | `StageRunStart`, `None`, `Fifo`, `OnRunComplete`, transcript=true |
| `ProjectedInput` | `Ignore`, `None`, `None`, `OnAccept`, transcript=false | `Ignore`, `None`, `Coalesce`, `OnAccept`, transcript=false |
| `ToolCallbackInput` | `StageRunBoundary`, `WakeIfIdle`, `Fifo`, `OnRunComplete`, transcript=true | `StageRunBoundary`, `None`, `Fifo`, `OnRunComplete`, transcript=true |

Profile overrides MAY change:

- queue mode for `ExternalEventInput`, `ResponseProgress`, and `ProjectedInput`
- wake mode for `ResponseProgress`
- transcript recording for `ProjectedInput`
- batching windows and explicit priority classes

Profile overrides MUST NOT change:

- `PromptInput`, `PeerInput` with `Request`, `PeerInput` with `ResponseTerminal`, or `FlowStepInput` from durable to ephemeral
- `ResponseTerminal` from runtime-visible to control-plane-only
- run control semantics
- crash recovery guarantees

## 18. RunBoundary and RunPrimitive

`Run` is the public concept. `RunPrimitive` is its core-facing application primitive.

```rust
pub enum RunBoundary {
    Immediate,
    RunStart,
    RunBoundary,
}

pub enum CoreRenderable {
    Text(String),
    Json(serde_json::Value),
    Reference { uri: String, mime: String, label: Option<String> },
}

pub enum ConversationAppendRole {
    User,
    Assistant,
    SystemNotice,
    Tool,
}

pub struct ConversationAppend {
    pub role: ConversationAppendRole,
    pub body: CoreRenderable,
    pub input_refs: Vec<InputId>,
}

pub struct ConversationContextAppend {
    pub body: CoreRenderable,
    pub input_refs: Vec<InputId>,
}

pub struct StagedRunInput {
    pub body: CoreRenderable,
    pub input_refs: Vec<InputId>,
}

pub struct RunPrimitive {
    pub boundary: RunBoundary,
    pub conversation_appends: Vec<ConversationAppend>,
    pub conversation_context_appends: Vec<ConversationContextAppend>,
    pub staged_input: Option<StagedRunInput>,
    pub contributing_input_ids: Vec<InputId>,
}
```

Rules:

- core MUST preserve `input_refs`
- core MUST echo `contributing_input_ids` back in `RunEvent`
- core MUST NOT interpret them
- core MUST NOT dedupe, reorder, or classify by these identifiers

## 19. Persisted Evidence of Boundary Application

For recovery, “persisted evidence proves the boundary was already applied” means all of the following:

- a `RunBoundaryReceipt` exists for the staged primitive
- the receipt references the same `contributing_input_ids`
- every referenced conversation/system-context append identified by the receipt is durably present
- if a staged run input existed, durable pending-run state contains the same digest and same `input_refs`

If any of those conditions are missing, the boundary is not proven applied.

```rust
pub struct RunBoundaryReceipt {
    pub run_id: Option<RunId>,
    pub boundary: RunBoundary,
    pub contributing_input_ids: Vec<InputId>,
    pub conversation_append_digests: Vec<String>,
    pub context_append_digests: Vec<String>,
    pub staged_input_digest: Option<String>,
    pub applied_at_unix_ms: u64,
}
```

## 20. RunControlCommand

Interruptions are separate from run primitives.

```rust
pub enum RunControlCommand {
    CancelCurrentRun { reason: String },
    StopRuntimeExecutor { reason: String },
}
```

Implementation requirements:

- `RunControlCommand` MUST travel on an out-of-band control path
- it MUST NOT share the same FIFO queue as `RunPrimitive`
- core MUST observe it at all long await points, including LLM streaming and tool waits

## 21. RunEvent

Core reports only run-interface events.

```rust
pub enum RunEvent {
    RunStarted {
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
    },
    BoundaryApplied {
        run_id: RunId,
        boundary: RunBoundary,
        contributing_input_ids: Vec<InputId>,
    },
    RunCompleted {
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
    },
    RunFailed {
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        error: String,
    },
    RunCancelled {
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        reason: String,
    },
}
```

## 22. Runtime State Machine

```rust
pub enum RuntimeState {
    Recovering,
    Idle,
    Running { run_id: RunId },
    Cancelling { run_id: RunId },
    Stopping,
    Stopped,
    Failed,
}
```

Required transitions:

| From | Event | To |
|---|---|---|
| `Recovering` | durable input state loaded, derived input rebuilt, core bound | `Idle` |
| `Idle` | wakeable queued/staged input dispatched | `Running` |
| `Running` | `RunCompleted` and more wakeable input exists | `Running` |
| `Running` | `RunCompleted` and no wakeable input exists | `Idle` |
| `Running` | `CancelCurrentRun` | `Cancelling` |
| `Cancelling` | `RunCancelled` | `Idle` or `Stopping` |
| `Idle` or `Running` | stop requested | `Stopping` |
| `Stopping` | executor stopped | `Stopped` |
| any non-`Stopped` | unrecoverable runtime failure | `Failed` |

Driver/runtime rules:

- while `Running`, newly accepted input MAY be queued or staged but MUST NOT interrupt
- terminal peer responses arriving while `Running` MUST stage at `RunBoundary`
- terminal peer responses arriving while `Idle` MUST wake
- progress peer responses MUST NOT wake by default

## 23. Runtime Interfaces

```rust
#[async_trait::async_trait]
pub trait CoreExecutor {
    async fn apply(&mut self, primitive: RunPrimitive) -> Result<(), String>;
    async fn control(&mut self, command: RunControlCommand) -> Result<(), String>;
}

#[async_trait::async_trait]
pub trait RuntimeDriver {
    async fn accept_input(
        &mut self,
        input: Input,
        scope: InputScope,
    ) -> Result<AcceptOutcome, String>;

    async fn on_runtime_event(&mut self, event: RuntimeEvent) -> Result<(), String>;

    async fn on_run_event(&mut self, event: RunEvent) -> Result<(), String>;

    async fn on_runtime_control(&mut self, command: RuntimeControlCommand) -> Result<(), String>;

    async fn recover(&mut self) -> Result<RecoveryReport, String>;
}

pub enum RuntimeControlCommand {
    Stop { reason: String },
    Resume,
}

#[async_trait::async_trait]
pub trait RuntimeControlPlane {
    async fn ingest(
        &self,
        target: LogicalRuntimeId,
        input: Input,
        scope: InputScope,
    ) -> Result<AcceptOutcome, String>;

    async fn publish_event(&self, event: RuntimeEvent) -> Result<(), String>;

    async fn retire_runtime(
        &self,
        runtime: LogicalRuntimeId,
        replacement_planned: bool,
    ) -> Result<RetireReport, String>;

    async fn respawn_runtime(&self, runtime: LogicalRuntimeId) -> Result<RespawnReport, String>;

    async fn reset_runtime(&self, reason: String) -> Result<ResetReport, String>;

    async fn recover_runtime(&self) -> Result<RecoveryReport, String>;
}
```

## 24. Recovery

Recovery is mandatory and ordered.

Required order:

1. recover persisted durable `InputState`
2. rebuild derived `InputState`
3. rebuild queues, coalescing indexes, and supersession indexes
4. inspect persisted conversation state and run-boundary receipts
5. reconcile staged and applied input
6. only then resume autonomous execution

Recovery rules:

- `Accepted` -> `Queued`
- `Queued` -> `Queued`
- `Staged` -> `Queued` unless persisted evidence proves already applied
- `AppliedPendingConsumption` -> `AppliedPendingConsumption`
- terminal states remain terminal

Derived recovery rules:

- derived inputs MUST be rebuilt from canonical source
- rebuilt derived inputs MAY receive new `input_id`
- inability to rebuild derived input MUST emit operator-visible diagnostics

Ephemeral recovery rule:

- ephemeral accepted input MAY be lost on crash/restart

## 25. Retire, Respawn, Reset, Destroy

These rules are mandatory.

Retire without replacement:

- all non-terminal durable input owned by that logical runtime MUST transition to `Abandoned { RuntimeRetired }`

Respawn same logical runtime:

- durable pending input remains attached to the same `LogicalRuntimeId`
- a replacement runtime instance binds to the same logical owner
- derived input is recomputed
- ephemeral input is dropped

Reset runtime:

- all non-terminal durable input MUST transition to `Abandoned { RuntimeReset }`

Destroy runtime:

- all non-terminal durable input MUST transition to `Abandoned { RuntimeDestroyed }`

Crash with unrecoverable durable store failure:

- all unrecoverable durable input MUST surface as operator-visible recovery failure
- the runtime MUST NOT silently resume with empty queue

## 26. Observability Contract

The runtime MUST internally observe every `RuntimeEvent`.

The runtime MUST emit operator-visible events for:

- durable input acceptance
- deduplication
- coalescing
- supersession
- staging
- boundary application
- consumption
- abandonment
- recovery start and completion
- retire/reset/destroy outcomes
- runtime crash and recovery
- critical health events

The runtime MAY sample or aggregate high-volume topology and health surfaces live, but canonical state MUST remain queryable.

Transcript visibility is controlled only by `PolicyDecision.record_transcript`.

## 27. Compliance Requirements

A runtime/control-plane implementation is compliant only if all are true:

- durable input is persisted before durable acceptance acknowledgement
- all accepted input gets `InputState`
- durable input state is persisted
- `AppliedPendingConsumption` is never re-applied
- durable input is never silently dropped
- recovery completes before autonomous execution resumes
- core receives only `RunPrimitive` and `RunControlCommand`
- orchestration semantics never cross into core
- opaque input refs survive end-to-end

## 28. Mandatory Conformance Tests

The conformance suite MUST include at least:

- unique durable input accepted and persisted before ack
- duplicate ingest returns `Deduplicated` and creates no new input state
- supersession within `(runtime, kind, supersession_key)` scope
- coalescing into new aggregate input state
- terminal peer response while idle
- terminal peer response while running
- progress peer response explicitly surfaced as `PeerInput(ResponseProgress)`
- retire without replacement
- respawn same logical runtime
- runtime reset
- crash during queued input
- crash during staged input
- crash during `AppliedPendingConsumption`
- repeated run failures while input remains `AppliedPendingConsumption`, with no duplicate side effects
- ephemeral accepted input lost across crash
- derived input rebuilt with new `input_id`
- cancel during long-running LLM wait
- cancel during long-running tool wait

## 29. Mob Alignment

This ontology applies cleanly to Mob:

- `Mob` is a structured orchestration layer over multiple runtimes
- `Member` is a runtime participating in a mob
- `Wiring` and `Topology` are control-plane structure over peers
- `Flow` and `Step` are explicit structured orchestration above runtime input/run semantics

Mob does not introduce a second foundational execution ontology.

## 30. Final Architectural Consequences

This specification means:

- public and internal models share one conceptual spine
- `meerkat-core` remains small and orchestration-agnostic
- runtime/control-plane owns lifecycle, queueing, and recovery
- peer `Message/Request/Response` survive as conventions without being mistaken for foundational execution primitives
- sub-agents fit naturally as runtimes invoked by other runtimes
- async tool or callback completions fit naturally as input, without inventing a grand cross-system operation ontology
- the input state ledger, not conversation transcript, is the source of truth for pending work

## 31. Filename Stability Rule

This document is a canonical standalone reference saved outside the repository and may be copied into ADRs, docs, or implementation plans.

Future revisions MUST either:

- update this file in place with a new version marker, or
- create a clearly versioned successor

Silent semantic drift outside this document is forbidden.
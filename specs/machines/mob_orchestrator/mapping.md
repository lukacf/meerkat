# MobOrchestratorMachine Mapping Note

This note maps the normative `0.5` `MobOrchestratorMachine` contract onto
current `0.4` anchors.

## Rust anchors

- central owner and command serialization:
  - `meerkat-mob/src/runtime/actor.rs`
- definition-level orchestrator identity:
  - `meerkat-mob/src/definition.rs`
  - `meerkat-mob/src/validate.rs`
- runtime builder/runtime-mode coordination:
  - `meerkat-mob/src/runtime/builder.rs`
  - `meerkat-mob/src/runtime/handle.rs`

## What is already aligned

- current code already has a distinguished orchestrator profile in definition
- actor-owned orchestration already tracks pending spawns, flow tasks, host
  loops, and runtime-mode coordination
- auto-wiring and lifecycle notifications already treat the orchestrator as a
  first-class coordination role

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- concrete roster/member identities
- topology rule contents
- supervisor escalation payloads
- exact spawn tickets and runtime-mode details
- task-board contents
- flow-engine inner behavior

Those refine orchestration ownership but do not change the top-level
coordination contract.

## Important `0.5` clarification

Today, "orchestrator" is more owner-semantics than a clearly named state
machine. The formal `0.5` contract makes that ownership explicit.

This is intentional:

- `MobLifecycleMachine` owns top-level lifecycle phase
- `FlowRunMachine` owns durable per-run semantics
- `MobOrchestratorMachine` owns coordination and authority across them

## Known `0.4` divergence

- current orchestrator identity is derived from definition + roster rather
  than an explicit named orchestration owner
- `MobActor` still centralizes the orchestration concerns this contract makes
  explicit

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobOrchestratorMachine`

### Code Anchors
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — orchestration owner precursor
- `mob_runtime_builder`: `meerkat-mob/src/runtime/builder.rs` — runtime-mode/builder orchestration precursor
- `mob_definition`: `meerkat-mob/src/definition.rs` — definition-level coordinator/topology precursor

### Scenarios
- `coordinator-bind-and-supervise` — orchestrator binds coordinator authority and supervision
- `pending-spawn-ledger` — pending spawn and completion semantics remain explicit
- `topology-revision` — topology/orchestration revisions remain monotonic and owned

### Transitions
- `InitializeOrchestrator`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`
- `BindCoordinator`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`
- `UnbindCoordinator`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`
- `StageSpawn`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `pending-spawn-ledger`
- `CompleteSpawn`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `pending-spawn-ledger`
- `StartFlow`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`
- `CompleteFlow`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`
- `StopOrchestrator`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`
- `ResumeOrchestrator`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`
- `MarkCompleted`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`
- `DestroyOrchestrator`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`

### Effects
- `ActivateSupervisor`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`
- `DeactivateSupervisor`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`
- `FlowActivated`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`
- `FlowDeactivated`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`
- `EmitOrchestratorNotice`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`

### Invariants
- `destroyed_is_terminal`
  - anchors: `mob_runtime_actor`, `mob_runtime_builder`, `mob_definition`
  - scenarios: `coordinator-bind-and-supervise`, `topology-revision`


<!-- GENERATED_COVERAGE_END -->

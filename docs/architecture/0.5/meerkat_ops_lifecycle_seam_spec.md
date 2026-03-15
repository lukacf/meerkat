# Meerkat 0.5 OpsLifecycleMachine Seam Spec

Status: normative `0.5` seam/ownership spec

Companion machine contract:

- `specs/machines/ops_lifecycle/contract.md`

## Purpose

The formal `OpsLifecycleMachine` semantics are already specified and
model-checked.

This document defines the missing concrete choices the formal model does not
choose by itself:

- the concrete Rust owner
- the concrete Rust trait seam
- the exact convergence path from today's split child-agent and background-op
  implementations to one shared async-operation lifecycle substrate

`0.5` does not keep a separate lightweight subagent mechanism. There are only
two user-facing agent wiring levels:

- `Comms`
- `Mobs`

Child-agent behavior therefore lives on the mob path, while async/background
tool work shares the same lifecycle substrate without becoming a third wiring
level.

## Firm Decisions

1. `OpsLifecycleMachine` is the common lifecycle substrate for all async
   operations.
2. Child-agent orchestration is mob-only in `0.5`; "subagent" is a mob
   preset/workflow, not a separate mechanism.
3. Parent/child conversation remains owned by `PeerCommsMachine`.
4. `OpsLifecycleMachine` owns only lifecycle state:
   - register
   - provision
   - peer-ready
   - progress
   - completion
   - failure
   - cancellation
   - retire
   - terminal observation
5. Background tool operations, including shell jobs, use the same lifecycle
   substrate but do not participate in the peer plane.
6. Transcript notices, tool-return summaries, and UI summaries are derivative
   projections of typed lifecycle state and typed `OperationInput`; they are
   not the authoritative lifecycle contract.
7. The authoritative owner lives per runtime/session instance in
   `meerkat-runtime`.
8. A session belongs to at most one active mob at a time.

## Final Ownership

### Contract crate

Core contract types and traits live in:

- `meerkat-core/src/ops_lifecycle.rs`

This module becomes the lifecycle counterpart to today's broader
`meerkat-core/src/ops.rs`.

### Runtime owner

The authoritative per-session owner lives in:

- `meerkat-runtime/src/ops_lifecycle.rs`

Canonical owner type:

- `RuntimeOpsLifecycleRegistry`

It is stored alongside the runtime/session entry managed by
`RuntimeSessionAdapter`.

### Mob adapter layer

Mob-specific adaptation lives in:

- `meerkat-mob/src/runtime/ops_adapter.rs`

That adapter translates mob provisioning, member adoption, and roster concerns
into the shared registry contract, but it is not itself the lifecycle owner.

### Background-tool adapters

Background-tool adaptation lives in the owning tool/runtime crates.

Shell jobs are a special case of background tool operation. Shell code may own
PID/process-group/stdout buffering mechanics, but it may not own the
authoritative lifecycle truth.

## Canonical Core Types

The final `0.5` contract surface should include the following types.

### Identity and kind

- `OperationId`
- `OperationKind`
  - `MobMemberChild`
  - `BackgroundToolOp`

### Registration/spec

`OperationSpec` contains lifecycle-relevant data:

- `id: OperationId`
- `kind: OperationKind`
- `owner_session_id: SessionId`
- `display_name: String`
- `source_label: String`
- `child_session_id: Option<SessionId>`
- `expect_peer_channel: bool`

Rules:

- child-agent operations set `expect_peer_channel = true`
- background tool operations set `expect_peer_channel = false`
- prompt text, budget, tool policy, flow-step data, and other spawn/tool
  policy inputs are not owned by the lifecycle machine once registration
  occurs

### Peer exposure

`OperationPeerHandle` contains the peer-facing connection surface exposed once
the operation is ready to participate in `PeerCommsMachine`.

Minimum fields:

- `peer_name: String`
- `trusted_peer: TrustedPeerSpec`

This is valid only for operation kinds that actually peer.

### Progress

`OperationProgressUpdate` contains:

- `message: String`
- `percent: Option<f32>`

### Terminal outcome

`OperationTerminalOutcome` is the typed terminal record:

- `Completed(OperationResult)`
- `Failed { error: String }`
- `Cancelled { reason: Option<String> }`
- `Retired`
- `Terminated { reason: String }`

### Snapshot

`OperationLifecycleSnapshot` contains:

- `id`
- `kind`
- `display_name`
- `status`
- `peer_ready`
- `progress_count`
- `watcher_count`
- `terminal_outcome`
- `child_session_id`

## Canonical Trait Seam

The core trait is:

- `OpsLifecycleRegistry`

It should be defined in `meerkat-core` and implemented by
`RuntimeOpsLifecycleRegistry`.

Required methods:

- `register_operation(spec: OperationSpec) -> Result<(), OpsLifecycleError>`
- `provisioning_succeeded(id: &OperationId) -> Result<(), OpsLifecycleError>`
- `provisioning_failed(id: &OperationId, error: String) -> Result<(), OpsLifecycleError>`
- `peer_ready(id: &OperationId, peer: OperationPeerHandle) -> Result<(), OpsLifecycleError>`
- `register_watcher(id: &OperationId) -> Result<OperationCompletionWatch, OpsLifecycleError>`
- `report_progress(id: &OperationId, update: OperationProgressUpdate) -> Result<(), OpsLifecycleError>`
- `complete_operation(id: &OperationId, result: OperationResult) -> Result<(), OpsLifecycleError>`
- `fail_operation(id: &OperationId, error: String) -> Result<(), OpsLifecycleError>`
- `cancel_operation(id: &OperationId, reason: Option<String>) -> Result<(), OpsLifecycleError>`
- `request_retire(id: &OperationId) -> Result<(), OpsLifecycleError>`
- `mark_retired(id: &OperationId) -> Result<(), OpsLifecycleError>`
- `snapshot(id: &OperationId) -> Option<OperationLifecycleSnapshot>`
- `list_operations() -> Vec<OperationLifecycleSnapshot>`
- `terminate_owner(reason: String) -> Result<(), OpsLifecycleError>`

`OperationCompletionWatch` should be a multi-waiter completion surface that
resolves exactly once with the operation's terminal outcome.

The registry may expose an internal event stream for runtime/operator
surfaces, but the methods above are the mandatory semantic seam.

## Runtime Admission Contract

`OpsLifecycleMachine` produces typed `OperationInput` into runtime admission.

Final `0.5` rule:

- runtime input taxonomy includes explicit operation/lifecycle admission
- lifecycle events are not smuggled through peer comms, transcript-only side
  effects, or shell-local job registries

Final runtime input target:

- `Input::Operation(OperationInput)`

`OperationInput` should minimally contain:

- `header: InputHeader`
- `event: OpEvent`
- `operation_id: OperationId`

## Comms And Mob Relationship

### Comms

`Comms` remains the lightweight peer plane. It does not spawn children and it
does not own lifecycle truth.

### Mobs

Mobs are the only child-agent orchestration path in `0.5`.

A "spawn a subagent" request therefore means:

1. create or reuse a mob
2. ensure the calling agent/session is the orchestrator
3. add a member
4. wire the member
5. observe progress and terminality through the shared ops lifecycle substrate

Required mob control-plane capabilities include:

- `attach_existing_session_as_orchestrator`
- `attach_existing_session_as_member`
- `spawn_member`
- `wire_member`

`SubAgentManager` is deleted. No separate subagent mechanism, public owner, or
parallel crate-level path survives.

## Mob Member Mapping

Today's anchors:

- `meerkat-mob/src/runtime/provisioner.rs`
- `meerkat-mob/src/runtime/actor.rs`

Final mapping:

### Provisioning

When mob code decides to create or adopt a member:

1. allocate `OperationId`
2. call `register_operation(kind = MobMemberChild, ...)`
3. create or attach the member session/runtime backing
4. call `provisioning_succeeded(...)` or `provisioning_failed(...)`
5. once comms wiring/trust is ready, call `peer_ready(...)`

### Runtime turns / progress / terminality

- mob code may still decide **when** a member should run
- the shared registry owns **what lifecycle state that member is in**
- terminal member outcomes resolve through `complete_operation`,
  `fail_operation`, `cancel_operation`, `request_retire`, and `mark_retired`

### What remains mob-local

- topology/roster policy
- flow dependency policy
- spawn policy
- supervision policy
- task-board policy

Release rule:

- mob member lifecycle must stop being owned directly by actor/provisioner-local
  ad hoc state

## Background Tool Mapping

Background tool work uses the same lifecycle substrate without becoming a mob
or peer concern.

### Registration

When a background tool operation is started:

1. allocate `OperationId`
2. call `register_operation(kind = BackgroundToolOp, ...)`
3. spawn the tool-side effectful work
4. call `provisioning_succeeded(...)` once the work is live
5. report progress, completion, failure, cancellation, and retirement through
   the shared registry

### What stays tool-local

- shell PID/process-group handling
- stdout/stderr buffering
- transport/client details for remote/background tool execution

Release rule:

- shell-local or tool-local job managers must stop being authoritative owners
  of terminality, watchers, or progress truth

## PeerComms Relationship

`OpsLifecycleMachine` does not own child conversation.

The split is:

- `OpsLifecycleMachine`
  - register/provision/progress/terminal lifecycle
- `PeerCommsMachine`
  - peer wiring
  - send/receive
  - request/response registry
  - trust/auth classification

Bridge rule:

- `peer_ready(...)` is the lifecycle handoff point that allows a mob member to
  participate in `PeerCommsMachine`

## Wait / Completion Relationship

The shared registry is the authoritative completion source for:

- mob-backed child wait interruption
- mob-member completion observation
- background tool completion observation
- terminal inspection APIs

This means:

- wait-style interruption on child completion should bind to registry
  completion watches or derived runtime notices
- transcript-visible completion summaries remain derivative projections

## Concrete Rust Integration Points

### `RuntimeSessionAdapter`

`RuntimeSessionAdapter` should own or expose:

- `ops_registry(session_id) -> Arc<dyn OpsLifecycleRegistry>`

That is the canonical access point for both mob orchestration code and
background-tool code.

### `meerkat-mob`

`meerkat-mob` should own the child-orchestration control plane, including
orchestrator/member attachment and member spawn/wire flows.

It should not own the lifecycle truth for those members directly.

### Tool/background owners

Background tool crates should call into `OpsLifecycleRegistry` for lifecycle
state and watches rather than maintaining separate authoritative completion
stores.

## Required Deletions

`0.5` should delete the following as authoritative lifecycle owners or public
contracts:

- `SubAgentManager`
- `SubagentResult`
- mob-local member completion tracking as a separate lifecycle owner
- transcript/tool-result delivery as the primary child-completion or
  background-op completion contract
- shell-local job registries as the primary async-op truth

These may survive only as projections or bounded caches over the shared
registry.

## Completion Criteria

This seam is finished only when all are true:

1. mob-backed child work and background tool operations both use the same
   per-session `OpsLifecycleRegistry`
2. no separate subagent mechanism or owner survives
3. child completion/progress/retire state no longer has separate authoritative
   owners in `meerkat-core`, `meerkat-mob`, and tool-local runtimes
4. typed `OperationInput` enters runtime admission explicitly
5. transcript/tool-result summaries are derivative projections
6. the formal machine maps directly to one concrete Rust owner

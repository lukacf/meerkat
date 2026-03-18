# OpsLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-runtime` / `generated::ops_lifecycle`

## State
- Phase enum: `Active`
- `known_operations`: `Set<OperationId>`
- `operation_status`: `Map<OperationId, OperationStatus>`
- `operation_kind`: `Map<OperationId, OperationKind>`
- `peer_ready`: `Map<OperationId, Bool>`
- `progress_count`: `Map<OperationId, u32>`
- `watcher_count`: `Map<OperationId, u32>`
- `terminal_outcome`: `Map<OperationId, OperationTerminalOutcome>`
- `terminal_buffered`: `Map<OperationId, Bool>`
- `completed_order`: `Seq<OperationId>`
- `max_completed`: `u32`
- `max_concurrent`: `u32`
- `active_count`: `u32`
- `created_at_ms`: `Map<OperationId, u64>`
- `completed_at_ms`: `Map<OperationId, u64>`

## Inputs
- `RegisterOperation`(operation_id: OperationId, operation_kind: OperationKind)
- `ProvisioningSucceeded`(operation_id: OperationId)
- `ProvisioningFailed`(operation_id: OperationId)
- `PeerReady`(operation_id: OperationId)
- `RegisterWatcher`(operation_id: OperationId)
- `ProgressReported`(operation_id: OperationId)
- `CompleteOperation`(operation_id: OperationId)
- `FailOperation`(operation_id: OperationId)
- `CancelOperation`(operation_id: OperationId)
- `RetireRequested`(operation_id: OperationId)
- `RetireCompleted`(operation_id: OperationId)
- `CollectTerminal`(operation_id: OperationId)
- `OwnerTerminated`
- `WaitAll`(operation_ids: Seq<OperationId>)
- `CollectCompleted`

## Effects
- `SubmitOpEvent`(operation_id: OperationId, event_kind: OpEventKind)
- `NotifyOpWatcher`(operation_id: OperationId, terminal_outcome: OperationTerminalOutcome)
- `ExposeOperationPeer`(operation_id: OperationId)
- `RetainTerminalRecord`(operation_id: OperationId, terminal_outcome: OperationTerminalOutcome)
- `EvictCompletedRecord`(operation_id: OperationId)
- `WaitAllSatisfied`(operation_ids: Seq<OperationId>)
- `CollectCompletedResult`(operation_ids: Seq<OperationId>)
- `ConcurrencyLimitExceeded`(operation_id: OperationId, limit: u32, active: u32)

## Helpers
- `status_of`(operation_id: OperationId) -> `OperationStatus`
- `kind_of`(operation_id: OperationId) -> `OperationKind`
- `peer_ready_of`(operation_id: OperationId) -> `Bool`
- `watcher_count_of`(operation_id: OperationId) -> `u32`
- `progress_count_of`(operation_id: OperationId) -> `u32`
- `terminal_outcome_of`(operation_id: OperationId) -> `OperationTerminalOutcome`
- `terminal_buffered_of`(operation_id: OperationId) -> `Bool`
- `is_terminal_status`(status: OperationStatus) -> `Bool`
- `is_owner_terminatable_status`(status: OperationStatus) -> `Bool`
- `terminal_outcome_matches_status`(status: OperationStatus, terminal_outcome: OperationTerminalOutcome) -> `Bool`

## Invariants
- `terminal_buffered_only_for_terminal_states`
- `peer_ready_implies_mob_member_child`
- `peer_ready_implies_present`
- `present_operations_keep_kind_identity`
- `terminal_statuses_have_matching_terminal_outcome`
- `nonterminal_statuses_have_no_terminal_outcome`

## Transitions
### `RegisterOperation`
- From: `Active`
- On: `RegisterOperation`(operation_id, operation_kind)
- Guards:
  - `operation_absent`
  - `kind_is_real`
  - `concurrency_limit_allows`
- To: `Active`

### `ProvisioningSucceeded`
- From: `Active`
- On: `ProvisioningSucceeded`(operation_id)
- Guards:
  - `is_provisioning`
- Emits: `SubmitOpEvent`
- To: `Active`

### `ProvisioningFailed`
- From: `Active`
- On: `ProvisioningFailed`(operation_id)
- Guards:
  - `status_allows_terminalization`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`, `RetainTerminalRecord`
- To: `Active`

### `PeerReady`
- From: `Active`
- On: `PeerReady`(operation_id)
- Guards:
  - `operation_running_or_retiring`
  - `operation_is_mob_member_child`
- Emits: `ExposeOperationPeer`
- To: `Active`

### `RegisterWatcher`
- From: `Active`
- On: `RegisterWatcher`(operation_id)
- Guards:
  - `operation_exists`
- To: `Active`

### `ProgressReported`
- From: `Active`
- On: `ProgressReported`(operation_id)
- Guards:
  - `operation_running_or_retiring`
- Emits: `SubmitOpEvent`
- To: `Active`

### `CompleteOperation`
- From: `Active`
- On: `CompleteOperation`(operation_id)
- Guards:
  - `status_allows_terminalization`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`, `RetainTerminalRecord`
- To: `Active`

### `FailOperation`
- From: `Active`
- On: `FailOperation`(operation_id)
- Guards:
  - `status_allows_terminalization`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`, `RetainTerminalRecord`
- To: `Active`

### `CancelOperation`
- From: `Active`
- On: `CancelOperation`(operation_id)
- Guards:
  - `status_allows_terminalization`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`, `RetainTerminalRecord`
- To: `Active`

### `RetireRequested`
- From: `Active`
- On: `RetireRequested`(operation_id)
- Guards:
  - `operation_running`
- To: `Active`

### `RetireCompleted`
- From: `Active`
- On: `RetireCompleted`(operation_id)
- Guards:
  - `status_allows_terminalization`
- Emits: `SubmitOpEvent`, `NotifyOpWatcher`, `RetainTerminalRecord`
- To: `Active`

### `CollectTerminal`
- From: `Active`
- On: `CollectTerminal`(operation_id)
- Guards:
  - `operation_is_terminal`
  - `terminal_is_buffered`
- To: `Active`

### `OwnerTerminated`
- From: `Active`
- On: `OwnerTerminated`()
- To: `Active`

### `WaitAll`
- From: `Active`
- On: `WaitAll`(operation_ids)
- Emits: `WaitAllSatisfied`
- To: `Active`

### `CollectCompleted`
- From: `Active`
- On: `CollectCompleted`()
- Emits: `CollectCompletedResult`
- To: `Active`

## Coverage
### Code Anchors
- `meerkat-core/src/ops.rs` — shared async-operation vocabulary precursor
- `meerkat-mob/src/runtime/provisioner.rs` — mob-backed child lifecycle precursor
- `meerkat-tools/src/builtin/shell/job_manager.rs` — background tool-operation lifecycle precursor

### Scenarios
- `register-progress-terminal` — async operation registers, reports progress, and reaches a terminal outcome
- `peer-ready-handoff` — child operation hands off to peer comms at peer_ready
- `cancel-and-watch` — async operation cancellation resolves watcher semantics exactly once

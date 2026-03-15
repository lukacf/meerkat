# Typed But Unwired

This is the initial carry-forward list for types and contracts that are
described by the `0.5` docs but are not yet safely assumed to be fully wired
through the shipped runtime path.

## Runtime Taxonomy And Control

- `OperationInput`
  documented by the ops seam and implementation plan, but not yet established
  as a proven runtime input family
- explicit continuation input/primitive
  required by host-mode cutover, but still transitional in current runtime docs
- runtime-authoritative multi-contributor drain snapshots for
  `contributing_input_ids`
  present in docs and receipts, but not yet proven end-to-end
- explicit `InterruptPolicy` / `DrainPolicy` / `RoutingDisposition` ownership
  described conceptually, but not yet proven as the final implementation split

## Shared Async-Operation Lifecycle

- `OperationId`
- `OperationKind`
- `OperationSpec`
- `OperationPeerHandle`
- `OperationProgressUpdate`
- `OperationTerminalOutcome`
- `OperationLifecycleSnapshot`
- `OperationCompletionWatch`
- `OpsLifecycleRegistry`
- `RuntimeOpsLifecycleRegistry`

These are all defined by the seam docs but still need real runtime wiring and
real-entrypoint evidence.

## Host-Mode Replacement

- `RuntimeCommsBridge`
- completion-aware reservation resolution
- runtime-owned continuation scheduling after terminal peer responses
- exact control-plane stop/dismiss mapping

## External Tool Surface

- canonical typed outward lifecycle delta
- canonical typed surface update notice
- browser runtime-scoped tool registration/update surface

## Mob Decomposition

- explicit `MobOrchestratorMachine` pending-spawn/coordinator/topology-revision
  state
- explicit `FlowRunMachine` durable run truth boundary
- explicit service boundaries for topology/task-board/supervision

## Surface Contracts

- canonical REST external-event route/payload wiring
- canonical JSON-RPC external-event method wiring
- explicit WASM capability envelope and runtime-scoped browser-local tool
  surface
- mob control-plane attach/adopt helpers for "spawn helper agent" flows

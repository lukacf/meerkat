# Finite Ownership Ledger

**Status**: Generated
**Source**: `xtask ownership-ledger`

This document is generated from the typed ownership registry in `xtask`.
It is the authoritative inventory of semantic state, semantic-operation boundaries, and keyed-store invariants for the current closure program.

## Summary

| Subsystem | State Cells | Semantic Operations | Coupling Invariants | Open State Cells | Open Operations | Open Invariants |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| runtime | 7 | 22 | 5 | 0 | 0 | 0 |
| mcp | 11 | 21 | 2 | 0 | 0 | 0 |
| mob | 6 | 49 | 3 | 0 | 0 | 0 |

## Boundary Manifest

| Family | Kind | Path | Type / Trait | Methods |
| --- | --- | --- | --- | --- |
| runtime-control-plane | trait-impl | `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionAdapter` / `RuntimeControlPlane` | `ingest`, `publish_event`, `retire`, `recycle`, `reset`, `recover`, `destroy` |
| runtime-session-adapter | public-inherent | `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionAdapter` | `register_session`, `set_session_silent_intents`, `register_session_with_executor`, `ensure_session_with_executor`, `unregister_session`, `interrupt_current_run`, `stop_runtime_executor`, `accept_input_and_run`, `accept_input_with_completion`, `maybe_spawn_comms_drain`, `abort_comms_drains`, `abort_comms_drain`, `wait_comms_drain` |
| mcp-router | public-inherent | `meerkat-mcp/src/router.rs` | `McpRouter` | `set_removal_timeout`, `add_server`, `stage_add`, `stage_remove`, `stage_reload`, `apply_staged`, `take_lifecycle_actions`, `take_external_updates`, `progress_removals`, `call_tool`, `shutdown` |
| mcp-router-adapter | public-inherent | `meerkat-mcp/src/adapter.rs` | `McpRouterAdapter` | `refresh_tools`, `stage_add`, `stage_remove`, `stage_reload`, `apply_staged`, `poll_lifecycle_actions`, `progress_removals`, `wait_until_ready`, `shutdown` |
| mob-handle | public-inherent | `meerkat-mob/src/runtime/handle.rs` | `MobHandle` | `spawn`, `spawn_with_backend`, `spawn_with_options`, `attach_existing_session`, `attach_existing_session_as_orchestrator`, `attach_existing_session_as_member`, `spawn_spec`, `spawn_many`, `retire`, `respawn`, `retire_all`, `wire`, `unwire`, `internal_turn`, `run_flow`, `run_flow_with_stream`, `cancel_flow`, `stop`, `resume`, `complete`, `reset`, `destroy`, `task_create`, `task_update`, `set_spawn_policy`, `shutdown`, `force_cancel_member`, `wait_one`, `wait_all`, `spawn_helper`, `fork_helper` |
| mob-command-dispatch | enum-dispatch | `meerkat-mob/src/runtime/actor.rs` | `MobActor` / `MobCommand` | `enqueue_spawn`, `handle_force_cancel`, `handle_retire`, `handle_respawn`, `handle_wire`, `handle_unwire`, `handle_external_turn`, `handle_internal_turn`, `retire_all_members`, `handle_task_create`, `handle_task_update`, `handle_run_flow`, `handle_cancel_flow`, `handle_flow_cleanup`, `handle_complete`, `handle_destroy`, `handle_reset` |
| manual-callback | manual-callback | `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionAdapter` | `notify_comms_drain_exited` |
| manual-callback | manual-callback | `meerkat-mcp/src/router.rs` | `McpRouter` | `process_pending_result` |
| manual-callback | manual-callback | `meerkat-mob/src/runtime/actor.rs` | `MobActor` | `handle_spawn_provisioned_batch` |
| shell-background-job-view | export-contract (app-facing) | `meerkat-tools/src/builtin/shell/types.rs` | `BackgroundJob` | `exports_raw_operation_id=false` |
| shell-job-summary-view | export-contract (app-facing) | `meerkat-tools/src/builtin/shell/types.rs` | `JobSummary` | `exports_raw_operation_id=false` |
| mob-member-ref | export-contract (app-facing) | `meerkat-mob/src/event.rs` | `MemberRef` | `exports_raw_operation_id=false` |
| mob-infra-member-spawn-receipt | export-contract (infra-canonical-op) | `meerkat-mob/src/runtime/handle.rs` | `MemberSpawnReceipt` | `exports_raw_operation_id=true` |

## Runtime State Cells

| Path | Symbol | Class | Status | Anchor | Contract |
| --- | --- | --- | --- | --- | --- |
| `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionAdapter.sessions` | `capability-index` | `closed` | `RuntimeSessionAdapter registered-session + attachment publication contract` | src: `registered session entries with recovered driver/completion capabilities`; trigger: `register/ensure/attach/detach/unregister/destroy transitions + dead-attachment normalization`; stale: `forbidden` |
| `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionAdapter.comms_drain_slots` | `capability-index` | `closed` | `RuntimeSessionAdapter registered-session contract + CommsDrainLifecycleMachine` | src: `registered session keys + comms drain lifecycle slot allocation`; trigger: `register/unregister/destroy + drain lifecycle transitions + control installation`; stale: `forbidden` |
| `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionEntry.driver` | `capability-handle` | `closed` | `RuntimeControlMachine + RuntimeIngressMachine + InputLifecycleMachine` | - |
| `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionEntry.attachment` | `capability-handle` | `closed` | `RuntimeSessionAdapter attachment publication contract` | - |
| `meerkat-runtime/src/session_adapter.rs` | `RuntimeSessionEntry.completions` | `capability-handle` | `closed` | `InputLifecycle terminal wait plumbing` | - |
| `meerkat-runtime/src/driver/ephemeral.rs` | `EphemeralRuntimeDriver.queue` | `derived-projection` | `closed` | `RuntimeIngressMachine queue lane` | src: `RuntimeIngressMachine.queue entries`; trigger: `any ingress queue mutation or rollback/recovery rebuild`; stale: `forbidden` |
| `meerkat-runtime/src/driver/ephemeral.rs` | `EphemeralRuntimeDriver.steer_queue` | `derived-projection` | `closed` | `RuntimeIngressMachine steer lane` | src: `RuntimeIngressMachine.steer entries`; trigger: `any ingress steer mutation or rollback/recovery rebuild`; stale: `forbidden` |

## Runtime Semantic Operations

| Path | Symbol | Boundary | Status | Anchor |
| --- | --- | --- | --- | --- |
| `meerkat-runtime/src/session_adapter.rs` | `register_session` | `public-inherent` | `closed` | `RuntimeSessionAdapter registration + recovery publication contract` |
| `meerkat-runtime/src/session_adapter.rs` | `set_session_silent_intents` | `public-inherent` | `closed` | `RuntimeIngressMachine + RuntimeControlMachine policy truth` |
| `meerkat-runtime/src/session_adapter.rs` | `register_session_with_executor` | `public-inherent` | `closed` | `RuntimeSessionAdapter registration + attachment publication contract` |
| `meerkat-runtime/src/session_adapter.rs` | `ensure_session_with_executor` | `public-inherent` | `closed` | `RuntimeSessionAdapter attachment publication contract + RuntimeControl transitions` |
| `meerkat-runtime/src/session_adapter.rs` | `unregister_session` | `public-inherent` | `closed` | `registered-session contract + CommsDrainLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `interrupt_current_run` | `public-inherent` | `closed` | `RuntimeControlMachine + runtime attachment publication contract` |
| `meerkat-runtime/src/session_adapter.rs` | `stop_runtime_executor` | `public-inherent` | `closed` | `RuntimeControlMachine + runtime attachment publication contract` |
| `meerkat-runtime/src/session_adapter.rs` | `accept_input_and_run` | `public-inherent` | `closed` | `RuntimeIngressMachine + InputLifecycleMachine + RuntimeControlMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `accept_input_with_completion` | `public-inherent` | `closed` | `RuntimeIngressMachine + InputLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `maybe_spawn_comms_drain` | `public-inherent` | `closed` | `CommsDrainLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `notify_comms_drain_exited` | `manual-callback` | `closed` | `CommsDrainLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `abort_comms_drains` | `public-inherent` | `closed` | `CommsDrainLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `abort_comms_drain` | `public-inherent` | `closed` | `CommsDrainLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `wait_comms_drain` | `public-inherent` | `closed` | `CommsDrainLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `publish_event` | `trait-impl` | `closed` | `RuntimeControlMachine + InputLifecycleMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `retire` | `trait-impl` | `closed` | `RuntimeControlMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `recycle` | `trait-impl` | `closed` | `RuntimeControlMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `reset` | `trait-impl` | `closed` | `RuntimeControlMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `recover` | `trait-impl` | `closed` | `RuntimeControlMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `destroy` | `trait-impl` | `closed` | `RuntimeControlMachine` |
| `meerkat-runtime/src/session_adapter.rs` | `ingest` | `trait-impl` | `closed` | `RuntimeIngressMachine + InputLifecycleMachine` |

## Runtime Coupling Invariants

| Name | Stores | Status | Anchor |
| --- | --- | --- | --- |
| `runtime_session_drain_subset` | `RuntimeSessionAdapter.sessions`, `RuntimeSessionAdapter.comms_drain_slots` | `closed` | `registered-session contract + CommsDrainLifecycleMachine` |
| `runtime_attachment_alignment` | `RuntimeSessionEntry.attachment`, `RuntimeSessionEntry.driver` | `closed` | `RuntimeSessionAdapter attachment publication contract + RuntimeControl transitions` |
| `runtime_queue_projection_alignment` | `RuntimeIngressMachine.queue`, `EphemeralRuntimeDriver.queue`, `EphemeralRuntimeDriver.steer_queue` | `closed` | `RuntimeIngressMachine` |
| `runtime_comms_bridge_projection_alignment` | `PeerCommsMachine.classified_interactions`, `RuntimeCommsBridge.runtime_input_projection` | `closed` | `PeerCommsMachine classification + RuntimeCommsBridge projection contract` |
| `runtime_external_event_projection_alignment` | `CLI.stdin_external_event_projection`, `PeerCommsMachine.plain_events`, `Runtime.ExternalEventInput`, `RuntimeLoop.external_event_rendering` | `closed` | `ExternalEventInput projection contract + runtime external-event render contract` |

## MCP State Cells

| Path | Symbol | Class | Status | Anchor | Contract |
| --- | --- | --- | --- | --- | --- |
| `meerkat-mcp/src/router.rs` | `McpRouter.servers` | `capability-index` | `closed` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` | src: `canonical surface state + live server handles`; trigger: `apply_staged completion, pending completion, removal finalization, shutdown`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `McpRouter.projection` | `derived-projection` | `closed` | `RouterProjectionSnapshot` | src: `ExternalToolSurfaceAuthority visibility + server manifests`; trigger: `snapshot rebuild at every visibility/routing invalidation`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `RouterProjectionSnapshot.tool_to_server` | `derived-projection` | `closed` | `RouterProjectionSnapshot` | src: `ExternalToolSurfaceAuthority visibility + server manifests`; trigger: `snapshot rebuild at every visibility/routing invalidation`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `RouterProjectionSnapshot.visible_tools` | `derived-projection` | `closed` | `RouterProjectionSnapshot` | src: `ExternalToolSurfaceAuthority visibility + server manifests`; trigger: `snapshot rebuild at every visibility/routing invalidation`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `RouterProjectionSnapshot.epoch` | `derived-projection` | `closed` | `ExternalToolSurfaceAuthority snapshot_epoch` | src: `ExternalToolSurfaceAuthority snapshot publication epoch`; trigger: `projection snapshot rebuild/publication`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `McpRouter.pending_obligations` | `capability-index` | `closed` | `surface_completion handoff protocol obligation identity` | src: `generated SurfaceCompletionObligation tokens from authority effects`; trigger: `schedule-surface-completion spawn + pending-result consumption`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `McpRouter.pending_snapshot_alignment` | `capability-index` | `closed` | `surface_snapshot_alignment handoff protocol obligation identity` | src: `generated SurfaceSnapshotAlignmentObligation token from authority effects`; trigger: `snapshot-alignment scheduling + alignment application`; stale: `forbidden` |
| `meerkat-mcp/src/router.rs` | `McpRouter.pending_tx` | `capability-handle` | `closed` | `surface handoff protocol async completion transport` | - |
| `meerkat-mcp/src/router.rs` | `McpRouter.pending_rx` | `transport-buffer` | `closed` | `surface handoff protocol async completion transport` | - |
| `meerkat-mcp/src/router.rs` | `McpRouter.completed_updates` | `transport-buffer` | `closed` | `ExternalToolSurfaceAuthority lifecycle deltas` | - |
| `meerkat-mcp/src/router.rs` | `McpRouter.staged_payloads` | `transport-buffer` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` | - |

## MCP Semantic Operations

| Path | Symbol | Boundary | Status | Anchor |
| --- | --- | --- | --- | --- |
| `meerkat-mcp/src/router.rs` | `set_removal_timeout` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority` |
| `meerkat-mcp/src/router.rs` | `add_server` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `stage_add` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/router.rs` | `stage_remove` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/router.rs` | `stage_reload` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/router.rs` | `apply_staged` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `process_pending_result` | `manual-callback` | `closed` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `take_lifecycle_actions` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `take_external_updates` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `progress_removals` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `call_tool` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority` |
| `meerkat-mcp/src/router.rs` | `shutdown` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `refresh_tools` | `public-inherent` | `closed` | `RouterProjectionSnapshot` |
| `meerkat-mcp/src/adapter.rs` | `stage_add` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/adapter.rs` | `stage_remove` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/adapter.rs` | `stage_reload` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/adapter.rs` | `apply_staged` | `public-inherent` | `closed` | `RouterProjectionSnapshot` |
| `meerkat-mcp/src/adapter.rs` | `poll_lifecycle_actions` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `progress_removals` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `wait_until_ready` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `shutdown` | `public-inherent` | `closed` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |

## MCP Coupling Invariants

| Name | Stores | Status | Anchor |
| --- | --- | --- | --- |
| `mcp_snapshot_alignment` | `McpRouter.servers`, `RouterProjectionSnapshot` | `closed` | `ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract` |
| `mcp_pending_lineage_alignment` | `McpRouter.pending_obligations`, `ExternalToolSurfaceAuthority pending lineage + surface_completion obligations` | `closed` | `ExternalToolSurfaceAuthority pending lineage + surface_completion handoff protocol obligations` |

## Mob State Cells

| Path | Symbol | Class | Status | Anchor | Contract |
| --- | --- | --- | --- | --- | --- |
| `meerkat-mob/src/runtime/actor.rs` | `MobActor.roster` | `derived-projection` | `closed` | `RosterAuthority + spawn/retire/wire event projection contract` | src: `MeerkatSpawned/Retired + PeersWired/PeersUnwired event lineage + session-bridge assignment updates`; trigger: `spawn finalization, disposal retirement, wire/unwire mutation, resume replay`; stale: `forbidden` |
| `meerkat-mob/src/runtime/actor.rs` | `MobActor.pending_spawns` | `derived-projection` | `closed` | `PendingSpawnLineage + MobOrchestratorAuthority.pending_spawn_count` | src: `staged spawn receipts + reply obligations + provision task handles`; trigger: `enqueue_spawn, spawn completion, respawn cancellation, lifecycle drain`; stale: `forbidden` |
| `meerkat-mob/src/runtime/pending_spawn_lineage.rs` | `PendingSpawnLineage.tasks` | `capability-index` | `closed` | `PendingSpawnLineage metadata + MobOrchestratorAuthority.pending_spawn_count` | src: `machine-owned pending spawn set`; trigger: `spawn begin/complete/rollback transitions`; stale: `forbidden` |
| `meerkat-mob/src/runtime/provisioner.rs` | `SessionBackend.runtime_sessions` | `capability-index` | `closed` | `RuntimeSessionAdapter registered sessions` | src: `runtime adapter registration truth + runtime bridge sidecar handles`; trigger: `runtime session ensure/reattach + retire/unregister + interrupt stale-bridge cleanup`; stale: `forbidden` |
| `meerkat-mob/src/runtime/provisioner.rs` | `RuntimeSessionState.queued_turns` | `transport-buffer` | `closed` | `InputLifecycle canonical input identity + runtime primitive contributing ids` | src: `event transport handles keyed by canonical input ids`; trigger: `accept/dedup rekey + primitive contributing-id consumption + retire/unregister clear`; stale: `forbidden` |
| `meerkat-mob/src/runtime/ops_adapter.rs` | `MobOpsAdapter.fallback_registry` | `capability-handle` | `closed` | `RuntimeOpsLifecycleRegistry` | - |

## Mob Semantic Operations

| Path | Symbol | Boundary | Status | Anchor |
| --- | --- | --- | --- | --- |
| `meerkat-mob/src/runtime/handle.rs` | `spawn` | `public-inherent` | `closed` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_with_backend` | `public-inherent` | `closed` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_with_options` | `public-inherent` | `closed` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `attach_existing_session` | `public-inherent` | `closed` | `PendingSpawnLineage + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `attach_existing_session_as_orchestrator` | `public-inherent` | `closed` | `PendingSpawnLineage + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `attach_existing_session_as_member` | `public-inherent` | `closed` | `PendingSpawnLineage + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_spec` | `public-inherent` | `closed` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_many` | `public-inherent` | `closed` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `retire` | `public-inherent` | `closed` | `RosterAuthority + disposal pipeline` |
| `meerkat-mob/src/runtime/handle.rs` | `respawn` | `public-inherent` | `closed` | `respawn helper contract + PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `retire_all` | `public-inherent` | `closed` | `PendingSpawnLineage + RosterAuthority + disposal pipeline` |
| `meerkat-mob/src/runtime/handle.rs` | `wire` | `public-inherent` | `closed` | `RosterAuthority wiring projection contract + trust-edge mutation + edge-lock discipline` |
| `meerkat-mob/src/runtime/handle.rs` | `unwire` | `public-inherent` | `closed` | `RosterAuthority wiring projection contract + trust-edge mutation + edge-lock discipline` |
| `meerkat-mob/src/runtime/handle.rs` | `internal_turn` | `public-inherent` | `closed` | `SessionBackend runtime bridge + InputLifecycle truth` |
| `meerkat-mob/src/runtime/handle.rs` | `run_flow` | `public-inherent` | `closed` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `run_flow_with_stream` | `public-inherent` | `closed` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `cancel_flow` | `public-inherent` | `closed` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `stop` | `public-inherent` | `closed` | `MobLifecycleAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `resume` | `public-inherent` | `closed` | `MobLifecycleAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `complete` | `public-inherent` | `closed` | `MobLifecycleAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `reset` | `public-inherent` | `closed` | `MobLifecycleAuthority + RosterAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `destroy` | `public-inherent` | `closed` | `MobLifecycleAuthority + RosterAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `task_create` | `public-inherent` | `closed` | `MobTaskBoardService event + projection contract` |
| `meerkat-mob/src/runtime/handle.rs` | `task_update` | `public-inherent` | `closed` | `MobTaskBoardService event + projection contract` |
| `meerkat-mob/src/runtime/handle.rs` | `set_spawn_policy` | `public-inherent` | `closed` | `MobSpawnPolicySurface` |
| `meerkat-mob/src/runtime/handle.rs` | `shutdown` | `public-inherent` | `closed` | `MobLifecycleAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `force_cancel_member` | `public-inherent` | `closed` | `SessionBackend runtime bridge + InputLifecycle truth` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_helper` | `public-inherent` | `closed` | `MobMemberTerminalClassifier` |
| `meerkat-mob/src/runtime/handle.rs` | `fork_helper` | `public-inherent` | `closed` | `MobMemberTerminalClassifier` |
| `meerkat-mob/src/runtime/handle.rs` | `wait_one` | `public-inherent` | `closed` | `MobMemberTerminalClassifier` |
| `meerkat-mob/src/runtime/handle.rs` | `wait_all` | `public-inherent` | `closed` | `MobMemberTerminalClassifier` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_spawn_provisioned_batch` | `manual-callback` | `closed` | `PendingSpawnLineage + RosterAuthority + PendingProvision rollback contract` |
| `meerkat-mob/src/runtime/actor.rs` | `enqueue_spawn` | `enum-dispatch` | `closed` | `PendingSpawnLineage + MobOrchestratorAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_force_cancel` | `enum-dispatch` | `closed` | `MobLifecycleAuthority active-member gate + SessionBackend::interrupt_member runtime-adapter ownership contract + InputLifecycle cancellation semantics` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_retire` | `enum-dispatch` | `closed` | `RosterAuthority + disposal pipeline + SessionBackend retire contract` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_respawn` | `enum-dispatch` | `closed` | `respawn helper contract + PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_wire` | `enum-dispatch` | `closed` | `Roster wiring projection contract + trust-edge mutation + edge-lock discipline` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_unwire` | `enum-dispatch` | `closed` | `Roster wiring projection contract + trust-edge mutation + edge-lock discipline` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_external_turn` | `enum-dispatch` | `closed` | `RosterAuthority + SessionBackend runtime bridge + spawn_from_policy_inline contract` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_internal_turn` | `enum-dispatch` | `closed` | `RosterAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/actor.rs` | `retire_all_members` | `enum-dispatch` | `closed` | `PendingSpawnLineage + RosterAuthority + disposal pipeline` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_task_create` | `enum-dispatch` | `closed` | `MobTaskBoardService event + projection contract` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_task_update` | `enum-dispatch` | `closed` | `MobTaskBoardService event + projection contract` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_run_flow` | `enum-dispatch` | `closed` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_cancel_flow` | `enum-dispatch` | `closed` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_flow_cleanup` | `enum-dispatch` | `closed` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_complete` | `enum-dispatch` | `closed` | `MobLifecycleAuthority + retire_all_members + PendingSpawnLineage` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_destroy` | `enum-dispatch` | `closed` | `MobLifecycleAuthority + retire_all_members + PendingSpawnLineage` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_reset` | `enum-dispatch` | `closed` | `MobLifecycleAuthority + retire_all_members + PendingSpawnLineage` |

## Mob Coupling Invariants

| Name | Stores | Status | Anchor |
| --- | --- | --- | --- |
| `mob_pending_spawn_alignment` | `MobActor.pending_spawns`, `PendingSpawnLineage.tasks`, `MobOrchestratorAuthority.pending_spawn_count` | `closed` | `pending spawn lineage helpers + MobOrchestratorAuthority.pending_spawn_count` |
| `mob_runtime_bridge_alignment` | `SessionBackend.runtime_sessions`, `RuntimeSessionState.queued_turns`, `MobOpsAdapter.fallback_registry` | `closed` | `SessionBackend runtime session sidecar contract + RuntimeSessionAdapter registration truth + RuntimeOpsLifecycleRegistry` |
| `mob_wiring_alignment` | `Roster.wired_to`, `trust edges`, `edge locks` | `closed` | `Roster wiring projection contract + do_wire/handle_unwire edge-lock discipline` |

## Open Findings

No ownership findings.

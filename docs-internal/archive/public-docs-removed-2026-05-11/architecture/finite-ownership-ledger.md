---
title: Finite Ownership Ledger
description: Generated inventory of Meerkat semantic ownership boundaries.
icon: diagram-project
---

# Finite Ownership Ledger

**Status**: Generated
**Source**: `xtask ownership-ledger`

This document is generated from the typed ownership registry in `xtask`.
It is the authoritative inventory of semantic state, semantic-operation boundaries, and keyed-store invariants for the current closure program.

## Summary

| Subsystem | State Cells | Semantic Operations | Coupling Invariants | Open State Cells | Open Operations | Open Invariants |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| runtime | 7 | 20 | 4 | 0 | 0 | 0 |
| mcp | 11 | 21 | 2 | 0 | 0 | 0 |
| mob | 6 | 37 | 3 | 0 | 0 | 0 |
| auth | 1 | 8 | 0 | 0 | 0 | 0 |

## Boundary Manifest

| Family | Kind | Path | Type / Trait | Methods |
| --- | --- | --- | --- | --- |
| runtime-control-plane | trait-impl | `meerkat-runtime/src/meerkat_machine/traits.rs` | `MeerkatMachine` / `RuntimeControlPlane` | `ingest`, `publish_event`, `retire`, `recycle`, `reset`, `recover`, `destroy` |
| auth-lease-registry | trait-impl | `meerkat-runtime/src/handles/auth_lease.rs` | `RuntimeAuthLeaseHandle` / `AuthLeaseHandle` | `acquire_lease`, `mark_expiring`, `begin_refresh`, `complete_refresh`, `refresh_failed`, `mark_reauth_required`, `release_lease`, `snapshot` |
| runtime-session-adapter | public-inherent | `meerkat-runtime/src/meerkat_machine/mod.rs` | `MeerkatMachine` | `register_session` |
| runtime-session-adapter | public-inherent | `meerkat-runtime/src/meerkat_machine/session_management.rs` | `MeerkatMachine` | `set_session_silent_intents`, `register_session_with_executor`, `ensure_session_with_executor`, `unregister_session` |
| runtime-session-adapter | public-inherent | `meerkat-runtime/src/user_interrupt.rs` | `MeerkatMachine` | `hard_cancel_current_run` |
| runtime-session-adapter | public-inherent | `meerkat-runtime/src/meerkat_machine/runtime_control.rs` | `MeerkatMachine` | `stop_runtime_executor`, `accept_input_with_completion` |
| runtime-session-adapter | public-inherent | `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `MeerkatMachine` | `update_peer_ingress_context`, `abort_comms_drains`, `abort_comms_drain`, `wait_comms_drain` |
| mcp-router | public-inherent | `meerkat-mcp/src/router.rs` | `McpRouter` | `set_removal_timeout`, `add_server`, `stage_add`, `stage_remove`, `stage_reload`, `apply_staged`, `take_lifecycle_actions`, `take_external_updates`, `progress_removals`, `call_tool`, `shutdown` |
| mcp-router-adapter | public-inherent | `meerkat-mcp/src/adapter.rs` | `McpRouterAdapter` | `refresh_tools`, `stage_add`, `stage_remove`, `stage_reload`, `apply_staged`, `poll_lifecycle_actions`, `progress_removals`, `wait_until_ready`, `shutdown` |
| mob-handle | public-inherent | `meerkat-mob/src/runtime/handle.rs` | `MobHandle` | `spawn_spec`, `spawn_many`, `retire`, `respawn`, `retire_all`, `wire`, `unwire`, `run_flow`, `run_flow_with_stream`, `cancel_flow`, `stop`, `resume`, `complete`, `reset`, `destroy`, `set_spawn_policy`, `shutdown`, `force_cancel_member`, `wait_one`, `wait_all`, `spawn_helper`, `fork_helper` |
| mob-member-handle | public-inherent | `meerkat-mob/src/runtime/handle.rs` | `MemberHandle` | `internal_turn` |
| mob-command-dispatch | enum-dispatch | `meerkat-mob/src/runtime/actor.rs` | `MobActor` / `MobCommand` | `enqueue_spawn`, `handle_force_cancel`, `handle_retire`, `handle_respawn`, `handle_submit_work`, `handle_cancel_all_work`, `handle_rotate_supervisor`, `handle_run_flow`, `handle_cancel_flow`, `handle_flow_cleanup`, `handle_complete`, `handle_destroy`, `handle_reset` |
| manual-callback | manual-callback | `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `MeerkatMachine` | `notify_comms_drain_exited` |
| manual-callback | manual-callback | `meerkat-mcp/src/router.rs` | `McpRouter` | `process_pending_result` |
| manual-callback | manual-callback | `meerkat-mob/src/runtime/actor.rs` | `MobActor` | `handle_spawn_provisioned_batch` |
| shell-background-job-view | export-contract (app-facing) | `meerkat-tools/src/builtin/shell/types.rs` | `BackgroundJob` | `exports_raw_operation_id=false` |
| shell-job-summary-view | export-contract (app-facing) | `meerkat-tools/src/builtin/shell/types.rs` | `JobSummary` | `exports_raw_operation_id=false` |
| mob-member-ref | export-contract (app-facing) | `meerkat-mob/src/event.rs` | `MemberRef` | `exports_raw_operation_id=false` |
| mob-infra-member-spawn-receipt | export-contract (infra-canonical-op) | `meerkat-mob/src/runtime/handle.rs` | `MemberSpawnReceipt` | `exports_raw_operation_id=true` |

## Runtime State Cells

| Path | Symbol | Class | Status | Anchor | Contract |
| --- | --- | --- | --- | --- | --- |
| `meerkat-runtime/src/meerkat_machine/mod.rs` | `MeerkatMachine.sessions` | `capability-index` | `closed` | `MeerkatMachine registered-session + attachment publication contract` | src: `registered session entries with recovered driver/completion capabilities`; trigger: `register/ensure/attach/detach/unregister/destroy transitions + dead-attachment normalization`; stale: `forbidden` |
| `meerkat-runtime/src/meerkat_machine/mod.rs` | `RuntimeSessionEntry.drain_slot` | `capability-index` | `closed` | `MeerkatMachine registered-session contract + drain-control region` | src: `per-session comms drain lifecycle slot co-owned by the registered-session entry`; trigger: `register/unregister/destroy + drain lifecycle transitions + control installation`; stale: `forbidden` |
| `meerkat-runtime/src/meerkat_machine/mod.rs` | `RuntimeSessionEntry.driver` | `capability-handle` | `closed` | `MeerkatMachine control + admission + input-lifecycle regions` | - |
| `meerkat-runtime/src/meerkat_machine/mod.rs` | `RuntimeSessionEntry.attachment_slot` | `capability-handle` | `closed` | `MeerkatMachine attachment publication contract` | - |
| `meerkat-runtime/src/meerkat_machine/mod.rs` | `RuntimeSessionEntry.completions` | `capability-handle` | `closed` | `InputLifecycle terminal wait plumbing` | - |
| `meerkat-runtime/src/driver/ephemeral.rs` | `EphemeralRuntimeDriver.queue` | `derived-projection` | `closed` | `MeerkatMachine admission queue lane` | src: `MeerkatMachine admission queue entries`; trigger: `any ingress queue mutation or rollback/recovery rebuild`; stale: `forbidden` |
| `meerkat-runtime/src/driver/ephemeral.rs` | `EphemeralRuntimeDriver.steer_queue` | `derived-projection` | `closed` | `MeerkatMachine admission steer lane` | src: `MeerkatMachine admission steer entries`; trigger: `any ingress steer mutation or rollback/recovery rebuild`; stale: `forbidden` |

## Runtime Semantic Operations

| Path | Symbol | Boundary | Status | Writes | Anchor |
| --- | --- | --- | --- | --- | --- |
| `meerkat-runtime/src/meerkat_machine/mod.rs` | `register_session` | `public-inherent` | `closed` | `sessions`, `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.ops_lifecycle`, `RuntimeSessionEntry.completions` | `MeerkatMachine registration + recovery publication contract` |
| `meerkat-runtime/src/meerkat_machine/session_management.rs` | `set_session_silent_intents` | `public-inherent` | `closed` | `RuntimeSessionEntry.driver` | `MeerkatMachine admission/control policy truth` |
| `meerkat-runtime/src/meerkat_machine/session_management.rs` | `register_session_with_executor` | `public-inherent` | `closed` | `sessions`, `RuntimeSessionEntry.attachment_slot`, `RuntimeSessionEntry.driver` | `MeerkatMachine registration + attachment publication contract` |
| `meerkat-runtime/src/meerkat_machine/session_management.rs` | `ensure_session_with_executor` | `public-inherent` | `closed` | `sessions`, `RuntimeSessionEntry.attachment_slot`, `RuntimeSessionEntry.driver` | `MeerkatMachine attachment publication contract + RuntimeControl transitions` |
| `meerkat-runtime/src/meerkat_machine/session_management.rs` | `unregister_session` | `public-inherent` | `closed` | `sessions`, `RuntimeSessionEntry.drain_slot` | `registered-session contract + MeerkatMachine drain-control region` |
| `meerkat-runtime/src/user_interrupt.rs` | `hard_cancel_current_run` | `public-inherent` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine control region + runtime attachment publication contract` |
| `meerkat-runtime/src/meerkat_machine/runtime_control.rs` | `stop_runtime_executor` | `public-inherent` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.completions`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine control region + runtime attachment publication contract` |
| `meerkat-runtime/src/meerkat_machine/runtime_control.rs` | `accept_input_with_completion` | `public-inherent` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.completions`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine admission + input-lifecycle regions` |
| `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `update_peer_ingress_context` | `public-inherent` | `closed` | `RuntimeSessionEntry.drain_slot` | `MeerkatMachine drain-control region` |
| `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `notify_comms_drain_exited` | `manual-callback` | `closed` | `RuntimeSessionEntry.drain_slot` | `MeerkatMachine drain-control region` |
| `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `abort_comms_drains` | `public-inherent` | `closed` | `RuntimeSessionEntry.drain_slot` | `MeerkatMachine drain-control region` |
| `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `abort_comms_drain` | `public-inherent` | `closed` | `RuntimeSessionEntry.drain_slot` | `MeerkatMachine drain-control region` |
| `meerkat-runtime/src/meerkat_machine/comms_drain.rs` | `wait_comms_drain` | `public-inherent` | `closed` | `RuntimeSessionEntry.drain_slot` | `MeerkatMachine drain-control region` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `publish_event` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `InputLedger.states` | `MeerkatMachine control + input-lifecycle regions` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `retire` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.completions`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine control region` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `recycle` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.completions`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine control region` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `reset` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.completions` | `MeerkatMachine control region` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `recover` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine control region` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `destroy` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `RuntimeSessionEntry.completions`, `RuntimeSessionEntry.attachment_slot` | `MeerkatMachine control region` |
| `meerkat-runtime/src/meerkat_machine/traits.rs` | `ingest` | `trait-impl` | `closed` | `RuntimeSessionEntry.driver`, `EphemeralRuntimeDriver.queue`, `EphemeralRuntimeDriver.steer_queue` | `MeerkatMachine admission + input-lifecycle regions` |

## Runtime Coupling Invariants

| Name | Stores | Status | Anchor |
| --- | --- | --- | --- |
| `runtime_attachment_alignment` | `RuntimeSessionEntry.attachment_slot`, `RuntimeSessionEntry.driver` | `closed` | `MeerkatMachine attachment publication contract + RuntimeControl transitions` |
| `runtime_queue_projection_alignment` | `MeerkatMachine.admission.queue`, `EphemeralRuntimeDriver.queue`, `EphemeralRuntimeDriver.steer_queue` | `closed` | `MeerkatMachine admission region` |
| `runtime_comms_bridge_projection_alignment` | `MeerkatMachine.peer_ingress.classified_interactions`, `RuntimeCommsBridge.runtime_input_projection` | `closed` | `MeerkatMachine peer-ingress classification + RuntimeCommsBridge projection contract` |
| `runtime_external_event_projection_alignment` | `CLI.stdin_external_event_projection`, `MeerkatMachine.peer_ingress.plain_events`, `Runtime.ExternalEventInput`, `RuntimeLoop.external_event_rendering` | `closed` | `ExternalEventInput projection contract + runtime external-event render contract` |

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

| Path | Symbol | Boundary | Status | Writes | Anchor |
| --- | --- | --- | --- | --- | --- |
| `meerkat-mcp/src/router.rs` | `set_removal_timeout` | `public-inherent` | `closed` | `surface_owner` | `ExternalToolSurfaceAuthority` |
| `meerkat-mcp/src/router.rs` | `add_server` | `public-inherent` | `closed` | `servers`, `projection`, `pending_obligations` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `stage_add` | `public-inherent` | `closed` | `staged_payloads`, `surface_owner` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/router.rs` | `stage_remove` | `public-inherent` | `closed` | `staged_payloads`, `surface_owner` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/router.rs` | `stage_reload` | `public-inherent` | `closed` | `staged_payloads`, `surface_owner` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/router.rs` | `apply_staged` | `public-inherent` | `closed` | `staged_payloads`, `pending_obligations`, `servers`, `projection` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `process_pending_result` | `manual-callback` | `closed` | `pending_obligations`, `servers`, `projection`, `completed_updates` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `take_lifecycle_actions` | `public-inherent` | `closed` | `completed_updates` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `take_external_updates` | `public-inherent` | `closed` | `completed_updates`, `projection`, `servers` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `progress_removals` | `public-inherent` | `closed` | `servers`, `projection` | `ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/router.rs` | `call_tool` | `public-inherent` | `closed` | `servers`, `projection`, `ServerEntry.active_calls` | `ExternalToolSurfaceAuthority` |
| `meerkat-mcp/src/router.rs` | `shutdown` | `public-inherent` | `closed` | `servers`, `pending_tx`, `pending_obligations`, `completed_updates` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `refresh_tools` | `public-inherent` | `closed` | `router`, `has_pending` | `RouterProjectionSnapshot` |
| `meerkat-mcp/src/adapter.rs` | `stage_add` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/adapter.rs` | `stage_remove` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/adapter.rs` | `stage_reload` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority staged intent sequence` |
| `meerkat-mcp/src/adapter.rs` | `apply_staged` | `public-inherent` | `closed` | `has_pending` | `RouterProjectionSnapshot` |
| `meerkat-mcp/src/adapter.rs` | `poll_lifecycle_actions` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `progress_removals` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `wait_until_ready` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |
| `meerkat-mcp/src/adapter.rs` | `shutdown` | `public-inherent` | `closed` | `router`, `has_pending` | `ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract` |

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
| `meerkat-mob/src/runtime/provisioner.rs` | `SessionBackend.runtime_sessions` | `capability-index` | `closed` | `MeerkatMachine registered sessions` | src: `runtime adapter registration truth + runtime bridge sidecar handles`; trigger: `runtime session ensure/reattach + retire/unregister + interrupt stale-bridge cleanup`; stale: `forbidden` |
| `meerkat-mob/src/runtime/provisioner.rs` | `RuntimeSessionState.queued_turns` | `transport-buffer` | `closed` | `InputLifecycle canonical input identity + runtime primitive contributing ids` | src: `event transport handles keyed by canonical input ids`; trigger: `accept/dedup rekey + primitive contributing-id consumption + retire/unregister clear`; stale: `forbidden` |
| `meerkat-mob/src/runtime/ops_adapter.rs` | `MobOpsAdapter.member_bindings` | `capability-handle` | `closed` | `RuntimeOpsLifecycleRegistry` | - |

## Mob Semantic Operations

| Path | Symbol | Boundary | Status | Writes | Anchor |
| --- | --- | --- | --- | --- | --- |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_spec` | `public-inherent` | `closed` | `MobActor.pending_spawns` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_many` | `public-inherent` | `closed` | `MobActor.pending_spawns` | `PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `retire` | `public-inherent` | `closed` | `roster`, `MobActor.retired_event_index` | `RosterAuthority + disposal pipeline` |
| `meerkat-mob/src/runtime/handle.rs` | `respawn` | `public-inherent` | `closed` | `roster`, `MobActor.pending_spawns`, `MobActor.dsl_authority` | `respawn helper contract + PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `retire_all` | `public-inherent` | `closed` | `roster`, `MobActor.pending_spawns` | `PendingSpawnLineage + RosterAuthority + disposal pipeline` |
| `meerkat-mob/src/runtime/handle.rs` | `wire` | `public-inherent` | `closed` | `roster`, `MobActor.dsl_authority`, `MobActor.edge_locks` | `RosterAuthority wiring projection contract + trust-edge mutation + edge-lock discipline` |
| `meerkat-mob/src/runtime/handle.rs` | `unwire` | `public-inherent` | `closed` | `roster`, `MobActor.dsl_authority`, `MobActor.edge_locks` | `RosterAuthority wiring projection contract + trust-edge mutation + edge-lock discipline` |
| `meerkat-mob/src/runtime/handle.rs` | `internal_turn` | `public-inherent` | `closed` | `MobActor.runtime_adapter`, `mob` | `SessionBackend runtime bridge + InputLifecycle truth` |
| `meerkat-mob/src/runtime/handle.rs` | `run_flow` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `flow_streams`, `run_store` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `run_flow_with_stream` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `flow_streams`, `run_store` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `cancel_flow` | `public-inherent` | `closed` | `MobActor.run_cancel_tokens`, `MobActor.dsl_authority`, `run_store` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `stop` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `MobActor.pending_spawns`, `MobActor.runtime_adapter` | `MobLifecycleAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `resume` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `MobActor.runtime_adapter` | `MobLifecycleAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `complete` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `roster`, `MobActor.runtime_adapter` | `MobLifecycleAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `reset` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `roster`, `MobActor.runtime_adapter`, `MobActor.pending_spawns` | `MobLifecycleAuthority + RosterAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `destroy` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `roster`, `MobActor.runtime_adapter`, `MobActor.pending_spawns` | `MobLifecycleAuthority + RosterAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `set_spawn_policy` | `public-inherent` | `closed` | `MobActor.spawn_policy` | `MobSpawnPolicySurface` |
| `meerkat-mob/src/runtime/handle.rs` | `shutdown` | `public-inherent` | `closed` | `MobActor.dsl_authority`, `MobActor.runtime_adapter`, `MobActor.pending_spawns`, `run_store` | `MobLifecycleAuthority + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/handle.rs` | `force_cancel_member` | `public-inherent` | `closed` | `roster`, `MobActor.runtime_adapter` | `SessionBackend runtime bridge + InputLifecycle truth` |
| `meerkat-mob/src/runtime/handle.rs` | `spawn_helper` | `public-inherent` | `closed` | `roster`, `MobActor.pending_spawns` | `MobMemberLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `fork_helper` | `public-inherent` | `closed` | `roster`, `MobActor.pending_spawns` | `MobMemberLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `wait_one` | `public-inherent` | `closed` | `roster` | `MobMemberLifecycleAuthority` |
| `meerkat-mob/src/runtime/handle.rs` | `wait_all` | `public-inherent` | `closed` | `roster` | `MobMemberLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_spawn_provisioned_batch` | `manual-callback` | `closed` | `pending_spawns`, `roster` | `PendingSpawnLineage + RosterAuthority + PendingProvision rollback contract` |
| `meerkat-mob/src/runtime/actor.rs` | `enqueue_spawn` | `enum-dispatch` | `closed` | `pending_spawns`, `roster` | `PendingSpawnLineage + MobOrchestratorAuthority + RosterAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_force_cancel` | `enum-dispatch` | `closed` | `runtime_adapter`, `roster` | `MobLifecycleAuthority active-member gate + SessionBackend::interrupt_member runtime-adapter ownership contract + InputLifecycle cancellation semantics` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_retire` | `enum-dispatch` | `closed` | `roster`, `dsl_authority`, `runtime_adapter` | `RosterAuthority + disposal pipeline + SessionBackend retire contract` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_respawn` | `enum-dispatch` | `closed` | `roster`, `pending_spawns` | `respawn helper contract + PendingSpawnLineage + RosterAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_submit_work` | `enum-dispatch` | `closed` | `runtime_adapter`, `pending_spawns`, `roster` | `MobMachine DSL work-origin legality + RosterAuthority + SessionBackend runtime bridge + spawn_from_policy_inline contract` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_cancel_all_work` | `enum-dispatch` | `closed` | `runtime_adapter`, `roster` | `MobMachine DSL CancelAllWork legality + SessionBackend runtime bridge` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_rotate_supervisor` | `enum-dispatch` | `closed` | `roster`, `runtime_adapter` | `Supervisor-bridge rotation protocol + fail-closed incomplete rotation on partial remote failure` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_run_flow` | `enum-dispatch` | `closed` | `dsl_authority`, `run_tasks`, `run_cancel_tokens`, `run_store` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_cancel_flow` | `enum-dispatch` | `closed` | `run_tasks`, `run_cancel_tokens`, `run_store` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_flow_cleanup` | `enum-dispatch` | `closed` | `run_tasks`, `run_cancel_tokens`, `flow_streams` | `MobOrchestratorAuthority + MobLifecycleAuthority` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_complete` | `enum-dispatch` | `closed` | `dsl_authority`, `roster`, `runtime_adapter` | `MobLifecycleAuthority + retire_all_members + PendingSpawnLineage` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_destroy` | `enum-dispatch` | `closed` | `dsl_authority`, `roster`, `runtime_adapter`, `pending_spawns` | `MobLifecycleAuthority + retire_all_members + PendingSpawnLineage` |
| `meerkat-mob/src/runtime/actor.rs` | `handle_reset` | `enum-dispatch` | `closed` | `dsl_authority`, `roster`, `runtime_adapter`, `pending_spawns` | `MobLifecycleAuthority + retire_all_members + PendingSpawnLineage` |

## Mob Coupling Invariants

| Name | Stores | Status | Anchor |
| --- | --- | --- | --- |
| `mob_pending_spawn_alignment` | `MobActor.pending_spawns`, `PendingSpawnLineage.tasks`, `MobOrchestratorAuthority.pending_spawn_count` | `closed` | `pending spawn lineage helpers + MobOrchestratorAuthority.pending_spawn_count` |
| `mob_runtime_bridge_alignment` | `SessionBackend.runtime_sessions`, `RuntimeSessionState.queued_turns`, `MobOpsAdapter.member_bindings` | `closed` | `SessionBackend runtime session sidecar contract + MeerkatMachine registration truth + RuntimeOpsLifecycleRegistry` |
| `mob_wiring_alignment` | `Roster.wired_to`, `trust edges`, `edge locks` | `closed` | `Roster wiring projection contract + do_wire/handle_unwire edge-lock discipline` |

## Auth State Cells

| Path | Symbol | Class | Status | Anchor | Contract |
| --- | --- | --- | --- | --- | --- |
| `meerkat-runtime/src/handles/auth_lease.rs` | `RuntimeAuthLeaseHandle.machines` | `capability-index` | `closed` | `per-binding AuthMachine kernel state` | src: `per-binding AuthMachineAuthority wrappers keyed by binding_key`; trigger: `acquire/expire/refresh/complete/fail/reauth/release DSL transitions + release removals`; stale: `forbidden` |

## Auth Semantic Operations

| Path | Symbol | Boundary | Status | Writes | Anchor |
| --- | --- | --- | --- | --- | --- |
| `meerkat-runtime/src/handles/auth_lease.rs` | `acquire_lease` | `trait-impl` | `closed` | `machines` | `AuthMachine Acquire transition — per-binding lease lifecycle` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `mark_expiring` | `trait-impl` | `closed` | `machines` | `AuthMachine MarkExpiring transition` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `begin_refresh` | `trait-impl` | `closed` | `machines` | `AuthMachine BeginRefresh transition — refresh dedup` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `complete_refresh` | `trait-impl` | `closed` | `machines` | `AuthMachine CompleteRefresh transition` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `refresh_failed` | `trait-impl` | `closed` | `machines` | `AuthMachine RefreshFailedTransient / RefreshFailedPermanent transitions` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `mark_reauth_required` | `trait-impl` | `closed` | `machines` | `AuthMachine MarkReauthRequired transition` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `release_lease` | `trait-impl` | `closed` | `machines` | `AuthMachine Release transition — terminal` |
| `meerkat-runtime/src/handles/auth_lease.rs` | `snapshot` | `trait-impl` | `closed` | `machines` | `AuthMachine observable snapshot — read boundary` |

## Auth Coupling Invariants

| Name | Stores | Status | Anchor |
| --- | --- | --- | --- |

## Open Findings

No ownership findings.

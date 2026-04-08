# mob_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `mob_lifecycle`: `MobLifecycleMachine` @ actor `mob_lifecycle_actor`
- `mob_orchestrator`: `MobOrchestratorMachine` @ actor `mob_orchestrator_actor`
- `mob_member_lifecycle`: `MobMemberLifecycleMachine` @ actor `mob_member_lifecycle_actor`
- `mob_runtime_bridge`: `MobRuntimeBridgeMachine` @ actor `mob_runtime_bridge_actor`
- `mob_wiring`: `MobWiringMachine` @ actor `mob_wiring_actor`
- `mob_member_bootstrap`: `MobMemberBootstrapMachine` @ actor `mob_member_bootstrap_actor`
- `flow_run`: `FlowRunMachine` @ actor `flow_engine`
- `ops_lifecycle`: `OpsLifecycleMachine` @ actor `ops_plane`
- `peer_comms`: `PeerCommsMachine` @ actor `peer_plane`
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `mob_supervisor_activation_starts_lifecycle`: `mob_orchestrator`.`ActivateSupervisor` -> `mob_lifecycle`.`Start` [Immediate]
- `flow_step_dispatch_enters_runtime_admission`: `flow_run`.`AdmitStepWork` -> `runtime_control`.`SubmitWork` [Immediate]
- `mob_flow_activation_starts_flow_run`: `mob_orchestrator`.`FlowActivated` -> `flow_run`.`StartRun` [Immediate]
- `mob_flow_activation_marks_lifecycle_run`: `mob_orchestrator`.`FlowActivated` -> `mob_lifecycle`.`StartRun` [Immediate]
- `mob_async_op_event_enters_runtime_admission`: `ops_lifecycle`.`SubmitOpEvent` -> `runtime_control`.`SubmitWork` [Immediate]
- `mob_peer_candidate_enters_runtime_admission`: `peer_comms`.`SubmitPeerInputCandidate` -> `runtime_control`.`SubmitWork` [Immediate]
- `mob_peer_candidate_tracks_wiring`: `peer_comms`.`SubmitPeerInputCandidate` -> `mob_wiring`.`PeerInputAdmitted` [Immediate]
- `mob_admitted_work_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]
- `mob_runtime_admission_tracks_wiring`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `mob_wiring`.`RuntimeWorkAdmitted` [Immediate]
- `mob_ingress_ready_starts_runtime_control`: `runtime_ingress`.`ReadyForRun` -> `runtime_control`.`BeginRun` [Immediate]
- `mob_runtime_control_starts_execution`: `runtime_control`.`SubmitRunPrimitive` -> `turn_execution`.`StartConversationRun` [Immediate]
- `mob_runtime_start_tracks_bridge`: `runtime_control`.`SubmitRunPrimitive` -> `mob_runtime_bridge`.`RuntimeRunSubmitted` [Immediate]
- `mob_flow_terminalization_completes_orchestrator`: `flow_run`.`FlowTerminalized` -> `mob_orchestrator`.`CompleteFlow` [Immediate]
- `mob_flow_deactivation_finishes_lifecycle_run`: `mob_orchestrator`.`FlowDeactivated` -> `mob_lifecycle`.`FinishRun` [Immediate]
- `mob_execution_boundary_updates_ingress`: `turn_execution`.`BoundaryApplied` -> `runtime_ingress`.`BoundaryApplied` [Immediate]
- `mob_execution_completion_updates_ingress`: `turn_execution`.`RunCompleted` -> `runtime_ingress`.`RunCompleted` [Immediate]
- `mob_execution_completion_notifies_control`: `turn_execution`.`RunCompleted` -> `runtime_control`.`RunCompleted` [Immediate]
- `mob_execution_completion_tracks_bridge`: `turn_execution`.`RunCompleted` -> `mob_runtime_bridge`.`RuntimeRunCompleted` [Immediate]
- `mob_execution_completion_updates_bootstrap`: `turn_execution`.`RunCompleted` -> `mob_member_bootstrap`.`KickoffStarted` [Immediate]
- `mob_execution_failure_updates_ingress`: `turn_execution`.`RunFailed` -> `runtime_ingress`.`RunFailed` [Immediate]
- `mob_execution_failure_notifies_control`: `turn_execution`.`RunFailed` -> `runtime_control`.`RunFailed` [Immediate]
- `mob_execution_failure_tracks_bridge`: `turn_execution`.`RunFailed` -> `mob_runtime_bridge`.`RuntimeRunFailed` [Immediate]
- `mob_execution_failure_updates_bootstrap`: `turn_execution`.`RunFailed` -> `mob_member_bootstrap`.`KickoffFailed` [Immediate]
- `mob_execution_cancel_updates_ingress`: `turn_execution`.`RunCancelled` -> `runtime_ingress`.`RunCancelled` [Immediate]
- `mob_execution_cancel_notifies_control`: `turn_execution`.`RunCancelled` -> `runtime_control`.`RunCancelled` [Immediate]
- `mob_execution_cancel_tracks_bridge`: `turn_execution`.`RunCancelled` -> `mob_runtime_bridge`.`RuntimeRunCancelled` [Immediate]
- `mob_execution_cancel_updates_bootstrap`: `turn_execution`.`RunCancelled` -> `mob_member_bootstrap`.`KickoffCancelled` [Immediate]
- `mob_deactivate_supervisor_stops_lifecycle`: `mob_orchestrator`.`DeactivateSupervisor` -> `mob_lifecycle`.`Stop` [Immediate]
- `mob_member_force_cancelled_stops_runtime`: `mob_orchestrator`.`MemberForceCancelled` -> `runtime_control`.`StopRequested` [Immediate]
- `mob_member_force_cancel_tracks_bridge`: `mob_orchestrator`.`MemberForceCancelled` -> `mob_runtime_bridge`.`RuntimeStopRequested` [Immediate]
- `mob_member_force_cancel_updates_bootstrap`: `mob_orchestrator`.`MemberForceCancelled` -> `mob_member_bootstrap`.`KickoffForceCancelled` [Immediate]
- `mob_escalate_supervisor_stops_orchestrator`: `flow_run`.`EscalateSupervisor` -> `mob_orchestrator`.`StopOrchestrator` [Immediate]
- `mob_cleanup_destroys_orchestrator`: `mob_lifecycle`.`RequestCleanup` -> `mob_orchestrator`.`DestroyOrchestrator` [Immediate]
- `mob_ops_peer_ready_trusts_peer_comms`: `ops_lifecycle`.`ExposeOperationPeer` -> `peer_comms`.`TrustPeer` [Immediate]
- `mob_ops_peer_ready_tracks_member_lifecycle`: `ops_lifecycle`.`ExposeOperationPeer` -> `mob_member_lifecycle`.`MemberPeerExposed` [Immediate]
- `mob_ops_peer_ready_tracks_wiring`: `ops_lifecycle`.`ExposeOperationPeer` -> `mob_wiring`.`OperationPeerTrusted` [Immediate]
- `mob_ops_terminal_tracks_member_lifecycle`: `ops_lifecycle`.`NotifyOpWatcher` -> `mob_member_lifecycle`.`MemberTerminalized` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `(none)`

## Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`

## Structural Requirements
- `control_preempts_mob_ingress` — runtime control outranks mob-side ingress work when both are ready

## Behavioral Invariants
- `mob_supervisor_activation_starts_lifecycle` — mob-orchestrator supervisor activation starts the lifecycle substrate through an explicit route
- `mob_flow_activation_starts_flow_run` — mob-orchestrator flow activation starts the flow-run machine through an explicit route
- `mob_flow_activation_marks_lifecycle_run` — mob-orchestrator flow activation increments lifecycle run ownership through an explicit route
- `flow_dispatch_uses_canonical_runtime_admission` — flow-run step dispatch reaches runtime only through the runtime-control admission surface
- `mob_async_lifecycle_events_use_operation_input` — mob-backed async lifecycle events re-enter runtime through the operation-input admission path
- `mob_peer_work_uses_canonical_runtime_admission` — member peer communication enters runtime only through canonical admission
- `mob_runtime_work_flows_into_ingress` — mob-originated admitted work is handed into canonical ingress ownership
- `mob_runtime_admission_updates_wiring` — mob runtime admission effects are recorded in the wiring owner
- `mob_runtime_start_updates_bridge` — mob run handoff into turn execution is recorded in the runtime bridge owner
- `mob_flow_terminalization_completes_orchestrator` — flow terminalization closes the orchestrator-side active flow through an explicit route
- `mob_flow_deactivation_finishes_lifecycle_run` — orchestrator flow deactivation closes the lifecycle run count through an explicit route
- `mob_execution_failure_is_handled` — mob turn-execution failure is handled by ingress/runtime control and recorded in runtime-bridge and member-bootstrap owner and runtime-bridge owner
- `mob_execution_cancel_is_handled` — mob turn-execution cancellation is handled by ingress/runtime control and recorded in runtime-bridge and member-bootstrap owner and runtime-bridge owner
- `mob_execution_completion_is_handled` — mob turn-execution completion is handled by ingress/runtime control and recorded in runtime-bridge and member-bootstrap owner and runtime-bridge owner
- `mob_ops_peer_ready_trusts_peer_comms` — trust handoff assumes the exposed operation peer is identified by the operation_id alias mapping
- `mob_member_lifecycle_terminal_events_are_observed` — mob member lifecycle terminal events are mirrored from ops lifecycle into the member lifecycle owner
- `mob_wiring_tracks_peer_candidates` — mob peer candidate admission is recorded in the wiring owner through an explicit route

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — mob orchestration precursor
- `meerkat-mob/src/runtime/handle.rs` — member-directed delivery capability
- `meerkat-mob/src/runtime/flow.rs` — flow dispatch precursor
- `meerkat-comms/src/runtime/comms_runtime.rs` — mob member peer communication precursor
- `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts` — mob-based WASM example coverage
- `examples/032-wasm-webcm-agent/web/src/main.ts` — browser mob workflow coverage
- `examples/033-the-office-demo-sh/web/src/main.ts` — browser local-tool + mob coverage

### Scenarios
- `mob-flow-dispatch` — flow step work enters runtime only through canonical admission
- `mob-child-report-back` — mob-backed child work reports progress and terminality through ops lifecycle
- `mob-peer-orchestration` — member-directed communication and orchestration stay inside the mob stack
- `wasm-mob-examples` — browser mob examples continue to fit the canonical mob/comms/runtime model

// Scoped clippy allow: the anchor constructors below parse hand-authored,
// compile-time-known machine/route slugs that name canonical schema elements.
// A parse failure here is a coverage-manifest authoring bug, never reachable
// from wire input. Inlining `parse(...)` error handling at every one of the
// ~46 anchor construction sites would drown the coverage manifest in
// boilerplate, so the typed-target constructors centralize it. The validator
// in `xtask` resolves every anchor target against the canonical schema and
// fails closed on any anchor that names a machine/route absent from the schema.
#![allow(clippy::expect_used)]

use crate::identity::{EffectVariantId, MachineId, RouteId, TransitionId};
use crate::{CompositionSchema, MachineSchema, SchedulerRule};

use super::{
    compositions::{
        adaptive_mob_bundle_composition, auth_lease_bundle_composition,
        meerkat_mob_seam_composition, schedule_bundle_composition, schedule_mob_bundle_composition,
        schedule_runtime_bundle_composition, workgraph_attention_bundle_composition,
    },
    dsl::{
        dsl_approval_lifecycle_machine, dsl_auth_machine, dsl_meerkat_machine, dsl_mob_machine,
        dsl_occurrence_lifecycle_machine, dsl_schedule_lifecycle_machine,
        dsl_session_document_machine, dsl_session_turn_admission_machine,
        dsl_work_attention_lifecycle_machine, dsl_workgraph_lifecycle_machine,
    },
};

/// A resolvable reference to the code location that realizes a slice of a
/// machine or composition's semantics.
///
/// Unlike kernel slugs, a symbol path names an on-disk Rust file/module and
/// may contain `/` and `.`; it is not a validated identity slug. It stays a
/// dedicated newtype rather than a bare `String` so the anchor cannot confuse
/// a code location with a schema target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRef(String);

impl SymbolRef {
    /// Borrow the underlying repo-relative path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The schema element an anchor claims to realize.
///
/// This is the dogma-load-bearing field: the target is a typed, schema-
/// resolvable reference rather than an inert string. The `xtask` coverage
/// validator resolves a [`CoverageSchemaTarget::Machine`] against the canonical
/// machine-id set and a [`CoverageSchemaTarget::Route`] against the owning
/// composition's declared routes, failing closed when the named element is
/// absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageSchemaTarget {
    /// Names a canonical machine (e.g. `MeerkatMachine`).
    Machine(MachineId),
    /// Names a declared composition route (e.g. `binding_request_reaches_meerkat`).
    Route(RouteId),
}

/// A typed coverage anchor binding a stable mapping id to a code location and
/// the schema element it realizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageAnchor {
    /// Stable mapping key referenced by [`SemanticCoverageEntry::anchor_ids`].
    pub id: String,
    /// The code location that realizes the targeted semantics.
    pub symbol: SymbolRef,
    /// The schema element (machine or route) this anchor claims to realize.
    pub target: CoverageSchemaTarget,
    /// Human-readable description of what the anchor covers. Documentation
    /// only — element attribution is the explicit typed [`Self::claims`]
    /// binding, never derived from this prose.
    pub note: String,
    /// Explicit typed binding from this anchor to the schema elements it
    /// realizes. The `xtask` coverage validator resolves every claim against
    /// the owning schema and fails closed on a claim that names a
    /// nonexistent element; elements no anchor claims are honestly reported
    /// UNCLAIMED (empty ids) rather than mis-attributed.
    pub claims: CoverageClaims,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioCoverage {
    pub id: String,
    pub summary: String,
    /// Explicit typed binding from this scenario to the schema elements it
    /// exercises — same contract as [`CoverageAnchor::claims`].
    pub claims: CoverageClaims,
}

/// Explicit typed anchor/scenario → schema-element binding.
///
/// This replaces the deleted token-containment matcher: an anchor or
/// scenario claims exactly the transitions/effects/invariants (machines) or
/// routes/scheduler-rules/invariants (compositions) listed here. Claims are
/// validated against the owning schema by the `xtask` coverage gate —
/// claiming a nonexistent element is a hard failure, and an element with no
/// claims stays honestly unclaimed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoverageClaims {
    pub transitions: Vec<TransitionId>,
    pub effects: Vec<EffectVariantId>,
    pub invariants: Vec<String>,
    pub routes: Vec<RouteId>,
    pub scheduler_rules: Vec<String>,
}

impl CoverageClaims {
    /// An empty claim set: the anchor/scenario documents a code location but
    /// attributes no schema elements.
    pub fn none() -> Self {
        Self::default()
    }

    /// Claim machine transitions by name.
    pub fn transitions(mut self, names: &[&str]) -> Self {
        self.transitions = names
            .iter()
            .map(|name| TransitionId::parse(*name).expect("valid transition slug"))
            .collect();
        self
    }

    /// Claim machine effect variants by name.
    pub fn effects(mut self, names: &[&str]) -> Self {
        self.effects = names
            .iter()
            .map(|name| EffectVariantId::parse(*name).expect("valid effect variant slug"))
            .collect();
        self
    }

    /// Claim machine or composition invariants by name.
    pub fn invariants(mut self, names: &[&str]) -> Self {
        self.invariants = names.iter().map(|name| (*name).to_owned()).collect();
        self
    }

    /// Claim composition routes by name.
    pub fn routes(mut self, names: &[&str]) -> Self {
        self.routes = names
            .iter()
            .map(|name| RouteId::parse(*name).expect("valid route slug"))
            .collect();
        self
    }

    /// Claim composition scheduler rules by rendered name
    /// (see [`scheduler_rule_coverage_name`]).
    pub fn scheduler_rules(mut self, names: &[&str]) -> Self {
        self.scheduler_rules = names.iter().map(|name| (*name).to_owned()).collect();
        self
    }

    fn claims_transition(&self, name: &str) -> bool {
        self.transitions.iter().any(|id| id.as_str() == name)
    }

    fn claims_effect(&self, name: &str) -> bool {
        self.effects.iter().any(|id| id.as_str() == name)
    }

    fn claims_invariant(&self, name: &str) -> bool {
        self.invariants.iter().any(|id| id == name)
    }

    fn claims_route(&self, name: &str) -> bool {
        self.routes.iter().any(|id| id.as_str() == name)
    }

    fn claims_scheduler_rule(&self, name: &str) -> bool {
        self.scheduler_rules.iter().any(|id| id == name)
    }
}

/// Canonical rendered name for a scheduler rule in coverage entries and
/// claims. Shared with the `xtask` coverage validator so the two sides can
/// never drift.
pub fn scheduler_rule_coverage_name(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady({higher}, {lower})")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCoverageEntry {
    pub name: String,
    pub anchor_ids: Vec<String>,
    pub scenario_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCoverageManifest {
    pub machine: crate::identity::MachineId,
    pub code_anchors: Vec<CoverageAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub transition_coverage: Vec<SemanticCoverageEntry>,
    pub effect_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionCoverageManifest {
    pub composition: crate::identity::CompositionId,
    pub code_anchors: Vec<CoverageAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub route_coverage: Vec<SemanticCoverageEntry>,
    pub scheduler_rule_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

pub fn canonical_machine_coverage_manifests() -> Vec<MachineCoverageManifest> {
    vec![
        machine_manifest_from_schema(
            &dsl_meerkat_machine(),
            &[
                machine_anchor(
                    "meerkat_machine",
                    "MeerkatMachine",
                    "meerkat-runtime/src/meerkat_machine/mod.rs",
                    "authoritative MeerkatMachine command dispatch and state ownership for initialize, recover initializing, register, unregister, deferred session stage, deferred session keep-alive update, deferred session promotion, deferred session archive, deferred session drop, mob operator access resolution/restoration/profile mutation/create scope/manage scope/spawn-profile scope, reconfigure, stage filters and tools, prepare bindings, drain, interrupt, cancel boundary, cancellation, abort, wait, ingest, publish event, accept input, recover input lifecycle, classify input terminality, classify envelope, append/context starts, run preparation, primitive applied conversation/immediate, enter extraction, extraction validation passed/failed retry/exhausted, recoverable/fatal failure, retry requested, budget exhausted, steer accepted, increment attempt count, rollback staged, consume on accept, commit, fail, pending/call/finalize tool surface, retire/retired, reset, stop/stopped executor, destroy/destroyed, ensure executor, runtime notice, silent intents, recycle, realtime binding, MCP server, peer ready operation, peer request, peer response, peer ingress, peer endpoint projection, interaction stream, product turn, live topology, ingress, supervisor, trust reconcile, ops barrier, local endpoint, admission, completion, completion consumer cursors, compaction, submit op event, progress reported op, terminate op, resolve op lifecycle transition rejected feedback, notify op watcher, recover op record, classify operation terminality, classify recovered operation record, recover ops completion cursor, recover/advance completion consumer cursors, evict completed op, collect completed op, collect/enqueue, terminal records, model routing status, set model routing baseline, finite switch turn, until changed switch turn, assistant turn admission, image operation begin activate complete restore, routing approval, routing denial, scoped override, sync visibility revisions, and persistent reconfigure",
                    CoverageClaims::none()
                        .transitions(&[
                            "Initialize",
                            "RegisterSessionRetired",
                            "RegisterSessionResumesStopped",
                            "RegisterSessionNewBindingFromStopped",
                            "StageDeferredSession",
                            "UpdateDeferredSessionKeepAlive",
                            "BeginDeferredSessionPromotion",
                            "BeginDeferredSessionArchive",
                            "RestoreDeferredSessionArchive",
                            "DropDeferredSessionStaged",
                            "SetMobOperatorProfileMutation",
                            "UnregisterSessionRetired",
                            "UnregisterSessionStopped",
                            "SetModelRoutingBaselineRetired",
                            "StagePersistentFilterRetired",
                            "RequestDeferredToolsRetired",
                            "PrepareBindingsInitializing",
                            "PrepareBindingsRetired",
                            "SetPeerIngressContextRetired",
                            "SetPeerIngressContextStopped",
                            "BoundaryAppliedPublish",
                            "Reset",
                            "StopRuntimeExecutorInitializing",
                            "StopRuntimeExecutorRetired",
                            "DestroyInitializing",
                            "Destroy",
                            "RecoverInitializing",
                            "RecoverRetired",
                            "RecoverStopped",
                            "SetSilentIntentsRetired",
                            "AbortRetired",
                            "AbortStopped",
                            "WaitRetired",
                            "WaitStopped",
                            "AbortAllRetired",
                            "AbortAllStopped",
                            "PublishEventRetired",
                            "PublishEventStopped",
                            "StartConversationRunInitializing",
                            "StartImmediateAppendInitializing",
                            "StartImmediateContextInitializing",
                            "PrimitiveAppliedConversation",
                            "PrimitiveAppliedImmediateCompleted",
                            "RegisterPendingOps",
                            "BoundaryCompleteCompleted",
                            "EnterExtraction",
                            "ExtractionStart",
                            "ExtractionValidationPassed",
                            "ExtractionValidationFailedRetry",
                            "ExtractionValidationFailedExhausted",
                            "ExtractionFailedTerminal",
                            "RecoverableFailure",
                            "FatalFailure",
                            "RetryRequested",
                            "BudgetExhausted",
                            "RunCompleted",
                            "RunFailed",
                            "RecoverInputLifecycleRetired",
                            "RecoverInputLifecycleStopped",
                            "QueueAcceptedRetired",
                            "QueueAcceptedStopped",
                            "SteerAcceptedRetired",
                            "SteerAcceptedStopped",
                            "StageForRunRetired",
                            "StageForRunStopped",
                            "IncrementAttemptCountRetired",
                            "IncrementAttemptCountStopped",
                            "RollbackStagedRetired",
                            "RollbackStagedStopped",
                            "ConsumeInputRetired",
                            "ConsumeInputStopped",
                            "RecoverOpsCompletionCursorRetired",
                            "RecoverOpsCompletionCursorStopped",
                            "RecoverCompletionConsumerCursorsRetired",
                            "RecoverCompletionConsumerCursorsStopped",
                            "ClassifySurfaceRequestTerminalPublishInitializing",
                            "ClassifySurfaceRequestTerminalFailedInitializing",
                            "CancelSurfaceRequestPendingInitializing",
                            "PublishSurfaceRequestPendingInitializing",
                            "RecordLiveCommandAcceptedRetired",
                            "RecordLiveCommandAcceptedStopped",
                            "RecordLiveCommandRejectedRetired",
                            "RecordLiveCommandRejectedStopped",
                            "ResolveWaitAllAdmissionAcceptedRetired",
                            "ResolveWaitAllAdmissionAcceptedStopped",
                            "RequestWaitAllRetired",
                            "RequestWaitAllStopped",
                            "CancelWaitAllRetired",
                            "CancelWaitAllStopped",
                            "SpawnDrainRetired",
                            "SpawnDrainStopped",
                            "StopDrainRetired",
                            "StopDrainStopped",
                            "StageVisibilityFilterRetired",
                            "StageVisibilityFilterStopped",
                            "CommitVisibilityFilterRetired",
                            "CommitVisibilityFilterStopped",
                            "ReplaceVisibilityStateRetired",
                            "ReplaceVisibilityStateStopped",
                            "McpServerFailedRetired",
                            "McpServerFailedStopped",
                            "PeerResponseRejectedRetired",
                            "PeerResponseRejectedStopped",
                            "AdvanceSessionContextRetired",
                            "AdvanceSessionContextStopped",
                            "InteractionStreamCompletedRetired",
                            "InteractionStreamCompletedStopped",
                            "BindSupervisorRetired",
                            "BindSupervisorStopped",
                            "RequestSupervisorTrustPublishRetired",
                            "RequestSupervisorTrustPublishStopped",
                        ])
                        .effects(&[
                            "RuntimeBound",
                            "RuntimeRetired",
                            "RuntimeDestroyed",
                            "TurnBoundaryApplied",
                            "TurnRunCompleted",
                            "TurnRunFailed",
                            "RuntimeNotice",
                            "ModelRoutingStatusChanged",
                            "SwitchTurnPersistentReconfigureRequested",
                            "ResolveAdmission",
                            "SubmitRunPrimitive",
                            "IngressAccepted",
                            "ReadyForRun",
                            "InputLifecycleNotice",
                            "IngressNotice",
                            "SilentIntentApplied",
                            "OperationTerminal",
                            "EvictCompletedRecord",
                            "SurfaceRequestAdmissionAccepted",
                            "SurfaceRequestTerminalPublish",
                            "SurfaceRequestCompleted",
                            "RejectSurfaceCall",
                            "McpServerStateChanged",
                            "PeerInteractionStateChanged",
                            "InteractionStreamStateChanged",
                            "LocalEndpointChanged",
                            "PeerProjectionChanged",
                        ]),
                ),
                machine_anchor(
                    "meerkat_public_surface",
                    "MeerkatMachine",
                    "meerkat/src/meerkat_machine.rs",
                    "MeerkatMachine snapshot/diagnostic facade",
                    CoverageClaims::none(),
                ),
            ],
            &[
                scenario(
                    "bind-run-boundary-terminal",
                    "runtime binds, runs work, applies a boundary, and reports a terminal outcome",
                    CoverageClaims::none().effects(&["RuntimeBound"]),
                ),
                scenario(
                    "retire-reset-destroy",
                    "runtime retires, resets, stops, and destroys without reopening superseded work",
                    CoverageClaims::none().transitions(&["Reset", "Destroy"]),
                ),
                scenario(
                    "staged_visibility_apply",
                    "tool visibility staged state promotes into the committed visible revision at a boundary",
                    CoverageClaims::none(),
                ),
                scenario(
                    "turn_interrupt_and_shutdown",
                    "running work records interrupt and shutdown intent without escaping the Meerkat authority boundary",
                    CoverageClaims::none(),
                ),
                scenario(
                    "session_registration_and_binding",
                    "initialize, recover initializing, register, unregister, deferred session stage, keep-alive update, promotion, archive, drop, mob operator access resolve/restore/scope mutation, reconfigure session identity, prepare bindings, ensure executor, attach session ingress, detach ingress, drain exit, and runtime bound/retired/destroyed notices",
                    CoverageClaims::none()
                        .transitions(&[
                            "Initialize",
                            "RegisterSessionRetired",
                            "StageDeferredSession",
                            "UpdateDeferredSessionKeepAlive",
                            "RestoreDeferredSessionArchive",
                            "UnregisterSessionRetired",
                            "PrepareBindingsInitializing",
                            "PrepareBindingsRetired",
                            "DestroyInitializing",
                            "Destroy",
                            "RecoverInitializing",
                            "RecoverRetired",
                            "AttachSessionIngressRetired",
                            "AttachMobIngressRetired",
                            "DetachIngressRetired",
                        ])
                        .effects(&[
                            "RuntimeBound",
                            "RuntimeRetired",
                            "RuntimeDestroyed",
                            "RuntimeNotice",
                            "IngressNotice",
                        ]),
                ),
                scenario(
                    "input_admission_and_queueing",
                    "ingest and publish event, accept input with or without completion, classify input terminality, classify external envelope or plain event, classify peer message, peer request, peer response, and peer ingress, prepare run work, primitive applied conversation or immediate, enter extraction, extraction validation passed, recoverable or fatal failure, budget exhausted, steer accepted, increment attempt count, consume on accept, enqueue classified entry, resolve admission, submit admitted ingress effect, post admission signal, and input or ingress notices",
                    CoverageClaims::none()
                        .transitions(&[
                            "PrimitiveAppliedConversation",
                            "EnterExtraction",
                            "ExtractionValidationPassed",
                            "RecoverableFailure",
                            "FatalFailure",
                            "BudgetExhausted",
                        ])
                        .effects(&[
                            "ResolveAdmission",
                            "SubmitAdmittedIngressEffect",
                            "SubmitRunPrimitive",
                            "IngressAccepted",
                            "PostAdmissionSignal",
                            "IngressNotice",
                            "PeerIngressClassified",
                        ]),
                ),
                scenario(
                    "ops_completion_and_waiters",
                    "abort, wait, abort all, peer ready operation, request cancellation at boundary, completion produced/resolved, wait all satisfied, collect completed result, recover op record, classify operation terminality, classify recovered operation record, recover ops completion cursor, recover/advance completion consumer cursors, evict completed op, collect completed op, submit op event, resolve op lifecycle transition rejected feedback, notify op watcher, reject surface call, retain discard or evict completed terminal records",
                    CoverageClaims::none()
                        .transitions(&["BoundaryCompleteCompleted"])
                        .effects(&[
                            "CompletionResolved",
                            "RetainTerminalRecord",
                            "DiscardRecoveredOperationRecord",
                            "OperationTerminal",
                            "EvictCompletedRecord",
                            "CompletionProduced",
                            "WaitAllSatisfied",
                            "CollectCompletedResult",
                            "SurfaceRequestCompleted",
                            "RejectSurfaceCall",
                        ]),
                ),
                scenario(
                    "realtime_connection_projection",
                    "project realtime intent, begin replace detach binding, require reattach, publish signal, reconnect progress, MCP server connect/connected/failed/disconnected/reload, advance session context, interaction stream reserved/attached/completed/expired/closed early, freshness, policy, and binding rotation",
                    CoverageClaims::none().transitions(&[
                        "McpServerConnectedAttached",
                        "McpServerFailedAttached",
                        "McpServerDisconnectedAttached",
                        "McpServerReloadAttached",
                        "AdvanceSessionContextAttached",
                        "InteractionStreamReservedAttached",
                        "InteractionStreamAttachedAttached",
                        "InteractionStreamCompletedAttached",
                        "InteractionStreamExpiredAttached",
                        "InteractionStreamClosedEarlyAttached",
                    ]),
                ),
                scenario(
                    "product_turn_streaming",
                    "product turn in flight, committed, output started, interrupted, terminal, realtime projection advance/refreshed/reset, client input submitted, mid turn activity, and turn terminated classification",
                    CoverageClaims::none().transitions(&["Reset"]),
                ),
                scenario(
                    "recycle_and_compaction",
                    "recycle from idle or retired, initiate recycle, check compaction, and re-enter ready runtime ownership without preserving stale completed records",
                    CoverageClaims::none()
                        .transitions(&["RunCompleted"])
                        .effects(&["RuntimeRetired", "InitiateRecycle", "CheckCompaction"]),
                ),
                scenario(
                    "model_routing_and_image_operation",
                    "set model routing baseline, request finite switch turn, request until changed switch turn, admit model routing assistant turn, begin image operation, activate image operation override, complete image operation, restore image operation override, project model routing status changed, switch turn denied, switch turn persistent reconfigure requested, switch turn finite override activated/restored, image operation phase changed/denied, and model routing approval terminalized",
                    CoverageClaims::none().effects(&[
                        "ModelRoutingStatusChanged",
                        "SwitchTurnDenied",
                        "SwitchTurnPersistentReconfigureRequested",
                        "SwitchTurnFiniteOverrideActivated",
                        "SwitchTurnFiniteOverrideRestored",
                        "ImageOperationPhaseChanged",
                        "ImageOperationDenied",
                        "ModelRoutingApprovalTerminalized",
                        "OperationTerminal",
                    ]),
                ),
                scenario(
                    "live_topology_and_supervision",
                    "begin live topology reconfigure, mark detached, apply identity or visibility, complete/abort/fail topology, bind/authorize/revoke supervisor, publish/revoke trust edge, comms trust reconcile, and local endpoint publish or clear",
                    CoverageClaims::none()
                        .effects(&["PublishSupervisorTrustEdge", "RevokeSupervisorTrustEdge"]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_mob_machine(),
            &[
                machine_anchor(
                    "mob_handle_surface",
                    "MobMachine",
                    "meerkat-mob/src/runtime/handle.rs",
                    "identity-first public MobMachine handle surface for ensure member, reconcile, and member command routing",
                    CoverageClaims::none(),
                ),
                machine_anchor(
                    "mob_actor_authority",
                    "MobMachine",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine actor authority and command execution for wire, unwire, spawn, ensure member, reconcile, observe runtime, submit work, retire, recover durable incarnations, complete, mark completed, stop/stopped, resume, force cancel, subscribe events, shutdown, classify exact autonomous shutdown interruption versus terminal retirement anchors, destroy, terminalized member, record operator action provenance, flow, run, create frame seed, create loop seed, project frame phase, project loop state, orchestrator, coordinator, cleanup, append failure ledger, escalate supervisor, peer, progress, notices, kickoff pending/replay and resolve started/callback pending/failed/clear, wiring graph, and session binding",
                    CoverageClaims::none()
                        .transitions(&[
                            "ReconcileStopped",
                            "ReconcileCompleted",
                            "KickoffMarkPending",
                            "KickoffMarkPendingReplayRunning",
                            "KickoffMarkPendingReplayStopped",
                            "KickoffMarkPendingReplayCompleted",
                            "KickoffResolveStartedStopped",
                            "KickoffResolveStartedCompleted",
                            "KickoffResolveCallbackPendingStopped",
                            "KickoffResolveCallbackPendingCompleted",
                            "KickoffClearStopped",
                            "KickoffClearCompleted",
                            "RetireMember",
                            "RetireRunningReleasing",
                            "RetireRunningPreservingBinding",
                            "RetireRunningNoBinding",
                            "RetireStoppedReleasing",
                            "RetireStoppedPreservingBinding",
                            "RetireStoppedNoBinding",
                            "MarkCompleted",
                            "DestroyMob",
                            "RecordOperatorActionProvenanceStopped",
                            "RecordOperatorActionProvenanceCompleted",
                            "ResumeStopped",
                            "ClearSupervisorAuthorityForDestroy",
                            "SubscribeMobEventsStopped",
                            "SubscribeMobEventsCompleted",
                            "ShutdownStopped",
                            "ShutdownCompleted",
                            "StopOrchestratorStopped",
                            "StopOrchestratorCompleted",
                            "ResumeOrchestratorStopped",
                            "ResumeOrchestratorCompleted",
                            "DestroyOrchestratorStopped",
                            "DestroyOrchestratorCompleted",
                            "RetireAllStopped",
                            "RetireAllCompleted",
                            "ResolveAutonomousShutdownMemberActionTerminalRetryAnchorRunning",
                            "ResolveAutonomousShutdownMemberActionTerminalRetryAnchorStopped",
                            "ResolveAutonomousShutdownMemberActionTerminalRetryAnchorCompleted",
                            "ResolveAutonomousShutdownMemberActionInterruptRunning",
                            "ResolveAutonomousShutdownMemberActionInterruptStopped",
                            "ResolveAutonomousShutdownMemberActionInterruptCompleted",
                        ])
                        .effects(&[
                            "AppendOperatorActionProvenance",
                            "AppendFailureLedger",
                            "FlowTerminalized",
                            "FlowRunTerminal",
                            "EscalateSupervisor",
                            "AutonomousShutdownMemberActionResolved",
                        ]),
                ),
                machine_anchor(
                    "mob_owner_bridge_cleanup_authority",
                    "MobMachine",
                    "meerkat-mob-mcp/src/lib.rs",
                    "MobMachine owner bridge session cleanup authority for owner bridge cleanup requires owner and implicit delegation requires owner invariants",
                    CoverageClaims::none().invariants(&[
                        "owner_bridge_cleanup_requires_owner",
                        "implicit_delegation_requires_owner",
                        "implicit_delegation_requires_cleanup",
                    ]),
                ),
                machine_anchor(
                    "mob_coordination_board_authority",
                    "MobMachine",
                    "meerkat-mob/src/coordination.rs",
                    "MobMachine coordination board authority: record work intent, record resource claim, update coordination work intent status planned active blocked completed cancelled, update coordination resource claim status active released expired cancelled, observe coordination resource claim overlap, and the recorded/status-changed/overlap-observed coordination effects",
                    CoverageClaims::none()
                        .transitions(&[
                            "RecordCoordinationWorkIntent",
                            "RecordCoordinationResourceClaim",
                            "UpdateCoordinationWorkIntentPlanned",
                            "UpdateCoordinationWorkIntentActive",
                            "UpdateCoordinationWorkIntentBlocked",
                            "UpdateCoordinationWorkIntentCompleted",
                            "UpdateCoordinationWorkIntentCancelled",
                            "UpdateCoordinationResourceClaimActive",
                            "UpdateCoordinationResourceClaimReleased",
                            "UpdateCoordinationResourceClaimExpired",
                            "UpdateCoordinationResourceClaimCancelled",
                            "ObserveCoordinationResourceClaimOverlap",
                        ])
                        .effects(&[
                            "WorkIntentRecorded",
                            "ResourceClaimRecorded",
                            "WorkIntentStatusChanged",
                            "ResourceClaimStatusChanged",
                            "ResourceClaimOverlapObserved",
                        ]),
                ),
                machine_anchor(
                    "mob_operator_admission_authority",
                    "MobMachine",
                    "meerkat-mob-mcp/src/agent_tools.rs",
                    "MobMachine operator-admission authority for the mob tool surface: resolve create mob admission from the create-mobs capability observation and resolve profile mutation admission from the mutate-profiles capability observation, emitting the create-mob and profile-mutation admission resolved verdicts the surface mirrors (denied -> access denied)",
                    CoverageClaims::none().effects(&[
                        "CreateMobAdmissionResolved",
                        "ProfileMutationAdmissionResolved",
                    ]),
                ),
                machine_anchor(
                    "mob_membership_classifier_authority",
                    "MobMachine",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine membership and runtime-incarnation classifiers owned by the actor: probe member admission duplicate or admitted from machine-owned binding and pending-spawn state; compute respawn generation successor; reconcile desired members to spawn retain or retire against current bindings emitting member spawn required, member retain required, and member retire required; set and observe external member rebind capability available or unavailable; classify turn timeout disposition detached canceled or retryable; and seed orphan budget once at startup, emitting the member admission probed, respawn generation computed, external member rebind capability, and turn timeout disposition classified effects",
                    CoverageClaims::none()
                        .transitions(&[
                            "RetireMember",
                            "RetireRunningReleasing",
                            "RetireRunningPreservingBinding",
                            "RetireRunningNoBinding",
                            "RetireStoppedReleasing",
                            "RetireStoppedPreservingBinding",
                            "RetireStoppedNoBinding",
                        ])
                        .effects(&[
                            "MemberAdmissionProbed",
                            "RespawnGenerationComputed",
                            "TurnTimeoutDispositionClassified",
                            "MemberSpawnRequired",
                            "MemberRetainRequired",
                            "MemberRetireRequired",
                        ]),
                ),
                machine_anchor(
                    "mob_flow_fault_topology_escalation_authority",
                    "MobMachine",
                    "meerkat-mob/src/runtime/flow.rs",
                    "MobMachine flow-step fault, topology-edge, and supervisor-escalation classifiers owned by the flow engine: classify step output fault retry or terminal malformed json into a step fault disposition; evaluate topology edge rule allow deny or default into a policy decision verdict; and escalate to supervisor target found with a real supervisor identity or no eligible target, emitting the step output fault classified, topology edge verdict resolved, supervisor escalation requested, and supervisor escalation failed effects",
                    CoverageClaims::none().effects(&[
                        "FlowStepTerminal",
                        "EscalateSupervisor",
                        "StepOutputFaultClassified",
                        "SupervisorEscalationRequested",
                        "SupervisorEscalationFailed",
                        "TopologyEdgeVerdictResolved",
                    ]),
                ),
            ],
            &[
                scenario(
                    "coordination-board-records-and-overlap",
                    "record coordination work intent and resource claim, update coordination work intent and resource claim status across planned active blocked completed cancelled released expired, and observe coordination resource claim overlap with recomputed revision and event sequence",
                    CoverageClaims::none().transitions(&[
                        "RecordCoordinationWorkIntent",
                        "RecordCoordinationResourceClaim",
                        "UpdateCoordinationWorkIntentPlanned",
                        "UpdateCoordinationWorkIntentActive",
                        "UpdateCoordinationWorkIntentBlocked",
                        "UpdateCoordinationWorkIntentCompleted",
                        "UpdateCoordinationWorkIntentCancelled",
                        "UpdateCoordinationResourceClaimActive",
                        "UpdateCoordinationResourceClaimReleased",
                        "UpdateCoordinationResourceClaimExpired",
                        "UpdateCoordinationResourceClaimCancelled",
                        "ObserveCoordinationResourceClaimOverlap",
                    ]),
                ),
                scenario(
                    "spawn-work-terminal",
                    "member spawn, ensure member, reconcile, runtime-ready observation, work submission, and terminal work closure",
                    CoverageClaims::none(),
                ),
                scenario(
                    "retire-recover-destroy",
                    "member retires, durable incarnation recovery preserves monotone identity history, stops/stopped, resumes, shuts down, and destroys cleanly",
                    CoverageClaims::none().transitions(&[
                        "RetireMember",
                        "RetireRunningReleasing",
                        "RetireRunningPreservingBinding",
                        "RetireRunningNoBinding",
                        "RetireStoppedReleasing",
                        "RetireStoppedPreservingBinding",
                        "RetireStoppedNoBinding",
                        "StopRunning",
                        "ResumeStopped",
                        "RespawnRunning",
                    ]),
                ),
                scenario(
                    "wiring-and-session-binding",
                    "wire and unwire members, enforce known identity for session bindings, expose pending spawn, member session binding changed, and wiring lifecycle notices",
                    CoverageClaims::none()
                        .effects(&["ExposePendingSpawn", "MemberSessionBindingChanged"]),
                ),
                scenario(
                    "flow-and-run-lifecycle",
                    "run flow, start flow, create run, create frame seed, create loop seed, project frame phase, project loop state, start run, complete flow, finish run, mark completed, kickoff resolve started or failed, kickoff clear, flow terminalized, and force cancel running work",
                    CoverageClaims::none()
                        .transitions(&[
                            "KickoffResolveStartedRunning",
                            "KickoffResolveStartedCompleted",
                            "KickoffClearRunning",
                            "KickoffClearCompleted",
                            "MarkCompleted",
                            "CompleteRunning",
                            "ForceCancelRunning",
                            "CancelFlowRunning",
                            "RunFlowRunning",
                            "CreateRunSeedRunning",
                            "CreateFrameSeedRunning",
                            "CreateLoopSeedRunning",
                            "StartFlowRunning",
                            "CreateRunRunning",
                            "StartRunRunning",
                            "CompleteFlowRunning",
                            "FinishRunRunning",
                        ])
                        .effects(&["FlowTerminalized", "FlowRunTerminal"]),
                ),
                scenario(
                    "event-subscriptions-and-notices",
                    "subscribe agent, all agent, and mob events; emit member, run, flow, progress, terminal, and wiring notices",
                    CoverageClaims::none().effects(&[
                        "EmitFlowRunNotice",
                        "FlowRunTerminal",
                        "EmitMemberTerminalNotice",
                    ]),
                ),
                scenario(
                    "orchestrator-coordinator-cleanup",
                    "initialize, stop, resume, and destroy orchestrator; bind or unbind coordinator; begin and finish cleanup; notify coordinator and escalate supervisor",
                    CoverageClaims::none().effects(&["EscalateSupervisor", "NotifyCoordinator"]),
                ),
                scenario(
                    "owner-bridge-cleanup",
                    "bind owner bridge session, owner bridge cleanup requires owner, implicit delegation requires owner, and recover owner bridge session authority for archive cleanup",
                    CoverageClaims::none().invariants(&[
                        "owner_bridge_cleanup_requires_owner",
                        "implicit_delegation_requires_owner",
                        "implicit_delegation_requires_cleanup",
                    ]),
                ),
                scenario(
                    "operator-provenance-and-peer-input",
                    "record operator action provenance, trust operation peer, admit peer input, append failure ledger, surface peer-exposed member inputs, and resolve operator create mob admission and profile mutation admission verdicts the tool surface mirrors",
                    CoverageClaims::none().effects(&[
                        "AppendOperatorActionProvenance",
                        "AppendFailureLedger",
                        "AdmitPeerInput",
                    ]),
                ),
                scenario(
                    "membership-admission-respawn-reconcile-rebind-timeout",
                    "probe member admission duplicate or admitted, compute respawn generation, reconcile desired members to spawn retain or retire emitting member spawn required member retain required and member retire required, set and observe external member rebind capability available or unavailable, classify turn timeout disposition detached canceled or retryable, and seed orphan budget",
                    CoverageClaims::none()
                        .transitions(&[
                            "RetireMember",
                            "RetireRunningReleasing",
                            "RetireRunningPreservingBinding",
                            "RetireRunningNoBinding",
                            "RetireStoppedReleasing",
                            "RetireStoppedPreservingBinding",
                            "RetireStoppedNoBinding",
                        ])
                        .effects(&[
                            "MemberSpawnRequired",
                            "MemberRetainRequired",
                            "MemberRetireRequired",
                        ]),
                ),
                scenario(
                    "flow-fault-topology-supervisor-escalation",
                    "classify step output fault retry or terminal malformed json into a step fault disposition, evaluate topology edge rule allow deny or default into a policy decision verdict resolved, and escalate to supervisor target found with eligible supervisor identity or no eligible target emitting supervisor escalation requested or failed",
                    CoverageClaims::none().effects(&[
                        "FlowStepTerminal",
                        "EscalateSupervisor",
                        "SupervisorEscalationRequested",
                        "SupervisorEscalationFailed",
                        "TopologyEdgeVerdictResolved",
                    ]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_schedule_lifecycle_machine(),
            &[machine_anchor(
                "schedule_lifecycle",
                "ScheduleLifecycleMachine",
                "meerkat-schedule/src/lifecycle.rs",
                "Schedule::apply domain-facing lifecycle transition seam over create, revise, update planning config active or paused, planning window, pause, resume, delete, supersede pending occurrences, sync target snapshot for active or paused materialized session bindings, revision, and planning cursor rules",
                CoverageClaims::none()
                    .transitions(&[
                        "CreateSchedule",
                        "ReviseActive",
                        "RevisePaused",
                        "UpdatePlanningConfigActive",
                        "UpdatePlanningConfigPaused",
                        "SyncTargetSnapshotActive",
                        "SyncTargetSnapshotPaused",
                        "DeleteActive",
                        "DeletePaused",
                    ])
                    .effects(&["SupersedePendingOccurrences"]),
            )],
            &[
                scenario(
                    "schedule_pause_resume_delete",
                    "schedule transitions through create, pause, resume, and delete while advancing revision",
                    CoverageClaims::none().transitions(&["CreateSchedule"]),
                ),
                scenario(
                    "schedule_revision_and_planning",
                    "active or paused schedules revise, update planning config active or paused, record planning windows, sync target snapshots for materialized session bindings, confirm superseded occurrences, supersede pending occurrences, maintain positive revision, and require occurrence progress for planning cursor",
                    CoverageClaims::none()
                        .transitions(&[
                            "ReviseActive",
                            "RevisePaused",
                            "UpdatePlanningConfigActive",
                            "UpdatePlanningConfigPaused",
                            "RecordPlanningWindowActive",
                            "SyncTargetSnapshotActive",
                            "SyncTargetSnapshotPaused",
                            "ConfirmOccurrencesSupersededActive",
                            "ConfirmOccurrencesSupersededPaused",
                        ])
                        .effects(&["SupersedePendingOccurrences"])
                        .invariants(&["revision_is_positive"]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_occurrence_lifecycle_machine(),
            &[machine_anchor(
                "occurrence_lifecycle",
                "OccurrenceLifecycleMachine",
                "meerkat-schedule/src/lifecycle.rs",
                "Occurrence::planned_from_schedule and Occurrence::apply domain-facing lifecycle transition seam over plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, classify due no action, due claim eligible, due misfire required, due lease expired, claim, claimed, dispatch, await completion, complete, resolve runtime completion outcome, completed, skip, skipped, misfire, misfired, supersede, superseded, delivery failure, lease expiry, live owner, revision, and failure classification",
                CoverageClaims::none()
                    .transitions(&[
                        "PlanOccurrenceFromPending",
                        "ClassifyDuePendingMisfire",
                        "ClassifyDuePendingClaimEligible",
                        "ClassifyDueClaimedLeaseExpired",
                        "ClassifyDueDispatchingLeaseExpired",
                        "ClassifyDueAwaitingCompletionLeaseExpired",
                        "SyncTargetSnapshotPending",
                        "SyncTargetSnapshotClaimed",
                        "RecordReceiptPending",
                        "RecordReceiptClaimed",
                        "RecordReceiptDispatching",
                        "RecordReceiptAwaitingCompletion",
                        "RecordReceiptCompleted",
                        "RecordReceiptSkipped",
                        "RecordReceiptMisfired",
                        "RecordReceiptSuperseded",
                        "RecordReceiptDeliveryFailed",
                        "ClaimPending",
                        "AwaitCompletionFromDispatching",
                        "RuntimeCompletionCompleted",
                        "DueMisfirePending",
                        "LeaseExpiredFromClaimed",
                        "LeaseExpiredFromDispatching",
                        "LeaseExpiredFromAwaitingCompletion",
                    ])
                    .effects(&[
                        "Claimed",
                        "AwaitingCompletion",
                        "Completed",
                        "Skipped",
                        "Misfired",
                        "Superseded",
                        "DueClaimEligible",
                        "DueMisfireRequired",
                        "DueLeaseExpired",
                        "DeliveryFailed",
                        "LeaseExpired",
                    ]),
            )],
            &[
                scenario(
                    "occurrence_start_complete_fail",
                    "occurrence transitions through pending, running, and terminal lifecycle states",
                    CoverageClaims::none(),
                ),
                scenario(
                    "occurrence_claim_dispatch_completion",
                    "plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, resolve runtime completion outcome, and record claimed/dispatch/awaiting/completed effects",
                    CoverageClaims::none()
                        .transitions(&[
                            "PlanOccurrenceFromPending",
                            "SyncTargetSnapshotPending",
                            "SyncTargetSnapshotClaimed",
                            "RecordReceiptPending",
                            "RecordReceiptClaimed",
                            "RecordReceiptDispatching",
                            "RecordReceiptAwaitingCompletion",
                            "RecordReceiptCompleted",
                            "RecordReceiptSkipped",
                            "RecordReceiptMisfired",
                            "RecordReceiptSuperseded",
                            "RecordReceiptDeliveryFailed",
                            "ClaimPending",
                            "DispatchStartedFromClaimed",
                            "AwaitCompletionFromDispatching",
                            "RuntimeCompletionCompleted",
                        ])
                        .effects(&[
                            "Claimed",
                            "DispatchStarted",
                            "AwaitingCompletion",
                            "Completed",
                            "Skipped",
                            "Misfired",
                            "Superseded",
                            "DeliveryFailed",
                        ]),
                ),
                scenario(
                    "occurrence_terminal_classification",
                    "skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes",
                    CoverageClaims::none()
                        .effects(&[
                            "Skipped",
                            "Misfired",
                            "Superseded",
                            "OccurrencesSuperseded",
                            "DeliveryFailed",
                        ])
                        .invariants(&[
                            "superseded_records_revision",
                            "delivery_failed_records_failure_class",
                        ]),
                ),
                scenario(
                    "occurrence_lease_recovery",
                    "classify due no action, due claim eligible, due misfire required, due lease expired, and lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery",
                    CoverageClaims::none()
                        .transitions(&[
                            "ClassifyDueClaimedLeaseExpired",
                            "ClassifyDueDispatchingLeaseExpired",
                            "ClassifyDueAwaitingCompletionLeaseExpired",
                            "AwaitCompletionFromDispatching",
                            "LeaseExpiredFromClaimed",
                            "LeaseExpiredFromDispatching",
                            "LeaseExpiredFromAwaitingCompletion",
                        ])
                        .effects(&[
                            "Claimed",
                            "AwaitingCompletion",
                            "DueClaimEligible",
                            "DueMisfireRequired",
                            "DueLeaseExpired",
                            "LeaseExpired",
                        ]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_auth_machine(),
            &[
                machine_anchor(
                    "auth_lease_handle",
                    "AuthMachine",
                    "meerkat-runtime/src/handles/auth_lease.rs",
                    "per-binding AuthMachine registry; AuthLeaseHandle trait impl drives acquire, observe credential freshness, expiring, expired, refresh, reauth, release, lifecycle event, and wake loop DSL transitions through it",
                    CoverageClaims::none()
                        .transitions(&[
                            "Acquire",
                            "ObserveCredentialFreshnessExpiring",
                            "ObserveCredentialFreshnessExpired",
                            "Release",
                        ])
                        .effects(&["WakeRefreshLoop"]),
                ),
                machine_anchor(
                    "oauth_flow_handle",
                    "AuthMachine",
                    "meerkat-runtime/src/handles/oauth_flow.rs",
                    "per-binding AuthMachine-owned OAuth browser and device flow lifecycle authority for admit, verify, begin poll, finish poll, consume, expire, valid, expiring, expired, refreshing, and reauth required phases",
                    CoverageClaims::none().transitions(&[
                        "AdmitOAuthBrowserFlowValid",
                        "AdmitOAuthBrowserFlowExpiring",
                        "AdmitOAuthBrowserFlowExpired",
                        "AdmitOAuthBrowserFlowRefreshing",
                        "AdmitOAuthBrowserFlowReauthRequired",
                        "VerifyOAuthBrowserFlowValid",
                        "VerifyOAuthBrowserFlowExpiring",
                        "VerifyOAuthBrowserFlowExpired",
                        "VerifyOAuthBrowserFlowRefreshing",
                        "VerifyOAuthBrowserFlowReauthRequired",
                        "ConsumeOAuthBrowserFlowValid",
                        "ConsumeOAuthBrowserFlowExpiring",
                        "ConsumeOAuthBrowserFlowExpired",
                        "ConsumeOAuthBrowserFlowRefreshing",
                        "ConsumeOAuthBrowserFlowReauthRequired",
                        "ExpireOAuthBrowserFlowValid",
                        "ExpireOAuthBrowserFlowExpiring",
                        "ExpireOAuthBrowserFlowExpired",
                        "ExpireOAuthBrowserFlowRefreshing",
                        "ExpireOAuthBrowserFlowReauthRequired",
                        "AdmitOAuthDeviceFlowValid",
                        "AdmitOAuthDeviceFlowExpiring",
                        "AdmitOAuthDeviceFlowExpired",
                        "AdmitOAuthDeviceFlowRefreshing",
                        "AdmitOAuthDeviceFlowReauthRequired",
                        "VerifyOAuthDeviceFlowValid",
                        "VerifyOAuthDeviceFlowExpiring",
                        "VerifyOAuthDeviceFlowExpired",
                        "VerifyOAuthDeviceFlowRefreshing",
                        "VerifyOAuthDeviceFlowReauthRequired",
                        "BeginOAuthDevicePollValid",
                        "BeginOAuthDevicePollExpiring",
                        "BeginOAuthDevicePollExpired",
                        "BeginOAuthDevicePollRefreshing",
                        "BeginOAuthDevicePollReauthRequired",
                        "FinishOAuthDevicePollValid",
                        "FinishOAuthDevicePollExpiring",
                        "FinishOAuthDevicePollExpired",
                        "FinishOAuthDevicePollRefreshing",
                        "FinishOAuthDevicePollReauthRequired",
                        "ConsumeOAuthDeviceFlowValid",
                        "ConsumeOAuthDeviceFlowExpiring",
                        "ConsumeOAuthDeviceFlowExpired",
                        "ConsumeOAuthDeviceFlowRefreshing",
                        "ConsumeOAuthDeviceFlowReauthRequired",
                        "ExpireOAuthDeviceFlowValid",
                        "ExpireOAuthDeviceFlowExpiring",
                        "ExpireOAuthDeviceFlowExpired",
                        "ExpireOAuthDeviceFlowRefreshing",
                        "ExpireOAuthDeviceFlowReauthRequired",
                    ]),
                ),
            ],
            &[
                scenario(
                    "acquire_expire_refresh_complete",
                    "lease transitions through valid, expiring, expired, refreshing, and back to valid on successful refresh",
                    CoverageClaims::none().transitions(&["Acquire", "CompleteRefresh"]),
                ),
                scenario(
                    "reauth_release_and_publication",
                    "reauth required from valid/expiring/expired/refreshing, observe credential freshness for released state, release lease, emit lifecycle event, and wake refresh loop publication",
                    CoverageClaims::none()
                        .transitions(&[
                            "ObserveCredentialFreshnessValid",
                            "ObserveCredentialFreshnessExpiringFromValid",
                            "ObserveCredentialFreshnessExpiredFromValid",
                            "ObserveCredentialFreshnessExpiring",
                            "ObserveCredentialFreshnessExpiredFromExpiring",
                            "ObserveCredentialFreshnessExpired",
                            "ObserveCredentialFreshnessRefreshing",
                            "ObserveCredentialFreshnessReauthRequired",
                            "ObserveCredentialFreshnessReleased",
                            "Release",
                        ])
                        .effects(&["EmitLifecycleEvent", "WakeRefreshLoop"]),
                ),
                scenario(
                    "oauth_browser_flow_lifecycle",
                    "OAuth browser flow admit, verify, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority",
                    CoverageClaims::none(),
                ),
                scenario(
                    "oauth_device_flow_lifecycle",
                    "OAuth device flow admit, verify, begin poll, finish poll, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority",
                    CoverageClaims::none(),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_approval_lifecycle_machine(),
            &[machine_anchor(
                "approval_lifecycle_authority",
                "ApprovalLifecycleMachine",
                "meerkat-core/src/generated/approval_lifecycle.rs",
                "generated ApprovalLifecycleMachine owner for CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, CreatePending, RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, RestoreRejectedInvalidRecord, ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, ObserveExpiryCancelledNoop, DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, DecideDeny, ApprovalStatusResolved, and ApprovalLifecycleRejected",
                CoverageClaims::none()
                    .transitions(&[
                        "CreateRejectedEmptyAllowedDecisions",
                        "CreateRejectedAlreadyExists",
                        "CreatePending",
                        "RestoreRejectedDuplicate",
                        "RestoreRejectedEmptyAllowedDecisions",
                        "RestorePending",
                        "RestoreExpired",
                        "RestoreCancelled",
                        "RestoreApproved",
                        "RestoreDenied",
                        "RestoreRejectedInvalidRecord",
                        "ObserveExpiryRejectedMissing",
                        "ObserveExpiryExpiresPending",
                        "ObserveExpiryPendingNoop",
                        "ObserveExpiryApprovedNoop",
                        "ObserveExpiryDeniedNoop",
                        "ObserveExpiryExpiredNoop",
                        "ObserveExpiryCancelledNoop",
                        "DecideRejectedMissing",
                        "DecideRejectedExpired",
                        "DecideRejectedAlreadyDecided",
                        "DecideRejectedApproveNotAllowed",
                        "DecideRejectedDenyNotAllowed",
                        "DecideApprove",
                        "DecideDeny",
                    ])
                    .effects(&["ApprovalStatusResolved", "ApprovalLifecycleRejected"]),
            )],
            &[
                scenario(
                    "approval_request_pending",
                    "CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, and CreatePending keep request creation and Pending status projection under ApprovalStatusResolved or ApprovalLifecycleRejected",
                    CoverageClaims::none()
                        .transitions(&[
                            "CreateRejectedEmptyAllowedDecisions",
                            "CreateRejectedAlreadyExists",
                            "CreatePending",
                        ])
                        .effects(&["ApprovalStatusResolved", "ApprovalLifecycleRejected"]),
                ),
                scenario(
                    "approval_decide_terminal",
                    "DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, and DecideDeny move Pending approvals to Approved or Denied only when generated allowed-decision state admits the terminal decision",
                    CoverageClaims::none().transitions(&[
                        "DecideRejectedMissing",
                        "DecideRejectedExpired",
                        "DecideRejectedAlreadyDecided",
                        "DecideRejectedApproveNotAllowed",
                        "DecideRejectedDenyNotAllowed",
                        "DecideApprove",
                        "DecideDeny",
                    ]),
                ),
                scenario(
                    "approval_expiry_feedback",
                    "ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, and ObserveExpiryCancelledNoop consume typed time observation and emit Expired or unchanged status without handwritten status mutation",
                    CoverageClaims::none().transitions(&[
                        "ObserveExpiryRejectedMissing",
                        "ObserveExpiryExpiresPending",
                        "ObserveExpiryPendingNoop",
                        "ObserveExpiryApprovedNoop",
                        "ObserveExpiryDeniedNoop",
                        "ObserveExpiryExpiredNoop",
                        "ObserveExpiryCancelledNoop",
                    ]),
                ),
                scenario(
                    "approval_restore_consistency",
                    "RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, and RestoreRejectedInvalidRecord validate persisted status, decision audit consistency, and allowed-decision compatibility before rehydrating approval lifecycle truth",
                    CoverageClaims::none()
                        .transitions(&[
                            "RestoreRejectedDuplicate",
                            "RestoreRejectedEmptyAllowedDecisions",
                            "RestorePending",
                            "RestoreExpired",
                            "RestoreCancelled",
                            "RestoreApproved",
                            "RestoreDenied",
                            "RestoreRejectedInvalidRecord",
                        ])
                        .effects(&["ApprovalLifecycleRejected"]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_session_document_machine(),
            &[machine_anchor(
                "session_document_authority",
                "SessionDocumentMachine",
                "meerkat-core/src/generated/session_document.rs",
                "generated SessionDocumentMachine owner for MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ResolveSessionFirstTurnOverridesAllowed, ResolveSessionFirstTurnOverridesDenied, StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, ConsumeSessionDeferredInputsConsumed, RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, RecoverSessionFirstTurnPhase, ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, ResolveSystemContextAppendNew, ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, ResolveSystemContextSteerCleanupItemNormal, RestoreSystemContextSnapshot, ResolveRealtimeItemObservedDiscardedAssistant, ResolveRealtimeItemObservedPresent, ResolveRealtimeItemSkipped, ResolveRealtimeUserTranscriptFinalEmpty, ResolveRealtimeUserTranscriptFinalStore, ResolveRealtimeUserTranscriptFinalReplayOrConflict, ResolveRealtimeAssistantDeltaInvalidOrDuplicate, ResolveRealtimeAssistantDeltaDiscarded, ResolveRealtimeAssistantDeltaLaneConflict, ResolveRealtimeAssistantDeltaAccepted, ResolveRealtimeAssistantReplacementInvalid, ResolveRealtimeAssistantReplacementDiscarded, ResolveRealtimeAssistantReplacementLocked, ResolveRealtimeAssistantReplacementLaneConflict, ResolveRealtimeAssistantReplacementAccepted, ResolveRealtimeAssistantTurnCompletedInvalid, ResolveRealtimeAssistantTurnCompletedDiscard, ResolveRealtimeAssistantTurnCompletedToolUse, ResolveRealtimeAssistantTurnCompletedRecord, ResolveRealtimeAssistantTurnInterruptedInvalid, ResolveRealtimeAssistantTurnInterruptedValid, ResolveRealtimeMaterializeAlreadyDone, ResolveRealtimeMaterializeWaitForPredecessor, ResolveRealtimeMaterializeSkipped, ResolveRealtimeMaterializeWaitForReadyText, ResolveRealtimeMaterializeUser, ResolveRealtimeMaterializeAssistant, ResolveRealtimeMaterializeAssistantMissingCompletion, AuthorizeRestoreRealtimeTranscriptState, SessionFirstTurnPhaseResolved, SessionFirstTurnOverridesResolved, SessionInitialPromptStageResolved, SessionToolResultsStageResolved, SessionConsumedInputsRestoreResolved, SessionFirstTurnPhaseRecovered, SystemContextAppendResolved, SystemContextPendingApplyItemResolved, SystemContextSteerCleanupItemResolved, SystemContextSnapshotRestoreAuthorized, RealtimeTranscriptEventResolved, RealtimeMaterializeCandidateResolved, RealtimeTranscriptSnapshotRestoreAuthorized, AuthorizeSessionMetadataPersist, AuthorizeSessionBuildStatePersist, RestoreSessionBuildState, AuthorizeSystemPromptMutation, SessionMetadataPersistAuthorized, SessionBuildStatePersistAuthorized, SessionBuildStateRestoreAuthorized, and SystemPromptMutationAuthorized",
                CoverageClaims::none()
                    .transitions(&[
                        "MarkSessionInitialTurnPendingInactiveOrPending",
                        "MarkSessionInitialTurnPendingConsumed",
                        "StartSessionInitialTurnPending",
                        "StartSessionInitialTurnInactive",
                        "StartSessionInitialTurnConsumed",
                        "ResolveSessionFirstTurnOverridesAllowed",
                        "ResolveSessionFirstTurnOverridesDenied",
                        "StageSessionInitialPromptStore",
                        "StageSessionInitialPromptClear",
                        "StageSessionToolResults",
                        "ConsumeSessionDeferredInputsPending",
                        "ConsumeSessionDeferredInputsInactive",
                        "ConsumeSessionDeferredInputsConsumed",
                        "RestoreSessionConsumedInputs",
                        "RestoreSessionConsumedInputsNoPhaseRollback",
                        "RecoverSessionFirstTurnPhase",
                        "ResolveSystemContextAppendEmpty",
                        "ResolveSystemContextAppendConflict",
                        "ResolveSystemContextAppendDuplicate",
                        "ResolveSystemContextAppendNew",
                        "ResolveSystemContextPendingApplyItemRuntimeSteer",
                        "ResolveSystemContextPendingApplyItemNormal",
                        "ResolveSystemContextSteerCleanupItemRuntimeSteer",
                        "ResolveSystemContextSteerCleanupItemNormal",
                        "RestoreSystemContextSnapshot",
                        "ResolveRealtimeItemObservedDiscardedAssistant",
                        "ResolveRealtimeItemObservedPresent",
                        "ResolveRealtimeItemSkipped",
                        "ResolveRealtimeUserTranscriptFinalEmpty",
                        "ResolveRealtimeUserTranscriptFinalStore",
                        "ResolveRealtimeUserTranscriptFinalReplayOrConflict",
                        "ResolveRealtimeAssistantDeltaInvalidOrDuplicate",
                        "ResolveRealtimeAssistantDeltaDiscarded",
                        "ResolveRealtimeAssistantDeltaLaneConflict",
                        "ResolveRealtimeAssistantDeltaAccepted",
                        "ResolveRealtimeAssistantReplacementInvalid",
                        "ResolveRealtimeAssistantReplacementDiscarded",
                        "ResolveRealtimeAssistantReplacementLocked",
                        "ResolveRealtimeAssistantReplacementLaneConflict",
                        "ResolveRealtimeAssistantReplacementAccepted",
                        "ResolveRealtimeAssistantTurnCompletedInvalid",
                        "ResolveRealtimeAssistantTurnCompletedDiscard",
                        "ResolveRealtimeAssistantTurnCompletedToolUse",
                        "ResolveRealtimeAssistantTurnCompletedRecord",
                        "ResolveRealtimeAssistantTurnInterruptedInvalid",
                        "ResolveRealtimeAssistantTurnInterruptedValid",
                        "ResolveRealtimeMaterializeAlreadyDone",
                        "ResolveRealtimeMaterializeWaitForPredecessor",
                        "ResolveRealtimeMaterializeSkipped",
                        "ResolveRealtimeMaterializeWaitForReadyText",
                        "ResolveRealtimeMaterializeUser",
                        "ResolveRealtimeMaterializeAssistant",
                        "ResolveRealtimeMaterializeAssistantMissingCompletion",
                        "AuthorizeRestoreRealtimeTranscriptState",
                        "AuthorizeSessionMetadataPersist",
                        "AuthorizeSessionBuildStatePersist",
                        "RestoreSessionBuildState",
                        "AuthorizeSystemPromptMutation",
                        "ApplyPendingToolResults",
                        "ResolveRuntimeCheckpointProjectionActive",
                        "ResolveRuntimeCheckpointProjectionArchived",
                        "ResolveLegacyCheckpointMigrationSnapshotIdenticalProjection",
                        "ResolveLegacyCheckpointMigrationSnapshotAheadOfProjection",
                        "ResolveLegacyCheckpointMigrationProjectionExtension",
                        "ResolveLegacyCheckpointMigrationDivergentCopies",
                        "ResolveLegacyCheckpointMigrationSnapshotOnly",
                        "ResolveLegacyCheckpointMigrationStoreRowOnly",
                        "ResolveLegacyCheckpointMigrationSnapshotLegacyProjectionTyped",
                    ])
                    .effects(&[
                        "SessionFirstTurnPhaseResolved",
                        "SessionFirstTurnOverridesResolved",
                        "SessionInitialPromptStageResolved",
                        "SessionToolResultsStageResolved",
                        "SessionConsumedInputsRestoreResolved",
                        "SessionFirstTurnPhaseRecovered",
                        "SystemContextAppendResolved",
                        "SystemContextPendingApplyItemResolved",
                        "SystemContextSteerCleanupItemResolved",
                        "SystemContextSnapshotRestoreAuthorized",
                        "RealtimeTranscriptEventResolved",
                        "RealtimeMaterializeCandidateResolved",
                        "RealtimeTranscriptSnapshotRestoreAuthorized",
                        "SessionMetadataPersistAuthorized",
                        "SessionBuildStatePersistAuthorized",
                        "SessionBuildStateRestoreAuthorized",
                        "SystemPromptMutationAuthorized",
                        "RuntimeCheckpointProjectionResolved",
                        "LegacyCheckpointMigrationResolved",
                    ]),
            )],
            &[
                scenario(
                    "session_first_turn_pending_consume",
                    "MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, and ConsumeSessionDeferredInputsConsumed own the per-session first-turn phase registry and emit SessionFirstTurnPhaseResolved without handwritten phase mutation",
                    CoverageClaims::none()
                        .transitions(&[
                            "MarkSessionInitialTurnPendingInactiveOrPending",
                            "MarkSessionInitialTurnPendingConsumed",
                            "StartSessionInitialTurnPending",
                            "StartSessionInitialTurnInactive",
                            "StartSessionInitialTurnConsumed",
                            "ConsumeSessionDeferredInputsPending",
                            "ConsumeSessionDeferredInputsInactive",
                            "ConsumeSessionDeferredInputsConsumed",
                        ])
                        .effects(&["SessionFirstTurnPhaseResolved"]),
                ),
                scenario(
                    "session_initial_inputs_stage",
                    "StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ResolveSessionFirstTurnOverridesAllowed, and ResolveSessionFirstTurnOverridesDenied resolve initial-prompt and tool-results staging plus build-override legality from the machine-owned phase map under SessionInitialPromptStageResolved, SessionToolResultsStageResolved, and SessionFirstTurnOverridesResolved",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveSessionFirstTurnOverridesAllowed",
                            "ResolveSessionFirstTurnOverridesDenied",
                            "StageSessionInitialPromptStore",
                            "StageSessionInitialPromptClear",
                            "StageSessionToolResults",
                        ])
                        .effects(&[
                            "SessionFirstTurnPhaseResolved",
                            "SessionFirstTurnOverridesResolved",
                            "SessionInitialPromptStageResolved",
                            "SessionToolResultsStageResolved",
                        ]),
                ),
                scenario(
                    "session_first_turn_restore_recover",
                    "RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, and RecoverSessionFirstTurnPhase rehydrate the per-session phase and presence/count registry from consumed-input rollback and durable snapshots under SessionConsumedInputsRestoreResolved and SessionFirstTurnPhaseRecovered",
                    CoverageClaims::none()
                        .transitions(&[
                            "RestoreSessionConsumedInputs",
                            "RestoreSessionConsumedInputsNoPhaseRollback",
                            "RecoverSessionFirstTurnPhase",
                        ])
                        .effects(&[
                            "SessionFirstTurnPhaseResolved",
                            "SessionConsumedInputsRestoreResolved",
                            "SessionFirstTurnPhaseRecovered",
                        ]),
                ),
                scenario(
                    "session_system_context_append_resolve",
                    "ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, and ResolveSystemContextAppendNew decide the runtime system-context append disposition from typed key-present/matches/conflicts observations under SystemContextAppendResolved without the shell deciding",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveSystemContextAppendEmpty",
                            "ResolveSystemContextAppendConflict",
                            "ResolveSystemContextAppendDuplicate",
                            "ResolveSystemContextAppendNew",
                        ])
                        .effects(&["SystemContextAppendResolved"]),
                ),
                scenario(
                    "session_system_context_apply_discard",
                    "ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, and ResolveSystemContextSteerCleanupItemNormal decide per-append apply/discard from the typed SystemContextSource marker (not a runtime:steer string prefix) under SystemContextPendingApplyItemResolved and SystemContextSteerCleanupItemResolved",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveSystemContextPendingApplyItemRuntimeSteer",
                            "ResolveSystemContextPendingApplyItemNormal",
                            "ResolveSystemContextSteerCleanupItemRuntimeSteer",
                            "ResolveSystemContextSteerCleanupItemNormal",
                        ])
                        .effects(&[
                            "SystemContextAppendResolved",
                            "SystemContextPendingApplyItemResolved",
                            "SystemContextSteerCleanupItemResolved",
                        ]),
                ),
                scenario(
                    "session_system_context_snapshot_restore",
                    "RestoreSystemContextSnapshot authorizes a durable system-context snapshot only when key-independent active-turn pending membership and its keyed rollback projection are consistent and seen keys match known appends under SystemContextSnapshotRestoreAuthorized",
                    CoverageClaims::none()
                        .transitions(&["RestoreSystemContextSnapshot"])
                        .effects(&["SystemContextSnapshotRestoreAuthorized"]),
                ),
                scenario(
                    "session_realtime_transcript_event_resolve",
                    "ResolveRealtimeItemObservedDiscardedAssistant, ResolveRealtimeItemObservedPresent, ResolveRealtimeItemSkipped, ResolveRealtimeUserTranscriptFinalEmpty, ResolveRealtimeUserTranscriptFinalStore, ResolveRealtimeUserTranscriptFinalReplayOrConflict, ResolveRealtimeAssistantDeltaInvalidOrDuplicate, ResolveRealtimeAssistantDeltaDiscarded, ResolveRealtimeAssistantDeltaLaneConflict, ResolveRealtimeAssistantDeltaAccepted, ResolveRealtimeAssistantReplacementInvalid, ResolveRealtimeAssistantReplacementDiscarded, ResolveRealtimeAssistantReplacementLocked, ResolveRealtimeAssistantReplacementLaneConflict, ResolveRealtimeAssistantReplacementAccepted, ResolveRealtimeAssistantTurnCompletedInvalid, ResolveRealtimeAssistantTurnCompletedDiscard, ResolveRealtimeAssistantTurnCompletedToolUse, ResolveRealtimeAssistantTurnInterruptedInvalid, and ResolveRealtimeAssistantTurnInterruptedValid resolve the realtime-transcript action vector from typed raw observations (set membership, segment concat emptiness, lane, completion) under RealtimeTranscriptEventResolved without the shell deciding; the shell mirrors the emitted action vector onto its bulky SessionRealtimeTranscriptState",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveRealtimeItemObservedDiscardedAssistant",
                            "ResolveRealtimeItemObservedPresent",
                            "ResolveRealtimeItemSkipped",
                            "ResolveRealtimeUserTranscriptFinalEmpty",
                            "ResolveRealtimeUserTranscriptFinalStore",
                            "ResolveRealtimeUserTranscriptFinalReplayOrConflict",
                            "ResolveRealtimeAssistantDeltaInvalidOrDuplicate",
                            "ResolveRealtimeAssistantDeltaDiscarded",
                            "ResolveRealtimeAssistantDeltaLaneConflict",
                            "ResolveRealtimeAssistantDeltaAccepted",
                            "ResolveRealtimeAssistantReplacementInvalid",
                            "ResolveRealtimeAssistantReplacementDiscarded",
                            "ResolveRealtimeAssistantReplacementLocked",
                            "ResolveRealtimeAssistantReplacementLaneConflict",
                            "ResolveRealtimeAssistantReplacementAccepted",
                            "ResolveRealtimeAssistantTurnCompletedInvalid",
                            "ResolveRealtimeAssistantTurnCompletedDiscard",
                            "ResolveRealtimeAssistantTurnCompletedToolUse",
                            "ResolveRealtimeAssistantTurnInterruptedInvalid",
                            "ResolveRealtimeAssistantTurnInterruptedValid",
                        ])
                        .effects(&["RealtimeTranscriptEventResolved"]),
                ),
                scenario(
                    "session_realtime_transcript_materialize_and_restore",
                    "ResolveRealtimeAssistantTurnCompletedRecord, ResolveRealtimeMaterializeAlreadyDone, ResolveRealtimeMaterializeWaitForPredecessor, ResolveRealtimeMaterializeSkipped, ResolveRealtimeMaterializeWaitForReadyText, ResolveRealtimeMaterializeUser, ResolveRealtimeMaterializeAssistant, ResolveRealtimeMaterializeAssistantMissingCompletion, and AuthorizeRestoreRealtimeTranscriptState resolve the per-item materialize verdict and durable snapshot-restore legality under RealtimeMaterializeCandidateResolved and RealtimeTranscriptSnapshotRestoreAuthorized; the shell performs only the topological ordering and message assembly",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveRealtimeItemSkipped",
                            "ResolveRealtimeAssistantTurnCompletedRecord",
                            "ResolveRealtimeMaterializeAlreadyDone",
                            "ResolveRealtimeMaterializeWaitForPredecessor",
                            "ResolveRealtimeMaterializeSkipped",
                            "ResolveRealtimeMaterializeWaitForReadyText",
                            "ResolveRealtimeMaterializeUser",
                            "ResolveRealtimeMaterializeAssistant",
                            "ResolveRealtimeMaterializeAssistantMissingCompletion",
                            "AuthorizeRestoreRealtimeTranscriptState",
                        ])
                        .effects(&[
                            "RealtimeMaterializeCandidateResolved",
                            "RealtimeTranscriptSnapshotRestoreAuthorized",
                        ]),
                ),
                scenario(
                    "session_durable_config_authorize_restore",
                    "AuthorizeSessionMetadataPersist, AuthorizeSessionBuildStatePersist, RestoreSessionBuildState, and AuthorizeSystemPromptMutation decide durable-config persist/restore/system-prompt admission from typed presence/count/kind observations the meerkat-core shell extracts from SessionMetadata and SessionBuildState under SessionMetadataPersistAuthorized, SessionBuildStatePersistAuthorized, SessionBuildStateRestoreAuthorized, and SystemPromptMutationAuthorized; a rejected request matches no transition and surfaces as Err, and the shell mirrors the verdict and passes the original typed value through unchanged",
                    CoverageClaims::none()
                        .transitions(&[
                            "AuthorizeSessionMetadataPersist",
                            "AuthorizeSessionBuildStatePersist",
                            "RestoreSessionBuildState",
                            "AuthorizeSystemPromptMutation",
                        ])
                        .effects(&[
                            "SessionMetadataPersistAuthorized",
                            "SessionBuildStatePersistAuthorized",
                            "SessionBuildStateRestoreAuthorized",
                            "SystemPromptMutationAuthorized",
                        ]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_session_turn_admission_machine(),
            &[machine_anchor(
                "session_turn_admission_authority",
                "SessionTurnAdmissionMachine",
                "meerkat-session/src/generated/session_turn_admission.rs",
                "generated SessionTurnAdmissionMachine owner for the ephemeral turn-admission lifecycle: ProjectTurnAdmission, ClaimTurn, AbortClaim, BeginTurn, ResolveTurn, FinalizeTurnToShutdown, FinalizeTurnToIdle, RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, RequestShutdownAlreadyShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, AuthorizeCancelAfterBoundaryRunning, AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, ResolveRuntimeKeepAliveEnable, ResolveRuntimeKeepAlivePreserve, and ResolveLastStartTurnPublicTerminalNoPending; effects TurnAdmissionProjected, TurnInterruptRequested, StartTurnDispatchResolved, CancelAfterBoundaryAuthorized, StartTurnDispositionResolved, StartTurnPublicTerminalResolved, RuntimeKeepAliveResolved; invariant shutdown_phase_is_not_active",
                CoverageClaims::none()
                    .transitions(&[
                        "ProjectTurnAdmissionIdle",
                        "ProjectTurnAdmissionAdmitted",
                        "ProjectTurnAdmissionRunning",
                        "ProjectTurnAdmissionCompleting",
                        "ProjectTurnAdmissionShuttingDown",
                        "ClaimTurn",
                        "AbortClaim",
                        "BeginTurn",
                        "ResolveTurn",
                        "FinalizeTurnToShutdown",
                        "FinalizeTurnToIdle",
                        "RequestInterruptAdmittedFirst",
                        "RequestInterruptAdmittedDuplicate",
                        "RequestInterruptRunningFirst",
                        "RequestInterruptRunningDuplicate",
                        "RequestShutdownImmediateIdle",
                        "RequestShutdownImmediateAdmitted",
                        "RequestShutdownDeferredRunning",
                        "RequestShutdownDeferredCompleting",
                        "RequestShutdownAlreadyShuttingDown",
                        "AuthorizeCancelAfterBoundaryAdmitted",
                        "AuthorizeStartTurnDispatchAdmitted",
                        "AuthorizeStartTurnDispatchShuttingDown",
                        "AuthorizeCancelAfterBoundaryRunning",
                        "ResolveDispositionContentTurn",
                        "ResolveDispositionResumePendingWithBoundary",
                        "ResolveDispositionResumePendingWithoutBoundary",
                        "ResolveDispositionDirectPrompt",
                        "ResolveDispositionDirectPending",
                        "ResolveDispositionDirectNoPending",
                        "ResolveRuntimeKeepAliveEnable",
                        "ResolveRuntimeKeepAlivePreserve",
                        "ResolveLastStartTurnPublicTerminalNoPendingIdle",
                        "ResolveLastStartTurnPublicTerminalNoPendingAdmitted",
                        "ResolveLastStartTurnPublicTerminalNoPendingRunning",
                        "ResolveLastStartTurnPublicTerminalNoPendingCompleting",
                        "ResolveLastStartTurnPublicTerminalNoPendingShuttingDown",
                    ])
                    .effects(&[
                        "TurnAdmissionProjected",
                        "TurnInterruptRequested",
                        "StartTurnDispatchResolved",
                        "CancelAfterBoundaryAuthorized",
                        "StartTurnDispositionResolved",
                        "StartTurnPublicTerminalResolved",
                        "RuntimeKeepAliveResolved",
                    ])
                    .invariants(&["shutdown_phase_is_not_active"]),
            )],
            &[
                scenario(
                    "turn_admission_claim_run_finalize",
                    "ClaimTurn, BeginTurn, ResolveTurn, FinalizeTurnToIdle, FinalizeTurnToShutdown, AbortClaim, and ProjectTurnAdmission own the Idle/Admitted/Running/Completing/ShuttingDown turn-admission phase and emit TurnAdmissionProjected without handwritten phase mutation",
                    CoverageClaims::none()
                        .transitions(&[
                            "ProjectTurnAdmissionIdle",
                            "ProjectTurnAdmissionAdmitted",
                            "ProjectTurnAdmissionRunning",
                            "ProjectTurnAdmissionCompleting",
                            "ProjectTurnAdmissionShuttingDown",
                            "ClaimTurn",
                            "AbortClaim",
                            "BeginTurn",
                            "ResolveTurn",
                            "FinalizeTurnToShutdown",
                            "FinalizeTurnToIdle",
                        ])
                        .effects(&["TurnAdmissionProjected"]),
                ),
                scenario(
                    "turn_admission_interrupt_and_shutdown",
                    "RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, and RequestShutdownAlreadyShuttingDown resolve interrupt wake feedback and immediate-or-deferred shutdown under TurnInterruptRequested and TurnAdmissionProjected while preserving the shutdown-not-active invariant",
                    CoverageClaims::none()
                        .transitions(&[
                            "ProjectTurnAdmissionIdle",
                            "ProjectTurnAdmissionAdmitted",
                            "ProjectTurnAdmissionRunning",
                            "ProjectTurnAdmissionCompleting",
                            "ProjectTurnAdmissionShuttingDown",
                            "ResolveTurn",
                            "RequestInterruptAdmittedFirst",
                            "RequestInterruptAdmittedDuplicate",
                            "RequestInterruptRunningFirst",
                            "RequestInterruptRunningDuplicate",
                            "RequestShutdownImmediateIdle",
                            "RequestShutdownImmediateAdmitted",
                            "RequestShutdownDeferredRunning",
                            "RequestShutdownDeferredCompleting",
                            "RequestShutdownAlreadyShuttingDown",
                        ])
                        .effects(&["TurnAdmissionProjected", "TurnInterruptRequested"]),
                ),
                scenario(
                    "turn_admission_dispatch_and_boundary_cancel",
                    "AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, and AuthorizeCancelAfterBoundaryRunning resolve start-turn dispatch authorization and boundary-cancel legality from the admission phase under StartTurnDispatchResolved and CancelAfterBoundaryAuthorized",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveTurn",
                            "AuthorizeCancelAfterBoundaryAdmitted",
                            "AuthorizeStartTurnDispatchAdmitted",
                            "AuthorizeStartTurnDispatchShuttingDown",
                            "AuthorizeCancelAfterBoundaryRunning",
                        ])
                        .effects(&["StartTurnDispatchResolved", "CancelAfterBoundaryAuthorized"]),
                ),
                scenario(
                    "turn_admission_start_turn_disposition",
                    "ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, and ResolveLastStartTurnPublicTerminalNoPending resolve the start-turn disposition from the execution kind, prompt content observation, and the SessionDocumentMachine-emitted PendingContinuationDisposition under StartTurnDispositionResolved and StartTurnPublicTerminalResolved; the shell mirrors the disposition and never decides",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveTurn",
                            "ResolveDispositionContentTurn",
                            "ResolveDispositionResumePendingWithBoundary",
                            "ResolveDispositionResumePendingWithoutBoundary",
                            "ResolveDispositionDirectPrompt",
                            "ResolveDispositionDirectPending",
                            "ResolveDispositionDirectNoPending",
                        ])
                        .effects(&[
                            "StartTurnDispositionResolved",
                            "StartTurnPublicTerminalResolved",
                        ]),
                ),
                scenario(
                    "turn_admission_runtime_keep_alive",
                    "ResolveRuntimeKeepAliveEnable and ResolveRuntimeKeepAlivePreserve resolve runtime keep-alive persistence from the typed keep-alive-policy-present observation under RuntimeKeepAliveResolved",
                    CoverageClaims::none()
                        .transitions(&[
                            "ResolveTurn",
                            "ResolveRuntimeKeepAliveEnable",
                            "ResolveRuntimeKeepAlivePreserve",
                        ])
                        .effects(&["RuntimeKeepAliveResolved"]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_workgraph_lifecycle_machine(),
            &[machine_anchor(
                "workgraph_lifecycle",
                "WorkGraphLifecycleMachine",
                "meerkat-workgraph/src/machine.rs",
                "WorkGraphMachine domain-facing lifecycle transition seam over CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, ClaimOpen, ClaimExpiredInProgress, ReleaseInProgress, BlockOpen, BlockInProgress, BlockBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, ValidateLink, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, ClassifyCreateStatusAdmissionOpen, ClassifyCreateStatusAdmissionBlocked, ClassifyCreateStatusAdmissionDeniedAbsent, ClassifyCreateStatusAdmissionDeniedInProgress, ClassifyCreateStatusAdmissionDeniedCompleted, ClassifyCreateStatusAdmissionDeniedCancelled, ClassifyCreateStatusAdmissionDeniedFailed, ClassifyPublicConfirmationAdmissionSelfAttest, ClassifyPublicConfirmationAdmissionHostConfirmed, ClassifyPublicConfirmationAdmissionPrincipalConfirmed, ClassifyPublicConfirmationAdmissionSupervisor, ClassifyPublicConfirmationAdmissionReviewerQuorum, ClassifyCompletionPolicyMutationAdmissionUnchanged, ClassifyCompletionPolicyMutationAdmissionChanged; effects Created, Updated, Claimed, Released, Blocked, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, LinkValidated, Closed, EvidenceAdded, CreateStatusAdmissionClassified, PublicConfirmationAdmissionClassified, CompletionPolicyMutationAdmissionClassified; invariants absent_has_zero_revision, live_has_positive_revision, terminal_has_terminal_time, claim_only_in_progress, blocked_has_no_claim, terminal_has_no_claim; revision, leases, due eligibility, unresolved blockers, blocker satisfaction, public status defaults, terminality classification, create status admission, public confirmation admission, completion policy mutation admission, and topology legality",
                CoverageClaims::none()
                    .transitions(&[
                        "CreateOpen",
                        "CreateBlocked",
                        "UpdateOpen",
                        "UpdateInProgress",
                        "UpdateBlocked",
                        "ClaimOpen",
                        "ClaimExpiredInProgress",
                        "ReleaseInProgress",
                        "BlockOpen",
                        "BlockInProgress",
                        "BlockBlocked",
                        "RefreshEligibilityOpen",
                        "RefreshEligibilityInProgress",
                        "RefreshEligibilityBlocked",
                        "ValidateLink",
                        "CloseOpenCompleted",
                        "CloseInProgressCompleted",
                        "CloseBlockedCompleted",
                        "CloseOpenCancelled",
                        "CloseInProgressCancelled",
                        "CloseBlockedCancelled",
                        "CloseOpenFailed",
                        "CloseInProgressFailed",
                        "CloseBlockedFailed",
                        "AddEvidenceOpen",
                        "AddEvidenceInProgress",
                        "AddEvidenceBlocked",
                        "AddEvidenceCompleted",
                        "AddEvidenceCancelled",
                        "AddEvidenceFailed",
                        "ClassifyTerminalityTerminalCompleted",
                        "ClassifyTerminalityTerminalCancelled",
                        "ClassifyTerminalityTerminalFailed",
                        "ClassifyTerminalityLiveAbsent",
                        "ClassifyTerminalityLiveOpen",
                        "ClassifyTerminalityLiveInProgress",
                        "ClassifyTerminalityLiveBlocked",
                        "ClassifyBlockerSatisfactionAbsent",
                        "ClassifyBlockerSatisfactionOpen",
                        "ClassifyBlockerSatisfactionInProgress",
                        "ClassifyBlockerSatisfactionBlocked",
                        "ClassifyBlockerSatisfactionCompleted",
                        "ClassifyBlockerSatisfactionCancelled",
                        "ClassifyBlockerSatisfactionFailed",
                        "ClassifyCreateStatusAdmissionOpenAbsent",
                        "ClassifyCreateStatusAdmissionOpenOpen",
                        "ClassifyCreateStatusAdmissionOpenInProgress",
                        "ClassifyCreateStatusAdmissionOpenBlocked",
                        "ClassifyCreateStatusAdmissionOpenCompleted",
                        "ClassifyCreateStatusAdmissionOpenCancelled",
                        "ClassifyCreateStatusAdmissionOpenFailed",
                        "ClassifyCreateStatusAdmissionBlockedAbsent",
                        "ClassifyCreateStatusAdmissionBlockedOpen",
                        "ClassifyCreateStatusAdmissionBlockedInProgress",
                        "ClassifyCreateStatusAdmissionBlockedBlocked",
                        "ClassifyCreateStatusAdmissionBlockedCompleted",
                        "ClassifyCreateStatusAdmissionBlockedCancelled",
                        "ClassifyCreateStatusAdmissionBlockedFailed",
                        "ClassifyCreateStatusAdmissionDeniedAbsentAbsent",
                        "ClassifyCreateStatusAdmissionDeniedAbsentOpen",
                        "ClassifyCreateStatusAdmissionDeniedAbsentInProgress",
                        "ClassifyCreateStatusAdmissionDeniedAbsentBlocked",
                        "ClassifyCreateStatusAdmissionDeniedAbsentCompleted",
                        "ClassifyCreateStatusAdmissionDeniedAbsentCancelled",
                        "ClassifyCreateStatusAdmissionDeniedAbsentFailed",
                        "ClassifyCreateStatusAdmissionDeniedInProgressAbsent",
                        "ClassifyCreateStatusAdmissionDeniedInProgressOpen",
                        "ClassifyCreateStatusAdmissionDeniedInProgressInProgress",
                        "ClassifyCreateStatusAdmissionDeniedInProgressBlocked",
                        "ClassifyCreateStatusAdmissionDeniedInProgressCompleted",
                        "ClassifyCreateStatusAdmissionDeniedInProgressCancelled",
                        "ClassifyCreateStatusAdmissionDeniedInProgressFailed",
                        "ClassifyCreateStatusAdmissionDeniedCompletedAbsent",
                        "ClassifyCreateStatusAdmissionDeniedCompletedOpen",
                        "ClassifyCreateStatusAdmissionDeniedCompletedInProgress",
                        "ClassifyCreateStatusAdmissionDeniedCompletedBlocked",
                        "ClassifyCreateStatusAdmissionDeniedCompletedCompleted",
                        "ClassifyCreateStatusAdmissionDeniedCompletedCancelled",
                        "ClassifyCreateStatusAdmissionDeniedCompletedFailed",
                        "ClassifyCreateStatusAdmissionDeniedCancelledAbsent",
                        "ClassifyCreateStatusAdmissionDeniedCancelledOpen",
                        "ClassifyCreateStatusAdmissionDeniedCancelledInProgress",
                        "ClassifyCreateStatusAdmissionDeniedCancelledBlocked",
                        "ClassifyCreateStatusAdmissionDeniedCancelledCompleted",
                        "ClassifyCreateStatusAdmissionDeniedCancelledCancelled",
                        "ClassifyCreateStatusAdmissionDeniedCancelledFailed",
                        "ClassifyCreateStatusAdmissionDeniedFailedAbsent",
                        "ClassifyCreateStatusAdmissionDeniedFailedOpen",
                        "ClassifyCreateStatusAdmissionDeniedFailedInProgress",
                        "ClassifyCreateStatusAdmissionDeniedFailedBlocked",
                        "ClassifyCreateStatusAdmissionDeniedFailedCompleted",
                        "ClassifyCreateStatusAdmissionDeniedFailedCancelled",
                        "ClassifyCreateStatusAdmissionDeniedFailedFailed",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestAbsent",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestOpen",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestInProgress",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestBlocked",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestCompleted",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestCancelled",
                        "ClassifyCreateCompletionPolicyAdmissionSelfAttestFailed",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedAbsent",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedOpen",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedInProgress",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedBlocked",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedCompleted",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedCancelled",
                        "ClassifyCreateCompletionPolicyAdmissionHostConfirmedFailed",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedAbsent",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedOpen",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedInProgress",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedBlocked",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCompleted",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedCancelled",
                        "ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmedFailed",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorAbsent",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorOpen",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorInProgress",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorBlocked",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorCompleted",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorCancelled",
                        "ClassifyCreateCompletionPolicyAdmissionSupervisorFailed",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumAbsent",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumOpen",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumInProgress",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumBlocked",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCompleted",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumCancelled",
                        "ClassifyCreateCompletionPolicyAdmissionReviewerQuorumFailed",
                        "ClassifyCloseStatusAdmissionCompletedAbsent",
                        "ClassifyCloseStatusAdmissionCompletedOpen",
                        "ClassifyCloseStatusAdmissionCompletedInProgress",
                        "ClassifyCloseStatusAdmissionCompletedBlocked",
                        "ClassifyCloseStatusAdmissionCompletedCompleted",
                        "ClassifyCloseStatusAdmissionCompletedCancelled",
                        "ClassifyCloseStatusAdmissionCompletedFailed",
                        "ClassifyCloseStatusAdmissionCancelledAbsent",
                        "ClassifyCloseStatusAdmissionCancelledOpen",
                        "ClassifyCloseStatusAdmissionCancelledInProgress",
                        "ClassifyCloseStatusAdmissionCancelledBlocked",
                        "ClassifyCloseStatusAdmissionCancelledCompleted",
                        "ClassifyCloseStatusAdmissionCancelledCancelled",
                        "ClassifyCloseStatusAdmissionCancelledFailed",
                        "ClassifyCloseStatusAdmissionFailedAbsent",
                        "ClassifyCloseStatusAdmissionFailedOpen",
                        "ClassifyCloseStatusAdmissionFailedInProgress",
                        "ClassifyCloseStatusAdmissionFailedBlocked",
                        "ClassifyCloseStatusAdmissionFailedCompleted",
                        "ClassifyCloseStatusAdmissionFailedCancelled",
                        "ClassifyCloseStatusAdmissionFailedFailed",
                        "ClassifyCloseStatusAdmissionDeniedAbsentAbsent",
                        "ClassifyCloseStatusAdmissionDeniedAbsentOpen",
                        "ClassifyCloseStatusAdmissionDeniedAbsentInProgress",
                        "ClassifyCloseStatusAdmissionDeniedAbsentBlocked",
                        "ClassifyCloseStatusAdmissionDeniedAbsentCompleted",
                        "ClassifyCloseStatusAdmissionDeniedAbsentCancelled",
                        "ClassifyCloseStatusAdmissionDeniedAbsentFailed",
                        "ClassifyCloseStatusAdmissionDeniedOpenAbsent",
                        "ClassifyCloseStatusAdmissionDeniedOpenOpen",
                        "ClassifyCloseStatusAdmissionDeniedOpenInProgress",
                        "ClassifyCloseStatusAdmissionDeniedOpenBlocked",
                        "ClassifyCloseStatusAdmissionDeniedOpenCompleted",
                        "ClassifyCloseStatusAdmissionDeniedOpenCancelled",
                        "ClassifyCloseStatusAdmissionDeniedOpenFailed",
                        "ClassifyCloseStatusAdmissionDeniedInProgressAbsent",
                        "ClassifyCloseStatusAdmissionDeniedInProgressOpen",
                        "ClassifyCloseStatusAdmissionDeniedInProgressInProgress",
                        "ClassifyCloseStatusAdmissionDeniedInProgressBlocked",
                        "ClassifyCloseStatusAdmissionDeniedInProgressCompleted",
                        "ClassifyCloseStatusAdmissionDeniedInProgressCancelled",
                        "ClassifyCloseStatusAdmissionDeniedInProgressFailed",
                        "ClassifyCloseStatusAdmissionDeniedBlockedAbsent",
                        "ClassifyCloseStatusAdmissionDeniedBlockedOpen",
                        "ClassifyCloseStatusAdmissionDeniedBlockedInProgress",
                        "ClassifyCloseStatusAdmissionDeniedBlockedBlocked",
                        "ClassifyCloseStatusAdmissionDeniedBlockedCompleted",
                        "ClassifyCloseStatusAdmissionDeniedBlockedCancelled",
                        "ClassifyCloseStatusAdmissionDeniedBlockedFailed",
                        "ClassifyPublicConfirmationAdmissionSelfAttestAbsent",
                        "ClassifyPublicConfirmationAdmissionSelfAttestOpen",
                        "ClassifyPublicConfirmationAdmissionSelfAttestInProgress",
                        "ClassifyPublicConfirmationAdmissionSelfAttestBlocked",
                        "ClassifyPublicConfirmationAdmissionSelfAttestCompleted",
                        "ClassifyPublicConfirmationAdmissionSelfAttestCancelled",
                        "ClassifyPublicConfirmationAdmissionSelfAttestFailed",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedAbsent",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedOpen",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedInProgress",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedBlocked",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedCompleted",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedCancelled",
                        "ClassifyPublicConfirmationAdmissionHostConfirmedFailed",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedAbsent",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedOpen",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedInProgress",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedBlocked",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedCompleted",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedCancelled",
                        "ClassifyPublicConfirmationAdmissionPrincipalConfirmedFailed",
                        "ClassifyPublicConfirmationAdmissionSupervisorAbsent",
                        "ClassifyPublicConfirmationAdmissionSupervisorOpen",
                        "ClassifyPublicConfirmationAdmissionSupervisorInProgress",
                        "ClassifyPublicConfirmationAdmissionSupervisorBlocked",
                        "ClassifyPublicConfirmationAdmissionSupervisorCompleted",
                        "ClassifyPublicConfirmationAdmissionSupervisorCancelled",
                        "ClassifyPublicConfirmationAdmissionSupervisorFailed",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumAbsent",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumOpen",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumInProgress",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumBlocked",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumCompleted",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumCancelled",
                        "ClassifyPublicConfirmationAdmissionReviewerQuorumFailed",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedAbsent",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedOpen",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedInProgress",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedBlocked",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedCompleted",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedCancelled",
                        "ClassifyCompletionPolicyMutationAdmissionUnchangedFailed",
                        "ClassifyCompletionPolicyMutationAdmissionChangedAbsent",
                        "ClassifyCompletionPolicyMutationAdmissionChangedOpen",
                        "ClassifyCompletionPolicyMutationAdmissionChangedInProgress",
                        "ClassifyCompletionPolicyMutationAdmissionChangedBlocked",
                        "ClassifyCompletionPolicyMutationAdmissionChangedCompleted",
                        "ClassifyCompletionPolicyMutationAdmissionChangedCancelled",
                        "ClassifyCompletionPolicyMutationAdmissionChangedFailed",
                    ])
                    .effects(&[
                        "Created",
                        "Updated",
                        "Claimed",
                        "Released",
                        "Blocked",
                        "LinkValidated",
                        "Closed",
                        "EvidenceAdded",
                        "BlockerSatisfactionClassified",
                        "CreateStatusAdmissionClassified",
                        "CreateCompletionPolicyAdmissionClassified",
                        "CloseStatusAdmissionClassified",
                        "PublicConfirmationAdmissionClassified",
                        "CompletionPolicyMutationAdmissionClassified",
                        "ConfirmationAdmissionClassified",
                    ])
                    .invariants(&[
                        "absent_has_zero_revision",
                        "live_has_positive_revision",
                        "terminal_has_terminal_time",
                        "claim_only_in_progress",
                        "blocked_has_no_claim",
                        "terminal_has_no_claim",
                    ]),
            )],
            &[
                scenario(
                    "workgraph_create_update_ready_claim",
                    "CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, public create status defaulting, create status admission classifies open and blocked as admissible creation states and denies the rest, and CAS revision",
                    CoverageClaims::none()
                        .transitions(&[
                            "CreateOpen",
                            "CreateBlocked",
                            "UpdateOpen",
                            "UpdateInProgress",
                            "UpdateBlocked",
                            "ClaimOpen",
                            "ClaimExpiredInProgress",
                            "BlockOpen",
                            "BlockInProgress",
                            "BlockBlocked",
                            "RefreshEligibilityOpen",
                            "RefreshEligibilityInProgress",
                            "RefreshEligibilityBlocked",
                        ])
                        .effects(&["Created", "Updated", "Claimed", "Blocked"]),
                ),
                scenario(
                    "workgraph_claim_release_recovery",
                    "only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim",
                    CoverageClaims::none()
                        .transitions(&[
                            "ClaimExpiredInProgress",
                            "ReleaseInProgress",
                            "BlockInProgress",
                            "BlockBlocked",
                        ])
                        .effects(&["Released", "Blocked"])
                        .invariants(&[
                            "claim_only_in_progress",
                            "blocked_has_no_claim",
                            "terminal_has_no_claim",
                        ]),
                ),
                scenario(
                    "workgraph_block_close_evidence",
                    "BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, public close status defaulting, public confirmation admission admits only a self-attested completion policy and denies every other policy as requiring trusted host, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time",
                    CoverageClaims::none()
                        .transitions(&[
                            "BlockOpen",
                            "BlockInProgress",
                            "BlockBlocked",
                            "CloseOpenCompleted",
                            "CloseInProgressCompleted",
                            "CloseBlockedCompleted",
                            "CloseOpenCancelled",
                            "CloseInProgressCancelled",
                            "CloseBlockedCancelled",
                            "CloseOpenFailed",
                            "CloseInProgressFailed",
                            "CloseBlockedFailed",
                            "AddEvidenceOpen",
                            "AddEvidenceInProgress",
                            "AddEvidenceBlocked",
                            "AddEvidenceCompleted",
                            "AddEvidenceCancelled",
                            "AddEvidenceFailed",
                        ])
                        .effects(&["Blocked", "Closed", "EvidenceAdded"])
                        .invariants(&[
                            "absent_has_zero_revision",
                            "live_has_positive_revision",
                            "terminal_has_terminal_time",
                        ]),
                ),
                scenario(
                    "workgraph_topology_legality",
                    "ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine",
                    CoverageClaims::none()
                        .transitions(&[
                            "BlockOpen",
                            "BlockInProgress",
                            "BlockBlocked",
                            "ValidateLink",
                            "ClassifyTerminalityTerminalCompleted",
                            "ClassifyTerminalityTerminalCancelled",
                            "ClassifyTerminalityTerminalFailed",
                        ])
                        .effects(&["Blocked", "LinkValidated"]),
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_work_attention_lifecycle_machine(),
            &[machine_anchor(
                "work_attention_lifecycle",
                "WorkAttentionLifecycleMachine",
                "meerkat-workgraph/src/machine.rs",
                "WorkAttentionMachine domain-facing lifecycle transition seam over Pause, Resume, Stop, and Supersede; effects Paused, Resumed, Stopped, Superseded; invariants active_has_no_pause_deadline, paused_has_pause_deadline, stopped_has_stop_time, superseded_has_target; revision, timed pause eligibility, stopped state, and supersession target ownership",
                CoverageClaims::none()
                    .transitions(&[
                        "PauseActive",
                        "PausePaused",
                        "ResumePaused",
                        "SupersedeActive",
                        "SupersedePaused",
                        "StopActive",
                        "StopPaused",
                    ])
                    .effects(&[
                        "AttentionPaused",
                        "AttentionResumed",
                        "AttentionSuperseded",
                        "AttentionStopped",
                    ])
                    .invariants(&["paused_has_pause_state"]),
            )],
            &[scenario(
                "work_attention_pause_resume_stop",
                "PauseActive, PausePaused, ResumePaused, SupersedeActive, SupersedePaused, StopActive, StopPaused, AttentionPaused, AttentionResumed, AttentionSuperseded, AttentionStopped, live_has_no_terminal_time, paused_has_pause_state, superseded_records_successor, timed pause eligibility, CAS revision, and terminal work item attention stop stay under WorkAttentionLifecycleMachine authority",
                CoverageClaims::none()
                    .transitions(&[
                        "PauseActive",
                        "PausePaused",
                        "ResumePaused",
                        "SupersedeActive",
                        "SupersedePaused",
                        "StopActive",
                        "StopPaused",
                    ])
                    .effects(&[
                        "AttentionPaused",
                        "AttentionResumed",
                        "AttentionSuperseded",
                        "AttentionStopped",
                    ])
                    .invariants(&[
                        "live_has_no_terminal_time",
                        "paused_has_pause_state",
                        "superseded_records_successor",
                    ]),
            )],
        ),
    ]
}

pub fn canonical_composition_coverage_manifests() -> Vec<CompositionCoverageManifest> {
    vec![
        composition_manifest_from_schema(
            &meerkat_mob_seam_composition(),
            &[
                route_anchor(
                    "mob_meerkat_seam",
                    "binding_request_reaches_meerkat",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine to MeerkatMachine seam realization for binding requests, work submission, cancellation, lifecycle notices, terminal outcomes, and peer ingress",
                    CoverageClaims::none(),
                ),
                machine_anchor(
                    "meerkat_runtime_entry",
                    "MeerkatMachine",
                    "meerkat-runtime/src/meerkat_machine/mod.rs",
                    "MeerkatMachine command authority consuming runtime binding, admitted work, cancellation, lifecycle, terminal, and peer ingress seam traffic",
                    CoverageClaims::none(),
                ),
            ],
            &[
                scenario(
                    "binding_round_trip",
                    "mob runtime binding request becomes a Meerkat binding and feeds readiness back to Mob",
                    CoverageClaims::none(),
                ),
                scenario(
                    "work_round_trip",
                    "mob submits work into Meerkat and observes terminal work outcomes back across the seam",
                    CoverageClaims::none(),
                ),
                scenario(
                    "peer-ingress-and-cancellation",
                    "peer input admission and cancellation requests cross the MobMachine to MeerkatMachine seam with explicit lifecycle notice feedback",
                    CoverageClaims::none(),
                ),
            ],
        ),
        composition_manifest_from_schema(
            &schedule_bundle_composition(),
            &[
                route_anchor(
                    "schedule_service",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-schedule/src/service.rs",
                    "schedule service precursor for revision supersession, rolling planning, occurrence materialization, pause resume, and delete lifecycle routing",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "schedule_store",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-schedule/src/store.rs",
                    "schedule store contract precursor for transactional claim, supersede persistence, occurrence progress, and revision-aware planning cursor updates",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "schedule_bundle_schema",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule bundle composition",
                    CoverageClaims::none(),
                ),
            ],
            &[
                scenario(
                    "revision-supersede-route",
                    "revision-affecting schedule updates supersede pending future occurrences through the explicit route",
                    CoverageClaims::none(),
                ),
                scenario(
                    "pause-resume-without-revision",
                    "pause and resume leave schedule revision unchanged while preserving typed ownership",
                    CoverageClaims::none(),
                ),
                scenario(
                    "rolling-planning-occurrence-materialization",
                    "rolling planning records a planning window and materializes or supersedes pending occurrences through revision-aware schedule routes",
                    CoverageClaims::none(),
                ),
            ],
        ),
        composition_manifest_from_schema(
            &schedule_runtime_bundle_composition(),
            &[
                route_anchor(
                    "schedule_driver",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-schedule/src/driver.rs",
                    "mechanical scheduler driver precursor for runtime-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "runtime_delivery_precursor",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-rpc/src/session_runtime.rs",
                    "runtime-owned prompt/event delivery precursor that scheduling must hand off into for dispatch, completion, failure, and lease recovery",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "schedule_runtime_bundle_schema",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule runtime bundle composition",
                    CoverageClaims::none(),
                ),
            ],
            &[
                scenario(
                    "runtime-delivery-feedback",
                    "DispatchToRuntime is realized by runtime-owned delivery and closed by typed completion feedback",
                    CoverageClaims::none(),
                ),
                scenario(
                    "runtime-lease-expiry",
                    "runtime owner fairness still allows lease expiry to return a stuck occurrence to claimable",
                    CoverageClaims::none(),
                ),
                scenario(
                    "runtime-revision-supersede",
                    "schedule revision supersede enters occurrence authority before runtime handoff so stale pending work is cancelled explicitly",
                    CoverageClaims::none()
                        .routes(&["revision_supersede_enters_occurrence_authority"]),
                ),
            ],
        ),
        composition_manifest_from_schema(
            &schedule_mob_bundle_composition(),
            &[
                route_anchor(
                    "schedule_driver",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-schedule/src/driver.rs",
                    "mechanical scheduler driver precursor for mob-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "mob_delivery_precursor",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-mob-mcp/src/lib.rs",
                    "mob-owned action delivery precursor that scheduling must hand off into for dispatch, completion, target materialization failure, and lease recovery",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "schedule_mob_bundle_schema",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule mob bundle composition",
                    CoverageClaims::none(),
                ),
            ],
            &[
                scenario(
                    "mob-delivery-feedback",
                    "DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback",
                    CoverageClaims::none(),
                ),
                scenario(
                    "materialization-failure-classification",
                    "mob-side delivery failure preserves explicit TargetMaterializationFailed classification",
                    CoverageClaims::none(),
                ),
                scenario(
                    "mob-revision-supersede",
                    "schedule revision supersede enters occurrence authority before mob handoff so stale pending work is cancelled explicitly",
                    CoverageClaims::none()
                        .routes(&["revision_supersede_enters_occurrence_authority"]),
                ),
            ],
        ),
        composition_manifest_from_schema(
            &adaptive_mob_bundle_composition(),
            &[
                machine_anchor(
                    "adaptive_mob_bundle_kernel",
                    "MobMachine",
                    "meerkat-mob/src/runtime/handle.rs",
                    "adaptive Mobpack control mob owns the adaptive run kernel while layer mobs publish terminal classifications through the driver seam",
                    CoverageClaims::none(),
                ),
                machine_anchor(
                    "adaptive_mob_bundle_driver",
                    "MobMachine",
                    "meerkat-mob/src/generated/adaptive_mob_bundle.rs",
                    "generated adaptive bundle driver watches layer terminal classification and dispatches typed terminal feedback into the control mob adaptive kernel",
                    CoverageClaims::none(),
                ),
            ],
            &[scenario(
                "layer-terminal-feedback",
                "a terminal child layer mob is observed by the adaptive bundle driver and fed back to the control mob adaptive kernel without a direct static route",
                CoverageClaims::none(),
            )],
        ),
        composition_manifest_from_schema(
            &auth_lease_bundle_composition(),
            &[
                machine_anchor(
                    "auth_lease_handle",
                    "AuthMachine",
                    "meerkat-runtime/src/handles/auth_lease.rs",
                    "runtime auth lease owner consumes canonical AuthMachine lifecycle acquire, refresh, reauth, release, wake, and publication events",
                    CoverageClaims::none(),
                ),
                machine_anchor(
                    "auth_lease_bundle_schema",
                    "AuthMachine",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal AuthMachine lifecycle publication handoff composition",
                    CoverageClaims::none(),
                ),
            ],
            &[scenario(
                "auth-lease-lifecycle-publication",
                "AuthMachine acquire, refresh, reauth, release, wake, and lifecycle transitions publish through the explicit auth lease handoff protocol",
                CoverageClaims::none(),
            )],
        ),
        composition_manifest_from_schema(
            &workgraph_attention_bundle_composition(),
            &[
                route_anchor(
                    "workgraph_attention_service_close",
                    "work_item_close_stops_attention",
                    "meerkat-workgraph/src/service.rs",
                    "WorkGraph service close path realizes the canonical WorkGraph Closed to WorkAttention Stop route with an atomic item-and-attention CAS update",
                    CoverageClaims::none(),
                ),
                route_anchor(
                    "workgraph_attention_bundle_schema",
                    "work_item_close_stops_attention",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal WorkGraph item closure to WorkAttention stop composition",
                    CoverageClaims::none(),
                ),
            ],
            &[scenario(
                "close-stops-attention",
                "terminal WorkGraph item closure routes to WorkAttention Stop so live goal attention bindings cannot survive their target item",
                CoverageClaims::none().routes(&["work_item_close_stops_attention"]),
            )],
        ),
    ]
}

fn machine_manifest_from_schema(
    schema: &MachineSchema,
    code_anchors: &[CoverageAnchor],
    scenarios: &[ScenarioCoverage],
) -> MachineCoverageManifest {
    MachineCoverageManifest {
        machine: schema.machine.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        transition_coverage: schema
            .transitions
            .iter()
            .map(|transition| {
                claimed_entry(
                    transition.name.as_str(),
                    code_anchors,
                    scenarios,
                    |claims, name| claims.claims_transition(name),
                )
            })
            .collect(),
        effect_coverage: schema
            .effects
            .variants
            .iter()
            .map(|effect| {
                claimed_entry(
                    effect.name.as_str(),
                    code_anchors,
                    scenarios,
                    |claims, name| claims.claims_effect(name),
                )
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| {
                claimed_entry(&invariant.name, code_anchors, scenarios, |claims, name| {
                    claims.claims_invariant(name)
                })
            })
            .collect(),
    }
}

fn composition_manifest_from_schema(
    schema: &CompositionSchema,
    code_anchors: &[CoverageAnchor],
    scenarios: &[ScenarioCoverage],
) -> CompositionCoverageManifest {
    CompositionCoverageManifest {
        composition: schema.name.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        route_coverage: schema
            .routes
            .iter()
            .map(|route| {
                claimed_entry(
                    route.name.as_str(),
                    code_anchors,
                    scenarios,
                    |claims, name| claims.claims_route(name),
                )
            })
            .collect(),
        scheduler_rule_coverage: schema
            .scheduler_rules
            .iter()
            .map(|rule| {
                claimed_entry(
                    &scheduler_rule_coverage_name(rule),
                    code_anchors,
                    scenarios,
                    |claims, name| claims.claims_scheduler_rule(name),
                )
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| {
                claimed_entry(&invariant.name, code_anchors, scenarios, |claims, name| {
                    claims.claims_invariant(name)
                })
            })
            .collect(),
    }
}

/// Build one semantic coverage entry from the explicit typed claims: the
/// element is attributed to exactly the anchors/scenarios that claim it.
/// An element nothing claims is honestly UNCLAIMED (empty id lists).
fn claimed_entry(
    name: &str,
    anchors: &[CoverageAnchor],
    scenarios: &[ScenarioCoverage],
    claimed_by: impl Fn(&CoverageClaims, &str) -> bool,
) -> SemanticCoverageEntry {
    SemanticCoverageEntry {
        name: name.to_owned(),
        anchor_ids: anchors
            .iter()
            .filter(|anchor| claimed_by(&anchor.claims, name))
            .map(|anchor| anchor.id.clone())
            .collect(),
        scenario_ids: scenarios
            .iter()
            .filter(|scenario| claimed_by(&scenario.claims, name))
            .map(|scenario| scenario.id.clone())
            .collect(),
    }
}

/// Construct an anchor whose schema target is a canonical machine.
///
/// `machine` must name a machine in the canonical schema set; the `xtask`
/// coverage validator resolves it (and every element claim) and fails closed
/// otherwise.
fn machine_anchor(
    id: &str,
    machine: &str,
    symbol: &str,
    note: &str,
    claims: CoverageClaims,
) -> CoverageAnchor {
    CoverageAnchor {
        id: id.into(),
        symbol: SymbolRef(symbol.into()),
        target: CoverageSchemaTarget::Machine(
            MachineId::parse(machine).expect("valid machine slug"),
        ),
        note: note.into(),
        claims,
    }
}

/// Construct an anchor whose schema target is a declared composition route.
///
/// `route` must name a route declared by the owning composition; the `xtask`
/// coverage validator resolves it (and every element claim) and fails closed
/// otherwise.
fn route_anchor(
    id: &str,
    route: &str,
    symbol: &str,
    note: &str,
    claims: CoverageClaims,
) -> CoverageAnchor {
    CoverageAnchor {
        id: id.into(),
        symbol: SymbolRef(symbol.into()),
        target: CoverageSchemaTarget::Route(RouteId::parse(route).expect("valid route slug")),
        note: note.into(),
        claims,
    }
}

fn scenario(id: &str, summary: &str, claims: CoverageClaims) -> ScenarioCoverage {
    ScenarioCoverage {
        id: id.into(),
        summary: summary.into(),
        claims,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claimed_entry_attributes_exactly_the_declared_claims() {
        // Attribution is the explicit typed claim binding — never note prose.
        let anchors = vec![
            machine_anchor(
                "claiming_anchor",
                "MeerkatMachine",
                "meerkat-runtime/src/meerkat_machine/mod.rs",
                "prose mentioning register session everywhere",
                CoverageClaims::none().transitions(&["RegisterSession"]),
            ),
            machine_anchor(
                "non_claiming_anchor",
                "MeerkatMachine",
                "meerkat/src/meerkat_machine.rs",
                "prose also mentioning register session",
                CoverageClaims::none(),
            ),
        ];
        let scenarios = vec![scenario(
            "claiming_scenario",
            "summary irrelevant to attribution",
            CoverageClaims::none().transitions(&["RegisterSession"]),
        )];

        let entry = claimed_entry("RegisterSession", &anchors, &scenarios, |claims, name| {
            claims.claims_transition(name)
        });
        assert_eq!(entry.anchor_ids, vec!["claiming_anchor".to_owned()]);
        assert_eq!(entry.scenario_ids, vec!["claiming_scenario".to_owned()]);

        // An element nothing claims is honestly UNCLAIMED — note prose that
        // happens to mention it attributes nothing.
        let unclaimed = claimed_entry(
            "RegisterRealtimeEndpoint",
            &anchors,
            &scenarios,
            |claims, name| claims.claims_transition(name),
        );
        assert!(unclaimed.anchor_ids.is_empty());
        assert!(unclaimed.scenario_ids.is_empty());
    }

    #[test]
    fn canonical_manifests_construct_with_typed_claims() {
        // The canonical manifests must still build; unclaimed entries are
        // permitted, claims of nonexistent elements are rejected by the
        // xtask coverage gate.
        let manifests = canonical_machine_coverage_manifests();
        assert!(!manifests.is_empty());
        let compositions = canonical_composition_coverage_manifests();
        assert!(!compositions.is_empty());
    }
}

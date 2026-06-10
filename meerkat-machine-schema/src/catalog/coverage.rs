// Scoped clippy allow: the anchor constructors below parse hand-authored,
// compile-time-known machine/route slugs that name canonical schema elements.
// A parse failure here is a coverage-manifest authoring bug, never reachable
// from wire input. Inlining `parse(...)` error handling at every one of the
// ~46 anchor construction sites would drown the coverage manifest in
// boilerplate, so the typed-target constructors centralize it. The validator
// in `xtask` resolves every anchor target against the canonical schema and
// fails closed on any anchor that names a machine/route absent from the schema.
#![allow(clippy::expect_used)]

use crate::identity::{MachineId, RouteId};
use crate::{CompositionSchema, MachineSchema};

use super::{
    compositions::{
        auth_lease_bundle_composition, meerkat_mob_seam_composition, schedule_bundle_composition,
        schedule_mob_bundle_composition, schedule_runtime_bundle_composition,
        workgraph_attention_bundle_composition,
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
    /// Human-readable description of what the anchor covers. Consumed by the
    /// semantic-id matcher to associate transitions/effects/routes with anchors.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioCoverage {
    pub id: String,
    pub summary: String,
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
                ),
                machine_anchor(
                    "meerkat_public_surface",
                    "MeerkatMachine",
                    "meerkat/src/meerkat_machine.rs",
                    "MeerkatMachine snapshot/diagnostic facade",
                ),
            ],
            &[
                scenario(
                    "bind-run-boundary-terminal",
                    "runtime binds, runs work, applies a boundary, and reports a terminal outcome",
                ),
                scenario(
                    "retire-reset-destroy",
                    "runtime retires, resets, stops, and destroys without reopening superseded work",
                ),
                scenario(
                    "staged_visibility_apply",
                    "tool visibility staged state promotes into the committed visible revision at a boundary",
                ),
                scenario(
                    "turn_interrupt_and_shutdown",
                    "running work records interrupt and shutdown intent without escaping the Meerkat authority boundary",
                ),
                scenario(
                    "session_registration_and_binding",
                    "initialize, recover initializing, register, unregister, deferred session stage, keep-alive update, promotion, archive, drop, mob operator access resolve/restore/scope mutation, reconfigure session identity, prepare bindings, ensure executor, attach session ingress, detach ingress, drain exit, and runtime bound/retired/destroyed notices",
                ),
                scenario(
                    "input_admission_and_queueing",
                    "ingest and publish event, accept input with or without completion, classify input terminality, classify external envelope or plain event, classify peer message, peer request, peer response, and peer ingress, prepare run work, primitive applied conversation or immediate, enter extraction, extraction validation passed, recoverable or fatal failure, budget exhausted, steer accepted, increment attempt count, consume on accept, enqueue classified entry, resolve admission, submit admitted ingress effect, post admission signal, and input or ingress notices",
                ),
                scenario(
                    "ops_completion_and_waiters",
                    "abort, wait, abort all, peer ready operation, request cancellation at boundary, completion produced/resolved, wait all satisfied, collect completed result, recover op record, classify operation terminality, classify recovered operation record, recover ops completion cursor, recover/advance completion consumer cursors, evict completed op, collect completed op, submit op event, resolve op lifecycle transition rejected feedback, notify op watcher, reject surface call, retain discard or evict completed terminal records",
                ),
                scenario(
                    "realtime_connection_projection",
                    "project realtime intent, begin replace detach binding, require reattach, publish signal, reconnect progress, MCP server connect/connected/failed/disconnected/reload, advance session context, interaction stream reserved/attached/completed/expired/closed early, freshness, policy, and binding rotation",
                ),
                scenario(
                    "product_turn_streaming",
                    "product turn in flight, committed, output started, interrupted, terminal, realtime projection advance/refreshed/reset, client input submitted, mid turn activity, and turn terminated classification",
                ),
                scenario(
                    "recycle_and_compaction",
                    "recycle from idle or retired, initiate recycle, check compaction, and re-enter ready runtime ownership without preserving stale completed records",
                ),
                scenario(
                    "model_routing_and_image_operation",
                    "set model routing baseline, request finite switch turn, request until changed switch turn, admit model routing assistant turn, begin image operation, activate image operation override, complete image operation, restore image operation override, project model routing status changed, switch turn denied, switch turn persistent reconfigure requested, switch turn finite override activated/restored, image operation phase changed/denied, and model routing approval terminalized",
                ),
                scenario(
                    "live_topology_and_supervision",
                    "begin live topology reconfigure, mark detached, apply identity or visibility, complete/abort/fail topology, bind/authorize/revoke supervisor, publish/revoke trust edge, comms trust reconcile, and local endpoint publish or clear",
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
                ),
                machine_anchor(
                    "mob_actor_authority",
                    "MobMachine",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine actor authority and command execution for wire, unwire, spawn, ensure member, reconcile, observe runtime, submit work, retire, reset, respawn, complete, mark completed, stop/stopped, resume, force cancel, subscribe events, shutdown, destroy, terminalized member, record operator action provenance, flow, run, create frame seed, create loop seed, project frame phase, project loop state, orchestrator, coordinator, cleanup, append failure ledger, escalate supervisor, peer, progress, notices, kickoff resolve started/callback pending/failed/clear, wiring graph, and session binding",
                ),
                machine_anchor(
                    "mob_owner_bridge_cleanup_authority",
                    "MobMachine",
                    "meerkat-mob-mcp/src/lib.rs",
                    "MobMachine owner bridge session cleanup authority for owner bridge cleanup requires owner and implicit delegation requires owner invariants",
                ),
                machine_anchor(
                    "mob_coordination_board_authority",
                    "MobMachine",
                    "meerkat-mob/src/coordination.rs",
                    "MobMachine coordination board authority: record work intent, record resource claim, update coordination work intent status planned active blocked completed cancelled, update coordination resource claim status active released expired cancelled, observe coordination resource claim overlap, and the recorded/status-changed/overlap-observed coordination effects",
                ),
                machine_anchor(
                    "mob_operator_admission_authority",
                    "MobMachine",
                    "meerkat-mob-mcp/src/agent_tools.rs",
                    "MobMachine operator-admission authority for the mob tool surface: resolve create mob admission from the create-mobs capability observation and resolve profile mutation admission from the mutate-profiles capability observation, emitting the create-mob and profile-mutation admission resolved verdicts the surface mirrors (denied -> access denied)",
                ),
                machine_anchor(
                    "mob_membership_classifier_authority",
                    "MobMachine",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine membership and runtime-incarnation classifiers owned by the actor: probe member admission duplicate or admitted from machine-owned binding and pending-spawn state; compute respawn generation successor; reconcile desired members to spawn retain or retire against current bindings emitting member spawn required, member retain required, and member retire required; set and observe external member rebind capability available or unavailable; classify turn timeout disposition detached canceled or retryable; and seed orphan budget once at startup, emitting the member admission probed, respawn generation computed, external member rebind capability, and turn timeout disposition classified effects",
                ),
                machine_anchor(
                    "mob_flow_fault_topology_escalation_authority",
                    "MobMachine",
                    "meerkat-mob/src/runtime/flow.rs",
                    "MobMachine flow-step fault, topology-edge, and supervisor-escalation classifiers owned by the flow engine: classify step output fault retry or terminal malformed json into a step fault disposition; evaluate topology edge rule allow deny or default into a policy decision verdict; and escalate to supervisor target found with a real supervisor identity or no eligible target, emitting the step output fault classified, topology edge verdict resolved, supervisor escalation requested, and supervisor escalation failed effects",
                ),
            ],
            &[
                scenario(
                    "coordination-board-records-and-overlap",
                    "record coordination work intent and resource claim, update coordination work intent and resource claim status across planned active blocked completed cancelled released expired, and observe coordination resource claim overlap with recomputed revision and event sequence",
                ),
                scenario(
                    "spawn-work-terminal",
                    "member spawn, ensure member, reconcile, runtime-ready observation, work submission, and terminal work closure",
                ),
                scenario(
                    "retire-respawn-destroy",
                    "member retires, resets, respawns with a new runtime incarnation, stops/stopped, resumes, shuts down, destroys cleanly, and resets to running when reusable",
                ),
                scenario(
                    "wiring-and-session-binding",
                    "wire and unwire members, enforce known identity for session bindings, expose pending spawn, member session binding changed, and wiring lifecycle notices",
                ),
                scenario(
                    "flow-and-run-lifecycle",
                    "run flow, start flow, create run, create frame seed, create loop seed, project frame phase, project loop state, start run, complete flow, finish run, mark completed, kickoff resolve started or failed, kickoff clear, flow terminalized, and force cancel running work",
                ),
                scenario(
                    "event-subscriptions-and-notices",
                    "subscribe agent, all agent, and mob events; emit member, run, flow, progress, terminal, and wiring notices",
                ),
                scenario(
                    "orchestrator-coordinator-cleanup",
                    "initialize, stop, resume, and destroy orchestrator; bind or unbind coordinator; begin and finish cleanup; notify coordinator and escalate supervisor",
                ),
                scenario(
                    "owner-bridge-cleanup",
                    "bind owner bridge session, owner bridge cleanup requires owner, implicit delegation requires owner, and recover owner bridge session authority for archive cleanup",
                ),
                scenario(
                    "operator-provenance-and-peer-input",
                    "record operator action provenance, trust operation peer, admit peer input, append failure ledger, surface peer-exposed member inputs, and resolve operator create mob admission and profile mutation admission verdicts the tool surface mirrors",
                ),
                scenario(
                    "membership-admission-respawn-reconcile-rebind-timeout",
                    "probe member admission duplicate or admitted, compute respawn generation, reconcile desired members to spawn retain or retire emitting member spawn required member retain required and member retire required, set and observe external member rebind capability available or unavailable, classify turn timeout disposition detached canceled or retryable, and seed orphan budget",
                ),
                scenario(
                    "flow-fault-topology-supervisor-escalation",
                    "classify step output fault retry or terminal malformed json into a step fault disposition, evaluate topology edge rule allow deny or default into a policy decision verdict resolved, and escalate to supervisor target found with eligible supervisor identity or no eligible target emitting supervisor escalation requested or failed",
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
            )],
            &[
                scenario(
                    "schedule_pause_resume_delete",
                    "schedule transitions through create, pause, resume, and delete while advancing revision",
                ),
                scenario(
                    "schedule_revision_and_planning",
                    "active or paused schedules revise, update planning config active or paused, record planning windows, sync target snapshots for materialized session bindings, confirm superseded occurrences, supersede pending occurrences, maintain positive revision, and require occurrence progress for planning cursor",
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
            )],
            &[
                scenario(
                    "occurrence_start_complete_fail",
                    "occurrence transitions through pending, running, and terminal lifecycle states",
                ),
                scenario(
                    "occurrence_claim_dispatch_completion",
                    "plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, resolve runtime completion outcome, and record claimed/dispatch/awaiting/completed effects",
                ),
                scenario(
                    "occurrence_terminal_classification",
                    "skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes",
                ),
                scenario(
                    "occurrence_lease_recovery",
                    "classify due no action, due claim eligible, due misfire required, due lease expired, and lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery",
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
                ),
                machine_anchor(
                    "oauth_flow_handle",
                    "AuthMachine",
                    "meerkat-runtime/src/handles/oauth_flow.rs",
                    "per-binding AuthMachine-owned OAuth browser and device flow lifecycle authority for admit, verify, begin poll, finish poll, consume, expire, valid, expiring, expired, refreshing, and reauth required phases",
                ),
            ],
            &[
                scenario(
                    "acquire_expire_refresh_complete",
                    "lease transitions through valid, expiring, expired, refreshing, and back to valid on successful refresh",
                ),
                scenario(
                    "reauth_release_and_publication",
                    "reauth required from valid/expiring/expired/refreshing, observe credential freshness for released state, release lease, emit lifecycle event, and wake refresh loop publication",
                ),
                scenario(
                    "oauth_browser_flow_lifecycle",
                    "OAuth browser flow admit, verify, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority",
                ),
                scenario(
                    "oauth_device_flow_lifecycle",
                    "OAuth device flow admit, verify, begin poll, finish poll, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority",
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
            )],
            &[
                scenario(
                    "approval_request_pending",
                    "CreateRejectedEmptyAllowedDecisions, CreateRejectedAlreadyExists, and CreatePending keep request creation and Pending status projection under ApprovalStatusResolved or ApprovalLifecycleRejected",
                ),
                scenario(
                    "approval_decide_terminal",
                    "DecideRejectedMissing, DecideRejectedExpired, DecideRejectedAlreadyDecided, DecideRejectedApproveNotAllowed, DecideRejectedDenyNotAllowed, DecideApprove, and DecideDeny move Pending approvals to Approved or Denied only when generated allowed-decision state admits the terminal decision",
                ),
                scenario(
                    "approval_expiry_feedback",
                    "ObserveExpiryRejectedMissing, ObserveExpiryExpiresPending, ObserveExpiryPendingNoop, ObserveExpiryApprovedNoop, ObserveExpiryDeniedNoop, ObserveExpiryExpiredNoop, and ObserveExpiryCancelledNoop consume typed time observation and emit Expired or unchanged status without handwritten status mutation",
                ),
                scenario(
                    "approval_restore_consistency",
                    "RestoreRejectedDuplicate, RestoreRejectedEmptyAllowedDecisions, RestorePending, RestoreExpired, RestoreCancelled, RestoreApproved, RestoreDenied, and RestoreRejectedInvalidRecord validate persisted status, decision audit consistency, and allowed-decision compatibility before rehydrating approval lifecycle truth",
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
            )],
            &[
                scenario(
                    "session_first_turn_pending_consume",
                    "MarkSessionInitialTurnPendingInactiveOrPending, MarkSessionInitialTurnPendingConsumed, StartSessionInitialTurnPending, StartSessionInitialTurnInactive, StartSessionInitialTurnConsumed, ConsumeSessionDeferredInputsPending, ConsumeSessionDeferredInputsInactive, and ConsumeSessionDeferredInputsConsumed own the per-session first-turn phase registry and emit SessionFirstTurnPhaseResolved without handwritten phase mutation",
                ),
                scenario(
                    "session_initial_inputs_stage",
                    "StageSessionInitialPromptStore, StageSessionInitialPromptClear, StageSessionToolResults, ResolveSessionFirstTurnOverridesAllowed, and ResolveSessionFirstTurnOverridesDenied resolve initial-prompt and tool-results staging plus build-override legality from the machine-owned phase map under SessionInitialPromptStageResolved, SessionToolResultsStageResolved, and SessionFirstTurnOverridesResolved",
                ),
                scenario(
                    "session_first_turn_restore_recover",
                    "RestoreSessionConsumedInputs, RestoreSessionConsumedInputsNoPhaseRollback, and RecoverSessionFirstTurnPhase rehydrate the per-session phase and presence/count registry from consumed-input rollback and durable snapshots under SessionConsumedInputsRestoreResolved and SessionFirstTurnPhaseRecovered",
                ),
                scenario(
                    "session_system_context_append_resolve",
                    "ResolveSystemContextAppendEmpty, ResolveSystemContextAppendConflict, ResolveSystemContextAppendDuplicate, and ResolveSystemContextAppendNew decide the runtime system-context append disposition from typed key-present/matches/conflicts observations under SystemContextAppendResolved without the shell deciding",
                ),
                scenario(
                    "session_system_context_apply_discard",
                    "ResolveSystemContextPendingApplyItemRuntimeSteer, ResolveSystemContextPendingApplyItemNormal, ResolveSystemContextSteerCleanupItemRuntimeSteer, and ResolveSystemContextSteerCleanupItemNormal decide per-append apply/discard from the typed SystemContextSource marker (not a runtime:steer string prefix) under SystemContextPendingApplyItemResolved and SystemContextSteerCleanupItemResolved",
                ),
                scenario(
                    "session_system_context_snapshot_restore",
                    "RestoreSystemContextSnapshot authorizes a durable system-context snapshot only when active keys have known pending-or-seen entries and seen keys match known appends under SystemContextSnapshotRestoreAuthorized",
                ),
                scenario(
                    "session_realtime_transcript_event_resolve",
                    "ResolveRealtimeItemObservedDiscardedAssistant, ResolveRealtimeItemObservedPresent, ResolveRealtimeItemSkipped, ResolveRealtimeUserTranscriptFinalEmpty, ResolveRealtimeUserTranscriptFinalStore, ResolveRealtimeUserTranscriptFinalReplayOrConflict, ResolveRealtimeAssistantDeltaInvalidOrDuplicate, ResolveRealtimeAssistantDeltaDiscarded, ResolveRealtimeAssistantDeltaLaneConflict, ResolveRealtimeAssistantDeltaAccepted, ResolveRealtimeAssistantReplacementInvalid, ResolveRealtimeAssistantReplacementDiscarded, ResolveRealtimeAssistantReplacementLocked, ResolveRealtimeAssistantReplacementLaneConflict, ResolveRealtimeAssistantReplacementAccepted, ResolveRealtimeAssistantTurnCompletedInvalid, ResolveRealtimeAssistantTurnCompletedDiscard, ResolveRealtimeAssistantTurnCompletedToolUse, ResolveRealtimeAssistantTurnInterruptedInvalid, and ResolveRealtimeAssistantTurnInterruptedValid resolve the realtime-transcript action vector from typed raw observations (set membership, segment concat emptiness, lane, completion) under RealtimeTranscriptEventResolved without the shell deciding; the shell mirrors the emitted action vector onto its bulky SessionRealtimeTranscriptState",
                ),
                scenario(
                    "session_realtime_transcript_materialize_and_restore",
                    "ResolveRealtimeAssistantTurnCompletedRecord, ResolveRealtimeMaterializeAlreadyDone, ResolveRealtimeMaterializeWaitForPredecessor, ResolveRealtimeMaterializeSkipped, ResolveRealtimeMaterializeWaitForReadyText, ResolveRealtimeMaterializeUser, ResolveRealtimeMaterializeAssistant, ResolveRealtimeMaterializeAssistantMissingCompletion, and AuthorizeRestoreRealtimeTranscriptState resolve the per-item materialize verdict and durable snapshot-restore legality under RealtimeMaterializeCandidateResolved and RealtimeTranscriptSnapshotRestoreAuthorized; the shell performs only the topological ordering and message assembly",
                ),
                scenario(
                    "session_durable_config_authorize_restore",
                    "AuthorizeSessionMetadataPersist, AuthorizeSessionBuildStatePersist, RestoreSessionBuildState, and AuthorizeSystemPromptMutation decide durable-config persist/restore/system-prompt admission from typed presence/count/kind observations the meerkat-core shell extracts from SessionMetadata and SessionBuildState under SessionMetadataPersistAuthorized, SessionBuildStatePersistAuthorized, SessionBuildStateRestoreAuthorized, and SystemPromptMutationAuthorized; a rejected request matches no transition and surfaces as Err, and the shell mirrors the verdict and passes the original typed value through unchanged",
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
            )],
            &[
                scenario(
                    "turn_admission_claim_run_finalize",
                    "ClaimTurn, BeginTurn, ResolveTurn, FinalizeTurnToIdle, FinalizeTurnToShutdown, AbortClaim, and ProjectTurnAdmission own the Idle/Admitted/Running/Completing/ShuttingDown turn-admission phase and emit TurnAdmissionProjected without handwritten phase mutation",
                ),
                scenario(
                    "turn_admission_interrupt_and_shutdown",
                    "RequestInterruptAdmittedFirst, RequestInterruptAdmittedDuplicate, RequestInterruptRunningFirst, RequestInterruptRunningDuplicate, RequestShutdownImmediateIdle, RequestShutdownImmediateAdmitted, RequestShutdownDeferredRunning, RequestShutdownDeferredCompleting, and RequestShutdownAlreadyShuttingDown resolve interrupt wake feedback and immediate-or-deferred shutdown under TurnInterruptRequested and TurnAdmissionProjected while preserving the shutdown-not-active invariant",
                ),
                scenario(
                    "turn_admission_dispatch_and_boundary_cancel",
                    "AuthorizeStartTurnDispatchAdmitted, AuthorizeStartTurnDispatchShuttingDown, AuthorizeCancelAfterBoundaryAdmitted, and AuthorizeCancelAfterBoundaryRunning resolve start-turn dispatch authorization and boundary-cancel legality from the admission phase under StartTurnDispatchResolved and CancelAfterBoundaryAuthorized",
                ),
                scenario(
                    "turn_admission_start_turn_disposition",
                    "ResolveDispositionContentTurn, ResolveDispositionResumePendingWithBoundary, ResolveDispositionResumePendingWithoutBoundary, ResolveDispositionDirectPrompt, ResolveDispositionDirectPending, ResolveDispositionDirectNoPending, and ResolveLastStartTurnPublicTerminalNoPending resolve the start-turn disposition from the execution kind, prompt content observation, and the SessionDocumentMachine-emitted PendingContinuationDisposition under StartTurnDispositionResolved and StartTurnPublicTerminalResolved; the shell mirrors the disposition and never decides",
                ),
                scenario(
                    "turn_admission_runtime_keep_alive",
                    "ResolveRuntimeKeepAliveEnable and ResolveRuntimeKeepAlivePreserve resolve runtime keep-alive persistence from the typed keep-alive-policy-present observation under RuntimeKeepAliveResolved",
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
            )],
            &[
                scenario(
                    "workgraph_create_update_ready_claim",
                    "CreateDefaultOrOpen, CreateRequestedBlocked, CreateOpen, CreateBlocked, UpdateOpen, UpdateInProgress, UpdateBlocked, RefreshEligibilityOpen, RefreshEligibilityInProgress, RefreshEligibilityBlocked, Created, Updated, ClaimOpen, ClaimExpiredInProgress, Claimed, due eligibility, blocker satisfaction, public create status defaulting, create status admission classifies open and blocked as admissible creation states and denies the rest, and CAS revision",
                ),
                scenario(
                    "workgraph_claim_release_recovery",
                    "only one active claim exists, ReleaseInProgress, Released, expired leases become recoverable through machine-approved claim, claim_only_in_progress, blocked_has_no_claim, and terminal_has_no_claim",
                ),
                scenario(
                    "workgraph_block_close_evidence",
                    "BlockOpen, BlockInProgress, BlockBlocked, Blocked, CloseOpenDefaultOrCompleted, CloseInProgressDefaultOrCompleted, CloseBlockedDefaultOrCompleted, CloseOpenRequestedCancelled, CloseInProgressRequestedCancelled, CloseBlockedRequestedCancelled, CloseOpenRequestedFailed, CloseInProgressRequestedFailed, CloseBlockedRequestedFailed, CloseOpenCompleted, CloseInProgressCompleted, CloseBlockedCompleted, CloseOpenCancelled, CloseInProgressCancelled, CloseBlockedCancelled, CloseOpenFailed, CloseInProgressFailed, CloseBlockedFailed, Closed, AddEvidenceOpen, AddEvidenceInProgress, AddEvidenceBlocked, AddEvidenceCompleted, AddEvidenceCancelled, AddEvidenceFailed, EvidenceAdded, public close status defaulting, public confirmation admission admits only a self-attested completion policy and denies every other policy as requiring trusted host, absent_has_zero_revision, live_has_positive_revision, and terminal_has_terminal_time",
                ),
                scenario(
                    "workgraph_topology_legality",
                    "ClassifyBlockerSatisfiedCompleted, ClassifyBlockerUnsatisfiedAbsent, ClassifyBlockerUnsatisfiedOpen, ClassifyBlockerUnsatisfiedInProgress, ClassifyBlockerUnsatisfiedBlocked, ClassifyBlockerUnsatisfiedCancelled, ClassifyBlockerUnsatisfiedFailed, ClassifyTerminalityAbsent, ClassifyTerminalityOpen, ClassifyTerminalityInProgress, ClassifyTerminalityBlocked, ClassifyTerminalityCompleted, ClassifyTerminalityCancelled, ClassifyTerminalityFailed, BlockerSatisfied, BlockerUnsatisfied, LifecycleTerminal, LifecycleNonTerminal, ValidateLink, and LinkValidated reject missing endpoints, self edges, duplicate edges, dependency cycles, and unsatisfied blockers without adding a separate topology machine",
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
            )],
            &[scenario(
                "work_attention_pause_resume_stop",
                "PauseActive, PausePaused, ResumePaused, SupersedeActive, SupersedePaused, StopActive, StopPaused, AttentionPaused, AttentionResumed, AttentionSuperseded, AttentionStopped, live_has_no_terminal_time, paused_has_pause_state, superseded_records_successor, timed pause eligibility, CAS revision, and terminal work item attention stop stay under WorkAttentionLifecycleMachine authority",
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
                ),
                machine_anchor(
                    "meerkat_runtime_entry",
                    "MeerkatMachine",
                    "meerkat-runtime/src/meerkat_machine/mod.rs",
                    "MeerkatMachine command authority consuming runtime binding, admitted work, cancellation, lifecycle, terminal, and peer ingress seam traffic",
                ),
            ],
            &[
                scenario(
                    "binding_round_trip",
                    "mob runtime binding request becomes a Meerkat binding and feeds readiness back to Mob",
                ),
                scenario(
                    "work_round_trip",
                    "mob submits work into Meerkat and observes terminal work outcomes back across the seam",
                ),
                scenario(
                    "peer-ingress-and-cancellation",
                    "peer input admission and cancellation requests cross the MobMachine to MeerkatMachine seam with explicit lifecycle notice feedback",
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
                ),
                route_anchor(
                    "schedule_store",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-schedule/src/store.rs",
                    "schedule store contract precursor for transactional claim, supersede persistence, occurrence progress, and revision-aware planning cursor updates",
                ),
                route_anchor(
                    "schedule_bundle_schema",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule bundle composition",
                ),
            ],
            &[
                scenario(
                    "revision-supersede-route",
                    "revision-affecting schedule updates supersede pending future occurrences through the explicit route",
                ),
                scenario(
                    "pause-resume-without-revision",
                    "pause and resume leave schedule revision unchanged while preserving typed ownership",
                ),
                scenario(
                    "rolling-planning-occurrence-materialization",
                    "rolling planning records a planning window and materializes or supersedes pending occurrences through revision-aware schedule routes",
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
                ),
                route_anchor(
                    "runtime_delivery_precursor",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-rpc/src/session_runtime.rs",
                    "runtime-owned prompt/event delivery precursor that scheduling must hand off into for dispatch, completion, failure, and lease recovery",
                ),
                route_anchor(
                    "schedule_runtime_bundle_schema",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule runtime bundle composition",
                ),
            ],
            &[
                scenario(
                    "runtime-delivery-feedback",
                    "DispatchToRuntime is realized by runtime-owned delivery and closed by typed completion feedback",
                ),
                scenario(
                    "runtime-lease-expiry",
                    "runtime owner fairness still allows lease expiry to return a stuck occurrence to claimable",
                ),
                scenario(
                    "runtime-revision-supersede",
                    "schedule revision supersede enters occurrence authority before runtime handoff so stale pending work is cancelled explicitly",
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
                ),
                route_anchor(
                    "mob_delivery_precursor",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-mob-mcp/src/lib.rs",
                    "mob-owned action delivery precursor that scheduling must hand off into for dispatch, completion, target materialization failure, and lease recovery",
                ),
                route_anchor(
                    "schedule_mob_bundle_schema",
                    "revision_supersede_enters_occurrence_authority",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule mob bundle composition",
                ),
            ],
            &[
                scenario(
                    "mob-delivery-feedback",
                    "DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback",
                ),
                scenario(
                    "materialization-failure-classification",
                    "mob-side delivery failure preserves explicit TargetMaterializationFailed classification",
                ),
                scenario(
                    "mob-revision-supersede",
                    "schedule revision supersede enters occurrence authority before mob handoff so stale pending work is cancelled explicitly",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &auth_lease_bundle_composition(),
            &[
                machine_anchor(
                    "auth_lease_handle",
                    "AuthMachine",
                    "meerkat-runtime/src/handles/auth_lease.rs",
                    "runtime auth lease owner consumes canonical AuthMachine lifecycle acquire, refresh, reauth, release, wake, and publication events",
                ),
                machine_anchor(
                    "auth_lease_bundle_schema",
                    "AuthMachine",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal AuthMachine lifecycle publication handoff composition",
                ),
            ],
            &[scenario(
                "auth-lease-lifecycle-publication",
                "AuthMachine acquire, refresh, reauth, release, wake, and lifecycle transitions publish through the explicit auth lease handoff protocol",
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
                ),
                route_anchor(
                    "workgraph_attention_bundle_schema",
                    "work_item_close_stops_attention",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal WorkGraph item closure to WorkAttention stop composition",
                ),
            ],
            &[scenario(
                "close-stops-attention",
                "terminal WorkGraph item closure routes to WorkAttention Stop so live goal attention bindings cannot survive their target item",
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
            .map(|transition| SemanticCoverageEntry {
                name: transition.name.as_str().to_owned(),
                anchor_ids: semantic_anchor_ids(transition.name.as_str(), code_anchors),
                scenario_ids: semantic_scenario_ids(transition.name.as_str(), scenarios),
            })
            .collect(),
        effect_coverage: schema
            .effects
            .variants
            .iter()
            .map(|effect| SemanticCoverageEntry {
                name: effect.name.as_str().to_owned(),
                anchor_ids: semantic_anchor_ids(effect.name.as_str(), code_anchors),
                scenario_ids: semantic_scenario_ids(effect.name.as_str(), scenarios),
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| SemanticCoverageEntry {
                name: invariant.name.clone(),
                anchor_ids: semantic_anchor_ids(&invariant.name, code_anchors),
                scenario_ids: semantic_scenario_ids(&invariant.name, scenarios),
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
            .map(|route| SemanticCoverageEntry {
                name: route.name.as_str().to_owned(),
                anchor_ids: semantic_anchor_ids(route.name.as_str(), code_anchors),
                scenario_ids: semantic_scenario_ids(route.name.as_str(), scenarios),
            })
            .collect(),
        scheduler_rule_coverage: schema
            .scheduler_rules
            .iter()
            .map(|rule| SemanticCoverageEntry {
                name: format!("{rule:?}"),
                anchor_ids: semantic_anchor_ids(&format!("{rule:?}"), code_anchors),
                scenario_ids: semantic_scenario_ids(&format!("{rule:?}"), scenarios),
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| SemanticCoverageEntry {
                name: invariant.name.clone(),
                anchor_ids: semantic_anchor_ids(&invariant.name, code_anchors),
                scenario_ids: semantic_scenario_ids(&invariant.name, scenarios),
            })
            .collect(),
    }
}

fn semantic_anchor_ids(name: &str, anchors: &[CoverageAnchor]) -> Vec<String> {
    semantic_ids(
        name,
        anchors,
        |anchor| anchor.id.as_str(),
        |anchor| anchor.note.as_str(),
    )
}

fn semantic_scenario_ids(name: &str, scenarios: &[ScenarioCoverage]) -> Vec<String> {
    semantic_ids(
        name,
        scenarios,
        |scenario| scenario.id.as_str(),
        |scenario| scenario.summary.as_str(),
    )
}

/// Associate a semantic element with the anchors/scenarios whose description
/// contains EVERY semantic token of the element's name.
///
/// Full containment is deliberately strict: the old max-partial-score rule
/// let an element lexically "win" the wrong anchor through coincidental
/// overlap of a minority of tokens (or ties between unrelated anchors). Under
/// full containment an anchor only claims an element when its note actually
/// enumerates all of the element's name tokens; an element nothing fully
/// describes is reported UNCLAIMED (empty ids) rather than mis-attributed —
/// absence of coverage is honest, a wrong anchor is laundered truth.
fn semantic_ids<T>(
    name: &str,
    items: &[T],
    id: impl Fn(&T) -> &str,
    description: impl Fn(&T) -> &str,
) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }

    let tokens = semantic_tokens(name);
    if tokens.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .filter(|item| {
            let haystack = format!("{} {}", id(item), description(item)).to_ascii_lowercase();
            tokens.iter().all(|token| haystack.contains(token.as_str()))
        })
        .map(|item| id(item).to_owned())
        .collect::<Vec<_>>()
}

fn semantic_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == ' ' || ch == '(' || ch == ')' || ch == ',' {
            if current.len() >= 3 {
                tokens.push(current.to_ascii_lowercase());
            }
            current.clear();
            previous_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower && current.len() >= 3 {
            tokens.push(current.to_ascii_lowercase());
            current.clear();
        }
        previous_lower = ch.is_ascii_lowercase();
        current.push(ch);
    }
    if current.len() >= 3 {
        tokens.push(current.to_ascii_lowercase());
    }
    tokens
}

/// Construct an anchor whose schema target is a canonical machine.
///
/// `machine` must name a machine in the canonical schema set; the `xtask`
/// coverage validator resolves it and fails closed otherwise.
fn machine_anchor(id: &str, machine: &str, symbol: &str, note: &str) -> CoverageAnchor {
    CoverageAnchor {
        id: id.into(),
        symbol: SymbolRef(symbol.into()),
        target: CoverageSchemaTarget::Machine(
            MachineId::parse(machine).expect("valid machine slug"),
        ),
        note: note.into(),
    }
}

/// Construct an anchor whose schema target is a declared composition route.
///
/// `route` must name a route declared by the owning composition; the `xtask`
/// coverage validator resolves it and fails closed otherwise.
fn route_anchor(id: &str, route: &str, symbol: &str, note: &str) -> CoverageAnchor {
    CoverageAnchor {
        id: id.into(),
        symbol: SymbolRef(symbol.into()),
        target: CoverageSchemaTarget::Route(RouteId::parse(route).expect("valid route slug")),
        note: note.into(),
    }
}

fn scenario(id: &str, summary: &str) -> ScenarioCoverage {
    ScenarioCoverage {
        id: id.into(),
        summary: summary.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_ids_require_full_token_containment_never_coincidental_overlap() {
        // Row #15 gate: under the old max-partial-score rule, an element whose
        // tokens only PARTIALLY overlap an unrelated anchor's note could
        // lexically "win" that anchor (wrong attribution). Full containment
        // claims an element only when every token is described.
        let anchors = vec![
            machine_anchor(
                "right_anchor",
                "MeerkatMachine",
                "meerkat-runtime/src/meerkat_machine/mod.rs",
                "register session and prepare bindings ownership",
            ),
            machine_anchor(
                "wrong_anchor",
                "MeerkatMachine",
                "meerkat/src/meerkat_machine.rs",
                "register snapshot facade",
            ),
        ];

        // Both anchors mention "register"; only the first also mentions
        // "session". Max-partial-score would have tied or mis-attributed;
        // full containment selects exactly the fully-describing anchor.
        assert_eq!(
            semantic_anchor_ids("RegisterSession", &anchors),
            vec!["right_anchor".to_owned()],
        );

        // An element nothing fully describes is honestly UNCLAIMED, not
        // attributed to whichever anchor coincidentally shares a token.
        assert!(
            semantic_anchor_ids("RegisterRealtimeEndpoint", &anchors).is_empty(),
            "partially-overlapping anchors must not be claimed"
        );
    }

    #[test]
    fn canonical_manifests_construct_with_full_containment_matching() {
        // The canonical manifests must still build (anchor notes enumerate
        // the semantic vocabulary); unclaimed entries are permitted, wrong
        // claims are not.
        let manifests = canonical_machine_coverage_manifests();
        assert!(!manifests.is_empty());
        let compositions = canonical_composition_coverage_manifests();
        assert!(!compositions.is_empty());
    }
}

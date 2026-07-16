//! Controlling-side upcall responder (design-upcalls §3/§4, DEC-U3).
//!
//! A dedicated task, spawned by the mob builder next to the actor, that
//! serves member-originated `MemberOperatorRequest` commands arriving at the
//! supervisor bridge runtime's inbox. Serving order per delivery (§3.5,
//! duplicates included):
//!
//! 1. Machine admission FIRST — `ResolveMemberOperatorAdmission` through the
//!    `MobHandle` verb (a duplicate after `RevokeHost` rejects, never
//!    replays). No authority claim rides the wire (R6).
//! 2. Actor-authorized stale-ledger prune — reclaim only rows whose full
//!    requester residency is absent from the actor's current MobMachine
//!    snapshot. Current Pending and terminal replay rows are never evicted.
//! 3. Durable request begin — atomically insert `Pending` under
//!    `(mob, requester identity, requester host, host binding generation,
//!    member session, requester generation, requester fence, request_id)`
//!    with the typed-operation digest. A terminal row replays
//!    verbatim; a different digest conflicts without execution; a recovered
//!    `Pending` becomes a stable indeterminate terminal and is NEVER
//!    re-executed.
//! 4. Recorded-authority execution — the durable capability-FACTS record
//!    (written by the spec compiler under the spawn transition witness) is
//!    read and RE-MINTED into a sealed context through the generated ladder
//!    (ADJ-15; a persisted context can never be rehydrated). Execution then
//!    re-runs the exact per-op machine admissions the local operator
//!    dispatcher runs.
//! 5. Reply — statuses are minted ONLY through the `host_reply` constructor
//!    seam (this file is bridge-classifier-listed and never names a
//!    `ResponseStatus` variant). Admitted replies stage a one-shot
//!    `DeclaredReplyEndpoint` from machine-recorded
//!    `member_peer_endpoints[identity]` (ADJ-16); rejected unknown-sender
//!    replies stay best-effort on the ingress route (documented limitation).
//!
//! O3 presence-fact discipline (T-F4): for upcall spawns the
//! `tool_access_policy_present` observation fed to the machine is the WIRE
//! fact `requested_tool_access_policy_present` — never derived from the
//! resolved policy — while the resolved policy flows into the spawn spec as
//! an explicit AllowList/DenyList. This is why spawn ops run the handle
//! admission seams directly instead of the JSON-args dispatcher path (which
//! would re-derive presence from the rebuilt args — exactly the laundering
//! the pin forbids).

use std::sync::Arc;

use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, PeerId, PeerRoute, SendError};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate};
use meerkat_core::service::{MobToolAuthorityContext, MobToolCallerProvenance};
use meerkat_runtime::mob_operator_authority::{
    MobOperatorAuthorityFacts, mint_mob_operator_authority_from_facts,
};
use serde_json::json;
use tokio::sync::oneshot;

use super::bridge_protocol::{
    BridgeCommand, BridgeCommandDecodeError, BridgeMemberOperatorPayload, BridgeRejectionCause,
    BridgeReply, MemberOperatorOp, MemberOperatorOutcome, MemberOperatorReply,
    MemberOperatorSpawnSpec, SUPERVISOR_BRIDGE_INTENT, decode_bridge_command,
};
use super::handle::{MemberOperatorAdmissionRequest, MemberOperatorAdmissionVerdict};
use super::host_reply::HostBridgeReply;
use super::member_upcall::{
    TOOL_FORCE_CANCEL_MEMBER, TOOL_LIST_MEMBERS, TOOL_MEMBER_STATUS, TOOL_MOB_CANCEL_FLOW,
    TOOL_MOB_FLOW_STATUS, TOOL_MOB_LIST_FLOWS, TOOL_MOB_RUN_FLOW, TOOL_RETIRE_MEMBER,
    TOOL_SPAWN_MANY_MEMBERS, TOOL_SPAWN_MEMBER, TOOL_UNWIRE_MEMBERS, TOOL_WIRE_MEMBERS,
    UpcallToolOutcome,
};
use super::tools::MobOperatorToolDispatcher;
use super::{
    MobHandle, SpawnMemberAdmission, SpawnMemberAdmissionObservations, SpawnMemberSpec,
    SpawnToolAdmission,
};
use crate::MobError;
use crate::ids::AgentIdentity;
use crate::machines::mob_machine as mob_dsl;
use crate::store::{
    MobMemberOperatorAuthorityRecord, MobMemberOperatorRequestBegin, MobMemberOperatorRequestKey,
    MobMemberOperatorRequestRecord, MobMemberOperatorRequestState, MobRuntimeMetadataStore,
    MobStoreError, member_operator_op_digest,
};

// ---------------------------------------------------------------------------
// ADJ-14 — MemberOperatorRejectKind → wire cause mapping
// ---------------------------------------------------------------------------

/// The frozen wire cause vocabulary has no member-operator-specific reject;
/// the machine kind rides the reason string verbatim. `ScopeDenied` is
/// deliberately NOT used (phase-5 principal-lane vocabulary).
pub(crate) fn map_member_operator_reject_kind(
    kind: mob_dsl::MemberOperatorRejectKind,
) -> BridgeRejectionCause {
    match kind {
        mob_dsl::MemberOperatorRejectKind::UnknownIdentity
        | mob_dsl::MemberOperatorRejectKind::SenderKeyMismatch => {
            BridgeRejectionCause::SenderMismatch
        }
        mob_dsl::MemberOperatorRejectKind::StaleGeneration
        | mob_dsl::MemberOperatorRejectKind::StaleFence
        | mob_dsl::MemberOperatorRejectKind::StaleSession
        | mob_dsl::MemberOperatorRejectKind::StaleHost
        | mob_dsl::MemberOperatorRejectKind::StaleHostBindingGeneration => {
            BridgeRejectionCause::StaleFence
        }
        mob_dsl::MemberOperatorRejectKind::NoPlacement => BridgeRejectionCause::Unavailable,
        mob_dsl::MemberOperatorRejectKind::HostRevoked => BridgeRejectionCause::NotBound,
    }
}

/// Reason string carrying the machine kind name verbatim (T-16 pin).
pub(crate) fn member_operator_reject_reason(kind: mob_dsl::MemberOperatorRejectKind) -> String {
    format!("member operator admission rejected: {kind:?}")
}

// ---------------------------------------------------------------------------
// Durable request/reply protocol
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Serve pipeline (§3.5 order) over narrow seams
// ---------------------------------------------------------------------------

/// Fault classes on the execution seam. Once Pending exists, even a
/// pre-effect failure is terminalized rather than deleting the request key.
#[derive(Debug)]
pub(crate) enum UpcallExecuteError {
    /// The generated authority ladder rejected the facts re-mint.
    AuthorityRemint(String),
}

/// Narrow seams the serve pipeline runs over. Production wires the
/// `MobHandle` + metadata store ([`MobUpcallSeams`]); unit rows use spies to
/// pin admission-before-authority and durable replay semantics (T-6..T-9).
#[async_trait::async_trait]
pub(crate) trait UpcallServeSeams: Send + Sync {
    /// Machine admission (`ResolveMemberOperatorAdmission` via the actor).
    async fn admit(
        &self,
        request: &MemberOperatorAdmissionRequest,
    ) -> Result<MemberOperatorAdmissionVerdict, MobError>;

    /// Actor-authorized stale-ledger reclamation. Runs only after machine
    /// admission and before durable begin, so superseded incarnations free
    /// global capacity while current Pending/terminal replay rows survive.
    async fn prune_stale_requests(&self) -> Result<u64, MobError>;

    /// Read the durable operator-authority FACTS record for an ADMITTED
    /// identity.
    async fn load_authority_record(
        &self,
        agent_identity: &AgentIdentity,
        requester_generation: u64,
    ) -> Result<Option<MobMemberOperatorAuthorityRecord>, MobStoreError>;

    /// Atomically persist Pending before execution, or return the existing
    /// durable row for replay/conflict/recovery handling.
    async fn begin_request(
        &self,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError>;

    /// Immutable Pending -> Terminal CAS.
    async fn terminalize_request(
        &self,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError>;

    /// Reload after a CAS race so the durable winner is the only reply sent.
    async fn load_request(
        &self,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError>;

    /// Re-mint sealed authority from the record and execute the op through
    /// the same per-op machine admissions the local dispatcher runs.
    async fn execute(
        &self,
        pending: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorAuthorityRecord,
        request_id: &str,
        op: MemberOperatorOp,
    ) -> Result<UpcallToolOutcome, UpcallExecuteError>;
}

fn rejected_reply(
    request_id: &str,
    cause: BridgeRejectionCause,
    reason: String,
) -> MemberOperatorReply {
    MemberOperatorReply {
        request_id: request_id.to_string(),
        outcome: MemberOperatorOutcome::Rejected { cause, reason },
    }
}

fn completed_reply(
    request_id: &str,
    outcome: &UpcallToolOutcome,
) -> Result<MemberOperatorReply, MobStoreError> {
    let result_value = serde_json::to_value(outcome).map_err(|error| {
        MobStoreError::Serialization(format!(
            "upcall outcome envelope serialization failed: {error}"
        ))
    })?;
    Ok(MemberOperatorReply {
        request_id: request_id.to_string(),
        outcome: MemberOperatorOutcome::Completed {
            result: super::bridge_protocol::WireOpaqueJson::from_value(&result_value),
        },
    })
}

/// Preserve the actor's typed mob error family across the member-upcall tool
/// envelope. The member side reconstructs `ExecutionFailedWithData`, so a
/// stale execution fence is machine-readable (`STALE_FENCE`) rather than only
/// discoverable by parsing the human message.
fn member_operator_execution_error_outcome(error: MobError) -> UpcallToolOutcome {
    let message = error.to_string();
    let tool_error = match error.wire_detail() {
        Some(detail) => match detail.detail_value() {
            Ok(details) => ToolError::execution_failed_with_data(
                message,
                json!({
                    "code": detail.code(),
                    "details": details,
                }),
            ),
            Err(_) => ToolError::execution_failed(message),
        },
        None => ToolError::execution_failed(message),
    };
    UpcallToolOutcome::from_tool_error(&tool_error)
}

async fn terminalize_or_load_winner(
    seams: &dyn UpcallServeSeams,
    pending: &MobMemberOperatorRequestRecord,
    reply: MemberOperatorReply,
) -> Result<MemberOperatorReply, MobStoreError> {
    let terminal = pending.terminal(reply.clone())?;
    if seams.terminalize_request(pending, &terminal).await? {
        return Ok(reply);
    }
    let Some(winner) = seams.load_request(pending).await? else {
        return Err(MobStoreError::CasConflict(format!(
            "member operator request '{}' disappeared during terminal persistence",
            pending.request_id
        )));
    };
    if winner.op_digest != pending.op_digest {
        return Err(MobStoreError::Internal(format!(
            "member operator request '{}' changed operation digest during terminal persistence",
            pending.request_id
        )));
    }
    winner.terminal_reply().cloned().ok_or_else(|| {
        MobStoreError::CasConflict(format!(
            "member operator request '{}' remained pending after terminal CAS loss",
            pending.request_id
        ))
    })
}

fn volatile_begin_failure_reply(request_id: &str, error: &MobStoreError) -> MemberOperatorReply {
    rejected_reply(
        request_id,
        BridgeRejectionCause::Unavailable,
        format!(
            "member operator pending begin persistence failed before execution; no effect was run and retry is safe: {error}"
        ),
    )
}

fn volatile_post_pending_persistence_failure_reply(
    request_id: &str,
    context: &str,
    error: &MobStoreError,
) -> MemberOperatorReply {
    rejected_reply(
        request_id,
        BridgeRejectionCause::Unavailable,
        format!(
            "member operator {context} persistence failed; the operation will not be re-executed: {error}"
        ),
    )
}

async fn terminalize_indeterminate(
    seams: &dyn UpcallServeSeams,
    pending: &MobMemberOperatorRequestRecord,
) -> Result<MemberOperatorReply, MobStoreError> {
    let reply = completed_reply(
        &pending.request_id,
        &UpcallToolOutcome::indeterminate(pending.op_digest.clone()),
    )?;
    terminalize_or_load_winner(seams, pending, reply).await
}

/// Serve one delivered `MemberOperatorRequest` (§3.5 order: machine
/// admission FIRST on every delivery, then actor-authorized stale-row prune,
/// generation-pinned durable begin, and recorded-authority execution).
pub(crate) async fn serve_member_operator_request(
    seams: &dyn UpcallServeSeams,
    payload: &BridgeMemberOperatorPayload,
    sender_peer_id: PeerId,
) -> MemberOperatorReply {
    let request_key = MobMemberOperatorRequestKey::new(
        payload.agent_identity.clone(),
        payload.requester_generation,
        payload.requester_fence_token,
        payload.requester_host_id.clone(),
        payload.requester_host_binding_generation,
        payload.requester_member_session_id.clone(),
        payload.request_id.clone(),
    );
    let requester = AgentIdentity::from(request_key.agent_identity.as_str());
    let admission_request = MemberOperatorAdmissionRequest {
        request_key: request_key.clone(),
        sender_peer_id,
    };

    let verdict = match seams.admit(&admission_request).await {
        Ok(verdict) => verdict,
        Err(error) => {
            return rejected_reply(
                &payload.request_id,
                BridgeRejectionCause::Unavailable,
                format!("member operator admission unavailable: {error}"),
            );
        }
    };
    if let MemberOperatorAdmissionVerdict::Rejected(kind) = verdict {
        return rejected_reply(
            &payload.request_id,
            map_member_operator_reject_kind(kind),
            member_operator_reject_reason(kind),
        );
    }

    if let Err(error) = seams.prune_stale_requests().await {
        return rejected_reply(
            &payload.request_id,
            BridgeRejectionCause::Unavailable,
            format!("member operator stale-ledger pruning failed before durable begin: {error}"),
        );
    }

    let authority_record = match seams
        .load_authority_record(&requester, payload.requester_generation)
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            return rejected_reply(
                &payload.request_id,
                BridgeRejectionCause::Unavailable,
                format!(
                    "no recorded operator authority for '{requester}' \
                     (retryable: the spawn-generation record may not have landed)"
                ),
            );
        }
        Err(error) => {
            return rejected_reply(
                &payload.request_id,
                BridgeRejectionCause::Unavailable,
                format!("operator authority record read failed: {error}"),
            );
        }
    };

    let op_digest = match member_operator_op_digest(&payload.op) {
        Ok(digest) => digest,
        Err(error) => {
            return rejected_reply(
                &payload.request_id,
                BridgeRejectionCause::Internal,
                format!("member operator operation digest failed: {error}"),
            );
        }
    };
    let pending = MobMemberOperatorRequestRecord::pending(request_key, op_digest.clone());
    match seams.begin_request(&pending).await {
        Ok(MobMemberOperatorRequestBegin::Started) => {}
        Ok(MobMemberOperatorRequestBegin::Existing(existing)) => {
            if existing.op_digest != op_digest {
                let reply = completed_reply(
                    &payload.request_id,
                    &UpcallToolOutcome::request_conflict(existing.op_digest, op_digest),
                )
                .unwrap_or_else(|error| {
                    rejected_reply(
                        &payload.request_id,
                        BridgeRejectionCause::Internal,
                        format!("request-conflict reply serialization failed: {error}"),
                    )
                });
                return reply;
            }
            if let Some(reply) = existing.terminal_reply() {
                return reply.clone();
            }
            let reply = match terminalize_indeterminate(seams, &existing).await {
                Ok(reply) => reply,
                Err(error) => volatile_post_pending_persistence_failure_reply(
                    &payload.request_id,
                    "indeterminate terminal",
                    &error,
                ),
            };
            return reply;
        }
        Err(error) => {
            return volatile_begin_failure_reply(&payload.request_id, &error);
        }
    }

    let outcome = match seams
        .execute(
            &pending,
            &authority_record,
            &payload.request_id,
            payload.op.clone(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(UpcallExecuteError::AuthorityRemint(detail)) => {
            let reply = rejected_reply(
                &payload.request_id,
                BridgeRejectionCause::Internal,
                format!("operator authority re-mint failed: {detail}"),
            );
            let reply = match terminalize_or_load_winner(seams, &pending, reply).await {
                Ok(reply) => reply,
                Err(error) => volatile_post_pending_persistence_failure_reply(
                    &payload.request_id,
                    "authority-failure terminal",
                    &error,
                ),
            };
            return reply;
        }
    };

    let reply = match completed_reply(&payload.request_id, &outcome) {
        Ok(reply) => reply,
        Err(error) => {
            let reply = match terminalize_indeterminate(seams, &pending).await {
                Ok(reply) => reply,
                Err(persistence_error) => volatile_post_pending_persistence_failure_reply(
                    &payload.request_id,
                    "serialization-failure indeterminate terminal",
                    &MobStoreError::Internal(format!(
                        "{error}; indeterminate persistence also failed: {persistence_error}"
                    )),
                ),
            };
            return reply;
        }
    };
    match terminalize_or_load_winner(seams, &pending, reply).await {
        Ok(reply) => reply,
        // Do not attempt a second CAS after an effect's success terminal
        // failed to persist. Leaving the durable row Pending is the crash
        // marker; this delivery returns non-success, and only a subsequent
        // delivery may convert the recovered Pending to indeterminate.
        Err(error) => volatile_post_pending_persistence_failure_reply(
            &payload.request_id,
            "post-effect terminal",
            &error,
        ),
    }
}

// ---------------------------------------------------------------------------
// Production seams — MobHandle + metadata store
// ---------------------------------------------------------------------------

pub(crate) struct MobUpcallSeams {
    handle: MobHandle,
    metadata_store: Arc<dyn MobRuntimeMetadataStore>,
    #[cfg(test)]
    projection_post_validation_gate: Option<ProjectionPostValidationGate>,
}

/// Deterministic test seam placed after a read-only projection has captured
/// its result and immediately before the final actor-linearized fence check.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ProjectionPostValidationGate {
    pub(crate) projection_captured: Arc<tokio::sync::Notify>,
    pub(crate) release_validation: Arc<tokio::sync::Notify>,
}

impl MobUpcallSeams {
    pub(crate) fn new(handle: MobHandle, metadata_store: Arc<dyn MobRuntimeMetadataStore>) -> Self {
        // §15.2 lane separation (phase 5, DEC-P5E-8/12): the upcall executor
        // is the agent authority lane end-to-end — its admission is the
        // machine ladder + the SAME MobOperatorToolDispatcher agent gates,
        // never principal ControlScope grants. Binding agent_lane() here
        // keeps chokepoint (a) from consulting grants for upcall-driven
        // commands (T-LS1/T-LS3), and guards the invisible-in-v1 failure
        // where a forgotten rebind would run upcalls as Owner (T-LS4).
        let handle =
            handle.with_command_authority(crate::control_policy::CommandAuthority::agent_lane());
        Self {
            handle,
            metadata_store,
            #[cfg(test)]
            projection_post_validation_gate: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_projection_post_validation_gate(
        mut self,
        gate: ProjectionPostValidationGate,
    ) -> Self {
        self.projection_post_validation_gate = Some(gate);
        self
    }

    /// Execute one non-spawn op through the SAME `MobOperatorToolDispatcher`
    /// (same entry gates, same machine-composed verdicts, same provenance
    /// recording) under the re-minted context.
    async fn execute_via_dispatcher(
        &self,
        context: MobToolAuthorityContext,
        request_id: &str,
        op: MemberOperatorOp,
    ) -> UpcallToolOutcome {
        let requires_final_fence_validation =
            member_operator_projection_requires_final_fence_validation(&op);
        let (tool_name, args) = match dispatcher_call_payload(&op) {
            Ok(pair) => pair,
            Err(error) => return UpcallToolOutcome::from_tool_error(&error),
        };
        let raw = match serde_json::value::RawValue::from_string(args.to_string()) {
            Ok(raw) => raw,
            Err(error) => {
                return UpcallToolOutcome::from_tool_error(&ToolError::execution_failed(format!(
                    "tool '{tool_name}' args serialization failed: {error}"
                )));
            }
        };
        let call = meerkat_core::types::ToolCallView {
            id: request_id,
            name: tool_name,
            args: &raw,
        };
        let dispatcher = MobOperatorToolDispatcher::new(self.handle.clone(), true, context);
        let outcome = match dispatcher.dispatch(call).await {
            Ok(outcome) => UpcallToolOutcome::from_tool_result(&outcome.result),
            Err(error) => UpcallToolOutcome::from_tool_error(&error),
        };
        if requires_final_fence_validation && matches!(&outcome, UpcallToolOutcome::Ok { .. }) {
            // ListMembers/ListFlows are projection-only and MemberStatus
            // finishes with watch/runtime projections after its actor query.
            // Their response therefore needs a second actor turn: the first
            // validation may have linearized before a concurrent host-binding
            // promotion, while the projection observed state after it. This
            // check happens only for read-only operations; post-validating a
            // mutator could report stale after its effect already committed.
            #[cfg(test)]
            if let Some(gate) = &self.projection_post_validation_gate {
                gate.projection_captured.notify_one();
                gate.release_validation.notified().await;
            }
            if let Err(error) = self.handle.validate_command_authority().await {
                return member_operator_execution_error_outcome(error);
            }
        }
        outcome
    }

    async fn execute_spawn_member(
        &self,
        context: &MobToolAuthorityContext,
        requester: &AgentIdentity,
        spec: MemberOperatorSpawnSpec,
    ) -> UpcallToolOutcome {
        match self.spawn_one(context, requester, spec).await {
            Ok(value) => UpcallToolOutcome::Ok {
                content: value.to_string(),
                is_error: false,
            },
            Err(error) => UpcallToolOutcome::from_tool_error(&error),
        }
    }

    async fn execute_spawn_many(
        &self,
        context: &MobToolAuthorityContext,
        requester: &AgentIdentity,
        specs: Vec<MemberOperatorSpawnSpec>,
    ) -> UpcallToolOutcome {
        match self.spawn_many(context, requester, specs).await {
            Ok(value) => UpcallToolOutcome::Ok {
                content: value.to_string(),
                is_error: false,
            },
            Err(error) => UpcallToolOutcome::from_tool_error(&error),
        }
    }

    /// Coarse spawn-tool gate (machine-composed disjunction; the local
    /// dispatcher's `ensure_spawn_tool_scope` twin).
    async fn ensure_spawn_tool_scope(
        &self,
        context: &MobToolAuthorityContext,
        tool_name: &str,
    ) -> Result<(), ToolError> {
        let mob_id = self.handle.mob_id().as_str();
        let admission = self
            .handle
            .resolve_spawn_tool_admission(
                context.can_manage_mob(mob_id),
                context.spawn_profile_scope_present(mob_id),
            )
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' spawn-tool admission failed: {error}"
                ))
            })?;
        match admission {
            SpawnToolAdmission::Allowed => Ok(()),
            SpawnToolAdmission::Denied => Err(ToolError::access_denied(tool_name)),
        }
    }

    /// Per-spec machine admission with the WIRE presence facts (T-F4: the
    /// presence bit is `requested_tool_access_policy_present`, never derived
    /// from the resolved policy; args this surface structurally excludes —
    /// backend, resume aliases — stay `false`).
    async fn ensure_spawn_member_scope(
        &self,
        context: &MobToolAuthorityContext,
        tool_name: &str,
        spec: &MemberOperatorSpawnSpec,
    ) -> Result<(), ToolError> {
        let mob_id = self.handle.mob_id().as_str();
        let observations = SpawnMemberAdmissionObservations {
            manage_scope_present: context.can_manage_mob(mob_id),
            profile_scope_contains: context.spawn_profile_scope_contains(mob_id, &spec.profile),
            runtime_mode_present: spec.runtime_mode.is_some(),
            launch_mode_present: spec.launch_mode.is_some(),
            tool_access_policy_present: spec.requested_tool_access_policy_present,
            ..SpawnMemberAdmissionObservations::default()
        };
        let admission = self
            .handle
            .resolve_spawn_member_admission(observations)
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' spawn-member admission failed: {error}"
                ))
            })?;
        match admission {
            SpawnMemberAdmission::Allowed => Ok(()),
            SpawnMemberAdmission::Denied => Err(ToolError::access_denied(tool_name)),
        }
    }

    async fn spawn_one(
        &self,
        context: &MobToolAuthorityContext,
        requester: &AgentIdentity,
        spec: MemberOperatorSpawnSpec,
    ) -> Result<serde_json::Value, ToolError> {
        let tool_name = TOOL_SPAWN_MEMBER;
        self.ensure_spawn_tool_scope(context, tool_name).await?;
        self.ensure_spawn_member_scope(context, tool_name, &spec)
            .await?;
        let domain_spec = self.domain_spawn_spec(tool_name, requester, spec)?;
        let result = self.handle.spawn_spec(domain_spec).await.map_err(|error| {
            ToolError::execution_failed(format!("tool '{tool_name}' failed: {error}"))
        })?;
        self.record_provenance(tool_name, context).await;
        Ok(self.spawn_result_payload(&result.agent_identity))
    }

    async fn spawn_many(
        &self,
        context: &MobToolAuthorityContext,
        requester: &AgentIdentity,
        specs: Vec<MemberOperatorSpawnSpec>,
    ) -> Result<serde_json::Value, ToolError> {
        let tool_name = TOOL_SPAWN_MANY_MEMBERS;
        self.ensure_spawn_tool_scope(context, tool_name).await?;
        for spec in &specs {
            self.ensure_spawn_member_scope(context, tool_name, spec)
                .await?;
        }
        let domain_specs = specs
            .into_iter()
            .map(|spec| self.domain_spawn_spec(tool_name, requester, spec))
            .collect::<Result<Vec<_>, _>>()?;
        let results = self
            .handle
            .spawn_many(domain_specs)
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!("tool '{tool_name}' failed: {error}"))
            })?
            .into_iter()
            .map(|result| match result {
                Ok(spawn_result) => {
                    let identity = spawn_result.agent_identity.to_string();
                    json!(meerkat_contracts::MobSpawnManyResultEntry::spawned(
                        identity,
                        self.member_ref_payload(&spawn_result.agent_identity),
                    ))
                }
                Err(error) => json!(meerkat_contracts::MobSpawnManyResultEntry::failed(
                    error.cause(),
                    error.to_string()
                )),
            })
            .collect::<Vec<_>>();
        self.record_provenance(tool_name, context).await;
        Ok(json!({ "results": results }))
    }

    /// Wire spawn spec → domain spec. Placement defaults to the REQUESTER's
    /// machine-recorded placement (§7.3: an upcall spawn without explicit
    /// placement lands on the requesting member's host).
    fn domain_spawn_spec(
        &self,
        tool_name: &str,
        requester: &AgentIdentity,
        spec: MemberOperatorSpawnSpec,
    ) -> Result<SpawnMemberSpec, ToolError> {
        let placement = match spec.placement {
            Some(host) => mob_dsl::HostId(host),
            None => self.requester_placement(requester).ok_or_else(|| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' failed: requester '{requester}' has no \
                     machine-recorded placement to default onto"
                ))
            })?,
        };
        let mut out = SpawnMemberSpec::new(spec.profile.clone(), spec.member_id.as_str());
        out.initial_message = spec.initial_message;
        out.runtime_mode = spec.runtime_mode.map(domain_runtime_mode);
        if let Some(launch) = spec.launch_mode {
            out.launch_mode = domain_launch_mode(tool_name, launch)?;
        }
        if let Some(policy) = spec.resolved_tool_access_policy {
            // Explicit AllowList/DenyList, untouched — `Inherit` is
            // structurally absent on this wire (O3).
            out.tool_access_policy = Some(domain_tool_access_policy(policy));
        }
        if let Some(auto_wire) = spec.auto_wire_parent {
            out.auto_wire_parent = auto_wire;
        }
        out.placement = Some(placement);
        Ok(out)
    }

    /// The requester's machine-recorded placement (read-only watch
    /// projection of the actor-owned MobMachine state).
    fn requester_placement(&self, requester: &AgentIdentity) -> Option<mob_dsl::HostId> {
        let state = self.handle.machine_state_watch_rx.borrow();
        state
            .member_placement
            .get(&mob_dsl::AgentIdentity::from(requester.as_str()))
            .cloned()
    }

    fn member_ref_payload(&self, identity: &AgentIdentity) -> meerkat_contracts::WireMemberRef {
        meerkat_contracts::WireMemberRef::encode(self.handle.mob_id().as_str(), identity.as_str())
    }

    fn spawn_result_payload(&self, identity: &AgentIdentity) -> serde_json::Value {
        json!({
            "agent_identity": identity,
            "member_ref": self.member_ref_payload(identity),
        })
    }

    async fn record_provenance(&self, tool_name: &str, context: &MobToolAuthorityContext) {
        if let Err(error) = self
            .handle
            .record_operator_action_provenance(tool_name, context)
            .await
        {
            tracing::warn!(
                tool_name,
                mob_id = %self.handle.mob_id(),
                error = %error,
                "upcall responder: operator provenance projection append failed"
            );
        }
    }
}

#[async_trait::async_trait]
impl UpcallServeSeams for MobUpcallSeams {
    async fn admit(
        &self,
        request: &MemberOperatorAdmissionRequest,
    ) -> Result<MemberOperatorAdmissionVerdict, MobError> {
        self.handle.resolve_member_operator_admission(request).await
    }

    async fn prune_stale_requests(&self) -> Result<u64, MobError> {
        self.handle.prune_stale_member_operator_requests().await
    }

    async fn load_authority_record(
        &self,
        agent_identity: &AgentIdentity,
        requester_generation: u64,
    ) -> Result<Option<MobMemberOperatorAuthorityRecord>, MobStoreError> {
        // Read ONLY the signed, machine-admitted generation from the canonical
        // COMMITTED placed-spawn carrier.  Pending attempts are cleanup-only
        // and must never project member-operator authority; the legacy
        // per-member authority table is deliberately not consulted here.
        self.metadata_store
            .load_committed_placed_member_operator_authority(
                self.handle.mob_id(),
                agent_identity.as_str(),
                requester_generation,
            )
            .await
    }

    async fn begin_request(
        &self,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
        self.metadata_store
            .begin_member_operator_request(self.handle.mob_id(), record)
            .await
    }

    async fn terminalize_request(
        &self,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError> {
        self.metadata_store
            .compare_and_put_member_operator_request(self.handle.mob_id(), expected, record)
            .await
    }

    async fn load_request(
        &self,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
        self.metadata_store
            .load_member_operator_request(self.handle.mob_id(), &record.key())
            .await
    }

    async fn execute(
        &self,
        pending: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorAuthorityRecord,
        request_id: &str,
        op: MemberOperatorOp,
    ) -> Result<UpcallToolOutcome, UpcallExecuteError> {
        // ADJ-15: the record stores capability FACTS; every read re-mints a
        // sealed context through the generated ladder. Caller provenance
        // names the member; the audit invocation id is the upcall
        // request_id.
        let requester = AgentIdentity::from(pending.agent_identity.as_str());
        let fenced_handle = self.handle.clone().with_command_authority(
            crate::control_policy::CommandAuthority::remote_member_operator(
                crate::control_policy::MemberOperatorExecutionFence {
                    agent_identity: pending.agent_identity.clone(),
                    host_id: pending.host_id.clone(),
                    host_binding_generation: pending.host_binding_generation,
                    requester_member_session_id: pending.member_session_id.clone(),
                    generation: pending.generation,
                    fence_token: pending.fence_token,
                },
            ),
        );
        let fenced = Self {
            handle: fenced_handle,
            metadata_store: Arc::clone(&self.metadata_store),
            #[cfg(test)]
            projection_post_validation_gate: self.projection_post_validation_gate.clone(),
        };
        // Pending persistence is not execution authority. Enter the actor once
        // with the exact fenced handle before ANY operation, including the
        // projection-only list verbs that otherwise read immutable/watch
        // surfaces without sending a command. Every later mutating actor
        // command carries the same fence and revalidates again at its own turn.
        if let Err(error) = fenced.handle.validate_command_authority().await {
            return Ok(member_operator_execution_error_outcome(error));
        }
        let facts = MobOperatorAuthorityFacts {
            can_create_mobs: record.can_create_mobs,
            can_mutate_profiles: record.can_mutate_profiles,
            managed_mob_scope: record.managed_mob_scope.clone(),
            spawn_profile_scope: record.spawn_profile_scope.clone(),
        };
        let provenance = MobToolCallerProvenance::default()
            .with_mob_id(fenced.handle.mob_id().as_str())
            .with_member_id(requester.as_str());
        let context = mint_mob_operator_authority_from_facts(&facts, provenance, request_id)
            .map_err(UpcallExecuteError::AuthorityRemint)?;

        Ok(match op {
            MemberOperatorOp::SpawnMember(spec) => {
                fenced
                    .execute_spawn_member(&context, &requester, *spec)
                    .await
            }
            MemberOperatorOp::SpawnManyMembers { specs } => {
                fenced.execute_spawn_many(&context, &requester, specs).await
            }
            other => {
                fenced
                    .execute_via_dispatcher(context, request_id, other)
                    .await
            }
        })
    }
}

/// Non-spawn op → (tool name, dispatcher args). Total over the closed wire
/// enum; the two spawn variants are routed by the caller and fail typed if
/// they ever reach here (no panic in library code).
fn dispatcher_call_payload(
    op: &MemberOperatorOp,
) -> Result<(&'static str, serde_json::Value), ToolError> {
    match op {
        MemberOperatorOp::SpawnMember(_) | MemberOperatorOp::SpawnManyMembers { .. } => {
            Err(ToolError::execution_failed(
                "spawn ops are served on the wire-facts admission path, not the JSON dispatcher path",
            ))
        }
        MemberOperatorOp::RetireMember { member_id } => {
            Ok((TOOL_RETIRE_MEMBER, json!({ "member_id": member_id })))
        }
        MemberOperatorOp::ForceCancelMember { member_id } => {
            Ok((TOOL_FORCE_CANCEL_MEMBER, json!({ "member_id": member_id })))
        }
        MemberOperatorOp::MemberStatus { member_id } => {
            Ok((TOOL_MEMBER_STATUS, json!({ "member_id": member_id })))
        }
        MemberOperatorOp::WireMembers {
            member_id,
            peer_member_id,
        } => Ok((
            TOOL_WIRE_MEMBERS,
            json!({ "member_id": member_id, "peer_member_id": peer_member_id }),
        )),
        MemberOperatorOp::UnwireMembers {
            member_id,
            peer_member_id,
        } => Ok((
            TOOL_UNWIRE_MEMBERS,
            json!({ "member_id": member_id, "peer_member_id": peer_member_id }),
        )),
        MemberOperatorOp::ListMembers => Ok((TOOL_LIST_MEMBERS, json!({}))),
        MemberOperatorOp::MobListFlows => Ok((TOOL_MOB_LIST_FLOWS, json!({}))),
        MemberOperatorOp::MobRunFlow { flow_id, params } => {
            let mut args = json!({ "flow_id": flow_id });
            if let Some(params) = params {
                let value = params.to_value().map_err(|error| {
                    ToolError::invalid_arguments(
                        TOOL_MOB_RUN_FLOW,
                        format!("flow params envelope is not valid JSON: {error}"),
                    )
                })?;
                args["params"] = value;
            }
            Ok((TOOL_MOB_RUN_FLOW, args))
        }
        MemberOperatorOp::MobFlowStatus { run_id } => {
            Ok((TOOL_MOB_FLOW_STATUS, json!({ "run_id": run_id })))
        }
        MemberOperatorOp::MobCancelFlow { run_id } => {
            Ok((TOOL_MOB_CANCEL_FLOW, json!({ "run_id": run_id })))
        }
    }
}

/// Operations whose successful response is assembled outside one actor turn.
///
/// The match is deliberately exhaustive: a future member-operator verb must
/// choose explicitly whether it needs a final fence check. Actor-routed reads
/// such as `MobFlowStatus` already have a linearization point at the routed
/// command; mutators must never be post-validated because an effect followed
/// by a stale error would make the durable terminal outcome ambiguous.
fn member_operator_projection_requires_final_fence_validation(op: &MemberOperatorOp) -> bool {
    match op {
        MemberOperatorOp::MemberStatus { .. }
        | MemberOperatorOp::ListMembers
        | MemberOperatorOp::MobListFlows => true,
        MemberOperatorOp::SpawnMember(_)
        | MemberOperatorOp::SpawnManyMembers { .. }
        | MemberOperatorOp::RetireMember { .. }
        | MemberOperatorOp::ForceCancelMember { .. }
        | MemberOperatorOp::WireMembers { .. }
        | MemberOperatorOp::UnwireMembers { .. }
        | MemberOperatorOp::MobRunFlow { .. }
        | MemberOperatorOp::MobFlowStatus { .. }
        | MemberOperatorOp::MobCancelFlow { .. } => false,
    }
}

fn domain_runtime_mode(mode: meerkat_contracts::wire::WireMobRuntimeMode) -> crate::MobRuntimeMode {
    match mode {
        meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost => {
            crate::MobRuntimeMode::AutonomousHost
        }
        meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven => {
            crate::MobRuntimeMode::TurnDriven
        }
    }
}

fn domain_launch_mode(
    tool_name: &str,
    mode: meerkat_contracts::wire::WireMemberLaunchMode,
) -> Result<crate::launch::MemberLaunchMode, ToolError> {
    use meerkat_contracts::wire::{WireForkContext, WireMemberLaunchMode};
    Ok(match mode {
        WireMemberLaunchMode::Fresh => crate::launch::MemberLaunchMode::Fresh,
        WireMemberLaunchMode::Resume { bridge_session_id } => {
            let session_id =
                meerkat_core::types::SessionId::parse(&bridge_session_id).map_err(|error| {
                    ToolError::invalid_arguments(
                        tool_name,
                        format!("invalid resume bridge session id: {error}"),
                    )
                })?;
            crate::launch::MemberLaunchMode::Resume {
                bridge_session_id: session_id,
            }
        }
        WireMemberLaunchMode::Fork {
            source_member_id,
            fork_context,
        } => crate::launch::MemberLaunchMode::Fork {
            source_member_id: AgentIdentity::from(source_member_id.as_str()),
            fork_context: match fork_context {
                WireForkContext::FullHistory => crate::launch::ForkContext::FullHistory,
                WireForkContext::LastMessages { count } => {
                    crate::launch::ForkContext::LastMessages { count }
                }
            },
        },
    })
}

fn domain_tool_access_policy(
    policy: meerkat_contracts::wire::WireResolvedToolAccessPolicy,
) -> meerkat_core::ops::ToolAccessPolicy {
    use meerkat_contracts::wire::WireResolvedToolAccessPolicy;
    match policy {
        WireResolvedToolAccessPolicy::AllowList(names) => {
            meerkat_core::ops::ToolAccessPolicy::AllowList(names.into_iter().collect())
        }
        WireResolvedToolAccessPolicy::DenyList(names) => {
            meerkat_core::ops::ToolAccessPolicy::DenyList(names.into_iter().collect())
        }
    }
}

// ---------------------------------------------------------------------------
// Responder task (§4, DEC-U3)
// ---------------------------------------------------------------------------

/// Handle to the spawned responder task; aborts on drop, or shut down
/// explicitly at mob shutdown/destroy.
pub(crate) struct MobUpcallResponderHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl MobUpcallResponderHandle {
    pub(crate) async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut self.join)
            .await
            .is_err()
        {
            self.join.abort();
            let _ = (&mut self.join).await;
        }
    }
}

impl Drop for MobUpcallResponderHandle {
    fn drop(&mut self) {
        // Dropping the sender completes the responder's shutdown branch;
        // abort is the backstop for a task parked mid-serve.
        self.join.abort();
    }
}

/// Spawn the upcall responder next to the mob actor (builder wiring).
pub(crate) fn spawn_upcall_responder(
    handle: MobHandle,
    metadata_store: Arc<dyn MobRuntimeMetadataStore>,
) -> MobUpcallResponderHandle {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let join = tokio::spawn(run_upcall_responder(handle, metadata_store, shutdown_rx));
    MobUpcallResponderHandle {
        shutdown_tx: Some(shutdown_tx),
        join,
    }
}

/// Shared-intake serve loop (§4): drain the supervisor bridge inbox, serve
/// bridge-intent requests serially in arrival order, and hand EVERYTHING
/// else to the bridge's shared buffer with a wake — neither this loop nor an
/// in-flight `wait_for_response` can starve the other.
async fn run_upcall_responder(
    handle: MobHandle,
    metadata_store: Arc<dyn MobRuntimeMetadataStore>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let seams = MobUpcallSeams::new(handle.clone(), metadata_store);
    loop {
        // Re-resolve per iteration: supervisor rotation swaps the bridge
        // runtime (buffered upcalls die with the rotation; members re-send
        // dedup-safe).
        let bridge = Arc::clone(&handle.supervisor_bridge);
        let runtime = bridge.runtime_core().await;
        let inbox_notify = runtime.inbox_notify();
        let intake_notify = bridge.intake_notify();
        // Arm both wakeups BEFORE draining so a candidate that lands between
        // drain and select still wakes this loop. Both notifies fire with
        // `notify_waiters()`, which only wakes registered listeners — a
        // `Notified` future registers on `enable()` (or first poll), not on
        // creation, so pinning alone is not arming.
        let inbox_notified = inbox_notify.notified();
        let intake_notified = intake_notify.notified();
        tokio::pin!(inbox_notified);
        tokio::pin!(intake_notified);
        inbox_notified.as_mut().enable();
        intake_notified.as_mut().enable();

        let mut requests = bridge.take_buffered_upcall_requests().await;
        let drained = runtime.drain_peer_input_candidates().await;
        let mut foreign = Vec::new();
        for candidate in drained {
            if matches!(
                candidate.interaction.content,
                InteractionContent::Request { .. }
            ) {
                requests.push(candidate);
            } else {
                foreign.push(candidate);
            }
        }
        if !foreign.is_empty() {
            // Responses/lifecycle events belong to `wait_for_response`; the
            // buffer push fires the shared intake wake.
            bridge.buffer_foreign_candidates(foreign).await;
        }

        let served_any = !requests.is_empty();
        for candidate in requests {
            serve_candidate(&seams, &handle, &runtime, &candidate).await;
        }

        match shutdown_rx.try_recv() {
            Ok(()) | Err(oneshot::error::TryRecvError::Closed) => break,
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
        if served_any {
            continue;
        }
        tokio::select! {
            _ = &mut shutdown_rx => break,
            () = &mut inbox_notified => {}
            () = &mut intake_notified => {}
        }
    }
}

/// Serve one drained Request candidate (record → decode → admission-first
/// pipeline → reply through the host_reply seam).
async fn serve_candidate(
    seams: &MobUpcallSeams,
    handle: &MobHandle,
    runtime: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
) {
    let InteractionContent::Request { intent, params, .. } = &candidate.interaction.content else {
        runtime.mark_interaction_complete(&candidate.interaction.id);
        return;
    };
    if intent != SUPERVISOR_BRIDGE_INTENT {
        send_failure(
            runtime,
            candidate,
            BridgeRejectionCause::Unsupported,
            format!("unsupported intent '{intent}' on the mob supervisor inbox"),
        )
        .await;
        return;
    }
    // Record the inbound request under complete request/response authority
    // BEFORE any decode or side effect (host responder boundary discipline).
    if !record_inbound_request(runtime, candidate) {
        return;
    }
    let command = match decode_bridge_command(params.clone()) {
        Ok(command) => command,
        Err(error) => {
            let cause = match &error {
                BridgeCommandDecodeError::UnsupportedProtocolVersion(_) => {
                    BridgeRejectionCause::UnsupportedProtocolVersion
                }
                BridgeCommandDecodeError::Invalid(_) => BridgeRejectionCause::Unsupported,
            };
            send_failure(
                runtime,
                candidate,
                cause,
                format!("invalid bridge command: {error}"),
            )
            .await;
            return;
        }
    };
    let BridgeCommand::MemberOperatorRequest(payload) = command else {
        send_failure(
            runtime,
            candidate,
            BridgeRejectionCause::Unsupported,
            "the mob supervisor inbox serves only member-originated operator requests",
        )
        .await;
        return;
    };
    // No authenticated sender ⇒ no admission fact can be constructed
    // (fail-closed observation assembly).
    let Some(sender_peer_id) = candidate.ingress.canonical_peer_id else {
        send_failure(
            runtime,
            candidate,
            BridgeRejectionCause::SenderMismatch,
            "unauthenticated sender: no canonical peer identity admitted at ingress",
        )
        .await;
        return;
    };

    let reply = serve_member_operator_request(seams, &payload, sender_peer_id).await;
    // ADJ-16: stage a one-shot reply endpoint from machine-recorded member
    // endpoint facts so the reply routes without a trust edge. Rejected
    // UNKNOWN senders have no machine endpoint and stay best-effort on the
    // ingress route (§15.6 mirror).
    stage_member_reply_endpoint(handle, runtime, candidate, &payload.agent_identity).await;
    let host_reply = match &reply.outcome {
        MemberOperatorOutcome::Completed { .. } => {
            HostBridgeReply::completed(BridgeReply::MemberOperatorReply(reply))
        }
        MemberOperatorOutcome::Rejected { .. } => {
            HostBridgeReply::failed(BridgeReply::MemberOperatorReply(reply))
        }
    };
    send_reply(runtime, candidate, host_reply).await;
}

/// Record the inbound request on the generated peer-interaction authority.
/// `false` means the candidate was completed and must not be decoded or
/// served (host_actor twin).
fn record_inbound_request(
    runtime: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
) -> bool {
    let Some(handle) = runtime.peer_request_response_authority_handle() else {
        tracing::warn!(
            interaction_id = %candidate.interaction.id,
            "upcall responder: rejected bridge request without complete peer request authority"
        );
        runtime.mark_interaction_complete(&candidate.interaction.id);
        return false;
    };
    let corr_id = meerkat_core::PeerCorrelationId::from_uuid(candidate.interaction.id.0);
    if handle.inbound_state(corr_id).is_some() {
        return true;
    }
    if let Err(error) = handle.request_received(corr_id, candidate.interaction.handling_mode) {
        tracing::warn!(
            error = %error,
            corr_id = %corr_id,
            "upcall responder: PeerInteractionHandle::request_received rejected bridge command"
        );
        runtime.mark_interaction_complete(&candidate.interaction.id);
        return false;
    }
    true
}

/// ADJ-16: prefer a correlation-bound endpoint declared inside the signed
/// Request, then fall back to a one-shot `DeclaredReplyEndpoint` from the
/// machine-recorded `member_peer_endpoints[identity]` (both address and signing key are
/// machine truth installed from the materialization ack; for an ADMITTED
/// upcall the recorded key IS the verified envelope signer — the
/// `member_peer_ids` guard matched it). `Unsupported` means the runtime has
/// no staging capability (in-proc resolves via the ingress route) and is
/// skipped; the concrete override already skips already-routable peers.
async fn stage_member_reply_endpoint(
    handle: &MobHandle,
    runtime: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
    agent_identity: &str,
) {
    if let (Some(endpoint), Some(sender_peer_id), Some(signing_pubkey)) = (
        candidate.ingress.declared_reply_endpoint.clone(),
        candidate.ingress.canonical_peer_id,
        candidate.ingress.signing_pubkey,
    ) {
        match runtime
            .stage_correlated_reply_endpoint(
                sender_peer_id,
                candidate.interaction.id,
                signing_pubkey,
                endpoint,
            )
            .await
        {
            Ok(()) => return,
            Err(SendError::Unsupported(_)) => {}
            Err(error) => {
                tracing::warn!(
                    agent_identity,
                    interaction_id = %candidate.interaction.id,
                    error = %error,
                    "upcall responder: authenticated correlated reply endpoint staging failed"
                );
                return;
            }
        }
    }

    let endpoint = {
        let state = handle.machine_state_watch_rx.borrow();
        state
            .member_peer_endpoints
            .get(&mob_dsl::AgentIdentity::from(agent_identity))
            .cloned()
    };
    let Some(endpoint) = endpoint else {
        return;
    };
    let signing_key = endpoint.signing_key.0;
    let dest = PeerId::from_ed25519_pubkey(&signing_key);
    match runtime
        .stage_declared_reply_endpoint(dest, signing_key, endpoint.address.0.clone())
        .await
    {
        Ok(()) | Err(SendError::Unsupported(_)) => {}
        Err(error) => {
            tracing::warn!(
                agent_identity,
                error = %error,
                "upcall responder: reply endpoint staging failed; reply falls back to the ingress route"
            );
        }
    }
}

async fn send_failure(
    runtime: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
    cause: BridgeRejectionCause,
    reason: impl Into<String>,
) {
    send_reply(runtime, candidate, HostBridgeReply::rejected(cause, reason)).await;
}

/// Send one reply on the supervisor bridge runtime. Status construction and
/// the serialization-failure downgrade live in the host_reply constructor
/// seam (DEC-P3F-5) — this file never names a status.
async fn send_reply(
    runtime: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
    reply: HostBridgeReply,
) {
    let (status, result) = reply.into_wire(candidate.interaction.id);
    let Some(to) = resolve_response_route(runtime, candidate).await else {
        tracing::warn!(
            interaction_id = %candidate.interaction.id,
            "upcall responder: failed to resolve reply peer route"
        );
        runtime.mark_interaction_complete(&candidate.interaction.id);
        return;
    };
    if let Err(error) = runtime
        .send(CommsCommand::PeerResponse {
            to,
            in_reply_to: candidate.interaction.id,
            status,
            result,
            blocks: None,
            content_taint: None,
            handling_mode: None,
            objective_id: candidate.interaction.objective_id,
        })
        .await
    {
        if let Some(sender_peer_id) = candidate.ingress.canonical_peer_id
            && candidate.ingress.declared_reply_endpoint.is_some()
            && let Err(cleanup_error) = runtime
                .unstage_correlated_reply_endpoint(sender_peer_id, candidate.interaction.id)
                .await
            && !matches!(cleanup_error, SendError::Unsupported(_))
        {
            tracing::warn!(
                interaction_id = %candidate.interaction.id,
                error = %cleanup_error,
                "upcall responder: failed to clear correlated reply endpoint after response failure"
            );
        }
        tracing::warn!(
            interaction_id = %candidate.interaction.id,
            error = %error,
            "upcall responder: failed to send member operator reply"
        );
    }
    runtime.mark_interaction_complete(&candidate.interaction.id);
}

async fn resolve_response_route(
    runtime: &Arc<dyn CoreCommsRuntime>,
    candidate: &PeerInputCandidate,
) -> Option<PeerRoute> {
    if let Some(sender_route) = candidate.ingress.route.clone() {
        if let Some(route) = resolve_peer_route(runtime, sender_route.peer_id).await {
            return Some(route);
        }
        return Some(sender_route);
    }
    if let Some(sender_peer_id) = candidate.ingress.canonical_peer_id {
        return resolve_peer_route(runtime, sender_peer_id)
            .await
            .or_else(|| Some(PeerRoute::new(sender_peer_id)));
    }
    None
}

async fn resolve_peer_route(
    runtime: &Arc<dyn CoreCommsRuntime>,
    peer_id: PeerId,
) -> Option<PeerRoute> {
    let peers = runtime.peers().await;
    peers
        .iter()
        .find(|entry| entry.peer_id == peer_id)
        .map(|entry| PeerRoute::with_display_name(entry.peer_id, entry.name.clone()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn payload(
        identity: &str,
        request_id: &str,
        op: MemberOperatorOp,
    ) -> BridgeMemberOperatorPayload {
        BridgeMemberOperatorPayload {
            agent_identity: identity.to_string(),
            requester_generation: 1,
            requester_fence_token: 1,
            requester_host_id: "host-a".to_string(),
            requester_host_binding_generation: 1,
            requester_member_session_id: "member-session-a".to_string(),
            request_id: request_id.to_string(),
            op,
            protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
        }
    }

    fn sender() -> PeerId {
        PeerId::from_ed25519_pubkey(&[3u8; 32])
    }

    fn record() -> MobMemberOperatorAuthorityRecord {
        MobMemberOperatorAuthorityRecord {
            agent_identity: "b2".to_string(),
            generation: 1,
            can_run_adaptive_packs: false,
            can_create_mobs: false,
            can_mutate_profiles: false,
            managed_mob_scope: std::collections::BTreeSet::new(),
            spawn_profile_scope: std::collections::BTreeMap::new(),
        }
    }

    /// Spy seams: scripted admission verdicts, counted store loads, counted
    /// executions.
    struct SpySeams {
        verdicts: std::sync::Mutex<Vec<MemberOperatorAdmissionVerdict>>,
        prunes: AtomicUsize,
        prune_failures: AtomicUsize,
        store_loads: AtomicUsize,
        executions: AtomicUsize,
        begin_failures: AtomicUsize,
        terminalize_failures: AtomicUsize,
        record: Option<MobMemberOperatorAuthorityRecord>,
        ledger: crate::store::InMemoryMobRuntimeMetadataStore,
        mob_id: crate::ids::MobId,
        execution_gate: Option<Arc<ExecutionGate>>,
    }

    struct ExecutionGate {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl SpySeams {
        fn new(
            verdicts: Vec<MemberOperatorAdmissionVerdict>,
            record: Option<MobMemberOperatorAuthorityRecord>,
        ) -> Self {
            Self {
                verdicts: std::sync::Mutex::new(verdicts),
                prunes: AtomicUsize::new(0),
                prune_failures: AtomicUsize::new(0),
                store_loads: AtomicUsize::new(0),
                executions: AtomicUsize::new(0),
                begin_failures: AtomicUsize::new(0),
                terminalize_failures: AtomicUsize::new(0),
                record,
                ledger: crate::store::InMemoryMobRuntimeMetadataStore::new(),
                mob_id: crate::ids::MobId::from("upcall-spy"),
                execution_gate: None,
            }
        }

        fn with_execution_gate(mut self, execution_gate: Arc<ExecutionGate>) -> Self {
            self.execution_gate = Some(execution_gate);
            self
        }

        fn with_begin_failures(self, failures: usize) -> Self {
            self.begin_failures.store(failures, Ordering::SeqCst);
            self
        }

        fn with_prune_failures(self, failures: usize) -> Self {
            self.prune_failures.store(failures, Ordering::SeqCst);
            self
        }

        fn with_terminalize_failures(self, failures: usize) -> Self {
            self.terminalize_failures.store(failures, Ordering::SeqCst);
            self
        }

        fn take_failure(counter: &AtomicUsize) -> bool {
            counter
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
        }
    }

    #[async_trait::async_trait]
    impl UpcallServeSeams for SpySeams {
        async fn admit(
            &self,
            _request: &MemberOperatorAdmissionRequest,
        ) -> Result<MemberOperatorAdmissionVerdict, MobError> {
            let mut verdicts = self.verdicts.lock().unwrap();
            if verdicts.is_empty() {
                Ok(MemberOperatorAdmissionVerdict::Admitted)
            } else {
                Ok(verdicts.remove(0))
            }
        }

        async fn prune_stale_requests(&self) -> Result<u64, MobError> {
            self.prunes.fetch_add(1, Ordering::SeqCst);
            if Self::take_failure(&self.prune_failures) {
                return Err(MobError::Internal(
                    "forced stale-ledger prune failure".to_string(),
                ));
            }
            Ok(0)
        }

        async fn load_authority_record(
            &self,
            _agent_identity: &AgentIdentity,
            requester_generation: u64,
        ) -> Result<Option<MobMemberOperatorAuthorityRecord>, MobStoreError> {
            self.store_loads.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .record
                .clone()
                .filter(|record| record.generation == requester_generation))
        }

        async fn begin_request(
            &self,
            record: &MobMemberOperatorRequestRecord,
        ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
            if Self::take_failure(&self.begin_failures) {
                return Err(MobStoreError::Internal(
                    "forced Pending begin persistence failure".to_string(),
                ));
            }
            self.ledger
                .begin_member_operator_request(&self.mob_id, record)
                .await
        }

        async fn terminalize_request(
            &self,
            expected: &MobMemberOperatorRequestRecord,
            record: &MobMemberOperatorRequestRecord,
        ) -> Result<bool, MobStoreError> {
            if Self::take_failure(&self.terminalize_failures) {
                return Err(MobStoreError::Internal(
                    "forced terminal persistence failure".to_string(),
                ));
            }
            self.ledger
                .compare_and_put_member_operator_request(&self.mob_id, expected, record)
                .await
        }

        async fn load_request(
            &self,
            record: &MobMemberOperatorRequestRecord,
        ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
            self.ledger
                .load_member_operator_request(&self.mob_id, &record.key())
                .await
        }

        async fn execute(
            &self,
            _pending: &MobMemberOperatorRequestRecord,
            _record: &MobMemberOperatorAuthorityRecord,
            _request_id: &str,
            _op: MemberOperatorOp,
        ) -> Result<UpcallToolOutcome, UpcallExecuteError> {
            if let Some(gate) = &self.execution_gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(UpcallToolOutcome::Ok {
                content: "{\"ok\":true}".to_string(),
                is_error: false,
            })
        }
    }

    fn decode_tool_outcome(reply: &MemberOperatorReply) -> UpcallToolOutcome {
        let MemberOperatorOutcome::Completed { result } = &reply.outcome else {
            panic!(
                "expected completed upcall envelope, got {:?}",
                reply.outcome
            );
        };
        serde_json::from_value(result.to_value().expect("opaque result JSON"))
            .expect("typed upcall tool outcome")
    }

    /// T-7 (ADJ-14): reject mapping table; reason carries the machine kind.
    #[test]
    fn reject_kind_maps_to_wire_causes() {
        use mob_dsl::MemberOperatorRejectKind as K;
        assert_eq!(
            map_member_operator_reject_kind(K::UnknownIdentity),
            BridgeRejectionCause::SenderMismatch
        );
        assert_eq!(
            map_member_operator_reject_kind(K::SenderKeyMismatch),
            BridgeRejectionCause::SenderMismatch
        );
        assert_eq!(
            map_member_operator_reject_kind(K::NoPlacement),
            BridgeRejectionCause::Unavailable
        );
        assert_eq!(
            map_member_operator_reject_kind(K::HostRevoked),
            BridgeRejectionCause::NotBound
        );
        assert_eq!(
            map_member_operator_reject_kind(K::StaleGeneration),
            BridgeRejectionCause::StaleFence
        );
        assert_eq!(
            map_member_operator_reject_kind(K::StaleFence),
            BridgeRejectionCause::StaleFence
        );
        assert_eq!(
            map_member_operator_reject_kind(K::StaleSession),
            BridgeRejectionCause::StaleFence
        );
        assert_eq!(
            map_member_operator_reject_kind(K::StaleHost),
            BridgeRejectionCause::StaleFence
        );
        assert_eq!(
            map_member_operator_reject_kind(K::StaleHostBindingGeneration),
            BridgeRejectionCause::StaleFence
        );
        assert!(member_operator_reject_reason(K::UnknownIdentity).contains("UnknownIdentity"));
        assert!(member_operator_reject_reason(K::HostRevoked).contains("HostRevoked"));
    }

    #[test]
    fn only_non_actor_linearized_projections_require_final_fence_validation() {
        assert!(member_operator_projection_requires_final_fence_validation(
            &MemberOperatorOp::ListMembers,
        ));
        assert!(member_operator_projection_requires_final_fence_validation(
            &MemberOperatorOp::MobListFlows,
        ));
        assert!(member_operator_projection_requires_final_fence_validation(
            &MemberOperatorOp::MemberStatus {
                member_id: "worker".to_string(),
            },
        ));

        assert!(!member_operator_projection_requires_final_fence_validation(
            &MemberOperatorOp::MobFlowStatus {
                run_id: "00000000-0000-0000-0000-000000000000".to_string(),
            },
        ));
        assert!(!member_operator_projection_requires_final_fence_validation(
            &MemberOperatorOp::RetireMember {
                member_id: "worker".to_string(),
            },
        ));
    }

    /// T-6: a machine-rejected admission never reads the authority store and
    /// never constructs a dispatcher (execution count zero).
    #[tokio::test]
    async fn rejected_admission_never_touches_authority_or_execution() {
        let seams = SpySeams::new(
            vec![MemberOperatorAdmissionVerdict::Rejected(
                mob_dsl::MemberOperatorRejectKind::UnknownIdentity,
            )],
            Some(record()),
        );
        let reply = serve_member_operator_request(
            &seams,
            &payload("ghost", "req-1", MemberOperatorOp::ListMembers),
            sender(),
        )
        .await;
        match reply.outcome {
            MemberOperatorOutcome::Rejected { cause, reason } => {
                assert_eq!(cause, BridgeRejectionCause::SenderMismatch);
                assert!(reason.contains("UnknownIdentity"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(
            seams.store_loads.load(Ordering::SeqCst),
            0,
            "zero store loads"
        );
        assert_eq!(seams.prunes.load(Ordering::SeqCst), 0, "zero prunes");
        assert_eq!(
            seams.executions.load(Ordering::SeqCst),
            0,
            "zero executions"
        );
        assert!(
            seams
                .ledger
                .list_member_operator_requests(&seams.mob_id)
                .await
                .expect("list request ledger")
                .is_empty(),
            "rejected deliveries never touch the durable ledger"
        );
    }

    #[tokio::test]
    async fn admitted_prune_failure_stops_before_authority_load_or_durable_begin() {
        let seams = SpySeams::new(vec![], Some(record())).with_prune_failures(1);
        let reply = serve_member_operator_request(
            &seams,
            &payload("b2", "req-prune-failure", MemberOperatorOp::ListMembers),
            sender(),
        )
        .await;
        assert!(matches!(
            reply.outcome,
            MemberOperatorOutcome::Rejected {
                cause: BridgeRejectionCause::Unavailable,
                ..
            }
        ));
        assert_eq!(seams.prunes.load(Ordering::SeqCst), 1);
        assert_eq!(seams.store_loads.load(Ordering::SeqCst), 0);
        assert_eq!(seams.executions.load(Ordering::SeqCst), 0);
        assert!(
            seams
                .ledger
                .list_member_operator_requests(&seams.mob_id)
                .await
                .expect("list ledger after prune failure")
                .is_empty(),
            "a prune failure must happen before durable Pending insertion"
        );
    }

    /// T-8: a durable terminal reply replays verbatim (one execution). A
    /// duplicate is STILL machine-admitted first — after RevokeHost the
    /// recorded reply is NOT replayed.
    #[tokio::test]
    async fn durable_terminal_replays_and_admission_stays_first() {
        let seams = SpySeams::new(vec![], Some(record()));
        let request = payload("b2", "req-1", MemberOperatorOp::ListMembers);

        let first_reply = serve_member_operator_request(&seams, &request, sender()).await;
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);

        let second_reply = serve_member_operator_request(&seams, &request, sender()).await;
        assert_eq!(
            serde_json::to_string(&first_reply).unwrap(),
            serde_json::to_string(&second_reply).unwrap(),
            "recorded reply replays verbatim"
        );
        assert_eq!(
            seams.executions.load(Ordering::SeqCst),
            1,
            "exactly one execution"
        );

        // Duplicate after RevokeHost: admission runs FIRST and rejects.
        seams.verdicts.lock().expect("verdict script").push(
            MemberOperatorAdmissionVerdict::Rejected(
                mob_dsl::MemberOperatorRejectKind::HostRevoked,
            ),
        );
        let third_reply = serve_member_operator_request(&seams, &request, sender()).await;
        match third_reply.outcome {
            MemberOperatorOutcome::Rejected { cause, .. } => {
                assert_eq!(
                    cause,
                    BridgeRejectionCause::NotBound,
                    "HostRevoked → NotBound"
                );
            }
            other => panic!("a post-revoke duplicate must reject, got {other:?}"),
        }
    }

    /// T-9: missing authority record for an ADMITTED identity ⇒ Rejected
    /// Unavailable, no execution, no ledger row (retry may proceed after the
    /// generation-pinned authority record lands).
    #[tokio::test]
    async fn missing_authority_record_rejects_unavailable_without_request_record() {
        let seams = SpySeams::new(vec![], None);
        let request = payload("b2", "req-1", MemberOperatorOp::ListMembers);

        let reply = serve_member_operator_request(&seams, &request, sender()).await;
        match reply.outcome {
            MemberOperatorOutcome::Rejected { cause, .. } => {
                assert_eq!(cause, BridgeRejectionCause::Unavailable);
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(seams.executions.load(Ordering::SeqCst), 0);
        assert!(
            seams
                .ledger
                .list_member_operator_requests(&seams.mob_id)
                .await
                .expect("list request ledger")
                .is_empty(),
            "no Pending row exists before the authority record is available"
        );
    }

    /// A failure to persist the initial Pending precedes every effect. The
    /// caller receives an explicitly retryable rejection, the ledger remains
    /// empty, and a later retry may become the sole executor.
    #[tokio::test]
    async fn pending_begin_failure_runs_no_effect_and_retry_may_execute() {
        let seams = SpySeams::new(vec![], Some(record())).with_begin_failures(1);
        let request = payload("b2", "req-begin-failure", MemberOperatorOp::ListMembers);

        let failed = serve_member_operator_request(&seams, &request, sender()).await;
        let MemberOperatorOutcome::Rejected { cause, reason } = failed.outcome else {
            panic!("Pending begin failure must reject non-success");
        };
        assert_eq!(cause, BridgeRejectionCause::Unavailable);
        assert!(reason.contains("no effect was run and retry is safe"));
        assert_eq!(seams.executions.load(Ordering::SeqCst), 0);
        assert!(
            seams
                .ledger
                .list_member_operator_requests(&seams.mob_id)
                .await
                .expect("list request ledger")
                .is_empty()
        );

        let retried = serve_member_operator_request(&seams, &request, sender()).await;
        assert!(matches!(
            retried.outcome,
            MemberOperatorOutcome::Completed { .. }
        ));
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);
    }

    /// T-10 (ADJ-15): the facts-path re-mint produces sealed generated
    /// authority with the member provenance and the request_id audit stamp;
    /// no stored-context rehydration exists on this path.
    #[test]
    fn facts_remint_produces_sealed_generated_authority() {
        let mut managed = std::collections::BTreeSet::new();
        managed.insert("mob-a".to_string());
        let mut spawn_scope = std::collections::BTreeMap::new();
        let mut profiles = std::collections::BTreeSet::new();
        profiles.insert("worker".to_string());
        spawn_scope.insert("mob-a".to_string(), profiles);
        let facts = MobOperatorAuthorityFacts {
            can_create_mobs: false,
            can_mutate_profiles: false,
            managed_mob_scope: managed,
            spawn_profile_scope: spawn_scope,
        };
        let provenance = MobToolCallerProvenance::default()
            .with_mob_id("mob-a")
            .with_member_id("b2");
        let context = mint_mob_operator_authority_from_facts(&facts, provenance, "req-1")
            .expect("facts re-mint through the generated ladder");
        assert!(
            context.is_generated_authority_context(),
            "sealed generated context"
        );
        assert!(context.can_manage_mob("mob-a"));
        assert!(context.spawn_profile_scope_contains("mob-a", "worker"));
        assert!(!context.can_manage_mob("mob-other"));
    }

    /// A recovered Pending for every high-risk mutator terminalizes
    /// indeterminate and never calls the executor. This models a crash after
    /// the effect but before terminal reply persistence; replay is stable.
    #[tokio::test]
    async fn recovered_pending_never_reexecutes_spawn_spawn_many_or_run_flow() {
        let seams = SpySeams::new(vec![], Some(record()));
        let spawn_spec = |member_id: &str| MemberOperatorSpawnSpec {
            profile: "worker".to_string(),
            member_id: member_id.to_string(),
            initial_message: None,
            runtime_mode: None,
            launch_mode: None,
            auto_wire_parent: None,
            placement: None,
            requested_tool_access_policy_present: false,
            resolved_tool_access_policy: None,
        };
        let cases = vec![
            MemberOperatorOp::SpawnMember(Box::new(spawn_spec("b21"))),
            MemberOperatorOp::SpawnManyMembers {
                specs: vec![spawn_spec("b22"), spawn_spec("b23")],
            },
            MemberOperatorOp::MobRunFlow {
                flow_id: "release".to_string(),
                params: None,
            },
        ];

        for (index, op) in cases.into_iter().enumerate() {
            let request_id = format!("req-crash-{index}");
            let digest = member_operator_op_digest(&op).expect("operation digest");
            let pending = MobMemberOperatorRequestRecord::pending(
                MobMemberOperatorRequestKey::new(
                    "b2",
                    record().generation,
                    1,
                    "host-a",
                    1,
                    "member-session-a",
                    request_id.clone(),
                ),
                digest,
            );
            assert!(matches!(
                seams.begin_request(&pending).await.expect("seed Pending"),
                MobMemberOperatorRequestBegin::Started
            ));

            let request = payload("b2", &request_id, op);
            let first = serve_member_operator_request(&seams, &request, sender()).await;
            let UpcallToolOutcome::DurabilityFailure(failure) = decode_tool_outcome(&first) else {
                panic!("recovered Pending must return a durability failure");
            };
            assert_eq!(
                failure.failure_kind,
                super::super::member_upcall::UpcallDurabilityFailureKind::Indeterminate
            );
            assert_eq!(
                seams.executions.load(Ordering::SeqCst),
                0,
                "recovered Pending may never execute {request_id}"
            );

            let second = serve_member_operator_request(&seams, &request, sender()).await;
            assert_eq!(first, second, "indeterminate terminal replays verbatim");
        }
    }

    /// A same-key delivery racing an already executing owner can only
    /// terminalize the durable Pending as indeterminate. The original owner
    /// must observe that terminal CAS winner and may never publish its later
    /// success over it; the effect executor still runs at most once.
    #[tokio::test]
    async fn concurrent_same_key_pending_has_one_executor_and_one_terminal_winner() {
        let gate = Arc::new(ExecutionGate {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let seams =
            Arc::new(SpySeams::new(vec![], Some(record())).with_execution_gate(Arc::clone(&gate)));
        let request = payload("b2", "req-concurrent", MemberOperatorOp::ListMembers);

        let first = tokio::spawn({
            let seams = Arc::clone(&seams);
            let request = request.clone();
            async move { serve_member_operator_request(seams.as_ref(), &request, sender()).await }
        });
        gate.entered.notified().await;

        let duplicate = serve_member_operator_request(seams.as_ref(), &request, sender()).await;
        let UpcallToolOutcome::DurabilityFailure(failure) = decode_tool_outcome(&duplicate) else {
            panic!("a concurrent Pending duplicate must become indeterminate");
        };
        assert_eq!(
            failure.failure_kind,
            super::super::member_upcall::UpcallDurabilityFailureKind::Indeterminate
        );

        gate.release.notify_one();
        let original = first.await.expect("original serving task");
        assert_eq!(
            original, duplicate,
            "the executing owner must return the durable terminal CAS winner"
        );
        assert_eq!(
            seams.executions.load(Ordering::SeqCst),
            1,
            "the request effect has exactly one executor"
        );

        let replay = serve_member_operator_request(seams.as_ref(), &request, sender()).await;
        assert_eq!(replay, duplicate, "the winning terminal remains immutable");
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);
    }

    /// Once an effect has run, terminal persistence failure must never leak a
    /// success. The durable row remains Pending; the next delivery converts
    /// it to one stable indeterminate terminal without re-executing.
    #[tokio::test]
    async fn post_effect_store_failure_leaves_pending_then_stable_indeterminate() {
        let seams = SpySeams::new(vec![], Some(record())).with_terminalize_failures(1);
        let request = payload(
            "b2",
            "req-post-effect-store-failure",
            MemberOperatorOp::ListMembers,
        );

        let first = serve_member_operator_request(&seams, &request, sender()).await;
        assert!(
            matches!(
                first.outcome,
                MemberOperatorOutcome::Rejected {
                    cause: BridgeRejectionCause::Unavailable,
                    ..
                }
            ),
            "unpersisted terminal may never be reported as Completed"
        );
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);
        let rows = seams
            .ledger
            .list_member_operator_requests(&seams.mob_id)
            .await
            .expect("list durable Pending");
        assert_eq!(rows.len(), 1);
        assert!(matches!(
            rows[0].state,
            MobMemberOperatorRequestState::Pending
        ));

        let recovered = serve_member_operator_request(&seams, &request, sender()).await;
        let UpcallToolOutcome::DurabilityFailure(failure) = decode_tool_outcome(&recovered) else {
            panic!("recovered Pending must become indeterminate");
        };
        assert_eq!(
            failure.failure_kind,
            super::super::member_upcall::UpcallDurabilityFailureKind::Indeterminate
        );
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);

        let replay = serve_member_operator_request(&seams, &request, sender()).await;
        assert_eq!(replay, recovered, "indeterminate terminal is immutable");
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);
    }

    /// Fence is part of the durable key, not merely an admission field. A
    /// reused request id under a same-generation refence cannot replay or
    /// conflict with the prior fence's terminal.
    #[tokio::test]
    async fn request_id_is_scoped_by_generation_and_fence() {
        let seams = SpySeams::new(vec![], Some(record()));
        let first = payload("b2", "req-refenced", MemberOperatorOp::ListMembers);
        let _first_reply = serve_member_operator_request(&seams, &first, sender()).await;

        let mut refenced = payload("b2", "req-refenced", MemberOperatorOp::MobListFlows);
        refenced.requester_fence_token = 2;
        let _second_reply = serve_member_operator_request(&seams, &refenced, sender()).await;

        assert_eq!(seams.executions.load(Ordering::SeqCst), 2);
        let rows = seams
            .ledger
            .list_member_operator_requests(&seams.mob_id)
            .await
            .expect("list request rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].fence_token, 1);
        assert_eq!(rows[1].fence_token, 2);
        assert_ne!(rows[0].op_digest, rows[1].op_digest);
    }

    /// One request key can never name two operations. The conflict does not
    /// overwrite the first terminal row, and neither the conflict nor a later
    /// original replay executes again.
    #[tokio::test]
    async fn conflicting_operation_digest_never_executes_or_overwrites_original() {
        let seams = SpySeams::new(vec![], Some(record()));
        let original = payload("b2", "req-conflict", MemberOperatorOp::ListMembers);
        let first = serve_member_operator_request(&seams, &original, sender()).await;
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);

        let conflicting = payload("b2", "req-conflict", MemberOperatorOp::MobListFlows);
        let conflict_reply = serve_member_operator_request(&seams, &conflicting, sender()).await;
        let UpcallToolOutcome::DurabilityFailure(failure) = decode_tool_outcome(&conflict_reply)
        else {
            panic!("digest conflict must return a durability failure");
        };
        assert_eq!(
            failure.failure_kind,
            super::super::member_upcall::UpcallDurabilityFailureKind::RequestConflict
        );
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);

        let replayed = serve_member_operator_request(&seams, &original, sender()).await;
        assert_eq!(
            first, replayed,
            "conflict must not replace original terminal"
        );
        assert_eq!(seams.executions.load(Ordering::SeqCst), 1);
    }

    /// Durable terminals are retained beyond the former 1,024-entry cache
    /// bound. Replaying the oldest request remains a no-execution hit.
    #[tokio::test]
    async fn durable_request_ledger_retains_more_than_1024_terminals() {
        const REQUESTS: usize = crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION + 1;
        let seams = SpySeams::new(vec![], Some(record()));
        let mut oldest = None;
        for index in 0..REQUESTS {
            let mut request = payload(
                "b2",
                &format!("req-retained-{index:04}"),
                MemberOperatorOp::ListMembers,
            );
            if index == crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION {
                // The durable ledger is globally wider than the former cache,
                // while each exact identity/generation/fence incarnation
                // retains its independent fail-closed storage quota.
                request.requester_fence_token = 2;
            }
            let reply = serve_member_operator_request(&seams, &request, sender()).await;
            if index == 0 {
                oldest = Some(reply);
            }
        }
        assert_eq!(seams.executions.load(Ordering::SeqCst), REQUESTS);
        assert_eq!(
            seams
                .ledger
                .list_member_operator_requests(&seams.mob_id)
                .await
                .expect("list retained requests")
                .len(),
            REQUESTS,
            "no capacity eviction"
        );

        let oldest_request = payload("b2", "req-retained-0000", MemberOperatorOp::ListMembers);
        let replayed = serve_member_operator_request(&seams, &oldest_request, sender()).await;
        assert_eq!(oldest.expect("oldest reply"), replayed);
        assert_eq!(seams.executions.load(Ordering::SeqCst), REQUESTS);
    }
}

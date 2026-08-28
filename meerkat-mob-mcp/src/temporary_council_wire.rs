//! The ONE wire↔domain conversion site for temporary councils.
//!
//! Every shipping surface — JSON-RPC (`mob/temporary_council_*`), REST
//! (`/mob/temporary-councils/*`), the CLI (`rkat mob council-*`), and the
//! owner-local public MCP tools — renders through this module. No surface
//! serializes a domain council type directly, and no surface re-derives council
//! semantics: [`TemporaryCouncilCoordinator`](crate::TemporaryCouncilCoordinator)
//! stays the only lifecycle owner.
//!
//! # Direction contracts
//!
//! * **wire → domain** validates. The caller's mob definition template goes
//!   through the ordinary public decoder ([`decode_public_mob_definition`]), ids
//!   go through their validating constructors, and numeric widths are checked
//!   rather than truncated. Structured-contract JSON Schema *compilation* is
//!   deliberately NOT done here: it stays a coordinator preflight, so a bad
//!   schema is refused by the same authority that would later validate against
//!   it.
//! * **domain → wire** strips. The durable custody record holds a full
//!   [`ForkedParticipantRef`](meerkat_mob::forked_participant::ForkedParticipantRef)
//!   including its bearer token; nothing in this module can project it, because
//!   no wire type has a field for it. Results and record projections carry
//!   non-secret provenance only.

use chrono::{DateTime, SecondsFormat, Utc};
use meerkat_contracts::wire::{
    MobTemporaryCouncilRecoverResult, MobTemporaryCouncilRunResult, WireHostBindingDescriptor,
    WireMobBackendKind, WireOpaqueJson, WireTemporaryCouncilAcquisition,
    WireTemporaryCouncilArtifactClaim, WireTemporaryCouncilBounds,
    WireTemporaryCouncilCapabilityProvenance, WireTemporaryCouncilClaimDetail,
    WireTemporaryCouncilCleanup, WireTemporaryCouncilCleanupDebt,
    WireTemporaryCouncilCleanupStatus, WireTemporaryCouncilConflictDetail,
    WireTemporaryCouncilDeadline, WireTemporaryCouncilDurability,
    WireTemporaryCouncilDurabilityDetail, WireTemporaryCouncilErrorDetail,
    WireTemporaryCouncilExchange, WireTemporaryCouncilExchangeOutcome,
    WireTemporaryCouncilExitReason, WireTemporaryCouncilFailureDetail,
    WireTemporaryCouncilFailureKind, WireTemporaryCouncilMergeBack,
    WireTemporaryCouncilMergeOutcome, WireTemporaryCouncilMergePolicyKind,
    WireTemporaryCouncilOwnerRoute, WireTemporaryCouncilParticipant,
    WireTemporaryCouncilParticipantCustody, WireTemporaryCouncilParticipantProvenance,
    WireTemporaryCouncilRecord, WireTemporaryCouncilRecoveryReport, WireTemporaryCouncilRequest,
    WireTemporaryCouncilResult, WireTemporaryCouncilReusePolicy, WireTemporaryCouncilScope,
    WireTemporaryCouncilSelectedExchange, WireTemporaryCouncilSourceProvenance,
    WireTemporaryCouncilStructuredContract, WireTemporaryCouncilStructuredContractIdentity,
};
use meerkat_mob::forked_participant::{
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute, ForkedParticipantReusePolicy,
};
use meerkat_mob::store::TemporaryCouncilRecord;
use meerkat_mob::temporary_council::{
    TemporaryCouncilAcquisition, TemporaryCouncilArtifactClaim,
    TemporaryCouncilCapabilityProvenance, TemporaryCouncilCleanupReceipt,
    TemporaryCouncilCleanupStatus, TemporaryCouncilDurability, TemporaryCouncilExchangeOutcome,
    TemporaryCouncilExchangeReceipt, TemporaryCouncilExitReason, TemporaryCouncilId,
    TemporaryCouncilMergeOutcome, TemporaryCouncilMergePolicyKind,
    TemporaryCouncilParticipantCustody, TemporaryCouncilParticipantProvenance,
    TemporaryCouncilResult, TemporaryCouncilSelectedExchange,
    TemporaryCouncilStructuredContractIdentity,
};
use meerkat_mob::{AgentIdentity, HostBindRequest, MobBackendKind, MobId, ProfileName};

use crate::decode_public_mob_definition;
use crate::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilDeadline, TemporaryCouncilError,
    TemporaryCouncilHostBootstrap, TemporaryCouncilOutcome, TemporaryCouncilParticipantSpec,
    TemporaryCouncilRecoveryReport, TemporaryCouncilRequest, TemporaryCouncilStructuredContract,
};

// ===========================================================================
// Small helpers
// ===========================================================================

fn invalid(detail: impl Into<String>) -> TemporaryCouncilError {
    TemporaryCouncilError::InvalidRequest {
        detail: detail.into(),
    }
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_timestamp(field: &str, raw: &str) -> Result<DateTime<Utc>, TemporaryCouncilError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| invalid(format!("{field} must be an RFC 3339 timestamp: {error}")))
}

fn to_usize(field: &str, raw: u64) -> Result<usize, TemporaryCouncilError> {
    usize::try_from(raw).map_err(|_| {
        invalid(format!(
            "{field} ({raw}) exceeds this platform's addressable size"
        ))
    })
}

/// Validate and wrap a caller-supplied council identity.
pub fn parse_temporary_council_id(raw: &str) -> Result<TemporaryCouncilId, TemporaryCouncilError> {
    TemporaryCouncilId::new(raw).map_err(|error| invalid(format!("invalid council id: {error}")))
}

// ===========================================================================
// wire → domain
// ===========================================================================

fn decode_scope(scope: WireTemporaryCouncilScope) -> ForkedParticipantOperationScope {
    match scope {
        WireTemporaryCouncilScope::Invoke => ForkedParticipantOperationScope::Invoke,
        WireTemporaryCouncilScope::Observe => ForkedParticipantOperationScope::Observe,
        WireTemporaryCouncilScope::InvokeAndObserve => {
            ForkedParticipantOperationScope::InvokeAndObserve
        }
    }
}

fn decode_backend(kind: WireMobBackendKind) -> MobBackendKind {
    match kind {
        WireMobBackendKind::Session => MobBackendKind::Session,
        WireMobBackendKind::External => MobBackendKind::External,
    }
}

fn decode_durability(durability: WireTemporaryCouncilDurability) -> TemporaryCouncilDurability {
    match durability {
        WireTemporaryCouncilDurability::Durable => TemporaryCouncilDurability::Durable,
        WireTemporaryCouncilDurability::ProcessBound => TemporaryCouncilDurability::ProcessBound,
    }
}

fn decode_deadline(
    deadline: WireTemporaryCouncilDeadline,
) -> Result<TemporaryCouncilDeadline, TemporaryCouncilError> {
    Ok(match deadline {
        WireTemporaryCouncilDeadline::Absolute { at } => TemporaryCouncilDeadline::Absolute {
            at: parse_timestamp("bounds.deadline.at", &at)?,
        },
        WireTemporaryCouncilDeadline::Relative { after_millis } => {
            TemporaryCouncilDeadline::Relative {
                after: std::time::Duration::from_millis(after_millis),
            }
        }
    })
}

fn decode_structured_contract(
    contract: WireTemporaryCouncilStructuredContract,
) -> Result<TemporaryCouncilStructuredContract, TemporaryCouncilError> {
    // Only JSON well-formedness is checked here. Schema COMPILATION stays a
    // coordinator preflight so one authority owns both compilation and the
    // later validation of the finalizer's output against it.
    let json_schema = contract.json_schema.to_value().map_err(|error| {
        invalid(format!(
            "merge_back.contract.json_schema must be valid JSON: {error}"
        ))
    })?;
    Ok(TemporaryCouncilStructuredContract::new(
        contract.schema_id,
        contract.schema_version,
        json_schema,
    ))
}

fn decode_merge_back(
    merge_back: WireTemporaryCouncilMergeBack,
) -> Result<MergeBackPolicy, TemporaryCouncilError> {
    Ok(match merge_back {
        WireTemporaryCouncilMergeBack::BoundedTextSummary {
            finalizer,
            max_bytes,
        } => MergeBackPolicy::BoundedTextSummary {
            finalizer: AgentIdentity::from(finalizer.as_str()),
            max_bytes: to_usize("merge_back.max_bytes", max_bytes)?,
        },
        WireTemporaryCouncilMergeBack::StructuredResult {
            finalizer,
            contract,
            max_bytes,
        } => MergeBackPolicy::StructuredResult {
            finalizer: AgentIdentity::from(finalizer.as_str()),
            contract: decode_structured_contract(contract)?,
            max_bytes: to_usize("merge_back.max_bytes", max_bytes)?,
        },
        WireTemporaryCouncilMergeBack::SelectedTranscript {
            participant,
            exchange_sequences,
            max_bytes,
        } => MergeBackPolicy::SelectedTranscript {
            participant: AgentIdentity::from(participant.as_str()),
            exchange_sequences,
            max_bytes: to_usize("merge_back.max_bytes", max_bytes)?,
        },
        WireTemporaryCouncilMergeBack::DurableArtifactReference {
            participant,
            max_bytes,
        } => MergeBackPolicy::DurableArtifactReference {
            participant: AgentIdentity::from(participant.as_str()),
            max_bytes: to_usize("merge_back.max_bytes", max_bytes)?,
        },
        WireTemporaryCouncilMergeBack::NoMerge => MergeBackPolicy::NoMerge,
    })
}

fn decode_participant(
    participant: WireTemporaryCouncilParticipant,
) -> Result<TemporaryCouncilParticipantSpec, TemporaryCouncilError> {
    let prefix_message_count = participant
        .prefix_message_count
        .map(|count| to_usize("participants[].prefix_message_count", count))
        .transpose()?;
    Ok(TemporaryCouncilParticipantSpec {
        order: participant.order,
        role: participant.role,
        source_mob_id: MobId::from(participant.source_mob_id.as_str()),
        source_identity: AgentIdentity::from(participant.source_identity.as_str()),
        target_identity: AgentIdentity::from(participant.target_identity.as_str()),
        target_profile: ProfileName::from(participant.target_profile.as_str()),
        target_backend: participant.target_backend.map(decode_backend),
        prefix_message_count,
        scope: decode_scope(participant.scope),
    })
}

/// Validate one wire council request into its domain form.
///
/// The mob definition template is decoded through the ordinary public
/// definition decoder, so a council cannot smuggle a definition shape the
/// `mob/create` surface would refuse.
pub fn decode_temporary_council_request(
    request: WireTemporaryCouncilRequest,
) -> Result<TemporaryCouncilRequest, TemporaryCouncilError> {
    let council_id = parse_temporary_council_id(&request.council_id)?;
    let definition_template = decode_public_mob_definition(request.definition_template)
        .map_err(|error| invalid(format!("invalid mob definition template: {error}")))?;
    let participants = request
        .participants
        .into_iter()
        .map(decode_participant)
        .collect::<Result<Vec<_>, _>>()?;
    let bounds = TemporaryCouncilBounds {
        deadline: decode_deadline(request.bounds.deadline)?,
        max_rounds: request.bounds.max_rounds,
        max_exchanges: request.bounds.max_exchanges,
        max_result_bytes: to_usize("bounds.max_result_bytes", request.bounds.max_result_bytes)?,
    };
    Ok(TemporaryCouncilRequest {
        council_id,
        definition_template,
        participants,
        topic: request.topic,
        bounds,
        merge_back: decode_merge_back(request.merge_back)?,
        durability: decode_durability(request.durability),
    })
}

/// Decode the one-time host bootstrap a council needs to seat HOST-owned
/// participants.
///
/// Nothing here is fingerprinted or persisted: the descriptors carry one-time
/// ceremony tokens that belong to the ATTEMPT, not to the council identity.
pub fn decode_temporary_council_host_bootstrap(
    descriptors: Vec<WireHostBindingDescriptor>,
) -> Result<TemporaryCouncilHostBootstrap, TemporaryCouncilError> {
    if descriptors.is_empty() {
        return Ok(TemporaryCouncilHostBootstrap::none());
    }
    let bindings = descriptors
        .iter()
        .map(|descriptor| {
            HostBindRequest::from_descriptor(descriptor)
                .map_err(|error| invalid(format!("invalid host binding descriptor: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TemporaryCouncilHostBootstrap::none().with_host_bindings(bindings))
}

// ===========================================================================
// domain → wire
// ===========================================================================

fn encode_scope(scope: ForkedParticipantOperationScope) -> WireTemporaryCouncilScope {
    match scope {
        ForkedParticipantOperationScope::Invoke => WireTemporaryCouncilScope::Invoke,
        ForkedParticipantOperationScope::Observe => WireTemporaryCouncilScope::Observe,
        ForkedParticipantOperationScope::InvokeAndObserve => {
            WireTemporaryCouncilScope::InvokeAndObserve
        }
    }
}

fn encode_durability(durability: TemporaryCouncilDurability) -> WireTemporaryCouncilDurability {
    match durability {
        TemporaryCouncilDurability::Durable => WireTemporaryCouncilDurability::Durable,
        TemporaryCouncilDurability::ProcessBound => WireTemporaryCouncilDurability::ProcessBound,
        // The domain enum is `#[non_exhaustive]`; a future variant must not be
        // silently rendered as one of today's declarations.
        other => unreachable_durability(other),
    }
}

fn unreachable_durability(
    durability: TemporaryCouncilDurability,
) -> WireTemporaryCouncilDurability {
    // A future durability class has no honest projection. Report the strictest
    // existing declaration rather than claiming process-bound custody for
    // something that may be durable.
    tracing::error!(
        ?durability,
        "unknown temporary-council durability class projected to `durable`"
    );
    WireTemporaryCouncilDurability::Durable
}

fn encode_reuse(reuse: ForkedParticipantReusePolicy) -> WireTemporaryCouncilReusePolicy {
    match reuse {
        ForkedParticipantReusePolicy::OneShot => WireTemporaryCouncilReusePolicy::OneShot,
        ForkedParticipantReusePolicy::BoundedReuse { max_uses } => {
            WireTemporaryCouncilReusePolicy::BoundedReuse { max_uses }
        }
    }
}

fn encode_owner_route(route: &ForkedParticipantOwnerRoute) -> WireTemporaryCouncilOwnerRoute {
    match route {
        ForkedParticipantOwnerRoute::Local { realm_id } => WireTemporaryCouncilOwnerRoute::Local {
            realm_id: realm_id.as_str().to_string(),
        },
        ForkedParticipantOwnerRoute::Host { realm_id, host_id } => {
            WireTemporaryCouncilOwnerRoute::Host {
                realm_id: realm_id.as_str().to_string(),
                host_id: host_id.as_str().to_string(),
            }
        }
    }
}

fn encode_capability_provenance(
    provenance: &TemporaryCouncilCapabilityProvenance,
) -> WireTemporaryCouncilCapabilityProvenance {
    WireTemporaryCouncilCapabilityProvenance {
        owner_route: encode_owner_route(&provenance.owner_route),
        fork_session_id: provenance.fork_session_id.to_string(),
        source: WireTemporaryCouncilSourceProvenance {
            source_session_id: provenance.source_provenance.source_session_id.to_string(),
            prefix_message_count: u64::try_from(provenance.source_provenance.prefix_message_count)
                .unwrap_or(u64::MAX),
            prefix_digest: provenance.source_provenance.prefix_digest.clone(),
        },
        scope: encode_scope(provenance.scope),
        reuse: encode_reuse(provenance.reuse),
        expires_at: timestamp(provenance.expires_at),
        correlation_hint: provenance.correlation_hint.clone(),
    }
}

fn encode_participant_provenance(
    participant: &TemporaryCouncilParticipantProvenance,
) -> WireTemporaryCouncilParticipantProvenance {
    WireTemporaryCouncilParticipantProvenance {
        order: participant.order,
        role: participant.role.clone(),
        source_mob_id: participant.source_mob_id.to_string(),
        source_identity: participant.source_identity.to_string(),
        target_identity: participant.target_identity.to_string(),
        scope: encode_scope(participant.scope),
        capability_request_id: participant.capability_request_id.as_str().to_string(),
        capability: participant
            .capability
            .as_ref()
            .map(encode_capability_provenance),
        attachment_id: participant.attachment_id.as_str().to_string(),
        seated: participant.seated,
    }
}

fn encode_exchange_outcome(
    outcome: &TemporaryCouncilExchangeOutcome,
) -> WireTemporaryCouncilExchangeOutcome {
    match outcome {
        TemporaryCouncilExchangeOutcome::Pending => WireTemporaryCouncilExchangeOutcome::Pending,
        TemporaryCouncilExchangeOutcome::Completed {
            text,
            truncated,
            session_id,
            completed_at,
        } => WireTemporaryCouncilExchangeOutcome::Completed {
            text: text.clone(),
            truncated: *truncated,
            session_id: session_id.to_string(),
            completed_at: timestamp(*completed_at),
        },
        TemporaryCouncilExchangeOutcome::Failed { detail, failed_at } => {
            WireTemporaryCouncilExchangeOutcome::Failed {
                detail: detail.clone(),
                failed_at: timestamp(*failed_at),
            }
        }
        other => WireTemporaryCouncilExchangeOutcome::Failed {
            detail: format!("unrepresentable exchange outcome: {other:?}"),
            failed_at: timestamp(Utc::now()),
        },
    }
}

fn encode_exchange(exchange: &TemporaryCouncilExchangeReceipt) -> WireTemporaryCouncilExchange {
    WireTemporaryCouncilExchange {
        round: exchange.round,
        sequence: exchange.sequence,
        participant_order: exchange.participant_order,
        target_identity: exchange.target_identity.to_string(),
        delivery_idempotency_key: exchange.delivery_idempotency_key.clone(),
        delivery_correlation_id: exchange.delivery_correlation_id.clone(),
        started_at: timestamp(exchange.started_at),
        outcome: encode_exchange_outcome(&exchange.outcome),
    }
}

fn encode_exit_reason(reason: &TemporaryCouncilExitReason) -> WireTemporaryCouncilExitReason {
    match reason {
        TemporaryCouncilExitReason::Completed => WireTemporaryCouncilExitReason::Completed,
        TemporaryCouncilExitReason::MaxExchangesReached => {
            WireTemporaryCouncilExitReason::MaxExchangesReached
        }
        TemporaryCouncilExitReason::DeadlineExceeded => {
            WireTemporaryCouncilExitReason::DeadlineExceeded
        }
        TemporaryCouncilExitReason::ParticipantSeatingFailed {
            participant_order,
            detail,
        } => WireTemporaryCouncilExitReason::ParticipantSeatingFailed {
            participant_order: *participant_order,
            detail: detail.clone(),
        },
        TemporaryCouncilExitReason::WiringIncomplete { detail } => {
            WireTemporaryCouncilExitReason::WiringIncomplete {
                detail: detail.clone(),
            }
        }
        TemporaryCouncilExitReason::ExchangeFailed {
            round,
            target_identity,
            detail,
        } => WireTemporaryCouncilExitReason::ExchangeFailed {
            round: *round,
            target_identity: target_identity.to_string(),
            detail: detail.clone(),
        },
        TemporaryCouncilExitReason::CoordinatorInterrupted => {
            WireTemporaryCouncilExitReason::CoordinatorInterrupted
        }
        other => WireTemporaryCouncilExitReason::WiringIncomplete {
            detail: format!("unrepresentable exit reason: {other:?}"),
        },
    }
}

fn encode_merge_policy_kind(
    kind: TemporaryCouncilMergePolicyKind,
) -> WireTemporaryCouncilMergePolicyKind {
    match kind {
        TemporaryCouncilMergePolicyKind::BoundedTextSummary => {
            WireTemporaryCouncilMergePolicyKind::BoundedTextSummary
        }
        TemporaryCouncilMergePolicyKind::StructuredResult => {
            WireTemporaryCouncilMergePolicyKind::StructuredResult
        }
        TemporaryCouncilMergePolicyKind::SelectedTranscript => {
            WireTemporaryCouncilMergePolicyKind::SelectedTranscript
        }
        TemporaryCouncilMergePolicyKind::DurableArtifactReference => {
            WireTemporaryCouncilMergePolicyKind::DurableArtifactReference
        }
        TemporaryCouncilMergePolicyKind::NoMerge => WireTemporaryCouncilMergePolicyKind::NoMerge,
        _ => WireTemporaryCouncilMergePolicyKind::NoMerge,
    }
}

fn encode_contract_identity(
    identity: &TemporaryCouncilStructuredContractIdentity,
) -> WireTemporaryCouncilStructuredContractIdentity {
    WireTemporaryCouncilStructuredContractIdentity {
        schema_id: identity.schema_id.clone(),
        schema_version: identity.schema_version,
        schema_digest: identity.schema_digest.clone(),
    }
}

fn encode_artifact_claim(
    claim: &TemporaryCouncilArtifactClaim,
) -> WireTemporaryCouncilArtifactClaim {
    WireTemporaryCouncilArtifactClaim {
        uri: claim.uri.clone(),
        media_type: claim.media_type.clone(),
        digest: claim.digest.clone(),
        byte_len: claim.byte_len,
    }
}

fn encode_selected_exchange(
    excerpt: &TemporaryCouncilSelectedExchange,
) -> WireTemporaryCouncilSelectedExchange {
    WireTemporaryCouncilSelectedExchange {
        sequence: excerpt.sequence,
        round: excerpt.round,
        participant_order: excerpt.participant_order,
        target_identity: excerpt.target_identity.to_string(),
        text: excerpt.text.clone(),
        truncated: excerpt.truncated,
    }
}

fn encode_merge_outcome(merge: &TemporaryCouncilMergeOutcome) -> WireTemporaryCouncilMergeOutcome {
    match merge {
        TemporaryCouncilMergeOutcome::NoMerge {
            confirmed_participants,
        } => WireTemporaryCouncilMergeOutcome::NoMerge {
            confirmed_participants: confirmed_participants
                .iter()
                .map(ToString::to_string)
                .collect(),
        },
        TemporaryCouncilMergeOutcome::BoundedTextSummary {
            finalizer,
            text,
            truncated,
        } => WireTemporaryCouncilMergeOutcome::BoundedTextSummary {
            finalizer: finalizer.to_string(),
            text: text.clone(),
            truncated: *truncated,
        },
        TemporaryCouncilMergeOutcome::StructuredResult {
            finalizer,
            contract,
            value,
            truncated,
        } => WireTemporaryCouncilMergeOutcome::StructuredResult {
            finalizer: finalizer.to_string(),
            contract: encode_contract_identity(contract),
            value: WireOpaqueJson::from_value(value),
            truncated: *truncated,
        },
        TemporaryCouncilMergeOutcome::SelectedTranscript {
            participant,
            exchange_sequences,
            excerpts,
            truncated,
        } => WireTemporaryCouncilMergeOutcome::SelectedTranscript {
            participant: participant.to_string(),
            exchange_sequences: exchange_sequences.clone(),
            excerpts: excerpts.iter().map(encode_selected_exchange).collect(),
            truncated: *truncated,
        },
        TemporaryCouncilMergeOutcome::DurableArtifactReference { participant, claim } => {
            WireTemporaryCouncilMergeOutcome::DurableArtifactReference {
                participant: participant.to_string(),
                claim: encode_artifact_claim(claim),
            }
        }
        TemporaryCouncilMergeOutcome::NotAttempted { reason } => {
            WireTemporaryCouncilMergeOutcome::NotAttempted {
                reason: reason.clone(),
            }
        }
        TemporaryCouncilMergeOutcome::Failed { policy, detail } => {
            WireTemporaryCouncilMergeOutcome::Failed {
                policy: encode_merge_policy_kind(*policy),
                detail: detail.clone(),
            }
        }
        other => WireTemporaryCouncilMergeOutcome::NotAttempted {
            reason: format!("unrepresentable merge outcome: {other:?}"),
        },
    }
}

/// Project one immutable council result onto the wire.
pub fn encode_temporary_council_result(
    result: &TemporaryCouncilResult,
) -> WireTemporaryCouncilResult {
    WireTemporaryCouncilResult {
        council_id: result.council_id.as_str().to_string(),
        request_fingerprint: result.request_fingerprint.clone(),
        temporary_mob_id: result.temporary_mob_id.to_string(),
        exit_reason: encode_exit_reason(&result.exit_reason),
        rounds_completed: result.rounds_completed,
        exchanges: result.exchanges.iter().map(encode_exchange).collect(),
        merge: encode_merge_outcome(&result.merge),
        participants: result
            .participants
            .iter()
            .map(encode_participant_provenance)
            .collect(),
        truncated_exchange_count: result.truncated_exchange_count,
        merge_truncated: result.merge_truncated,
        durability: encode_durability(result.durability),
        concluded_at: timestamp(result.concluded_at),
    }
}

fn encode_cleanup_status(
    status: TemporaryCouncilCleanupStatus,
) -> WireTemporaryCouncilCleanupStatus {
    match status {
        TemporaryCouncilCleanupStatus::Settled => WireTemporaryCouncilCleanupStatus::Settled,
        TemporaryCouncilCleanupStatus::Debt => WireTemporaryCouncilCleanupStatus::Debt,
        TemporaryCouncilCleanupStatus::Pending => WireTemporaryCouncilCleanupStatus::Pending,
        // An unknown status must never read as `settled`: unsettled is the
        // fail-closed answer for an obligation this build cannot classify.
        _ => WireTemporaryCouncilCleanupStatus::Pending,
    }
}

/// Project one cleanup receipt onto the wire, carrying the machine's own
/// settlement verdict rather than a surface re-derivation.
pub fn encode_temporary_council_cleanup(
    cleanup: &TemporaryCouncilCleanupReceipt,
) -> WireTemporaryCouncilCleanup {
    WireTemporaryCouncilCleanup {
        status: encode_cleanup_status(cleanup.status()),
        attempted_at: timestamp(cleanup.attempted_at),
        attempts: cleanup.attempts,
        temporary_mob_destroyed: cleanup.temporary_mob_destroyed,
        released_participants: cleanup.released_participants.clone(),
        revoked_participants: cleanup.revoked_participants.clone(),
        debts: cleanup
            .debts
            .iter()
            .map(|debt| WireTemporaryCouncilCleanupDebt {
                subject: debt.subject.clone(),
                detail: debt.detail.clone(),
            })
            .collect(),
        budget_exhausted: cleanup.budget_exhausted,
    }
}

/// Project one council outcome (immutable result + separate cleanup verdict).
pub fn encode_temporary_council_outcome(
    outcome: &TemporaryCouncilOutcome,
) -> MobTemporaryCouncilRunResult {
    MobTemporaryCouncilRunResult {
        result: encode_temporary_council_result(&outcome.result),
        cleanup: encode_temporary_council_cleanup(&outcome.cleanup),
        replayed: outcome.replayed,
    }
}

fn encode_acquisition(acquisition: TemporaryCouncilAcquisition) -> WireTemporaryCouncilAcquisition {
    match acquisition {
        TemporaryCouncilAcquisition::NotAttempted => WireTemporaryCouncilAcquisition::NotAttempted,
        TemporaryCouncilAcquisition::Pending => WireTemporaryCouncilAcquisition::Pending,
        TemporaryCouncilAcquisition::Acquired => WireTemporaryCouncilAcquisition::Acquired,
        TemporaryCouncilAcquisition::Ambiguous => WireTemporaryCouncilAcquisition::Ambiguous,
        // An unknown acquisition class must not read as "nothing was created".
        _ => WireTemporaryCouncilAcquisition::Ambiguous,
    }
}

fn encode_custody(
    custody: &TemporaryCouncilParticipantCustody,
) -> WireTemporaryCouncilParticipantCustody {
    // `custody.capability_ref` is the FULL capability reference, bearer token
    // included. It has no wire field and is deliberately not read here.
    WireTemporaryCouncilParticipantCustody {
        order: custody.order,
        role: custody.role.clone(),
        source_mob_id: custody.source_mob_id.to_string(),
        source_identity: custody.source_identity.to_string(),
        target_identity: custody.target_identity.to_string(),
        target_profile: custody.target_profile.to_string(),
        scope: encode_scope(custody.scope),
        capability_request_id: custody.capability_request_id.as_str().to_string(),
        capability_correlation_hint: custody.capability_correlation_hint.clone(),
        attachment_id: custody.attachment_id.as_str().to_string(),
        acquisition: encode_acquisition(custody.acquisition),
        seated: custody.seated,
        seated_session_id: custody.seated_session_id.as_ref().map(ToString::to_string),
    }
}

/// Project one durable council record onto the sealed wire projection.
///
/// Store revision, persisted machine state, and the coordinator claim lease are
/// deliberately absent: they are coordinator authority, not caller-observable
/// state. `unfinished` is the canonical machine's own verdict, never a surface
/// re-derivation of a phase.
pub fn encode_temporary_council_record(
    record: &TemporaryCouncilRecord,
) -> WireTemporaryCouncilRecord {
    WireTemporaryCouncilRecord {
        council_id: record.council_id.as_str().to_string(),
        request_fingerprint: record.request_fingerprint.clone(),
        temporary_mob_id: record.temporary_mob_id.to_string(),
        deadline: timestamp(record.deadline),
        durability: encode_durability(record.durability),
        unfinished: record.is_unfinished(),
        participants: record.participants.iter().map(encode_custody).collect(),
        exchanges: record.exchanges.iter().map(encode_exchange).collect(),
        result: record.result.as_ref().map(encode_temporary_council_result),
        cleanup: record
            .cleanup
            .as_ref()
            .map(encode_temporary_council_cleanup),
        created_at: timestamp(record.created_at),
        updated_at: timestamp(record.updated_at),
    }
}

/// Project one recovery sweep's reports.
pub fn encode_temporary_council_recovery(
    reports: &[TemporaryCouncilRecoveryReport],
) -> MobTemporaryCouncilRecoverResult {
    MobTemporaryCouncilRecoverResult {
        reports: reports
            .iter()
            .map(|report| WireTemporaryCouncilRecoveryReport {
                council_id: report.council_id.as_str().to_string(),
                sealed_interrupted_result: report.sealed_interrupted_result,
                settled: report.settled,
                cleanup: encode_temporary_council_cleanup(&report.cleanup),
            })
            .collect(),
    }
}

// ===========================================================================
// Errors
// ===========================================================================

/// Map one typed council failure onto its stable wire code + `data` payload.
///
/// Cleanup debt is NOT mapped here: a sealed result with outstanding cleanup is
/// a success carrying [`WireTemporaryCouncilCleanupStatus::Debt`] or `Pending`.
pub fn temporary_council_error_detail(
    error: &TemporaryCouncilError,
) -> WireTemporaryCouncilErrorDetail {
    match error {
        TemporaryCouncilError::InvalidRequest { detail } => {
            WireTemporaryCouncilErrorDetail::InvalidRequest(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::InvalidRequest,
                detail: detail.clone(),
            })
        }
        TemporaryCouncilError::ConflictingRequest {
            council_id,
            stored_fingerprint,
            presented_fingerprint,
        } => WireTemporaryCouncilErrorDetail::ConflictingRequest(
            WireTemporaryCouncilConflictDetail {
                council_id: council_id.as_str().to_string(),
                stored_fingerprint: stored_fingerprint.clone(),
                presented_fingerprint: presented_fingerprint.clone(),
            },
        ),
        TemporaryCouncilError::HeldByAnotherCoordinator {
            council_id,
            current_claim_epoch,
        } => WireTemporaryCouncilErrorDetail::HeldByAnotherCoordinator(
            WireTemporaryCouncilClaimDetail {
                council_id: council_id.as_str().to_string(),
                current_claim_epoch: *current_claim_epoch,
            },
        ),
        TemporaryCouncilError::Fenced {
            council_id,
            current_claim_epoch,
        } => WireTemporaryCouncilErrorDetail::Fenced(WireTemporaryCouncilClaimDetail {
            council_id: council_id.as_str().to_string(),
            current_claim_epoch: *current_claim_epoch,
        }),
        // `satisfies` fails only for a `Durable` declaration against a
        // process-bound store, so the available class is exact, not a guess.
        TemporaryCouncilError::DurabilityUnavailable { council_id } => {
            WireTemporaryCouncilErrorDetail::DurabilityUnavailable(
                WireTemporaryCouncilDurabilityDetail {
                    council_id: council_id.as_str().to_string(),
                    required: WireTemporaryCouncilDurability::Durable,
                    available: WireTemporaryCouncilDurability::ProcessBound,
                },
            )
        }
        TemporaryCouncilError::Store { detail } => {
            WireTemporaryCouncilErrorDetail::Coordinator(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::Store,
                detail: detail.clone(),
            })
        }
        TemporaryCouncilError::Lifecycle { detail } => {
            WireTemporaryCouncilErrorDetail::Coordinator(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::Lifecycle,
                detail: detail.clone(),
            })
        }
        // The domain's `Mob` variant flattens its cause to a string, so there
        // is no typed `MobError` left to reuse the mob wire mapping on. It is
        // reported as a coordinator fault with an explicit `mob` kind rather
        // than being dressed up as a classified mob console error.
        TemporaryCouncilError::Mob { detail } => {
            WireTemporaryCouncilErrorDetail::Coordinator(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::Mob,
                detail: detail.clone(),
            })
        }
        TemporaryCouncilError::CoordinatorUnavailable { detail } => {
            WireTemporaryCouncilErrorDetail::Coordinator(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::CoordinatorUnavailable,
                detail: detail.clone(),
            })
        }
    }
}

/// Render one typed council failure as a public-MCP tool error.
///
/// Uses the SAME code/detail pairing the RPC and REST surfaces render, so a
/// tool caller and a console caller cannot be told different things about the
/// same failure.
pub fn temporary_council_tool_error(error: &TemporaryCouncilError) -> crate::McpToolError {
    let detail = temporary_council_error_detail(error);
    crate::McpToolError {
        code: detail.code().jsonrpc_code(),
        message: error.to_string(),
        data: detail.detail_value().ok(),
    }
}

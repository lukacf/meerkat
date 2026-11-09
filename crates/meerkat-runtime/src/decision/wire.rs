//! Projection of durable decisions onto their wire contracts. It lives in
//! the runtime, beside the decision service, so every surface reports a
//! decision, a listing and a refusal the same way. A decision's
//! authentication scope is never projected.

use std::num::NonZeroU32;

use meerkat_contracts::{
    DecisionAudienceRequest, DecisionListResult, DecisionPresentationRequest, WireCanonicalPart,
    WireDecision, WireDecisionAssurance, WireDecisionEffect, WireDecisionEvidence,
    WireDecisionNoticeState, WireDecisionOutcome, WireDecisionPhase, WireDecisionRefusal,
    WireDecisionReviewVerdict, WireDecisionStage, WireDecisionTier, WirePresentationAudience,
    WirePresentationLink, WirePresentationRecord, WireUnreadableDecision,
};
use meerkat_core::ApprovalAssurance;
use meerkat_core::approval::decision::{
    ApproverId, CanonicalPart, DecisionContractError, DecisionEvidence, DecisionNotice,
    DecisionOutcome, DecisionPhase, DecisionPrincipalKey, DecisionReference, DecisionReviewVerdict,
    DecisionStage, DecisionTier, PresentationAudience, PresentationLink, PresentationRecord,
    Sha256Digest, StoredDecision, UnreadableDecision,
};

use super::effect::{DecisionEffect, decision_effect};
use super::service::{DecisionListing, DecisionRefusal};

/// One decision as its row stands, with its held request when
/// `include_request`, and, once it is executed, the effect its kind names.
#[must_use]
pub fn wire_decision(row: &StoredDecision, include_request: bool) -> WireDecision {
    let descriptor = &row.descriptor;
    let state = &row.state;
    WireDecision {
        decision_id: row.decision_id.to_string(),
        kind: descriptor.kind.to_string(),
        subject_key: descriptor.subject_key.to_string(),
        operation_id: descriptor.operation_id.to_string(),
        request_digest: descriptor.request_digest.to_string(),
        requester: descriptor.requester.to_string(),
        approver_id: descriptor.approver_id.to_string(),
        reviewer: descriptor.reviewer.as_ref().map(ToString::to_string),
        phase: wire_decision_phase(state.phase),
        stage: match state.stage {
            DecisionStage::Human => WireDecisionStage::Human,
            DecisionStage::Review => WireDecisionStage::Review,
        },
        notice: match state.notice {
            DecisionNotice::NotRequired => WireDecisionNoticeState::NotRequired,
            DecisionNotice::NotDue => WireDecisionNoticeState::NotDue,
            DecisionNotice::Owed => WireDecisionNoticeState::Owed,
            DecisionNotice::Delivered => WireDecisionNoticeState::Delivered,
            DecisionNotice::Failed => WireDecisionNoticeState::Failed,
        },
        deadline_at_ms: descriptor.deadline_at_ms,
        created_at_ms: descriptor.created_at_ms,
        subject_digest: descriptor.subject_digest.to_string(),
        renderer: descriptor.renderer.to_string(),
        first_presented_to_approver_at_ms: state.first_presented_to_approver_at_ms,
        decided_at_ms: state.decided_at_ms,
        ended_at_ms: state.ended_at_ms,
        permit_issued: state.permit_issued,
        presentations: row.presentations.iter().map(wire_presentation).collect(),
        outcome: row.outcome.as_ref().map(wire_decision_outcome),
        execution_refusal: row.execution_refusal.as_ref().map(ToString::to_string),
        effect: decision_effect(row).map(|effect| wire_decision_effect(&effect)),
        revision: row.revision,
        request: include_request
            .then(|| serde_json::from_str(descriptor.request.get()).ok())
            .flatten(),
    }
}

/// A decision phase.
#[must_use]
pub fn wire_decision_phase(phase: DecisionPhase) -> WireDecisionPhase {
    match phase {
        DecisionPhase::Open => WireDecisionPhase::Open,
        DecisionPhase::Approved => WireDecisionPhase::Approved,
        DecisionPhase::Rejected => WireDecisionPhase::Rejected,
        DecisionPhase::Expired => WireDecisionPhase::Expired,
        DecisionPhase::Withdrawn => WireDecisionPhase::Withdrawn,
        DecisionPhase::Executed => WireDecisionPhase::Executed,
        DecisionPhase::ExecutionRefused => WireDecisionPhase::ExecutionRefused,
    }
}

/// The phase a wire phase names; `None` for one this version does not know.
#[must_use]
pub fn decision_phase_from_wire(phase: WireDecisionPhase) -> Option<DecisionPhase> {
    match phase {
        WireDecisionPhase::Open => Some(DecisionPhase::Open),
        WireDecisionPhase::Approved => Some(DecisionPhase::Approved),
        WireDecisionPhase::Rejected => Some(DecisionPhase::Rejected),
        WireDecisionPhase::Expired => Some(DecisionPhase::Expired),
        WireDecisionPhase::Withdrawn => Some(DecisionPhase::Withdrawn),
        WireDecisionPhase::Executed => Some(DecisionPhase::Executed),
        WireDecisionPhase::ExecutionRefused => Some(DecisionPhase::ExecutionRefused),
        WireDecisionPhase::Unknown => None,
    }
}

/// One page of decisions, and the rows of the page the server cannot read.
#[must_use]
pub fn wire_decision_listing(listing: &DecisionListing) -> DecisionListResult {
    DecisionListResult {
        decisions: listing
            .decisions
            .iter()
            .map(|row| wire_decision(row, false))
            .collect(),
        unreadable: listing
            .unreadable
            .iter()
            .map(wire_unreadable_decision)
            .collect(),
        next_after: listing.next_after.clone(),
    }
}

/// A row the server cannot read.
#[must_use]
pub fn wire_unreadable_decision(row: &UnreadableDecision) -> WireUnreadableDecision {
    WireUnreadableDecision {
        decision_id: row.decision_id.clone(),
        detail: row.reason.clone(),
    }
}

/// A decision's refusal.
#[must_use]
pub fn wire_decision_refusal(refusal: DecisionRefusal) -> WireDecisionRefusal {
    match refusal {
        DecisionRefusal::InvalidWindow => WireDecisionRefusal::InvalidWindow,
        DecisionRefusal::SelfReview => WireDecisionRefusal::SelfReview,
        DecisionRefusal::HoldConflict => WireDecisionRefusal::HoldConflict,
        DecisionRefusal::NotOpen => WireDecisionRefusal::NotOpen,
        DecisionRefusal::Expired => WireDecisionRefusal::Expired,
        DecisionRefusal::SubjectChanged => WireDecisionRefusal::SubjectChanged,
        DecisionRefusal::PresentationIncomplete => WireDecisionRefusal::PresentationIncomplete,
        DecisionRefusal::WrongAudience => WireDecisionRefusal::WrongAudience,
        DecisionRefusal::ReviewStage => WireDecisionRefusal::ReviewStage,
        DecisionRefusal::HumanStage => WireDecisionRefusal::HumanStage,
        DecisionRefusal::EvidenceRequired => WireDecisionRefusal::EvidenceRequired,
        DecisionRefusal::NotEligible => WireDecisionRefusal::NotEligible,
        DecisionRefusal::SelfApproval => WireDecisionRefusal::SelfApproval,
        DecisionRefusal::WrongReviewer => WireDecisionRefusal::WrongReviewer,
        DecisionRefusal::NotRequester => WireDecisionRefusal::NotRequester,
        DecisionRefusal::NotApproved => WireDecisionRefusal::NotApproved,
        DecisionRefusal::BindingMismatch => WireDecisionRefusal::BindingMismatch,
        DecisionRefusal::NotAuthorized => WireDecisionRefusal::NotAuthorized,
        DecisionRefusal::SettlementOwed => WireDecisionRefusal::SettlementOwed,
        DecisionRefusal::NotOwed => WireDecisionRefusal::NotOwed,
        DecisionRefusal::DecidedBeforePresentation => {
            WireDecisionRefusal::DecidedBeforePresentation
        }
    }
}

/// One canonical part.
#[must_use]
pub fn wire_canonical_part(part: &CanonicalPart) -> WireCanonicalPart {
    WireCanonicalPart {
        index: part.index,
        count: part.count,
        part_digest: part.part_digest.to_string(),
    }
}

/// One recorded presentation.
#[must_use]
pub fn wire_presentation(record: &PresentationRecord) -> WirePresentationRecord {
    WirePresentationRecord {
        audience: match &record.audience {
            PresentationAudience::Approver { approver_id } => WirePresentationAudience::Approver {
                approver_id: approver_id.to_string(),
            },
            PresentationAudience::Reviewer { reviewer } => WirePresentationAudience::Reviewer {
                reviewer: reviewer.to_string(),
            },
        },
        subject_digest: record.subject_digest.to_string(),
        max_part_bytes: record.max_part_bytes.map(NonZeroU32::get),
        parts_shown: record.parts_shown.iter().map(wire_canonical_part).collect(),
        link: record.link.as_ref().map(|link| WirePresentationLink {
            reference: link.reference.to_string(),
            served_digest: link.served_digest.to_string(),
        }),
        channel_ref: record.channel_ref.to_string(),
        message_ref: record.message_ref.to_string(),
        at_ms: record.at_ms,
    }
}

fn wire_decision_outcome(outcome: &DecisionOutcome) -> WireDecisionOutcome {
    WireDecisionOutcome {
        tier: match outcome.tier {
            DecisionTier::Human => WireDecisionTier::Human,
            DecisionTier::Review => WireDecisionTier::Review,
            _ => WireDecisionTier::Unknown,
        },
        evidence: wire_decision_evidence(&outcome.evidence),
    }
}

fn wire_decision_effect(effect: &DecisionEffect) -> WireDecisionEffect {
    match effect {
        DecisionEffect::Grant { grant_id } => WireDecisionEffect {
            grant_id: Some(grant_id.as_str().to_owned()),
        },
    }
}

fn wire_decision_evidence(evidence: &DecisionEvidence) -> WireDecisionEvidence {
    match evidence {
        DecisionEvidence::Human {
            decider,
            assurance,
            ingress,
            event_ref,
            observed_at_ms,
            approver_id,
            presented_digest,
            presentation,
            approve,
            reason,
            ..
        } => WireDecisionEvidence::Human {
            decider: decider.to_string(),
            assurance: match assurance {
                ApprovalAssurance::PersonAuthenticated => {
                    WireDecisionAssurance::PersonAuthenticated
                }
                ApprovalAssurance::ServiceCredential => WireDecisionAssurance::ServiceCredential,
                ApprovalAssurance::DeviceTrusted => WireDecisionAssurance::DeviceTrusted,
            },
            ingress: ingress.to_string(),
            event_ref: event_ref.to_string(),
            observed_at_ms: *observed_at_ms,
            approver_id: approver_id.to_string(),
            presented_digest: presented_digest.to_string(),
            presentation: presentation.as_ref().map(wire_presentation),
            approve: *approve,
            reason: reason.clone(),
        },
        DecisionEvidence::Review {
            reviewer,
            origin_session,
            verdict_call_ref,
            verdict,
            presented_digest,
            feedback,
            ..
        } => WireDecisionEvidence::Review {
            reviewer: reviewer.to_string(),
            origin_session: origin_session.to_string(),
            verdict_call_ref: verdict_call_ref.to_string(),
            verdict: match verdict {
                DecisionReviewVerdict::Allow => WireDecisionReviewVerdict::Allow,
                DecisionReviewVerdict::Deny => WireDecisionReviewVerdict::Deny,
                DecisionReviewVerdict::Escalate => WireDecisionReviewVerdict::Escalate,
            },
            presented_digest: presented_digest.to_string(),
            feedback: feedback.clone(),
        },
        _ => WireDecisionEvidence::Unknown,
    }
}

/// The presentation record a host presenter's request states, with the link
/// a summary carried when the request names one.
///
/// # Errors
/// [`DecisionContractError::Invalid`] for a value that is not valid: an
/// approver id, principal key, digest or reference (a link's included), or
/// a part size of 0.
pub fn presentation_from_wire(
    request: &DecisionPresentationRequest,
) -> Result<PresentationRecord, DecisionContractError> {
    let audience = match &request.audience {
        DecisionAudienceRequest::Approver { approver_id } => PresentationAudience::Approver {
            approver_id: ApproverId::new(approver_id.clone())?,
        },
        DecisionAudienceRequest::Reviewer { reviewer } => PresentationAudience::Reviewer {
            reviewer: DecisionPrincipalKey::new(reviewer.clone())?,
        },
    };
    let max_part_bytes = match request.max_part_bytes {
        None => None,
        Some(bytes) => Some(
            NonZeroU32::new(bytes).ok_or(DecisionContractError::Invalid { what: "part size" })?,
        ),
    };
    let parts_shown = request
        .parts_shown
        .iter()
        .map(|part| {
            Ok(CanonicalPart::new(
                part.index,
                part.count,
                Sha256Digest::new(part.part_digest.clone())?,
            ))
        })
        .collect::<Result<Vec<_>, DecisionContractError>>()?;
    let record = PresentationRecord::new(
        audience,
        Sha256Digest::new(request.subject_digest.clone())?,
        max_part_bytes,
        parts_shown,
        DecisionReference::new(request.channel_ref.clone())?,
        DecisionReference::new(request.message_ref.clone())?,
        request.at_ms,
    );
    Ok(match &request.link {
        None => record,
        Some(link) => record.with_link(PresentationLink::new(
            DecisionReference::new(link.reference.clone())?,
            Sha256Digest::new(link.served_digest.clone())?,
        )),
    })
}

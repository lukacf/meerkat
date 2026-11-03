//! Attested decisions, attributed reviews and their durable evidence.
//!
//! [`AttestedDecision`] and [`AttributedReview`] are not `Deserialize`: the
//! trusted host adapter that authenticated a human's event, or attributed a
//! reviewer's verdict to its origin session, builds them, and no wire
//! surface builds them from caller text. What a decision stores is their
//! [`DecisionEvidence`], and the machine binds the decision to the digest of
//! that evidence.

use serde::{Deserialize, Serialize};

use super::{
    ApproverId, DecisionContractError, DecisionIngress, DecisionPrincipalKey, DecisionReference,
    PresentationRecord, Sha256Digest,
};
use crate::approval::ApprovalAssurance;
use crate::approval::consent::AuthenticatedApprover;
use crate::{PrincipalKind, PrincipalRef, SessionId};

/// Largest decision reason or review feedback, in bytes.
pub const DECISION_REASON_MAX_BYTES: usize = 4 * 1024;

/// Who decides a decision. More tiers may be added with new kinds of
/// evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DecisionTier {
    /// An authenticated human acting for the decision's approver id (R3).
    Human,
    /// The decision's named reviewer, through its verdict (R2).
    Review,
}

/// A reviewer's verdict on a decision in its review stage.
///
/// Matched exhaustively: each verdict is one of the decision machine's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReviewVerdict {
    /// Approve the held operation.
    Allow,
    /// Reject it.
    Deny,
    /// Hand the decision to the human approver.
    Escalate,
}

fn bounded_text(what: &'static str, text: String) -> Result<String, DecisionContractError> {
    if text.len() > DECISION_REASON_MAX_BYTES {
        return Err(DecisionContractError::TooLarge {
            what,
            max_bytes: DECISION_REASON_MAX_BYTES,
        });
    }
    Ok(text)
}

/// One authenticated human's decision on one durable decision, bound by the
/// trusted host adapter to the event it authenticated: the ingress and event
/// reference it arrived on, the time the ingress observed the event, the
/// approver id the human decides for, and the digest of the rendering the
/// human saw, with the presentation the event carried, if any.
///
/// The decider is a qualified human ([`AuthenticatedApprover`] refuses any
/// other principal). Whether the decider may act for the approver id now is
/// a separate, current eligibility check the owner service makes when the
/// decision commits. The decision machine refuses a decision observed
/// before the decision's first presentation to the approver.
#[derive(Debug, Clone)]
pub struct AttestedDecision {
    approver: AuthenticatedApprover,
    decider: DecisionPrincipalKey,
    ingress: DecisionIngress,
    event_ref: DecisionReference,
    observed_at_ms: u64,
    approver_id: ApproverId,
    presented_digest: Sha256Digest,
    presentation: Option<PresentationRecord>,
    approve: bool,
    reason: Option<String>,
}

impl AttestedDecision {
    /// A human's decision, `approve` or reject, on the rendering whose digest
    /// is `presented_digest`, which the trusted ingress observed at
    /// `observed_at_ms` (UNIX milliseconds, the ingress's own reading of the
    /// event, such as a chat provider's message time).
    ///
    /// The decision's machine compares that reading with each presentation's
    /// `at_ms`, a reading of the host clock, and refuses a decision observed
    /// before the first presentation. An ingress clock that truncates (to
    /// whole seconds, for example) or runs behind the host's can only make a
    /// decision look earlier: the worst case is a retryable
    /// `decided_before_presentation` (the decision stays open). An ingress
    /// clock that runs ahead of the host's by `d` lets a decision made up to
    /// `d` before the presentation be accepted, and a rounding clock runs
    /// ahead by up to half its unit, so the ingress must bound that skew. A
    /// presentation time of 0 is no reading: a decision carrying a
    /// presentation at 0 is refused outright (`presentation_incomplete`),
    /// even after a valid presentation was recorded.
    ///
    /// # Errors
    /// [`DecisionContractError::UnqualifiedPrincipal`] when the approver's
    /// principal has no key.
    pub fn new(
        approver: AuthenticatedApprover,
        ingress: DecisionIngress,
        event_ref: DecisionReference,
        observed_at_ms: u64,
        approver_id: ApproverId,
        presented_digest: Sha256Digest,
        approve: bool,
    ) -> Result<Self, DecisionContractError> {
        let decider = DecisionPrincipalKey::of(approver.principal())?;
        Ok(Self {
            approver,
            decider,
            ingress,
            event_ref,
            observed_at_ms,
            approver_id,
            presented_digest,
            presentation: None,
            approve,
            reason: None,
        })
    }

    /// The presentation the decision event carried (a one-part
    /// presentation, for example).
    #[must_use]
    pub fn with_presentation(mut self, presentation: PresentationRecord) -> Self {
        self.presentation = Some(presentation);
        self
    }

    /// The decider's reason, at most 4 KiB.
    ///
    /// # Errors
    /// [`DecisionContractError::TooLarge`] past 4 KiB.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Result<Self, DecisionContractError> {
        self.reason = Some(bounded_text("reason", reason.into())?);
        Ok(self)
    }

    #[must_use]
    pub fn approver(&self) -> &AuthenticatedApprover {
        &self.approver
    }

    /// The decider's principal key.
    #[must_use]
    pub fn decider(&self) -> &DecisionPrincipalKey {
        &self.decider
    }

    #[must_use]
    pub fn ingress(&self) -> &DecisionIngress {
        &self.ingress
    }

    #[must_use]
    pub fn event_ref(&self) -> &DecisionReference {
        &self.event_ref
    }

    /// When the trusted ingress observed the decision event, in UNIX
    /// milliseconds.
    #[must_use]
    pub fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }

    /// The approver id the human decides for.
    #[must_use]
    pub fn approver_id(&self) -> &ApproverId {
        &self.approver_id
    }

    /// The digest of the rendering the human saw.
    #[must_use]
    pub fn presented_digest(&self) -> &Sha256Digest {
        &self.presented_digest
    }

    /// The presentation the decision event carried.
    #[must_use]
    pub fn presentation(&self) -> Option<&PresentationRecord> {
        self.presentation.as_ref()
    }

    /// Whether the human approves.
    #[must_use]
    pub fn approves(&self) -> bool {
        self.approve
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The durable evidence of this decision.
    #[must_use]
    pub fn evidence(&self) -> DecisionEvidence {
        DecisionEvidence::Human {
            decider: self.decider.clone(),
            principal: self.approver.principal().clone(),
            assurance: self.approver.assurance(),
            authentication_scope: self
                .approver
                .authentication_scope()
                .map(|scope| scope.as_str().to_owned()),
            ingress: self.ingress.clone(),
            event_ref: self.event_ref.clone(),
            observed_at_ms: Some(self.observed_at_ms),
            approver_id: self.approver_id.clone(),
            presented_digest: self.presented_digest.clone(),
            presentation: self.presentation.clone(),
            approve: self.approve,
            reason: self.reason.clone(),
        }
    }
}

/// A reviewer's verdict on one durable decision, attributed by the trusted
/// host to the agent principal of the reviewer's origin session and to the
/// verdict call that carried it.
#[derive(Debug, Clone)]
pub struct AttributedReview {
    reviewer: PrincipalRef,
    reviewer_key: DecisionPrincipalKey,
    origin_session: SessionId,
    verdict_call_ref: DecisionReference,
    verdict: DecisionReviewVerdict,
    presented_digest: Sha256Digest,
    feedback: Option<String>,
}

impl AttributedReview {
    /// The `verdict` of `reviewer`, an agent principal, on the rendering
    /// whose digest is `presented_digest`.
    ///
    /// # Errors
    /// [`DecisionContractError::ReviewerNotAgent`] for a principal that is not
    /// an agent, and [`DecisionContractError::UnqualifiedPrincipal`] for one
    /// without a trust domain.
    pub fn new(
        reviewer: PrincipalRef,
        origin_session: SessionId,
        verdict_call_ref: DecisionReference,
        verdict: DecisionReviewVerdict,
        presented_digest: Sha256Digest,
    ) -> Result<Self, DecisionContractError> {
        if !matches!(
            reviewer.kind,
            PrincipalKind::PersonalAgent | PrincipalKind::SharedAgent
        ) {
            return Err(DecisionContractError::ReviewerNotAgent);
        }
        let reviewer_key = DecisionPrincipalKey::of(&reviewer)?;
        Ok(Self {
            reviewer,
            reviewer_key,
            origin_session,
            verdict_call_ref,
            verdict,
            presented_digest,
            feedback: None,
        })
    }

    /// The reviewer's feedback, at most 4 KiB.
    ///
    /// # Errors
    /// [`DecisionContractError::TooLarge`] past 4 KiB.
    pub fn with_feedback(
        mut self,
        feedback: impl Into<String>,
    ) -> Result<Self, DecisionContractError> {
        self.feedback = Some(bounded_text("feedback", feedback.into())?);
        Ok(self)
    }

    #[must_use]
    pub fn reviewer(&self) -> &PrincipalRef {
        &self.reviewer
    }

    /// The reviewer's principal key.
    #[must_use]
    pub fn reviewer_key(&self) -> &DecisionPrincipalKey {
        &self.reviewer_key
    }

    #[must_use]
    pub fn verdict(&self) -> DecisionReviewVerdict {
        self.verdict
    }

    /// The digest of the rendering the reviewer saw.
    #[must_use]
    pub fn presented_digest(&self) -> &Sha256Digest {
        &self.presented_digest
    }

    #[must_use]
    pub fn feedback(&self) -> Option<&str> {
        self.feedback.as_deref()
    }

    /// The durable evidence of this verdict.
    #[must_use]
    pub fn evidence(&self) -> DecisionEvidence {
        DecisionEvidence::Review {
            reviewer: self.reviewer_key.clone(),
            principal: self.reviewer.clone(),
            origin_session: self.origin_session.clone(),
            verdict_call_ref: self.verdict_call_ref.clone(),
            verdict: self.verdict,
            presented_digest: self.presented_digest.clone(),
            feedback: self.feedback.clone(),
        }
    }
}

/// The durable evidence of a decision: what an attested decision or an
/// attributed review established, as the owner service stores it. The
/// decision's machine state carries the digest of this record
/// ([`Self::digest`]); a row whose stored evidence does not hash to it is
/// torn and is never read as decided. More kinds may be added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DecisionEvidence {
    /// An authenticated human's decision.
    Human {
        decider: DecisionPrincipalKey,
        principal: PrincipalRef,
        assurance: ApprovalAssurance,
        /// The host authentication scope (login or session) of the event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authentication_scope: Option<String>,
        ingress: DecisionIngress,
        event_ref: DecisionReference,
        /// When the ingress observed the decision event, in UNIX
        /// milliseconds. Absent in evidence recorded before decisions
        /// carried it: such evidence decodes, and keeps its bytes and its
        /// digest.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_at_ms: Option<u64>,
        approver_id: ApproverId,
        presented_digest: Sha256Digest,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        presentation: Option<PresentationRecord>,
        approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A reviewer's attributed verdict.
    Review {
        reviewer: DecisionPrincipalKey,
        principal: PrincipalRef,
        origin_session: SessionId,
        verdict_call_ref: DecisionReference,
        verdict: DecisionReviewVerdict,
        presented_digest: Sha256Digest,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
}

impl DecisionEvidence {
    /// Who decided.
    #[must_use]
    pub fn tier(&self) -> DecisionTier {
        match self {
            Self::Human { .. } => DecisionTier::Human,
            Self::Review { .. } => DecisionTier::Review,
        }
    }

    /// The digest the decision's machine state binds this evidence by: the
    /// sha256 of its canonical JSON under a fixed domain. Any field of the
    /// evidence changes it.
    ///
    /// # Errors
    /// [`DecisionContractError::Encoding`] when the evidence cannot be
    /// encoded.
    pub fn digest(&self) -> Result<Sha256Digest, DecisionContractError> {
        #[derive(Serialize)]
        struct DigestMaterial<'a> {
            domain: &'static str,
            evidence: &'a DecisionEvidence,
        }
        let bytes = serde_json::to_vec(&DigestMaterial {
            domain: "meerkat.decision.evidence.v1",
            evidence: self,
        })
        .map_err(|error| DecisionContractError::Encoding(error.to_string()))?;
        Ok(Sha256Digest::of(&bytes))
    }
}

//! The one containment rule for resuming a capability-protected fork session.
//!
//! A fork child's session id is VISIBLE: it rides in the capability's own
//! provenance, in host binding rows, and in every reply that names the
//! residency. Resuming a session by id is therefore not an ordinary operation
//! for these sessions — without a rule, a caller who merely LEARNED the id
//! could seat the branch and the authenticated capability would be decorative.
//!
//! Two surfaces can reach a protected session, and they hold different kinds
//! of proof:
//!
//! * the LOCAL mob actor, where the branch is seated by an attached spawn that
//!   already admitted the exact attach, or rebuilt for a member that still
//!   holds a durable association; and
//! * the remote HOST actor, where a V6 `MaterializeMember` carries the bearer
//!   capability itself.
//!
//! Those are genuinely different EVIDENCE, not different rules. This module
//! makes that explicit: each surface converts what it has into one typed
//! [`ForkedParticipantResumeProof`], and a single pure function decides. There
//! is exactly one place that compares a presented reference against owner
//! truth, exactly one place that decides what an absent proof means, and
//! exactly one place that knows a reserved-but-unactivated record admits
//! nothing. A surface that grows a new proof shape adds a variant here rather
//! than a second rule somewhere else.

use meerkat_core::SessionId;

use super::types::{
    ForkedParticipantAttachmentId, ForkedParticipantForkProtection, ForkedParticipantOwnerRoute,
    ForkedParticipantRef, bridge_ref,
};
use crate::ids::AgentIdentity;

/// What a caller offers as authority to resume a protected fork session.
///
/// `Absent` is a first-class variant rather than an `Option`: "the caller
/// presented nothing" is a decision the adjudicator must own, not a branch
/// each surface re-derives.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkedParticipantResumeProof {
    /// No capability authority was presented.
    Absent,
    /// Local-surface proof: the seat is authorized by this process's own
    /// durable custody, established when the attach was admitted.
    ///
    /// The local path never carries a bearer, so there is nothing to compare
    /// field-for-field. What it can prove instead is that ITS OWN durable
    /// association names this exact member and this exact fork session.
    LocalAttachedSpawn {
        /// Member identity the seat is for.
        member: AgentIdentity,
        /// Fork session the seat resumes.
        session: SessionId,
        /// Durable association evidence: the reference and member this process
        /// recorded when it admitted the attach. `None` means the spawn is the
        /// capability-aware attached spawn itself, which admitted the attach
        /// in this same operation and has not yet written its association.
        association: Option<LocalAssociationEvidence>,
    },
    /// Host-surface proof: the V6 command carried the bearer capability.
    HostCapabilityAttachment {
        /// Full immutable reference the caller presented.
        full_ref: ForkedParticipantRef,
        /// Attachment identity the caller presented, unvalidated.
        attachment_id: String,
        /// Route the serving owner actually owns.
        owner_route: ForkedParticipantOwnerRoute,
    },
}

/// One durable local association, as the local surface recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAssociationEvidence {
    /// Member the association was recorded for.
    pub member: AgentIdentity,
    /// Capability reference the association carries.
    pub capability: ForkedParticipantRef,
}

/// Why a resume of a protected fork session was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkedParticipantResumeAdmission {
    /// No capability record claims the session: an ordinary resume, unchanged.
    Unprotected,
    /// The local surface proved its own durable custody of the exact seat.
    LocalCustody,
    /// The local surface is the capability-aware attached spawn that admitted
    /// the attach in this operation.
    LocalAttachedSpawnInFlight,
    /// The presented bearer reference is exactly the one the owner recorded.
    HostCapability,
}

/// Why a resume of a protected fork session was refused.
///
/// Every variant is a distinct, surface-independent fact. Surfaces map these
/// onto their own error vocabularies; they never invent a reason of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkedParticipantResumeRejection {
    /// The session is capability custody and no authority was presented. This
    /// is the bypass the rule exists for.
    AuthorityRequired {
        /// Non-secret correlation handle of the owning capability.
        capability_hint: String,
    },
    /// The record owns the session but has not activated: no reference exists
    /// yet, so nothing can authenticate against it.
    ReservedNotActivated { capability_hint: String },
    /// A presented reference that is not the one the owner recorded.
    ReferenceMismatch { capability_hint: String },
    /// The capability is owned by a route this surface does not serve.
    ForeignRoute { capability_hint: String },
    /// Local custody evidence names a different member than the seat.
    MemberMismatch { capability_hint: String },
    /// Local custody evidence names a different fork session than the seat.
    SessionMismatch { capability_hint: String },
    /// The presented attachment identity is not a valid identity.
    MalformedAttachmentId { capability_hint: String },
}

impl ForkedParticipantResumeRejection {
    /// Non-secret correlation handle of the capability that refused.
    #[must_use]
    pub fn capability_hint(&self) -> &str {
        match self {
            Self::AuthorityRequired { capability_hint }
            | Self::ReservedNotActivated { capability_hint }
            | Self::ReferenceMismatch { capability_hint }
            | Self::ForeignRoute { capability_hint }
            | Self::MemberMismatch { capability_hint }
            | Self::SessionMismatch { capability_hint }
            | Self::MalformedAttachmentId { capability_hint } => capability_hint,
        }
    }
}

impl std::fmt::Display for ForkedParticipantResumeRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hint = self.capability_hint();
        match self {
            Self::AuthorityRequired { .. } => write!(
                f,
                "the session is capability-protected ({hint}); the authenticated forked \
                 participant authority is required"
            ),
            Self::ReservedNotActivated { .. } => write!(
                f,
                "the session is capability-protected ({hint}) but the capability has no \
                 activated reference to authenticate against"
            ),
            Self::ReferenceMismatch { .. } => write!(
                f,
                "the presented forked participant reference is not the one its owner recorded \
                 ({hint})"
            ),
            Self::ForeignRoute { .. } => {
                write!(f, "the capability is owned by another route ({hint})")
            }
            Self::MemberMismatch { .. } => write!(
                f,
                "the durable association names a different member than this seat ({hint})"
            ),
            Self::SessionMismatch { .. } => write!(
                f,
                "the durable association names a different fork session than this seat ({hint})"
            ),
            Self::MalformedAttachmentId { .. } => {
                write!(f, "the presented attachment id is malformed ({hint})")
            }
        }
    }
}

/// THE containment rule. Pure and total.
///
/// `protection` is owner truth, read from the durable capability record keyed
/// by fork child session. `proof` is what the calling surface can demonstrate.
/// Nothing else participates: no clock, no ambient state, no surface identity.
///
/// * No protection ⇒ admit. An unprotected session's resume is unchanged, and
///   a proof offered for it is simply irrelevant here — the surface's own
///   admission path owns whatever it wants to say about that.
/// * Protected + [`ForkedParticipantResumeProof::Absent`] ⇒ refuse.
/// * Protected + reserved-but-unactivated record ⇒ refuse every proof shape.
/// * Protected + local proof ⇒ admit only when the surface's own durable
///   association names this exact member AND this exact fork session, or when
///   the seat IS the attach-admitting spawn.
/// * Protected + host proof ⇒ admit only on exact full-reference equality with
///   owner truth, an owned route, and a well-formed attachment id.
pub fn adjudicate_protected_resume(
    protection: Option<&ForkedParticipantForkProtection>,
    proof: &ForkedParticipantResumeProof,
) -> Result<ForkedParticipantResumeAdmission, ForkedParticipantResumeRejection> {
    let Some(protection) = protection else {
        return Ok(ForkedParticipantResumeAdmission::Unprotected);
    };
    let hint = || protection.capability_hint.clone();

    let ForkedParticipantResumeProof::Absent = proof else {
        // A protected record that has not activated cannot authenticate ANY
        // proof shape: there is no reference to compare and no attach could
        // have been admitted against it. Checked once, ahead of both surface
        // arms, so neither can forget it.
        let Some(recorded) = protection.capability.as_ref() else {
            return Err(ForkedParticipantResumeRejection::ReservedNotActivated {
                capability_hint: hint(),
            });
        };
        return match proof {
            ForkedParticipantResumeProof::Absent => unreachable!("guarded by the outer let-else"),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member,
                session,
                association,
            } => adjudicate_local(protection, recorded, member, session, association.as_ref()),
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref,
                attachment_id,
                owner_route,
            } => adjudicate_host(protection, recorded, full_ref, attachment_id, owner_route),
        };
    };
    Err(ForkedParticipantResumeRejection::AuthorityRequired {
        capability_hint: hint(),
    })
}

fn adjudicate_local(
    protection: &ForkedParticipantForkProtection,
    recorded: &ForkedParticipantRef,
    member: &AgentIdentity,
    session: &SessionId,
    association: Option<&LocalAssociationEvidence>,
) -> Result<ForkedParticipantResumeAdmission, ForkedParticipantResumeRejection> {
    let hint = || protection.capability_hint.clone();
    // The local surface only ever serves locally-owned capabilities. A placed
    // capability's custody lives on its owning host and must travel the V6
    // path, so admitting it here would be a second, weaker door to the same
    // branch.
    if !matches!(
        protection.owner_route,
        ForkedParticipantOwnerRoute::Local { .. }
    ) {
        return Err(ForkedParticipantResumeRejection::ForeignRoute {
            capability_hint: hint(),
        });
    }
    let Some(association) = association else {
        // The attach-admitting spawn itself: custody was established in this
        // same operation and its association row is written as part of it.
        return Ok(ForkedParticipantResumeAdmission::LocalAttachedSpawnInFlight);
    };
    if association.member != *member {
        return Err(ForkedParticipantResumeRejection::MemberMismatch {
            capability_hint: hint(),
        });
    }
    if association.capability.fork_session_id() != session {
        return Err(ForkedParticipantResumeRejection::SessionMismatch {
            capability_hint: hint(),
        });
    }
    // The association is this process's own durable record, so the comparison
    // against owner truth is still exact rather than presence-only: a row that
    // drifted from the capability it claims custody of proves nothing.
    if association.capability != *recorded {
        return Err(ForkedParticipantResumeRejection::ReferenceMismatch {
            capability_hint: hint(),
        });
    }
    Ok(ForkedParticipantResumeAdmission::LocalCustody)
}

fn adjudicate_host(
    protection: &ForkedParticipantForkProtection,
    recorded: &ForkedParticipantRef,
    presented: &ForkedParticipantRef,
    attachment_id: &str,
    owner_route: &ForkedParticipantOwnerRoute,
) -> Result<ForkedParticipantResumeAdmission, ForkedParticipantResumeRejection> {
    let hint = || protection.capability_hint.clone();
    // Field-for-field against owner truth, through the wire projection both
    // sides agree on: a widened scope, a moved route, a stretched expiry, or
    // rewritten provenance all fail here rather than at a later,
    // effect-carrying step.
    if bridge_ref(recorded) != bridge_ref(presented) {
        return Err(ForkedParticipantResumeRejection::ReferenceMismatch {
            capability_hint: hint(),
        });
    }
    if recorded.owner_route() != owner_route {
        return Err(ForkedParticipantResumeRejection::ForeignRoute {
            capability_hint: hint(),
        });
    }
    if ForkedParticipantAttachmentId::new(attachment_id).is_err() {
        return Err(ForkedParticipantResumeRejection::MalformedAttachmentId {
            capability_hint: hint(),
        });
    }
    Ok(ForkedParticipantResumeAdmission::HostCapability)
}

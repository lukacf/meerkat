//! Source-owned forked-participant capabilities (issue #159, phase 2).
//!
//! This module owns the feature's domain vocabulary and the source-owner
//! service that drives the canonical `ForkedParticipantLifecycleMachine`. The
//! durable contract lives in the mob store layer
//! ([`crate::store::ForkedParticipantStore`]).

mod containment;
mod service;
mod types;

pub use containment::{
    ForkedParticipantResumeAdmission, ForkedParticipantResumeProof,
    ForkedParticipantResumeRejection, LocalAssociationEvidence, adjudicate_protected_resume,
};
pub use service::{
    ForkedParticipantError, ForkedParticipantService, ForkedParticipantSourceRuntime,
    PlannedChildEvidence, PlannedForkOutcome, PlannedForkRequest, SessionExecutionEvidence,
};

pub use types::{
    FORKED_PARTICIPANT_BEARER_TOKEN_LEN, FORKED_PARTICIPANT_CLEANUP_CLAIM_TTL,
    FORKED_PARTICIPANT_FINGERPRINT_VERSION, ForkedParticipantAttachmentAssociation,
    ForkedParticipantAttachmentId, ForkedParticipantCapabilityId,
    ForkedParticipantCleanupAttemptId, ForkedParticipantCleanupClaim,
    ForkedParticipantCleanupClaimOutcome, ForkedParticipantCleanupDebt, ForkedParticipantCleanupId,
    ForkedParticipantCleanupLease, ForkedParticipantCleanupPublish, ForkedParticipantCleanupReport,
    ForkedParticipantExpirySweepReport, ForkedParticipantFingerprintError,
    ForkedParticipantForkProtection, ForkedParticipantGrant, ForkedParticipantIdentityError,
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute,
    ForkedParticipantPendingAttachment, ForkedParticipantPendingAttachmentReport,
    ForkedParticipantPendingTerminal, ForkedParticipantProvenance, ForkedParticipantRef,
    ForkedParticipantReleaseOutcome, ForkedParticipantRequest, ForkedParticipantRequestId,
    ForkedParticipantReservation, ForkedParticipantReusePolicy, ForkedParticipantRevocationId,
    ForkedParticipantRevocationOutcome, ForkedParticipantSweepEntry,
    MAX_FORKED_PARTICIPANT_ATTACHMENT_ID_LEN, MAX_FORKED_PARTICIPANT_REQUEST_ID_LEN,
    MAX_FORKED_PARTICIPANT_TTL, MAX_FORKED_PARTICIPANT_USES, bridge_ref, domain_ref,
};

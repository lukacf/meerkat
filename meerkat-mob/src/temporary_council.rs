//! Temporary-council orchestration vocabulary (issue #159, phase 4).
//!
//! A *temporary council* is a bounded conversation between forked-participant
//! capabilities seated as ordinary members of a REAL, short-lived mob. This
//! module owns only the durable orchestration vocabulary:
//!
//!   * the validated council identity,
//!   * the per-participant custody the coordinator must hold to finish or
//!     clean up after a crash,
//!   * the bounded exchange receipts,
//!   * the immutable result (including its typed exit reason and merge
//!     outcome), and
//!   * the cleanup receipt/debt.
//!
//! It owns NO mob, member, or capability lifecycle. `MobMachine`, the member
//! machines, and `ForkedParticipantLifecycleMachine` remain canonical there,
//! and nothing here duplicates a member phase. The council record's OWN
//! lifecycle — request binding, discussion/merge advance, result sealing,
//! cleanup settlement — is owned by the canonical
//! [`crate::machines::temporary_council_lifecycle::TemporaryCouncilLifecycleMachine`],
//! whose generated state the store persists. There is deliberately no
//! handwritten phase enum here.
//!
//! # Capability custody
//!
//! Each participant slot records the FULL immutable
//! [`ForkedParticipantRef`] it acquired, persisted immediately after creation
//! and BEFORE the attach. That reference is exactly what issue #159 designs
//! for a holder to carry, and holding it is what makes cleanup work for a
//! HOST-owned capability: the remote owner's record lives in the remote host's
//! store, so a coordinator that crashed between create and attach could never
//! resolve it from realm-local custody. Persisting it in the realm-scoped
//! council database is custody, not shadow lifecycle — the owner's machine
//! still decides every attach/release/revoke verdict.
//!
//! The reference redacts its bearer token in `Debug` and has no `Display`, so
//! it cannot reach a log line by accident. The council RESULT never carries it:
//! results carry non-secret provenance only.

use crate::forked_participant::{
    ForkedParticipantAttachmentId, ForkedParticipantOperationScope, ForkedParticipantOwnerRoute,
    ForkedParticipantProvenance, ForkedParticipantRef, ForkedParticipantRequestId,
    ForkedParticipantReusePolicy, MAX_FORKED_PARTICIPANT_TTL,
};
use crate::ids::ProfileName;
use crate::ids::{AgentIdentity, MobId};
use chrono::{DateTime, Utc};
use meerkat_core::SessionId;
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;
use thiserror::Error;

/// Maximum canonical length of a caller-supplied council identity.
pub const MAX_TEMPORARY_COUNCIL_ID_LEN: usize = 128;

/// Maximum number of participants one council may seat.
pub const MAX_TEMPORARY_COUNCIL_PARTICIPANTS: usize = 8;

/// Maximum number of sequential discussion rounds.
pub const MAX_TEMPORARY_COUNCIL_ROUNDS: u32 = 16;

/// Maximum number of individual participant exchanges across all rounds.
pub const MAX_TEMPORARY_COUNCIL_EXCHANGES: u32 = 64;

/// Maximum receiver-bounded byte cap for a single exchange or merge result.
pub const MAX_TEMPORARY_COUNCIL_RESULT_BYTES: usize = 64 * 1024;

/// Minimum receiver-bounded byte cap. Mirrors the bounded-helper contract's
/// requirement that the explicit truncation marker always fits.
pub const MIN_TEMPORARY_COUNCIL_RESULT_BYTES: usize = 256;

/// Absolute ceiling on a council deadline.
///
/// A council may never outlive the capabilities it seats, so this is pinned to
/// the forked-participant TTL cap rather than chosen independently.
pub const MAX_TEMPORARY_COUNCIL_DURATION: Duration = MAX_FORKED_PARTICIPANT_TTL;

/// Whether a council's custody survives process restart.
///
/// This is an explicit caller declaration, never an inference. A council that
/// declares `Durable` is refused unless the state's council store actually
/// survives a restart, so crash recovery is never silently promised on top of
/// in-memory custody. `ProcessBound` is the opt-in for tests, embedders, and
/// ephemeral surfaces: it says out loud that a process death loses the record
/// and the source capability TTL is the only remaining backstop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilDurability {
    /// Custody must survive process restart; crash recovery is guaranteed.
    Durable,
    /// Custody is process-bound; a process death loses the record.
    ProcessBound,
}

/// What a [`crate::store::TemporaryCouncilStore`] backend actually provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilStoreDurability {
    /// Records survive process restart.
    Durable,
    /// Records live only for this process.
    ProcessBound,
}

impl TemporaryCouncilStoreDurability {
    /// Whether this backend satisfies a caller's declared requirement.
    #[must_use]
    pub const fn satisfies(self, required: TemporaryCouncilDurability) -> bool {
        matches!(
            (required, self),
            (TemporaryCouncilDurability::Durable, Self::Durable)
                | (TemporaryCouncilDurability::ProcessBound, _)
        )
    }
}

/// Explicit version of the canonical council request fingerprint shape.
pub const TEMPORARY_COUNCIL_FINGERPRINT_VERSION: u32 = 1;

/// Deterministic prefix of every council-owned temporary mob id.
pub const TEMPORARY_COUNCIL_MOB_ID_PREFIX: &str = "council--";

/// Rejection of a malformed council identity.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TemporaryCouncilIdentityError {
    /// The identity was empty or whitespace only.
    #[error("temporary council id must not be empty")]
    Empty,
    /// The identity exceeded [`MAX_TEMPORARY_COUNCIL_ID_LEN`].
    #[error("temporary council id must not exceed {MAX_TEMPORARY_COUNCIL_ID_LEN} bytes")]
    TooLong,
    /// The identity was not already in canonical (trimmed) form.
    #[error("temporary council id must be supplied in canonical trimmed form")]
    NonCanonical,
    /// The identity carried a character outside the canonical alphabet.
    #[error(
        "temporary council id may only contain ASCII alphanumerics, '-', '_', '.', or ':' (found {found:?})"
    )]
    IllegalCharacter {
        /// The first rejected character.
        found: char,
    },
}

/// Validated, canonical identity of one temporary council.
///
/// The alphabet is restricted so the identity can be embedded verbatim in a
/// deterministic [`MobId`], a capability request id, an attachment id, and a
/// delivery idempotency key without any escaping step that could collide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TemporaryCouncilId(String);

impl TemporaryCouncilId {
    /// Validate and wrap a caller-supplied council identity.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, TemporaryCouncilIdentityError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(TemporaryCouncilIdentityError::Empty);
        }
        if raw.trim() != raw {
            return Err(TemporaryCouncilIdentityError::NonCanonical);
        }
        if raw.len() > MAX_TEMPORARY_COUNCIL_ID_LEN {
            return Err(TemporaryCouncilIdentityError::TooLong);
        }
        if let Some(found) = raw
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')))
        {
            return Err(TemporaryCouncilIdentityError::IllegalCharacter { found });
        }
        Ok(Self(raw.to_owned()))
    }

    /// Borrow the validated identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Deterministic temporary [`MobId`] this council owns.
    ///
    /// The same council id always names the same temporary mob, so a retry
    /// after a crash finds (rather than duplicates) the real mob it created.
    #[must_use]
    pub fn temporary_mob_id(&self) -> MobId {
        MobId::from(format!("{TEMPORARY_COUNCIL_MOB_ID_PREFIX}{}", self.0))
    }

    /// Deterministic capability request id for one participant slot.
    pub fn capability_request_id(
        &self,
        order: u32,
    ) -> Result<ForkedParticipantRequestId, crate::forked_participant::ForkedParticipantIdentityError>
    {
        ForkedParticipantRequestId::new(format!("council:{}:p{order}", self.0))
    }

    /// Deterministic attachment id for one participant slot.
    pub fn attachment_id(
        &self,
        order: u32,
    ) -> Result<
        ForkedParticipantAttachmentId,
        crate::forked_participant::ForkedParticipantIdentityError,
    > {
        ForkedParticipantAttachmentId::new(format!("council:{}:p{order}", self.0))
    }

    /// Deterministic delivery idempotency key for one exchange slot.
    #[must_use]
    pub fn delivery_idempotency_key(&self, round: u32, order: u32, purpose: &str) -> String {
        format!("council:{}:{purpose}:r{round}:p{order}", self.0)
    }
}

impl std::fmt::Display for TemporaryCouncilId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TemporaryCouncilId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// How far capability acquisition got for one participant slot.
///
/// The distinction that matters is `Pending`: the coordinator persisted the
/// INTENT to call the source owner and then either crashed or failed its
/// post-create commit. A capability may or may not exist. Cleanup must treat
/// that as an explicit ambiguity to reconcile by the deterministic request
/// identity — never as a proven absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilAcquisition {
    /// No create call has been issued for this slot.
    NotAttempted,
    /// The create call was issued; its outcome is not durably known.
    Pending,
    /// The exact capability reference is durably held.
    Acquired,
    /// A capability WAS returned but its custody commit did not succeed.
    ///
    /// The in-memory reference was re-committed on a best-effort second write;
    /// if that also failed the record stays `Pending`. Either way the slot is
    /// never reported as "nothing was created".
    Ambiguous,
}

impl TemporaryCouncilAcquisition {
    /// Whether a capability may exist for this slot.
    ///
    /// `Pending` answers TRUE: the honest reading of an unknown create outcome
    /// is "may exist", so cleanup reconciles instead of assuming absence.
    #[must_use]
    pub const fn may_exist(self) -> bool {
        !matches!(self, Self::NotAttempted)
    }

    /// Whether the exact reference is durably held.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        matches!(self, Self::Acquired | Self::Ambiguous)
    }
}

/// Durable custody for one council participant slot.
///
/// Every field is either caller intent that must survive a crash, or a
/// reference to authority owned elsewhere. Nothing here is a copy of a
/// capability bearer token or of live member state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilParticipantCustody {
    /// Deterministic slot order. Also the turn order within a round.
    pub order: u32,
    /// Deterministic role label carried into provenance.
    pub role: String,
    /// Mob that owns the source member being forked.
    pub source_mob_id: MobId,
    /// Source member identity in `source_mob_id`.
    pub source_identity: AgentIdentity,
    /// Identity this participant is seated under in the temporary mob.
    pub target_identity: AgentIdentity,
    /// Profile name resolved from the caller-supplied definition template.
    pub target_profile: ProfileName,
    /// Granted operation scope.
    pub scope: ForkedParticipantOperationScope,
    /// Deterministic capability request id.
    ///
    /// This is the durable REFERENCE to the capability record. The capability
    /// store owns the bearer material; recovery resolves the exact reference
    /// through this id rather than reading a copied one.
    pub capability_request_id: ForkedParticipantRequestId,
    /// Non-secret correlation hint of the acquired capability, once created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_correlation_hint: Option<String>,
    /// The FULL immutable capability reference, persisted immediately after
    /// creation and BEFORE the attach.
    ///
    /// This is the holder-side custody issue #159 designs for. Cleanup and
    /// crash recovery present this exact reference to
    /// `MobHandle::revoke_forked_participant`, which routes by the
    /// reference's OWN owner route — so a HOST-owned capability is revocable
    /// at its owning host even when this realm's stores never held its record
    /// and even after the source member or mob is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_ref: Option<ForkedParticipantRef>,
    /// Deterministic attachment id used for the attached spawn.
    pub attachment_id: ForkedParticipantAttachmentId,
    /// How far capability acquisition got for this slot.
    pub acquisition: TemporaryCouncilAcquisition,
    /// Whether the attached spawn was durably observed to succeed.
    pub seated: bool,
    /// Fork session the seated member resumed, once seated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seated_session_id: Option<SessionId>,
}

impl TemporaryCouncilParticipantCustody {
    /// Whether this slot may hold a capability that was never attached.
    ///
    /// Those are exactly the capabilities cleanup must explicitly revoke: no
    /// attachment/release path will ever reach them. A `Pending` acquisition
    /// is included, because an unknown create outcome must be reconciled
    /// rather than assumed absent.
    #[must_use]
    pub const fn acquired_but_unattached(&self) -> bool {
        self.acquisition.may_exist() && !self.seated
    }
}

/// Non-secret provenance of the exact capability one participant was seated
/// under.
///
/// This is what makes a council result provenance-carrying per issue #159: it
/// names the source transcript the branch was forked from, its prefix digest,
/// the owning route, the fork session identity, and the granted scope, expiry
/// and reuse policy. It deliberately carries NO capability bearer token — the
/// bearer stays in capability custody and in the council record, never in a
/// returned result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilCapabilityProvenance {
    /// Typed route to the runtime that owns the capability.
    pub owner_route: ForkedParticipantOwnerRoute,
    /// Fork session the seated branch resumed.
    pub fork_session_id: SessionId,
    /// Exact source transcript provenance: source session, selected prefix
    /// length, and the prefix digest the owner computed.
    pub source_provenance: ForkedParticipantProvenance,
    /// Operations the holder was granted.
    pub scope: ForkedParticipantOperationScope,
    /// Reuse policy the capability was minted under.
    pub reuse: ForkedParticipantReusePolicy,
    /// Absolute expiry of the capability.
    pub expires_at: DateTime<Utc>,
    /// Non-secret correlation hint of the capability.
    pub correlation_hint: String,
}

impl TemporaryCouncilCapabilityProvenance {
    /// Project the non-secret provenance of an acquired capability.
    ///
    /// The bearer token is deliberately not read here: a result must be safe to
    /// hand to the council's caller.
    #[must_use]
    pub fn from_reference(capability: &ForkedParticipantRef) -> Self {
        Self {
            owner_route: capability.owner_route().clone(),
            fork_session_id: capability.fork_session_id().clone(),
            source_provenance: capability.provenance().clone(),
            scope: capability.scope(),
            reuse: capability.reuse(),
            expires_at: capability.expires_at(),
            correlation_hint: capability.capability_id().correlation_hint(),
        }
    }
}

/// Non-secret provenance of one participant, carried in the immutable result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilParticipantProvenance {
    /// Deterministic slot order.
    pub order: u32,
    /// Deterministic role label.
    pub role: String,
    /// Mob that owned the source member.
    pub source_mob_id: MobId,
    /// Source member identity.
    pub source_identity: AgentIdentity,
    /// Identity the participant was seated under.
    pub target_identity: AgentIdentity,
    /// Granted operation scope.
    pub scope: ForkedParticipantOperationScope,
    /// Deterministic capability request id (non-secret idempotency
    /// correlation, not custody).
    pub capability_request_id: ForkedParticipantRequestId,
    /// Exact non-secret capability provenance, once a capability was acquired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<TemporaryCouncilCapabilityProvenance>,
    /// Attachment id the temporary mob held.
    pub attachment_id: ForkedParticipantAttachmentId,
    /// Whether the participant was actually seated.
    pub seated: bool,
}

/// Terminal classification of one bounded exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilExchangeOutcome {
    /// The delivery identity is durably recorded but no terminal was observed.
    ///
    /// A record left in this state is what a coordinator crash looks like.
    Pending,
    /// The exchange committed an exact receiver-bounded turn.
    Completed {
        /// Receiver-bounded compact text.
        text: String,
        /// Whether the receiver bound truncated the member's output.
        truncated: bool,
        /// Session the committed turn belongs to.
        session_id: SessionId,
        /// When the terminal was observed.
        completed_at: DateTime<Utc>,
    },
    /// The exchange failed, timed out, or was cancelled.
    Failed {
        /// Typed detail, already bounded by the caller.
        detail: String,
        /// When the failure was observed.
        failed_at: DateTime<Utc>,
    },
}

/// Durable receipt of one participant turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilExchangeReceipt {
    /// Zero-based round index.
    pub round: u32,
    /// Monotonic exchange sequence across the whole council.
    pub sequence: u32,
    /// Participant slot order.
    pub participant_order: u32,
    /// Identity the turn was delivered to.
    pub target_identity: AgentIdentity,
    /// Deterministic delivery idempotency key persisted BEFORE the send.
    pub delivery_idempotency_key: String,
    /// Deterministic delivery correlation id persisted BEFORE the send.
    pub delivery_correlation_id: String,
    /// When the send was recorded.
    pub started_at: DateTime<Utc>,
    /// Terminal classification.
    pub outcome: TemporaryCouncilExchangeOutcome,
}

impl TemporaryCouncilExchangeReceipt {
    /// Committed bounded text, when the exchange completed.
    #[must_use]
    pub fn completed_text(&self) -> Option<&str> {
        match &self.outcome {
            TemporaryCouncilExchangeOutcome::Completed { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    /// Whether the receiver bound truncated this exchange.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        matches!(
            self.outcome,
            TemporaryCouncilExchangeOutcome::Completed {
                truncated: true,
                ..
            }
        )
    }
}

/// A participant's UNVERIFIED, typed claim about an artifact it says it
/// produced.
///
/// This is deliberately NOT [`meerkat_core::artifact::ArtifactHandle`]. That
/// type names a Meerkat-durable artifact by canonical `ArtifactId` and implies
/// the artifact exists in an `ArtifactStore`; this type is a bounded reference
/// a council participant emitted and the council parsed. The council performs
/// no store lookup, no fetch, and no existence check, so calling it a durable
/// artifact handle would assert something no one verified.
///
/// It is a reference, never an inlined body: the artifact merge policy exists
/// precisely so a council does not copy content back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilArtifactClaim {
    /// Participant-reported location. Not resolved or validated by the council.
    pub uri: String,
    /// Optional media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional content digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Optional byte length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_len: Option<u64>,
}

/// What the explicit merge-back policy actually produced.
///
/// No variant carries a whole transcript, and no variant mutates the caller's
/// session: merge-back is a returned value, never an implicit parent write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilMergeOutcome {
    /// Observation only: provenance and confirmation, no content.
    NoMerge {
        /// Participants confirmed seated at merge time.
        confirmed_participants: Vec<AgentIdentity>,
    },
    /// One bounded natural-language summary from the named finalizer.
    BoundedTextSummary {
        /// Participant that produced the summary.
        finalizer: AgentIdentity,
        /// Receiver-bounded summary text.
        text: String,
        /// Whether the receiver bound truncated the summary.
        truncated: bool,
    },
    /// One strict-JSON structured result, validated against the caller's
    /// declared contract.
    StructuredResult {
        /// Participant that produced the value.
        finalizer: AgentIdentity,
        /// Contract the value was validated against.
        contract: TemporaryCouncilStructuredContractIdentity,
        /// Strictly parsed and schema-valid JSON value.
        value: serde_json::Value,
        /// Whether the receiver bound truncated the raw output before parsing.
        truncated: bool,
    },
    /// Explicitly selected COUNCIL EXCHANGES, by their exchange sequence.
    ///
    /// The selection domain is deliberately the council's own bounded
    /// exchange receipts, NOT the seated fork session's transcript. A fork
    /// session begins with the inherited source prefix (system instructions,
    /// prior user turns, prior assistant turns), so indexing it would let a
    /// low index exfiltrate source-context content the council never
    /// produced. No inherited prefix message is selectable by construction:
    /// this variant can only ever name text a council participant emitted in
    /// response to a council prompt.
    SelectedTranscript {
        /// Participant whose council exchanges were selected.
        participant: AgentIdentity,
        /// The exact exchange sequences that were requested and found.
        exchange_sequences: Vec<u32>,
        /// One bounded excerpt per returned sequence, in the same order.
        excerpts: Vec<TemporaryCouncilSelectedExchange>,
        /// Whether the total byte cap truncated the selection.
        truncated: bool,
    },
    /// A typed, participant-reported artifact reference.
    ///
    /// The claim is parsed and bounded, never resolved: see
    /// [`TemporaryCouncilArtifactClaim`].
    DurableArtifactReference {
        /// Participant that produced the claim.
        participant: AgentIdentity,
        /// Parsed typed claim. UNVERIFIED by the council.
        claim: TemporaryCouncilArtifactClaim,
    },
    /// The council terminated before merge was attempted.
    NotAttempted {
        /// Why the merge never ran.
        reason: String,
    },
    /// The merge ran and failed in a typed, non-fatal way.
    Failed {
        /// Which policy failed.
        policy: TemporaryCouncilMergePolicyKind,
        /// Typed detail.
        detail: String,
    },
}

/// One selected council exchange, with its own provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilSelectedExchange {
    /// Council-wide exchange sequence.
    pub sequence: u32,
    /// Round the exchange belongs to.
    pub round: u32,
    /// Participant slot that produced it.
    pub participant_order: u32,
    /// Identity that produced it.
    pub target_identity: AgentIdentity,
    /// Bounded excerpt of the exchange's own receiver-bounded text.
    pub text: String,
    /// Whether this excerpt was cut by the selection's total byte cap.
    pub truncated: bool,
}

/// Stable discriminant of the caller's merge-back policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilMergePolicyKind {
    /// Bounded text summary.
    BoundedTextSummary,
    /// Strict-JSON structured result.
    StructuredResult,
    /// Selected transcript indices.
    SelectedTranscript,
    /// Durable artifact reference.
    DurableArtifactReference,
    /// No merge.
    NoMerge,
}

/// Non-secret identity of the structured-result contract a value satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilStructuredContractIdentity {
    /// Caller-stable contract identity.
    pub schema_id: String,
    /// Caller-declared contract version.
    pub schema_version: u32,
    /// Digest of the exact JSON Schema the value was validated against.
    pub schema_digest: String,
}

/// Why the council stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilExitReason {
    /// The configured round schedule ran to completion within bounds.
    Completed,
    /// The exchange budget cut the round schedule short.
    MaxExchangesReached,
    /// The absolute deadline elapsed.
    DeadlineExceeded,
    /// One participant could not be seated.
    ParticipantSeatingFailed {
        /// Slot that failed.
        participant_order: u32,
        /// Typed detail.
        detail: String,
    },
    /// The requested topology could not be fully wired.
    WiringIncomplete {
        /// Typed detail, including the observed wiring report.
        detail: String,
    },
    /// One exchange failed terminally.
    ExchangeFailed {
        /// Round the failure occurred in.
        round: u32,
        /// Identity the turn was delivered to.
        target_identity: AgentIdentity,
        /// Typed detail.
        detail: String,
    },
    /// The coordinator process died before committing a result.
    ///
    /// Recovery commits this reason rather than silently re-executing: a
    /// re-run would duplicate model work and result delivery.
    CoordinatorInterrupted,
}

impl TemporaryCouncilExitReason {
    /// Whether the council reached a complete, non-partial conclusion.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Immutable result of one temporary council.
///
/// Persisted BEFORE cleanup runs, so it stays valid even when cleanup later
/// fails. Cleanup status is reported separately in
/// [`TemporaryCouncilCleanupReceipt`] and never folded into this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilResult {
    /// Council identity.
    pub council_id: TemporaryCouncilId,
    /// Canonical fingerprint of the request that produced this result.
    pub request_fingerprint: String,
    /// The real temporary mob the council ran in.
    pub temporary_mob_id: MobId,
    /// Why the council stopped.
    pub exit_reason: TemporaryCouncilExitReason,
    /// Number of rounds that completed at least one exchange.
    pub rounds_completed: u32,
    /// Every bounded exchange receipt, in order.
    pub exchanges: Vec<TemporaryCouncilExchangeReceipt>,
    /// Explicit merge-back outcome.
    pub merge: TemporaryCouncilMergeOutcome,
    /// Non-secret provenance for each participant slot.
    pub participants: Vec<TemporaryCouncilParticipantProvenance>,
    /// Number of exchanges whose text was truncated by the receiver bound.
    pub truncated_exchange_count: u32,
    /// Whether the merge output was truncated by its own byte cap.
    pub merge_truncated: bool,
    /// The durability the caller declared and the store honoured.
    pub durability: TemporaryCouncilDurability,
    /// When the result was sealed.
    pub concluded_at: DateTime<Utc>,
}

/// One unpaid cleanup obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilCleanupDebt {
    /// What could not be cleaned (mob id, participant slot, capability id).
    pub subject: String,
    /// Typed detail of the failure.
    pub detail: String,
}

/// Durable receipt of one cleanup attempt.
///
/// Retained (and overwritten by later attempts) until `settled` is true, so a
/// retry can converge instead of losing the obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilCleanupReceipt {
    /// When this attempt ran.
    pub attempted_at: DateTime<Utc>,
    /// Number of cleanup attempts including this one.
    pub attempts: u32,
    /// Whether the temporary mob is gone.
    pub temporary_mob_destroyed: bool,
    /// Participant slots whose seated member was retired (releasing its
    /// attachment through the ordinary association path).
    pub released_participants: Vec<u32>,
    /// Participant slots whose acquired-but-unattached capability was
    /// explicitly revoked.
    pub revoked_participants: Vec<u32>,
    /// Outstanding obligations. Empty means the attempt fully converged.
    pub debts: Vec<TemporaryCouncilCleanupDebt>,
    /// Whether the bounded cleanup budget expired with work still outstanding.
    ///
    /// The council result is already sealed and immutable at this point, so
    /// the caller is handed that result plus this pending receipt rather than
    /// being held past its deadline by a stuck teardown. The obligation stays
    /// durable and a later sweep retries it.
    pub budget_exhausted: bool,
}

/// How a cleanup attempt ended, as a single typed verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilCleanupStatus {
    /// Every obligation was discharged.
    Settled,
    /// Cleanup ran to completion and retained typed debt.
    Debt,
    /// The bounded cleanup budget expired with work still outstanding.
    Pending,
}

impl TemporaryCouncilCleanupReceipt {
    /// Whether this attempt discharged every obligation.
    #[must_use]
    pub fn settled(&self) -> bool {
        self.temporary_mob_destroyed && self.debts.is_empty() && !self.budget_exhausted
    }

    /// Typed verdict for this attempt.
    #[must_use]
    pub fn status(&self) -> TemporaryCouncilCleanupStatus {
        if self.settled() {
            TemporaryCouncilCleanupStatus::Settled
        } else if self.budget_exhausted {
            TemporaryCouncilCleanupStatus::Pending
        } else {
            TemporaryCouncilCleanupStatus::Debt
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn council_id_rejects_non_canonical_and_illegal_text() {
        assert_eq!(
            TemporaryCouncilId::new(""),
            Err(TemporaryCouncilIdentityError::Empty)
        );
        assert_eq!(
            TemporaryCouncilId::new(" pad "),
            Err(TemporaryCouncilIdentityError::NonCanonical)
        );
        assert_eq!(
            TemporaryCouncilId::new("a".repeat(MAX_TEMPORARY_COUNCIL_ID_LEN + 1)),
            Err(TemporaryCouncilIdentityError::TooLong)
        );
        assert_eq!(
            TemporaryCouncilId::new("has space"),
            Err(TemporaryCouncilIdentityError::IllegalCharacter { found: ' ' })
        );
        assert_eq!(
            TemporaryCouncilId::new("has/slash"),
            Err(TemporaryCouncilIdentityError::IllegalCharacter { found: '/' })
        );
        let ok = TemporaryCouncilId::new("council.A-1_2:x").expect("canonical id");
        assert_eq!(ok.as_str(), "council.A-1_2:x");
    }

    #[test]
    fn derived_identities_are_deterministic_and_slot_scoped() {
        let id = TemporaryCouncilId::new("demo").expect("id");
        assert_eq!(id.temporary_mob_id().as_str(), "council--demo");
        assert_eq!(
            id.capability_request_id(2).expect("request id").as_str(),
            "council:demo:p2"
        );
        assert_eq!(
            id.attachment_id(2).expect("attachment id").as_str(),
            "council:demo:p2"
        );
        assert_ne!(
            id.capability_request_id(1).expect("request id"),
            id.capability_request_id(2).expect("request id")
        );
        assert_eq!(
            id.delivery_idempotency_key(0, 1, "round"),
            "council:demo:round:r0:p1"
        );
        assert_ne!(
            id.delivery_idempotency_key(0, 1, "round"),
            id.delivery_idempotency_key(0, 1, "merge")
        );
    }

    #[test]
    fn council_id_round_trips_through_validating_deserialization() {
        let id = TemporaryCouncilId::new("round-trip").expect("id");
        let encoded = serde_json::to_string(&id).expect("encode");
        assert_eq!(encoded, "\"round-trip\"");
        let decoded: TemporaryCouncilId = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, id);
        assert!(
            serde_json::from_str::<TemporaryCouncilId>("\" bad \"").is_err(),
            "deserialization must validate, not trim"
        );
    }

    #[test]
    fn cleanup_receipt_settles_only_without_debt() {
        let mut receipt = TemporaryCouncilCleanupReceipt {
            attempted_at: Utc::now(),
            attempts: 1,
            temporary_mob_destroyed: true,
            released_participants: vec![0],
            revoked_participants: Vec::new(),
            debts: Vec::new(),
            budget_exhausted: false,
        };
        assert!(receipt.settled());
        assert_eq!(receipt.status(), TemporaryCouncilCleanupStatus::Settled);
        receipt.debts.push(TemporaryCouncilCleanupDebt {
            subject: "participant:0".to_string(),
            detail: "release failed".to_string(),
        });
        assert!(!receipt.settled());
        assert_eq!(receipt.status(), TemporaryCouncilCleanupStatus::Debt);
        receipt.debts.clear();
        receipt.temporary_mob_destroyed = false;
        assert!(!receipt.settled());
        receipt.temporary_mob_destroyed = true;
        receipt.budget_exhausted = true;
        assert!(
            !receipt.settled(),
            "an exhausted budget is not a settlement"
        );
        assert_eq!(receipt.status(), TemporaryCouncilCleanupStatus::Pending);
    }
}

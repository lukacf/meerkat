//! Operator instructions of a mob: durable System-plane instructions a host
//! sets for the mob's members, one record per slot.
//!
//! A host writes a slot's body at a revision for a target (every member, the
//! members of one profile, or one member); a revoke is a higher revision
//! whose effect is Absent. MobMachine owns every lifecycle fact: the desired
//! records, the generation every committed write advances, and each member's
//! delivery facts (owed, recorded, blocked). The mob journals a committed
//! record and the targets its members' sessions recorded at mob event schema
//! [`meerkat_core::OPERATOR_INSTRUCTION_MOB_EVENT_SCHEMA_VERSION`]; every
//! other mob event keeps the current version.
//!
//! A member's session holds the member's whole snapshot as its recorded
//! target: the mob delivers it through the member session's actor while the
//! session is live and store-only while it is dormant, never waking or
//! materializing a member to deliver. A member born on a new session carries
//! the snapshot in its first build. The session applies the target at its
//! next fresh run start. This module holds the public vocabulary, the
//! journaled payloads and the sealed delivery capabilities; the delivery
//! itself lives in the runtime.

use meerkat_core::types::SessionId;
use meerkat_core::{
    InstructionContentDigest, InstructionEffect, InstructionEntryKind, InstructionKey,
    InstructionNamespace, InstructionRevision, InstructionSetErrorCode, InstructionSlot,
    InstructionTargetRecord, InstructionWriteError,
};
use serde::{Deserialize, Serialize};

use crate::error::MobError;
use crate::event::MobEventKind;
use crate::ids::{AgentIdentity, MobId, ProfileName};

/// Which members an operator instruction targets: every member of the mob,
/// the members of one profile, or one member.
///
/// A target is a predicate over the roster, not a reference: a profile or a
/// member that does not exist (yet) is legal and selects nobody until it
/// does. A member added later, or moved to the profile later, receives the
/// record then.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperatorInstructionTarget {
    /// Every member of the mob.
    Mob,
    /// The members of one profile.
    Profile { profile: ProfileName },
    /// One member.
    Member { agent_identity: AgentIdentity },
}

impl OperatorInstructionTarget {
    /// Whether this target selects `agent_identity`, a member of profile
    /// `member_profile` (the machine's target predicate).
    #[must_use]
    pub fn selects(&self, agent_identity: &AgentIdentity, member_profile: &ProfileName) -> bool {
        let (kind, profile, member) = self.machine_parts();
        crate::machines::mob_machine::operator_instruction_target_applies(
            kind,
            &profile,
            member.as_deref(),
            Some(member_profile.as_str()),
            agent_identity.as_str(),
        )
    }

    /// The target as MobMachine holds it: the kind, the profile name of a
    /// Profile target (empty otherwise) and the member of a Member target.
    pub(crate) fn machine_parts(
        &self,
    ) -> (
        crate::machines::mob_machine::InstructionTargetKind,
        String,
        Option<String>,
    ) {
        use crate::machines::mob_machine::InstructionTargetKind;
        match self {
            Self::Mob => (InstructionTargetKind::Mob, String::new(), None),
            Self::Profile { profile } => (
                InstructionTargetKind::Profile,
                profile.as_str().to_owned(),
                None,
            ),
            Self::Member { agent_identity } => (
                InstructionTargetKind::Member,
                String::new(),
                Some(agent_identity.as_str().to_owned()),
            ),
        }
    }

    /// The target MobMachine holds for a record, back in the public
    /// vocabulary.
    pub(crate) fn from_machine_parts(
        kind: crate::machines::mob_machine::InstructionTargetKind,
        profile: &str,
        member: Option<&str>,
    ) -> Result<Self, MobError> {
        use crate::machines::mob_machine::InstructionTargetKind;
        match (kind, profile, member) {
            (InstructionTargetKind::Mob, "", None) => Ok(Self::Mob),
            (InstructionTargetKind::Profile, profile, None) if !profile.is_empty() => {
                Ok(Self::Profile {
                    profile: ProfileName::from(profile),
                })
            }
            (InstructionTargetKind::Member, "", Some(member)) if !member.is_empty() => {
                Ok(Self::Member {
                    agent_identity: AgentIdentity::from(member),
                })
            }
            (kind, profile, member) => Err(MobError::Internal(format!(
                "MobMachine holds a malformed operator instruction target: kind {kind:?}, \
                 profile {profile:?}, member {member:?}"
            ))),
        }
    }

    /// Refuse a target whose name is empty: a typed request cannot name
    /// nobody.
    pub(crate) fn validate(&self) -> Result<(), InstructionWriteError> {
        match self {
            Self::Mob => Ok(()),
            Self::Profile { profile } if profile.as_str().is_empty() => {
                Err(InstructionWriteError::InvalidRequest(
                    "a profile target names its profile".to_string(),
                ))
            }
            Self::Member { agent_identity } if agent_identity.as_str().is_empty() => {
                Err(InstructionWriteError::InvalidRequest(
                    "a member target names its member".to_string(),
                ))
            }
            Self::Profile { .. } | Self::Member { .. } => Ok(()),
        }
    }
}

/// One operator-instruction write: a slot's body at a revision, for the
/// members the target selects.
///
/// The revision is strictly increasing per slot within the mob: a lower
/// revision is refused stale, and the same revision with other content
/// (body or target) is refused as a conflict, while an exact repeat answers
/// as a duplicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorInstructionWrite {
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    pub body: String,
    /// The body's digest as the caller computed it; one that is not the
    /// body's is refused.
    pub content_sha256: Option<InstructionContentDigest>,
    pub target: OperatorInstructionTarget,
}

/// What a committed or repeated write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorInstructionDisposition {
    /// The record is committed and journaled: the members it selects (and
    /// those its previous target selected) are owed the new generation.
    Committed,
    /// An exact repeat of the slot's current revision: nothing changed.
    Duplicate,
}

/// The receipt of an operator-instruction write or revoke, returned once the
/// committed record is durable in the mob's journal. Delivery to the members
/// is asynchronous: the status read reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorInstructionReceipt {
    pub mob_id: MobId,
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    pub disposition: OperatorInstructionDisposition,
    /// The mob's operator-instruction generation after the write.
    pub generation: u64,
    /// How many current members the record's target selects: a target that
    /// names a missing profile or member shows zero.
    pub matched_members: u64,
}

/// Why an operator-instruction write or read failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OperatorInstructionError {
    /// The write was refused, typed; nothing changed.
    #[error(transparent)]
    Refused(InstructionWriteError),
    /// The mob is completed or destroyed and takes no write.
    #[error("the mob is completed or destroyed and takes no operator instruction write")]
    MobTerminal,
    /// The mob could not answer (admission closed, scope denied, the actor
    /// or its journal failed).
    #[error(transparent)]
    Mob(#[from] MobError),
}

impl OperatorInstructionError {
    /// The closed refusal code, when the failure has one.
    #[must_use]
    pub fn code(&self) -> Option<InstructionSetErrorCode> {
        match self {
            Self::Refused(error) => Some(error.code()),
            Self::MobTerminal => Some(InstructionSetErrorCode::MobTerminal),
            Self::Mob(_) => None,
        }
    }
}

impl From<InstructionWriteError> for OperatorInstructionError {
    fn from(error: InstructionWriteError) -> Self {
        Self::Refused(error)
    }
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// A committed operator-instruction write, as the mob journals it: the slot's
/// record at its revision and the mob's generation after the write. The
/// public event carries the body's digest and length; the body itself is
/// journaled for replay and delivery only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorInstructionCommittedEvent {
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<InstructionContentDigest>,
    /// The UTF-8 length of the Active body; zero while Absent.
    pub body_bytes: u64,
    pub target: OperatorInstructionTarget,
    /// The mob's operator-instruction generation after the write.
    pub generation: u64,
    /// The Active body: stored, never on the public event.
    #[serde(skip)]
    pub(crate) body: Option<String>,
}

impl OperatorInstructionCommittedEvent {
    /// The Active body the journal keeps for replay and delivery.
    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

/// The member targets one delivery wave recorded: each member and the
/// generation its session recorded. Replay closes the members' obligations
/// again, for the session each is bound to at that point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberInstructionTargetsRecordedEvent {
    pub members: Vec<RecordedMemberInstructionTarget>,
}

/// One member's recorded target in a delivery wave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedMemberInstructionTarget {
    pub agent_identity: AgentIdentity,
    pub generation: u64,
}

impl MobEventKind {
    /// Whether this event is one of the operator-instruction journal events,
    /// stored at mob event schema
    /// [`meerkat_core::OPERATOR_INSTRUCTION_MOB_EVENT_SCHEMA_VERSION`].
    #[must_use]
    pub fn journals_operator_instructions(&self) -> bool {
        matches!(
            self,
            Self::OperatorInstructionCommitted(_) | Self::MemberInstructionTargetsRecorded(_)
        )
    }

    pub(crate) fn operator_instruction_body(&self) -> Option<&str> {
        match self {
            Self::OperatorInstructionCommitted(committed) => committed.body.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn set_operator_instruction_body(&mut self, body: String) {
        if let Self::OperatorInstructionCommitted(committed) = self {
            committed.body = Some(body);
        }
    }
}

/// The stored type tags of the operator-instruction journal events.
pub(crate) const OPERATOR_INSTRUCTION_EVENT_TYPES: [&str; 2] = [
    "operator_instruction_committed",
    "member_instruction_targets_recorded",
];

// ---------------------------------------------------------------------------
// Sealed delivery capabilities
// ---------------------------------------------------------------------------

/// One member snapshot delivery MobMachine authorized: the member's whole
/// snapshot at one generation, for the session bound to the member.
///
/// Only the mob runtime mints one, from the machine's prepared delivery (or
/// a member's born carry); a session service records it through
/// [`crate::MobSessionService::record_member_instruction_target`] and cannot
/// fabricate one, so no other caller records a mob-owned target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInstructionDelivery {
    session_id: SessionId,
    record: InstructionTargetRecord,
}

impl MemberInstructionDelivery {
    pub(crate) fn authorized(session_id: SessionId, record: InstructionTargetRecord) -> Self {
        Self { session_id, record }
    }

    /// The member session the delivery is for.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The member's snapshot.
    #[must_use]
    pub fn record(&self) -> &InstructionTargetRecord {
        &self.record
    }

    /// The mob generation of the snapshot.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.record.generation
    }

    /// The session and the snapshot, for the session service that records
    /// it.
    #[must_use]
    pub fn into_parts(self) -> (SessionId, InstructionTargetRecord) {
        (self.session_id, self.record)
    }
}

/// One release MobMachine authorized: a retiring member's mob releases the
/// member session's target, so the session's host (or a later owner) can
/// own it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInstructionRelease {
    session_id: SessionId,
    mob_id: MobId,
}

impl MemberInstructionRelease {
    pub(crate) fn authorized(session_id: SessionId, mob_id: MobId) -> Self {
        Self { session_id, mob_id }
    }

    /// The member session whose target is released.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The mob that releases it.
    #[must_use]
    pub fn mob_id(&self) -> &MobId {
        &self.mob_id
    }
}

// ---------------------------------------------------------------------------
// Status read
// ---------------------------------------------------------------------------

/// Default and largest page of the status read.
pub const OPERATOR_INSTRUCTION_STATUS_DEFAULT_LIMIT: usize = 100;
pub const OPERATOR_INSTRUCTION_STATUS_MAX_LIMIT: usize = 200;

/// A query of a mob's operator-instruction status: the records (optionally
/// one namespace or slot) and one page of members (optionally one member or
/// one profile).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperatorInstructionStatusQuery {
    pub namespace: Option<InstructionNamespace>,
    pub key: Option<InstructionKey>,
    pub agent_identity: Option<AgentIdentity>,
    pub profile: Option<ProfileName>,
    pub offset: usize,
    /// 1 to 200 members; `None` reads 100.
    pub limit: Option<usize>,
}

/// One page of a mob's operator-instruction status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorInstructionStatusPage {
    pub mob_id: MobId,
    pub generation: u64,
    pub desired: Vec<OperatorInstructionRecordStatus>,
    pub members: Vec<MemberInstructionStatus>,
    pub next_offset: Option<usize>,
}

/// One desired record of the mob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorInstructionRecordStatus {
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
    pub target: OperatorInstructionTarget,
    pub content_sha256: Option<InstructionContentDigest>,
    pub body_bytes: Option<u64>,
}

/// Where a member's session lives now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberInstructionResidency {
    Live,
    Dormant,
    /// The member's seat has no session yet: its first wake creates it with
    /// the snapshot.
    Unborn,
}

/// Why a member's delivery is blocked until a write selects the member again
/// or it is seated again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberInstructionBlockReason {
    /// The member's host takes no operator-instruction delivery.
    MemberHostUnsupported,
    /// The member session's own instruction activations manage a slot of
    /// the snapshot.
    SlotManagedByActivation,
    /// The member's session service keeps no durable session document.
    DurabilityUnavailable,
    /// The member session's own host owns its target.
    OwnedBySession,
    /// Another mob owns the member session's target until it releases it.
    OwnedByOtherMob,
}

/// A member's delivery state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberInstructionDeliveryState {
    /// The member's session recorded its current snapshot.
    Current,
    /// The member is owed the snapshot at `generation`; the last retryable
    /// failure of its delivery, if any.
    Owed {
        generation: u64,
        last_failure: Option<String>,
    },
    /// The member's delivery is blocked.
    Blocked {
        reason: MemberInstructionBlockReason,
    },
}

/// A member's delivery facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInstructionDeliveryStatus {
    /// The generation the member's session recorded, as the mob knows it.
    pub recorded_generation: Option<u64>,
    pub state: MemberInstructionDeliveryState,
}

/// A slot's revision and effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberInstructionSlotView {
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
}

/// The entry that put a slot in effect in the member's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInstructionEffectiveSlot {
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
    pub entry_sequence: u64,
    pub kind: InstructionEntryKind,
    pub run_id: Option<meerkat_core::lifecycle::RunId>,
    pub message_index: usize,
}

/// Why a slot's desired view is not yet in effect in the member's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberInstructionPendingReason {
    /// Recorded; the session's next fresh run start applies it.
    AwaitingRunStart,
    /// Recorded; the member's model takes System rows only inside the
    /// leading System prefix, so it applies at a run start whose transcript
    /// is still that prefix, or at a compaction fold.
    LeadingPrefixOnlyLowering,
    /// Recorded; the member's model wire carries no System row an entry can
    /// take.
    LoweringUnavailable,
    /// The member's seat has no session yet.
    SessionUnborn,
    /// The mob has not recorded the snapshot in the member's session yet.
    DeliveryOwed,
}

/// A slot's state for one member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberInstructionSlotState {
    /// The member's session has the desired view in effect.
    Effective,
    Pending {
        reason: MemberInstructionPendingReason,
    },
    Blocked {
        reason: MemberInstructionBlockReason,
    },
    /// The record does not select the member, and the member's session has
    /// nothing of the slot.
    NotTargeted,
}

/// Whether the member's committed session state could be read. A member
/// whose session cannot be read is reported with its reason, and the page
/// still answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberInstructionEffectiveRead {
    Ok,
    Unreadable { reason: String },
}

/// One slot of one member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInstructionSlotStatus {
    pub slot: InstructionSlot,
    /// The member's view of the mob's record.
    pub desired: MemberInstructionSlotView,
    /// The member session's recorded target for the slot.
    pub recorded: Option<MemberInstructionSlotView>,
    /// The committed entry that put the slot in effect.
    pub effective: Option<MemberInstructionEffectiveSlot>,
    pub state: MemberInstructionSlotState,
}

/// One member's operator-instruction status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberInstructionStatus {
    pub agent_identity: AgentIdentity,
    pub profile: ProfileName,
    pub residency: MemberInstructionResidency,
    pub delivery: MemberInstructionDeliveryStatus,
    pub slots: Vec<MemberInstructionSlotStatus>,
    pub effective_read: MemberInstructionEffectiveRead,
    /// A live channel is open on the member's session. Operator instructions
    /// do not reach a live channel in this release: they apply to the
    /// member's turns.
    pub live_channel_open: bool,
}

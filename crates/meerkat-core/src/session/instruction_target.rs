//! The recorded operator-instruction target of a session, and the
//! session-document authority seam that decides every change to it.
//!
//! The target is the durable projection under
//! [`SESSION_INSTRUCTION_STATE_KEY`]: the owner, the target generation, the
//! entry-sequence high-water and one record per slot, with a body kept only
//! while its slot is pending (an applied body lives in its entry row). The
//! effective set is derived from the session's own entry rows
//! ([`effective_operator_instructions`]). Before every decision this module
//! seeds a fresh `SessionDocumentMachine` authority from both
//! (`RecoverSessionInstructionState`), feeds it the mechanical observations
//! the operation needs, and mirrors the verdict it emits. The machine decides
//! ownership, revision order, the slot limit, whether a run start appends an
//! entry, and what a compaction fold renders; this module decides nothing.
//!
//! Nothing here wakes a session or applies inside a running turn: a write or
//! a record only changes the target, and the only application points are a
//! fresh run start (one coalesced delta entry, appended before the run's own
//! inputs) and a compaction fold at a run's first request.

use super::operator_instructions::{
    ActiveOperatorInstruction, EffectiveInstructionSlot, EffectiveOperatorInstructions,
    InstructionEffect, InstructionRevision, InstructionSlot, MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
    MAX_OPERATOR_INSTRUCTION_SLOTS, OperatorInstructionChangeContent,
    OperatorInstructionEntryHeader, effective_operator_instructions,
    mint_operator_instruction_delta,
};
use super::{MAX_INSTRUCTION_BODY_BYTES, SESSION_INSTRUCTION_STATE_KEY, Session};
use crate::generated::session_document as document;
use crate::lifecycle::RunId;
use crate::types::{InstructionContentDigest, InstructionKey, InstructionNamespace, Message};
use serde::{Deserialize, Serialize};

/// Who owns a session's recorded operator-instruction target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstructionTargetOwner {
    /// The session's own host writes the target slot by slot.
    Session,
    /// A mob delivers the member's whole snapshot by generation.
    Mob {
        mob_id: String,
        agent_identity: String,
    },
}

impl InstructionTargetOwner {
    fn machine_owner(&self) -> document::InstructionTargetOwner {
        match self {
            Self::Session => document::InstructionTargetOwner::Session,
            Self::Mob { .. } => document::InstructionTargetOwner::Mob,
        }
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// One slot of a session's recorded target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionTargetSlot {
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
    /// The digest of the body while the slot is Active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<InstructionContentDigest>,
    /// The UTF-8 length of the Active body; zero while Absent.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub body_bytes: u64,
    /// The body while the slot is pending (recorded, not yet applied by an
    /// entry). An applied body lives in its entry row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_body: Option<String>,
}

impl InstructionTargetSlot {
    #[must_use]
    pub fn slot(&self) -> InstructionSlot {
        InstructionSlot {
            namespace: self.namespace.clone(),
            key: self.key.clone(),
        }
    }

    fn names(&self, slot: &InstructionSlot) -> bool {
        self.namespace == slot.namespace && self.key == slot.key
    }

    fn order_key(&self) -> (&InstructionNamespace, &InstructionKey) {
        (&self.namespace, &self.key)
    }

    fn active(slot: &InstructionSlot, revision: InstructionRevision, body: String) -> Self {
        Self {
            namespace: slot.namespace.clone(),
            key: slot.key.clone(),
            revision,
            effect: InstructionEffect::Active,
            content_sha256: Some(InstructionContentDigest::for_body(&body)),
            body_bytes: body.len() as u64,
            pending_body: Some(body),
        }
    }

    fn revoked(slot: &InstructionSlot, revision: InstructionRevision) -> Self {
        Self {
            namespace: slot.namespace.clone(),
            key: slot.key.clone(),
            revision,
            effect: InstructionEffect::Absent,
            content_sha256: None,
            body_bytes: 0,
            pending_body: None,
        }
    }
}

/// A session's recorded operator-instruction target: its owner, the target
/// generation, the entry-sequence high-water and one record per slot, sorted
/// by slot. Stored under [`SESSION_INSTRUCTION_STATE_KEY`]; a document that
/// carries it is written at envelope 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionInstructionState {
    owner: InstructionTargetOwner,
    target_generation: u64,
    entry_sequence: u64,
    #[serde(default)]
    slots: Vec<InstructionTargetSlot>,
}

impl SessionInstructionState {
    #[must_use]
    pub fn owner(&self) -> &InstructionTargetOwner {
        &self.owner
    }

    /// The generation of the recorded target: a mob's snapshot generation,
    /// or the count of a session owner's recorded writes since it took the
    /// target.
    #[must_use]
    pub fn target_generation(&self) -> u64 {
        self.target_generation
    }

    /// The highest entry sequence this session minted.
    #[must_use]
    pub fn entry_sequence(&self) -> u64 {
        self.entry_sequence
    }

    /// Every recorded slot, sorted by slot.
    #[must_use]
    pub fn slots(&self) -> &[InstructionTargetSlot] {
        &self.slots
    }

    #[must_use]
    pub fn slot(&self, slot: &InstructionSlot) -> Option<&InstructionTargetSlot> {
        self.slots
            .binary_search_by(|record| record.order_key().cmp(&(&slot.namespace, &slot.key)))
            .ok()
            .and_then(|index| self.slots.get(index))
    }

    fn upsert(&mut self, record: InstructionTargetSlot) {
        match self
            .slots
            .binary_search_by(|current| current.order_key().cmp(&record.order_key()))
        {
            Ok(index) => {
                if let Some(current) = self.slots.get_mut(index) {
                    *current = record;
                }
            }
            Err(index) => self.slots.insert(index, record),
        }
    }

    fn active_body_bytes(&self) -> u64 {
        self.slots
            .iter()
            .filter(|record| record.effect == InstructionEffect::Active)
            .fold(0u64, |total, record| {
                total.saturating_add(record.body_bytes)
            })
    }

    fn clear_pending_bodies(&mut self) {
        for record in &mut self.slots {
            record.pending_body = None;
        }
    }

    /// The recorded shape: sorted unique slots within the slot and byte
    /// budgets, a digest and a length exactly for Active slots, and a
    /// pending body that is the recorded content.
    fn validate(&self) -> Result<(), String> {
        if self.slots.len() > MAX_OPERATOR_INSTRUCTION_SLOTS {
            return Err(format!(
                "the target holds more than {MAX_OPERATOR_INSTRUCTION_SLOTS} slots"
            ));
        }
        if !self
            .slots
            .windows(2)
            .all(|pair| pair[0].order_key() < pair[1].order_key())
        {
            return Err("the target's slots must be sorted by slot without repeats".to_string());
        }
        for record in &self.slots {
            let slot = record.slot();
            match record.effect {
                InstructionEffect::Active => {
                    let Some(digest) = record.content_sha256.as_ref() else {
                        return Err(format!("the Active slot {slot} has no content digest"));
                    };
                    if record.body_bytes > MAX_INSTRUCTION_BODY_BYTES as u64 {
                        return Err(format!(
                            "the body of {slot} exceeds {MAX_INSTRUCTION_BODY_BYTES} UTF-8 bytes"
                        ));
                    }
                    if let Some(body) = record.pending_body.as_deref()
                        && (body.len() as u64 != record.body_bytes
                            || InstructionContentDigest::for_body(body) != *digest)
                    {
                        return Err(format!(
                            "the pending body of {slot} is not its recorded content"
                        ));
                    }
                }
                InstructionEffect::Absent => {
                    if record.content_sha256.is_some()
                        || record.body_bytes != 0
                        || record.pending_body.is_some()
                    {
                        return Err(format!("the revoked slot {slot} carries a body"));
                    }
                }
            }
        }
        if self.active_body_bytes() > MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES as u64 {
            return Err(format!(
                "the target's Active bodies exceed {MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES} UTF-8 \
                 bytes"
            ));
        }
        Ok(())
    }
}

/// The closed vocabulary of operator-instruction write refusals, as hosts
/// and SDKs read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstructionSetErrorCode {
    InvalidRequest,
    DigestMismatch,
    StaleRevision,
    RevisionConflict,
    UnknownSlot,
    SlotLimitExceeded,
    BodyTooLarge,
    ActiveSetBudgetExceeded,
    OwnedByMob,
    OwnedBySession,
    SlotManagedByActivation,
    DurabilityUnavailable,
    MobTerminal,
    MalformedInstructionHistory,
}

/// Why the recorded target or the session's entry rows cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstructionStateError {
    #[error("the recorded operator-instruction target is malformed: {0}")]
    MalformedState(String),
    #[error("the session's operator-instruction entries are malformed: {0}")]
    MalformedHistory(String),
}

/// Why an operator-instruction write, record or rebind was refused. A
/// refusal changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstructionWriteError {
    #[error("invalid operator instruction request: {0}")]
    InvalidRequest(String),
    #[error("operator instruction body digest mismatch: declared {declared}, computed {computed}")]
    DigestMismatch {
        declared: InstructionContentDigest,
        computed: InstructionContentDigest,
    },
    #[error("operator instruction revision {revision} is below the recorded revision {current}")]
    StaleRevision {
        revision: InstructionRevision,
        current: InstructionRevision,
    },
    #[error("operator instruction revision {revision} is already bound to other content")]
    RevisionConflict { revision: InstructionRevision },
    #[error("operator instruction slot {slot} has no record to revoke")]
    UnknownSlot { slot: InstructionSlot },
    #[error("the operator instruction target already holds {max} slots", max = MAX_OPERATOR_INSTRUCTION_SLOTS)]
    SlotLimitExceeded,
    #[error("operator instruction body exceeds {max_bytes} UTF-8 bytes")]
    BodyTooLarge { max_bytes: usize },
    #[error("the operator instruction Active bodies would exceed {max_bytes} UTF-8 bytes")]
    ActiveSetBudgetExceeded { max_bytes: usize },
    #[error("the operator instruction target is owned by a mob")]
    OwnedByMob,
    #[error("the operator instruction target is owned by the session")]
    OwnedBySession,
    #[error(
        "operator instruction slot {slot} is managed by instruction activations in this session"
    )]
    SlotManagedByActivation { slot: InstructionSlot },
    #[error(transparent)]
    Malformed(#[from] InstructionStateError),
}

impl InstructionWriteError {
    #[must_use]
    pub const fn code(&self) -> InstructionSetErrorCode {
        match self {
            Self::InvalidRequest(_) => InstructionSetErrorCode::InvalidRequest,
            Self::DigestMismatch { .. } => InstructionSetErrorCode::DigestMismatch,
            Self::StaleRevision { .. } => InstructionSetErrorCode::StaleRevision,
            Self::RevisionConflict { .. } => InstructionSetErrorCode::RevisionConflict,
            Self::UnknownSlot { .. } => InstructionSetErrorCode::UnknownSlot,
            Self::SlotLimitExceeded => InstructionSetErrorCode::SlotLimitExceeded,
            Self::BodyTooLarge { .. } => InstructionSetErrorCode::BodyTooLarge,
            Self::ActiveSetBudgetExceeded { .. } => {
                InstructionSetErrorCode::ActiveSetBudgetExceeded
            }
            Self::OwnedByMob => InstructionSetErrorCode::OwnedByMob,
            Self::OwnedBySession => InstructionSetErrorCode::OwnedBySession,
            Self::SlotManagedByActivation { .. } => {
                InstructionSetErrorCode::SlotManagedByActivation
            }
            Self::Malformed(_) => InstructionSetErrorCode::MalformedInstructionHistory,
        }
    }
}

/// What a session-owned write does to its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionInstructionChange {
    /// The body applies from the next fresh run start. A declared digest
    /// must be the body's.
    Activate {
        body: String,
        content_sha256: Option<InstructionContentDigest>,
    },
    /// The slot's body stops applying from the next fresh run start.
    Revoke,
}

/// One session-owned operator-instruction write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInstructionWrite {
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    pub change: SessionInstructionChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionWriteDisposition {
    /// The write changed the target.
    Recorded,
    /// An exact retry: the target already holds this revision and content.
    Duplicate,
}

/// The receipt of a session-owned write: the target is in the session
/// document once the caller commits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInstructionReceipt {
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    pub disposition: InstructionWriteDisposition,
    pub target_generation: u64,
}

/// One slot of a mob member's snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionTargetRecordSlot {
    pub slot: InstructionSlot,
    pub revision: InstructionRevision,
    /// The body when the mob record applies to this member; none when it is
    /// Absent for it (revoked, or not targeted at this member).
    pub body: Option<String>,
}

/// A mob member's whole operator-instruction snapshot at one mob
/// generation, as mob delivery records it in the member's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionTargetRecord {
    pub mob_id: String,
    pub agent_identity: String,
    pub generation: u64,
    pub slots: Vec<InstructionTargetRecordSlot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionRecordDisposition {
    /// The snapshot replaced the target.
    Recorded,
    /// The session already recorded this generation or a later one: a
    /// repeated delivery writes nothing.
    AlreadyRecorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstructionRecordReceipt {
    pub disposition: InstructionRecordDisposition,
    /// The generation the session holds after the record.
    pub target_generation: u64,
}

/// How a turn's run starts, from its resolved start disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionRunStart {
    /// A content turn, or a run of a pending tail.
    FreshRun,
    /// A run resumed after an interruption: it continues, nothing applies.
    ResumeRun,
    /// A turn with no pending boundary runs nothing.
    NoRun,
}

impl From<InstructionRunStart> for document::InstructionRunStart {
    fn from(value: InstructionRunStart) -> Self {
        match value {
            InstructionRunStart::FreshRun => Self::FreshRun,
            InstructionRunStart::ResumeRun => Self::ResumeRun,
            InstructionRunStart::NoRun => Self::NoRun,
        }
    }
}

/// How the session's model carries a System row after conversation rows,
/// from its resolved capability surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionLowering {
    /// System rows lower at any transcript position.
    MidConversation,
    /// System rows lower only inside the leading System prefix.
    LeadingPrefixOnly,
    /// The wire carries no System row; nothing is ever lowered as anything
    /// else.
    Unavailable,
}

impl InstructionLowering {
    /// The lowering of a model as its backend serves it
    /// (`ModelRegistry::profile_for_identity`): a model that lowers ordered
    /// System rows after conversation rows takes an entry anywhere; any other
    /// model, and one without a resolved profile, takes one only inside the
    /// leading System prefix, which every wire carries as its system
    /// instructions.
    #[must_use]
    pub fn for_resolved_profile(profile: Option<&crate::model_profile::ModelProfile>) -> Self {
        match profile {
            Some(profile) if profile.supports_mid_conversation_system_messages => {
                Self::MidConversation
            }
            _ => Self::LeadingPrefixOnly,
        }
    }
}

impl From<InstructionLowering> for document::InstructionLowering {
    fn from(value: InstructionLowering) -> Self {
        match value {
            InstructionLowering::MidConversation => Self::MidConversation,
            InstructionLowering::LeadingPrefixOnly => Self::LeadingPrefixOnly,
            InstructionLowering::Unavailable => Self::Unavailable,
        }
    }
}

impl From<InstructionEffect> for document::InstructionEffect {
    fn from(value: InstructionEffect) -> Self {
        match value {
            InstructionEffect::Absent => Self::Absent,
            InstructionEffect::Active => Self::Active,
        }
    }
}

/// Why a fresh run start left pending changes pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionDeferReason {
    /// The model lowers System rows only inside the leading System prefix,
    /// and the transcript has conversation rows.
    LeadingPrefixOnly,
    /// The model's wire carries no System row.
    LoweringUnavailable,
}

impl From<document::InstructionDeferReason> for InstructionDeferReason {
    fn from(value: document::InstructionDeferReason) -> Self {
        match value {
            document::InstructionDeferReason::LeadingPrefixOnly => Self::LeadingPrefixOnly,
            document::InstructionDeferReason::LoweringUnavailable => Self::LoweringUnavailable,
        }
    }
}

/// What a run start did with the recorded target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStartInstructionOutcome {
    /// One delta entry, carrying every pending change, was appended at the
    /// transcript tail. It is effective once the run's commit persists it.
    Applied {
        entry_sequence: u64,
        target_generation: u64,
    },
    /// Nothing to apply, or not a fresh run start.
    NotNeeded,
    /// Pending changes stay pending.
    Deferred(InstructionDeferReason),
}

/// The instruction region's per-session registry key: every operation seeds
/// one fresh authority for its one session.
const SESSION_DOCUMENT_INSTRUCTION_KEY: &str = "operator_instructions";

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// A `SessionDocumentMachine` authority seeded with one session's
/// recorded target and effective set.
struct InstructionDocument {
    authority: document::SessionDocumentMachineAuthority,
    key: document::SessionDocumentKey,
}

impl InstructionDocument {
    fn seed(
        state: Option<&SessionInstructionState>,
        effective: &EffectiveOperatorInstructions,
    ) -> Result<Self, InstructionStateError> {
        let mut authority = document::SessionDocumentMachineAuthority::new();
        let key = document::SessionDocumentKey::new(SESSION_DOCUMENT_INSTRUCTION_KEY);
        let (owner, generation, slot_count, recorded_sequence) = state.map_or(
            (document::InstructionTargetOwner::Unowned, 0, 0, 0),
            |state| {
                (
                    state.owner.machine_owner(),
                    state.target_generation,
                    count(state.slots.len()),
                    state.entry_sequence,
                )
            },
        );
        authority
            .recover_session_instruction_state(
                key.clone(),
                owner,
                generation,
                slot_count,
                count(effective.active_count()),
                recorded_sequence.max(effective.last_entry_sequence()),
            )
            .map_err(|error| {
                InstructionStateError::MalformedState(format!(
                    "the session-document authority refused the recorded projection: {error}"
                ))
            })?;
        Ok(Self { authority, key })
    }

    fn key(&self) -> document::SessionDocumentKey {
        self.key.clone()
    }
}

fn refused_by_authority(error: &document::SessionDocumentError) -> InstructionStateError {
    InstructionStateError::MalformedState(format!(
        "the session-document authority refused a seeded operation: {error}"
    ))
}

/// The verdict of a write or a record.
enum WriteVerdict {
    Recorded(u64),
    Duplicate(u64),
    Refused(document::InstructionWriteRefusal),
}

fn write_verdict(
    effects: &[document::SessionDocumentEffect],
) -> Result<WriteVerdict, InstructionStateError> {
    effects
        .iter()
        .find_map(|effect| match effect {
            document::SessionDocumentEffect::InstructionTargetRecorded {
                generation,
                outcome,
            } => Some(match outcome {
                document::InstructionWriteOutcome::Recorded => WriteVerdict::Recorded(*generation),
                document::InstructionWriteOutcome::Duplicate => {
                    WriteVerdict::Duplicate(*generation)
                }
            }),
            document::SessionDocumentEffect::InstructionTargetRefused { refusal } => {
                Some(WriteVerdict::Refused(*refusal))
            }
            _ => None,
        })
        .ok_or_else(|| {
            InstructionStateError::MalformedState(
                "the session-document authority emitted no write verdict".to_string(),
            )
        })
}

/// The verdict of a run start.
enum RunStartVerdict {
    Apply {
        entry_sequence: u64,
        generation: u64,
    },
    NotNeeded,
    Deferred(document::InstructionDeferReason),
}

/// Everything a run start would apply: the changes from the effective set to
/// the recorded target, and the whole active set after them.
struct PendingInstructionChanges {
    changes: Vec<OperatorInstructionChangeContent>,
    active_after: Vec<ActiveOperatorInstruction>,
}

/// Compare the recorded target with the effective set, slot by slot. A slot
/// is unchanged when the target holds what is in effect (the same revision
/// and content, or Absent where nothing applies); an effective slot the
/// target no longer names is revoked at the revision in effect.
fn pending_instruction_changes(
    state: &SessionInstructionState,
    effective: &EffectiveOperatorInstructions,
) -> Result<PendingInstructionChanges, InstructionStateError> {
    let mut changes = Vec::new();
    let mut active_after = Vec::new();
    for record in &state.slots {
        let slot = record.slot();
        let in_effect = effective
            .slot(&slot)
            .filter(|current| current.effect() == InstructionEffect::Active);
        match record.effect {
            InstructionEffect::Active => {
                let same_content = |current: &&EffectiveInstructionSlot| {
                    current.content_sha256() == record.content_sha256.as_ref()
                };
                let body = match (record.pending_body.as_deref(), in_effect) {
                    (Some(body), _) => body.to_string(),
                    (None, Some(current)) if same_content(&current) => {
                        current.body().map(str::to_string).ok_or_else(|| {
                            InstructionStateError::MalformedHistory(format!(
                                "the effective body of {slot} is missing"
                            ))
                        })?
                    }
                    (None, _) => {
                        return Err(InstructionStateError::MalformedState(format!(
                            "the recorded body of {slot} is neither pending nor in effect"
                        )));
                    }
                };
                let unchanged = in_effect.is_some_and(|current| {
                    current.revision() == record.revision && same_content(&current)
                });
                if !unchanged {
                    changes.push(OperatorInstructionChangeContent::Activate {
                        slot: slot.clone(),
                        revision: record.revision,
                        supersedes: in_effect.map(EffectiveInstructionSlot::revision),
                        body: body.clone(),
                    });
                }
                active_after.push(ActiveOperatorInstruction {
                    slot,
                    revision: record.revision,
                    body,
                });
            }
            InstructionEffect::Absent => {
                if let Some(current) = in_effect {
                    changes.push(OperatorInstructionChangeContent::Revoke {
                        slot,
                        revision: record.revision,
                        revokes: current.revision(),
                    });
                }
            }
        }
    }
    for (slot, current) in effective.slots() {
        if current.effect() == InstructionEffect::Active && state.slot(slot).is_none() {
            changes.push(OperatorInstructionChangeContent::Revoke {
                slot: slot.clone(),
                revision: current.revision(),
                revokes: current.revision(),
            });
        }
    }
    Ok(PendingInstructionChanges {
        changes,
        active_after,
    })
}

impl Session {
    /// The recorded operator-instruction target, if one was ever recorded.
    /// A recorded target that does not read back typed fails closed.
    pub fn instruction_target_state(
        &self,
    ) -> Result<Option<SessionInstructionState>, InstructionStateError> {
        let Some(value) = self.metadata.get(SESSION_INSTRUCTION_STATE_KEY) else {
            return Ok(None);
        };
        let state: SessionInstructionState = serde_json::from_value(value.clone())
            .map_err(|error| InstructionStateError::MalformedState(error.to_string()))?;
        state
            .validate()
            .map_err(InstructionStateError::MalformedState)?;
        Ok(Some(state))
    }

    fn store_instruction_target_state(
        &mut self,
        state: &SessionInstructionState,
    ) -> Result<(), InstructionStateError> {
        state
            .validate()
            .map_err(InstructionStateError::MalformedState)?;
        let value = serde_json::to_value(state)
            .map_err(|error| InstructionStateError::MalformedState(error.to_string()))?;
        self.set_metadata_unchecked(SESSION_INSTRUCTION_STATE_KEY, value);
        Ok(())
    }

    fn own_effective_instructions(
        &self,
    ) -> Result<EffectiveOperatorInstructions, InstructionStateError> {
        effective_operator_instructions(self.id(), self.messages())
            .map_err(InstructionStateError::MalformedHistory)
    }

    /// Whether an instruction activation row names `slot`: such a slot is
    /// managed by activations in this session and takes no operator
    /// instruction.
    fn slot_has_activation_rows(&self, slot: &InstructionSlot) -> bool {
        self.messages().iter().any(|message| {
            matches!(
                message,
                Message::System(system)
                    if system.instruction_activation.as_ref().is_some_and(|identity| {
                        identity.revision.namespace == slot.namespace
                            && identity.revision.key == slot.key
                    })
            )
        })
    }

    /// Whether the recorded target or the effective set names `slot`: such
    /// a slot takes no instruction activation.
    pub(crate) fn names_operator_instruction_slot(
        &self,
        slot: &InstructionSlot,
    ) -> Result<bool, InstructionStateError> {
        let recorded = self
            .instruction_target_state()?
            .is_some_and(|state| state.slot(slot).is_some());
        Ok(recorded || self.own_effective_instructions()?.slot(slot).is_some())
    }

    /// Record one session-owned write in the target. The session-document
    /// authority decides it against the slot's record; a recorded write
    /// changes only the target (it applies at the next fresh run start), and
    /// a refusal or a duplicate changes nothing.
    pub fn set_session_instruction(
        &mut self,
        write: SessionInstructionWrite,
    ) -> Result<SessionInstructionReceipt, InstructionWriteError> {
        let (effect, body) = match write.change {
            SessionInstructionChange::Activate {
                body,
                content_sha256,
            } => {
                if body.len() > MAX_INSTRUCTION_BODY_BYTES {
                    return Err(InstructionWriteError::BodyTooLarge {
                        max_bytes: MAX_INSTRUCTION_BODY_BYTES,
                    });
                }
                let computed = InstructionContentDigest::for_body(&body);
                if let Some(declared) = content_sha256
                    && declared != computed
                {
                    return Err(InstructionWriteError::DigestMismatch { declared, computed });
                }
                (InstructionEffect::Active, Some(body))
            }
            SessionInstructionChange::Revoke => (InstructionEffect::Absent, None),
        };
        if self.slot_has_activation_rows(&write.slot) {
            return Err(InstructionWriteError::SlotManagedByActivation { slot: write.slot });
        }
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let current = state.as_ref().and_then(|state| state.slot(&write.slot));
        let digest = body.as_deref().map(InstructionContentDigest::for_body);
        let current_matches = current
            .is_some_and(|record| record.effect == effect && record.content_sha256 == digest);
        let current_revision = current.map(|record| record.revision);
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let effects = document
            .authority
            .set_session_instruction(
                key,
                write.revision.get(),
                effect.into(),
                current.is_some(),
                current_revision.map_or(0, InstructionRevision::get),
                current_matches,
            )
            .map_err(|error| refused_by_authority(&error))?;
        let receipt = |disposition, target_generation| SessionInstructionReceipt {
            slot: write.slot.clone(),
            revision: write.revision,
            disposition,
            target_generation,
        };
        match write_verdict(&effects)? {
            WriteVerdict::Recorded(generation) => {
                let record = match body {
                    Some(body) => InstructionTargetSlot::active(&write.slot, write.revision, body),
                    None => InstructionTargetSlot::revoked(&write.slot, write.revision),
                };
                let mut next = state.unwrap_or(SessionInstructionState {
                    owner: InstructionTargetOwner::Session,
                    target_generation: 0,
                    entry_sequence: 0,
                    slots: Vec::new(),
                });
                next.owner = InstructionTargetOwner::Session;
                next.target_generation = generation;
                next.entry_sequence = next.entry_sequence.max(effective.last_entry_sequence());
                next.upsert(record);
                if next.active_body_bytes() > MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES as u64 {
                    return Err(InstructionWriteError::ActiveSetBudgetExceeded {
                        max_bytes: MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
                    });
                }
                self.store_instruction_target_state(&next)?;
                Ok(receipt(InstructionWriteDisposition::Recorded, generation))
            }
            WriteVerdict::Duplicate(generation) => {
                Ok(receipt(InstructionWriteDisposition::Duplicate, generation))
            }
            WriteVerdict::Refused(refusal) => Err(match refusal {
                document::InstructionWriteRefusal::StaleRevision => {
                    InstructionWriteError::StaleRevision {
                        revision: write.revision,
                        current: current_revision.unwrap_or(write.revision),
                    }
                }
                document::InstructionWriteRefusal::RevisionConflict => {
                    InstructionWriteError::RevisionConflict {
                        revision: write.revision,
                    }
                }
                document::InstructionWriteRefusal::UnknownSlot => {
                    InstructionWriteError::UnknownSlot { slot: write.slot }
                }
                document::InstructionWriteRefusal::SlotLimitExceeded => {
                    InstructionWriteError::SlotLimitExceeded
                }
                document::InstructionWriteRefusal::OwnedByMob => InstructionWriteError::OwnedByMob,
                document::InstructionWriteRefusal::OwnedBySession => {
                    InstructionWriteError::OwnedBySession
                }
            }),
        }
    }

    /// Record a mob member's whole snapshot in the target. The
    /// session-document authority accepts a snapshot by generation alone: a
    /// generation at or below the recorded one answers "already recorded"
    /// and writes nothing, so a repeated delivery never writes twice.
    /// Recording changes only the target and wakes nothing.
    pub fn record_instruction_target(
        &mut self,
        record: InstructionTargetRecord,
    ) -> Result<InstructionRecordReceipt, InstructionWriteError> {
        if record.generation == 0 {
            return Err(InstructionWriteError::InvalidRequest(
                "a recorded operator instruction target needs a generation of at least 1"
                    .to_string(),
            ));
        }
        if record.slots.len() > MAX_OPERATOR_INSTRUCTION_SLOTS {
            return Err(InstructionWriteError::SlotLimitExceeded);
        }
        let mut slots = Vec::with_capacity(record.slots.len());
        for entry in record.slots {
            let target = match entry.body {
                Some(body) => {
                    if body.len() > MAX_INSTRUCTION_BODY_BYTES {
                        return Err(InstructionWriteError::BodyTooLarge {
                            max_bytes: MAX_INSTRUCTION_BODY_BYTES,
                        });
                    }
                    if self.slot_has_activation_rows(&entry.slot) {
                        return Err(InstructionWriteError::SlotManagedByActivation {
                            slot: entry.slot,
                        });
                    }
                    InstructionTargetSlot::active(&entry.slot, entry.revision, body)
                }
                None => InstructionTargetSlot::revoked(&entry.slot, entry.revision),
            };
            slots.push(target);
        }
        slots.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        if slots
            .windows(2)
            .any(|pair| pair[0].order_key() == pair[1].order_key())
        {
            return Err(InstructionWriteError::InvalidRequest(
                "an operator instruction snapshot names a slot twice".to_string(),
            ));
        }
        let active_bytes = slots
            .iter()
            .filter(|slot| slot.effect == InstructionEffect::Active)
            .fold(0u64, |total, slot| total.saturating_add(slot.body_bytes));
        if active_bytes > MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES as u64 {
            return Err(InstructionWriteError::ActiveSetBudgetExceeded {
                max_bytes: MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
            });
        }
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let effects = document
            .authority
            .record_instruction_target(key, record.generation, count(slots.len()))
            .map_err(|error| refused_by_authority(&error))?;
        match write_verdict(&effects)? {
            WriteVerdict::Recorded(generation) => {
                // A body the effective set already holds lives in its entry
                // row; only a pending body is kept.
                for slot in &mut slots {
                    let in_effect = effective.slot(&slot.slot()).is_some_and(|current| {
                        current.effect() == InstructionEffect::Active
                            && current.revision() == slot.revision
                            && current.content_sha256() == slot.content_sha256.as_ref()
                    });
                    if in_effect {
                        slot.pending_body = None;
                    }
                }
                let entry_sequence = state
                    .as_ref()
                    .map_or(0, SessionInstructionState::entry_sequence)
                    .max(effective.last_entry_sequence());
                self.store_instruction_target_state(&SessionInstructionState {
                    owner: InstructionTargetOwner::Mob {
                        mob_id: record.mob_id,
                        agent_identity: record.agent_identity,
                    },
                    target_generation: generation,
                    entry_sequence,
                    slots,
                })?;
                Ok(InstructionRecordReceipt {
                    disposition: InstructionRecordDisposition::Recorded,
                    target_generation: generation,
                })
            }
            WriteVerdict::Duplicate(generation) => Ok(InstructionRecordReceipt {
                disposition: InstructionRecordDisposition::AlreadyRecorded,
                target_generation: generation,
            }),
            WriteVerdict::Refused(document::InstructionWriteRefusal::OwnedBySession) => {
                Err(InstructionWriteError::OwnedBySession)
            }
            WriteVerdict::Refused(other) => Err(InstructionWriteError::Malformed(
                InstructionStateError::MalformedState(format!(
                    "the session-document authority refused a record with {other:?}"
                )),
            )),
        }
    }

    /// Rebind the target to a new owner, on an explicit attach or a host's
    /// standalone adoption. The new owner starts from what is in effect: the
    /// target becomes the effective set, its generation restarts, and no
    /// pending write of the previous owner carries over. Rebinding to the
    /// owner the target already has is refused.
    pub fn rebind_instruction_owner(
        &mut self,
        owner: InstructionTargetOwner,
    ) -> Result<(), InstructionWriteError> {
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        document
            .authority
            .rebind_instruction_owner(key, owner.machine_owner())
            .map_err(|error| {
                InstructionWriteError::InvalidRequest(format!(
                    "the operator instruction target cannot be rebound to this owner: {error}"
                ))
            })?;
        let target_generation = document
            .authority
            .session_instruction_target_generation_for(&document.key)
            .unwrap_or(0);
        let slots = effective
            .slots()
            .filter_map(|(slot, current)| {
                let body = current.body()?;
                Some(InstructionTargetSlot {
                    namespace: slot.namespace.clone(),
                    key: slot.key.clone(),
                    revision: current.revision(),
                    effect: InstructionEffect::Active,
                    content_sha256: current.content_sha256().cloned(),
                    body_bytes: body.len() as u64,
                    pending_body: None,
                })
            })
            .collect();
        let entry_sequence = state
            .as_ref()
            .map_or(0, SessionInstructionState::entry_sequence)
            .max(effective.last_entry_sequence());
        self.store_instruction_target_state(&SessionInstructionState {
            owner,
            target_generation,
            entry_sequence,
            slots,
        })?;
        Ok(())
    }

    /// At a turn's run start, after its start disposition is resolved and
    /// before any input of the run is appended: let the session-document
    /// authority decide whether the pending changes apply now and, if they
    /// do, append the one delta entry that carries all of them at the
    /// transcript tail. The entry and the cleared pending bodies persist with
    /// the run's commits; a run that commits nothing leaves the changes
    /// pending for the next run start.
    pub fn apply_run_start_instructions(
        &mut self,
        run_start: InstructionRunStart,
        lowering: InstructionLowering,
        run_id: Option<RunId>,
    ) -> Result<RunStartInstructionOutcome, InstructionStateError> {
        let Some(mut state) = self.instruction_target_state()? else {
            // No target was ever recorded, so nothing can be pending.
            return Ok(RunStartInstructionOutcome::NotNeeded);
        };
        let effective = self.own_effective_instructions()?;
        let pending = pending_instruction_changes(&state, &effective)?;
        let transcript_is_system_prefix = self
            .messages()
            .iter()
            .all(|message| matches!(message, Message::System(_)));
        let mut document = InstructionDocument::seed(Some(&state), &effective)?;
        let key = document.key();
        let effects = document
            .authority
            .resolve_run_start_instructions(
                key,
                run_start.into(),
                lowering.into(),
                transcript_is_system_prefix,
                count(pending.changes.len()),
                count(pending.active_after.len()),
            )
            .map_err(|error| refused_by_authority(&error))?;
        let verdict = effects
            .iter()
            .find_map(|effect| match effect {
                document::SessionDocumentEffect::InstructionEntryAuthorized {
                    entry_sequence,
                    generation,
                } => Some(RunStartVerdict::Apply {
                    entry_sequence: *entry_sequence,
                    generation: *generation,
                }),
                document::SessionDocumentEffect::InstructionEntryNotNeeded => {
                    Some(RunStartVerdict::NotNeeded)
                }
                document::SessionDocumentEffect::InstructionApplicationDeferred { reason } => {
                    Some(RunStartVerdict::Deferred(*reason))
                }
                _ => None,
            })
            .ok_or_else(|| {
                InstructionStateError::MalformedState(
                    "the session-document authority emitted no run-start verdict".to_string(),
                )
            })?;
        match verdict {
            RunStartVerdict::Apply {
                entry_sequence,
                generation,
            } => {
                let entry = mint_operator_instruction_delta(
                    OperatorInstructionEntryHeader {
                        origin_session_id: self.id().clone(),
                        entry_sequence,
                        target_generation: generation,
                        run_id,
                    },
                    pending.active_after,
                    pending.changes,
                    crate::types::message_timestamp_now(),
                )
                .map_err(InstructionStateError::MalformedHistory)?;
                self.append_operator_instruction_entry(entry)
                    .map_err(InstructionStateError::MalformedHistory)?;
                state.entry_sequence = entry_sequence;
                state.clear_pending_bodies();
                self.store_instruction_target_state(&state)?;
                Ok(RunStartInstructionOutcome::Applied {
                    entry_sequence,
                    target_generation: generation,
                })
            }
            RunStartVerdict::NotNeeded => Ok(RunStartInstructionOutcome::NotNeeded),
            RunStartVerdict::Deferred(reason) => {
                Ok(RunStartInstructionOutcome::Deferred(reason.into()))
            }
        }
    }
}

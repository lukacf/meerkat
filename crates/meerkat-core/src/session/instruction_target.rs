//! The recorded operator-instruction target of a session, and the
//! session-document authority seam that decides every change to it.
//!
//! `SessionDocumentMachine` holds a session's operator-instruction facts per
//! slot: the recorded target (revision, effect, digest and body bytes per
//! slot), its owner (and the mob a mob owner is) and generation, the slots in
//! effect (the same per slot) and the generation they realize, and the
//! entry-sequence high-water. Before every operation this module recovers a
//! fresh authority in three steps: the owner and the recorded high-water,
//! then the target from its durable projection under
//! [`SESSION_INSTRUCTION_STATE_KEY`], then the slots in effect from the
//! session's own entry rows ([`Session::effective_operator_instruction_set`]),
//! a transcript fact.
//! It then feeds the operation's request and transcript facts, lets the
//! authority decide (ownership, revision order, the slot limit, the active
//! budget, whether a run start appends an entry, what a compaction fold
//! renders), and writes the authority's resulting state back. Its own share
//! is the bodies, kept by digest while their slot is pending (an applied
//! body lives in its entry row), the rendering of entries from the
//! authority's state, and request validation (a body's size, a declared
//! digest).
//!
//! Nothing here wakes a session or applies inside a running turn: a write, a
//! record, a rebind or a release only changes the target, and the only
//! application points are a fresh run start (one coalesced delta entry,
//! appended before the run's own inputs) and a compaction fold at a run's
//! first request.
//!
//! A compaction folds every entry row into one snapshot
//! ([`Session::authorize_instruction_fold`]): of what is in effect, or of the
//! recorded target when it runs at a fresh run's first request. The fold
//! records the entry-sequence high-water in the target state, so a session
//! whose rewrite history holds entry rows always carries the state too.

use super::operator_instructions::{
    ActiveOperatorInstruction, EffectiveOperatorInstructions, InstructionEffect,
    InstructionRevision, InstructionSlot, MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
    MAX_OPERATOR_INSTRUCTION_SLOTS, OperatorInstructionChangeContent,
    OperatorInstructionEntryHeader, mint_operator_instruction_delta,
    mint_operator_instruction_snapshot,
};
use super::{MAX_INSTRUCTION_BODY_BYTES, SESSION_INSTRUCTION_STATE_KEY, Session};
use crate::generated::session_document as document;
use crate::lifecycle::RunId;
use crate::types::{
    InstructionContentDigest, InstructionKey, InstructionNamespace, Message, SystemMessage,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Who owns a session's recorded operator-instruction target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstructionTargetOwner {
    /// No owner holds a target: none was recorded, or the owning mob
    /// released it. The target is empty, so whatever is still in effect is
    /// revoked at the next fresh run start; any owner can take it, starting
    /// from the empty target. A rebind never names it.
    Unowned,
    /// The session's own host writes the target slot by slot.
    Session,
    /// A mob delivers the member's whole snapshot by generation. The target
    /// belongs to that mob until the mob releases it.
    Mob {
        mob_id: String,
        agent_identity: String,
    },
}

impl InstructionTargetOwner {
    /// The owner kind and the mob id the session-document authority records
    /// (empty unless the owner is a mob).
    fn machine_owner(&self) -> (document::InstructionTargetOwner, String) {
        match self {
            Self::Unowned => (document::InstructionTargetOwner::Unowned, String::new()),
            Self::Session => (document::InstructionTargetOwner::Session, String::new()),
            Self::Mob { mob_id, .. } => (document::InstructionTargetOwner::Mob, mob_id.clone()),
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

    fn order_key(&self) -> (&InstructionNamespace, &InstructionKey) {
        (&self.namespace, &self.key)
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

/// A session's recorded operator-instruction target, as the
/// session-document authority holds it, with the bodies still pending: its
/// owner, the target generation, the entry-sequence high-water and one
/// record per slot, sorted by slot. Stored under
/// [`SESSION_INSTRUCTION_STATE_KEY`]; a document that carries it is written
/// at envelope 5.
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

    fn active_body_bytes(&self) -> u64 {
        self.slots
            .iter()
            .filter(|record| record.effect == InstructionEffect::Active)
            .fold(0u64, |total, record| {
                total.saturating_add(record.body_bytes)
            })
    }

    /// The recorded shape: an unowned target empty at generation 0 and a mob
    /// owner naming its mob, sorted unique slots within the slot and byte
    /// budgets, a digest and a length exactly for Active slots, and a pending
    /// body that is the recorded content.
    fn validate(&self) -> Result<(), String> {
        match &self.owner {
            InstructionTargetOwner::Unowned
                if self.target_generation != 0 || !self.slots.is_empty() =>
            {
                return Err("an unowned target records no slot and no generation".to_string());
            }
            InstructionTargetOwner::Mob { mob_id, .. } if mob_id.is_empty() => {
                return Err("a mob-owned target names its mob".to_string());
            }
            _ => {}
        }
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
    OwnedByOtherMob,
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

/// Why an operator-instruction write, record, rebind or release was refused.
/// A refusal changes nothing.
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
    #[error("the operator instruction target is owned by another mob")]
    OwnedByOtherMob,
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
            Self::OwnedByOtherMob => InstructionSetErrorCode::OwnedByOtherMob,
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

/// What a release by a member's mob did to the member session's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionReleaseDisposition {
    /// The mob's target is now empty and unowned; the next fresh run start
    /// revokes what the mob had in effect.
    Released,
    /// No owner held the target: a repeated release writes nothing.
    AlreadyUnowned,
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
    /// The wire carries no System row an entry can take: none at all, or
    /// only the one leading row the session's prompt holds. Nothing is ever
    /// lowered as anything else.
    Unavailable,
}

impl InstructionLowering {
    /// The lowering of a model as its backend serves it
    /// (`ModelRegistry::profile_for_identity`): a model that lowers ordered
    /// System rows after conversation rows takes an entry anywhere; any other
    /// model, and one without a resolved profile, takes one only inside the
    /// leading System prefix. A backend whose wire carries only one leading
    /// System row takes none: resolve a session's lowering with
    /// `ModelRegistry::instruction_lowering_for_identity`.
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
    /// The model's wire carries no System row an entry can take.
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

/// A compaction fold of a session's operator-instruction entry rows, as the
/// session-document authority authorized it: the one snapshot entry the
/// rewrite puts right after its leading System prefix (none for an empty
/// set), and the recorded target the rewrite commits with it (the
/// authority's state after the fold: the fold's entry sequence as the
/// high-water, and no pending body a fold at a run's first request applied).
/// Only the validated compaction path inserts the snapshot.
#[derive(Debug, Clone)]
pub(crate) struct InstructionFold {
    snapshot: Option<SystemMessage>,
    state_after: SessionInstructionState,
}

impl InstructionFold {
    /// The snapshot entry row the fold inserts; none for an empty set.
    pub(crate) fn snapshot(&self) -> Option<&SystemMessage> {
        self.snapshot.as_ref()
    }
}

/// The instruction region's per-session registry key: every operation seeds
/// one fresh authority for its one session.
const SESSION_DOCUMENT_INSTRUCTION_KEY: &str = "operator_instructions";

/// The canonical key the session-document authority holds a slot under:
/// the JSON array `[namespace, key]` ([`InstructionSlot::machine_key`]).
fn slot_machine_key(slot: &InstructionSlot) -> String {
    slot.machine_key()
}

fn slot_from_machine_key(key: &str) -> Result<InstructionSlot, InstructionStateError> {
    InstructionSlot::from_machine_key(key)
}

/// One slot set as the session-document authority holds it: the revision,
/// the effect, the digest (empty while Absent) and the body bytes (zero
/// while Absent) per slot key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InstructionSlotMaps {
    revision: BTreeMap<String, u64>,
    effect: BTreeMap<String, document::InstructionEffect>,
    digest: BTreeMap<String, String>,
    bytes: BTreeMap<String, u64>,
}

/// One slot of an authority slot set, read back in slot order.
struct AuthoritySlot {
    slot: InstructionSlot,
    key: String,
    revision: InstructionRevision,
    effect: InstructionEffect,
    digest: Option<InstructionContentDigest>,
    bytes: u64,
}

impl InstructionSlotMaps {
    fn insert(
        &mut self,
        slot: &InstructionSlot,
        revision: InstructionRevision,
        effect: InstructionEffect,
        digest: Option<&InstructionContentDigest>,
        body_bytes: u64,
    ) {
        let key = slot_machine_key(slot);
        self.revision.insert(key.clone(), revision.get());
        self.effect.insert(key.clone(), effect.into());
        self.digest.insert(
            key.clone(),
            digest.map_or_else(String::new, |digest| digest.as_str().to_owned()),
        );
        self.bytes.insert(key, body_bytes);
    }

    /// The recorded target's slots, as its durable projection holds them.
    fn from_target(state: &SessionInstructionState) -> Self {
        let mut maps = Self::default();
        for record in &state.slots {
            maps.insert(
                &record.slot(),
                record.revision,
                record.effect,
                record.content_sha256.as_ref(),
                record.body_bytes,
            );
        }
        maps
    }

    /// The slots in effect, as the session's own entry rows leave them. A
    /// slot an entry revoked is not in effect, exactly as a slot no entry
    /// named.
    fn from_effective(effective: &EffectiveOperatorInstructions) -> Self {
        let mut maps = Self::default();
        for (slot, current) in effective.slots() {
            if let Some(body) = current.body() {
                maps.insert(
                    slot,
                    current.revision(),
                    InstructionEffect::Active,
                    current.content_sha256(),
                    body.len() as u64,
                );
            }
        }
        maps
    }

    /// The slot in effect at `key`, with its revision and digest; none when
    /// the set holds the slot Absent or not at all.
    fn active(&self, key: &str) -> Option<(u64, &str)> {
        if self.effect.get(key) != Some(&document::InstructionEffect::Active) {
            return None;
        }
        Some((
            self.revision.get(key).copied()?,
            self.digest.get(key).map(String::as_str)?,
        ))
    }

    /// Every slot, in slot order.
    fn slots(&self) -> Result<Vec<AuthoritySlot>, InstructionStateError> {
        let malformed = |detail: String| {
            InstructionStateError::MalformedState(format!(
                "the session-document authority holds a malformed slot: {detail}"
            ))
        };
        let mut slots = Vec::with_capacity(self.revision.len());
        for (key, revision) in &self.revision {
            let slot = slot_from_machine_key(key)?;
            let revision = InstructionRevision::new(*revision)
                .map_err(|error| malformed(error.to_string()))?;
            let effect = match self.effect.get(key) {
                Some(document::InstructionEffect::Active) => InstructionEffect::Active,
                Some(document::InstructionEffect::Absent) => InstructionEffect::Absent,
                None => return Err(malformed(format!("{slot} has no effect"))),
            };
            let digest = match self.digest.get(key).map(String::as_str) {
                None => return Err(malformed(format!("{slot} has no digest"))),
                Some("") => None,
                Some(digest) => Some(
                    InstructionContentDigest::new(digest)
                        .map_err(|error| malformed(error.to_string()))?,
                ),
            };
            let Some(bytes) = self.bytes.get(key).copied() else {
                return Err(malformed(format!("{slot} has no body bytes")));
            };
            slots.push(AuthoritySlot {
                slot,
                key: key.clone(),
                revision,
                effect,
                digest,
                bytes,
            });
        }
        slots.sort_by(|left, right| left.slot.cmp(&right.slot));
        Ok(slots)
    }

    /// The Active slots with their bodies, in slot order.
    fn active_instructions(
        &self,
        bodies: &InstructionBodies,
    ) -> Result<Vec<ActiveOperatorInstruction>, InstructionStateError> {
        let mut active = Vec::new();
        for slot in self.slots()? {
            if slot.effect != InstructionEffect::Active {
                continue;
            }
            let body = bodies.body_of(&slot.slot, slot.digest.as_ref())?;
            active.push(ActiveOperatorInstruction {
                slot: slot.slot,
                revision: slot.revision,
                body: body.to_string(),
            });
        }
        Ok(active)
    }
}

/// The bodies the shell keeps, by digest: the recorded target's pending
/// bodies, the bodies in effect (read from the entry rows) and a request's.
#[derive(Debug, Default)]
struct InstructionBodies(BTreeMap<InstructionContentDigest, String>);

impl InstructionBodies {
    fn from_sources(
        state: Option<&SessionInstructionState>,
        effective: &EffectiveOperatorInstructions,
    ) -> Self {
        let mut bodies = Self::default();
        for record in state
            .map(SessionInstructionState::slots)
            .unwrap_or_default()
        {
            if let (Some(digest), Some(body)) = (&record.content_sha256, &record.pending_body) {
                bodies.0.insert(digest.clone(), body.clone());
            }
        }
        for (_, current) in effective.slots() {
            if let (Some(digest), Some(body)) = (current.content_sha256(), current.body()) {
                bodies.0.insert(digest.clone(), body.to_string());
            }
        }
        bodies
    }

    /// Keep a request's body; answers its digest.
    fn add(&mut self, body: String) -> InstructionContentDigest {
        let digest = InstructionContentDigest::for_body(&body);
        self.0.insert(digest.clone(), body);
        digest
    }

    fn body_of(
        &self,
        slot: &InstructionSlot,
        digest: Option<&InstructionContentDigest>,
    ) -> Result<&str, InstructionStateError> {
        digest
            .and_then(|digest| self.0.get(digest))
            .map(String::as_str)
            .ok_or_else(|| {
                InstructionStateError::MalformedState(format!(
                    "the body of {slot} is neither pending nor in effect"
                ))
            })
    }
}

/// A `SessionDocumentMachine` authority recovered with one session's
/// recorded target and the slots in effect.
struct InstructionDocument {
    authority: document::SessionDocumentMachineAuthority,
    key: document::SessionDocumentKey,
}

impl InstructionDocument {
    /// Recover the authority in its three steps: the owner, the target
    /// generation and the recorded entry-sequence high-water, then the
    /// recorded target, then the slots in effect with the generation and the
    /// sequence of the session's last entry row (transcript facts; the
    /// authority takes the later high-water).
    fn seed(
        state: Option<&SessionInstructionState>,
        effective: &EffectiveOperatorInstructions,
    ) -> Result<Self, InstructionStateError> {
        let refused = |error: document::SessionDocumentError| {
            InstructionStateError::MalformedState(format!(
                "the session-document authority refused the recorded projection: {error}"
            ))
        };
        let mut authority = document::SessionDocumentMachineAuthority::new();
        let key = document::SessionDocumentKey::new(SESSION_DOCUMENT_INSTRUCTION_KEY);
        let target = state
            .map(InstructionSlotMaps::from_target)
            .unwrap_or_default();
        let in_effect = InstructionSlotMaps::from_effective(effective);
        let (owner, owner_mob) = state.map_or_else(
            || (document::InstructionTargetOwner::Unowned, String::new()),
            |state| state.owner.machine_owner(),
        );
        let target_generation = state.map_or(0, SessionInstructionState::target_generation);
        let recorded_sequence = state.map_or(0, SessionInstructionState::entry_sequence);
        authority
            .recover_session_instruction_state(
                key.clone(),
                owner,
                owner_mob,
                target_generation,
                recorded_sequence,
            )
            .map_err(refused)?;
        authority
            .recover_session_instruction_target(
                key.clone(),
                target.revision,
                target.effect,
                target.digest,
                target.bytes,
            )
            .map_err(refused)?;
        authority
            .recover_session_instruction_effective(
                key.clone(),
                in_effect.revision,
                in_effect.effect,
                in_effect.digest,
                in_effect.bytes,
                effective.last_target_generation(),
                effective.last_entry_sequence(),
            )
            .map_err(refused)?;
        Ok(Self { authority, key })
    }

    fn key(&self) -> document::SessionDocumentKey {
        self.key.clone()
    }

    /// The recorded target the authority holds now.
    fn target(&self) -> InstructionSlotMaps {
        InstructionSlotMaps {
            revision: self
                .authority
                .session_instruction_target_revision_for(&self.key)
                .unwrap_or_default(),
            effect: self
                .authority
                .session_instruction_target_effect_for(&self.key)
                .unwrap_or_default(),
            digest: self
                .authority
                .session_instruction_target_digest_for(&self.key)
                .unwrap_or_default(),
            bytes: self
                .authority
                .session_instruction_target_bytes_for(&self.key)
                .unwrap_or_default(),
        }
    }

    /// The slots in effect the authority holds now.
    fn effective(&self) -> InstructionSlotMaps {
        InstructionSlotMaps {
            revision: self
                .authority
                .session_instruction_effective_revision_for(&self.key)
                .unwrap_or_default(),
            effect: self
                .authority
                .session_instruction_effective_effect_for(&self.key)
                .unwrap_or_default(),
            digest: self
                .authority
                .session_instruction_effective_digest_for(&self.key)
                .unwrap_or_default(),
            bytes: self
                .authority
                .session_instruction_effective_bytes_for(&self.key)
                .unwrap_or_default(),
        }
    }

    fn owner_kind(&self) -> document::InstructionTargetOwner {
        self.authority
            .session_instruction_owner_for(&self.key)
            .unwrap_or_default()
    }

    /// The recorded target as the authority holds it now, ready to store:
    /// `owner` is the owner the operation knows (an owner's descriptive
    /// facts, such as a mob member's identity, are the shell's), and each
    /// Active slot that is not in effect keeps its body as pending.
    fn recorded_state(
        &self,
        owner: InstructionTargetOwner,
        bodies: &InstructionBodies,
    ) -> Result<SessionInstructionState, InstructionStateError> {
        let (owner_kind, owner_mob) = owner.machine_owner();
        let held_mob = self
            .authority
            .session_instruction_owner_mob_for(&self.key)
            .unwrap_or_default();
        if owner_kind != self.owner_kind() || owner_mob != held_mob {
            return Err(InstructionStateError::MalformedState(
                "the stored owner is not the owner the session-document authority holds"
                    .to_string(),
            ));
        }
        let in_effect = self.effective();
        let mut slots = Vec::new();
        for slot in self.target().slots()? {
            let record = match slot.effect {
                InstructionEffect::Active => {
                    let body = bodies.body_of(&slot.slot, slot.digest.as_ref())?;
                    let applied = in_effect
                        .active(&slot.key)
                        .is_some_and(|(revision, digest)| {
                            revision == slot.revision.get()
                                && slot
                                    .digest
                                    .as_ref()
                                    .is_some_and(|recorded| recorded.as_str() == digest)
                        });
                    InstructionTargetSlot {
                        namespace: slot.slot.namespace.clone(),
                        key: slot.slot.key.clone(),
                        revision: slot.revision,
                        effect: InstructionEffect::Active,
                        content_sha256: slot.digest.clone(),
                        body_bytes: slot.bytes,
                        pending_body: (!applied).then(|| body.to_string()),
                    }
                }
                InstructionEffect::Absent => {
                    InstructionTargetSlot::revoked(&slot.slot, slot.revision)
                }
            };
            slots.push(record);
        }
        let state = SessionInstructionState {
            owner,
            target_generation: self
                .authority
                .session_instruction_target_generation_for(&self.key)
                .unwrap_or_default(),
            entry_sequence: self
                .authority
                .session_instruction_entry_sequence_for(&self.key)
                .unwrap_or_default(),
            slots,
        };
        state
            .validate()
            .map_err(InstructionStateError::MalformedState)?;
        Ok(state)
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

/// Everything a run start applies: the changes from the effective set to the
/// recorded target, and the whole active set after them.
struct PendingInstructionChanges {
    changes: Vec<OperatorInstructionChangeContent>,
    active_after: Vec<ActiveOperatorInstruction>,
}

/// Render the changes a run start's entry carries from the authority's
/// target and effective set, as they stood when it authorized the entry: an
/// Active target slot not in effect at its revision and digest activates,
/// an Absent target slot still in effect is revoked, and a slot in effect the
/// target no longer names is revoked at the revision in effect.
fn render_pending_changes(
    target: &InstructionSlotMaps,
    in_effect: &InstructionSlotMaps,
    bodies: &InstructionBodies,
) -> Result<PendingInstructionChanges, InstructionStateError> {
    let revision = |value: u64| {
        InstructionRevision::new(value).map_err(|error| {
            InstructionStateError::MalformedState(format!(
                "the session-document authority holds an invalid revision: {error}"
            ))
        })
    };
    let mut changes = Vec::new();
    let mut active_after = Vec::new();
    for slot in target.slots()? {
        let current = in_effect.active(&slot.key);
        match slot.effect {
            InstructionEffect::Active => {
                let body = bodies.body_of(&slot.slot, slot.digest.as_ref())?;
                let unchanged = current.is_some_and(|(current_revision, current_digest)| {
                    current_revision == slot.revision.get()
                        && slot
                            .digest
                            .as_ref()
                            .is_some_and(|digest| digest.as_str() == current_digest)
                });
                if !unchanged {
                    changes.push(OperatorInstructionChangeContent::Activate {
                        slot: slot.slot.clone(),
                        revision: slot.revision,
                        supersedes: current
                            .map(|(current_revision, _)| revision(current_revision))
                            .transpose()?,
                        body: body.to_string(),
                    });
                }
                active_after.push(ActiveOperatorInstruction {
                    slot: slot.slot,
                    revision: slot.revision,
                    body: body.to_string(),
                });
            }
            InstructionEffect::Absent => {
                if let Some((current_revision, _)) = current {
                    changes.push(OperatorInstructionChangeContent::Revoke {
                        slot: slot.slot,
                        revision: slot.revision,
                        revokes: revision(current_revision)?,
                    });
                }
            }
        }
    }
    for slot in in_effect.slots()? {
        if slot.effect == InstructionEffect::Active && !target.revision.contains_key(&slot.key) {
            changes.push(OperatorInstructionChangeContent::Revoke {
                slot: slot.slot,
                revision: slot.revision,
                revokes: slot.revision,
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

    /// This session's effective set, from its own entry rows; each row's
    /// bytes are verified once per transcript buffer.
    fn own_effective_instructions(
        &self,
    ) -> Result<EffectiveOperatorInstructions, InstructionStateError> {
        self.effective_operator_instruction_set()
            .map_err(InstructionStateError::MalformedHistory)
    }

    /// Whether an instruction activation row names `slot`: such a slot is
    /// managed by activations in this session and takes no operator
    /// instruction. This check only ever refuses; it admits nothing.
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

    /// The session's binding fact for the session-document authority:
    /// whether its metadata names a mob member binding, which a mob stamps
    /// when it seats the session as a member's. Metadata that does not read
    /// fails the write, typed, instead of reading as a session no mob seated.
    fn instruction_member_bound(&self) -> Result<bool, InstructionWriteError> {
        self.try_session_metadata()
            .map(|metadata| metadata.is_some_and(|metadata| metadata.mob_member_binding.is_some()))
            .map_err(|error| {
                InstructionWriteError::Malformed(InstructionStateError::MalformedState(format!(
                    "the session's metadata does not read: {error}"
                )))
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
    /// authority decides it against the slot it holds; a recorded write
    /// changes only the target (it applies at the next fresh run start), and
    /// a refusal or a duplicate changes nothing. A mob member's session (its
    /// metadata names a mob member binding) takes no write of its own host:
    /// the authority refuses it `owned_by_mob` from that binding fact, whether
    /// or not the mob has recorded a target yet, because the member's
    /// instructions come from its mob.
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
        let member_bound = self.instruction_member_bound()?;
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let mut bodies = InstructionBodies::from_sources(state.as_ref(), &effective);
        let body_bytes = body.as_ref().map_or(0, |body| body.len() as u64);
        let digest = body.map(|body| bodies.add(body));
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let slot_key = slot_machine_key(&write.slot);
        let current_revision = document
            .target()
            .revision
            .get(&slot_key)
            .copied()
            .and_then(|current| InstructionRevision::new(current).ok());
        let effects = document
            .authority
            .set_session_instruction(
                key,
                slot_key,
                write.revision.get(),
                effect.into(),
                digest
                    .as_ref()
                    .map_or_else(String::new, |digest| digest.as_str().to_owned()),
                body_bytes,
                member_bound,
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
                let next = document.recorded_state(InstructionTargetOwner::Session, &bodies)?;
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
                document::InstructionWriteRefusal::OwnedByOtherMob => {
                    InstructionWriteError::OwnedByOtherMob
                }
                document::InstructionWriteRefusal::ActiveSetBudgetExceeded => {
                    InstructionWriteError::ActiveSetBudgetExceeded {
                        max_bytes: MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
                    }
                }
            }),
        }
    }

    /// Record a mob member's whole snapshot in the target. The
    /// session-document authority accepts a snapshot by generation alone, and
    /// only from the mob that owns the target (or into an unowned one): a
    /// generation at or below the recorded one answers "already recorded" and
    /// writes nothing, so a repeated delivery never writes twice, and another
    /// mob's snapshot is refused until the owning mob releases the target. A
    /// member seated over an existing session whose own host wrote its target
    /// before the mob seated it (an attach) hands that target to the mob at
    /// the mob's first snapshot: from the session's binding fact (its
    /// metadata names a mob member binding), the authority makes the mob the
    /// owner and records the snapshot in one step, so the mob starts from its
    /// snapshot and what the host had in effect is revoked at the next fresh
    /// run start unless the snapshot keeps it. A session-owned target of a
    /// session no mob seated is refused (`owned_by_session`). The authority
    /// also decides the slot limit and the active budget. A snapshot never
    /// names a slot that instruction activations manage in this session,
    /// whether it records the slot Active or Absent. Recording changes only
    /// the target and wakes nothing.
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
        if record.mob_id.is_empty() {
            return Err(InstructionWriteError::InvalidRequest(
                "a recorded operator instruction target names its mob".to_string(),
            ));
        }
        // A fast path: the session-document authority refuses a snapshot
        // over the slot limit too.
        if record.slots.len() > MAX_OPERATOR_INSTRUCTION_SLOTS {
            return Err(InstructionWriteError::SlotLimitExceeded);
        }
        let member_bound = self.instruction_member_bound()?;
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let mut bodies = InstructionBodies::from_sources(state.as_ref(), &effective);
        let mut snapshot = InstructionSlotMaps::default();
        let mut named = BTreeSet::new();
        for entry in record.slots {
            if !named.insert(entry.slot.clone()) {
                return Err(InstructionWriteError::InvalidRequest(
                    "an operator instruction snapshot names a slot twice".to_string(),
                ));
            }
            if self.slot_has_activation_rows(&entry.slot) {
                return Err(InstructionWriteError::SlotManagedByActivation { slot: entry.slot });
            }
            match entry.body {
                Some(body) => {
                    if body.len() > MAX_INSTRUCTION_BODY_BYTES {
                        return Err(InstructionWriteError::BodyTooLarge {
                            max_bytes: MAX_INSTRUCTION_BODY_BYTES,
                        });
                    }
                    let body_bytes = body.len() as u64;
                    let digest = bodies.add(body);
                    snapshot.insert(
                        &entry.slot,
                        entry.revision,
                        InstructionEffect::Active,
                        Some(&digest),
                        body_bytes,
                    );
                }
                None => {
                    snapshot.insert(
                        &entry.slot,
                        entry.revision,
                        InstructionEffect::Absent,
                        None,
                        0,
                    );
                }
            }
        }
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let effects = document
            .authority
            .record_instruction_target(
                key,
                record.mob_id.clone(),
                record.generation,
                snapshot.revision,
                snapshot.effect,
                snapshot.digest,
                snapshot.bytes,
                member_bound,
            )
            .map_err(|error| refused_by_authority(&error))?;
        match write_verdict(&effects)? {
            WriteVerdict::Recorded(generation) => {
                let next = document.recorded_state(
                    InstructionTargetOwner::Mob {
                        mob_id: record.mob_id,
                        agent_identity: record.agent_identity,
                    },
                    &bodies,
                )?;
                self.store_instruction_target_state(&next)?;
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
            WriteVerdict::Refused(document::InstructionWriteRefusal::OwnedByOtherMob) => {
                Err(InstructionWriteError::OwnedByOtherMob)
            }
            WriteVerdict::Refused(document::InstructionWriteRefusal::ActiveSetBudgetExceeded) => {
                Err(InstructionWriteError::ActiveSetBudgetExceeded {
                    max_bytes: MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
                })
            }
            WriteVerdict::Refused(document::InstructionWriteRefusal::SlotLimitExceeded) => {
                Err(InstructionWriteError::SlotLimitExceeded)
            }
            WriteVerdict::Refused(other) => Err(InstructionWriteError::Malformed(
                InstructionStateError::MalformedState(format!(
                    "the session-document authority refused a record with {other:?}"
                )),
            )),
        }
    }

    /// Rebind the target to a new owner, on an explicit attach or a host's
    /// standalone adoption; the generation restarts. A mob that attaches to
    /// a session-owned target starts from what is in effect: the target
    /// becomes the slots in effect, and no pending write of the session
    /// carries over. A new owner of an unowned target (a host adopting a
    /// session no mob built or seated, a mob attaching to it) starts from
    /// that empty target, so what is still in effect is revoked at the next
    /// fresh run start. The authority refuses a rebind to the owner the
    /// target already has, to no owner, and of a mob-owned target, which
    /// only its mob's release frees; a mob member's session refuses its own
    /// host's adoption (`owned_by_mob`), as it refuses its host's writes,
    /// and still does after its mob released the target: the member
    /// binding a mob stamps is never cleared.
    pub fn rebind_instruction_owner(
        &mut self,
        owner: InstructionTargetOwner,
    ) -> Result<(), InstructionWriteError> {
        let member_bound = self.instruction_member_bound()?;
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let bodies = InstructionBodies::from_sources(state.as_ref(), &effective);
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let (owner_kind, mob_id) = owner.machine_owner();
        let effects = document
            .authority
            .rebind_instruction_owner(key, owner_kind, mob_id, member_bound)
            .map_err(|error| {
                InstructionWriteError::InvalidRequest(format!(
                    "the operator instruction target cannot be rebound to this owner: {error}"
                ))
            })?;
        if effects.iter().any(|effect| {
            matches!(
                effect,
                document::SessionDocumentEffect::InstructionTargetRefused {
                    refusal: document::InstructionWriteRefusal::OwnedByMob,
                }
            )
        }) {
            return Err(InstructionWriteError::OwnedByMob);
        }
        let next = document.recorded_state(owner, &bodies)?;
        self.store_instruction_target_state(&next)?;
        Ok(())
    }

    /// Release a member session's target on behalf of the mob that owns it
    /// (its member retires, the mob is destroyed): the target empties and
    /// is unowned, so another owner may bind it, starting from the empty
    /// target. The release applies nothing by itself: the session's next
    /// fresh run start revokes what the mob had in effect. Another owner's
    /// target is refused; a target no owner holds answers "already unowned"
    /// and writes nothing.
    pub fn release_instruction_target(
        &mut self,
        mob_id: &str,
    ) -> Result<InstructionReleaseDisposition, InstructionWriteError> {
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let bodies = InstructionBodies::from_sources(state.as_ref(), &effective);
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let effects = document
            .authority
            .release_instruction_target(key, mob_id.to_owned())
            .map_err(|error| refused_by_authority(&error))?;
        let refusal = effects.iter().find_map(|effect| match effect {
            document::SessionDocumentEffect::InstructionTargetRefused { refusal } => Some(*refusal),
            _ => None,
        });
        match refusal {
            Some(document::InstructionWriteRefusal::OwnedByOtherMob) => {
                return Err(InstructionWriteError::OwnedByOtherMob);
            }
            Some(document::InstructionWriteRefusal::OwnedBySession) => {
                return Err(InstructionWriteError::OwnedBySession);
            }
            Some(other) => {
                return Err(InstructionWriteError::Malformed(
                    InstructionStateError::MalformedState(format!(
                        "the session-document authority refused a release with {other:?}"
                    )),
                ));
            }
            None => {}
        }
        // The authority's verdict says whether the releasing mob owned the
        // target; a target no owner held writes nothing.
        let Some(was_owned) = effects.iter().find_map(|effect| match effect {
            document::SessionDocumentEffect::InstructionTargetReleased { was_owned } => {
                Some(*was_owned)
            }
            _ => None,
        }) else {
            return Err(InstructionWriteError::Malformed(
                InstructionStateError::MalformedState(
                    "the session-document authority emitted no release verdict".to_string(),
                ),
            ));
        };
        if !was_owned {
            return Ok(InstructionReleaseDisposition::AlreadyUnowned);
        }
        let next = document.recorded_state(InstructionTargetOwner::Unowned, &bodies)?;
        self.store_instruction_target_state(&next)?;
        Ok(InstructionReleaseDisposition::Released)
    }

    /// At a turn's run start, after its start disposition is resolved and
    /// before any input of the run is appended: let the session-document
    /// authority decide, from the target and effective set it holds, whether
    /// the pending changes apply now and, if they do, append the one delta
    /// entry that carries all of them at the transcript tail. The entry and
    /// the authority's resulting target persist with the run's commits; a
    /// run that commits nothing leaves the changes pending for the next run
    /// start.
    pub fn apply_run_start_instructions(
        &mut self,
        run_start: InstructionRunStart,
        lowering: InstructionLowering,
        run_id: Option<RunId>,
    ) -> Result<RunStartInstructionOutcome, InstructionStateError> {
        let state = self.instruction_target_state()?;
        // A session that records no target state holds no entry row of its
        // own (every entry it mints records the state), so nothing of its own
        // is in effect: the authority recovers it unowned and decides.
        let effective = if state.is_some() {
            self.own_effective_instructions()?
        } else {
            EffectiveOperatorInstructions::default()
        };
        let bodies = InstructionBodies::from_sources(state.as_ref(), &effective);
        let transcript_is_system_prefix = self
            .messages()
            .iter()
            .all(|message| matches!(message, Message::System(_)));
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let target = document.target();
        let in_effect = document.effective();
        let effects = document
            .authority
            .resolve_run_start_instructions(
                key,
                run_start.into(),
                lowering.into(),
                transcript_is_system_prefix,
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
                let pending = render_pending_changes(&target, &in_effect, &bodies)?;
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
                // The state the entry leaves is ready before anything is
                // appended, so the entry and the state change together.
                let owner = state
                    .as_ref()
                    .map_or(InstructionTargetOwner::Unowned, |state| state.owner.clone());
                let next = document.recorded_state(owner, &bodies)?;
                self.append_operator_instruction_entry(entry)
                    .map_err(InstructionStateError::MalformedHistory)?;
                self.store_instruction_target_state(&next)?;
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

    /// Before a compaction rewrite: let the session-document authority
    /// authorize the fold of this session's operator-instruction entry rows
    /// and mint the snapshot it renders. `at_run_start` is whether the
    /// compaction runs before the first model response of a fresh run,
    /// `lowering` how the session's model carries System rows, and `run_id`
    /// the run in flight. The authority folds the recorded target at a run's
    /// first request (unless the model carries no System row), and what is
    /// in effect otherwise. None when the session carries no
    /// operator-instruction fact: its compaction is unchanged.
    ///
    /// Nothing changes here: the caller inserts the snapshot in the rebuilt
    /// transcript and records the fold with the rewrite
    /// ([`Self::record_instruction_fold`]).
    pub(crate) fn authorize_instruction_fold(
        &self,
        at_run_start: bool,
        lowering: InstructionLowering,
        run_id: Option<RunId>,
    ) -> Result<Option<InstructionFold>, InstructionStateError> {
        if !self.carries_operator_instruction_facts() {
            return Ok(None);
        }
        let state = self.instruction_target_state()?;
        let effective = self.own_effective_instructions()?;
        let bodies = InstructionBodies::from_sources(state.as_ref(), &effective);
        let mut document = InstructionDocument::seed(state.as_ref(), &effective)?;
        let key = document.key();
        let target = document.target();
        let in_effect = document.effective();
        let effects = document
            .authority
            .authorize_instruction_fold(key, at_run_start, lowering.into())
            .map_err(|error| refused_by_authority(&error))?;
        let (entry_sequence, source, target_generation) = effects
            .iter()
            .find_map(|effect| match effect {
                document::SessionDocumentEffect::InstructionFoldAuthorized {
                    entry_sequence,
                    source,
                    generation,
                } => Some((*entry_sequence, *source, *generation)),
                _ => None,
            })
            .ok_or_else(|| {
                InstructionStateError::MalformedState(
                    "the session-document authority emitted no fold verdict".to_string(),
                )
            })?;
        // A fold of the target applies it at this run's start and names the
        // run; a fold of what is in effect applies nothing and names none.
        let (active, run_id) = match source {
            document::InstructionFoldSource::Target => {
                (target.active_instructions(&bodies)?, run_id)
            }
            document::InstructionFoldSource::Effective => {
                (in_effect.active_instructions(&bodies)?, None)
            }
        };
        let snapshot = mint_operator_instruction_snapshot(
            OperatorInstructionEntryHeader {
                origin_session_id: self.id().clone(),
                entry_sequence,
                target_generation,
                run_id,
            },
            active,
            crate::types::message_timestamp_now(),
        )
        .map_err(InstructionStateError::MalformedHistory)?;
        let owner = state
            .as_ref()
            .map_or(InstructionTargetOwner::Unowned, |state| state.owner.clone());
        let state_after = document.recorded_state(owner, &bodies)?;
        Ok(Some(InstructionFold {
            snapshot,
            state_after,
        }))
    }

    /// Record an unowned operator-instruction state (no owner, generation 0,
    /// entry sequence 0, no slot) unless the session already records one.
    /// The state key alone answers whether a session's document carries a
    /// fact only envelope 5 reads ([`Session::carries_operator_instruction_facts`]),
    /// so every such fact records it: a session that receives entry rows by
    /// copy (a fork child: the copied rows name another origin, and this
    /// session's own entry sequence starts at zero), a session whose
    /// transcript holds an instruction activation row with a retired body
    /// (a compaction retired it, or a fork copied it), and a session in
    /// which a rewrite moved an instruction activation row to another
    /// offset.
    pub(crate) fn record_unowned_instruction_state(&mut self) -> Result<(), InstructionStateError> {
        if self.instruction_target_state()?.is_some() {
            return Ok(());
        }
        self.store_instruction_target_state(&SessionInstructionState {
            owner: InstructionTargetOwner::Unowned,
            target_generation: 0,
            entry_sequence: 0,
            slots: Vec::new(),
        })
    }

    /// Record an authorized fold's target state in the session whose
    /// compaction rewrite inserted its snapshot, in the same prepared
    /// session value, so the two commit, roll back and persist together.
    pub(crate) fn record_instruction_fold(
        &mut self,
        fold: &InstructionFold,
    ) -> Result<(), InstructionStateError> {
        self.store_instruction_target_state(&fold.state_after)
    }
}

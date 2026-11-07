//! Operator-instruction RPC wire contracts: the mob verbs
//! (`mob/set_instruction`, `mob/revoke_instruction`, `mob/instruction_status`)
//! and the session verbs (`session/set_instruction`,
//! `session/revoke_instruction`, `session/instruction_status`).
//!
//! Requests, receipts, dispositions and the state kinds of a status read are
//! closed. The advisory hints a status read carries (why a change is still
//! pending, why a delivery is blocked, where a member's session lives, how a
//! model lowers entries) are tolerant: a value this version does not know
//! reads as `Unknown` with the value as it was sent, and is written back the
//! same, so a newer server never fails an older reader's page.

use meerkat_core::lifecycle::RunId;
use meerkat_core::service::SessionInstructionRead;
use meerkat_core::{
    InstructionContentDigest, InstructionEffect, InstructionEntryKind, InstructionKey,
    InstructionNamespace, InstructionRevision, InstructionSetErrorCode, InstructionSlot,
    InstructionTargetOwner,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The `data` of a JSON-RPC error that refuses an operator-instruction write
/// or read: its code in the closed refusal vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InstructionSetErrorData {
    pub instruction_set_code: InstructionSetErrorCode,
}

/// A tolerant advisory vocabulary: the values this version knows, plus
/// `Unknown` holding a value it does not, as it was sent. Serialized as the
/// plain string; its schema lists the known values.
macro_rules! tolerant_vocabulary {
    (
        $(#[$meta:meta])*
        $name:ident in $schema_mod:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(from = "String", into = "String")]
        #[non_exhaustive]
        pub enum $name {
            $( $(#[$variant_meta])* $variant, )+
            /// A value this version does not know, as it was sent.
            Unknown(String),
        }

        impl $name {
            /// The value on the wire.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $wire, )+
                    Self::Unknown(raw) => raw,
                }
            }
        }

        impl From<String> for $name {
            fn from(raw: String) -> Self {
                match raw.as_str() {
                    $( $wire => Self::$variant, )+
                    _ => Self::Unknown(raw),
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                match value {
                    $name::Unknown(raw) => raw,
                    known => known.as_str().to_owned(),
                }
            }
        }

        // The schema lists the values this version knows; a reader accepts
        // any other string as unknown.
        #[cfg(feature = "schema")]
        mod $schema_mod {
            #[derive(schemars::JsonSchema)]
            #[schemars(rename_all = "snake_case")]
            #[allow(dead_code)]
            pub(super) enum $name {
                $( $variant, )+
            }
        }

        #[cfg(feature = "schema")]
        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                <$schema_mod::$name as schemars::JsonSchema>::json_schema(generator)
            }
        }
    };
}

tolerant_vocabulary! {
    /// Why a slot's desired state is not yet in effect. Advisory: the
    /// vocabulary may grow.
    WireInstructionPendingReason in pending_reason_schema {
        /// Recorded; the session's next fresh run start applies it.
        AwaitingRunStart => "awaiting_run_start",
        /// Recorded; the model takes System rows only inside the leading
        /// System prefix, so a run start whose transcript is still that prefix,
        /// or a compaction fold at a run start, applies it.
        LeadingPrefixOnlyLowering => "leading_prefix_only_lowering",
        /// Recorded; the model's wire carries no System row an entry can take.
        LoweringUnavailable => "lowering_unavailable",
        /// The member's seat has no session yet: its first wake creates the
        /// session with the snapshot.
        SessionUnborn => "session_unborn",
        /// The mob has not recorded the snapshot in the member's session yet.
        DeliveryOwed => "delivery_owed",
    }
}

tolerant_vocabulary! {
    /// Why a member's delivery is blocked until a write selects it again or it
    /// is seated again. Advisory: the vocabulary may grow.
    WireInstructionBlockReason in block_reason_schema {
        /// The member's host takes no operator-instruction delivery.
        MemberHostUnsupported => "member_host_unsupported",
        /// The member session's own instruction activations manage a slot of
        /// the snapshot.
        SlotManagedByActivation => "slot_managed_by_activation",
        /// The member's session service keeps no durable session document.
        DurabilityUnavailable => "durability_unavailable",
        /// The member session's own host owns its target.
        OwnedBySession => "owned_by_session",
        /// Another mob owns the member session's target until it releases it.
        OwnedByOtherMob => "owned_by_other_mob",
    }
}

tolerant_vocabulary! {
    /// Where a member's session lives now. Advisory: the vocabulary may grow.
    WireInstructionResidency in residency_schema {
        Live => "live",
        Dormant => "dormant",
        /// The member's seat has no session yet.
        Unborn => "unborn",
    }
}

tolerant_vocabulary! {
    /// How a session's model carries an operator-instruction entry after
    /// conversation rows. Advisory: the vocabulary may grow.
    WireInstructionLowering in lowering_schema {
        /// System rows lower at any transcript position.
        MidConversation => "mid_conversation",
        /// System rows lower only inside the leading System prefix.
        LeadingPrefixOnly => "leading_prefix_only",
        /// The wire carries no System row an entry can take.
        Unavailable => "unavailable",
    }
}

impl From<meerkat_core::InstructionLowering> for WireInstructionLowering {
    fn from(lowering: meerkat_core::InstructionLowering) -> Self {
        match lowering {
            meerkat_core::InstructionLowering::MidConversation => Self::MidConversation,
            meerkat_core::InstructionLowering::LeadingPrefixOnly => Self::LeadingPrefixOnly,
            meerkat_core::InstructionLowering::Unavailable => Self::Unavailable,
        }
    }
}

impl WireInstructionPendingReason {
    /// Why a recorded change waits, from how the session's model lowers
    /// entries: the next fresh run start, unless the model takes System rows
    /// only in the leading prefix or cannot carry them at all.
    #[must_use]
    pub fn for_lowering(lowering: Option<meerkat_core::InstructionLowering>) -> Self {
        match lowering {
            Some(meerkat_core::InstructionLowering::LeadingPrefixOnly) => {
                Self::LeadingPrefixOnlyLowering
            }
            Some(meerkat_core::InstructionLowering::Unavailable) => Self::LoweringUnavailable,
            Some(meerkat_core::InstructionLowering::MidConversation) | None => {
                Self::AwaitingRunStart
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Targets and the mob verbs
// ---------------------------------------------------------------------------

/// Which members of a mob an operator instruction targets: every member, the
/// members of one profile, or one member. A target is a predicate over the
/// roster: a profile or member that does not exist yet selects nobody until
/// it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireInstructionTarget {
    /// Every member of the mob.
    Mob,
    /// The members of one profile.
    Profile { profile: String },
    /// One member.
    Member { agent_identity: String },
}

/// Request payload for `mob/set_instruction`: one slot's body at a revision,
/// for the members the target selects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSetInstructionParams {
    pub mob_id: String,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    /// Strictly increasing per slot within the mob.
    pub revision: InstructionRevision,
    /// The exact UTF-8 body, at most 256 KiB.
    pub body: String,
    /// The body's digest as the caller computed it; one that is not the
    /// body's is refused (`digest_mismatch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<InstructionContentDigest>,
    pub target: WireInstructionTarget,
}

impl MobSetInstructionParams {
    /// The slot the write names.
    #[must_use]
    pub fn slot(&self) -> InstructionSlot {
        InstructionSlot {
            namespace: self.namespace.clone(),
            key: self.key.clone(),
        }
    }
}

/// Request payload for `mob/revoke_instruction`: revoke one slot at a higher
/// revision. The record keeps its target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobRevokeInstructionParams {
    pub mob_id: String,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
}

impl MobRevokeInstructionParams {
    /// The slot the revoke names.
    #[must_use]
    pub fn slot(&self) -> InstructionSlot {
        InstructionSlot {
            namespace: self.namespace.clone(),
            key: self.key.clone(),
        }
    }
}

/// What a mob operator-instruction write or revoke did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMobInstructionDisposition {
    // The record is committed and journaled; the members it selects (and
    // those its previous target selected) are owed the new generation.
    Committed,
    // An exact repeat of the slot's current revision: nothing changed.
    Duplicate,
}

/// Response payload for `mob/set_instruction` and `mob/revoke_instruction`,
/// returned once the committed record is durable in the mob's journal.
/// Delivery to the members is asynchronous: `mob/instruction_status` reports
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobInstructionWriteResult {
    pub mob_id: String,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
    pub disposition: WireMobInstructionDisposition,
    /// The mob's operator-instruction generation after the write.
    pub generation: u64,
    /// How many current members the record's target selects; a target that
    /// names a missing profile or member shows zero.
    pub matched_members: u64,
}

/// Request payload for `mob/instruction_status`: the mob's records
/// (optionally one namespace or slot) and one page of members (optionally
/// one member or one profile).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobInstructionStatusParams {
    pub mob_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<InstructionNamespace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<InstructionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Members to skip, in identity order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// 1 to 200 members; absent reads 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// One desired record of a mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInstructionRecord {
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
    pub target: WireInstructionTarget,
    /// The body's digest while the record is Active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<InstructionContentDigest>,
    /// The body's UTF-8 length while the record is Active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<u64>,
}

/// A slot's revision and effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInstructionSlotView {
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
}

/// The committed entry that put a slot's state in effect in a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInstructionEffectiveSlot {
    pub revision: InstructionRevision,
    pub effect: InstructionEffect,
    /// The entry's sequence: strictly increasing per session.
    pub entry_sequence: u64,
    /// A delta appended at a run start, or a compaction's snapshot.
    pub kind: InstructionEntryKind,
    /// The run whose start applied the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// The entry row's index in the transcript; a compaction can change it,
    /// the entry sequence stays.
    pub message_index: u64,
}

/// A slot's state for one member or session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WireInstructionSlotState {
    /// The desired state is in effect.
    Effective,
    /// The desired state is not in effect yet.
    Pending {
        reason: WireInstructionPendingReason,
    },
    /// The member's delivery is blocked.
    Blocked { reason: WireInstructionBlockReason },
    /// Nothing selects the agent for the slot and nothing of it is in effect.
    NotTargeted,
}

/// One slot of one mob member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireMemberInstructionSlot {
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    /// The member's view of the mob's record.
    pub desired: WireInstructionSlotView,
    /// The member session's recorded target for the slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<WireInstructionSlotView>,
    /// The committed entry that put the slot in effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<WireInstructionEffectiveSlot>,
    pub state: WireInstructionSlotState,
}

/// A member's delivery state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WireMemberInstructionDeliveryState {
    /// The member's session recorded its current snapshot.
    Current,
    /// The member is owed the snapshot at `generation`.
    Owed {
        generation: u64,
        /// The last retryable failure of the member's delivery.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_failure: Option<String>,
    },
    /// The member's delivery is blocked until a write selects it again or it
    /// is seated again.
    Blocked { reason: WireInstructionBlockReason },
}

/// A member's delivery facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireMemberInstructionDelivery {
    /// The generation the member's session recorded, as the mob knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_generation: Option<u64>,
    pub state: WireMemberInstructionDeliveryState,
}

/// Whether a member's committed session state could be read. An unreadable
/// member keeps its row with the reason; nothing of its session is
/// reported, and the page still answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WireInstructionEffectiveRead {
    Ok,
    Unreadable { reason: String },
}

/// One member's operator-instruction status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireMemberInstructionStatus {
    pub agent_identity: String,
    pub profile: String,
    pub residency: WireInstructionResidency,
    pub delivery: WireMemberInstructionDelivery,
    pub slots: Vec<WireMemberInstructionSlot>,
    pub effective_read: WireInstructionEffectiveRead,
    /// A live channel is open on the member's session. Operator instructions
    /// do not reach a live channel: they apply to the member's turns.
    pub live_channel_open: bool,
}

/// Response payload for `mob/instruction_status`. Reads the committed session
/// documents and never wakes or materializes a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobInstructionStatusResult {
    pub mob_id: String,
    /// The mob's operator-instruction generation.
    pub generation: u64,
    /// The mob's records, sorted by slot.
    pub desired: Vec<WireInstructionRecord>,
    /// One page of members, in identity order.
    pub members: Vec<WireMemberInstructionStatus>,
    /// The offset of the next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

// ---------------------------------------------------------------------------
// Session verbs
// ---------------------------------------------------------------------------

/// Request payload for `session/set_instruction`: one session-owned write of
/// a slot's body at a revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SetSessionInstructionParams {
    pub session_id: String,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    /// Strictly increasing per slot within the session.
    pub revision: InstructionRevision,
    /// The exact UTF-8 body, at most 256 KiB.
    pub body: String,
    /// The body's digest as the caller computed it; one that is not the
    /// body's is refused (`digest_mismatch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<InstructionContentDigest>,
}

impl SetSessionInstructionParams {
    /// The write in the session service's vocabulary.
    #[must_use]
    pub fn write(&self) -> meerkat_core::SessionInstructionWrite {
        meerkat_core::SessionInstructionWrite {
            slot: InstructionSlot {
                namespace: self.namespace.clone(),
                key: self.key.clone(),
            },
            revision: self.revision,
            change: meerkat_core::SessionInstructionChange::Activate {
                body: self.body.clone(),
                content_sha256: self.content_sha256.clone(),
            },
        }
    }
}

/// Request payload for `session/revoke_instruction`: revoke one session-owned
/// slot at a higher revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RevokeSessionInstructionParams {
    pub session_id: String,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
}

impl RevokeSessionInstructionParams {
    /// The revoke in the session service's vocabulary.
    #[must_use]
    pub fn write(&self) -> meerkat_core::SessionInstructionWrite {
        meerkat_core::SessionInstructionWrite {
            slot: InstructionSlot {
                namespace: self.namespace.clone(),
                key: self.key.clone(),
            },
            revision: self.revision,
            change: meerkat_core::SessionInstructionChange::Revoke,
        }
    }
}

/// Request payload for `session/instruction_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SessionInstructionStatusParams {
    pub session_id: String,
}

/// What a session-owned write or revoke did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireSessionInstructionDisposition {
    // The write changed the session's target.
    Recorded,
    // An exact retry: the target already holds this revision and content.
    Duplicate,
}

impl From<meerkat_core::InstructionWriteDisposition> for WireSessionInstructionDisposition {
    fn from(disposition: meerkat_core::InstructionWriteDisposition) -> Self {
        match disposition {
            meerkat_core::InstructionWriteDisposition::Recorded => Self::Recorded,
            meerkat_core::InstructionWriteDisposition::Duplicate => Self::Duplicate,
        }
    }
}

/// Response payload for `session/set_instruction` and
/// `session/revoke_instruction`, returned once the target is durable in the
/// session document. The change applies at the session's next fresh run
/// start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionInstructionWriteResult {
    pub session_id: String,
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
    pub disposition: WireSessionInstructionDisposition,
    /// The generation of the session's recorded target after the write.
    pub target_generation: u64,
}

impl SessionInstructionWriteResult {
    /// The receipt of a session-owned write, as the wire carries it.
    #[must_use]
    pub fn from_receipt(
        session_id: &meerkat_core::types::SessionId,
        receipt: &meerkat_core::SessionInstructionReceipt,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            namespace: receipt.slot.namespace.clone(),
            key: receipt.slot.key.clone(),
            revision: receipt.revision,
            disposition: receipt.disposition.into(),
            target_generation: receipt.target_generation,
        }
    }
}

/// Who owns a session's recorded operator-instruction target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireInstructionTargetOwner {
    /// No owner holds a target.
    Unowned,
    /// The session's own host writes the target slot by slot.
    Session,
    /// A mob delivers its member's whole snapshot; only the mob writes it.
    Mob {
        mob_id: String,
        agent_identity: String,
    },
}

impl From<&InstructionTargetOwner> for WireInstructionTargetOwner {
    fn from(owner: &InstructionTargetOwner) -> Self {
        match owner {
            InstructionTargetOwner::Unowned => Self::Unowned,
            InstructionTargetOwner::Session => Self::Session,
            InstructionTargetOwner::Mob {
                mob_id,
                agent_identity,
            } => Self::Mob {
                mob_id: mob_id.clone(),
                agent_identity: agent_identity.clone(),
            },
        }
    }
}

/// One slot of a session: its recorded target and the entry in effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionInstructionSlot {
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    /// The session's recorded target for the slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<WireInstructionSlotView>,
    /// The committed entry that put the slot in effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<WireInstructionEffectiveSlot>,
    pub state: WireInstructionSlotState,
}

/// Response payload for `session/instruction_status`: the session's
/// committed operator-instruction state, read without materializing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionInstructionStatusResult {
    pub session_id: String,
    pub owner: WireInstructionTargetOwner,
    /// The generation of the recorded target.
    pub target_generation: u64,
    /// The highest entry sequence the session minted.
    pub entry_sequence: u64,
    /// How the session's model lowers entries, when its service can tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowering: Option<WireInstructionLowering>,
    /// Every slot the target or an entry names, sorted by slot.
    pub slots: Vec<WireSessionInstructionSlot>,
}

impl SessionInstructionStatusResult {
    /// The status of one session from its committed read: per slot, what is
    /// recorded, the entry that put it in effect, and whether it is in
    /// effect or still pending (and why, from the session's lowering).
    #[must_use]
    pub fn from_read(
        session_id: &meerkat_core::types::SessionId,
        read: &SessionInstructionRead,
    ) -> Self {
        let pending = WireInstructionPendingReason::for_lowering(read.lowering);
        let mut slots: BTreeSet<InstructionSlot> = read
            .effective
            .slots()
            .map(|(slot, _)| slot.clone())
            .collect();
        if let Some(target) = read.target.as_ref() {
            slots.extend(target.slots().iter().map(|record| record.slot()));
        }
        let slots = slots
            .into_iter()
            .map(|slot| {
                let recorded = read
                    .target
                    .as_ref()
                    .and_then(|target| target.slot(&slot))
                    .map(|record| WireInstructionSlotView {
                        revision: record.revision,
                        effect: record.effect,
                    });
                let effective =
                    read.effective
                        .slot(&slot)
                        .map(|effective| WireInstructionEffectiveSlot {
                            revision: effective.revision(),
                            effect: effective.effect(),
                            entry_sequence: effective.entry_sequence(),
                            kind: effective.entry_kind(),
                            run_id: effective.run_id().cloned(),
                            message_index: u64::try_from(effective.message_index())
                                .unwrap_or(u64::MAX),
                        });
                let in_effect = effective.as_ref().map(|effective| WireInstructionSlotView {
                    revision: effective.revision,
                    effect: effective.effect,
                });
                // The mob status read's rule, with the recorded target as the
                // desired state: nothing Active anywhere is not targeted, a
                // recorded state in effect is effective, and anything else
                // (a change recorded, or a slot the target no longer names
                // still in effect) waits for a run start.
                let active = |view: Option<WireInstructionSlotView>| -> bool {
                    view.is_some_and(|view| view.effect == InstructionEffect::Active)
                };
                let state = if !active(recorded) && !active(in_effect) {
                    WireInstructionSlotState::NotTargeted
                } else if recorded == in_effect {
                    WireInstructionSlotState::Effective
                } else {
                    WireInstructionSlotState::Pending {
                        reason: pending.clone(),
                    }
                };
                WireSessionInstructionSlot {
                    namespace: slot.namespace,
                    key: slot.key,
                    recorded,
                    effective,
                    state,
                }
            })
            .collect();
        let (owner, target_generation, entry_sequence) = match read.target.as_ref() {
            Some(target) => (
                WireInstructionTargetOwner::from(target.owner()),
                target.target_generation(),
                target.entry_sequence(),
            ),
            None => (
                WireInstructionTargetOwner::Unowned,
                0,
                read.effective.last_entry_sequence(),
            ),
        };
        Self {
            session_id: session_id.to_string(),
            owner,
            target_generation,
            entry_sequence,
            lowering: read.lowering.map(WireInstructionLowering::from),
            slots,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The advisory values of a status read are tolerant: a value this
    /// version does not know reads as `Unknown` with the value as it was
    /// sent, and is written back the same, while the known ones read typed.
    #[test]
    fn status_wire_reads_unknown_advisory_values_raw() {
        let member: WireMemberInstructionStatus = serde_json::from_value(json!({
            "agent_identity": "worker-1",
            "profile": "worker",
            "residency": "orbiting",
            "delivery": {
                "recorded_generation": 1,
                "state": { "state": "blocked", "reason": "host_on_holiday" }
            },
            "slots": [{
                "namespace": "app.example",
                "key": "tone",
                "desired": { "revision": 2, "effect": "active" },
                "state": { "state": "pending", "reason": "awaiting_the_moon" }
            }],
            "effective_read": { "status": "ok" },
            "live_channel_open": false
        }))
        .expect("unknown advisory values never fail the read");
        assert_eq!(
            member.residency,
            WireInstructionResidency::Unknown("orbiting".to_string())
        );
        assert_eq!(
            member.delivery.state,
            WireMemberInstructionDeliveryState::Blocked {
                reason: WireInstructionBlockReason::Unknown("host_on_holiday".to_string())
            }
        );
        assert_eq!(
            member.slots[0].state,
            WireInstructionSlotState::Pending {
                reason: WireInstructionPendingReason::Unknown("awaiting_the_moon".to_string())
            }
        );
        let written = serde_json::to_value(&member).unwrap();
        assert_eq!(written["residency"], "orbiting");
        assert_eq!(written["delivery"]["state"]["reason"], "host_on_holiday");
        assert_eq!(written["slots"][0]["state"]["reason"], "awaiting_the_moon");

        let lowering: WireInstructionLowering =
            serde_json::from_value(json!("carrier_pigeon")).unwrap();
        assert_eq!(
            lowering,
            WireInstructionLowering::Unknown("carrier_pigeon".to_string())
        );
        let known: WireInstructionLowering =
            serde_json::from_value(json!("leading_prefix_only")).unwrap();
        assert_eq!(known, WireInstructionLowering::LeadingPrefixOnly);
        assert_eq!(known.as_str(), "leading_prefix_only");
    }

    /// The state kinds, dispositions, effects and the target are closed: a
    /// value this version does not know fails the read.
    #[test]
    fn status_wire_refuses_unknown_closed_values() {
        for state in [
            json!({ "state": "levitating" }),
            json!({ "state": "pending" }),
        ] {
            assert!(
                serde_json::from_value::<WireInstructionSlotState>(state.clone()).is_err(),
                "{state}"
            );
        }
        assert!(
            serde_json::from_value::<WireMemberInstructionDeliveryState>(
                json!({ "state": "teleported" })
            )
            .is_err()
        );
        assert!(serde_json::from_value::<WireMobInstructionDisposition>(json!("merged")).is_err());
        assert!(
            serde_json::from_value::<WireSessionInstructionDisposition>(json!("merged")).is_err()
        );
        assert!(
            serde_json::from_value::<WireInstructionTarget>(json!({ "kind": "everyone" })).is_err()
        );
        assert!(
            serde_json::from_value::<WireInstructionTargetOwner>(json!({ "kind": "committee" }))
                .is_err()
        );
    }

    /// A write's params name their slot and target exactly; a field the
    /// request does not know is refused.
    #[test]
    fn instruction_params_refuse_unknown_fields_and_map_to_the_write() {
        let params: MobSetInstructionParams = serde_json::from_value(json!({
            "mob_id": "team",
            "namespace": "app.example",
            "key": "tone",
            "revision": 1,
            "body": "Answer formally.",
            "target": { "kind": "member", "agent_identity": "worker-1" }
        }))
        .unwrap();
        assert_eq!(
            params.target,
            WireInstructionTarget::Member {
                agent_identity: "worker-1".to_string()
            }
        );
        assert_eq!(params.slot().key.as_str(), "tone");
        assert!(
            serde_json::from_value::<MobSetInstructionParams>(json!({
                "mob_id": "team",
                "namespace": "app.example",
                "key": "tone",
                "revision": 1,
                "body": "Answer formally.",
                "target": { "kind": "mob" },
                "priority": "high"
            }))
            .is_err()
        );
        // A target refuses a field its kind does not have, the mob kind's
        // too: a member or profile name sent with the wrong kind must not
        // widen the write to every member.
        for target in [
            json!({ "kind": "mob", "agent_identity": "worker-1" }),
            json!({ "kind": "mob", "profile": "worker" }),
            json!({ "kind": "profile", "profile": "worker", "agent_identity": "worker-1" }),
            json!({ "kind": "member", "agent_identity": "worker-1", "profile": "worker" }),
        ] {
            assert!(
                serde_json::from_value::<WireInstructionTarget>(target.clone()).is_err(),
                "a target with a field its kind does not have is refused: {target}"
            );
        }
        let widened = json!({
            "mob_id": "team",
            "namespace": "app.example",
            "key": "tone",
            "revision": 1,
            "body": "Answer formally.",
            "target": { "kind": "mob", "agent_identity": "worker-1" }
        });
        assert!(
            serde_json::from_value::<MobSetInstructionParams>(widened).is_err(),
            "a write whose mob target names a member is refused, not widened"
        );
        let mob: WireInstructionTarget = serde_json::from_value(json!({ "kind": "mob" })).unwrap();
        assert_eq!(mob, WireInstructionTarget::Mob {});
        assert_eq!(
            serde_json::to_value(&mob).unwrap(),
            json!({ "kind": "mob" }),
            "the mob target's wire form is its kind alone"
        );
        let revoke: RevokeSessionInstructionParams = serde_json::from_value(json!({
            "session_id": "019a2b3c-0000-7000-8000-000000000001",
            "namespace": "app.example",
            "key": "tone",
            "revision": 3
        }))
        .unwrap();
        assert_eq!(
            revoke.write().change,
            meerkat_core::SessionInstructionChange::Revoke
        );
        assert_eq!(revoke.write().revision.get(), 3);
    }
}

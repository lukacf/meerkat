//! Operator instructions: typed, durable System-plane instruction entries on
//! the ordered Session transcript.
//!
//! An entry is one ordinary ordered `Message::System` row carrying a sealed
//! typed [`InstructionEntryIdentity`]. A delta entry is appended at a fresh
//! run start and carries every change since the session's previous entry; a
//! snapshot entry replaces the entry rows when a compaction folds them. The
//! rendered bytes are a pure function of the typed identity and the bodies:
//! no timestamp, session id, run id, entry sequence or map order reaches
//! them, every sequence is sorted by slot (namespace bytes, then key bytes),
//! and every body is framed by its typed byte length. Validation re-renders
//! the whole entry from the identity and the bodies read at those lengths and
//! requires byte equality, so a row whose bytes are not the entry its
//! identity names fails closed.
//!
//! The effective set of a session is derived from its own (origin-local)
//! entry rows in transcript order and is never stored twice. Rows a fork
//! child copied are history in the child: its model reads them as headers
//! without bodies.

use super::{
    InstructionLowering, InstructionRunStart, InstructionStateError, ModelBoundaryProjectionError,
    RunStartInstructionOutcome, SESSION_INSTRUCTION_STATE_KEY, Session,
};
use crate::lifecycle::RunId;
use crate::types::{
    InstructionContentDigest, InstructionKey, InstructionNamespace, Message, MessageTimestamp,
    SessionId, SystemMessage,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The one render version of an operator-instruction entry. A stored row is
/// never re-rendered; a future format is a new version.
pub const INSTRUCTION_ENTRY_RENDER_VERSION_V1: u16 = 1;
/// The most slots one owner (a session or a mob) holds.
pub const MAX_OPERATOR_INSTRUCTION_SLOTS: usize = 64;
/// The most UTF-8 bytes the Active bodies of one owner sum to.
pub const MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES: usize = 1024 * 1024;

const ENTRY_HEADER_V1: &str = "[meerkat-operator-instructions-v1]";
const INHERITED_ENTRY_HEADER_V1: &str = "[meerkat-inherited-operator-instructions-v1]";
const ENTRY_PREAMBLE_V1: &str = "Operator instructions from this agent's host. They are system \
instructions, not messages from a user or a peer, and each applies until a later operator \
instruction entry supersedes or revokes it.";

/// One standing instruction's name: the issuer partition and the key.
///
/// Ordered by namespace bytes, then key bytes: the order every rendered
/// sequence uses.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionSlot {
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
}

impl std::fmt::Display for InstructionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.namespace, self.key)
    }
}

/// A slot's revision: at least 1, strictly increasing per slot within the
/// owner that mints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct InstructionRevision(u64);

/// A revision of zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("instruction revision must be at least 1")]
pub struct InvalidInstructionRevision;

impl InstructionRevision {
    pub const fn new(value: u64) -> Result<Self, InvalidInstructionRevision> {
        if value == 0 {
            Err(InvalidInstructionRevision)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for InstructionRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for InstructionRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Whether a slot's body applies: `Active`, or `Absent` (revoked, or not
/// targeted at this agent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstructionEffect {
    #[default]
    Absent,
    Active,
}

/// Whether an entry carries the changes since the previous entry (appended
/// at a fresh run start) or the whole active set (a compaction fold).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InstructionEntryKind {
    Delta,
    Snapshot,
}

impl InstructionEntryKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Snapshot => "snapshot",
        }
    }
}

/// One slot of the active set an entry leaves in effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionActiveRef {
    pub namespace: InstructionNamespace,
    pub key: InstructionKey,
    pub revision: InstructionRevision,
    pub content_sha256: InstructionContentDigest,
    /// Length of the body in UTF-8 bytes: a snapshot reads each body at
    /// this length.
    pub body_bytes: u64,
}

impl InstructionActiveRef {
    #[must_use]
    pub fn slot(&self) -> InstructionSlot {
        InstructionSlot {
            namespace: self.namespace.clone(),
            key: self.key.clone(),
        }
    }
}

/// One change a delta entry carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstructionChange {
    /// The body at `revision` applies from this entry on. `supersedes` is
    /// the revision that was in effect for the slot, if one was.
    Activate {
        namespace: InstructionNamespace,
        key: InstructionKey,
        revision: InstructionRevision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<InstructionRevision>,
        content_sha256: InstructionContentDigest,
        body_bytes: u64,
    },
    /// The slot's body stops applying: `revokes` is the revision that was in
    /// effect.
    Revoke {
        namespace: InstructionNamespace,
        key: InstructionKey,
        revision: InstructionRevision,
        revokes: InstructionRevision,
    },
}

impl InstructionChange {
    #[must_use]
    pub fn slot(&self) -> InstructionSlot {
        let (namespace, key) = match self {
            Self::Activate { namespace, key, .. } | Self::Revoke { namespace, key, .. } => {
                (namespace, key)
            }
        };
        InstructionSlot {
            namespace: namespace.clone(),
            key: key.clone(),
        }
    }

    #[must_use]
    pub fn revision(&self) -> InstructionRevision {
        match self {
            Self::Activate { revision, .. } | Self::Revoke { revision, .. } => *revision,
        }
    }
}

/// Typed identity sealed onto one operator-instruction entry row.
///
/// It has no public constructor: only the session-document authorization of
/// a run start or of a compaction fold mints one, and rewrite input cannot
/// express it. Reads (history, wire) carry it as they carry the other System
/// identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InstructionEntryIdentity {
    render_version: u16,
    origin_session_id: SessionId,
    entry_sequence: u64,
    kind: InstructionEntryKind,
    target_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    active: Vec<InstructionActiveRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changes: Vec<InstructionChange>,
}

impl InstructionEntryIdentity {
    #[must_use]
    pub fn render_version(&self) -> u16 {
        self.render_version
    }

    /// The session that minted the entry. In any other session (a fork
    /// child) the row is history, not an instruction.
    #[must_use]
    pub fn origin_session_id(&self) -> &SessionId {
        &self.origin_session_id
    }

    /// Strictly increasing per origin session, across deltas and snapshots.
    #[must_use]
    pub fn entry_sequence(&self) -> u64 {
        self.entry_sequence
    }

    #[must_use]
    pub fn kind(&self) -> InstructionEntryKind {
        self.kind
    }

    /// The recorded target generation this entry realizes.
    #[must_use]
    pub fn target_generation(&self) -> u64 {
        self.target_generation
    }

    /// The run whose start applied the entry, the run the agent itself runs:
    /// a delta's run, or the run at whose first request a fold applied the
    /// recorded target. None for a fold of what was already in effect.
    #[must_use]
    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    /// The whole active set after this entry, sorted by slot.
    #[must_use]
    pub fn active(&self) -> &[InstructionActiveRef] {
        &self.active
    }

    /// A delta's changes, sorted by slot; empty for a snapshot.
    #[must_use]
    pub fn changes(&self) -> &[InstructionChange] {
        &self.changes
    }
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

/// The bytes of an entry before its first section: the header, the fixed
/// preamble, the kind, the active set (a delta lists it without bodies), the
/// change count of a delta, then one empty line.
fn render_prefix_v1(identity: &InstructionEntryIdentity) -> String {
    let mut out = String::with_capacity(320);
    out.push_str(ENTRY_HEADER_V1);
    out.push('\n');
    out.push_str(ENTRY_PREAMBLE_V1);
    out.push('\n');
    out.push_str(&format!("kind=\"{}\"\n", identity.kind.as_str()));
    out.push_str(&format!("active_count={}\n", identity.active.len()));
    if identity.kind == InstructionEntryKind::Delta {
        for active in &identity.active {
            out.push_str(&format!(
                "active={} {} {}\n",
                json_string(active.namespace.as_str()),
                json_string(active.key.as_str()),
                active.revision
            ));
        }
        out.push_str(&format!("change_count={}\n", identity.changes.len()));
    }
    out.push('\n');
    out
}

/// A delta section before its body; a revoke section whole (it has none).
fn render_change_head_v1(change: &InstructionChange) -> String {
    match change {
        InstructionChange::Activate {
            namespace,
            key,
            revision,
            supersedes,
            content_sha256,
            body_bytes,
        } => format!(
            "change=\"activate\"\nnamespace={}\nkey={}\nrevision={revision}\nsupersedes={}\ncontent_sha256={content_sha256}\nbody_bytes={body_bytes}\n\n",
            json_string(namespace.as_str()),
            json_string(key.as_str()),
            supersedes.map_or_else(|| "null".to_string(), |value| value.to_string()),
        ),
        InstructionChange::Revoke {
            namespace,
            key,
            revision,
            revokes,
        } => format!(
            "change=\"revoke\"\nnamespace={}\nkey={}\nrevision={revision}\nrevokes={revokes}\n",
            json_string(namespace.as_str()),
            json_string(key.as_str()),
        ),
    }
}

/// A snapshot section before its body.
fn render_snapshot_head_v1(active: &InstructionActiveRef) -> String {
    format!(
        "instruction\nnamespace={}\nkey={}\nrevision={}\ncontent_sha256={}\nbody_bytes={}\n\n",
        json_string(active.namespace.as_str()),
        json_string(active.key.as_str()),
        active.revision,
        active.content_sha256,
        active.body_bytes,
    )
}

/// Render the whole entry from its identity and its bodies, in identity
/// order (a delta's activate changes, or a snapshot's active set). Each body
/// ends with one line feed.
fn render_entry_v1(identity: &InstructionEntryIdentity, bodies: &[&str]) -> Result<String, String> {
    let mut out = render_prefix_v1(identity);
    let mut bodies = bodies.iter();
    let mut next_body = |section: &str| {
        bodies
            .next()
            .copied()
            .ok_or_else(|| format!("operator instruction entry has no body for {section}"))
    };
    match identity.kind {
        InstructionEntryKind::Delta => {
            for change in &identity.changes {
                out.push_str(&render_change_head_v1(change));
                if matches!(change, InstructionChange::Activate { .. }) {
                    out.push_str(next_body(&change.slot().to_string())?);
                    out.push('\n');
                }
            }
        }
        InstructionEntryKind::Snapshot => {
            for active in &identity.active {
                out.push_str(&render_snapshot_head_v1(active));
                out.push_str(next_body(&active.slot().to_string())?);
                out.push('\n');
            }
        }
    }
    if bodies.next().is_some() {
        return Err("operator instruction entry was given more bodies than sections".to_string());
    }
    Ok(out)
}

/// One body at its typed length, followed by its line feed.
fn take_body(rest: &str, body_bytes: u64) -> Result<(&str, &str), String> {
    let length = usize::try_from(body_bytes)
        .map_err(|_| "operator instruction body length does not fit this target".to_string())?;
    let body = rest.get(..length).ok_or_else(|| {
        "operator instruction body is shorter than its declared length or splits a character"
            .to_string()
    })?;
    let after = rest
        .get(length..)
        .and_then(|after| after.strip_prefix('\n'))
        .ok_or_else(|| "operator instruction body is not followed by its line feed".to_string())?;
    Ok((body, after))
}

fn check_body(
    body: &str,
    declared: &InstructionContentDigest,
    section: &InstructionSlot,
) -> Result<(), String> {
    let computed = InstructionContentDigest::for_body(body);
    if &computed != declared {
        return Err(format!(
            "operator instruction body for {section} does not match its digest: declared \
             {declared}, computed {computed}"
        ));
    }
    Ok(())
}

/// How [`walk_entry_bodies`] reads an entry's bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyRead {
    /// Check every body against its digest, then the whole row against a
    /// re-render of its typed identity.
    Verify,
    /// The row's transcript buffer already verified it, and a row cannot
    /// change in place without dropping that record: locate the bodies at
    /// their typed lengths only.
    Verified,
}

/// The bodies an entry's content holds, in identity order, read at their
/// typed byte lengths. Every byte of the content is compared with the render
/// of the typed identity, so a row whose bytes are not the entry its identity
/// names is refused, whatever its body says.
fn entry_bodies<'a>(
    identity: &InstructionEntryIdentity,
    content: &'a str,
) -> Result<Vec<&'a str>, String> {
    walk_entry_bodies(identity, content, BodyRead::Verify)
}

fn walk_entry_bodies<'a>(
    identity: &InstructionEntryIdentity,
    content: &'a str,
    read: BodyRead,
) -> Result<Vec<&'a str>, String> {
    let prefix = render_prefix_v1(identity);
    let mut rest = content.strip_prefix(prefix.as_str()).ok_or_else(|| {
        "operator instruction entry header does not match its typed identity".to_string()
    })?;
    let mut bodies = Vec::new();
    match identity.kind {
        InstructionEntryKind::Delta => {
            for change in &identity.changes {
                let head = render_change_head_v1(change);
                rest = rest.strip_prefix(head.as_str()).ok_or_else(|| {
                    format!(
                        "operator instruction change for {} does not match its typed identity",
                        change.slot()
                    )
                })?;
                if let InstructionChange::Activate {
                    content_sha256,
                    body_bytes,
                    ..
                } = change
                {
                    let (body, after) = take_body(rest, *body_bytes)?;
                    if read == BodyRead::Verify {
                        check_body(body, content_sha256, &change.slot())?;
                    }
                    bodies.push(body);
                    rest = after;
                }
            }
        }
        InstructionEntryKind::Snapshot => {
            for active in &identity.active {
                let head = render_snapshot_head_v1(active);
                rest = rest.strip_prefix(head.as_str()).ok_or_else(|| {
                    format!(
                        "operator instruction snapshot section for {} does not match its typed \
                         identity",
                        active.slot()
                    )
                })?;
                let (body, after) = take_body(rest, active.body_bytes)?;
                if read == BodyRead::Verify {
                    check_body(body, &active.content_sha256, &active.slot())?;
                }
                bodies.push(body);
                rest = after;
            }
        }
    }
    if !rest.is_empty() {
        return Err("operator instruction entry has bytes after its last section".to_string());
    }
    // The walk compared every byte; the whole re-render is the same check
    // stated once, kept so a future renderer change cannot drift from it.
    if read == BodyRead::Verify && render_entry_v1(identity, &bodies)? != content {
        return Err("operator instruction entry bytes do not match its typed identity".to_string());
    }
    Ok(bodies)
}

fn strictly_sorted<T, K: Ord>(items: &[T], key: impl Fn(&T) -> K) -> bool {
    items.windows(2).all(|pair| key(&pair[0]) < key(&pair[1]))
}

/// The identity's own shape: version, sequence, canonical (sorted, unique)
/// sequences, a delta's changes agreeing with its active set, and the slot
/// and byte budgets of the active set.
fn validate_identity_shape(identity: &InstructionEntryIdentity) -> Result<(), String> {
    if identity.render_version != INSTRUCTION_ENTRY_RENDER_VERSION_V1 {
        return Err(format!(
            "unsupported operator instruction entry render version {}",
            identity.render_version
        ));
    }
    if identity.entry_sequence == 0 {
        return Err("operator instruction entry sequence must be at least 1".to_string());
    }
    if !strictly_sorted(&identity.active, InstructionActiveRef::slot) {
        return Err(
            "operator instruction active set must be sorted by slot without repeats".to_string(),
        );
    }
    if identity.active.len() > MAX_OPERATOR_INSTRUCTION_SLOTS {
        return Err(format!(
            "operator instruction active set holds more than {MAX_OPERATOR_INSTRUCTION_SLOTS} slots"
        ));
    }
    let mut active_bytes = 0u64;
    for active in &identity.active {
        if active.body_bytes > super::MAX_INSTRUCTION_BODY_BYTES as u64 {
            return Err(format!(
                "operator instruction body for {} exceeds {} UTF-8 bytes",
                active.slot(),
                super::MAX_INSTRUCTION_BODY_BYTES
            ));
        }
        active_bytes = active_bytes.saturating_add(active.body_bytes);
    }
    if active_bytes > MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES as u64 {
        return Err(format!(
            "operator instruction active set exceeds {MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES} \
             UTF-8 bytes"
        ));
    }
    match identity.kind {
        InstructionEntryKind::Delta => {
            if identity.changes.is_empty() {
                return Err("operator instruction delta carries no change".to_string());
            }
            if !strictly_sorted(&identity.changes, InstructionChange::slot) {
                return Err(
                    "operator instruction changes must be sorted by slot without repeats"
                        .to_string(),
                );
            }
            for change in &identity.changes {
                let slot = change.slot();
                let listed = identity.active.iter().find(|active| active.slot() == slot);
                match change {
                    InstructionChange::Activate {
                        revision,
                        content_sha256,
                        body_bytes,
                        ..
                    } => {
                        let agrees = listed.is_some_and(|active| {
                            active.revision == *revision
                                && &active.content_sha256 == content_sha256
                                && active.body_bytes == *body_bytes
                        });
                        if !agrees {
                            return Err(format!(
                                "operator instruction activation of {slot} is not in the active \
                                 set it leaves"
                            ));
                        }
                    }
                    InstructionChange::Revoke { .. } => {
                        if listed.is_some() {
                            return Err(format!(
                                "operator instruction revocation of {slot} leaves it active"
                            ));
                        }
                    }
                }
            }
        }
        InstructionEntryKind::Snapshot => {
            if !identity.changes.is_empty() {
                return Err("operator instruction snapshot carries changes".to_string());
            }
            if identity.active.is_empty() {
                return Err("operator instruction snapshot of an empty set".to_string());
            }
        }
    }
    Ok(())
}

/// One slot as an origin's entry chain leaves it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainSlot {
    revision: InstructionRevision,
    /// The digest and body length while the slot is Active.
    active: Option<(InstructionContentDigest, u64)>,
}

/// One origin session's entries, folded in transcript order.
#[derive(Debug, Default)]
struct OriginChain {
    last_sequence: u64,
    slots: BTreeMap<InstructionSlot, ChainSlot>,
}

impl OriginChain {
    /// Fold one entry, refusing one that does not continue the chain: a
    /// sequence that does not increase, a change whose predecessor is not
    /// the revision in effect, or an active set the changes do not produce.
    fn apply(&mut self, identity: &InstructionEntryIdentity) -> Result<(), String> {
        if identity.entry_sequence <= self.last_sequence {
            return Err(format!(
                "operator instruction entry sequence {} does not follow {}",
                identity.entry_sequence, self.last_sequence
            ));
        }
        match identity.kind {
            InstructionEntryKind::Delta => {
                for change in &identity.changes {
                    let slot = change.slot();
                    let in_effect = self
                        .slots
                        .get(&slot)
                        .and_then(|state| state.active.as_ref().map(|_| state.revision));
                    match change {
                        InstructionChange::Activate {
                            revision,
                            supersedes,
                            content_sha256,
                            body_bytes,
                            ..
                        } => {
                            if *supersedes != in_effect {
                                return Err(format!(
                                    "operator instruction activation of {slot} supersedes \
                                     {supersedes:?}, but the revision in effect is {in_effect:?}"
                                ));
                            }
                            self.slots.insert(
                                slot,
                                ChainSlot {
                                    revision: *revision,
                                    active: Some((content_sha256.clone(), *body_bytes)),
                                },
                            );
                        }
                        InstructionChange::Revoke {
                            revision, revokes, ..
                        } => {
                            if in_effect != Some(*revokes) {
                                return Err(format!(
                                    "operator instruction revocation of {slot} revokes \
                                     {revokes}, but the revision in effect is {in_effect:?}"
                                ));
                            }
                            self.slots.insert(
                                slot,
                                ChainSlot {
                                    revision: *revision,
                                    active: None,
                                },
                            );
                        }
                    }
                }
                let produced = self
                    .slots
                    .iter()
                    .filter_map(|(slot, state)| {
                        state.active.as_ref().map(|(digest, body_bytes)| {
                            (slot.clone(), state.revision, digest.clone(), *body_bytes)
                        })
                    })
                    .collect::<Vec<_>>();
                let listed = identity
                    .active
                    .iter()
                    .map(|active| {
                        (
                            active.slot(),
                            active.revision,
                            active.content_sha256.clone(),
                            active.body_bytes,
                        )
                    })
                    .collect::<Vec<_>>();
                if produced != listed {
                    return Err(
                        "operator instruction delta lists an active set its changes do not \
                         produce"
                            .to_string(),
                    );
                }
            }
            InstructionEntryKind::Snapshot => {
                self.slots = identity
                    .active
                    .iter()
                    .map(|active| {
                        (
                            active.slot(),
                            ChainSlot {
                                revision: active.revision,
                                active: Some((active.content_sha256.clone(), active.body_bytes)),
                            },
                        )
                    })
                    .collect();
            }
        }
        self.last_sequence = identity.entry_sequence;
        Ok(())
    }
}

/// Whether `message` is an operator-instruction entry row.
pub(crate) fn is_operator_instruction_entry(message: &Message) -> bool {
    matches!(message, Message::System(system) if system.instruction_entry.is_some())
}

/// Every operator-instruction entry row with its transcript index.
pub(crate) fn operator_instruction_entry_rows(messages: &[Message]) -> Vec<(usize, &Message)> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| is_operator_instruction_entry(message))
        .collect()
}

/// Every operator-instruction entry row with its place: the number of rows
/// before it that are not System rows, and its order among the System rows
/// of its gap (the rows after the last of those) that the model reads, that
/// is every System row but a prompt version a later version of its key
/// supersedes ([`crate::types::superseded_system_prompt_offsets`]). A row
/// carries its typed identity (origin session and entry sequence), so two of
/// these lists are equal when the same entry rows stand at the same places,
/// whatever their offsets. A keyed system-prompt update leaves the list as
/// it was: the first version of a key replaces the ordinary System row it
/// adopts and stands where that row stood, and a later version, inserted
/// right after the current one, takes the place the current one leaves when
/// it is superseded. Minting, altering or erasing an entry row, or moving
/// one across a conversation row or a System row the model reads, changes
/// it.
pub(crate) fn operator_instruction_entry_places(
    messages: &[Message],
) -> Vec<(usize, usize, &Message)> {
    let superseded = crate::types::superseded_system_prompt_offsets(messages);
    let mut conversation_rows = 0;
    let mut gap_rows = 0;
    let mut places = Vec::new();
    for (offset, message) in messages.iter().enumerate() {
        let Message::System(system) = message else {
            conversation_rows += 1;
            gap_rows = 0;
            continue;
        };
        if system.instruction_entry.is_some() {
            places.push((conversation_rows, gap_rows, message));
        }
        if !superseded.contains(&offset) {
            gap_rows += 1;
        }
    }
    places
}

/// How many rows of `messages` are not operator-instruction entries: what
/// the compaction shrink rule counts. A fold's snapshot and the entry rows
/// it takes out are neutral in it, so a compaction makes progress only when
/// the compactor's own rebuild takes rows out.
pub(crate) fn rows_besides_operator_instruction_entries(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| !is_operator_instruction_entry(message))
        .count()
}

/// Why a model change cannot carry operator-instruction entry rows where
/// they stand in the requests the target would serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionEntryLoweringRefusal {
    /// An entry row stands after a conversation row, and the target carries
    /// System rows only before the first conversation row.
    AfterConversationRow,
    /// An entry row stands beside another System row, and the target's wire
    /// carries one leading System row.
    BesideAnotherSystemRow,
}

impl std::fmt::Display for InstructionEntryLoweringRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AfterConversationRow => {
                "an entry row stands after a conversation row, and the target carries System \
                 rows only before the first one"
            }
            Self::BesideAnotherSystemRow => {
                "an entry row stands beside another System row, and the target's wire carries \
                 one leading System row"
            }
        })
    }
}

/// Where operator-instruction entry rows stand in a model request: what a
/// model change must carry. [`Self::refusal`] says whether a target that
/// lowers entries as an [`InstructionLowering`] carries them there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstructionEntryPlacement {
    /// The entry rows judged.
    entries: usize,
    /// The request's System rows that are not entry rows.
    other_system_rows: usize,
    /// Whether a judged entry row stands after a conversation row (a row
    /// that is not a System row).
    after_conversation_row: bool,
}

impl InstructionEntryPlacement {
    /// The placement of every operator-instruction entry row in `request`,
    /// the rows a model is asked with.
    #[must_use]
    pub fn of_request(request: &[Message]) -> Self {
        Self::of(request, |_| true)
    }

    /// The placement of the entry rows `judged` selects among `messages`. An
    /// entry row it does not select is neither judged nor another System
    /// row.
    fn of(messages: &[Message], judged: impl Fn(&InstructionEntryIdentity) -> bool) -> Self {
        let mut placement = Self::default();
        let mut conversation_seen = false;
        for message in messages {
            let Message::System(system) = message else {
                conversation_seen = true;
                continue;
            };
            match system.instruction_entry.as_ref() {
                Some(identity) if judged(identity) => {
                    placement.entries = placement.entries.saturating_add(1);
                    placement.after_conversation_row |= conversation_seen;
                }
                Some(_) => {}
                None => {
                    placement.other_system_rows = placement.other_system_rows.saturating_add(1);
                }
            }
        }
        placement
    }

    /// Whether the placement holds no entry row: every target carries it.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.entries == 0
    }

    /// This placement with one more System row besides the entry rows: a
    /// row a build adds to the request, such as the prompt it authors.
    #[must_use]
    pub fn with_another_system_row(self) -> Self {
        Self {
            other_system_rows: self.other_system_rows.saturating_add(1),
            ..self
        }
    }

    /// Why a target that lowers operator-instruction entries as `lowering`
    /// cannot carry entry rows placed so, or `None` when it can. A target
    /// that lowers them anywhere carries every placement; one that lowers
    /// them only inside the leading System prefix cannot carry an entry
    /// after a conversation row; a wire that carries one leading System row
    /// cannot carry that either, nor an entry beside another System row.
    #[must_use]
    pub fn refusal(self, lowering: InstructionLowering) -> Option<InstructionEntryLoweringRefusal> {
        if self.is_empty() {
            return None;
        }
        match lowering {
            InstructionLowering::MidConversation => None,
            InstructionLowering::LeadingPrefixOnly => self
                .after_conversation_row
                .then_some(InstructionEntryLoweringRefusal::AfterConversationRow),
            InstructionLowering::Unavailable => {
                if self.after_conversation_row {
                    Some(InstructionEntryLoweringRefusal::AfterConversationRow)
                } else if self.entries > 1 || self.other_system_rows > 0 {
                    Some(InstructionEntryLoweringRefusal::BesideAnotherSystemRow)
                } else {
                    None
                }
            }
        }
    }
}

fn entry_rows(
    messages: &[Message],
) -> impl Iterator<Item = (usize, &SystemMessage, &InstructionEntryIdentity)> {
    messages.iter().enumerate().filter_map(|(index, message)| {
        let Message::System(system) = message else {
            return None;
        };
        system
            .instruction_entry
            .as_ref()
            .map(|identity| (index, system, identity))
    })
}

/// Validate every operator-instruction entry row: the typed identity's
/// shape, the bytes against the re-render (digests included), and each
/// origin session's chronology (strictly increasing sequence, predecessors
/// that match the revision in effect, active sets the changes produce).
///
/// Exclusivity with the other System identities is checked by
/// [`super::validate_instruction_activation_messages`], which calls this.
pub fn validate_operator_instruction_messages(messages: &[Message]) -> Result<(), String> {
    let mut chains: BTreeMap<SessionId, OriginChain> = BTreeMap::new();
    for (_, system, identity) in entry_rows(messages) {
        validate_identity_shape(identity)?;
        entry_bodies(identity, &system.content)?;
        chains
            .entry(identity.origin_session_id.clone())
            .or_default()
            .apply(identity)?;
    }
    Ok(())
}

/// An entry row a fork child copied, as the child's model reads it: the
/// slots, revisions and digests the origin had in effect, never a body.
fn render_inherited_v1(
    identity: &InstructionEntryIdentity,
    child_session_id: &SessionId,
) -> String {
    let mut out = String::with_capacity(256);
    out.push_str(INHERITED_ENTRY_HEADER_V1);
    out.push('\n');
    out.push_str(&format!(
        "origin_session_id={}\n",
        json_string(&identity.origin_session_id.to_string())
    ));
    out.push_str(&format!(
        "child_session_id={}\n",
        json_string(&child_session_id.to_string())
    ));
    out.push_str(&format!("kind=\"{}\"\n", identity.kind.as_str()));
    out.push_str(&format!("active_count={}\n", identity.active.len()));
    for active in &identity.active {
        out.push_str(&format!(
            "active={} {} {} {}\n",
            json_string(active.namespace.as_str()),
            json_string(active.key.as_str()),
            active.revision,
            active.content_sha256
        ));
    }
    out.push_str("status=not_active_in_child");
    out
}

/// Project entry rows in place for `session_id`'s model: a row it minted
/// keeps its stored bytes; a row another session minted (copied by a fork)
/// reads as its inherited header. A row whose bytes are not the entry its
/// identity names fails the projection.
fn project_operator_instruction_messages_in_place(
    session_id: &SessionId,
    messages: &mut [Message],
) -> Result<(), String> {
    for (_, system, identity) in entry_rows(messages) {
        validate_identity_shape(identity)?;
        entry_bodies(identity, &system.content)?;
    }
    project_verified_operator_instruction_messages_in_place(session_id, messages);
    Ok(())
}

/// [`project_operator_instruction_messages_in_place`] over entry rows the
/// session's transcript buffer already verified
/// ([`Session::verify_operator_instruction_rows`]): nothing is re-rendered
/// or re-hashed, a copied row only reads as its inherited header.
pub(crate) fn project_verified_operator_instruction_messages_in_place(
    session_id: &SessionId,
    messages: &mut [Message],
) {
    for message in messages.iter_mut() {
        let Message::System(system) = message else {
            continue;
        };
        let Some(identity) = system.instruction_entry.as_ref() else {
            continue;
        };
        if &identity.origin_session_id != session_id {
            system.content = render_inherited_v1(identity, session_id);
        }
    }
}

/// [`project_operator_instruction_messages_in_place`] over a copy.
pub fn materialize_operator_instruction_messages(
    session_id: &SessionId,
    messages: &[Message],
) -> Result<Vec<Message>, String> {
    let mut projected = messages.to_vec();
    project_operator_instruction_messages_in_place(session_id, &mut projected)?;
    Ok(projected)
}

/// The body of an Active effective slot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveBody {
    content_sha256: InstructionContentDigest,
    body: String,
}

/// One slot of a session's effective set, and the entry that put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveInstructionSlot {
    revision: InstructionRevision,
    active: Option<EffectiveBody>,
    entry_sequence: u64,
    entry_kind: InstructionEntryKind,
    run_id: Option<RunId>,
    message_index: usize,
}

impl EffectiveInstructionSlot {
    #[must_use]
    pub fn revision(&self) -> InstructionRevision {
        self.revision
    }

    #[must_use]
    pub fn effect(&self) -> InstructionEffect {
        if self.active.is_some() {
            InstructionEffect::Active
        } else {
            InstructionEffect::Absent
        }
    }

    /// The digest of the body in effect; none while the slot is Absent.
    #[must_use]
    pub fn content_sha256(&self) -> Option<&InstructionContentDigest> {
        self.active.as_ref().map(|active| &active.content_sha256)
    }

    /// The body in effect, read from the entry row that holds it; none while
    /// the slot is Absent.
    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.active.as_ref().map(|active| active.body.as_str())
    }

    /// The entry that made this state effective.
    #[must_use]
    pub fn entry_sequence(&self) -> u64 {
        self.entry_sequence
    }

    #[must_use]
    pub fn entry_kind(&self) -> InstructionEntryKind {
        self.entry_kind
    }

    #[must_use]
    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    /// Index of that entry row in the transcript it was read from. A
    /// compaction can change it; the entry sequence stays.
    #[must_use]
    pub fn message_index(&self) -> usize {
        self.message_index
    }
}

/// A session's effective operator-instruction set: its own entry rows,
/// folded in transcript order. Rows a fork copied from another session do
/// not count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveOperatorInstructions {
    slots: BTreeMap<InstructionSlot, EffectiveInstructionSlot>,
    last_entry_sequence: u64,
    last_target_generation: u64,
}

impl EffectiveOperatorInstructions {
    /// No origin-local entry has put any slot in effect.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Slots whose body applies.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.slots
            .values()
            .filter(|slot| slot.active.is_some())
            .count()
    }

    /// The sequence of the session's last entry row; zero without one.
    #[must_use]
    pub fn last_entry_sequence(&self) -> u64 {
        self.last_entry_sequence
    }

    /// The target generation the session's last entry row realized; zero
    /// without one.
    #[must_use]
    pub fn last_target_generation(&self) -> u64 {
        self.last_target_generation
    }

    #[must_use]
    pub fn slot(&self, slot: &InstructionSlot) -> Option<&EffectiveInstructionSlot> {
        self.slots.get(slot)
    }

    /// Every slot an entry named, in slot order, Absent ones included.
    pub fn slots(&self) -> impl Iterator<Item = (&InstructionSlot, &EffectiveInstructionSlot)> {
        self.slots.iter()
    }

    fn apply(
        &mut self,
        identity: &InstructionEntryIdentity,
        bodies: &[&str],
        message_index: usize,
    ) -> Result<(), String> {
        let at = |revision: InstructionRevision, active: Option<EffectiveBody>| {
            EffectiveInstructionSlot {
                revision,
                active,
                entry_sequence: identity.entry_sequence,
                entry_kind: identity.kind,
                run_id: identity.run_id.clone(),
                message_index,
            }
        };
        let mut bodies = bodies.iter();
        let mut next_body = || {
            bodies
                .next()
                .map(|body| (*body).to_string())
                .ok_or_else(|| "operator instruction entry is missing a body".to_string())
        };
        match identity.kind {
            InstructionEntryKind::Delta => {
                for change in &identity.changes {
                    match change {
                        InstructionChange::Activate {
                            revision,
                            content_sha256,
                            ..
                        } => {
                            let body = next_body()?;
                            self.slots.insert(
                                change.slot(),
                                at(
                                    *revision,
                                    Some(EffectiveBody {
                                        content_sha256: content_sha256.clone(),
                                        body,
                                    }),
                                ),
                            );
                        }
                        InstructionChange::Revoke { revision, .. } => {
                            self.slots.insert(change.slot(), at(*revision, None));
                        }
                    }
                }
            }
            InstructionEntryKind::Snapshot => {
                let mut slots = BTreeMap::new();
                for active in &identity.active {
                    let body = next_body()?;
                    slots.insert(
                        active.slot(),
                        at(
                            active.revision,
                            Some(EffectiveBody {
                                content_sha256: active.content_sha256.clone(),
                                body,
                            }),
                        ),
                    );
                }
                self.slots = slots;
            }
        }
        self.last_entry_sequence = identity.entry_sequence;
        self.last_target_generation = identity.target_generation;
        Ok(())
    }
}

/// `session_id`'s effective set, derived from its own entry rows in
/// `messages`. Every entry row is checked as
/// [`validate_operator_instruction_messages`] checks it.
pub fn effective_operator_instructions(
    session_id: &SessionId,
    messages: &[Message],
) -> Result<EffectiveOperatorInstructions, String> {
    effective_set(session_id, messages, BodyRead::Verify)
}

fn effective_set(
    session_id: &SessionId,
    messages: &[Message],
    read: BodyRead,
) -> Result<EffectiveOperatorInstructions, String> {
    let mut chains: BTreeMap<SessionId, OriginChain> = BTreeMap::new();
    let mut effective = EffectiveOperatorInstructions::default();
    for (index, system, identity) in entry_rows(messages) {
        let own = &identity.origin_session_id == session_id;
        let bodies = match read {
            BodyRead::Verify => {
                validate_identity_shape(identity)?;
                entry_bodies(identity, &system.content)?
            }
            BodyRead::Verified if own => walk_entry_bodies(identity, &system.content, read)?,
            // A copied row adds no body to this session's set.
            BodyRead::Verified => Vec::new(),
        };
        chains
            .entry(identity.origin_session_id.clone())
            .or_default()
            .apply(identity)?;
        if own {
            effective.apply(identity, &bodies, index)?;
        }
    }
    Ok(effective)
}

/// The facts an entry carries besides its changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperatorInstructionEntryHeader {
    pub(crate) origin_session_id: SessionId,
    pub(crate) entry_sequence: u64,
    pub(crate) target_generation: u64,
    pub(crate) run_id: Option<RunId>,
}

/// One active instruction with its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveOperatorInstruction {
    pub(crate) slot: InstructionSlot,
    pub(crate) revision: InstructionRevision,
    pub(crate) body: String,
}

impl ActiveOperatorInstruction {
    fn active_ref(&self) -> InstructionActiveRef {
        InstructionActiveRef {
            namespace: self.slot.namespace.clone(),
            key: self.slot.key.clone(),
            revision: self.revision,
            content_sha256: InstructionContentDigest::for_body(&self.body),
            body_bytes: self.body.len() as u64,
        }
    }
}

/// One change of a delta, with the body an activation carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperatorInstructionChangeContent {
    Activate {
        slot: InstructionSlot,
        revision: InstructionRevision,
        supersedes: Option<InstructionRevision>,
        body: String,
    },
    Revoke {
        slot: InstructionSlot,
        revision: InstructionRevision,
        revokes: InstructionRevision,
    },
}

impl OperatorInstructionChangeContent {
    fn slot(&self) -> &InstructionSlot {
        match self {
            Self::Activate { slot, .. } | Self::Revoke { slot, .. } => slot,
        }
    }

    fn change(&self) -> InstructionChange {
        match self {
            Self::Activate {
                slot,
                revision,
                supersedes,
                body,
            } => InstructionChange::Activate {
                namespace: slot.namespace.clone(),
                key: slot.key.clone(),
                revision: *revision,
                supersedes: *supersedes,
                content_sha256: InstructionContentDigest::for_body(body),
                body_bytes: body.len() as u64,
            },
            Self::Revoke {
                slot,
                revision,
                revokes,
            } => InstructionChange::Revoke {
                namespace: slot.namespace.clone(),
                key: slot.key.clone(),
                revision: *revision,
                revokes: *revokes,
            },
        }
    }
}

fn entry_row(
    identity: InstructionEntryIdentity,
    content: String,
    created_at: MessageTimestamp,
) -> SystemMessage {
    SystemMessage {
        content,
        created_at,
        identity: None,
        prompt_version: None,
        instruction_activation: None,
        instruction_entry: Some(identity),
    }
}

/// Mint one delta entry: `changes` and the whole `active_after` set, in any
/// order (both are sorted by slot here).
pub(crate) fn mint_operator_instruction_delta(
    header: OperatorInstructionEntryHeader,
    mut active_after: Vec<ActiveOperatorInstruction>,
    mut changes: Vec<OperatorInstructionChangeContent>,
    created_at: MessageTimestamp,
) -> Result<SystemMessage, String> {
    active_after.sort_by(|left, right| left.slot.cmp(&right.slot));
    changes.sort_by(|left, right| left.slot().cmp(right.slot()));
    let identity = InstructionEntryIdentity {
        render_version: INSTRUCTION_ENTRY_RENDER_VERSION_V1,
        origin_session_id: header.origin_session_id,
        entry_sequence: header.entry_sequence,
        kind: InstructionEntryKind::Delta,
        target_generation: header.target_generation,
        run_id: header.run_id,
        active: active_after
            .iter()
            .map(ActiveOperatorInstruction::active_ref)
            .collect(),
        changes: changes
            .iter()
            .map(OperatorInstructionChangeContent::change)
            .collect(),
    };
    validate_identity_shape(&identity)?;
    let bodies = changes
        .iter()
        .filter_map(|change| match change {
            OperatorInstructionChangeContent::Activate { body, .. } => Some(body.as_str()),
            OperatorInstructionChangeContent::Revoke { .. } => None,
        })
        .collect::<Vec<_>>();
    let content = render_entry_v1(&identity, &bodies)?;
    Ok(entry_row(identity, content, created_at))
}

/// Mint one snapshot entry of `active`, in any order; none for an empty set.
pub(crate) fn mint_operator_instruction_snapshot(
    header: OperatorInstructionEntryHeader,
    mut active: Vec<ActiveOperatorInstruction>,
    created_at: MessageTimestamp,
) -> Result<Option<SystemMessage>, String> {
    if active.is_empty() {
        return Ok(None);
    }
    active.sort_by(|left, right| left.slot.cmp(&right.slot));
    let identity = InstructionEntryIdentity {
        render_version: INSTRUCTION_ENTRY_RENDER_VERSION_V1,
        origin_session_id: header.origin_session_id,
        entry_sequence: header.entry_sequence,
        kind: InstructionEntryKind::Snapshot,
        target_generation: header.target_generation,
        run_id: header.run_id,
        active: active
            .iter()
            .map(ActiveOperatorInstruction::active_ref)
            .collect(),
        changes: Vec::new(),
    };
    validate_identity_shape(&identity)?;
    let bodies = active
        .iter()
        .map(|instruction| instruction.body.as_str())
        .collect::<Vec<_>>();
    let content = render_entry_v1(&identity, &bodies)?;
    Ok(Some(entry_row(identity, content, created_at)))
}

impl Session {
    /// Append one minted operator-instruction entry at the transcript tail,
    /// refusing a row that does not hold the bytes its identity names or does
    /// not continue its origin's entry chronology. A session that holds an
    /// entry row records the instruction state, so the state key alone
    /// answers whether its document carries instruction facts: an append into
    /// a session that records none records an unowned one. A refusal changes
    /// nothing.
    pub(crate) fn append_operator_instruction_entry(
        &mut self,
        entry: SystemMessage,
    ) -> Result<(), String> {
        let Some(identity) = entry.instruction_entry.as_ref() else {
            return Err("an operator instruction entry row needs its typed identity".to_string());
        };
        if entry.identity.is_some()
            || entry.prompt_version.is_some()
            || entry.instruction_activation.is_some()
        {
            return Err(
                "System message semantic identities must be mutually exclusive".to_string(),
            );
        }
        self.verify_operator_instruction_rows(self.messages.len())?;
        validate_identity_shape(identity)?;
        entry_bodies(identity, &entry.content)?;
        let mut chains: BTreeMap<SessionId, OriginChain> = BTreeMap::new();
        for (_, _, existing) in entry_rows(self.messages()) {
            chains
                .entry(existing.origin_session_id.clone())
                .or_default()
                .apply(existing)?;
        }
        chains
            .entry(identity.origin_session_id.clone())
            .or_default()
            .apply(identity)?;
        if !self.carries_operator_instruction_facts() {
            self.record_unowned_instruction_state()
                .map_err(|error| error.to_string())?;
        }
        self.messages.push(Message::System(entry));
        let appended = self.messages.len();
        self.messages.record_verified_entry_rows(appended);
        self.mark_content_mutated(crate::time_compat::SystemTime::now());
        Ok(())
    }

    /// Verify the operator-instruction entry rows among the first `count`
    /// rows that this transcript buffer has not verified yet: each holds the
    /// bytes its typed identity names (shape, digests and the whole render).
    /// A buffer's rows change in place only through a non-append mutation,
    /// which drops what it recorded as verified, so every row is verified
    /// once rather than on every model request, run start or write.
    pub(crate) fn verify_operator_instruction_rows(&self, count: usize) -> Result<(), String> {
        let count = count.min(self.messages.len());
        let verified = self.messages.verified_entry_row_count();
        let Some(rows) = self.messages.get(verified..count) else {
            return Ok(());
        };
        for (_, system, identity) in entry_rows(rows) {
            validate_identity_shape(identity)?;
            entry_bodies(identity, &system.content)?;
        }
        self.messages.record_verified_entry_rows(count);
        Ok(())
    }

    /// This session's effective set, as [`effective_operator_instructions`]
    /// derives it from its own entry rows. Each row's bytes are verified once
    /// per transcript buffer; a later read locates the bodies at their typed
    /// lengths without re-rendering or re-hashing them.
    pub fn effective_operator_instruction_set(
        &self,
    ) -> Result<EffectiveOperatorInstructions, String> {
        self.verify_operator_instruction_rows(self.messages.len())?;
        effective_set(self.id(), self.messages(), BodyRead::Verified)
    }

    /// The model boundary projection ([`Self::messages_for_model_boundary`])
    /// for a model that carries operator-instruction rows as `lowering`
    /// says. A row a fork copied is history the session never applies; where
    /// the model cannot lower it (after the leading System prefix of a model
    /// that lowers System rows only there, anywhere on a wire that carries
    /// no System row besides its prompt) the request leaves it out, and the
    /// transcript keeps it. The session's own rows are always lowerable: a
    /// run start and a compaction fold place them for the lowering in force.
    pub fn messages_for_model_request(
        &self,
        lowering: InstructionLowering,
    ) -> Result<Vec<Message>, ModelBoundaryProjectionError> {
        let mut messages = self.messages_for_model_boundary()?;
        retain_lowerable_inherited_entry_rows(self.id(), &mut messages, lowering);
        Ok(messages)
    }

    /// Where the operator-instruction entry rows this session wrote itself
    /// stand in its model requests ([`Self::messages_for_model_boundary`]).
    /// Every request carries them there, whatever the lowering
    /// ([`Self::messages_for_model_request`] leaves out only rows a fork
    /// copied), so a model change must carry them there
    /// ([`InstructionEntryPlacement::refusal`]). A copied row is neither the
    /// session's own nor another System row: a request on a wire that
    /// carries no System row besides its prompt leaves it out. A session
    /// without instruction facts holds no entry row.
    ///
    /// # Errors
    ///
    /// [`ModelBoundaryProjectionError`] when the transcript cannot be
    /// projected for a model request.
    pub fn own_operator_instruction_entry_placement(
        &self,
    ) -> Result<InstructionEntryPlacement, ModelBoundaryProjectionError> {
        if !self.carries_operator_instruction_facts() {
            return Ok(InstructionEntryPlacement::default());
        }
        let projection = self.messages_for_model_boundary()?;
        Ok(InstructionEntryPlacement::of(&projection, |identity| {
            identity.origin_session_id == self.id
        }))
    }

    /// A fork child that copied operator-instruction entry rows records the
    /// instruction state of a session without a target of its own (unowned,
    /// nothing pending), so the state key alone keeps answering whether its
    /// document carries instruction facts. `source_records_state` is whether
    /// the source records the key: only such a source holds entry rows, so
    /// any other fork is not scanned.
    pub(super) fn record_copied_operator_instruction_rows(&mut self, source_records_state: bool) {
        if !source_records_state || !self.messages().iter().any(is_operator_instruction_entry) {
            return;
        }
        // A fork child records no state before this (the source's is
        // session authority, which a fork does not copy), so recording the
        // unowned state has nothing to refuse.
        if let Err(error) = self.record_unowned_instruction_state() {
            tracing::error!(
                session_id = %self.id(),
                %error,
                "a fork child could not record the instruction state of the operator-instruction \
                 entry rows it copied"
            );
        }
    }

    /// [`Self::apply_run_start_instructions`], and what an applied entry
    /// changed: the entry row and the recorded target before it. A run that
    /// never starts (a refusal or a veto before it, an interrupt before it
    /// began) hands the application back to
    /// [`Self::revert_run_start_instructions`], so no entry names a run that
    /// never ran and the changes stay pending. A refused application changes
    /// nothing, even after it appended its entry.
    pub fn apply_run_start_instructions_reversibly(
        &mut self,
        run_start: InstructionRunStart,
        lowering: InstructionLowering,
        run_id: Option<RunId>,
    ) -> Result<
        (
            RunStartInstructionOutcome,
            Option<RunStartInstructionApplication>,
        ),
        InstructionStateError,
    > {
        let entry_index = self.messages.len();
        let state_before = self.metadata.get(SESSION_INSTRUCTION_STATE_KEY).cloned();
        let outcome = match self.apply_run_start_instructions(run_start, lowering, run_id) {
            Ok(outcome) => outcome,
            Err(error) => {
                if self
                    .messages
                    .get(entry_index)
                    .is_some_and(is_operator_instruction_entry)
                {
                    self.messages.remove_row(entry_index);
                    if let Some(state_before) = state_before {
                        self.set_metadata_unchecked(SESSION_INSTRUCTION_STATE_KEY, state_before);
                    }
                }
                return Err(error);
            }
        };
        let application = match (outcome, state_before) {
            (RunStartInstructionOutcome::Applied { entry_sequence, .. }, Some(state_before)) => {
                Some(RunStartInstructionApplication {
                    entry_index,
                    entry_sequence,
                    state_before,
                })
            }
            _ => None,
        };
        Ok((outcome, application))
    }

    /// Take back the entry a run start applied for a run that never started:
    /// the entry row leaves the transcript and the recorded target is
    /// restored as it was before the application, so its changes are pending
    /// again. Rows appended after the entry stay. Refused, changing nothing,
    /// when the entry is no longer at its row or already committed.
    pub fn revert_run_start_instructions(
        &mut self,
        application: RunStartInstructionApplication,
    ) -> Result<(), String> {
        let RunStartInstructionApplication {
            entry_index,
            entry_sequence,
            state_before,
        } = application;
        let entry = match self.messages.get(entry_index) {
            Some(Message::System(system)) => system.instruction_entry.as_ref(),
            _ => None,
        };
        let names_the_entry = entry.is_some_and(|identity| {
            identity.kind == InstructionEntryKind::Delta
                && identity.entry_sequence == entry_sequence
                && identity.origin_session_id == self.id
        });
        if !names_the_entry {
            return Err(format!(
                "operator instruction entry {entry_sequence} is no longer at transcript row \
                 {entry_index}"
            ));
        }
        let index = u64::try_from(entry_index).unwrap_or(u64::MAX);
        if self
            .messages
            .committed_row_prefix_count()
            .is_some_and(|committed| committed > index)
        {
            return Err(format!(
                "operator instruction entry {entry_sequence} is already committed"
            ));
        }
        self.messages.remove_row(entry_index);
        self.set_metadata_unchecked(SESSION_INSTRUCTION_STATE_KEY, state_before);
        self.mark_content_mutated(crate::time_compat::SystemTime::now());
        Ok(())
    }
}

/// What a fresh run start's operator-instruction entry changed in its
/// session ([`Session::apply_run_start_instructions_reversibly`]): the entry
/// row's place and sequence, and the recorded target before it.
#[derive(Debug, Clone)]
pub struct RunStartInstructionApplication {
    entry_index: usize,
    entry_sequence: u64,
    state_before: serde_json::Value,
}

/// Take out of a model request the operator-instruction rows a fork copied
/// that the model cannot lower where they stand
/// ([`Session::messages_for_model_request`]).
fn retain_lowerable_inherited_entry_rows(
    session_id: &SessionId,
    messages: &mut Vec<Message>,
    lowering: InstructionLowering,
) {
    let copied = |message: &Message| {
        matches!(
            message,
            Message::System(system)
                if system
                    .instruction_entry
                    .as_ref()
                    .is_some_and(|identity| &identity.origin_session_id != session_id)
        )
    };
    match lowering {
        InstructionLowering::MidConversation => {}
        InstructionLowering::LeadingPrefixOnly => {
            let leading = messages
                .iter()
                .position(|message| !matches!(message, Message::System(_)))
                .unwrap_or(messages.len());
            let mut index = 0;
            messages.retain(|message| {
                let keep = index < leading || !copied(message);
                index += 1;
                keep
            });
        }
        InstructionLowering::Unavailable => messages.retain(|message| !copied(message)),
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Session {
    /// Test seam: append one delta entry that activates `body` at `revision`
    /// for (`namespace`, `key`), superseding the revision in effect, as a run
    /// start would. It records no target: like every entry append, it
    /// records an unowned instruction state when the session records none.
    /// Production code never calls it.
    #[doc(hidden)]
    pub fn append_operator_instruction_activation_for_test(
        &mut self,
        namespace: &str,
        key: &str,
        revision: u64,
        body: &str,
    ) -> Result<(), String> {
        let slot = InstructionSlot {
            namespace: InstructionNamespace::new(namespace).map_err(|error| error.to_string())?,
            key: InstructionKey::new(key).map_err(|error| error.to_string())?,
        };
        let revision = InstructionRevision::new(revision).map_err(|error| error.to_string())?;
        let effective = self.effective_operator_instruction_set()?;
        let supersedes = effective
            .slot(&slot)
            .filter(|state| state.effect() == InstructionEffect::Active)
            .map(EffectiveInstructionSlot::revision);
        let mut active_after = effective
            .slots()
            .filter(|(other, _)| **other != slot)
            .filter_map(|(other, state)| {
                state.body().map(|body| ActiveOperatorInstruction {
                    slot: other.clone(),
                    revision: state.revision(),
                    body: body.to_string(),
                })
            })
            .collect::<Vec<_>>();
        active_after.push(ActiveOperatorInstruction {
            slot: slot.clone(),
            revision,
            body: body.to_string(),
        });
        let sequence = effective.last_entry_sequence().saturating_add(1);
        let entry = mint_operator_instruction_delta(
            OperatorInstructionEntryHeader {
                origin_session_id: self.id().clone(),
                entry_sequence: sequence,
                target_generation: sequence,
                run_id: None,
            },
            active_after,
            vec![OperatorInstructionChangeContent::Activate {
                slot,
                revision,
                supersedes,
                body: body.to_string(),
            }],
            crate::types::message_timestamp_now(),
        )?;
        self.append_operator_instruction_entry(entry)
    }
}

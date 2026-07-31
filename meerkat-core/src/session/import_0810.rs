//! Explicit, one-time importer for the exact Meerkat 0.8.10 Session envelope.
//!
//! Released checkpoint and transcript-witness formats are deliberately absent
//! from ordinary [`Session`] deserialization. This module is the sole boundary
//! allowed to interpret them. A successful import validates the exact released
//! domain shape, strips every retired proof carrier as untrusted metadata, and
//! returns a domain Session plus a non-cloneable receipt that a store must
//! consume while atomically adopting the imported state under its own physical
//! authority.

use super::*;
use crate::types::SystemMessage;
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};

const RELEASED_SESSION_ENVELOPE_VERSION: u32 = 2;
const RELEASED_CHECKPOINT_STAMP_KEY: &str = "session_checkpoint_stamp_v1";
const RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY: &str = "session_runtime_checkpoint_provenance_v1";
const RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY: &str =
    "session_transcript_history_checkpoint_digest_v1";
const RELEASED_SYSTEM_CONTEXT_STATE_KEY: &str = "session_system_context_state";
const RELEASED_SYSTEM_CONTEXT_RENDER_LABEL: &str = "[Runtime System Context]";

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FrozenSystemContextState0810 {
    #[serde(default)]
    pending: Vec<FrozenSystemContextAppend0810>,
    #[serde(default)]
    applied: Vec<FrozenSystemContextAppend0810>,
    #[serde(default)]
    seen: BTreeMap<String, FrozenSeenSystemContextKey0810>,
    #[serde(default)]
    active_turn_pending_keys: BTreeSet<String>,
    #[serde(default)]
    active_turn_pending_indices: BTreeSet<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FrozenSystemContextAppend0810 {
    content: crate::lifecycle::run_primitive::CoreRenderable,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    source_kind: FrozenSystemContextSource0810,
    #[serde(default)]
    peer_response_terminal: Option<crate::handles::PeerResponseTerminalFact>,
    accepted_at: SystemTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FrozenSeenSystemContextKey0810 {
    content: crate::lifecycle::run_primitive::CoreRenderable,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    source_kind: FrozenSystemContextSource0810,
    #[serde(default)]
    peer_response_terminal: Option<crate::handles::PeerResponseTerminalFact>,
    state: FrozenSeenSystemContextState0810,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FrozenSystemContextSource0810 {
    #[default]
    Normal,
    RuntimeSteer,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FrozenSeenSystemContextState0810 {
    Pending,
    Applied,
}

pub(crate) fn is_released_checkpoint_metadata_key(key: &str) -> bool {
    matches!(
        key,
        RELEASED_CHECKPOINT_STAMP_KEY
            | RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY
            | RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY
    )
}

pub(super) fn contains_released_checkpoint_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    metadata
        .keys()
        .any(|key| is_released_checkpoint_metadata_key(key))
}

/// Why the exact released document may be adopted.
///
/// Every 0.8.10 row requires store authorization. The backend must consume the
/// receipt in the same operation that proves the released physical store
/// schema, the exact source row/blob identity, and installs a store-issued
/// current authority. Envelope bytes and retired checkpoint metadata cannot
/// establish physical authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released0810ImportEvidence {
    StoreAuthorizationRequired,
}

/// Single-use evidence for one exact 0.8.10 import.
///
/// The type is intentionally not `Clone`. Reading its fields is sufficient to
/// compare it with a backend-issued source authority, but adoption must take
/// ownership of the receipt so no second store transition can reuse it.
#[derive(Debug)]
#[must_use = "released import receipts must be consumed by one store-adoption transaction"]
pub struct Released0810ImportReceipt {
    session_id: SessionId,
    source_document_sha256: [u8; 32],
    evidence: Released0810ImportEvidence,
}

impl Released0810ImportReceipt {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn source_document_sha256(&self) -> &[u8; 32] {
        &self.source_document_sha256
    }

    #[must_use]
    pub const fn evidence(&self) -> Released0810ImportEvidence {
        self.evidence
    }
}

/// Domain state and the one receipt authorizing its store adoption.
#[derive(Debug)]
#[must_use = "released imported state must be adopted through its single-use receipt"]
pub struct ImportedReleased0810Session {
    session: Session,
    receipt: Released0810ImportReceipt,
}

impl ImportedReleased0810Session {
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn receipt(&self) -> &Released0810ImportReceipt {
        &self.receipt
    }

    /// Split the imported domain state from the single-use adoption receipt.
    pub fn into_parts(self) -> (Session, Released0810ImportReceipt) {
        (self.session, self.receipt)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Released0810ImportError {
    #[error("released 0.8.10 session document is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error(
        "released importer accepts exact session envelope version {expected}, observed {observed}"
    )]
    EnvelopeVersion { expected: u32, observed: u32 },
    #[error("released importer refuses current transcript-history wire")]
    CurrentTranscriptHistory,
    #[error("released importer refuses current-only metadata `{0}`")]
    CurrentMetadata(&'static str),
    #[error("released transcript-history import failed: {0}")]
    TranscriptHistory(String),
    #[error("released System-context timestamp is outside the supported UTC range")]
    SystemContextTimestamp,
    #[error("released 0.8.10 transcript row is malformed: {0}")]
    TranscriptRow(String),
}

/// Frozen 0.8.10 System-message provenance.
///
/// This private wire is intentionally copied from the release tag instead of
/// interpreting old rows through the current `SystemMessage`: 0.8.11 replaced
/// `mutation_kind` with a different identity carrier.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReleasedSystemPromptMutationKind0810 {
    #[default]
    Unspecified,
    DirectMutation,
    ExplicitBuild,
    DefaultBuild,
    WasmDefaultBuild,
    RuntimeContextAppend,
    RuntimeSteerCleanup,
}

impl ReleasedSystemPromptMutationKind0810 {
    const fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ReleasedSystemMessageRow0810 {
    role: String,
    content: String,
    #[serde(default)]
    mutation_kind: ReleasedSystemPromptMutationKind0810,
    created_at: crate::types::MessageTimestamp,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ReleasedSystemMessageDigest0810<'a> {
    role: &'static str,
    content: &'a str,
    #[serde(
        default,
        skip_serializing_if = "ReleasedSystemPromptMutationKind0810::is_unspecified"
    )]
    mutation_kind: ReleasedSystemPromptMutationKind0810,
    created_at: crate::types::MessageTimestamp,
}

type ReleasedRawObject0810 = BTreeMap<String, Box<RawValue>>;

fn malformed_released_transcript_row(detail: impl std::fmt::Display) -> Released0810ImportError {
    Released0810ImportError::TranscriptRow(detail.to_string())
}

fn released_raw_object_0810(raw: &[u8]) -> Result<ReleasedRawObject0810, Released0810ImportError> {
    serde_json::from_slice(raw).map_err(malformed_released_transcript_row)
}

fn released_raw_object_value_0810(
    raw: &RawValue,
) -> Result<ReleasedRawObject0810, Released0810ImportError> {
    serde_json::from_str(raw.get()).map_err(malformed_released_transcript_row)
}

fn released_raw_array_0810(raw: &RawValue) -> Result<Vec<Box<RawValue>>, Released0810ImportError> {
    serde_json::from_str(raw.get()).map_err(malformed_released_transcript_row)
}

fn released_raw_field_0810<'a>(
    object: &'a ReleasedRawObject0810,
    field: &str,
) -> Result<&'a RawValue, Released0810ImportError> {
    object.get(field).map(Box::as_ref).ok_or_else(|| {
        malformed_released_transcript_row(format!("message has no required '{field}' field"))
    })
}

fn released_raw_string_0810(
    object: &ReleasedRawObject0810,
    field: &str,
) -> Result<String, Released0810ImportError> {
    serde_json::from_str(released_raw_field_0810(object, field)?.get())
        .map_err(malformed_released_transcript_row)
}

fn collect_released_content_raw_slots_0810(
    content: &RawValue,
    slots: &mut Vec<String>,
) -> Result<(), Released0810ImportError> {
    if !content.get().trim_start().starts_with('[') {
        return Ok(());
    }
    for block in released_raw_array_0810(content)? {
        let block = released_raw_object_value_0810(&block)?;
        if released_raw_string_0810(&block, "type")? == "structured" {
            slots.push(released_raw_field_0810(&block, "data")?.get().to_string());
        }
    }
    Ok(())
}

/// Collect the two RawValue-bearing durable message slots in exactly the same
/// depth-first order as the released message serializer.
fn collect_released_message_raw_slots_0810(
    row: &[u8],
) -> Result<Vec<String>, Released0810ImportError> {
    let message = released_raw_object_0810(row)?;
    let role = released_raw_string_0810(&message, "role")?;
    let mut slots = Vec::new();
    match role.as_str() {
        "user" => collect_released_content_raw_slots_0810(
            released_raw_field_0810(&message, "content")?,
            &mut slots,
        )?,
        "block_assistant" => {
            for block in released_raw_array_0810(released_raw_field_0810(&message, "blocks")?)? {
                let block = released_raw_object_value_0810(&block)?;
                if released_raw_string_0810(&block, "block_type")? != "tool_use" {
                    continue;
                }
                let data =
                    released_raw_object_value_0810(released_raw_field_0810(&block, "data")?)?;
                slots.push(released_raw_field_0810(&data, "args")?.get().to_string());
            }
        }
        "tool_results" => {
            for result in released_raw_array_0810(released_raw_field_0810(&message, "results")?)? {
                let result = released_raw_object_value_0810(&result)?;
                collect_released_content_raw_slots_0810(
                    released_raw_field_0810(&result, "content")?,
                    &mut slots,
                )?;
            }
        }
        "system_notice" => {
            let Some(blocks) = message.get("blocks") else {
                return Ok(slots);
            };
            for block in released_raw_array_0810(blocks)? {
                let block = released_raw_object_value_0810(&block)?;
                if matches!(
                    released_raw_string_0810(&block, "type")?.as_str(),
                    "comms" | "external_event"
                ) && let Some(content) = block.get("content")
                {
                    collect_released_content_raw_slots_0810(content, &mut slots)?;
                }
            }
        }
        "system" => {}
        other => {
            return Err(malformed_released_transcript_row(format!(
                "unsupported released message role '{other}'"
            )));
        }
    }
    Ok(slots)
}

fn visit_released_content_raw_slots_0810(
    blocks: &mut [crate::types::ContentBlock],
    replace: &mut impl FnMut(&mut Box<RawValue>) -> Result<(), Released0810ImportError>,
) -> Result<(), Released0810ImportError> {
    for block in blocks {
        if let crate::types::ContentBlock::Structured { data } = block {
            replace(data)?;
        }
    }
    Ok(())
}

fn replace_released_message_raw_slots_0810(
    message: &mut Message,
    sentinels: &[String],
) -> Result<(), Released0810ImportError> {
    let mut index = 0usize;
    let mut replace = |raw: &mut Box<RawValue>| {
        let sentinel = sentinels.get(index).ok_or_else(|| {
            malformed_released_transcript_row(
                "decoded message exposes more RawValue slots than its exact row",
            )
        })?;
        *raw =
            RawValue::from_string(sentinel.clone()).map_err(malformed_released_transcript_row)?;
        index += 1;
        Ok(())
    };
    match message {
        Message::User(user) => {
            visit_released_content_raw_slots_0810(&mut user.content, &mut replace)?;
        }
        Message::BlockAssistant(assistant) => {
            for block in &mut assistant.blocks {
                if let crate::types::AssistantBlock::ToolUse { args, .. } = block {
                    replace(args)?;
                }
            }
        }
        Message::ToolResults { results, .. } => {
            for result in results {
                visit_released_content_raw_slots_0810(&mut result.content, &mut replace)?;
            }
        }
        Message::SystemNotice(notice) => {
            for block in &mut notice.blocks {
                match block {
                    crate::types::SystemNoticeBlock::Comms { content, .. }
                    | crate::types::SystemNoticeBlock::ExternalEvent { content, .. } => {
                        visit_released_content_raw_slots_0810(content, &mut replace)?;
                    }
                    _ => {}
                }
            }
        }
        Message::System(_) => {}
    }
    if index != sentinels.len() {
        return Err(malformed_released_transcript_row(format!(
            "exact row carries {} RawValue slots but the decoded message exposes {index}",
            sentinels.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static RELEASED_0810_RAW_SCAN_STEPS: std::cell::Cell<u64> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn reset_released_0810_raw_scan_steps() {
    RELEASED_0810_RAW_SCAN_STEPS.set(0);
}

#[cfg(test)]
fn released_0810_raw_scan_steps() -> u64 {
    RELEASED_0810_RAW_SCAN_STEPS.get()
}

fn record_released_0810_raw_scan_steps(steps: usize) {
    #[cfg(test)]
    RELEASED_0810_RAW_SCAN_STEPS.set(
        RELEASED_0810_RAW_SCAN_STEPS
            .get()
            .saturating_add(u64::try_from(steps).unwrap_or(u64::MAX)),
    );
    #[cfg(not(test))]
    let _ = steps;
}

fn find_released_0810_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    debug_assert!(!needle.is_empty());
    if needle.len() > haystack.len() {
        return None;
    }
    for index in 0..=haystack.len() - needle.len() {
        record_released_0810_raw_scan_steps(1);
        if haystack[index..].starts_with(needle) {
            return Some(index);
        }
    }
    None
}

fn hex_released_0810_digest(digest: [u8; 32]) -> String {
    let mut hex = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        hex.push(HEX[(byte >> 4) as usize] as char);
        hex.push(HEX[(byte & 0x0f) as usize] as char);
    }
    hex
}

/// Choose one document-specific namespace, then prove in one pass that no
/// source row already contains it. Every slot marker derives from this prefix,
/// so no per-slot source scan is necessary.
fn released_raw_sentinel_namespace_0810(
    source_rows: &[Vec<u8>],
) -> Result<String, Released0810ImportError> {
    let mut seed = Vec::with_capacity(source_rows.len().saturating_mul(40));
    for row in source_rows {
        let len = u64::try_from(row.len()).map_err(|_| {
            malformed_released_transcript_row("released message row length exceeds u64")
        })?;
        seed.extend_from_slice(&len.to_be_bytes());
        seed.extend_from_slice(&sha256_key(row));
    }
    let namespace = format!(
        "\"__meerkat_released_0810_raw_{}_slot_",
        hex_released_0810_digest(sha256_key(&seed))
    );
    if source_rows
        .iter()
        .any(|row| find_released_0810_subslice(row, namespace.as_bytes()).is_some())
    {
        return Err(malformed_released_transcript_row(
            "released row collides with its deterministic RawValue sentinel namespace",
        ));
    }
    Ok(namespace)
}

fn released_raw_sentinel_0810(namespace: &str, slot_index: usize) -> String {
    format!("{namespace}{slot_index}__\"")
}

/// Restore every exact RawValue in one ordered scan over the serialized
/// message projection.
///
/// The namespace cannot occur in source rows. A normalized non-Raw field could
/// still manufacture it (for example through an escaped string spelling), so
/// the pass also fails closed unless the observed markers are exactly the
/// expected unique sequence.
fn restore_released_raw_slots_0810(
    bytes: &[u8],
    namespace: &str,
    sentinels: &[String],
    exact_raw_slots: &[String],
) -> Result<Vec<u8>, Released0810ImportError> {
    if sentinels.len() != exact_raw_slots.len() {
        return Err(malformed_released_transcript_row(
            "RawValue sentinel and source slot counts differ",
        ));
    }
    let namespace = namespace.as_bytes();
    let mut restored = Vec::with_capacity(bytes.len());
    let mut cursor = 0usize;
    let mut slot_index = 0usize;
    while let Some(relative) = find_released_0810_subslice(&bytes[cursor..], namespace) {
        let offset = cursor.checked_add(relative).ok_or_else(|| {
            malformed_released_transcript_row("RawValue sentinel offset overflowed")
        })?;
        restored.extend_from_slice(&bytes[cursor..offset]);
        let expected = sentinels.get(slot_index).ok_or_else(|| {
            malformed_released_transcript_row(
                "frozen digest projection contains an unexpected extra RawValue sentinel",
            )
        })?;
        if !bytes[offset..].starts_with(expected.as_bytes()) {
            return Err(malformed_released_transcript_row(format!(
                "frozen digest projection contains an out-of-order RawValue sentinel at slot {slot_index}"
            )));
        }
        restored.extend_from_slice(exact_raw_slots[slot_index].as_bytes());
        cursor = offset
            .checked_add(expected.len())
            .ok_or_else(|| malformed_released_transcript_row("RawValue sentinel end overflowed"))?;
        slot_index += 1;
    }
    restored.extend_from_slice(&bytes[cursor..]);
    if slot_index != sentinels.len() {
        return Err(malformed_released_transcript_row(format!(
            "frozen digest projection restored {slot_index}/{} RawValue sentinels",
            sentinels.len()
        )));
    }
    Ok(restored)
}

fn released_system_digest_row_0810(row: &[u8]) -> Result<Vec<u8>, Released0810ImportError> {
    let released: ReleasedSystemMessageRow0810 =
        serde_json::from_slice(row).map_err(malformed_released_transcript_row)?;
    if released.role != "system" {
        return Err(malformed_released_transcript_row(format!(
            "frozen System decoder observed role '{}'",
            released.role
        )));
    }
    let _ = released.created_at;
    serde_json::to_vec(&ReleasedSystemMessageDigest0810 {
        role: "system",
        content: &released.content,
        mutation_kind: released.mutation_kind,
        created_at: chrono::DateTime::UNIX_EPOCH,
    })
    .map_err(malformed_released_transcript_row)
}

fn released_message_digest_row_0810(
    row: &[u8],
    namespace: &str,
    sentinels: &[String],
    exact_raw_slots: &[String],
) -> Result<Vec<u8>, Released0810ImportError> {
    let role = released_raw_string_0810(&released_raw_object_0810(row)?, "role")?;
    if role == "system" {
        if !sentinels.is_empty() || !exact_raw_slots.is_empty() {
            return Err(malformed_released_transcript_row(
                "released System row unexpectedly carries RawValue slots",
            ));
        }
        return released_system_digest_row_0810(row);
    }
    let mut message: Message =
        serde_json::from_slice(row).map_err(malformed_released_transcript_row)?;
    replace_released_message_raw_slots_0810(&mut message, sentinels)?;
    let [canonical] =
        super::canonicalize_released_0810_messages_for_digest(std::slice::from_ref(&message))
            .try_into()
            .map_err(|_| malformed_released_transcript_row("message projection is not singular"))?;
    let bytes = serde_json::to_vec(&canonical).map_err(malformed_released_transcript_row)?;
    restore_released_raw_slots_0810(&bytes, namespace, sentinels, exact_raw_slots)
}

/// Recompute one exact released-0.8.10 transcript digest from physical message
/// row bytes.
///
/// This is an importer-only O(document) verifier. It reproduces the released
/// format-2 `[` + comma-separated message JSON + `]` framing, including exact
/// producer spelling in the two durable `RawValue` slots. It must never be
/// called by current live-runtime persistence.
#[doc(hidden)]
pub fn released_0810_transcript_serialized_rows_digest(
    rows: &[Vec<u8>],
) -> Result<String, Released0810ImportError> {
    let raw_slots = rows
        .iter()
        .map(|row| collect_released_message_raw_slots_0810(row))
        .collect::<Result<Vec<_>, _>>()?;
    let namespace = released_raw_sentinel_namespace_0810(rows)?;
    let mut next_slot = 0usize;
    let mut row_sentinels = Vec::with_capacity(rows.len());
    for row_slots in &raw_slots {
        let mut sentinels = Vec::with_capacity(row_slots.len());
        for _ in row_slots {
            sentinels.push(released_raw_sentinel_0810(&namespace, next_slot));
            next_slot = next_slot.checked_add(1).ok_or_else(|| {
                malformed_released_transcript_row("released RawValue slot count overflowed")
            })?;
        }
        row_sentinels.push(sentinels);
    }

    let mut framed = Vec::new();
    framed.push(b'[');
    for (index, ((row, sentinels), exact_raw_slots)) in
        rows.iter().zip(&row_sentinels).zip(&raw_slots).enumerate()
    {
        if index > 0 {
            framed.push(b',');
        }
        framed.extend_from_slice(&released_message_digest_row_0810(
            row,
            &namespace,
            sentinels,
            exact_raw_slots,
        )?);
    }
    framed.push(b']');
    let hex = hex_released_0810_digest(sha256_key(&framed));
    Ok(format!("sha256:{hex}"))
}

/// Import one exact Meerkat 0.8.10 Session document.
///
/// This is intentionally not part of [`Deserialize`] or
/// [`Session::from_persisted_bytes`]. Current loaders never inspect a legacy
/// schema: backend activation first classifies an exact released physical row,
/// invokes this importer once, and consumes the returned receipt while
/// replacing that row with store-owned current authority.
pub fn import_released_0810_session(
    serialized: &[u8],
) -> Result<ImportedReleased0810Session, Released0810ImportError> {
    let source_document_sha256 = sha256_key(serialized);
    let mut deserializer = serde_json::Deserializer::from_slice(serialized);
    let serde_repr = SessionSerde::deserialize(&mut deserializer)?;
    deserializer.end()?;
    if serde_repr.version != RELEASED_SESSION_ENVELOPE_VERSION {
        return Err(Released0810ImportError::EnvelopeVersion {
            expected: RELEASED_SESSION_ENVELOPE_VERSION,
            observed: serde_repr.version,
        });
    }

    let mut session = released_session_from_serde(serde_repr)?;
    let history_kind = transcript_history_wire_kind(&session.metadata)
        .map_err(Released0810ImportError::TranscriptHistory)?;
    if matches!(history_kind, Some(TranscriptHistoryWireKind::Current)) {
        return Err(Released0810ImportError::CurrentTranscriptHistory);
    }
    if session
        .metadata
        .contains_key(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY)
    {
        return Err(Released0810ImportError::CurrentMetadata(
            SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY,
        ));
    }

    let imported_history = session
        .metadata
        .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        .cloned()
        .map(import_released_0810_history)
        .transpose()
        .map_err(|error| Released0810ImportError::TranscriptHistory(error.to_string()))?
        .flatten()
        .map(Arc::new);

    if let Some(history) = imported_history {
        install_imported_history(&mut session, history)?;
    } else if matches!(history_kind, Some(TranscriptHistoryWireKind::Released0810)) {
        // Every released occurrence collapsed to one current semantic body.
        // Retaining the predecessor full-body wire would make ordinary
        // current ingress reinterpret it, so the migration squash removes it.
        session
            .metadata
            .remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    }
    adopt_released_system_context_into_transcript(&mut session)?;
    strip_released_checkpoint_metadata(&mut session);
    session.version = SESSION_VERSION;

    let session_id = session.id().clone();
    Ok(ImportedReleased0810Session {
        session,
        receipt: Released0810ImportReceipt {
            session_id,
            source_document_sha256,
            evidence: Released0810ImportEvidence::StoreAuthorizationRequired,
        },
    })
}

/// Retire the 0.8.10 out-of-band prompt projection while the frozen envelope
/// is being imported but before current store authority is installed.
///
/// The released shape did not retain an original transcript position for
/// applied prompt context. Its only honest ordered conversion point is this
/// adoption boundary. Every entry remains distinct and ordered; no System
/// content is coalesced, replaced, or normalized.
fn adopt_released_system_context_into_transcript(
    session: &mut Session,
) -> Result<(), Released0810ImportError> {
    let Some(encoded) = session.metadata.remove(RELEASED_SYSTEM_CONTEXT_STATE_KEY) else {
        return Ok(());
    };
    let state: FrozenSystemContextState0810 = serde_json::from_value(encoded)?;
    validate_released_system_context_state_0810(&state)?;

    // The released state partition is itself the ordering witness: every
    // applied entry crossed an earlier model boundary, while `pending`
    // contains only entries accepted after the last such boundary. Each
    // partition preserves admission order. Concatenating applied then pending
    // therefore reconstructs the only ordering the 0.8.10 representation
    // retained, without sorting by a wall clock that may move backwards.
    let appends = state.applied.into_iter().chain(state.pending);
    for append in appends {
        // A runtime steer is owned by its pending RuntimeStore input and is
        // re-armed from that input on recovery. The Session copy was never
        // durable conversation data.
        if append.source_kind == FrozenSystemContextSource0810::RuntimeSteer {
            continue;
        }

        if append.peer_response_terminal.is_some() {
            let mut notice = append.content.into_system_notice_message();
            notice.created_at = released_message_timestamp_0810(append.accepted_at)?;
            let already_present = session.messages().iter().any(|message| {
                matches!(
                    message,
                    Message::SystemNotice(existing)
                        if existing.kind == notice.kind
                            && existing.body == notice.body
                            && existing.blocks == notice.blocks
                )
            });
            if !already_present {
                session.push(Message::SystemNotice(notice));
            }
            continue;
        }

        if matches!(
            &append.content,
            crate::lifecycle::run_primitive::CoreRenderable::SystemNotice { .. }
        ) {
            let mut notice = append.content.into_system_notice_message();
            notice.created_at = released_message_timestamp_0810(append.accepted_at)?;
            session.push(Message::SystemNotice(notice));
            continue;
        }

        let rendered =
            render_released_system_context_block_0810(&append.content, append.source.as_deref());
        // Import never applies NEW ingress idempotency semantics to already
        // accepted 0.8.10 facts. Every stored entry becomes one distinct
        // ordinary System row, even when content or identity repeats.
        session.push(Message::System(SystemMessage::with_identity_at(
            rendered,
            append.source,
            append.idempotency_key,
            released_message_timestamp_0810(append.accepted_at)?,
        )));
    }

    // These were auxiliary indices for the retired sidecar. Deserializing
    // them above makes unknown released shapes fail closed; successful
    // adoption intentionally carries none of them into current Session state.
    let _ = (
        state.seen,
        state.active_turn_pending_keys,
        state.active_turn_pending_indices,
    );
    Ok(())
}

fn released_message_timestamp_0810(
    timestamp: SystemTime,
) -> Result<crate::types::MessageTimestamp, Released0810ImportError> {
    let elapsed = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| Released0810ImportError::SystemContextTimestamp)?;
    let seconds = i64::try_from(elapsed.as_secs())
        .map_err(|_| Released0810ImportError::SystemContextTimestamp)?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, elapsed.subsec_nanos())
        .ok_or(Released0810ImportError::SystemContextTimestamp)
}

fn validate_released_system_context_state_0810(
    state: &FrozenSystemContextState0810,
) -> Result<(), Released0810ImportError> {
    let invalid = |message: String| {
        Released0810ImportError::Malformed(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )))
    };
    let mut keyed: BTreeMap<
        String,
        (
            FrozenSeenSystemContextState0810,
            &FrozenSystemContextAppend0810,
        ),
    > = BTreeMap::new();
    for (lifecycle, appends) in [
        (
            FrozenSeenSystemContextState0810::Applied,
            state.applied.as_slice(),
        ),
        (
            FrozenSeenSystemContextState0810::Pending,
            state.pending.as_slice(),
        ),
    ] {
        for append in appends {
            let Some(key) = append.idempotency_key.as_ref() else {
                continue;
            };
            if keyed.insert(key.clone(), (lifecycle, append)).is_some() {
                return Err(invalid(format!(
                    "released 0.8.10 system-context state repeats idempotency key `{key}`"
                )));
            }
        }
    }
    if keyed.len() != state.seen.len() {
        return Err(invalid(
            "released 0.8.10 system-context seen index does not cover the exact keyed entries"
                .to_string(),
        ));
    }
    for (key, seen) in &state.seen {
        let Some((lifecycle, append)) = keyed.get(key) else {
            return Err(invalid(format!(
                "released 0.8.10 system-context seen key `{key}` has no stored append"
            )));
        };
        if *lifecycle != seen.state
            || append.content != seen.content
            || append.source != seen.source
            || append.source_kind != seen.source_kind
            || append.peer_response_terminal != seen.peer_response_terminal
        {
            return Err(invalid(format!(
                "released 0.8.10 system-context seen key `{key}` contradicts its stored append"
            )));
        }
    }
    if state
        .active_turn_pending_indices
        .iter()
        .any(|index| usize::try_from(*index).map_or(true, |index| index >= state.pending.len()))
    {
        return Err(invalid(
            "released 0.8.10 active-turn pending index is out of bounds".to_string(),
        ));
    }
    for key in &state.active_turn_pending_keys {
        let Some((FrozenSeenSystemContextState0810::Pending, _)) = keyed.get(key) else {
            return Err(invalid(format!(
                "released 0.8.10 active-turn key `{key}` is not pending"
            )));
        };
    }
    Ok(())
}

fn render_released_system_context_block_0810(
    content: &crate::lifecycle::run_primitive::CoreRenderable,
    source: Option<&str>,
) -> String {
    let mut rendered = String::from(RELEASED_SYSTEM_CONTEXT_RENDER_LABEL);
    if let Some(source) = source {
        rendered.push_str("\nsource: ");
        rendered.push_str(source);
    }
    rendered.push_str("\n\n");
    rendered.push_str(content.render_text().trim());
    rendered
}

fn released_session_from_serde(
    serde_repr: SessionSerde,
) -> Result<Session, Released0810ImportError> {
    let mut metadata = serde_repr.metadata;
    let realtime_transcript = match metadata.remove(SESSION_REALTIME_TRANSCRIPT_STATE_KEY) {
        Some(value) => {
            let state = serde_json::from_value(value)?;
            SessionRealtimeTranscriptProjection::from_inline_snapshot(&serde_repr.id, state)
                .map_err(|error| {
                    Released0810ImportError::Malformed(serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error.to_string(),
                    )))
                })?
        }
        None => SessionRealtimeTranscriptProjection::empty(&serde_repr.id),
    };
    Ok(Session {
        version: RELEASED_SESSION_ENVELOPE_VERSION,
        id: serde_repr.id,
        messages: TranscriptMessages::from_vec(serde_repr.messages),
        created_at: serde_repr.created_at,
        updated_at: serde_repr.updated_at,
        metadata,
        realtime_transcript: Box::new(realtime_transcript),
        history_caches: Box::default(),
        transcript_history_metadata_validation:
            TranscriptHistoryMetadataValidation::RequiresValidation,
        usage: serde_repr.usage,
    })
}

fn install_imported_history(
    session: &mut Session,
    history: Arc<TranscriptHistoryState>,
) -> Result<(), Released0810ImportError> {
    let exact_live_prefix = history
        .derive_live_row_lineage_after_final_semantic_replay(session.messages())
        .map_err(|error| Released0810ImportError::TranscriptHistory(error.to_string()))?
        .ok_or_else(|| {
            Released0810ImportError::TranscriptHistory(
                "live transcript does not preserve the released audited endpoint".to_string(),
            )
        })?;
    let endpoint_prefix = history
        .final_endpoint_witness()
        .ok_or_else(|| {
            Released0810ImportError::TranscriptHistory(
                "imported graph has no final endpoint witness".to_string(),
            )
        })?
        .row_prefix()
        .clone();
    if !session.install_exact_message_row_lineage(endpoint_prefix, exact_live_prefix) {
        return Err(Released0810ImportError::TranscriptHistory(
            "failed to install imported message-row lineage".to_string(),
        ));
    }
    session
        .metadata
        .remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    session
        .metadata
        .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    session
        .history_caches
        .shared_state
        .set(Arc::clone(&history));
    session.transcript_history_metadata_validation = TranscriptHistoryMetadataValidation::Validated;
    Ok(())
}

fn strip_released_checkpoint_metadata(session: &mut Session) {
    session.metadata.remove(RELEASED_CHECKPOINT_STAMP_KEY);
    session
        .metadata
        .remove(RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY);
    session
        .metadata
        .remove(RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY);
    session
        .metadata
        .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    if session.history_caches.shared_state.get().is_none() {
        session.transcript_history_metadata_validation =
            TranscriptHistoryMetadataValidation::Validated;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn frozen_append(text: &str, source: Option<&str>, source_kind: &str) -> serde_json::Value {
        serde_json::json!({
            "content": {
                "type": "text",
                "text": text,
            },
            "source": source,
            "source_kind": source_kind,
            "accepted_at": SystemTime::now(),
        })
    }

    #[test]
    fn released_sidecar_adoption_preserves_existing_and_distinct_system_rows() {
        let mut session = Session::new();
        session.append_system_message("existing");
        session.set_metadata_unchecked_for_test(
            RELEASED_SYSTEM_CONTEXT_STATE_KEY,
            serde_json::json!({
                "applied": [
                    frozen_append("  duplicate  ", Some("first"), "normal"),
                    frozen_append("  duplicate  ", Some("first"), "normal"),
                ],
                "pending": [
                    frozen_append("   ", None, "normal"),
                    frozen_append("never durable", Some("steer"), "runtime_steer"),
                ],
            }),
        );

        adopt_released_system_context_into_transcript(&mut session)
            .expect("frozen sidecar should adopt");

        let systems = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(systems.len(), 4);
        assert_eq!(systems[0].content, "existing");
        assert_eq!(
            systems[1].content,
            "[Runtime System Context]\nsource: first\n\nduplicate"
        );
        assert_eq!(systems[2].content, systems[1].content);
        assert_eq!(systems[3].content, "[Runtime System Context]\n\n");
        assert_eq!(
            systems[1]
                .identity
                .as_ref()
                .and_then(|identity| identity.source.as_deref()),
            Some("first")
        );
        assert!(
            !session
                .metadata()
                .contains_key(RELEASED_SYSTEM_CONTEXT_STATE_KEY)
        );
    }

    #[test]
    fn released_keyed_sidecar_adoption_embeds_exact_cross_store_identity() {
        let accepted_at = serde_json::to_value(SystemTime::now()).expect("time");
        let content = serde_json::json!({
            "type": "text",
            "text": "  shared instruction  ",
        });
        let mut session = Session::new();
        session.set_metadata_unchecked_for_test(
            RELEASED_SYSTEM_CONTEXT_STATE_KEY,
            serde_json::json!({
                "applied": [{
                    "content": content,
                    "source": "shared-key",
                    "idempotency_key": "shared-key",
                    "accepted_at": accepted_at,
                }],
                "seen": {
                    "shared-key": {
                        "content": content,
                        "source": "shared-key",
                        "state": "applied",
                    },
                },
            }),
        );

        adopt_released_system_context_into_transcript(&mut session)
            .expect("keyed frozen sidecar should adopt");

        let Message::System(system) = &session.messages()[0] else {
            panic!("adopted row must be System");
        };
        assert_eq!(
            system.content,
            "[Runtime System Context]\nsource: shared-key\n\nshared instruction"
        );
        assert_eq!(
            system.identity.as_ref(),
            Some(&crate::types::SystemMessageIdentity {
                source: Some("shared-key".to_string()),
                idempotency_key: Some("shared-key".to_string()),
            })
        );
    }

    fn digest_label(bytes: &[u8]) -> String {
        let digest = sha256_key(bytes);
        let mut hex = String::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in digest {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0x0f) as usize] as char);
        }
        format!("sha256:{hex}")
    }

    #[test]
    fn exact_released_row_digest_preserves_every_raw_slot_and_system_wire() {
        let rows = [
            br#"{"role":"system","content":"ordered","mutation_kind":"runtime_context_append","created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
            br#"{"role":"user","content":[{"type":"structured","data":{"z":1,"a":2}},{"type":"structured","data":[3, 2, 1]}],"created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
            br#"{"role":"block_assistant","blocks":[{"block_type":"tool_use","data":{"id":"call-1","name":"one","args":{"q":1, "a":2}}},{"block_type":"tool_use","data":{"id":"call-2","name":"two","args":[{"z":3,"a":4}]}}],"stop_reason":"tool_use","created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
            br#"{"role":"tool_results","results":[{"tool_use_id":"call-1","content":[{"type":"structured","data":{"result":true, "alpha":0}}],"is_error":false}],"created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
            br#"{"role":"system_notice","kind":"external_event","blocks":[{"type":"external_event","source":"fixture","event_type":"changed","content":[{"type":"structured","data":{"notice":2, "a":1}}]}],"created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
            br#"{"role":"system_notice","kind":"generic","body":"no blocks","created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
        ];
        let expected = br#"[{"role":"system","content":"ordered","mutation_kind":"runtime_context_append","created_at":"1970-01-01T00:00:00Z"},{"role":"user","content":[{"type":"structured","data":{"z":1,"a":2}},{"type":"structured","data":[3, 2, 1]}],"created_at":"1970-01-01T00:00:00Z"},{"role":"block_assistant","blocks":[{"block_type":"tool_use","data":{"id":"call-1","name":"one","args":{"q":1, "a":2}}},{"block_type":"tool_use","data":{"id":"call-2","name":"two","args":[{"z":3,"a":4}]}}],"stop_reason":"tool_use","created_at":"1970-01-01T00:00:00Z"},{"role":"tool_results","results":[{"tool_use_id":"call-1","content":[{"type":"structured","data":{"result":true, "alpha":0}}],"is_error":false}],"created_at":"1970-01-01T00:00:00Z"},{"role":"system_notice","kind":"external_event","blocks":[{"type":"external_event","source":"fixture","event_type":"changed","content":[{"type":"structured","data":{"notice":2, "a":1}}]}],"created_at":"1970-01-01T00:00:00Z"},{"role":"system_notice","kind":"generic","body":"no blocks","created_at":"1970-01-01T00:00:00Z"}]"#;

        assert_eq!(
            released_0810_transcript_serialized_rows_digest(&rows).expect("frozen digest"),
            digest_label(expected)
        );

        let mut raw_tamper = rows.to_vec();
        raw_tamper[1] = String::from_utf8(raw_tamper[1].clone())
            .expect("fixture UTF-8")
            .replace(r#""z":1"#, r#""z":9"#)
            .into_bytes();
        assert_ne!(
            released_0810_transcript_serialized_rows_digest(&raw_tamper)
                .expect("semantic raw tamper remains valid JSON"),
            digest_label(expected)
        );

        let mut ordinary_tamper = rows.to_vec();
        ordinary_tamper[0] = String::from_utf8(ordinary_tamper[0].clone())
            .expect("fixture UTF-8")
            .replace("ordered", "changed")
            .into_bytes();
        assert_ne!(
            released_0810_transcript_serialized_rows_digest(&ordinary_tamper)
                .expect("ordinary semantic tamper remains valid JSON"),
            digest_label(expected)
        );
    }

    #[test]
    fn exact_released_row_digest_scans_many_raw_slots_with_linear_work() {
        let slot_count = 512usize;
        let mut content = String::new();
        for index in 0..slot_count {
            if index > 0 {
                content.push(',');
            }
            let reverse = slot_count - index;
            content.push_str(&format!(
                r#"{{"type":"structured","data":{{"z":{index}, "a":{reverse}}}}}"#
            ));
        }
        let source_time = "2026-01-02T03:04:05Z";
        let row =
            format!(r#"{{"role":"user","content":[{content}],"created_at":"{source_time}"}}"#);
        let rows = vec![row.as_bytes().to_vec()];
        let expected = format!(
            r#"[{{"role":"user","content":[{content}],"created_at":"1970-01-01T00:00:00Z"}}]"#
        );

        reset_released_0810_raw_scan_steps();
        assert_eq!(
            released_0810_transcript_serialized_rows_digest(&rows)
                .expect("many-slot frozen digest"),
            digest_label(expected.as_bytes())
        );

        let source_bytes = rows.iter().map(Vec::len).sum::<usize>();
        let linear_budget = source_bytes
            .saturating_add(expected.len())
            .saturating_mul(4);
        let scan_steps = released_0810_raw_scan_steps();
        assert!(
            scan_steps <= u64::try_from(linear_budget).expect("linear scan budget fits u64"),
            "many-slot frozen digest used {scan_steps} scan steps for {} source bytes and {} \
             expected bytes; linear budget is {linear_budget}",
            source_bytes,
            expected.len()
        );
    }

    #[test]
    fn raw_heavy_released_row_digest_has_a_linear_scan_budget() {
        use std::fmt::Write as _;

        const SLOT_COUNT: usize = 512;
        let mut blocks = String::new();
        for index in 0..SLOT_COUNT {
            if index > 0 {
                blocks.push(',');
            }
            write!(
                blocks,
                r#"{{"type":"structured","data":{{"z":{index},"a":{index}}}}}"#
            )
            .expect("write raw-heavy block");
        }
        let source_row = format!(
            r#"{{"role":"user","content":[{blocks}],"created_at":"2026-01-02T03:04:05Z"}}"#
        );
        let expected_row = format!(
            r#"{{"role":"user","content":[{blocks}],"created_at":"1970-01-01T00:00:00Z"}}"#
        );
        let expected_framed = format!("[{expected_row}]");

        reset_released_0810_raw_scan_steps();
        assert_eq!(
            released_0810_transcript_serialized_rows_digest(&[source_row.as_bytes().to_vec()])
                .expect("raw-heavy frozen digest"),
            digest_label(expected_framed.as_bytes())
        );
        let scan_steps = released_0810_raw_scan_steps();
        let linear_budget = u64::try_from(
            source_row
                .len()
                .saturating_add(expected_framed.len())
                .saturating_mul(4),
        )
        .unwrap_or(u64::MAX);
        assert!(
            scan_steps <= linear_budget,
            "raw-slot restoration scanned {scan_steps} positions for {} source/expected bytes",
            source_row.len().saturating_add(expected_framed.len())
        );
    }

    #[test]
    fn explicit_released_unspecified_system_mutation_is_digest_omitted() {
        let rows = vec![
            br#"{"role":"system","content":"same","mutation_kind":"unspecified","created_at":"2026-01-02T03:04:05Z"}"#.to_vec(),
        ];
        let expected =
            br#"[{"role":"system","content":"same","created_at":"1970-01-01T00:00:00Z"}]"#;
        assert_eq!(
            released_0810_transcript_serialized_rows_digest(&rows).expect("frozen digest"),
            digest_label(expected)
        );
    }
}

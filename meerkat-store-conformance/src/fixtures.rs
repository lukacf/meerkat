//! Deterministic session/blob fixtures shared by the chapters.
//!
//! Legacy fixtures fabricate **current-envelope** (`version: 2`) session
//! documents whose metadata lacks the `session_checkpoint_stamp_v1` key —
//! the exact shape 0.7.x fleets persisted. Do NOT fabricate v0/v1 envelopes:
//! `meerkat_core::adopt_legacy_session` rejects them by design (the envelope
//! version authority fails closed on pre-current envelopes).

use meerkat_core::{
    Message, SESSION_CHECKPOINT_STAMP_KEY, SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY, Session,
    UserMessage,
};

use crate::failure::ConformanceFailure;

const FIXTURE_CHAPTER: &str = "fixtures";

/// A fresh session with one user text message per entry in `texts`.
pub fn session_with_texts(texts: &[&str]) -> Session {
    let mut session = Session::new();
    for text in texts {
        session.push(Message::User(UserMessage::text((*text).to_string())));
    }
    session
}

/// Append one user text message.
pub fn push_text(session: &mut Session, text: &str) {
    session.push(Message::User(UserMessage::text(text.to_string())));
}

/// Same-id session with the transcript truncated to the first `keep`
/// messages (fabricated through the serialized document, mirroring the
/// projection-shrink shape the append-only guard must reject).
pub fn with_transcript_truncated(
    session: &Session,
    keep: usize,
) -> Result<Session, ConformanceFailure> {
    let mut document = to_document(session, "with_transcript_truncated")?;
    document
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            ConformanceFailure::new(
                FIXTURE_CHAPTER,
                "with_transcript_truncated",
                "serialized session document carries no messages array",
            )
        })?
        .truncate(keep);
    from_document(document, "with_transcript_truncated")
}

/// Same-id, same-length session whose LAST message diverges from the
/// persisted transcript (a non-continuation the continuity guard must
/// reject).
pub fn with_divergent_tail(
    session: &Session,
    replacement_text: &str,
) -> Result<Session, ConformanceFailure> {
    let mut document = to_document(session, "with_divergent_tail")?;
    let replacement = serde_json::to_value(Message::User(UserMessage::text(
        replacement_text.to_string(),
    )))
    .map_err(|error| {
        ConformanceFailure::new(FIXTURE_CHAPTER, "with_divergent_tail", error.to_string())
    })?;
    let messages = document
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            ConformanceFailure::new(
                FIXTURE_CHAPTER,
                "with_divergent_tail",
                "serialized session document carries no messages array",
            )
        })?;
    let last = messages.last_mut().ok_or_else(|| {
        ConformanceFailure::new(
            FIXTURE_CHAPTER,
            "with_divergent_tail",
            "session has no messages to diverge",
        )
    })?;
    *last = replacement;
    from_document(document, "with_divergent_tail")
}

/// Serialize a session into a legacy (pre-typed-checkpoint) source BLOB.
///
/// Self-checks the fabricated shape: current envelope version, and metadata
/// that lacks the reserved `session_checkpoint_stamp_v1` key.
pub fn legacy_session_blob(session: &Session) -> Result<Vec<u8>, ConformanceFailure> {
    let document = to_document(session, "legacy_session_blob")?;
    let stamped = document
        .get("metadata")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|metadata| metadata.contains_key(SESSION_CHECKPOINT_STAMP_KEY));
    if stamped {
        return Err(ConformanceFailure::new(
            FIXTURE_CHAPTER,
            "legacy_session_blob",
            "legacy fixture must not carry a typed checkpoint stamp",
        ));
    }
    serde_json::to_vec(&document).map_err(|error| {
        ConformanceFailure::new(FIXTURE_CHAPTER, "legacy_session_blob", error.to_string())
    })
}

/// Same session with the legacy runtime-checkpoint provenance boolean marker
/// (`session_runtime_checkpoint_provenance_v1`) installed in metadata — the
/// optional marker 0.7.x runtime checkpoints carried.
pub fn with_legacy_runtime_checkpoint_marker(
    session: &Session,
    value: bool,
) -> Result<Session, ConformanceFailure> {
    let mut document = to_document(session, "with_legacy_runtime_checkpoint_marker")?;
    document
        .get_mut("metadata")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            ConformanceFailure::new(
                FIXTURE_CHAPTER,
                "with_legacy_runtime_checkpoint_marker",
                "serialized session document carries no metadata object",
            )
        })?
        .insert(
            SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY.to_string(),
            serde_json::Value::Bool(value),
        );
    from_document(document, "with_legacy_runtime_checkpoint_marker")
}

/// A deterministic text payload of `len` bytes (for large-payload steps).
pub fn large_text(len: usize) -> String {
    "meerkat-storage-conformance-payload-"
        .chars()
        .cycle()
        .take(len)
        .collect()
}

/// A tiny valid base64-encoded 1x1 PNG (passes payload signature gates).
pub const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn to_document(
    session: &Session,
    step: &'static str,
) -> Result<serde_json::Value, ConformanceFailure> {
    serde_json::to_value(session)
        .map_err(|error| ConformanceFailure::new(FIXTURE_CHAPTER, step, error.to_string()))
}

fn from_document(
    document: serde_json::Value,
    step: &'static str,
) -> Result<Session, ConformanceFailure> {
    serde_json::from_value(document)
        .map_err(|error| ConformanceFailure::new(FIXTURE_CHAPTER, step, error.to_string()))
}

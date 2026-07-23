//! Deterministic session/blob fixtures shared by the chapters.
//!
//! Legacy fixtures are **byte-literal** serialized 0.7.x documents (see
//! [`legacy_v07_session_fixture`]): current-envelope (`version: 2`) session
//! JSON whose metadata lacks the `session_checkpoint_stamp_v1` key — the
//! exact shape 0.7.x fleets persisted, pinned as bytes so the fixture cannot
//! silently track current-serializer drift. Do NOT fabricate v0/v1
//! envelopes: `meerkat_core::adopt_legacy_session` rejects them by design
//! (the envelope version authority fails closed on pre-current envelopes).

use meerkat_core::{
    Message, SESSION_CHECKPOINT_STAMP_KEY, SESSION_RUNTIME_CHECKPOINT_PROVENANCE_KEY, Session,
    SessionId, UserMessage,
};

use crate::failure::ConformanceFailure;

const FIXTURE_CHAPTER: &str = "fixtures";

/// Placeholder id inside the byte-literal legacy templates. Instantiation
/// substitutes a freshly minted [`SessionId`] (same textual length), so
/// fixture installs never collide across chapter runs over shared storage.
const LEGACY_FIXTURE_ID_PLACEHOLDER: &str = "00000000-0000-0000-0000-000000000000";

/// Byte-literal serialized 0.7.x session document — the N−1 on-disk shape
/// the legacy read paths in `meerkat-store` decode: version-2 envelope,
/// plain-string user content (the legacy single-text form), adjacently
/// tagged assistant blocks, and **unstamped** metadata. Derived from the
/// current wire format minus the newest additions (no `render_metadata` /
/// `identity` / `transcript_role` / `mutation_kind` keys, no checkpoint
/// stamp).
const LEGACY_V07_SESSION_DOCUMENT: &str = concat!(
    r#"{"version":2,"id":"00000000-0000-0000-0000-000000000000","messages":["#,
    r#"{"role":"user","content":"legacy fixture turn one","created_at":"2025-11-02T08:30:00Z"},"#,
    r#"{"role":"block_assistant","blocks":[{"block_type":"text","data":{"text":"legacy fixture assistant reply"}}],"stop_reason":"end_turn","created_at":"2025-11-02T08:30:30Z"},"#,
    r#"{"role":"user","content":"legacy fixture turn two","created_at":"2025-11-02T08:31:00Z"}],"#,
    r#""created_at":{"secs_since_epoch":1762072200,"nanos_since_epoch":0},"#,
    r#""updated_at":{"secs_since_epoch":1762072260,"nanos_since_epoch":0},"#,
    r#""metadata":{},"#,
    r#""usage":{"input_tokens":42,"output_tokens":17,"cache_creation_tokens":null,"cache_read_tokens":null}}"#,
);

/// The same N−1 shape carrying the optional 0.7.x runtime-checkpoint
/// provenance boolean marker (`session_runtime_checkpoint_provenance_v1`).
const LEGACY_V07_RUNTIME_CHECKPOINT_DOCUMENT: &str = concat!(
    r#"{"version":2,"id":"00000000-0000-0000-0000-000000000000","messages":["#,
    r#"{"role":"user","content":"legacy runtime checkpoint turn","created_at":"2025-11-02T09:00:00Z"}],"#,
    r#""created_at":{"secs_since_epoch":1762074000,"nanos_since_epoch":0},"#,
    r#""updated_at":{"secs_since_epoch":1762074000,"nanos_since_epoch":0},"#,
    r#""metadata":{"session_runtime_checkpoint_provenance_v1":true},"#,
    r#""usage":{"input_tokens":7,"output_tokens":0,"cache_creation_tokens":null,"cache_read_tokens":null}}"#,
);

/// One instantiated byte-literal legacy fixture: the raw persisted document
/// bytes and the session id they carry.
pub struct LegacySessionFixture {
    pub id: SessionId,
    pub document: Vec<u8>,
}

fn instantiate_legacy_fixture(template: &'static str) -> LegacySessionFixture {
    let id = SessionId::new();
    let document = template
        .replace(LEGACY_FIXTURE_ID_PLACEHOLDER, &id.to_string())
        .into_bytes();
    LegacySessionFixture { id, document }
}

/// A plain (unmarked) 0.7.x-shaped session document with a fresh id.
pub fn legacy_v07_session_fixture() -> LegacySessionFixture {
    instantiate_legacy_fixture(LEGACY_V07_SESSION_DOCUMENT)
}

/// A 0.7.x-shaped session document carrying the runtime-checkpoint
/// provenance marker, with a fresh id.
pub fn legacy_v07_runtime_checkpoint_fixture() -> LegacySessionFixture {
    instantiate_legacy_fixture(LEGACY_V07_RUNTIME_CHECKPOINT_DOCUMENT)
}

/// Decode a persisted session document through the exact serde row-decode
/// path every in-repo store uses for its session rows.
pub fn decode_session_document(document: &[u8]) -> Result<Session, ConformanceFailure> {
    serde_json::from_slice(document).map_err(|error| {
        ConformanceFailure::new(
            FIXTURE_CHAPTER,
            "decode_session_document",
            error.to_string(),
        )
    })
}

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

/// A second, distinct PNG-signature payload (the 8-byte PNG magic plus one
/// trailing byte, base64-encoded). Content-addresses to a different blob id
/// than [`TINY_PNG_BASE64`]; used by cross-handle identity steps.
pub const TINY_PNG_VARIANT_BASE64: &str = "iVBORw0KGgoB";

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

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use meerkat_core::{SessionCheckpointMetadataState, session_checkpoint_metadata_state};

    use super::*;

    #[test]
    fn legacy_fixture_bytes_decode_and_report_unverified() {
        let fixture = legacy_v07_session_fixture();
        let decoded = decode_session_document(&fixture.document).expect("fixture must decode");
        assert_eq!(decoded.id(), &fixture.id);
        assert_eq!(decoded.messages().len(), 3);
        match session_checkpoint_metadata_state(decoded.id(), decoded.metadata())
            .expect("metadata state")
        {
            SessionCheckpointMetadataState::LegacyUnverified {
                legacy_runtime_checkpoint: false,
            } => {}
            other => panic!("plain legacy fixture must decode LegacyUnverified: {other:?}"),
        }
    }

    #[test]
    fn legacy_runtime_checkpoint_fixture_carries_marker() {
        let fixture = legacy_v07_runtime_checkpoint_fixture();
        let decoded = decode_session_document(&fixture.document).expect("fixture must decode");
        match session_checkpoint_metadata_state(decoded.id(), decoded.metadata())
            .expect("metadata state")
        {
            SessionCheckpointMetadataState::LegacyUnverified {
                legacy_runtime_checkpoint: true,
            } => {}
            other => panic!("marker fixture must decode LegacyUnverified(marker): {other:?}"),
        }
    }

    /// The N−1 document must decode/encode stably through the current wire:
    /// a drift here breaks migration custody for every store that persisted
    /// the 0.7.x shape.
    #[test]
    fn legacy_fixture_bytes_reencode_identically() {
        for fixture in [
            legacy_v07_session_fixture(),
            legacy_v07_runtime_checkpoint_fixture(),
        ] {
            let decoded = decode_session_document(&fixture.document).expect("fixture must decode");
            let reencoded = serde_json::to_value(&decoded).expect("re-encode");
            let raw: serde_json::Value =
                serde_json::from_slice(&fixture.document).expect("fixture is JSON");
            assert_eq!(reencoded, raw);
        }
    }

    #[test]
    fn png_fixture_payloads_are_distinct_and_signature_valid() {
        use base64_free_decode::decode_prefix;
        // The variant must decode to bytes starting with the PNG magic; a
        // hand-rolled prefix check keeps base64 out of the dependency tree.
        assert_ne!(TINY_PNG_BASE64, TINY_PNG_VARIANT_BASE64);
        let magic = decode_prefix(TINY_PNG_VARIANT_BASE64);
        assert_eq!(&magic[..8], &b"\x89PNG\r\n\x1a\n"[..]);
    }

    /// Minimal standard-alphabet base64 decoder for the fixture self-check.
    mod base64_free_decode {
        pub fn decode_prefix(encoded: &str) -> Vec<u8> {
            let value = |c: u8| -> u32 {
                match c {
                    b'A'..=b'Z' => u32::from(c - b'A'),
                    b'a'..=b'z' => u32::from(c - b'a') + 26,
                    b'0'..=b'9' => u32::from(c - b'0') + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => panic!("unexpected base64 char {c}"),
                }
            };
            let bytes = encoded.as_bytes();
            let mut out = Vec::new();
            for chunk in bytes.chunks(4) {
                let mut acc = 0u32;
                for &c in chunk {
                    acc = (acc << 6) | value(c);
                }
                acc <<= 6 * (4 - chunk.len());
                out.push((acc >> 16) as u8);
                if chunk.len() > 2 {
                    out.push((acc >> 8) as u8);
                }
                if chunk.len() > 3 {
                    out.push(acc as u8);
                }
            }
            out
        }
    }
}

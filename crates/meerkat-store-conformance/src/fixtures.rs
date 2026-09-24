//! Deterministic session/blob fixtures shared by the chapters.

use meerkat_core::{Message, Session, UserMessage};

use crate::failure::ConformanceFailure;

const FIXTURE_CHAPTER: &str = "fixtures";

/// A fresh session with one user text message per entry in `texts`.
pub fn session_with_texts(texts: &[&str]) -> Result<Session, ConformanceFailure> {
    let mut session = Session::new();
    for text in texts {
        session.push(Message::User(UserMessage::text((*text).to_string())));
    }
    Ok(session)
}

/// Append one user text message.
pub fn push_text(session: &mut Session, text: &str) -> Result<(), ConformanceFailure> {
    session.push(Message::User(UserMessage::text(text.to_string())));
    Ok(())
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
    use super::*;

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

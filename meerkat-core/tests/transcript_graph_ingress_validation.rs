//! Regression coverage for transcript-graph ingress validation.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};
use meerkat_core::types::{Message, UserMessage};
use meerkat_core::{SESSION_TRANSCRIPT_HISTORY_STATE_KEY, Session};

/// A prior valid decode must not create process-global trust that can hide a
/// later document's invalid graph authority.
#[test]
fn warm_decode_cannot_hide_corrupt_rewrite_prefix() {
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(
        "transcript-graph rewrite-prefix integrity before",
    )));
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "transcript-graph rewrite-prefix integrity after",
            ))],
            TranscriptRewriteReason::new("integrity-regression"),
            Some("transcript-graph-ingress-regression".to_string()),
            None,
        )
        .expect("fixture rewrite is valid");

    let clean = serde_json::to_value(&session).expect("valid session serializes");
    let _: Session = serde_json::from_value(clean.clone()).expect("valid graph warms decode path");

    let mut corrupt = clean;
    let rewrite_prefix =
        &mut corrupt["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["rewrite_prefix"];
    let original = rewrite_prefix["digest"]
        .as_str()
        .expect("fixture carries a rewrite-prefix digest");
    let forged = format!("sha256:{}", "0".repeat(64));
    assert_ne!(original, forged, "fixture digest must differ from forgery");
    rewrite_prefix["digest"] = serde_json::Value::String(forged);

    let error = serde_json::from_value::<Session>(corrupt)
        .expect_err("a corrupt rewrite prefix must fail every decode");
    assert!(
        error
            .to_string()
            .contains("rewrite-prefix accumulator does not bind"),
        "unexpected corrupt-prefix error: {error}"
    );
}

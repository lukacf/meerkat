//! Operator probe for a captured WholeBlob body that fails the audited-endpoint
//! ingress guard. Reads the raw document named by `WEDGE_BODY_PATH`, evaluates
//! the shared endpoint relation on its rows without the ingress guard, and
//! prints structural facts only (counts, kinds, indexes, digest prefixes).
//! Ignored by default; run with `--ignored` and the env var set.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::types::Message;
use meerkat_core::{SESSION_TRANSCRIPT_HISTORY_STATE_KEY, Session, TranscriptHistoryState};

#[test]
#[ignore = "needs WEDGE_BODY_PATH pointing at a captured WholeBlob body"]
fn captured_body_reports_its_audited_endpoint_divergence() {
    let path = std::env::var("WEDGE_BODY_PATH").expect("WEDGE_BODY_PATH");
    let bytes = std::fs::read(&path).expect("read body");
    let ingress = Session::from_persisted_bytes(&bytes)
        .err()
        .map(|error| error.to_string());
    println!("ingress_refusal_present={}", ingress.is_some());
    if let Some(message) = &ingress {
        println!(
            "ingress_refusal_mentions_audited_endpoint={}",
            message.contains("audited endpoint")
        );
    }
    let document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let live: Vec<Message> =
        serde_json::from_value(document["messages"].clone()).expect("messages decode");
    let history = &document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY];
    if history.is_null() {
        println!(
            "history_state=absent live_rows={} (no graph, no endpoint relation)",
            live.len()
        );
        return;
    }
    let state: TranscriptHistoryState =
        serde_json::from_value(history.clone()).expect("history state decode");
    let divergence = meerkat_core::audited_endpoint_divergence(&state, &live).expect("relation");
    match divergence {
        None => println!("divergence=none live_rows={}", live.len()),
        Some(divergence) => println!(
            "divergence_kind={:?} endpoint_rows={} live_rows={} first_divergent_row={:?} endpoint_revision_prefix={}",
            divergence.kind,
            divergence.endpoint_row_count,
            divergence.live_row_count,
            divergence.first_divergent_row,
            &divergence.endpoint_revision[..divergence.endpoint_revision.len().min(24)]
        ),
    }
}

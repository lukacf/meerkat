//! Persisted bytes per boundary on a whole-blob backend stay bounded under
//! transcript-history retention.
//!
//! A blob store writes the whole session document at every boundary, so the
//! document size is the per-turn write cost. Before retention the rewrite
//! graph kept every message the session ever produced, and that cost grew
//! with lifetime history. This drives the real JSONL file backend, including
//! its append-only and rewrite save guards against re-anchored predecessors,
//! and measures the file after each boundary.

#![cfg(feature = "jsonl")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_core::types::{Message, UserMessage};
use meerkat_core::{
    Session, SessionStore, TranscriptHistoryRetention, TranscriptRewriteReason,
    TranscriptRewriteSelection,
};
use meerkat_store::JsonlStore;

struct Run {
    sizes: Vec<u64>,
    session: Session,
    store: JsonlStore,
    _dir: tempfile::TempDir,
}

async fn run_cycles(retain: Option<usize>, cycles: usize) -> Run {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = JsonlStore::builder(dir.path().to_path_buf())
        .pretty_print(false)
        .build();
    store.init().await.expect("init");
    let mut session = Session::new();
    let file = dir.path().join(format!("{}.jsonl", session.id().0));
    let mut sizes = Vec::with_capacity(cycles);
    for cycle in 0..cycles {
        for turn in 0..8 {
            session.push(Message::User(UserMessage::text(format!(
                "cycle {cycle} turn {turn}: {}",
                "history ".repeat(64)
            ))));
        }
        // Ordinary boundary: append-only save over the stored predecessor.
        store.save(&session).await.expect("boundary save");
        let end = session.messages().len();
        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end },
                vec![Message::User(UserMessage::text(format!(
                    "summary of cycle {cycle}"
                )))],
                TranscriptRewriteReason::new("compaction"),
                Some("retention-bytes-test".to_string()),
                None,
            )
            .expect("rewrite");
        if let Some(retain) = retain {
            session
                .retire_transcript_history(TranscriptHistoryRetention::from_count(retain).unwrap())
                .expect("retire");
        }
        // Compaction boundary: the rewrite save guard checks the incoming
        // graph against the stored (possibly re-anchored) predecessor.
        store
            .save_transcript_rewrite(&session, &commit)
            .await
            .expect("rewrite save");
        sizes.push(std::fs::metadata(&file).expect("session file").len());
    }
    Run {
        sizes,
        session,
        store,
        _dir: dir,
    }
}

#[tokio::test]
async fn whole_blob_bytes_per_boundary_stay_bounded_under_retention() {
    let run = run_cycles(Some(3), 40).await;
    let retained = &run.sizes;
    let steady = retained[9];
    for (cycle, size) in retained.iter().enumerate().skip(10) {
        assert!(
            *size <= steady + 64 * 1024,
            "cycle {cycle}: persisted document grew to {size} bytes (steady state {steady})"
        );
    }

    let unbounded = run_cycles(None, 40).await.sizes;
    eprintln!(
        "JSONL bytes per boundary at cycles 10/20/40: retained {}/{}/{}, unbounded {}/{}/{}",
        retained[9], retained[19], retained[39], unbounded[9], unbounded[19], unbounded[39]
    );
    assert!(
        unbounded[39] > 5 * retained[39],
        "without retention the document ({} bytes) grows with lifetime history; with it, {} bytes",
        unbounded[39],
        retained[39]
    );

    let loaded = run
        .store
        .load(run.session.id())
        .await
        .expect("load")
        .expect("session present");
    assert_eq!(loaded.messages(), run.session.messages());
    let graph = loaded
        .transcript_history_state()
        .expect("graph")
        .expect("graph present");
    assert_eq!(graph.commit_count(), 40);
    assert_eq!(graph.retired_count(), 37);
}

//! Framed-midstate checkpoint digest: correctness and cost contracts, pinned
//! from outside the crate through the public API only.
//!
//! `session_checkpoint_digest` serves a retained SHA-256 midstate over the
//! canonical checkpoint document's transcript span and finalizes it with the
//! turn-sized suffix, byte-identical to the full canonical-document pass
//! (`framed_session_checkpoint_digest` in `meerkat-core/src/checkpoint.rs`).
//! In debug/test builds the in-crate sampled cross-check verifies EVERY
//! framed serve against the untouched full path with `assert_eq!` — so every
//! `session_checkpoint_digest` call these tests make across shapes,
//! mutations, marker collisions, and rewrite reseeds IS a mutation proof: a
//! framing seam that changes the canonical byte stream without invalidating
//! the midstate panics inside the crate before any assertion here runs.
//!
//! What that cross-check cannot see is COST: a framed path that silently
//! falls back to the full O(document) pass on every call stays correct and
//! keeps every equality green while killing the optimization. The byte
//! budget test below pins that dimension with
//! `session_content_digest_bytes()` deltas (a thread-local counter, so each
//! measurement stays inside one test function).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
};
use meerkat_core::{
    Session, SessionCheckpointDigest, session_checkpoint_digest, session_content_digest_bytes,
};

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant(text: &str) -> Message {
    Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: text.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    ))
}

/// A session with `count` alternating messages of roughly `text_len` bytes.
fn session_with_messages(count: usize, text_len: usize) -> Session {
    let mut session = Session::new();
    for index in 0..count {
        let body = format!("{index}:{}", "x".repeat(text_len));
        if index % 2 == 0 {
            session.push(user(&body));
        } else {
            session.push(assistant(&body));
        }
    }
    session
}

/// Commit an audited rewrite of the last message, giving the session a
/// transcript-history graph (and, mid-test, replacing the live vector).
fn commit_rewrite(session: &mut Session, label: &str) {
    let end = session.messages().len();
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: end - 1,
                end,
            },
            vec![assistant(label)],
            TranscriptRewriteReason::new("framed-checkpoint-test"),
            Some("framed-checkpoint-test".to_string()),
            None,
        )
        .expect("audited rewrite");
}

/// The full-path reference digest: a serde round trip yields a session with
/// no retained midstate, so its first checkpoint digest is computed cold —
/// and, like every call in this file, cross-checked in-crate against the
/// full document path.
fn cold_reference_digest(session: &Session) -> SessionCheckpointDigest {
    let encoded = serde_json::to_string(session).expect("session encodes");
    let decoded: Session = serde_json::from_str(&encoded).expect("session decodes");
    session_checkpoint_digest(&decoded).expect("cold reference digest")
}

/// Seed-then-serve equality across transcript shapes, with and without a
/// transcript-history graph, under metadata whose canonical spelling
/// exercises unicode, quotes, and backslash escaping.
///
/// The first `session_checkpoint_digest` call seeds the framed midstate
/// (fallback-or-reseed), the second serves it warm; a mutation must move the
/// digest and land exactly on the cold full-path reference. Simply making
/// these calls broadly is the mutation proof: the in-crate debug cross-check
/// asserts framed == full on every serve, so a stale or misframed midstate
/// panics rather than slipping past an equality below.
#[test]
fn framed_checkpoint_digest_matches_full_recompute_across_shapes() {
    for count in [0usize, 1, 2, 7, 40] {
        for with_graph in [false, true] {
            // An empty transcript has no message to rewrite into a graph.
            if with_graph && count == 0 {
                continue;
            }
            let mut session = session_with_messages(count, 60);
            session
                .try_set_metadata(
                    "framed_digest_probe",
                    serde_json::json!({
                        "note": "üñïçødé 😀 \"quotes\" and \\back\\slashes\\ and a \u{1} control",
                        "nested": { "z": "sorts last", "a": ["mixed", 1, true] },
                    }),
                )
                .expect("plain metadata key is not reserved");
            if with_graph {
                commit_rewrite(&mut session, "audited replacement");
            }

            let first = session_checkpoint_digest(&session).unwrap_or_else(|err| {
                panic!("seed digest (count {count}, graph {with_graph}): {err}")
            });
            let second = session_checkpoint_digest(&session).unwrap_or_else(|err| {
                panic!("warm digest (count {count}, graph {with_graph}): {err}")
            });
            assert_eq!(
                first, second,
                "warm framed serve must reproduce the seeded digest \
                 (count {count}, graph {with_graph})"
            );

            session.push(user("appended after seeding"));
            let mutated = session_checkpoint_digest(&session).unwrap_or_else(|err| {
                panic!("post-append digest (count {count}, graph {with_graph}): {err}")
            });
            assert_ne!(
                second, mutated,
                "an appended message must change the checkpoint digest \
                 (count {count}, graph {with_graph})"
            );
            assert_eq!(
                mutated,
                cold_reference_digest(&session),
                "framed digest after an append must match the cold full-path \
                 reference (count {count}, graph {with_graph})"
            );
        }
    }
}

/// Warm checkpoint-digest bytes must be O(metadata + delta), not O(document).
///
/// Graph-FREE sessions deliberately: a history-bearing session folds the
/// witness derivation (its own byte bucket) into the same call window, which
/// would blur the `checkpoint-digest` bucket this test isolates.
///
/// Calibration: the warm serve hashes the framed document's prefix and
/// suffix — identity fields, metadata, usage, timestamps, all turn-sized and
/// structurally identical for both sessions — and zero bytes per retained
/// message. The small/large deltas therefore differ only by scalar
/// spellings (a few bytes); 2x is a deliberately loose band that still fails
/// loudly if either side degrades to a document-sized pass, because the
/// large session's canonical document (~480 KB of message text) is three
/// orders of magnitude above its framing.
#[test]
fn warm_checkpoint_digest_bytes_are_independent_of_transcript_size() {
    fn cold_and_warm_bytes(count: usize, text_len: usize) -> (u64, u64) {
        let mut session = session_with_messages(count, text_len);

        let before = session_content_digest_bytes();
        let seeded = session_checkpoint_digest(&session).expect("seed digest");
        let cold = session_content_digest_bytes() - before;

        // One small append: extends the framed midstate O(delta) without
        // touching the retained prefix, and moves `updated_at` in the
        // suffix, so the warm call below recomputes rather than trivially
        // matching.
        session.push(user("one more"));

        let before = session_content_digest_bytes();
        let warm_digest = session_checkpoint_digest(&session).expect("warm digest");
        let warm = session_content_digest_bytes() - before;
        assert_ne!(seeded, warm_digest, "the append must move the digest");
        (cold, warm)
    }

    let (small_cold, small_warm) = cold_and_warm_bytes(10, 120);
    let (large_cold, large_warm) = cold_and_warm_bytes(400, 1_200);
    println!(
        "checkpoint-digest bytes: small cold {small_cold} / warm {small_warm}, \
         large cold {large_cold} / warm {large_warm}"
    );

    // The cost dimension the in-crate cross-check cannot see: a framed path
    // that always fell back would stay correct and keep every equality in
    // this file green while hashing the whole document per call. The warm
    // serve must come in far below the seeding pass.
    assert!(
        large_warm * 3 < large_cold,
        "warm framed serve must cost far less than the seeding pass \
         (warm {large_warm} vs cold {large_cold} bytes): the framed path is \
         falling back to the full O(document) pass"
    );
    assert!(
        large_warm <= small_warm * 2 && small_warm <= large_warm * 2,
        "warm checkpoint-digest bytes must be O(metadata + delta), not \
         O(document) (small warm {small_warm}, large warm {large_warm})"
    );
}

/// A metadata value carrying the splice-marker spelling collides with the
/// per-call marker, the exactly-once split refuses, and the digest falls
/// back to the full document path: correctness survives marker collision.
#[test]
fn metadata_containing_the_splice_marker_falls_back_to_the_full_path() {
    // The marker counter is process-global and monotonic; every framed
    // document build (one per session_checkpoint_digest call, from any test
    // sharing this binary's process) consumes one value. Planting 0..1024
    // covers every value the counter can plausibly reach here under both
    // nextest (process-per-test) and plain `cargo test` (shared process,
    // ~50 total calls across this file).
    let markers: Vec<serde_json::Value> = (0..1024)
        .map(|n| serde_json::Value::String(format!("\u{0}meerkat-transcript-splice-{n}")))
        .collect();

    let mut session = session_with_messages(300, 600);
    session
        .try_set_metadata("splice_collision_probe", serde_json::Value::Array(markers))
        .expect("plain metadata key is not reserved");

    let first = session_checkpoint_digest(&session).expect("collision digest");
    let before = session_content_digest_bytes();
    let second = session_checkpoint_digest(&session).expect("collision digest repeat");
    let fallback_bytes = session_content_digest_bytes() - before;

    assert_eq!(first, second);
    assert_eq!(
        second,
        cold_reference_digest(&session),
        "marker collision must fall back to the full path, never splice at \
         the wrong site"
    );

    // Prove the fallback actually ran: the repeat call must hash the whole
    // canonical document (~180 KB of message text plus ~40 KB of planted
    // markers), not serve a warm midstate (which would count only the
    // ~40 KB framed suffix — no message bytes). If `split_exactly_once`
    // were loosened to take the first match, the first call would seed the
    // midstate and this repeat would come in far under the floor.
    assert!(
        fallback_bytes > 120_000,
        "a marker-colliding document must recompute on the full path \
         (counted {fallback_bytes} bytes; expected the whole ~220 KB document)"
    );
}

/// Byte-identity against real pre-change bytes: the committed 0.8.8 fixture
/// and its manifest's `checkpoint_digest` literal pin the digest VALUE
/// itself, not merely self-consistency of the current binary.
#[test]
fn checkpoint_digest_matches_committed_v0_8_8_fixture() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let session_bytes = std::fs::read(fixtures.join("v0_8_8_full_session.json"))
        .expect("committed 0.8.8 session fixture");
    let manifest_bytes = std::fs::read(fixtures.join("v0_8_8_manifest.json"))
        .expect("committed 0.8.8 manifest fixture");
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).expect("manifest decodes");
    let expected = manifest
        .get("checkpoint_digest")
        .and_then(serde_json::Value::as_str)
        .expect("manifest carries a checkpoint_digest literal");
    let session: Session =
        serde_json::from_slice(&session_bytes).expect("0.8.8 session fixture decodes");
    let digest = session_checkpoint_digest(&session).expect("fixture digest");
    assert_eq!(
        digest.as_str(),
        expected,
        "checkpoint digest of the committed 0.8.8 document must be byte-identical"
    );
}

/// A transcript rewrite replaces the live message vector, which must
/// invalidate the framed midstate; the next digest reseeds over the
/// post-rewrite transcript. A stale serve would panic inside the crate's
/// per-serve cross-check before this test's own assertions run — the
/// equalities below additionally pin the reseeded value to the cold
/// full-path reference.
#[test]
fn transcript_replacement_reseeds_the_framed_midstate_correctly() {
    let mut session = session_with_messages(9, 80);
    commit_rewrite(&mut session, "first audited replacement");

    let seeded = session_checkpoint_digest(&session).expect("seed digest");

    session.push(user("turn between rewrites"));
    commit_rewrite(&mut session, "second audited replacement");

    let reseeded = session_checkpoint_digest(&session).expect("post-rewrite digest");
    assert_ne!(seeded, reseeded, "the rewrite must move the digest");
    assert_eq!(
        reseeded,
        cold_reference_digest(&session),
        "the reseeded framed digest must match the cold full-path reference"
    );
}

//! Turn-boundary digest budget: the work a save boundary spends on transcript
//! digests must not depend on how big the transcript is.
//!
//! `session_content_digest_computations()` counts full O(document)
//! canonical-JSON + SHA-256 passes over session content. It is a `thread_local`
//! counter, so every measurement here stays on one thread and inside one test
//! function.
//!
//! The production defect these tests pin: an identical one-word turn measured
//! 60 s at 14 MB and over 180 s at 94 MB, because each turn boundary recomputed
//! the whole-document digest a handful of times. Asserting "fewer digests" is
//! not enough — a constant number of O(document) passes still scales with the
//! document. These tests assert the counted passes are EQUAL at two very
//! different transcript sizes, and that the steady-state boundary count is
//! zero.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::session_store::{SessionHead, TranscriptStrandId, append_only_save_guard};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
};
use meerkat_core::{Session, session_content_digest_bytes, session_content_digest_computations};

const SMALL: usize = 8;
const LARGE: usize = 2_000;

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

/// A session with `turns` prior conversational turns.
fn session_with_turns(turns: usize) -> Session {
    let mut session = Session::new();
    session.append_system_message("system".to_string());
    for index in 0..turns {
        session.push(user(&format!(
            "question {index} with some body text to make the message non-trivial"
        )));
        session.push(assistant(&format!(
            "answer {index} with some body text to make the message non-trivial"
        )));
    }
    session
}

fn strand() -> TranscriptStrandId {
    TranscriptStrandId::root()
}

/// One ordinary turn boundary: append the turn, guard the save against the
/// previously persisted row, project the durable head row.
fn boundary_save_digest_count(turns: usize) -> u64 {
    let mut live = session_with_turns(turns);
    // Steady state: this session has already been saved at least once, so the
    // previous row and the live document are both warm. Two warm-up boundaries
    // keep the measurement out of first-sight seeding.
    let mut previous = live.clone();
    for warmup in 0..2 {
        append_only_save_guard(&live, Some(&previous)).expect("warm-up guard");
        SessionHead::from_session(&live, strand(), 0).expect("warm-up head");
        previous = live.clone();
        live.push(user(&format!("warm-up {warmup}")));
        live.push(assistant("warm-up reply"));
    }

    let before = session_content_digest_computations();
    append_only_save_guard(&live, Some(&previous)).expect("boundary guard");
    SessionHead::from_session(&live, strand(), 0).expect("boundary head");
    session_content_digest_computations() - before
}

#[test]
fn turn_boundary_digest_count_is_independent_of_transcript_size() {
    let small = boundary_save_digest_count(SMALL);
    let large = boundary_save_digest_count(LARGE);
    println!(
        "boundary-save full-document digest passes: {SMALL} turns => {small}, {LARGE} turns => {large}"
    );
    assert_eq!(
        small, large,
        "turn-boundary digest work must not depend on transcript size \
         ({SMALL} turns => {small} passes, {LARGE} turns => {large} passes)"
    );
}

#[test]
fn steady_state_turn_boundary_spends_no_full_document_digest() {
    assert_eq!(
        boundary_save_digest_count(LARGE),
        0,
        "a warm boundary save must serve every transcript digest from the \
         incremental accumulator"
    );
}

/// A single append must not re-hash the transcript it appended to.
#[test]
fn append_digest_count_is_independent_of_transcript_size() {
    fn measure(turns: usize) -> u64 {
        let mut live = session_with_turns(turns);
        // Seed: the first digest of a materialized session is the one
        // mandatory full pass per process.
        let seeded = live
            .transcript_content_digest()
            .expect("seed transcript digest");
        assert!(seeded.starts_with("sha256:"));
        let before = session_content_digest_computations();
        live.push(user("one word"));
        let after_append = live
            .transcript_content_digest()
            .expect("append transcript digest");
        assert_ne!(seeded, after_append);
        session_content_digest_computations() - before
    }

    let small = measure(SMALL);
    let large = measure(LARGE);
    println!(
        "append full-document digest passes: {SMALL} turns => {small}, {LARGE} turns => {large}"
    );
    assert_eq!(small, large);
    assert_eq!(large, 0, "an append must not re-hash the whole transcript");
}

/// System messages are ordinary ordered transcript rows: they may be appended
/// anywhere and more than once. Appending a later system row must use the same
/// incremental digest path as every other ordered append.
#[test]
fn ordered_system_message_append_digest_budget_is_constant() {
    fn measure(turns: usize) -> u64 {
        let mut previous = session_with_turns(turns);
        previous.push(user("before a later system message"));
        previous.append_system_message("later system message".to_string());
        let mut live = previous.clone();
        live.append_system_message("another later system message".to_string());
        live.push(user("after the later system message"));
        let before = session_content_digest_computations();
        append_only_save_guard(&live, Some(&previous)).expect("ordered system append guard");
        session_content_digest_computations() - before
    }

    let small = measure(SMALL);
    let large = measure(LARGE);
    println!(
        "ordered system-message append digest passes: {SMALL} turns => {small}, {LARGE} turns => {large}"
    );
    assert_eq!(
        small, large,
        "ordinary ordered System appends must keep a transcript-size-independent digest budget"
    );
    assert_eq!(large, 0, "a System append must not re-hash prior rows");
}

#[test]
fn synthetic_notice_refresh_branch_digest_budget_is_constant() {
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};

    fn notice(server: &str) -> Message {
        Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::McpPending,
            None,
            SystemNoticeBlock::Mcp {
                server_id: Some(server.to_string()),
                operation: None,
                phase: None,
                persisted: false,
                detail: None,
                pending_sources: Vec::new(),
            },
        ))
    }

    fn measure(turns: usize) -> u64 {
        let mut previous = session_with_turns(turns);
        previous.push(notice("mcp pending"));
        let mut live = previous.clone();
        live.replace_synthetic_notices(SystemNoticeKind::McpPending, vec![notice("mcp ready")])
            .expect("synthetic notice refresh");
        let before = session_content_digest_computations();
        let _ = append_only_save_guard(&live, Some(&previous));
        session_content_digest_computations() - before
    }

    let small = measure(SMALL);
    let large = measure(LARGE);
    println!(
        "synthetic-notice-refresh branch digest passes: {SMALL} turns => {small}, {LARGE} turns => {large}"
    );
    assert_eq!(
        small, large,
        "the synthetic-notice-refresh acceptance branch must keep a constant digest budget"
    );
}

/// The PRODUCTION shape: a history-bearing (compacted/rewritten) session at
/// an ordinary turn boundary — guard plus durable head projection. This is
/// the case the original budget suite did not cover. The former whole-graph
/// canonical pass (`session_transcript_history_checkpoint_digest` via
/// `SessionHead::from_session`) was invisible to the pass counter, so
/// the suite reported zero at both sizes while release timing grew 211x.
/// When first made honest, this measured 1 pass at BOTH sizes (why "equal
/// counts" lied) hashing 9,560 bytes at 8 turns vs 1,993,054 at 2000.
///
/// The audited graph no longer changes on ordinary appends: its head is the
/// latest rewrite occurrence, while the live tail is owned by Session
/// messages and the incremental transcript accumulator. Once the initial
/// rewrite boundary is warm, neither the guard nor `SessionHead` may hash the
/// retained graph again. This pins ZERO full content passes and ZERO content
/// bytes at both transcript sizes.
#[test]
fn history_bearing_boundary_save_hashes_zero_content_bytes() {
    use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};

    fn measure(turns: usize) -> (u64, u64) {
        let mut live = session_with_turns(turns);
        let end = live.messages().len();
        live.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: end - 1,
                end,
            },
            vec![assistant("audited replacement")],
            TranscriptRewriteReason::new("unit-test"),
            Some("unit-test".to_string()),
            None,
        )
        .expect("audited rewrite");
        let audited_graph = live
            .metadata()
            .get(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .cloned()
            .expect("audited graph");
        // Steady state: two warm-up boundaries, exactly like the plain
        // boundary measurement, each followed by an appended turn.
        let mut previous = live.clone();
        for warmup in 0..2 {
            append_only_save_guard(&live, Some(&previous)).expect("warm-up guard");
            SessionHead::from_session(&live, strand(), 1).expect("warm-up head");
            previous = live.clone();
            live.push(user(&format!("warm-up {warmup}")));
            live.push(assistant("warm-up reply"));
        }

        let passes_before = session_content_digest_computations();
        let bytes_before = session_content_digest_bytes();
        append_only_save_guard(&live, Some(&previous)).expect("boundary guard");
        SessionHead::from_session(&live, strand(), 1).expect("boundary head");
        let passes = session_content_digest_computations() - passes_before;
        let bytes = session_content_digest_bytes() - bytes_before;
        assert_eq!(
            live.metadata()
                .get(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            Some(&audited_graph),
            "ordinary appends must leave audited graph bytes untouched"
        );
        (passes, bytes)
    }

    let (small_passes, small_bytes) = measure(SMALL);
    let (large_passes, large_bytes) = measure(LARGE);
    println!(
        "history-bearing boundary save: {SMALL} turns => {small_passes} passes / {small_bytes} bytes, \
         {LARGE} turns => {large_passes} passes / {large_bytes} bytes"
    );
    assert_eq!(
        (small_passes, small_bytes),
        (0, 0),
        "a warm small history-bearing boundary must not re-hash live or audited history"
    );
    assert_eq!(
        (large_passes, large_bytes),
        (0, 0),
        "a warm large history-bearing boundary must not re-hash live or audited history"
    );
}

/// The byte dimension of the plain boundary budget: a warm boundary on a
/// plain session must hash zero content bytes, at any size.
#[test]
fn steady_state_turn_boundary_hashes_zero_content_bytes() {
    fn boundary_bytes(turns: usize) -> u64 {
        let mut live = session_with_turns(turns);
        let mut previous = live.clone();
        for warmup in 0..2 {
            append_only_save_guard(&live, Some(&previous)).expect("warm-up guard");
            SessionHead::from_session(&live, strand(), 0).expect("warm-up head");
            previous = live.clone();
            live.push(user(&format!("warm-up {warmup}")));
            live.push(assistant("warm-up reply"));
        }
        let before = session_content_digest_bytes();
        append_only_save_guard(&live, Some(&previous)).expect("boundary guard");
        SessionHead::from_session(&live, strand(), 0).expect("boundary head");
        session_content_digest_bytes() - before
    }

    assert_eq!(
        boundary_bytes(LARGE),
        0,
        "a warm plain boundary save must hash zero content bytes"
    );
}

/// Release-mode timing evidence for the history-bearing boundary (the audit's
/// measurement). Run manually:
///   cargo test --release -p meerkat-core --test transcript_digest_budget \
///     history_bearing_boundary_release_timing -- --ignored --nocapture
/// Wall time is reported, never asserted — CI machines vary; the asserted
/// contract is the byte budget above.
#[test]
#[ignore = "manual release-timing evidence"]
fn history_bearing_boundary_release_timing() {
    use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};

    fn median_boundary(turns: usize) -> std::time::Duration {
        let mut live = session_with_turns(turns);
        let end = live.messages().len();
        let _rewrite = live
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: end - 1,
                    end,
                },
                vec![assistant("audited replacement")],
                TranscriptRewriteReason::new("timing"),
                Some("timing".to_string()),
                None,
            )
            .expect("audited rewrite");
        let mut previous = live.clone();
        let mut samples = Vec::new();
        // Enough rounds to exhaust the release-mode sampled verification
        // budget (the first N witness serves per process recompute and
        // compare); the median of the LAST 9 rounds is the steady state a
        // long-lived production process sees.
        for round in 0..45 {
            append_only_save_guard(&live, Some(&previous)).expect("warm guard");
            SessionHead::from_session(&live, strand(), 1).expect("warm head");
            previous = live.clone();
            live.push(user(&format!("turn {round}")));
            live.push(assistant("reply"));
            let start = std::time::Instant::now();
            append_only_save_guard(&live, Some(&previous)).expect("boundary guard");
            SessionHead::from_session(&live, strand(), 1).expect("boundary head");
            samples.push(start.elapsed());
        }
        let mut steady = samples.split_off(samples.len() - 9);
        steady.sort();
        steady[steady.len() / 2]
    }

    let small = median_boundary(SMALL);
    let large = median_boundary(LARGE);
    println!(
        "warm history-bearing boundary median: {SMALL} turns => {small:?}, {LARGE} turns => {large:?} \
         (ratio {:.1}x)",
        large.as_secs_f64() / small.as_secs_f64().max(f64::EPSILON)
    );
}

/// History-bearing sessions (anything that ever compacted or was rewritten)
/// used to pay TWO full graph validations plus a full head digest on every
/// appended batch, each of which hashes every retained revision body. An
/// append onto an already-validated graph proves nothing new about the
/// retained bodies, so the steady-state budget must be flat.
#[test]
fn history_bearing_append_digest_count_is_independent_of_transcript_size() {
    use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};

    fn measure(turns: usize) -> u64 {
        let mut live = session_with_turns(turns);
        let end = live.messages().len();
        let rewrite = live
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: end - 1,
                    end,
                },
                vec![assistant("audited replacement")],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("audited rewrite");
        let audited_graph = live
            .metadata()
            .get(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .cloned()
            .expect("audited graph");
        assert!(
            live.transcript_history_state()
                .expect("history state decodes")
                .is_some(),
            "fixture must carry a retained transcript graph"
        );
        // Warm up: the first append after a rewrite reseeds, later appends are
        // the steady state every subsequent turn sees.
        for warmup in 0..3 {
            live.push(user(&format!("warm-up {warmup}")));
        }

        let before = session_content_digest_computations();
        live.push(user("one word"));
        let after = session_content_digest_computations() - before;

        // The audited graph stays byte-identical while the live revision
        // advances independently through the transcript accumulator.
        let state = live
            .transcript_history_state()
            .expect("history state decodes")
            .expect("history state present");
        assert_eq!(
            state.head(),
            rewrite.revision,
            "graph head must remain the latest audited rewrite endpoint"
        );
        assert_ne!(
            state.head(),
            live.transcript_content_digest().expect("live digest"),
            "ordinary append must advance live identity without manufacturing a graph head"
        );
        assert_eq!(
            live.metadata()
                .get(meerkat_core::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            Some(&audited_graph),
            "ordinary append must leave graph bytes untouched"
        );
        live.validate_transcript_history_state()
            .expect("graph must still validate after the fast-path appends");
        after
    }

    let small = measure(SMALL);
    let large = measure(LARGE);
    println!(
        "history-bearing append full-document digest passes: {SMALL} turns => {small}, {LARGE} turns => {large}"
    );
    assert_eq!(
        small, large,
        "appending to a history-bearing session must not scale with the retained graph"
    );
}

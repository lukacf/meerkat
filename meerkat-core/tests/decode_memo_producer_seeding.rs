//! Producer-seeding mutation proof for the validated transcript-graph decode
//! memo.
//!
//! The validated decode memo used to be populated only by the decode path
//! itself, so the first decode of a graph the SAME process had just proven
//! and persisted was a guaranteed first-sight miss — a full O(document)
//! canonical-JSON + SHA-256 validation per load. The producer seams
//! (`set_validated_transcript_history_metadata_with_state` on the append
//! path, the rewrite-commit seam, and the `FullVerify` branch of
//! `compact_transcript_history_metadata_for_snapshot`) now admit the graph
//! they just proved under the same `TRANSCRIPT_GRAPH_FACT_VALIDATED` shape
//! key the decode consults, so the next decode of those exact bytes
//! SUBSTITUTES the proven graph without hashing a single content byte.
//!
//! Pinned here, each independently:
//! - the whole win: a produce → encode → decode round trip charges ZERO
//!   bytes to the decode digest bucket (and zero digest bytes at all on the
//!   decoding thread) — deleting the producer-side recorder call, seeding
//!   under the wrong fact tag, or dropping any shape-key input turns this
//!   red;
//! - soundness: a hit substitutes the PROVEN graph, never blesses the
//!   incoming bytes — forged content under a still-matching shape key is
//!   discarded, and corrupting a key-pinned field misses the memo and fails
//!   the full validation that runs on the miss;
//! - the `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` kill-switch takes the memo out
//!   of the path end to end (proven in a child process, because the memo and
//!   the env read are process-global).

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult, UserMessage,
};
use meerkat_core::{
    DIGEST_SITE_LABELS, SESSION_TRANSCRIPT_HISTORY_STATE_KEY, Session, digest_site_bytes,
    session_content_digest_bytes,
};
use std::sync::{Mutex, MutexGuard, PoisonError};

const KILL_SWITCH_ENV: &str = "MEERKAT_DISABLE_GRAPH_DECODE_MEMO";

/// Survives only inside the rewrite commit's retained parent revision body
/// (the rewrite replaces the live message carrying it), so flipping it in the
/// persisted bytes forges graph-only content without touching the live
/// transcript. Same length as [`FORGED_BODY_MARKER`]: the shape key pins
/// message counts and every digest-erased field, none of which move under a
/// same-length text flip.
const RETAINED_BODY_MARKER: &str = "retained-body-canary-original-aaaa";
const FORGED_BODY_MARKER: &str = "retained-body-canary-original-aaab";

/// The digest-site counters and both decode memos are process-global.
/// `cargo nextest` runs each test in its own process, but plain `cargo test`
/// shares one process across this binary's tests, so every test that
/// measures the counters (or depends on its own memo entries) serializes on
/// this lock to keep a sibling's decode from landing inside a measurement
/// window.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial_guard() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn decode_site() -> usize {
    DIGEST_SITE_LABELS
        .iter()
        .position(|label| *label == "decode")
        .expect("decode bucket present in DIGEST_SITE_LABELS")
}

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

/// A session whose transcript graph carries one audited rewrite commit and
/// whose post-rewrite appends have driven the producer seeding seam: every
/// append refreshes the graph head through
/// `set_validated_transcript_history_metadata_with_state`, which seeds the
/// validated decode memo with exactly the state serialized into the durable
/// document.
fn session_with_producer_seeded_graph() -> Session {
    let mut session = Session::new();
    session.set_system_prompt("system".to_string());
    for index in 0..4 {
        session.push(user(&format!(
            "question {index} with some body text to make the message non-trivial"
        )));
        session.push(assistant(&format!(
            "answer {index} with some body text to make the message non-trivial"
        )));
    }
    session.push(user(RETAINED_BODY_MARKER));
    let end = session.messages().len();
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: end - 1,
                end,
            },
            vec![assistant("audited replacement")],
            TranscriptRewriteReason::new("edit"),
            Some("test".to_string()),
            None,
        )
        .expect("audited rewrite");
    for index in 0..3 {
        session.push(user(&format!("post-rewrite question {index}")));
        session.push(assistant(&format!("post-rewrite answer {index}")));
    }
    // A tool exchange whose args are pass-through `RawValue` in the
    // NORMALIZED spelling `deserialize_tool_use_args` emits (sorted keys,
    // compact) — the shape every decoded document and every wire-ingested
    // tool call carries. Riding this through the retained head body means
    // the kill-switch full-cost decode below re-hashes producer-serialized
    // RawValue bytes against their revision strings, pinning the round trip
    // a byte-identity review flagged as untested.
    session.push(tool_exchange_with_args(
        "toolu_rawvalue_pin",
        r#"{"alpha":{"nested":[1,2,3]},"zeta":1}"#,
    ));
    session.push(Message::tool_results(vec![ToolResult::new(
        "toolu_rawvalue_pin".to_string(),
        "raw probe complete".to_string(),
        false,
    )]));
    session
}

fn tool_exchange_with_args(id: &str, raw_args: &str) -> Message {
    Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::ToolUse {
            id: id.to_string(),
            name: "raw_probe".to_string(),
            args: serde_json::value::RawValue::from_string(raw_args.to_string())
                .expect("valid raw tool args"),
            meta: None,
        }],
        StopReason::ToolUse,
    ))
}

fn proven_graph_value(session: &Session) -> serde_json::Value {
    serde_json::to_value(
        session
            .transcript_history_state()
            .expect("graph decodes")
            .expect("fixture carries a transcript graph"),
    )
    .expect("graph encodes")
}

/// The whole win: the producer admitted the exact persisted shape, so the
/// next decode substitutes the proven graph and hashes NO content bytes.
/// Calibration when this was made honest: producer seeding on, the decode
/// bucket moves 0 bytes; with the producer-side recorder deleted (or keyed
/// under the heal-probe fact tag, or missing any key input) the decode pays
/// the full multi-pass graph validation again and the delta returns to
/// several whole-graph passes.
#[test]
fn producer_seeded_graph_is_substituted_on_the_next_decode() {
    let _serial = serial_guard();
    let session = session_with_producer_seeded_graph();
    let proven = proven_graph_value(&session);
    let bytes = serde_json::to_vec(&session).expect("encode session");

    let site = decode_site();
    let site_before = digest_site_bytes()[site];
    let thread_before = session_content_digest_bytes();
    let decoded: Session = serde_json::from_slice(&bytes).expect("decode session");
    let site_delta = digest_site_bytes()[site] - site_before;
    let thread_delta = session_content_digest_bytes() - thread_before;
    println!(
        "producer-seeded decode: decode-bucket {site_delta} bytes, \
         decoding-thread total {thread_delta} bytes"
    );
    assert_eq!(
        site_delta, 0,
        "the producer admitted the exact persisted graph shape, so the next \
         decode must substitute the proven graph without hashing any body"
    );
    // Stronger, and immune to attribution: a memo hit hashes nothing on the
    // decoding thread AT ALL, so no nested pass can hide the cost in a
    // sibling bucket (attribution is innermost-wins).
    assert_eq!(
        thread_delta, 0,
        "a substituted decode must charge zero content-digest bytes on the \
         decoding thread, in any bucket"
    );
    assert_eq!(
        proven_graph_value(&decoded),
        proven,
        "the decoded session must carry the graph the producer proved"
    );
}

/// Soundness pin: substitution, not blessing. Content is deliberately NOT in
/// the shape key (the revision strings are content addresses over it), so a
/// forged body text under a seeded key still matches — and the hit must
/// serve the PROVEN graph, discarding the forgery. A future refactor from
/// "substitute the memoized value" to "trust the incoming bytes on a hit"
/// fails here rather than in production.
#[test]
fn forged_body_under_a_seeded_key_is_not_served() {
    let _serial = serial_guard();
    let session = session_with_producer_seeded_graph();
    let proven = proven_graph_value(&session);
    let text =
        String::from_utf8(serde_json::to_vec(&session).expect("encode session")).expect("utf8");
    assert_eq!(
        text.matches(RETAINED_BODY_MARKER).count(),
        1,
        "fixture must retain the marker exactly once: inside the commit's \
         parent revision body"
    );
    let forged = text.replacen(RETAINED_BODY_MARKER, FORGED_BODY_MARKER, 1);
    assert_eq!(
        text.len(),
        forged.len(),
        "the flip must be same-length so every key-pinned count is unchanged"
    );

    match serde_json::from_slice::<Session>(forged.as_bytes()) {
        // A typed refusal is also acceptable: with the memo out of the path
        // (e.g. the kill-switch set in the environment) the full validator
        // catches the forged body's digest mismatch on the miss.
        Err(error) => println!("forged decode refused typed: {error}"),
        Ok(decoded) => {
            let decoded_graph = proven_graph_value(&decoded);
            assert_eq!(
                decoded_graph, proven,
                "a memo hit must SUBSTITUTE the proven graph wholesale, \
                 never bless the incoming bytes"
            );
            let decoded_text = serde_json::to_string(&decoded_graph).expect("graph re-encodes");
            assert!(
                !decoded_text.contains(FORGED_BODY_MARKER),
                "the forged text must not be served as validated content"
            );
            assert!(
                decoded_text.contains(RETAINED_BODY_MARKER),
                "the proven content must be what the decode serves"
            );
        }
    }
}

/// The complementary soundness pin: every body's `revision` string is pinned
/// by the shape key, so corrupting one misses the memo, the full validation
/// runs on the miss, and the corrupted content address fails it. Never `Ok`
/// with the corrupt value.
#[test]
fn corrupting_a_key_pinned_field_misses_the_memo_and_fails_validation() {
    let _serial = serial_guard();
    let session = session_with_producer_seeded_graph();
    let bytes = serde_json::to_vec(&session).expect("encode session");
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes).expect("document parses");
    let revision = doc
        .get_mut("metadata")
        .and_then(|metadata| metadata.get_mut(SESSION_TRANSCRIPT_HISTORY_STATE_KEY))
        .and_then(|graph| graph.get_mut("revisions"))
        .and_then(|revisions| revisions.get_mut(0))
        .and_then(|body| body.get_mut("revision"))
        .expect("first retained body carries a revision string");
    let mut corrupted = revision.as_str().expect("revision is a string").to_string();
    let last = corrupted.pop().expect("revision string is non-empty");
    corrupted.push(if last == '0' { '1' } else { '0' });
    *revision = serde_json::Value::String(corrupted);

    let corrupted_bytes = serde_json::to_vec(&doc).expect("re-encode corrupted document");
    let error = serde_json::from_slice::<Session>(&corrupted_bytes).expect_err(
        "a corrupted key-pinned revision string must miss the memo and \
         fail the full validation that runs on the miss",
    );
    println!("corrupted revision refused typed: {error}");
}

/// Child half of the kill-switch proof (idiom: `mcp_config_crossproc.rs`).
/// A no-op under a normal suite run; the orchestrating test below re-invokes
/// this binary with the kill-switch set from process birth, under which
/// producer seeding and memo lookups are both disabled and the decode must
/// pay the full graph validation again — the red-first shape of the
/// substitution test above.
#[test]
fn kill_switch_probe_decodes_at_full_cost_child() {
    if std::env::var_os(KILL_SWITCH_ENV).is_none() {
        return;
    }
    let _serial = serial_guard();
    let session = session_with_producer_seeded_graph();
    let bytes = serde_json::to_vec(&session).expect("encode session");
    let site = decode_site();
    let before = digest_site_bytes()[site];
    let _decoded: Session = serde_json::from_slice(&bytes).expect("decode session");
    let delta = digest_site_bytes()[site] - before;
    println!("kill-switch decode-bucket digest bytes: {delta}");
    assert!(
        delta > 0,
        "with {KILL_SWITCH_ENV} set the memo must be out of the path: the \
         decode re-validates the graph and hashes content bytes"
    );
}

/// The kill-switch env read and both decode memos are process-global, so the
/// disabled state must be proven in a child process born with the switch
/// set — setting the variable mid-process would race sibling tests.
#[test]
fn kill_switch_disables_producer_seeding() {
    let executable = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(executable)
        .arg("--exact")
        .arg("kill_switch_probe_decodes_at_full_cost_child")
        .arg("--nocapture")
        .env(KILL_SWITCH_ENV, "1")
        .output()
        .expect("spawn kill-switch probe");
    assert!(
        output.status.success(),
        "kill-switch probe failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// PRE-EXISTING (0.8.8-parity) sharp edge, pinned as a typed FAIL-CLOSED
/// refusal rather than fixed here: a retained graph body whose `ToolUse`
/// args carry a NON-normalized `RawValue` spelling (unsorted keys, interior
/// whitespace) cannot survive an encode/decode cycle under full validation.
/// The transcript digest content-addresses the VERBATIM raw bytes, while
/// `deserialize_tool_use_args` must tolerate Message-level serde buffering
/// by round-tripping through `Value` — which re-spells the args (sorted
/// keys, compact) — so the decoded body re-hashes to a different digest
/// than its recorded revision string and the document refuses typed.
///
/// Neither seam changed in 0.8.9 (the digest canonicalizer and the args
/// deserializer are byte-identical to 0.8.8); wire-ingested and once-decoded
/// sessions always carry the normalized spelling and round trip cleanly
/// (pinned by the suite above). Making the two seams agree is tracked as a
/// follow-up — it requires a digest-semantics decision that must not ride a
/// release whose safety story is "digest values unchanged". If this test
/// starts PASSING the decode, that decision has been made: move the
/// construction into `session_with_producer_seeded_graph` and delete this.
#[test]
fn non_normalized_raw_args_in_a_retained_body_fail_closed_on_cold_decode() {
    let _serial = serial_guard();
    let mut session = session_with_producer_seeded_graph();
    session.push(tool_exchange_with_args(
        "toolu_rawvalue_sharp_edge",
        "{\"zeta\": 1,  \"alpha\":\t{\"nested\" :[1, 2,3]}}",
    ));
    session.push(Message::tool_results(vec![ToolResult::new(
        "toolu_rawvalue_sharp_edge".to_string(),
        "raw probe complete".to_string(),
        false,
    )]));
    let bytes = serde_json::to_vec(&session).expect("encode session");
    // Force the cold-reader path: strip the producer-seeded memo entry's
    // effect by corrupting nothing and instead decoding in a fresh shape —
    // the appended exchange changed the head, and the producer seeded THAT
    // shape too, so a warm decode would substitute and mask the edge. Use
    // the serialized bytes but flip one digest-erased field (a message
    // `created_at`) to re-key the memo without touching content.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("parse document");
    let history = value
        .get_mut("metadata")
        .and_then(|metadata| metadata.get_mut(SESSION_TRANSCRIPT_HISTORY_STATE_KEY))
        .expect("history graph present");
    let body_created_at = history
        .get_mut("revisions")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|revisions| revisions.first_mut())
        .and_then(|body| body.get_mut("created_at"))
        .and_then(|created| created.get_mut("secs_since_epoch"))
        .expect("first retained body timestamp");
    *body_created_at = serde_json::json!(1_000_000_000u64);
    let rekeyed = serde_json::to_vec(&value).expect("re-encode document");
    let error = serde_json::from_slice::<Session>(&rekeyed)
        .expect_err("non-normalized raw args must fail closed on a cold decode");
    assert!(
        error.to_string().contains("has digest"),
        "expected the typed revision-body digest refusal, got: {error}"
    );
}

//! Witness v3 migration contract (docs-internal/witness-v3-migration.md).
//!
//! The transcript-history witness has its own version axis (`witness_format`:
//! 2 = sequential whole-graph canonical hash, 3 = revision-identity digest)
//! carried by the typed [`TranscriptHistoryWitness`] carrier, and the
//! checkpoint stamp schema is the downgrade one-way door: a document whose
//! stamp digest folds a format-3 witness mints stamp schema 3, which the
//! published 0.8.8 reader refuses through its typed future-schema path.
//!
//! These tests pin the migration story through public API only: mint at
//! current, verify under the format the evidence declares, refuse unknown
//! formats typed at ingress before any healing, keep slim v2 rows v2, prove
//! every mutation of a v3-stamped document fails closed, and pin the v3
//! derivation's byte budget to O(revision count + commit log) — independent
//! of retained BODY bytes, never of revision count.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::checkpoint::{
    SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3, SessionCheckpointProvenance,
    SessionCheckpointStamp, SessionCheckpointState, TranscriptHistoryWitness,
    session_checkpoint_digest, session_transcript_history_checkpoint_digest,
    session_transcript_history_witness,
};
use meerkat_core::service::{TranscriptRewriteReason, TranscriptRewriteSelection};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
};
use meerkat_core::{
    SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION, SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY,
    SESSION_TRANSCRIPT_HISTORY_STATE_KEY, Session, session_content_digest_bytes,
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

/// A session with `turns` prior conversational turns, each message carrying
/// `body` so tests can control retained body bytes without changing the
/// message or revision count.
fn session_with_turns(turns: usize, body: &str) -> Session {
    let mut session = Session::new();
    session.set_system_prompt("system".to_string());
    for index in 0..turns {
        session.push(user(&format!("question {index}: {body}")));
        session.push(assistant(&format!("answer {index}: {body}")));
    }
    session
}

/// Rewrite the last message so the session retains a transcript graph:
/// one commit plus its two audited endpoint revision bodies.
fn rewrite_last(session: &mut Session) {
    let end = session.messages().len();
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: end - 1,
                end,
            },
            vec![assistant("audited replacement")],
            TranscriptRewriteReason::new("witness-v3-test"),
            Some("witness-v3-test".to_string()),
            None,
        )
        .expect("audited rewrite");
}

/// A graph-bearing session with message bodies of the given size.
fn graph_session(body: &str) -> Session {
    let mut session = session_with_turns(6, body);
    rewrite_last(&mut session);
    assert!(
        session
            .transcript_history_state()
            .expect("history state decodes")
            .is_some(),
        "fixture must carry a retained transcript graph"
    );
    session
}

/// A graph-bearing session with a freshly minted-and-installed v3 root stamp.
fn v3_stamped_session() -> Session {
    let mut session = graph_session("with some body text to make the message non-trivial");
    let stamp = SessionCheckpointStamp::root(&session, SessionCheckpointProvenance::SessionCreated)
        .expect("mint graph-bearing root stamp");
    assert_eq!(
        stamp.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3,
        "a graph-bearing mint must fold the v3 witness and advertise stamp schema 3"
    );
    session
        .install_checkpoint_stamp(stamp)
        .expect("install v3 stamp");
    session
}

fn document_of(session: &Session) -> serde_json::Value {
    serde_json::to_value(session).expect("serialize session document")
}

fn decode(document: serde_json::Value) -> Result<Session, serde_json::Error> {
    serde_json::from_value(document)
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).unwrap_or_else(|error| {
        panic!("fixture {name} must be committed under meerkat-core/tests/fixtures: {error}")
    })
}

/// The committed manifest records ONE digest pair for both fixture
/// projections: the checkpoint digest is storage-representation-invariant
/// (full and slim documents of the same session fold the same witness
/// marker), so the full document and its head-canonical slim row share both
/// values.
fn manifest_digest(manifest: &serde_json::Value, field: &str) -> String {
    manifest[field]
        .as_str()
        .unwrap_or_else(|| panic!("manifest[{field}] must be a digest string"))
        .to_string()
}

/// Obligation 1: A graph-bearing document mints at current: format-3 witness, stamp
/// schema 3, and the resulting document verifies. A session with NO graph
/// keeps minting schema 1, so plain sessions stay downgrade-readable.
#[test]
fn graph_bearing_mint_advances_to_witness_v3_stamp_schema() {
    let mut session = graph_session("with some body text");
    let stamp = SessionCheckpointStamp::root(&session, SessionCheckpointProvenance::SessionCreated)
        .expect("mint graph-bearing root stamp");
    assert_eq!(
        stamp.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3
    );
    session
        .install_checkpoint_stamp(stamp)
        .expect("install v3 stamp");
    match session.try_checkpoint_state().expect("checkpoint state") {
        SessionCheckpointState::Verified(stamp) => assert_eq!(
            stamp.schema_version(),
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3
        ),
        other => panic!("v3-stamped document must verify, got {other:?}"),
    }

    let mut plain = session_with_turns(3, "with some body text");
    let stamp = SessionCheckpointStamp::root(&plain, SessionCheckpointProvenance::SessionCreated)
        .expect("mint plain root stamp");
    assert_eq!(
        stamp.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
        "a session with no rewrite graph must keep minting schema 1 so plain \
         sessions stay readable by pre-v3 binaries after a downgrade"
    );
    plain
        .install_checkpoint_stamp(stamp)
        .expect("install plain stamp");
    match plain.try_checkpoint_state().expect("checkpoint state") {
        SessionCheckpointState::Verified(stamp) => {
            assert_eq!(
                stamp.schema_version(),
                SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION
            );
        }
        other => panic!("plain stamped document must verify, got {other:?}"),
    }
}

/// Obligation 2: One-way-door pin. The published 0.8.8 reader accepts exactly
/// `schema_version == 1` for ordinary provenances and refuses anything newer
/// through its typed future-schema path (`UnsupportedSchemaVersion`) — never
/// unknown-enum corruption, never a silent v2 interpretation. The v3
/// constant advancing past 1 therefore IS the downgrade refusal: once a
/// session's stamp digest folds a format-3 witness, that session is one-way
/// per session, the same contract as every meerkat_schema ledger bump. If
/// either assertion below ever changes, the migration story's downgrade
/// safety has been redesigned, not merely renumbered.
#[test]
fn v3_stamp_schema_is_refused_by_the_pre_v3_reader_rule() {
    assert_eq!(SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3, 3);
    assert_eq!(SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION, 1);
    assert_ne!(
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3, SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
        "the 0.8.8 reader accepts only schema_version == 1; a v3 stamp must \
         not satisfy that check"
    );
}

/// Obligation 3: An unknown carried `witness_format` refuses typed at `Session`
/// deserialization — before graph normalization, healing, or any mutation of
/// the row could reinterpret it under an algorithm this binary predates.
#[test]
fn unknown_witness_format_refuses_at_decode_before_healing() {
    let session = graph_session("with some body text");
    let mut document = document_of(&session);
    document["metadata"][SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY] = serde_json::json!({
        "witness_format": 9,
        "revision_digest_format": 2,
        "digest": format!("sha256:{}", "0".repeat(64)),
    });
    let error = decode(document).expect_err("unknown witness format must refuse at decode");
    let text = error.to_string();
    assert!(
        text.contains("unsupported transcript-history witness format 9"),
        "refusal must name the unsupported witness format, got: {text}"
    );
}

/// Obligation 3 (sibling). An unknown `revision_digest_format` on the carrier refuses
/// the same way: the revision-string axis is independent and pinned at 2.
#[test]
fn unknown_revision_digest_format_refuses_at_decode() {
    let session = graph_session("with some body text");
    let mut document = document_of(&session);
    document["metadata"][SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY] = serde_json::json!({
        "witness_format": 3,
        "revision_digest_format": 3,
        "digest": format!("sha256:{}", "0".repeat(64)),
    });
    let error = decode(document).expect_err("unknown revision digest format must refuse at decode");
    let text = error.to_string();
    assert!(
        text.contains("unsupported transcript-history revision digest format 3"),
        "refusal must name the unsupported revision digest format, got: {text}"
    );
}

/// Obligation 3 (sibling). A carrier that is neither a digest string nor a typed object
/// is malformed and refuses typed — never laundered into absence.
#[test]
fn numeric_witness_carrier_refuses_as_malformed() {
    let session = graph_session("with some body text");
    let mut document = document_of(&session);
    document["metadata"][SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY] = serde_json::json!(9);
    let error = decode(document).expect_err("numeric witness carrier must refuse at decode");
    let text = error.to_string();
    assert!(
        text.contains("malformed transcript-history witness carrier"),
        "refusal must classify the carrier as malformed, got: {text}"
    );
}

/// Obligation 4: Every pre-v3 durable row carries a bare digest string; it normalizes
/// to the typed `{witness_format: 2, revision_digest_format: 2}` carrier and
/// persists back as the byte-identical bare string every pre-v3 reader
/// understands.
#[test]
fn bare_string_carrier_normalizes_to_v2_and_round_trips_bare() {
    let bare = serde_json::Value::String(format!("sha256:{}", "ab".repeat(32)));
    let witness =
        TranscriptHistoryWitness::from_carried_value(&bare).expect("bare string carrier parses");
    assert_eq!(witness.witness_format(), 2);
    assert_eq!(witness.revision_digest_format(), 2);
    assert_eq!(
        witness.digest().as_str(),
        bare.as_str().expect("bare carrier is a string")
    );
    assert_eq!(
        witness.to_carried_value(),
        bare,
        "a v2 witness must persist as the byte-identical bare string"
    );
}

/// Obligation 5: Full → slim → full: the typed v3 witness survives dropping the graph
/// (carried out of line on the slim row) and re-attaching it (the two
/// representations cross-check and resolve to the same typed value).
#[test]
fn v3_full_slim_full_round_trip_preserves_the_typed_witness() {
    let session = v3_stamped_session();
    let witness = session_transcript_history_witness(&session)
        .expect("witness resolves")
        .expect("graph-bearing session carries a witness");
    assert_eq!(witness.witness_format(), 3);
    assert_eq!(witness.revision_digest_format(), 2);

    let full = document_of(&session);
    let graph = full["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY].clone();
    assert!(graph.is_object(), "full document must carry the graph");

    let mut slim = full;
    slim["metadata"]
        .as_object_mut()
        .expect("metadata object")
        .remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    slim["metadata"][SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY] = witness.to_carried_value();
    let slim_session = decode(slim.clone()).expect("slim document decodes");
    let slim_witness = session_transcript_history_witness(&slim_session)
        .expect("slim witness resolves")
        .expect("slim row carries the witness");
    assert_eq!(slim_witness.witness_format(), 3);
    assert_eq!(slim_witness.revision_digest_format(), 2);
    assert_eq!(slim_witness.digest(), witness.digest());

    let mut full_again = slim;
    full_again["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY] = graph;
    let full_again_session = decode(full_again).expect("full-again document decodes");
    let round_tripped = session_transcript_history_witness(&full_again_session)
        .expect("full-again witness resolves")
        .expect("full-again document carries the witness");
    assert_eq!(round_tripped.witness_format(), 3);
    assert_eq!(round_tripped.digest(), witness.digest());
}

/// Obligation 6: A slim v2 projection can never relabel itself v3: it lacks the
/// retained bodies an authority would need to reconstruct and validate the
/// complete graph first. Every public path — resolution, persistence, and
/// even a mint-at-current stamp — keeps the row at format 2 / schema 1.
#[test]
fn slim_v2_row_can_never_relabel_itself_v3() {
    let bare_digest = format!("sha256:{}", "cd".repeat(32));
    let plain = session_with_turns(3, "with some body text");
    let mut document = document_of(&plain);
    assert!(
        document["metadata"]
            .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .is_none(),
        "slim fixture must carry no graph"
    );
    document["metadata"][SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY] =
        serde_json::Value::String(bare_digest.clone());
    let slim = decode(document).expect("slim v2 document decodes");

    let witness = session_transcript_history_witness(&slim)
        .expect("witness resolves")
        .expect("slim row carries the witness");
    assert_eq!(witness.witness_format(), 2);
    assert_eq!(
        witness.to_carried_value(),
        serde_json::Value::String(bare_digest),
        "the slim row must keep persisting the bare v2 string"
    );

    // Mint-at-current is the most upgrade-eager public path; even it keeps
    // the carried v2 format and therefore the schema-1 stamp on a slim row.
    let stamp = SessionCheckpointStamp::root(&slim, SessionCheckpointProvenance::SessionCreated)
        .expect("mint slim root stamp");
    assert_eq!(
        stamp.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
        "no public path may yield a v3 stamp from a slim v2 row"
    );
}

/// Obligation 7: REAL 0.8.8 evidence (committed fixtures generated by real 0.8.8 code,
/// never minted by the code under test) keeps verifying under this
/// v3-writing binary: v2 evidence verifies with the v2 computation
/// indefinitely — mixed stores, no flag day.
#[test]
fn v2_evidence_keeps_verifying_under_the_v3_binary() {
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_fixture("v0_8_8_manifest.json")).expect("manifest decodes");

    let manifest_session_id = manifest["session_id"]
        .as_str()
        .expect("manifest names the fixture session id");

    let full: Session = serde_json::from_slice(&read_fixture("v0_8_8_full_session.json"))
        .expect("full v0.8.8 fixture decodes under the v3 binary");
    assert_eq!(full.id().to_string(), manifest_session_id);
    match full.try_checkpoint_state().expect("full fixture verifies") {
        SessionCheckpointState::Verified(stamp) => assert_eq!(
            stamp.schema_version(),
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
            "0.8.8 evidence must still advertise stamp schema 1"
        ),
        other => panic!("full v0.8.8 fixture must verify, got {other:?}"),
    }
    assert_eq!(
        session_checkpoint_digest(&full)
            .expect("recompute checkpoint digest")
            .as_str(),
        manifest_digest(&manifest, "checkpoint_digest"),
        "recomputed checkpoint digest must equal the 0.8.8-recorded value"
    );
    assert_eq!(
        session_transcript_history_checkpoint_digest(&full)
            .expect("witness resolves")
            .expect("full fixture carries a graph")
            .as_str(),
        manifest_digest(&manifest, "history_witness"),
        "recomputed v2 history witness must equal the 0.8.8-recorded value"
    );

    let slim: Session = serde_json::from_slice(&read_fixture("v0_8_8_slim_session.json"))
        .expect("slim v0.8.8 fixture decodes under the v3 binary");
    assert_eq!(slim.id().to_string(), manifest_session_id);
    match slim.try_checkpoint_state().expect("slim fixture verifies") {
        SessionCheckpointState::Verified(stamp) => {
            assert_eq!(
                stamp.schema_version(),
                SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION
            );
        }
        other => panic!("slim v0.8.8 fixture must verify, got {other:?}"),
    }
    assert_eq!(
        session_checkpoint_digest(&slim)
            .expect("recompute slim checkpoint digest")
            .as_str(),
        manifest_digest(&manifest, "checkpoint_digest"),
    );
    let witness = session_transcript_history_witness(&slim)
        .expect("slim witness resolves")
        .expect("slim fixture carries the out-of-line witness");
    assert_eq!(witness.witness_format(), 2);
    assert_eq!(
        witness.digest().as_str(),
        manifest_digest(&manifest, "history_witness"),
    );
}

/// Decode-or-verify refusal for one mutation of a serialized v3-stamped
/// document. Ingress may catch the mutation typed (graph validation); when
/// it does not, the checkpoint digest comparison must — a mutated document
/// must never verify.
fn assert_mutation_never_verifies(document: serde_json::Value, what: &str) {
    match decode(document) {
        Err(error) => println!("{what}: refused typed at decode ingress: {error}"),
        Ok(session) => {
            let state = session.try_checkpoint_state();
            assert!(
                !matches!(state, Ok(SessionCheckpointState::Verified(_))),
                "{what}: a mutated v3-stamped document must never verify"
            );
            match state {
                Err(error) => println!("{what}: refused at verification: {error}"),
                Ok(other) => println!("{what}: did not verify: {other:?}"),
            }
        }
    }
}

/// Obligation 8: Mutation proofs for the three facts the v3 witness pins: the head
/// revision, the commit log, and the retained revision-ID set. Each mutation
/// of the serialized document either refuses typed at ingress or fails the
/// checkpoint digest — never a verified read.
#[test]
fn v3_stamped_document_mutations_never_verify() {
    let session = v3_stamped_session();
    let document = document_of(&session);
    // Control: the unmutated round trip still verifies, so every refusal
    // below is caused by its mutation and nothing else.
    let control = decode(document.clone()).expect("unmutated document decodes");
    assert!(matches!(
        control.try_checkpoint_state().expect("control verifies"),
        SessionCheckpointState::Verified(_)
    ));

    let mut head_flip = document.clone();
    head_flip["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["head"] =
        serde_json::json!(format!("sha256:{}", "0123456789abcdef".repeat(4)));
    assert_mutation_never_verifies(head_flip, "graph head flip");

    let mut commit_drop = document.clone();
    let commits = commit_drop["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["commits"]
        .as_array_mut()
        .expect("commit log array");
    assert!(!commits.is_empty(), "fixture must carry a commit");
    commits.pop();
    assert_mutation_never_verifies(commit_drop, "commit-log removal");

    let mut body_drop = document;
    let revisions = body_drop["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["revisions"]
        .as_array_mut()
        .expect("retained revisions array");
    assert!(
        revisions.len() >= 2,
        "fixture must retain both audited rewrite endpoints"
    );
    revisions.remove(0);
    assert_mutation_never_verifies(body_drop, "retained revision removal");
}

/// Flip one message's text inside the first retained revision body of the
/// serialized document.
fn flip_retained_body_text(document: &mut serde_json::Value) {
    let revisions = document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["revisions"]
        .as_array_mut()
        .expect("retained revisions array");
    let message = revisions[0]["messages"]
        .as_array_mut()
        .expect("retained body messages")
        .iter_mut()
        .find(|message| message["role"] == "user" && message["content"].is_string())
        .expect("retained body carries a text user message");
    let text = message["content"]
        .as_str()
        .expect("text user message content")
        .to_string();
    message["content"] = serde_json::json!(format!("{text} flipped"));
}

/// Obligation 8 (retained-body byte flip). Seal-at-ingress owns body bytes now — that
/// IS the design: the v3 witness deliberately stops re-verifying body bytes
/// per derivation because ingress verifies every retained body against its
/// revision string, so the integrity budget moved to ingress rather than
/// vanished. A flipped body byte must therefore die typed at decode, before
/// any digest comparison is reached.
///
/// The flip alone cannot reach the validator in the producer's own process:
/// the decode memo's shape key binds revision strings and the digest-ERASED
/// bookkeeping fields, never digest-visible content (a valid document pins
/// content transitively through its revision strings), so a pure content
/// flip key-collides with the graph this process already proved and decode
/// substitutes the proven content instead of validating the incoming bytes
/// (see the pure-flip sibling below). To exercise the fresh-reader ingress
/// path — the deployment case the design pins, a reader with no memo of
/// this graph — the flip here also bumps the tampered body's `created_at`,
/// a field the contract explicitly classifies as storage bookkeeping, not
/// transcript identity: it changes the memo key but is not validated, so
/// the typed refusal below can only come from the content flip.
#[test]
fn retained_body_byte_flip_refuses_typed_at_ingress() {
    let session = v3_stamped_session();
    let mut document = document_of(&session);
    flip_retained_body_text(&mut document);
    let created_at_secs = document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["revisions"]
        [0]["created_at"]["secs_since_epoch"]
        .as_u64()
        .expect("retained body created_at seconds");
    document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["revisions"][0]["created_at"]["secs_since_epoch"] =
        serde_json::json!(created_at_secs + 1);
    let error = decode(document)
        .expect_err("a retained-body byte flip must refuse typed at decode ingress");
    let text = error.to_string();
    assert!(
        text.contains("transcript revision body"),
        "refusal must name the revision-body digest mismatch, got: {text}"
    );
    println!("retained-body byte flip refused: {error}");
}

/// Obligation 8 (pure retained-body byte flip, producer process). In the process that
/// proved the clean graph, a pure content flip key-collides with the
/// process-lifetime decode memo (its shape key deliberately omits
/// digest-visible content) and decode SUBSTITUTES the previously proven
/// graph rather than validating the incoming bodies — the memo's contract
/// is "never trust the incoming bodies", and substitution serves only
/// content this process fully validated. The tampered bytes must therefore
/// never surface: decode either refuses typed (the fresh-reader behavior,
/// and what a content-binding memo key would produce) or yields a session
/// whose graph is byte-for-byte the proven one. Both outcomes keep the
/// tamper out of the process; this test accepts exactly those two and
/// nothing else.
#[test]
fn pure_retained_body_byte_flip_never_serves_tampered_bytes() {
    let session = v3_stamped_session();
    let clean_document = document_of(&session);
    let clean_graph = clean_document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY].clone();
    let mut tampered = clean_document;
    flip_retained_body_text(&mut tampered);
    match decode(tampered) {
        Err(error) => println!("pure body byte flip refused typed at ingress: {error}"),
        Ok(decoded) => {
            let served = document_of(&decoded);
            assert_eq!(
                served["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY], clean_graph,
                "a decode that accepts a content-flipped document may only be serving \
                 the previously proven graph, never the tampered bytes"
            );
            assert!(
                !serde_json::to_string(&served)
                    .expect("re-encode decoded session")
                    .contains(" flipped"),
                "tampered body bytes must never survive into the decoded session"
            );
        }
    }
}

/// Obligation 9: Mixed v2 and v3 sessions coexist in one process: real 0.8.8 evidence
/// verifies under the v2 computation while a freshly minted v3 session
/// verifies under the v3 computation, interleaved, with no cross-talk.
#[test]
fn mixed_v2_and_v3_sessions_coexist_in_one_process() {
    let v2: Session = serde_json::from_slice(&read_fixture("v0_8_8_full_session.json"))
        .expect("v0.8.8 fixture decodes");
    match v2.try_checkpoint_state().expect("v2 verifies") {
        SessionCheckpointState::Verified(stamp) => {
            assert_eq!(
                stamp.schema_version(),
                SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION
            );
        }
        other => panic!("v2 fixture must verify, got {other:?}"),
    }

    let v3 = v3_stamped_session();
    match v3.try_checkpoint_state().expect("v3 verifies") {
        SessionCheckpointState::Verified(stamp) => assert_eq!(
            stamp.schema_version(),
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3
        ),
        other => panic!("v3 session must verify, got {other:?}"),
    }

    // Interleave: the v2 evidence must keep verifying after v3 activity, on
    // both the live instance and a fresh decode of the same bytes.
    assert!(matches!(
        v2.try_checkpoint_state().expect("v2 still verifies"),
        SessionCheckpointState::Verified(_)
    ));
    let v2_again: Session = serde_json::from_slice(&read_fixture("v0_8_8_full_session.json"))
        .expect("v0.8.8 fixture re-decodes");
    assert!(matches!(
        v2_again
            .try_checkpoint_state()
            .expect("fresh v2 decode verifies"),
        SessionCheckpointState::Verified(_)
    ));
    assert!(matches!(
        v3.try_checkpoint_state().expect("v3 still verifies"),
        SessionCheckpointState::Verified(_)
    ));
}

/// Obligation 10: Lazy conversion is one atomic document write that either lands or
/// does not: minting and installing the v3 successor on a copy leaves the
/// v2 authority document readable and verified — exactly the state a crash
/// or CAS failure during conversion leaves behind.
#[test]
fn lazy_conversion_leaves_the_v2_authority_readable() {
    let original: Session = serde_json::from_slice(&read_fixture("v0_8_8_full_session.json"))
        .expect("v0.8.8 fixture decodes");
    let old_stamp = match original.try_checkpoint_state().expect("v2 verifies") {
        SessionCheckpointState::Verified(stamp) => stamp,
        other => panic!("v2 fixture must verify, got {other:?}"),
    };
    assert_eq!(
        old_stamp.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION
    );

    let mut converted = original.clone();
    let successor = SessionCheckpointStamp::successor(
        &converted,
        &old_stamp,
        SessionCheckpointProvenance::RunBoundaryCommit,
    )
    .expect("mint-at-current successor");
    assert_eq!(
        successor.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3,
        "an authoritative full-graph write mints the v3 witness and its schema-3 stamp"
    );
    converted
        .install_checkpoint_stamp(successor)
        .expect("install v3 successor");
    assert!(matches!(
        converted
            .try_checkpoint_state()
            .expect("converted document verifies"),
        SessionCheckpointState::Verified(_)
    ));

    match original
        .try_checkpoint_state()
        .expect("the v2 authority must remain readable after the conversion attempt")
    {
        SessionCheckpointState::Verified(stamp) => assert_eq!(
            stamp.schema_version(),
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION,
            "the untouched v2 authority must keep its schema-1 stamp"
        ),
        other => panic!("the v2 authority must still verify, got {other:?}"),
    }
}

/// Obligation 11: The v3 derivation's byte budget is O(retained revision count + commit
/// log), never O(retained body BYTES). Independence is claimed from BODY
/// BYTES only — never from revision count: replacing bodies 20x larger while
/// holding the message, commit, and revision counts fixed must leave the
/// derivation's hashed bytes essentially unchanged.
#[test]
fn v3_witness_bytes_are_independent_of_retained_body_bytes() {
    fn v3_witness_derivation_bytes(body: &str) -> u64 {
        let mut session = session_with_turns(6, body);
        rewrite_last(&mut session);
        let stamp =
            SessionCheckpointStamp::root(&session, SessionCheckpointProvenance::SessionCreated)
                .expect("mint v3 root stamp");
        assert_eq!(
            stamp.schema_version(),
            SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3
        );
        session
            .install_checkpoint_stamp(stamp)
            .expect("install v3 stamp");
        // A FRESH deserialized copy per measurement: the per-session witness
        // memo absorbs repeat derivations, and the mint above already fed it
        // on `session` — measuring there would observe the memo, not the
        // derivation.
        let encoded = serde_json::to_string(&session).expect("encode session");
        let fresh: Session = serde_json::from_str(&encoded).expect("decode fresh copy");
        // The derivation below runs synchronously on this thread and is the
        // only digest work inside the window, so the thread-local counter
        // delta IS the witness-bucket byte cost of the v3 derivation — and,
        // unlike the process-global `digest_site_bytes` buckets, it cannot
        // be polluted by sibling tests running on other threads in the same
        // process.
        let before = session_content_digest_bytes();
        let witness = session_transcript_history_witness(&fresh)
            .expect("witness resolves")
            .expect("graph-bearing session carries a witness");
        assert_eq!(witness.witness_format(), 3);
        session_content_digest_bytes() - before
    }

    let small = v3_witness_derivation_bytes(&"s".repeat(32));
    let large = v3_witness_derivation_bytes(&"l".repeat(640));
    println!("v3 witness derivation bytes: small bodies => {small}, 20x bodies => {large}");
    assert!(
        small > 0,
        "the measured derivation must be a real (non-memoized) v3 computation"
    );
    assert!(
        large < small.saturating_mul(2) && small < large.saturating_mul(2),
        "the v3 witness derivation must hash O(revision count + commit log) bytes, \
         independent of retained body bytes (small bodies => {small}, 20x bodies => {large})"
    );
}

/// Codex-review P2: a transitional FULL document can retain a stale carried
/// v2 witness beside its graph (typed graph writes remove the carried key,
/// but hand-assembled or older-writer documents can carry both). The stamp
/// is the verification target, so its schema must outrank the stale carrier
/// at BOTH seams — the install slow path (the stamp being installed is not
/// yet in metadata) and document-evidence resolution afterwards — while the
/// carrier keeps being cross-checked under its own format. Before this pin,
/// the carrier won: a valid freshly minted schema-3 stamp was recomputed
/// under v2 and refused as a digest mismatch.
#[test]
fn schema3_stamp_installs_and_verifies_over_a_stale_carried_v2_witness() {
    let session = graph_session("carrier coexistence body");
    let state = session
        .transcript_history_state()
        .expect("graph decodes")
        .expect("graph present");
    let v2 = meerkat_core::checkpoint::transcript_history_checkpoint_digest(&state)
        .expect("v2 witness computes");
    let mut document = document_of(&session);
    document
        .get_mut("metadata")
        .and_then(serde_json::Value::as_object_mut)
        .expect("metadata object")
        .insert(
            SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY.to_string(),
            serde_json::Value::String(v2.as_str().to_string()),
        );

    let minted_on = decode(document.clone()).expect("coexistence document decodes");
    let stamp =
        SessionCheckpointStamp::root(&minted_on, SessionCheckpointProvenance::SessionCreated)
            .expect("mint over the coexistence document");
    assert_eq!(
        stamp.schema_version(),
        SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3,
        "a graph-bearing mint stays v3 regardless of the stale carrier"
    );

    // A separate, identical instance: no mint-time digest seal, so the
    // install takes the slow path and must verify under the stamp's schema,
    // not the carrier's.
    let mut installed_on = decode(document).expect("second instance decodes");
    installed_on
        .install_checkpoint_stamp(stamp)
        .expect("schema-3 stamp must install despite the stale v2 carrier");

    // Document evidence afterwards resolves the same way: the stamped
    // coexistence document verifies (and the carrier is still checked under
    // its own format — corrupt it and this fails closed).
    assert!(
        matches!(
            installed_on
                .try_checkpoint_state()
                .expect("checkpoint state readable"),
            SessionCheckpointState::Verified(_)
        ),
        "the stamped coexistence document must verify under the stamp's format"
    );
}

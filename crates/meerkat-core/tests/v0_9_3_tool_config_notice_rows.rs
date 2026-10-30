//! Sessions stored by 0.9.3 that hold tool-config and MCP notices keep
//! loading and extending after the lifecycle bit was renamed `settled`
//! (#1658).
//!
//! The fixture holds message rows, session heads and whole session documents
//! written by meerkat-core 0.9.3 (see its `how` field and the generator next
//! to it). A session head revision and a session's row lineage are digests
//! over the re-serialized messages, so a decoded 0.9.3 notice has to serialize
//! back to exactly the bytes 0.9.3 wrote: the load seams, the whole-document
//! decode, the append save guard and a compaction rewrite from the stored head
//! all compare against what 0.9.3 stored.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::session_store::{
    SaveGuardWitness, append_only_save_guard_with_witness, validate_commit_rewrite_transition,
};
use meerkat_core::types::{Message, UserMessage};
use meerkat_core::{
    AgentEvent, Session, SessionHead, SessionHeadCas, TranscriptRevisionBody,
    TranscriptRewriteCommit, TranscriptRewriteReason, TranscriptRewriteRecord,
    TranscriptRewriteSelection, session_head_cas_token, transcript_messages_digest,
};
use sha2::{Digest, Sha256};

const FIXTURE: &str = include_str!("fixtures/v0_9_3_tool_config_notice_sessions.json");
const FIXTURE_SHA256: &str = "63edee270610a9b8a05e2c9ee4d9a151274bafe6740bb893a19f84482c9c5c73";

struct Case {
    name: String,
    head: SessionHead,
    rows: Vec<String>,
    session_document: String,
    events: Vec<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").unwrap();
    }
    encoded
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

fn cases() -> Vec<Case> {
    assert_eq!(sha256_hex(FIXTURE.as_bytes()), FIXTURE_SHA256);
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture["written_by"], "meerkat-core 0.9.3 (tag v0.9.3)");
    let cases: Vec<Case> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| Case {
            name: case["case"].as_str().unwrap().to_string(),
            head: serde_json::from_value(case["head"].clone()).unwrap(),
            rows: strings(&case["rows"]),
            session_document: case["session_document"].as_str().unwrap().to_string(),
            events: strings(&case["events"]),
        })
        .collect();
    assert_eq!(cases.len(), 12);
    cases
}

fn raw_rows(case: &Case) -> Vec<Vec<u8>> {
    case.rows
        .iter()
        .map(|row| row.clone().into_bytes())
        .collect()
}

fn decoded(case: &Case) -> Vec<Message> {
    case.rows
        .iter()
        .map(|row| serde_json::from_str(row).unwrap())
        .collect()
}

/// The session a store rebuilds from the rows, without the head check, so
/// the save guard and the rewrite are each exercised on their own.
fn session_from_rows(case: &Case) -> Session {
    let mut session = Session::with_id(case.head.id.clone());
    for message in decoded(case) {
        session.push(message);
    }
    session
}

/// The cases whose 0.9.3 rows carry a tool-config or MCP lifecycle bit.
fn notice_cases() -> Vec<Case> {
    cases()
        .into_iter()
        .filter(|case| case.name != "plain")
        .collect()
}

/// Every case is checked; the failures are reported together.
fn assert_no_failures(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn v0_9_3_notice_rows_and_events_reserialize_byte_exactly() {
    let mut failures = Vec::new();
    for case in cases() {
        for (index, row) in case.rows.iter().enumerate() {
            let message: Message = serde_json::from_str(row).unwrap();
            let written = serde_json::to_string(&message).unwrap();
            if &written != row {
                failures.push(format!("{} row {index}: wrote {written}", case.name));
            }
        }
        for (index, event) in case.events.iter().enumerate() {
            let decoded_event: AgentEvent = serde_json::from_str(event).unwrap();
            let written = serde_json::to_string(&decoded_event).unwrap();
            if &written != event {
                failures.push(format!("{} event {index}: wrote {written}", case.name));
            }
        }
    }
    assert_no_failures(&failures);
}

#[test]
fn v0_9_3_heads_load_through_the_store_seam() {
    let mut failures = Vec::new();
    for case in cases() {
        let digest = transcript_messages_digest(&decoded(&case)).unwrap();
        if digest != case.head.head_revision {
            failures.push(format!(
                "{}: rows digest {digest}, stored head revision {}",
                case.name, case.head.head_revision
            ));
        }
        match case
            .head
            .clone()
            .into_session_from_serialized_rows(raw_rows(&case))
        {
            Ok(session) => assert_eq!(session.messages().len(), case.rows.len(), "{}", case.name),
            Err(error) => failures.push(format!("{} load: {error:?}", case.name)),
        }
        if let Err(error) = case.head.clone().verify_serialized_rows(raw_rows(&case)) {
            failures.push(format!("{} verify: {error:?}", case.name));
        }
        // The typed seam serializes the decoded messages again for the row
        // lineage as well as for the digest.
        if let Err(error) = case.head.clone().into_session(decoded(&case)) {
            failures.push(format!("{} typed load: {error:?}", case.name));
        }
    }
    assert_no_failures(&failures);
}

#[test]
fn v0_9_3_session_documents_load_and_reserialize_byte_exactly() {
    let mut failures = Vec::new();
    for case in cases() {
        match serde_json::from_str::<Session>(&case.session_document) {
            Ok(session) => {
                assert_eq!(session.messages().len(), case.rows.len(), "{}", case.name);
                let written = serde_json::to_string(&session).unwrap();
                if written != case.session_document {
                    failures.push(format!("{} document: wrote {written}", case.name));
                }
            }
            Err(error) => failures.push(format!("{} document load: {error}", case.name)),
        }
    }
    assert_no_failures(&failures);
}

/// Golden: a document written by a released binary, without any
/// operator-instruction fact, reloads and writes back at envelope 4 with the
/// exact bytes the released binary wrote, and its head is projected at
/// envelope 4 too. Only documents carrying an instruction fact move to
/// envelope 5, so a rollback to the released binaries reads every other
/// session as before.
#[test]
fn documents_without_instruction_facts_stay_envelope_4_byte_identical() {
    let mut failures = Vec::new();
    for case in cases() {
        let released: serde_json::Value = serde_json::from_str(&case.session_document).unwrap();
        assert_eq!(
            released["version"], 4,
            "{}: the fixture is envelope 4",
            case.name
        );
        assert_eq!(
            case.head.version, 4,
            "{}: the fixture head is envelope 4",
            case.name
        );
        let session = match serde_json::from_str::<Session>(&case.session_document) {
            Ok(session) => session,
            Err(error) => {
                failures.push(format!("{} document load: {error}", case.name));
                continue;
            }
        };
        let written = session.to_persisted_bytes().unwrap();
        if written != case.session_document.as_bytes() {
            failures.push(format!(
                "{} document: wrote {}",
                case.name,
                String::from_utf8_lossy(&written)
            ));
        }
        match meerkat_core::SessionHead::from_session(
            &session,
            meerkat_core::TranscriptStrandId::root(),
            0,
        ) {
            Ok(head) if head.version == 4 => {}
            Ok(head) => failures.push(format!("{} head: envelope {}", case.name, head.version)),
            Err(error) => failures.push(format!("{} head: {error:?}", case.name)),
        }
        match case.head.clone().into_session(decoded(&case)) {
            Ok(loaded) => match meerkat_core::SessionHead::from_session(
                &loaded,
                meerkat_core::TranscriptStrandId::root(),
                0,
            ) {
                Ok(head) if head.version == case.head.version => {}
                Ok(head) => failures.push(format!(
                    "{} reprojected head: envelope {}",
                    case.name, head.version
                )),
                Err(error) => failures.push(format!("{} reprojected head: {error:?}", case.name)),
            },
            Err(error) => failures.push(format!("{} typed load: {error:?}", case.name)),
        }
    }
    assert_no_failures(&failures);
}

#[test]
fn v0_9_3_sessions_extend_through_the_append_save_guard() {
    let mut failures = Vec::new();
    for case in notice_cases() {
        let previous = session_from_rows(&case);
        let mut incoming = previous.clone();
        incoming.push(Message::User(UserMessage::text("next".to_string())));
        if let Err(error) = append_only_save_guard_with_witness(
            &incoming,
            Some(&previous),
            SaveGuardWitness::none().with_previous_revision(&case.head.head_revision),
        ) {
            failures.push(format!("{}: {error:?}", case.name));
        }
    }
    assert_no_failures(&failures);
}

#[test]
fn v0_9_3_heads_accept_a_compaction_rewrite() {
    let mut failures = Vec::new();
    for case in notice_cases() {
        let parent = decoded(&case);
        let summary = vec![Message::User(UserMessage::text("summary".to_string()))];
        let mut revised = vec![parent[0].clone()];
        revised.extend(summary.iter().cloned());
        let parent_revision = case.head.head_revision.clone();
        let revision = transcript_messages_digest(&revised).unwrap();
        let now = std::time::SystemTime::now();
        let record = match TranscriptRewriteRecord::new(
            TranscriptRewriteCommit {
                rewrite_generation: 1,
                parent_revision: parent_revision.clone(),
                revision: revision.clone(),
                selection: TranscriptRewriteSelection::MessageRange {
                    start: 1,
                    end: parent.len(),
                },
                original_span_digest: transcript_messages_digest(&parent[1..]).unwrap(),
                replacement_digest: transcript_messages_digest(&summary).unwrap(),
                messages_before: parent.len(),
                messages_after: revised.len(),
                reason: TranscriptRewriteReason::new("compaction"),
                actor: Some("compactor".to_string()),
                committed_at: now,
            },
            TranscriptRevisionBody {
                revision: parent_revision.clone(),
                parent_revision: None,
                messages: parent.clone(),
                created_at: now,
            },
            TranscriptRevisionBody {
                revision,
                parent_revision: Some(parent_revision),
                messages: revised,
                created_at: now,
            },
        ) {
            Ok(record) => record,
            Err(error) => {
                failures.push(format!("{} record: {error:?}", case.name));
                continue;
            }
        };
        // The store digests its persisted parent rows; the head is the one
        // 0.9.3 stored.
        let parent_prefix_digest = transcript_messages_digest(&parent).unwrap();
        let token = session_head_cas_token(&case.head).unwrap();
        if let Err(error) = validate_commit_rewrite_transition(
            &case.head.id,
            &record,
            &case.head,
            &token,
            &SessionHeadCas::IfToken(token.clone()),
            &parent_prefix_digest,
        ) {
            failures.push(format!("{} transition: {error:?}", case.name));
        }
    }
    assert_no_failures(&failures);
}

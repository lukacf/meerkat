//! A session document without operator-instruction facts stays
//! byte-identical at envelope 4 on every store kind a realm uses: SQLite
//! realms (the HeadCanonical head and rows), JSONL realms (the SQLite
//! WholeBlob runtime store) and host store realms (the host store adapter
//! over the in-memory test host). A document that carries an
//! operator-instruction entry row, or an instruction activation row whose
//! body a compaction retired, is written at envelope 5 on each of them and
//! reads back unchanged.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::lifecycle::{RunApplyBoundary, RunBoundaryReceipt, RunId};
use meerkat_core::session_store::{IncrementalSessionStore, SessionStore};
use meerkat_core::types::{Message, UserMessage};
use meerkat_core::{Session, TranscriptRewriteCommit};

use crate::LogicalRuntimeId;
use crate::store::spi::test_host::TestHost;
use crate::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use crate::store::{
    PreparedRuntimeSessionCommit, RuntimeStore, SerializedSessionSnapshot, WholeBlobSessionImport,
};

fn envelope_version(bytes: &[u8]) -> u64 {
    serde_json::from_slice::<serde_json::Value>(bytes).unwrap()["version"]
        .as_u64()
        .unwrap()
}

fn plain_session() -> Session {
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session.push(Message::User(UserMessage::text("hello")));
    session
}

fn session_with_entry() -> Session {
    let mut session = plain_session();
    session
        .append_operator_instruction_activation_for_test(
            "app.example",
            "tone",
            1,
            "Answer in a formal tone.",
        )
        .expect("an operator-instruction entry row");
    session
}

/// The bytes a JSONL realm's runtime store (SQLite, WholeBlob) commits.
///
/// The document lands the way a run's terminal boundary commits it: one
/// prepared success boundary carrying the sealed session, the run's receipt
/// and the owner session key. That boundary records the compaction
/// projection intents the document carries in the store's outbox in the
/// same transaction; a snapshot committed outside a boundary is refused for
/// an intent the outbox does not hold yet.
async fn sqlite_whole_blob_bytes(session: &Session) -> Vec<u8> {
    let dir = tempfile::TempDir::new().unwrap();
    let store = crate::SqliteRuntimeStore::new_whole_blob(dir.path().join("runtime.sqlite3"))
        .expect("whole-blob runtime store");
    let runtime_id = LogicalRuntimeId::for_session(session.id());
    let receipt = RunBoundaryReceipt {
        run_id: RunId::new(),
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids: Vec::new(),
        conversation_digest: Some(session.transcript_content_digest().unwrap()),
        message_count: session.messages().len(),
        sequence: 1,
        owner_contributions: Vec::new(),
    };
    RuntimeStore::commit_prepared_session_boundary(
        &store,
        &runtime_id,
        PreparedRuntimeSessionCommit::success(
            Some(BoundSessionCommit::sealed(Arc::new(session.clone())).unwrap()),
            receipt,
            Vec::new(),
            Some(session.id().clone()),
        ),
    )
    .await
    .expect("the run boundary commits");
    store
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .unwrap()
        .expect("the committed document")
        .bytes()
        .to_vec()
}

/// The bytes a host store realm commits (the host store adapter over the
/// in-memory test host).
async fn host_store_bytes(session: &Session) -> Vec<u8> {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("realm opens");
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("store opens");
    let runtime_id = LogicalRuntimeId::for_session(session.id());
    let outcome = store
        .import_whole_blob_session(
            &runtime_id,
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(session.to_persisted_bytes().unwrap()),
            },
        )
        .await
        .expect("the import lands");
    assert!(matches!(outcome, WholeBlobSessionImport::Created(_)));
    store
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .unwrap()
        .expect("the committed document")
        .bytes()
        .to_vec()
}

/// The envelope version a SQLite realm's head row carries, and the session
/// the head and its rows load back as.
async fn sqlite_head(session: &Session) -> (u32, Session) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = meerkat_store::SqliteSessionStore::open(dir.path().join("sessions.sqlite3"))
        .expect("head-canonical session store");
    SessionStore::save(&store, session)
        .await
        .expect("the session saves");
    let head = IncrementalSessionStore::load_head(&store, session.id())
        .await
        .unwrap()
        .expect("the head row");
    let loaded = SessionStore::load(&store, session.id())
        .await
        .unwrap()
        .expect("the session loads");
    (head.version, loaded)
}

/// [`sqlite_head`] for a compacted document. The store takes it the way a
/// compaction reaches a store: a plain first save of the uncompacted
/// document, then the compaction through the store's rewrite/audit path,
/// which checks the compacted document against the stored predecessor and
/// the compaction's own rewrite commit. A plain first save of a compacted
/// document is refused: it would seed transcript history state outside
/// that path.
async fn sqlite_head_after_compaction(
    uncompacted: &Session,
    compacted: &Session,
    commit: &TranscriptRewriteCommit,
) -> (u32, Session) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = meerkat_store::SqliteSessionStore::open(dir.path().join("sessions.sqlite3"))
        .expect("head-canonical session store");
    SessionStore::save(&store, uncompacted)
        .await
        .expect("the uncompacted document saves");
    SessionStore::save_transcript_rewrite(&store, compacted, commit)
        .await
        .expect("the compaction saves through the rewrite path");
    let head = IncrementalSessionStore::load_head(&store, compacted.id())
        .await
        .unwrap()
        .expect("the head row");
    let loaded = SessionStore::load(&store, compacted.id())
        .await
        .unwrap()
        .expect("the session loads");
    (head.version, loaded)
}

#[tokio::test]
async fn documents_without_instruction_facts_stay_byte_identical_on_every_store_kind() {
    let session = plain_session();
    let written = session.to_persisted_bytes().unwrap();
    assert_eq!(envelope_version(&written), 4);

    let jsonl = sqlite_whole_blob_bytes(&session).await;
    assert_eq!(jsonl, written, "a JSONL realm keeps the exact bytes");
    let host = host_store_bytes(&session).await;
    assert_eq!(host, written, "a host store realm keeps the exact bytes");
    for (kind, bytes) in [("jsonl", &jsonl), ("host store", &host)] {
        let reloaded = Session::from_persisted_bytes(bytes).unwrap();
        assert_eq!(
            reloaded.to_persisted_bytes().unwrap(),
            written,
            "{kind}: a reload writes back the same envelope-4 bytes"
        );
    }

    let (head_version, loaded) = sqlite_head(&session).await;
    assert_eq!(head_version, 4, "a SQLite realm's head stays at envelope 4");
    assert_eq!(loaded.messages(), session.messages());
    assert_eq!(envelope_version(&loaded.to_persisted_bytes().unwrap()), 4);

    // A document that carries an entry row moves to envelope 5 on each
    // store kind, and reads back unchanged.
    let with_entry = session_with_entry();
    let written = with_entry.to_persisted_bytes().unwrap();
    assert_eq!(envelope_version(&written), 5);
    assert_eq!(sqlite_whole_blob_bytes(&with_entry).await, written);
    assert_eq!(host_store_bytes(&with_entry).await, written);
    let (head_version, loaded) = sqlite_head(&with_entry).await;
    assert_eq!(head_version, 5);
    assert_eq!(loaded.messages(), with_entry.messages());
}

/// A compaction that retires no instruction activation body (the session's
/// only activation row is its key's newest) keeps the document at envelope
/// 4 and byte-identical on every store kind.
#[tokio::test]
async fn a_compaction_that_retires_nothing_keeps_the_document_byte_identical_on_every_store_kind() {
    let body = "Answer in a formal tone.";
    let mut session = plain_session();
    session.push(Message::User(UserMessage::text("and then?")));
    session
        .activate_instruction(meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::types::InstructionRevisionRef {
                namespace: meerkat_core::types::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::types::InstructionKey::new("tone").unwrap(),
                revision_id: meerkat_core::types::InstructionRevisionId::new("tone-r1").unwrap(),
                content_sha256: meerkat_core::types::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::types::InstructionActivationId::new("act-1").unwrap(),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        })
        .expect("an activation row");
    session.push(Message::User(UserMessage::text("and after that?")));
    // [prompt, user, user, activation, user] -> [prompt, summary, activation, user]
    let uncompacted = session.clone();
    let rows = session.messages().to_vec();
    let (commit, _) = session
        .stage_validated_compaction_for_test(
            vec![
                rows[0].clone(),
                Message::User(UserMessage::compaction_summary("summary")),
                rows[3].clone(),
                rows[4].clone(),
            ],
            1,
        )
        .expect("the compaction commits");
    assert_eq!(session.messages()[2], rows[3], "the activation row stays");

    let written = session.to_persisted_bytes().unwrap();
    assert_eq!(envelope_version(&written), 4);
    assert_eq!(
        sqlite_whole_blob_bytes(&session).await,
        written,
        "a JSONL realm keeps the exact bytes"
    );
    assert_eq!(
        host_store_bytes(&session).await,
        written,
        "a host store realm keeps the exact bytes"
    );
    let (head_version, loaded) =
        sqlite_head_after_compaction(&uncompacted, &session, &commit).await;
    assert_eq!(head_version, 4, "a SQLite realm's head stays at envelope 4");
    assert_eq!(loaded.messages(), session.messages());
}

/// `[prompt, user, activation, user, activation, user]`, the second
/// activation superseding the first, after a compaction that retired the
/// superseded body: `[prompt, summary, retired activation, activation,
/// user]`. Returned with the document before the compaction and the
/// compaction's rewrite commit.
fn session_with_a_retiring_compaction() -> (Session, Session, TranscriptRewriteCommit) {
    let activation = |id: &str, supersedes: Option<&str>, body: &str| {
        meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::types::InstructionRevisionRef {
                namespace: meerkat_core::types::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::types::InstructionKey::new("tone").unwrap(),
                revision_id: meerkat_core::types::InstructionRevisionId::new(format!("tone-{id}"))
                    .unwrap(),
                content_sha256: meerkat_core::types::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::types::InstructionActivationId::new(id).unwrap(),
            expectation: supersedes.map_or(
                meerkat_core::InstructionActivationExpectation::Absent,
                |previous| {
                    meerkat_core::InstructionActivationExpectation::Effective(
                        meerkat_core::types::InstructionActivationId::new(previous).unwrap(),
                    )
                },
            ),
            supersedes: supersedes.map(|previous| {
                meerkat_core::types::InstructionActivationId::new(previous).unwrap()
            }),
            body: body.to_string(),
        }
    };
    let mut session = plain_session();
    session
        .activate_instruction(activation("act-1", None, "Answer in a formal tone."))
        .expect("the first activation row");
    session.push(Message::User(UserMessage::text("and then?")));
    session
        .activate_instruction(activation(
            "act-2",
            Some("act-1"),
            "Answer in a friendly tone.",
        ))
        .expect("the superseding activation row");
    session.push(Message::User(UserMessage::text("and after that?")));
    let uncompacted = session.clone();
    let rows = session.messages().to_vec();
    let (commit, _) = session
        .stage_retiring_compaction_for_test(
            vec![
                rows[0].clone(),
                Message::User(UserMessage::compaction_summary("summary")),
                rows[2].clone(),
                rows[4].clone(),
                rows[5].clone(),
            ],
            1,
        )
        .expect("the retiring compaction commits");
    (uncompacted, session, commit)
}

/// A compaction that retires a superseded instruction activation body
/// writes the document at envelope 5 on every store kind: a SQLite realm's
/// head carries envelope 5 and its rows read back the retired row with its
/// marker, and a JSONL realm and a host store realm keep the exact bytes,
/// which reload to the same document and write back the same bytes.
#[tokio::test]
async fn a_retiring_compaction_round_trips_on_every_store_kind() {
    let (uncompacted, session, commit) = session_with_a_retiring_compaction();
    assert!(
        matches!(
            &session.messages()[2],
            Message::System(system)
                if system.instruction_activation.is_some()
                    && system.retired_instruction_body.is_some()
        ),
        "the compaction retired the superseded body"
    );
    let written = session.to_persisted_bytes().unwrap();
    assert_eq!(envelope_version(&written), 5);

    let jsonl = sqlite_whole_blob_bytes(&session).await;
    assert_eq!(jsonl, written, "a JSONL realm keeps the exact bytes");
    let host = host_store_bytes(&session).await;
    assert_eq!(host, written, "a host store realm keeps the exact bytes");
    for (kind, bytes) in [("jsonl", &jsonl), ("host store", &host)] {
        let reloaded = Session::from_persisted_bytes(bytes)
            .unwrap_or_else(|error| panic!("{kind}: the document reloads: {error}"));
        assert_eq!(
            reloaded.messages(),
            session.messages(),
            "{kind}: the retired row reads back with its marker"
        );
        assert_eq!(
            reloaded.to_persisted_bytes().unwrap(),
            written,
            "{kind}: a reload writes back the same envelope-5 bytes"
        );
    }

    let (head_version, loaded) =
        sqlite_head_after_compaction(&uncompacted, &session, &commit).await;
    assert_eq!(head_version, 5, "a SQLite realm's head is at envelope 5");
    assert_eq!(
        loaded.messages(),
        session.messages(),
        "the head's rows read back the retired row with its marker"
    );
    assert_eq!(envelope_version(&loaded.to_persisted_bytes().unwrap()), 5);
}

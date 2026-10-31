//! A session document without operator-instruction facts stays
//! byte-identical at envelope 4 on every store kind a realm uses: SQLite
//! realms (the HeadCanonical head and rows), JSONL realms (the SQLite
//! WholeBlob runtime store) and host store realms (the host store adapter
//! over the in-memory test host). A document that carries an
//! operator-instruction entry row is written at envelope 5 on each of them
//! and reads back unchanged.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::Session;
use meerkat_core::session_store::{IncrementalSessionStore, SessionStore};
use meerkat_core::types::{Message, UserMessage};

use crate::LogicalRuntimeId;
use crate::store::spi::test_host::TestHost;
use crate::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use crate::store::{RuntimeStore, SerializedSessionSnapshot, WholeBlobSessionImport};

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
async fn sqlite_whole_blob_bytes(session: &Session) -> Vec<u8> {
    let dir = tempfile::TempDir::new().unwrap();
    let store = crate::SqliteRuntimeStore::new_whole_blob(dir.path().join("runtime.sqlite3"))
        .expect("whole-blob runtime store");
    let runtime_id = LogicalRuntimeId::for_session(session.id());
    store
        .commit_session_snapshot(
            &runtime_id,
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(session.to_persisted_bytes().unwrap()),
            },
        )
        .await
        .expect("the snapshot commits");
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

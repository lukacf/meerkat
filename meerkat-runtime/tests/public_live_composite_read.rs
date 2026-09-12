#![cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]

use std::path::Path;
use std::sync::Arc;

use meerkat_contracts::wire::live_observation::{
    LiveObservationRecord, LiveObservationWireCodecV1,
};
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::{
    LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
};
use meerkat_core::{Message, Session};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::live_ledger::record::LiveLedgerRecord;
use meerkat_runtime::live_ledger::transcript::{LiveLedgerPrefixDigest, StoredLiveObservation};
use meerkat_runtime::store::live_read::{
    LiveCompositeRead, LiveCompositeReadRequest, read_live_composite,
};
use meerkat_runtime::store::{
    InMemoryRuntimeStore, RuntimeStore, SerializedSessionSnapshot, SqliteRuntimeStore,
};
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn orphan_live_rows_never_become_absent_history_with_or_without_an_actor() -> TestResult {
    for keep_actor in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        let store = SqliteRuntimeStore::new_whole_blob(&path)?;
        let session = session();
        save_actor(&store, &session).await?;
        seed_live_records(
            &path,
            &session,
            &[observation(1, "channel", "retained".into())?],
        )?;
        let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        conn.execute(
            "DELETE FROM runtime_live_heads WHERE session_id=?1",
            [session.id().to_string()],
        )?;
        if !keep_actor {
            conn.execute(
                "DELETE FROM runtime_whole_blob_authority WHERE runtime_id=?1",
                [LogicalRuntimeId::for_session(session.id()).to_string()],
            )?;
        }
        let result = read_live_composite(
            store.live_ledger_ops().ok_or("capability")?,
            LiveCompositeReadRequest::new(
                session.id().clone(),
                Some(LiveChannelId::new("channel")),
                0,
                64,
            )?,
        )
        .await;
        assert!(
            result.is_err(),
            "orphan rows must not turn into None or empty history"
        );
    }
    Ok(())
}

#[test]
fn record_union_preserves_exact_codec_units_and_rejects_unknown_or_mixed_shapes() -> TestResult {
    use meerkat_runtime::live_ledger::completion_budget::{
        maximal_completion_events, maximal_completion_record,
    };
    let record = observation(1, "voice", "same exact bytes".into())?;
    let bytes = record.encode()?;
    let decoded: LiveLedgerRecord = serde_json::from_slice(&bytes)?;
    assert_eq!(decoded.encode()?, bytes);
    if let LiveLedgerRecord::Observation(observation) = record {
        assert_eq!(
            LiveObservationWireCodecV1::encode_ledger_record(&observation)?,
            bytes
        );
    }
    let mut mixed: serde_json::Value = serde_json::from_slice(&bytes)?;
    mixed["event"] = serde_json::json!({"kind":"future"});
    assert!(serde_json::from_value::<LiveLedgerRecord>(mixed).is_err());
    assert!(
        serde_json::from_value::<LiveLedgerRecord>(
            serde_json::json!({"format":"live_ledger_v2","event":{"kind":"future"}})
        )
        .is_err()
    );
    for event in maximal_completion_events()? {
        let completion = maximal_completion_record(event)?;
        let expected = completion.encode()?.bytes().to_vec();
        let union = LiveLedgerRecord::Completion(completion);
        assert_eq!(
            LiveObservationWireCodecV1::encode_ledger_record(&union)?,
            expected
        );
        assert_eq!(union.encode()?, expected);
        let restored: LiveLedgerRecord = serde_json::from_slice(&expected)?;
        assert_eq!(restored.encode()?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn unsupported_backend_cannot_mint_composite_authority_from_capture_parts() -> TestResult {
    use meerkat_runtime::store::live_read::{
        LiveCompositeCapture, LiveCompositeReadProfile, RuntimeLiveLedgerOps,
    };
    struct Unsupported(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl RuntimeLiveLedgerOps for Unsupported {
        fn composite_read_profile(&self) -> LiveCompositeReadProfile {
            LiveCompositeReadProfile::Unsupported
        }
        async fn capture_live_composite(
            &self,
            _: &LiveCompositeReadRequest,
        ) -> Result<Option<LiveCompositeCapture>, meerkat_runtime::store::RuntimeStoreError>
        {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
    }
    let backend = Unsupported(std::sync::atomic::AtomicUsize::new(0));
    let result = read_live_composite(
        &backend,
        LiveCompositeReadRequest::new(Session::new().id().clone(), None, 0, 64)?,
    )
    .await;
    assert!(matches!(
        result,
        Err(meerkat_runtime::store::RuntimeStoreError::Unsupported(_))
    ));
    assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn whole_blob_capture_refuses_body_paired_with_another_committed_revision() -> TestResult {
    use meerkat_runtime::store::live_read::LiveCompositeCapture;
    let store = InMemoryRuntimeStore::new();
    let original = session();
    save_actor(&store, &original).await?;
    let snapshot = store
        .load_committed_whole_blob_snapshot(&LogicalRuntimeId::for_session(original.id()))
        .await?
        .ok_or("snapshot")?;
    let mut different = original.clone();
    different.push(Message::User(meerkat_core::UserMessage::text(
        "not that revision",
    )));
    assert!(
        LiveCompositeCapture::whole_blob(
            Arc::new(serde_json::to_vec(&different)?),
            snapshot.authority().clone(),
            None,
            Vec::new(),
            false,
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn sqlite_snapshot_cannot_tear_when_both_domains_commit_between_its_reads() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(&path)?;
    let session = session();
    save_actor(&store, &session).await?;
    seed_live_records(
        &path,
        &session,
        &[observation(1, "channel", "voice".into())?],
    )?;
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let paused = SqliteRuntimeStore::new_whole_blob(&path)?
        .with_live_composite_test_pause(reached_tx, resume_rx);
    let request = LiveCompositeReadRequest::new(session.id().clone(), None, 0, 64)?;
    let reading = tokio::spawn(async move { read_live_composite(&paused, request).await });
    tokio::task::spawn_blocking(move || {
        reached_rx.recv_timeout(std::time::Duration::from_secs(10))
    })
    .await??;

    // One paired physical fixture commit. This tests read-snapshot custody,
    // not a generated Live write/admission API.
    let mut successor = session.clone();
    successor.push(Message::User(meerkat_core::UserMessage::text(
        "committed during read",
    )));
    let bytes = serde_json::to_vec(&successor)?;
    let decoded = Session::decode_whole_blob_document(&bytes)?;
    let token = decoded.row_sha256_token().to_owned();
    let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO runtime_whole_blob_bodies (blob_sha256,session_snapshot) VALUES (?1,?2)",
        rusqlite::params![token, bytes],
    )?;
    tx.execute("UPDATE runtime_whole_blob_authority SET store_revision=2, blob_sha256=?1 WHERE runtime_id=?2",
        rusqlite::params![token, LogicalRuntimeId::for_session(session.id()).to_string()])?;
    tx.execute(
        "UPDATE runtime_session_snapshots SET session_snapshot=?1 WHERE runtime_id=?2",
        rusqlite::params![
            bytes,
            LogicalRuntimeId::for_session(session.id()).to_string()
        ],
    )?;
    tx.execute(
        "UPDATE runtime_live_heads SET revision=2 WHERE session_id=?1",
        [session.id().to_string()],
    )?;
    tx.commit()?;
    resume_tx.send(())?;
    let captured = reading.await??.ok_or("captured read")?;
    assert_eq!(captured.authority().actor().store_revision(), 1);
    assert_eq!(captured.authority().live_head().ok_or("head")?.revision, 1);
    assert_eq!(captured.session().messages(), session.messages());
    let next = read(&store, &session, 0, 64).await?;
    assert_eq!(next.authority().actor().store_revision(), 2);
    assert_eq!(next.authority().live_head().ok_or("head")?.revision, 2);
    assert_eq!(next.session().messages(), successor.messages());
    Ok(())
}

#[tokio::test]
async fn head_canonical_read_excludes_a_newer_physical_actor_tail() -> TestResult {
    use meerkat_core::SessionStore;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    let sessions = meerkat_store::SqliteSessionStore::open(&path)?;
    let session = session();
    sessions.save(&session).await?;
    let initial = SqliteRuntimeStore::new_whole_blob(&path)?;
    save_actor(&initial, &session).await?;
    drop(initial);
    let store = SqliteRuntimeStore::new_head_canonical(&path)?;
    let before = read(&store, &session, 0, 64).await?;
    let mut physical = sessions
        .load(session.id())
        .await?
        .ok_or("physical session")?;
    physical.push(Message::User(meerkat_core::UserMessage::text(
        "not runtime-committed",
    )));
    sessions.save(&physical).await?;
    assert_eq!(
        sessions
            .load(session.id())
            .await?
            .ok_or("physical session")?
            .messages()
            .len(),
        2
    );
    let captured = read(&store, &session, 0, 64).await?;
    assert_eq!(captured.session().messages(), before.session().messages());
    assert_eq!(captured.authority().actor(), before.authority().actor());
    assert_eq!(captured.session().messages().len(), 1);
    Ok(())
}

fn session() -> Session {
    let mut session = Session::new();
    session.push(Message::User(meerkat_core::UserMessage::text(
        "committed actor",
    )));
    session
}

async fn save_actor(store: &dyn RuntimeStore, session: &Session) -> TestResult {
    store
        .commit_session_snapshot(
            &LogicalRuntimeId::for_session(session.id()),
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(serde_json::to_vec(session)?),
            },
        )
        .await?;
    Ok(())
}

fn observation(
    sequence: u64,
    channel: &str,
    text: String,
) -> Result<LiveLedgerRecord, Box<dyn std::error::Error>> {
    let fit = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
        sequence: LiveObservationSeq::new(sequence)?,
        channel_id: LiveChannelId::new(channel),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(0.25, 1.75)?,
            text,
        ),
    })?;
    Ok(LiveLedgerRecord::Observation(
        StoredLiveObservation::from_fit(&fit),
    ))
}

fn seed_live_records(path: &Path, session: &Session, records: &[LiveLedgerRecord]) -> TestResult {
    let mut conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
    let tx = conn.transaction()?;
    let mut prefix = LiveLedgerPrefixDigest::empty(session.id(), 1);
    let mut encoded = Vec::new();
    for record in records {
        let bytes = record.encode()?;
        prefix = prefix.appended(record.sequence(), &bytes);
        encoded.push((record, bytes, prefix));
    }
    let count = records.last().map_or(0, |record| record.sequence().get());
    tx.execute(
        "INSERT INTO runtime_live_heads
         (session_id,format_version,generation,revision,event_count,prefix_digest,
          used_records,used_bytes,reserved_records,reserved_bytes,ingress_generation,
          transcript_snapshot,request_snapshot,commit_digest)
         VALUES (?1,1,1,1,?2,?3,?2,0,0,0,1,?4,?4,zeroblob(32))",
        rusqlite::params![
            session.id().to_string(),
            count,
            prefix.as_bytes().as_slice(),
            b"{}".as_slice()
        ],
    )?;
    for (record, bytes, record_prefix) in encoded {
        tx.execute(
            "INSERT INTO runtime_live_events (session_id,sequence,channel_id,record,record_digest,commit_revision,prefix_digest)
             VALUES (?1,?2,?3,?4,?5,1,?6)",
            rusqlite::params![
                session.id().to_string(),
                record.sequence().get(),
                record.channel_id().as_str(),
                bytes,
                Sha256::digest(&bytes).as_slice(),
                record_prefix.as_bytes().as_slice()
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

async fn read(
    store: &dyn RuntimeStore,
    session: &Session,
    after: u64,
    limit: usize,
) -> Result<LiveCompositeRead, Box<dyn std::error::Error>> {
    read_live_composite(
        store
            .live_ledger_ops()
            .ok_or("atomic read capability missing")?,
        LiveCompositeReadRequest::new(session.id().clone(), None, after, limit)?,
    )
    .await?
    .ok_or_else(|| "composite session missing".into())
}

#[tokio::test]
async fn memory_reads_one_committed_actor_with_explicit_absent_live_history() -> TestResult {
    let store = InMemoryRuntimeStore::new();
    let session = session();
    save_actor(&store, &session).await?;
    let composite = read(&store, &session, 0, 64).await?;
    assert_eq!(composite.session().messages(), session.messages());
    assert!(composite.authority().live_head().is_none());
    assert!(composite.records().is_empty());
    assert!(!composite.has_more());
    Ok(())
}

#[tokio::test]
async fn sqlite_whole_blob_and_head_canonical_read_exact_actor_and_live_partition() -> TestResult {
    for head_canonical in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        let session = session();
        if head_canonical {
            let sessions = meerkat_store::SqliteSessionStore::open(&path)?;
            meerkat_core::SessionStore::save(&sessions, &session).await?;
        }
        let initial = SqliteRuntimeStore::new_whole_blob(&path)?;
        save_actor(&initial, &session).await?;
        seed_live_records(
            &path,
            &session,
            &[observation(1, "channel", " exact\nvoice ".into())?],
        )?;
        drop(initial);
        let store = if head_canonical {
            SqliteRuntimeStore::new_head_canonical(&path)?
        } else {
            SqliteRuntimeStore::new_whole_blob(&path)?
        };
        let composite = read(&store, &session, 0, 64).await?;
        assert_eq!(composite.session().messages(), session.messages());
        assert_eq!(
            composite.authority().actor().head_canonical().is_some(),
            head_canonical
        );
        assert_eq!(
            composite
                .authority()
                .live_head()
                .ok_or("live head")?
                .event_count,
            1
        );
        assert_eq!(composite.records().len(), 1);
        let LiveLedgerRecord::Observation(record) = &composite.records()[0] else {
            return Err("observation changed kind".into());
        };
        assert_eq!(record.record().observation.text(), " exact\nvoice ");
        assert!(!composite.has_more());
    }
    Ok(())
}

#[tokio::test]
async fn byte_bounded_windows_advance_and_reconstruct_accepted_rows_after_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(&path)?;
    let session = session();
    save_actor(&store, &session).await?;
    let records = (1..=3)
        .map(|sequence| observation(sequence, "channel", "\0".repeat(16000)))
        .collect::<Result<Vec<_>, _>>()?;
    seed_live_records(&path, &session, &records)?;
    drop(store);
    let mut after = 0;
    let mut restored = Vec::new();
    loop {
        let store = SqliteRuntimeStore::new_whole_blob(&path)?;
        let composite = read(&store, &session, after, 256).await?;
        assert_eq!(
            composite.records().len(),
            1,
            "encoded-byte bound must win over row limit"
        );
        for record in composite.records() {
            assert!(record.sequence().get() > after);
            after = record.sequence().get();
            restored.push(record.encode()?);
        }
        if !composite.has_more() {
            break;
        }
    }
    assert_eq!(
        restored,
        records
            .iter()
            .map(LiveLedgerRecord::encode)
            .collect::<Result<Vec<_>, _>>()?
    );
    Ok(())
}

#[tokio::test]
async fn corrupted_record_or_index_fails_instead_of_skipping_accepted_content() -> TestResult {
    for corrupt in [
        "UPDATE runtime_live_events SET record_digest = zeroblob(32)",
        "UPDATE runtime_live_events SET channel_id = 'wrong-index'",
        "UPDATE runtime_live_events SET record = json_set(record,'$.record.channel_id','wrong-payload')",
        "UPDATE runtime_live_events SET record = zeroblob(200000)",
        "DELETE FROM runtime_live_events",
    ] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        let store = SqliteRuntimeStore::new_whole_blob(&path)?;
        let session = session();
        save_actor(&store, &session).await?;
        seed_live_records(
            &path,
            &session,
            &[observation(1, "channel", "retained".into())?],
        )?;
        let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
        conn.execute_batch(corrupt)?;
        assert!(read(&store, &session, 0, 64).await.is_err());
        assert!(
            read_live_composite(
                store.live_ledger_ops().ok_or("live read capability")?,
                LiveCompositeReadRequest::new(
                    session.id().clone(),
                    Some(LiveChannelId::new("channel")),
                    0,
                    64
                )?,
            )
            .await
            .is_err(),
            "a stale channel index must not turn corruption into an empty filtered success"
        );
    }
    Ok(())
}

#[tokio::test]
async fn independent_live_partition_does_not_advance_actor_authority() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(&path)?;
    let mut session = session();
    save_actor(&store, &session).await?;
    let before = read(&store, &session, 0, 64).await?;
    seed_live_records(
        &path,
        &session,
        &[observation(1, "channel", "voice".into())?],
    )?;
    let after_live = read(&store, &session, 0, 64).await?;
    assert_eq!(before.authority().actor(), after_live.authority().actor());
    session.push(Message::User(meerkat_core::UserMessage::text(
        "next actor commit",
    )));
    save_actor(&store, &session).await?;
    let after_actor = read(&store, &session, 0, 64).await?;
    assert_ne!(
        after_live.authority().actor(),
        after_actor.authority().actor()
    );
    assert_eq!(
        after_live.authority().live_head(),
        after_actor.authority().live_head()
    );
    assert_eq!(
        after_live.authority().record_window_digest(),
        after_actor.authority().record_window_digest()
    );
    Ok(())
}

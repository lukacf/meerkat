use meerkat_core::live_execution::evidence::LiveObservationInterval;
use meerkat_core::live_execution::request::LiveSourceKey;
use meerkat_runtime::live_source::{
    LiveSourceEntryRecord, LiveSourceFingerprint, LiveSourceRecordError,
    LiveSourceReservationRecord,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn source_row_restore_checks_channel_bytes_before_reading_or_hashing_content() -> TestResult {
    use meerkat_runtime::live_ledger::source::LiveSourceRow;
    for channel in ["x".repeat(129), format!("{}x", "\u{e9}".repeat(64))] {
        let source: LiveSourceKey = serde_json::from_value(json!({
            "session_id":uuid::Uuid::new_v4(),"channel_id":channel,
            "source":{"kind":"client_delegation","delegation":"d"}
        }))?;
        let error = LiveSourceRow::restore(source, vec![], &[])
            .err()
            .ok_or("invalid storage key accepted")?;
        assert!(
            matches!(error, meerkat_runtime::store::RuntimeStoreError::ReadFailed(ref message)
            if message == "live source channel must contain 1-128 UTF-8 bytes")
        );
    }
    Ok(())
}

#[test]
fn source_storage_row_binds_exact_owner_bytes_and_separate_variable_key_charge() -> TestResult {
    use meerkat_runtime::live_ledger::source::LiveSourceRow;
    use meerkat_runtime::live_resources::LIVE_RECORD_STORAGE_ALLOWANCE_BYTES;
    let record: LiveSourceReservationRecord = serde_json::from_value(image()?)?;
    let entry = LiveSourceEntryRecord::Reservation {
        record: Box::new(record),
    };
    let row = LiveSourceRow::encode(&entry)?;
    let restored = LiveSourceRow::restore(
        row.source().clone(),
        row.bytes().to_vec(),
        row.digest().as_bytes(),
    )?;
    assert_eq!(restored.record()?, entry);
    assert_eq!(restored.bytes(), row.bytes());
    let identity = serde_json::to_vec(row.source().source())?;
    assert_eq!(row.charge()?.records, 1);
    assert_eq!(
        row.charge()?.encoded_bytes,
        row.bytes().len() as u64 + 2 * identity.len() as u64 + LIVE_RECORD_STORAGE_ALLOWANCE_BYTES
    );
    let mut wrong_owner = serde_json::to_value(row.source())?;
    wrong_owner["session_id"] = json!(uuid::Uuid::new_v4());
    assert!(
        LiveSourceRow::restore(
            serde_json::from_value(wrong_owner)?,
            row.bytes().to_vec(),
            row.digest().as_bytes()
        )
        .is_err()
    );
    let mut changed = row.bytes().to_vec();
    changed.push(b' ');
    assert!(
        LiveSourceRow::restore(row.source().clone(), changed, row.digest().as_bytes()).is_err()
    );
    assert!(LiveSourceRow::restore(row.source().clone(), row.bytes().to_vec(), &[0; 32]).is_err());
    assert!(!format!("{row:?}").contains("frozen_request"));
    Ok(())
}

#[test]
fn source_storage_reads_reject_oversized_or_unknown_content_without_assuming_absence() -> TestResult
{
    use meerkat_runtime::live_ledger::source::{LIVE_SOURCE_ROW_MAX_BYTES, LiveSourceRow};
    use sha2::{Digest, Sha256};
    let record: LiveSourceReservationRecord = serde_json::from_value(image()?)?;
    let row = LiveSourceRow::encode(&LiveSourceEntryRecord::Reservation {
        record: Box::new(record),
    })?;
    assert!(
        LiveSourceRow::restore(
            row.source().clone(),
            vec![b' '; LIVE_SOURCE_ROW_MAX_BYTES + 1],
            row.digest().as_bytes()
        )
        .is_err()
    );
    assert!(LiveSourceRow::restore(row.source().clone(), row.bytes().to_vec(), &[0; 33]).is_err());
    let mut unknown: Value = serde_json::from_slice(row.bytes())?;
    unknown["grant_authority"] = json!("forged");
    let bytes = serde_json::to_vec(&unknown)?;
    let key = serde_json::to_vec(row.source())?;
    let mut hash = Sha256::new();
    hash.update(b"meerkat.live-source-row.v1\0");
    for part in [key.as_slice(), bytes.as_slice()] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    assert!(
        LiveSourceRow::restore(row.source().clone(), bytes, hash.finalize().as_slice()).is_err()
    );
    Ok(())
}

#[test]
fn application_fingerprint_must_bind_the_same_frozen_interval() -> TestResult {
    use meerkat_runtime::live_source::LiveSourceReservationParts;
    let original_interval = LiveObservationInterval::new(1, 2)?;
    let mut value = image()?;
    value["source"]["source"] =
        json!({"kind":"application_request","request_id":uuid::Uuid::new_v4()});
    value["fingerprint"] = serde_json::to_value(LiveSourceFingerprint::application_request(
        original_interval,
    ))?;
    value["context"]["interval"] = json!({"after":1,"through":3});
    value["frozen_request"]["observations"] = json!({"after":1,"through":3});
    let parts: LiveSourceReservationParts = serde_json::from_value(value.clone())?;
    assert_eq!(
        LiveSourceReservationRecord::try_from(parts).err(),
        Some(LiveSourceRecordError::SourcePayloadConflict)
    );
    assert!(serde_json::from_value::<LiveSourceReservationRecord>(value.clone()).is_err());
    value["context"]["interval"] = serde_json::to_value(original_interval)?;
    value["frozen_request"]["observations"] = serde_json::to_value(original_interval)?;
    let valid: LiveSourceReservationRecord = serde_json::from_value(value)?;
    assert_eq!(
        valid
            .replay(
                valid.source(),
                LiveSourceFingerprint::application_request(original_interval)
            )?
            .reserved_frontier(),
        2
    );
    Ok(())
}

#[test]
fn automatic_empty_work_and_empty_refusal_with_executable_text_are_inexpressible() -> TestResult {
    let mut value = image()?;
    value["context"]["interval"] = json!({"after":3,"through":3});
    value["frozen_request"]["observations"] = json!({"after":3,"through":3});
    assert!(serde_json::from_value::<LiveSourceReservationRecord>(value.clone()).is_err());
    value["disposition"] = json!({
        "kind":"admitted",
        "receipt":{
            "source":value["source"],"input_id":uuid::Uuid::new_v4(),
            "executor":{"session_id":value["source"]["session_id"],"realm":"executor",
                "runtime_epoch":uuid::Uuid::new_v4(),"binding_generation":1},
            "grant":value["grant"],"ingress_generation_at_admission":1,
            "commit":{"revision":4,"digest":vec![5;32]}
        }
    });
    assert!(serde_json::from_value::<LiveSourceReservationRecord>(value).is_err());
    let mut value = image()?;
    value["disposition"] = json!({"kind":"refused","reason":"empty"});
    assert!(serde_json::from_value::<LiveSourceReservationRecord>(value.clone()).is_err());
    value["frozen_request"] = Value::Null;
    let nonzero: LiveSourceReservationRecord = serde_json::from_value(value.clone())?;
    assert_eq!(
        nonzero.context().interval,
        LiveObservationInterval::new(0, 3)?
    );
    value["context"]["read_coverage"] = json!("window_continues");
    assert!(serde_json::from_value::<LiveSourceReservationRecord>(value.clone()).is_err());
    value["context"]["read_coverage"] = json!("complete_to_captured_head");
    value["context"]["interval"] = json!({"after":3,"through":3});
    let valid: LiveSourceReservationRecord = serde_json::from_value(value)?;
    assert_eq!(valid.context().interval.after(), valid.reserved_frontier());
    Ok(())
}

#[test]
fn structured_function_work_does_not_acquire_an_automatic_transcript_prefix_requirement()
-> TestResult {
    let mut value = image()?;
    value["source"]["source"] =
        json!({"kind":"function_call","delegation":"d1","response":"r1","call":"c1"});
    value["fingerprint"] = serde_json::to_value(LiveSourceFingerprint::function_call(
        "invoke_meerkat",
        r#"{"request":"work"}"#,
    ))?;
    value["context"]["interval"] = json!({"after":3,"through":3});
    value["context"]["read_coverage"] = json!("window_continues");
    value["frozen_request"] = json!({"kind":"structured_function_request","request":"work"});
    let record: LiveSourceReservationRecord = serde_json::from_value(value)?;
    assert_eq!(record.reserved_frontier(), 3);
    assert_eq!(
        record.frozen_request().ok_or("request")?.request().as_str(),
        "work"
    );
    Ok(())
}

#[tokio::test]
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]
async fn reservation_context_comes_from_one_actual_composite_read() -> TestResult {
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_execution::request::{LiveProviderReference, LiveSourceIdentity};
    use meerkat_runtime::identifiers::LogicalRuntimeId;
    use meerkat_runtime::live_ledger::transcript::LiveLedgerPrefixDigest;
    use meerkat_runtime::live_source::{
        LiveSourceContextReference, LiveSourceDisposition, LiveSourceRefusal,
        LiveSourceReservationParts,
    };
    use meerkat_runtime::store::live_read::{LiveCompositeReadRequest, read_live_composite};
    use meerkat_runtime::store::{RuntimeStore, SerializedSessionSnapshot, SqliteRuntimeStore};
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("source.sqlite3");
    let store = SqliteRuntimeStore::new_whole_blob(&path)?;
    let session = meerkat_core::Session::new();
    store
        .commit_session_snapshot(
            &LogicalRuntimeId::for_session(session.id()),
            SerializedSessionSnapshot {
                session_snapshot: std::sync::Arc::new(serde_json::to_vec(&session)?),
            },
        )
        .await?;
    let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
    conn.execute(
        "INSERT INTO runtime_live_heads
         (session_id,format_version,generation,revision,event_count,prefix_digest,
          used_records,used_bytes,reserved_records,reserved_bytes,ingress_generation,
          transcript_snapshot,request_snapshot,commit_digest)
         VALUES (?1,1,1,1,0,?2,0,0,0,0,1,?3,?3,zeroblob(32))",
        rusqlite::params![
            session.id().to_string(),
            LiveLedgerPrefixDigest::empty(session.id(), 1)
                .as_bytes()
                .as_slice(),
            b"{}".as_slice()
        ],
    )?;
    let source = LiveSourceKey::new(
        session.id().clone(),
        LiveChannelId::new("voice"),
        LiveSourceIdentity::ClientDelegation {
            delegation: LiveProviderReference::new("d0")?,
        },
    )?;
    let read = read_live_composite(
        store.live_ledger_ops().ok_or("capability")?,
        LiveCompositeReadRequest::new(
            session.id().clone(),
            Some(LiveChannelId::new("voice")),
            0,
            64,
        )?,
    )
    .await?
    .ok_or("composite")?;
    let interval = LiveObservationInterval::new(0, 0)?;
    let context = LiveSourceContextReference::from_composite(&read, &source, interval)?;
    let record = LiveSourceReservationRecord::try_from(LiveSourceReservationParts {
        source: source.clone(),
        request_id: meerkat_core::ops::OperationId(uuid::Uuid::new_v4()),
        fingerprint: LiveSourceFingerprint::client_delegation(0.0)?,
        context,
        frozen_request: None,
        grant: None,
        cancellation: None,
        disposition: LiveSourceDisposition::Refused {
            reason: LiveSourceRefusal::Empty,
        },
    })?;
    assert_eq!(record.reserved_frontier(), 0);
    let other = LiveSourceKey::new(
        session.id().clone(),
        LiveChannelId::new("other"),
        source.source().clone(),
    )?;
    assert!(LiveSourceContextReference::from_composite(&read, &other, interval).is_err());
    Ok(())
}

#[test]
fn admitted_source_binds_the_exact_receipt_without_inventing_a_run() -> TestResult {
    let mut value = image()?;
    let receipt = json!({
        "source":value["source"],
        "input_id":"00000000-0000-0000-0000-000000000007",
        "executor":{
            "session_id":"00000000-0000-0000-0000-000000000001",
            "realm":"executor","runtime_epoch":"00000000-0000-0000-0000-000000000008",
            "binding_generation":1
        },
        "grant":value["grant"],"ingress_generation_at_admission":1,
        "commit":{"revision":4,"digest":vec![5;32]}
    });
    value["disposition"] = json!({"kind":"admitted","receipt":receipt});
    let record: LiveSourceReservationRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(&record)?, value);
    assert!(value["disposition"]["receipt"].get("run_id").is_none());
    for (pointer, replacement) in [
        (
            "/disposition/receipt/source/source/delegation",
            json!("different-source"),
        ),
        ("/disposition/receipt/grant/generation", json!(99)),
    ] {
        let mut altered = value.clone();
        *altered.pointer_mut(pointer).ok_or("fixture field")? = replacement;
        assert!(serde_json::from_value::<LiveSourceReservationRecord>(altered).is_err());
    }
    Ok(())
}

fn image() -> Result<Value, LiveSourceRecordError> {
    Ok(json!({
        "source":{
            "session_id":"00000000-0000-0000-0000-000000000001",
            "channel_id":"voice",
            "source":{"kind":"client_delegation","delegation":"d1"}
        },
        "request_id":"00000000-0000-0000-0000-000000000002",
        "fingerprint":LiveSourceFingerprint::client_delegation(2.5)?,
        "context":{
            "actor":{
                "session_id":"00000000-0000-0000-0000-000000000001",
                "profile":"whole_blob_v1","revision":1,"token":format!("row-sha256:{}", "a".repeat(64))
            },
            "live_head":{
                "format":"live_ledger_v1","session_id":"00000000-0000-0000-0000-000000000001",
                "generation":1,"revision":2,"event_count":3,"prefix_digest":vec![3;32]
            },
            "channel_id":"voice","interval":{"after":0,"through":3},
            "read_coverage":"complete_to_captured_head","record_window_digest":vec![4;32]
        },
        "frozen_request":{
            "kind":"application_snapshot","observations":{"after":0,"through":3},
            "request":" original provisional request "
        },
        "grant":{"id":"00000000-0000-0000-0000-000000000005","issuer_realm":"owner","generation":6},
        "cancellation":null,
        "disposition":{"kind":"reserved"}
    }))
}

#[test]
fn replay_returns_original_snapshot_and_frontier_without_new_text_or_grant_inputs() -> TestResult {
    let original: LiveSourceReservationRecord = serde_json::from_value(image()?)?;
    let mut newer = image()?;
    newer["context"]["live_head"]["event_count"] = json!(9);
    newer["context"]["interval"]["through"] = json!(9);
    newer["frozen_request"]["observations"]["through"] = json!(9);
    newer["frozen_request"]["request"] = json!("later text");
    let later: LiveSourceReservationRecord = serde_json::from_value(newer)?;
    let replay = original.replay(
        later.source(),
        LiveSourceFingerprint::client_delegation(2.5)?,
    )?;
    assert!(std::ptr::eq(replay, &raw const original));
    assert_eq!(replay.reserved_frontier(), 3);
    assert_eq!(
        replay.frozen_request().ok_or("request")?.request().as_str(),
        " original provisional request "
    );
    assert_eq!(
        serde_json::from_value::<LiveSourceReservationRecord>(serde_json::to_value(&original)?)?,
        original
    );
    Ok(())
}

#[test]
fn changed_source_metadata_is_conflict_not_new_identity_or_refreeze() -> TestResult {
    let record: LiveSourceReservationRecord = serde_json::from_value(image()?)?;
    assert_eq!(
        record
            .replay(
                record.source(),
                LiveSourceFingerprint::client_delegation(3.5)?
            )
            .err(),
        Some(LiveSourceRecordError::SourcePayloadConflict)
    );
    let mut other = serde_json::to_value(record.source())?;
    other["source"]["delegation"] = json!("different-source");
    let other: LiveSourceKey = serde_json::from_value(other)?;
    assert_eq!(
        record
            .replay(&other, LiveSourceFingerprint::client_delegation(2.5)?)
            .err(),
        Some(LiveSourceRecordError::SourceMismatch)
    );
    assert_ne!(
        LiveSourceFingerprint::function_call("invoke_meerkat", "{\"request\":\"work\"}"),
        LiveSourceFingerprint::function_call("invoke_meerkat", "{ \"request\":\"work\"}")
    );
    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(LiveSourceFingerprint::client_delegation(invalid).is_err());
    }
    Ok(())
}

#[test]
fn refused_and_cancelled_sources_retain_range_without_becoming_late_permission() -> TestResult {
    for disposition in [
        json!({"kind":"refused","reason":"empty"}),
        json!({"kind":"refused","reason":"gap"}),
        json!({"kind":"refused","reason":"budget"}),
        json!({"kind":"refused","reason":"permission"}),
        json!({"kind":"refused","reason":"ingress_closed"}),
        json!({"kind":"cancelled_without_run","reason":"operator_requested"}),
    ] {
        let mut value = image()?;
        if disposition == json!({"kind":"refused","reason":"empty"}) {
            value["context"]["interval"] = json!({"after":3,"through":3});
        }
        value["disposition"] = disposition;
        value["frozen_request"] = Value::Null;
        value["grant"] = Value::Null;
        let record: LiveSourceReservationRecord = serde_json::from_value(value)?;
        let replay = record.replay(
            record.source(),
            LiveSourceFingerprint::client_delegation(2.5)?,
        )?;
        assert_eq!(replay.reserved_frontier(), 3);
        assert!(replay.frozen_request().is_none());
        assert_eq!(
            serde_json::from_value::<LiveSourceReservationRecord>(serde_json::to_value(&record)?)?,
            record
        );
    }
    Ok(())
}

#[test]
fn cancellation_can_exist_before_snapshot_or_input_without_spending_a_frontier() -> TestResult {
    let value = json!({
        "kind":"cancellation_only",
        "intent":{"source":image()?["source"],"reason":"operator_requested"}
    });
    let entry: LiveSourceEntryRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(&entry)?, value);
    let LiveSourceEntryRecord::CancellationOnly { intent } = entry else {
        return Err("intent".into());
    };
    assert_eq!(intent.source.channel_id().as_str(), "voice");
    for field in ["context", "input_id", "run_id", "reserved_frontier"] {
        let mut altered = value.clone();
        altered[field] = json!(1);
        assert!(serde_json::from_value::<LiveSourceEntryRecord>(altered).is_err());
    }
    Ok(())
}

#[test]
fn incomplete_future_or_wrong_owner_snapshots_cannot_be_reserved_for_execution() -> TestResult {
    for (pointer, replacement) in [
        ("/context/read_coverage", json!("window_continues")),
        ("/context/interval/through", json!(4)),
        (
            "/context/live_head/session_id",
            json!("00000000-0000-0000-0000-000000000099"),
        ),
        ("/context/channel_id", json!("different")),
        ("/frozen_request/observations/after", json!(1)),
        ("/grant", Value::Null),
    ] {
        let mut value = image()?;
        *value.pointer_mut(pointer).ok_or("fixture field")? = replacement;
        assert!(
            serde_json::from_value::<LiveSourceReservationRecord>(value).is_err(),
            "{pointer}"
        );
    }
    let mut value = image()?;
    value["context"]["interval"]["through"] = json!(2);
    value["frozen_request"]["observations"]["through"] = json!(2);
    let selected: LiveSourceReservationRecord = serde_json::from_value(value)?;
    assert_eq!(selected.reserved_frontier(), 2);
    // The generated transcript owner, not the mixed ledger head count, owns
    // the channel watermark. A DTO is not a reservation capability.
    Ok(())
}

#[test]
fn explicit_application_resubmission_uses_a_new_source_and_an_explicit_range() -> TestResult {
    let interval = LiveObservationInterval::new(1, 2)?;
    let mut value = image()?;
    value["source"]["source"] =
        json!({"kind":"application_request","request_id":uuid::Uuid::new_v4()});
    value["context"]["interval"] = serde_json::to_value(interval)?;
    value["frozen_request"]["observations"] = serde_json::to_value(interval)?;
    value["fingerprint"] =
        serde_json::to_value(LiveSourceFingerprint::application_request(interval))?;
    let record: LiveSourceReservationRecord = serde_json::from_value(value)?;
    assert_eq!(record.reserved_frontier(), 2);
    assert!(record.encoded_charge()?.encoded_bytes > 0);
    Ok(())
}

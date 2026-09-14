use meerkat_contracts::wire::live_observation::{
    LiveObservationRecord, LiveObservationWireCodecV1,
};
use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::{
    LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
};
use meerkat_runtime::live_ledger::transcript::{
    KnownLiveReceiveGap, LiveDiscontinuity, LiveHeadReference, LiveLedgerFormatV1,
    LiveLedgerPrefixDigest, StoredLiveObservation,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn observation() -> Result<StoredLiveObservation, Box<dyn std::error::Error>> {
    let fit = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
        sequence: LiveObservationSeq::new(1)?,
        channel_id: LiveChannelId::new("incarnation"),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(1066.5390310178611, 1066.5390310178614)?,
            " \nunfinished\t\\\0 ",
        ),
    })?;
    Ok(StoredLiveObservation::from_fit(&fit))
}

#[test]
fn persisted_observation_preserves_exact_encoding_and_revalidates_receipt() -> TestResult {
    let stored = observation()?;
    let bytes = serde_json::to_vec(&stored)?;
    let restored: StoredLiveObservation = serde_json::from_slice(&bytes)?;
    assert_eq!(restored.record(), stored.record());
    assert_eq!(restored.wire_receipt(), stored.wire_receipt());
    assert_eq!(serde_json::to_vec(&restored)?, bytes);
    for field in ["format", "new_schema"] {
        let mut value = serde_json::to_value(&stored)?;
        value[field] = json!("live_ledger_v2");
        assert!(serde_json::from_value::<StoredLiveObservation>(value).is_err());
    }
    let mut changed = serde_json::to_value(stored)?;
    changed["record"]["observation"]["text"] = json!("different accepted text");
    assert!(serde_json::from_value::<StoredLiveObservation>(changed).is_err());
    Ok(())
}

#[test]
fn unknown_crash_extent_cannot_acquire_fabricated_receive_bounds() -> TestResult {
    let session = SessionId::new();
    let head = LiveHeadReference {
        format: LiveLedgerFormatV1::V1,
        session_id: session.clone(),
        generation: 1,
        revision: 8,
        event_count: 10,
        prefix_digest: LiveLedgerPrefixDigest::empty(&session, 1),
    };
    let discontinuity = LiveDiscontinuity::UnknownExtentCrashDiscontinuity {
        last_accepted_head: head,
        old_incarnation: LiveChannelId::new("old-channel"),
    };
    let encoded = serde_json::to_value(&discontinuity)?;
    assert_eq!(
        serde_json::from_value::<LiveDiscontinuity>(encoded.clone())?,
        discontinuity
    );
    for field in ["received_watermark", "lost_count", "observed_bounds"] {
        let mut fabricated = encoded.clone();
        fabricated[field] = json!(100);
        assert!(serde_json::from_value::<LiveDiscontinuity>(fabricated).is_err());
    }
    assert!(KnownLiveReceiveGap::new(4, 4).is_err());
    assert!(KnownLiveReceiveGap::new(5, 4).is_err());
    let known = LiveDiscontinuity::KnownLocalGap {
        channel_id: LiveChannelId::new("channel"),
        observed_bounds: KnownLiveReceiveGap::new(4, 9)?,
    };
    assert_eq!(
        serde_json::from_value::<LiveDiscontinuity>(serde_json::to_value(&known)?)?,
        known
    );
    assert_ne!(known, discontinuity);
    Ok(())
}

#[test]
fn live_prefix_is_domain_session_generation_sequence_and_bytes_bound() -> TestResult {
    let session = SessionId::new();
    let empty = LiveLedgerPrefixDigest::empty(&session, 1);
    assert_ne!(empty, LiveLedgerPrefixDigest::empty(&session, 2));
    assert_ne!(empty, LiveLedgerPrefixDigest::empty(&SessionId::new(), 1));
    let bytes = serde_json::to_vec(&observation()?)?;
    let prefix = empty.appended(LiveObservationSeq::new(1)?, &bytes);
    assert_ne!(prefix, empty.appended(LiveObservationSeq::new(2)?, &bytes));
    assert_ne!(
        prefix,
        empty.appended(LiveObservationSeq::new(1)?, b"different")
    );
    assert_eq!(
        serde_json::from_slice::<LiveLedgerPrefixDigest>(&serde_json::to_vec(&prefix)?)?,
        prefix
    );
    Ok(())
}

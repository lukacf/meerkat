use meerkat_contracts::wire::live_observation::{
    LIVE_OBSERVATION_REPLY_MAX_BYTES, LIVE_OBSERVATION_TEXT_MAX_BYTES, LiveObservationCoverage,
    LiveObservationEncodingError, LiveObservationFilter, LiveObservationOwner,
    LiveObservationRecord, LiveObservationSnapshot, LiveObservationWireCodecV1 as Codec,
    LiveObservationWireFit, LiveObservationWireReceipt,
};
use meerkat_contracts::wire::supervisor_bridge::BridgeReply;
use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::{
    LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn record(
    sequence: u64,
    text: impl Into<Box<str>>,
) -> Result<LiveObservationRecord, Box<dyn std::error::Error>> {
    Ok(LiveObservationRecord {
        sequence: LiveObservationSeq::new(sequence)?,
        channel_id: LiveChannelId::new("\0".repeat(128)),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(1.25, 2.5)?,
            text,
        ),
    })
}

fn owner() -> LiveObservationOwner {
    LiveObservationOwner::Member {
        session_id: SessionId(uuid::Uuid::nil()),
        mob_id: "\0".repeat(128),
        agent_identity: "\0".repeat(128),
    }
}

fn snapshot(end_sequence: u64) -> LiveObservationSnapshot {
    LiveObservationSnapshot {
        generation: u64::MAX,
        revision: u64::MAX,
        end_sequence,
        prefix_digest: format!("sha256:{}", "f".repeat(64)),
        coverage: LiveObservationCoverage::UnknownExtentCrashDiscontinuity,
    }
}

fn filter() -> LiveObservationFilter {
    LiveObservationFilter::Channel {
        channel_id: LiveChannelId::new("\0".repeat(128)),
    }
}

fn maximal_fitting_text(unit: &str) -> Result<LiveObservationWireFit, Box<dyn std::error::Error>> {
    let mut lower = 0;
    let mut upper = LIVE_OBSERVATION_TEXT_MAX_BYTES / unit.len();
    while lower < upper {
        let middle = lower + (upper - lower).div_ceil(2);
        match Codec::check_record_fit(record(1, unit.repeat(middle))?) {
            Ok(_) => lower = middle,
            Err(LiveObservationEncodingError::EncodedReplyTooLarge) => upper = middle - 1,
            Err(error) => return Err(error.into()),
        }
    }
    let fit = Codec::check_record_fit(record(1, unit.repeat(lower))?)?;
    if lower < LIVE_OBSERVATION_TEXT_MAX_BYTES / unit.len() {
        assert!(matches!(
            Codec::check_record_fit(record(1, unit.repeat(lower + 1))?),
            Err(LiveObservationEncodingError::EncodedReplyTooLarge)
        ));
    }
    Ok(fit)
}

#[test]
fn decoded_ceiling_does_not_imply_complete_reply_fit() -> TestResult {
    for unit in ["\n", "\\", "\0"] {
        assert!(matches!(
            Codec::check_record_fit(record(1, unit.repeat(LIVE_OBSERVATION_TEXT_MAX_BYTES))?),
            Err(LiveObservationEncodingError::EncodedReplyTooLarge)
        ));
    }
    assert!(matches!(
        Codec::check_record_fit(record(1, "a".repeat(LIVE_OBSERVATION_TEXT_MAX_BYTES + 1))?),
        Err(LiveObservationEncodingError::TextTooLarge)
    ));
    Ok(())
}

#[test]
fn maximal_accepted_escaped_records_survive_persisted_roundtrip_and_full_reply() -> TestResult {
    for unit in ["\n", "\\", "\0", "\u{1f9a6}"] {
        let fit = maximal_fitting_text(unit)?;
        let persisted_record = serde_json::to_vec(fit.record())?;
        let persisted_receipt = serde_json::to_vec(fit.receipt())?;
        let restored = Codec::restore_record_fit(
            serde_json::from_slice(&persisted_record)?,
            &serde_json::from_slice::<LiveObservationWireReceipt>(&persisted_receipt)?,
        )?;
        assert_eq!(restored.record(), fit.record());
        assert_eq!(restored.receipt(), fit.receipt());
        assert_eq!(
            restored.receipt().record_encoded_bytes,
            persisted_record.len()
        );
        let page = Codec::page(
            owner(),
            filter(),
            snapshot(u64::MAX),
            0,
            &[restored],
            1,
            true,
        )?;
        let encoded = Codec::encode_reply(&page)?;
        assert!(encoded.len() <= LIVE_OBSERVATION_REPLY_MAX_BYTES);
        assert!(encoded.len() <= fit.receipt().maximal_single_reply_bytes);
        assert_eq!(
            encoded,
            serde_json::to_vec(&BridgeReply::MemberLiveObservationPage(page.clone()))?
        );
        let decoded: BridgeReply = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, BridgeReply::MemberLiveObservationPage(page));
    }
    Ok(())
}

#[test]
fn byte_limited_pages_advance_and_reconstruct_the_captured_prefix() -> TestResult {
    let mut records = Vec::new();
    for sequence in 1..=12 {
        records.push(Codec::check_record_fit(record(
            sequence,
            format!("record-{sequence}\0{}", "\0".repeat(18_000)),
        )?)?);
    }
    let snapshot = snapshot(12);
    let mut after_sequence = 0;
    let mut observed = Vec::new();
    while after_sequence < snapshot.end_sequence {
        let remaining = records
            .iter()
            .filter(|fit| fit.record().sequence.get() > after_sequence)
            .cloned()
            .collect::<Vec<_>>();
        let page = Codec::page(
            owner(),
            filter(),
            snapshot.clone(),
            after_sequence,
            &remaining,
            256,
            false,
        )?;
        assert!(!page.records.is_empty());
        assert!(Codec::encode_reply(&page)?.len() <= LIVE_OBSERVATION_REPLY_MAX_BYTES);
        observed.extend(page.records.iter().map(|record| record.as_ref().clone()));
        let last = page.records.last().ok_or("nonempty page required")?;
        assert!(last.sequence.get() > after_sequence);
        after_sequence = if let Some(cursor) = &page.next_cursor {
            let next = Codec::cursor_after_sequence(cursor, &owner(), &filter(), &snapshot)?;
            assert_eq!(next, last.sequence.get());
            assert!(page.has_more);
            next
        } else {
            assert!(!page.has_more);
            last.sequence.get()
        };
    }
    assert_eq!(
        observed,
        records
            .iter()
            .map(|fit| fit.record().clone())
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn record_limit_and_byte_limit_share_exact_cursor_semantics() -> TestResult {
    let records = (1..=5)
        .map(|sequence| Ok(Codec::check_record_fit(record(sequence, "small")?)?))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let snapshot = snapshot(5);
    let page = Codec::page(owner(), filter(), snapshot.clone(), 0, &records, 2, false)?;
    assert_eq!(page.records.len(), 2);
    let cursor = page.next_cursor.as_ref().ok_or("expected next cursor")?;
    assert_eq!(
        Codec::cursor_after_sequence(cursor, &owner(), &filter(), &snapshot)?,
        2
    );
    let wrong_owner = LiveObservationOwner::Session {
        session_id: SessionId::new(),
    };
    assert!(matches!(
        Codec::cursor_after_sequence(cursor, &wrong_owner, &filter(), &snapshot),
        Err(LiveObservationEncodingError::CursorMismatch)
    ));
    assert!(matches!(
        Codec::cursor_after_sequence(
            cursor,
            &owner(),
            &LiveObservationFilter::AllChannels {},
            &snapshot
        ),
        Err(LiveObservationEncodingError::CursorMismatch)
    ));
    Ok(())
}

#[test]
fn altered_record_or_encoding_charge_cannot_restore_a_fit_receipt() -> TestResult {
    let fit = Codec::check_record_fit(record(1, "original")?)?;
    assert!(matches!(
        Codec::restore_record_fit(record(1, "modified")?, fit.receipt()),
        Err(LiveObservationEncodingError::ReceiptMismatch)
    ));
    let mut altered = fit.receipt().clone();
    altered.record_encoded_bytes += 1;
    assert!(matches!(
        Codec::restore_record_fit(fit.record().clone(), &altered),
        Err(LiveObservationEncodingError::ReceiptMismatch)
    ));
    let mut encoded = serde_json::to_value(fit.receipt())?;
    encoded["profile"] = json!("v2");
    assert!(serde_json::from_value::<LiveObservationWireReceipt>(encoded).is_err());
    Ok(())
}

#[test]
fn invalid_windows_do_not_return_empty_success_or_skip_records() -> TestResult {
    assert!(
        serde_json::from_value::<LiveObservationFilter>(
            json!({"kind": "all_channels", "channel_id": "must-not-disappear"})
        )
        .is_err()
    );
    let first = Codec::check_record_fit(record(1, "first")?)?;
    let second = Codec::check_record_fit(record(2, "second")?)?;
    for records in [
        vec![second.clone(), first.clone()],
        vec![first.clone(), first.clone()],
    ] {
        assert!(matches!(
            Codec::page(owner(), filter(), snapshot(2), 0, &records, 2, false),
            Err(LiveObservationEncodingError::InvalidWindow)
        ));
    }
    assert!(matches!(
        Codec::page(owner(), filter(), snapshot(2), 0, &[], 2, true),
        Err(LiveObservationEncodingError::InvalidWindow)
    ));
    assert!(matches!(
        Codec::page(owner(), filter(), snapshot(2), 0, &[first], 0, false),
        Err(LiveObservationEncodingError::InvalidPageLimit)
    ));
    assert!(matches!(
        Codec::page(owner(), filter(), snapshot(2), 0, &[second], 1, true),
        Err(LiveObservationEncodingError::InvalidWindow)
    ));
    Ok(())
}

#[test]
fn final_maximum_sequence_is_retrievable_without_cursor_overflow() -> TestResult {
    let fit = Codec::check_record_fit(record(u64::MAX, "last")?)?;
    let page = Codec::page(
        owner(),
        filter(),
        snapshot(u64::MAX),
        u64::MAX - 1,
        &[fit],
        1,
        false,
    )?;
    assert_eq!(page.records.len(), 1);
    assert!(!page.has_more);
    assert!(page.next_cursor.is_none());
    assert!(Codec::encode_reply(&page)?.len() <= LIVE_OBSERVATION_REPLY_MAX_BYTES);
    Ok(())
}

#[test]
fn exact_fractional_bits_and_receipt_survive_persisted_json() -> TestResult {
    for center in [
        430.186_245_990_098_03_f64,
        0.845_512_408_225_570_1_f64,
        1.234_567_890_123_456_7_f64,
        9_007_199.254_740_993_f64,
    ] {
        let center_bits = center.to_bits();
        for bits in center_bits - 16..=center_bits + 16 {
            let end_ms = f64::from_bits(bits);
            let fit = Codec::check_record_fit(LiveObservationRecord {
                sequence: LiveObservationSeq::new(1)?,
                channel_id: LiveChannelId::new("fractional-channel"),
                observation: LiveTranscriptObservation::new(
                    LiveTranscriptDirection::Output,
                    LiveTranscriptRange::new(0.0, end_ms)?,
                    "unchanged observation",
                ),
            })?;
            let persisted = serde_json::to_vec(fit.record())?;
            let decoded: LiveObservationRecord = serde_json::from_slice(&persisted)?;
            assert_eq!(decoded.observation.range().end_ms().to_bits(), bits);
            assert_eq!(serde_json::to_vec(&decoded)?, persisted);
            let restored = Codec::restore_record_fit(decoded, fit.receipt())?;
            assert_eq!(restored.receipt(), fit.receipt());
        }
    }
    Ok(())
}

#[test]
fn wire_realtime_is_a_projection_not_another_stored_profile_field() -> TestResult {
    use meerkat_core::{ModelInteractionKind, ModelProfile, ModelReleaseStage, Provider};
    for (interaction_kind, realtime) in [
        (ModelInteractionKind::Text, false),
        (ModelInteractionKind::TurnBasedRealtime, true),
        (ModelInteractionKind::ContinuousLive, true),
    ] {
        let profile = ModelProfile {
            provider: Provider::Other,
            release_stage: ModelReleaseStage::OperatorDefined,
            model_family: "projection-fixture".into(),
            supports_temperature: false,
            supports_thinking: false,
            supports_reasoning: false,
            inline_video: false,
            vision: false,
            image_input: false,
            image_tool_results: false,
            interaction_kind,
            supports_web_search: false,
            supports_mid_conversation_system_messages: false,
            image_generation: false,
            params_schema: json!({}),
            beta_headers: Vec::new(),
            call_timeout_secs: None,
        };
        let stored = serde_json::to_value(&profile)?;
        assert!(stored.get("realtime").is_none());
        assert_eq!(
            stored["interaction_kind"],
            serde_json::to_value(interaction_kind)?
        );
        let wire = meerkat_contracts::WireModelProfile::from(&profile);
        assert_eq!(wire.realtime, realtime);
        assert_eq!(serde_json::to_value(wire)?["realtime"], json!(realtime));
    }
    Ok(())
}

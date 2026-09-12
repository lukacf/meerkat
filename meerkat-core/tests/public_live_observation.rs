use meerkat_core::live_observation::{
    LiveObservationSeq, LiveObservationValueError, LiveTranscriptDirection,
    LiveTranscriptObservation, LiveTranscriptRange,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn observation_preserves_exact_text_and_fractional_range() -> TestResult {
    let value = LiveTranscriptObservation::new(
        LiveTranscriptDirection::Input,
        LiveTranscriptRange::new(10.25, 20.5)?,
        " unfinished\n\\\0",
    );
    let encoded = serde_json::to_value(&value)?;
    assert_eq!(
        encoded,
        json!({
            "direction": "input",
            "range": {"start_ms": 10.25, "end_ms": 20.5},
            "text": " unfinished\n\\\0",
        })
    );
    let decoded: LiveTranscriptObservation = serde_json::from_value(encoded)?;
    assert_eq!(decoded, value);
    assert_eq!(decoded.text(), " unfinished\n\\\0");
    assert_eq!(decoded.range().start_ms().to_bits(), 10.25_f64.to_bits());
    assert_eq!(decoded.range().end_ms().to_bits(), 20.5_f64.to_bits());
    Ok(())
}

#[test]
fn empty_and_overlapping_observations_remain_observations() -> TestResult {
    let input = LiveTranscriptObservation::new(
        LiveTranscriptDirection::Input,
        LiveTranscriptRange::new(0.0, 20.0)?,
        "",
    );
    let output = LiveTranscriptObservation::new(
        LiveTranscriptDirection::Output,
        LiveTranscriptRange::new(10.0, 10.0)?,
        "overlap",
    );
    assert_eq!(input.text(), "");
    assert_eq!(output.direction(), LiveTranscriptDirection::Output);
    for value in [&input, &output] {
        let encoded = serde_json::to_value(value)?;
        for field in ["is_final", "turn_id", "response_id", "played", "permission"] {
            assert!(encoded.get(field).is_none());
        }
    }
    Ok(())
}

#[test]
fn observation_ranges_reject_nonfinite_negative_and_reversed_values() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            LiveTranscriptRange::new(value, 20.0),
            Err(LiveObservationValueError::NonFiniteRange)
        );
        assert_eq!(
            LiveTranscriptRange::new(0.0, value),
            Err(LiveObservationValueError::NonFiniteRange)
        );
    }
    for (start, end) in [(-1.0, 1.0), (2.0, 1.0), (0.0, -1.0)] {
        assert_eq!(
            LiveTranscriptRange::new(start, end),
            Err(LiveObservationValueError::InvalidRange)
        );
    }
}

#[test]
fn deserialization_cannot_bypass_range_validation_or_add_finality() {
    for range in [
        json!({"start_ms": -1, "end_ms": 1}),
        json!({"start_ms": 2, "end_ms": 1}),
        json!({"start_ms": null, "end_ms": 1}),
        json!({"start_ms": "1", "end_ms": 2}),
        json!({"start_ms": 1, "end_ms": 2, "is_final": true}),
    ] {
        assert!(serde_json::from_value::<LiveTranscriptRange>(range).is_err());
    }
    assert!(
        serde_json::from_value::<LiveTranscriptObservation>(json!({
            "direction": "input",
            "range": {"start_ms": 1, "end_ms": 2},
            "text": "not authority",
            "is_final": true,
        }))
        .is_err()
    );
}

#[test]
fn committed_sequence_is_positive_and_roundtrips() -> TestResult {
    assert_eq!(
        LiveObservationSeq::new(0),
        Err(LiveObservationValueError::ZeroSequence)
    );
    assert!(serde_json::from_value::<LiveObservationSeq>(json!(0)).is_err());
    for ordinal in [1, 2, u64::MAX] {
        let sequence = LiveObservationSeq::new(ordinal)?;
        assert_eq!(sequence.get(), ordinal);
        assert_eq!(serde_json::to_value(sequence)?, json!(ordinal));
        assert_eq!(
            serde_json::from_value::<LiveObservationSeq>(json!(ordinal))?,
            sequence
        );
    }
    Ok(())
}

#[test]
fn observation_debug_does_not_disclose_text() -> TestResult {
    let value = LiveTranscriptObservation::new(
        LiveTranscriptDirection::Input,
        LiveTranscriptRange::new(0.0, 1.0)?,
        "private-observation-canary",
    );
    let debug = format!("{value:?}");
    assert!(!debug.contains("private-observation-canary"));
    assert!(debug.contains("text_bytes"));
    Ok(())
}

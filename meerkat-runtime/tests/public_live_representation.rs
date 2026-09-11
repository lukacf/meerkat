use meerkat_runtime::live_delivery::{
    LiveContextAcknowledgmentAttribution, LiveContextChunkDeliveryState,
    LiveContinuationBatchEligibility, LiveContinuationState, LiveResultDeliveryState,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn public_result_write_facts_do_not_claim_consumption_or_playback() -> TestResult {
    for state in [
        LiveResultDeliveryState::Authorized,
        LiveResultDeliveryState::Claimed,
        LiveResultDeliveryState::NotEnqueued,
        LiveResultDeliveryState::WrittenConsumptionUnconfirmed,
        LiveResultDeliveryState::AmbiguousUnfenced,
        LiveResultDeliveryState::AmbiguousFenced,
        LiveResultDeliveryState::NotSent,
        LiveResultDeliveryState::RejectedAfterWrite,
        LiveResultDeliveryState::AbandonedByClose,
        LiveResultDeliveryState::AbandonedByReplacement,
    ] {
        assert_eq!(
            serde_json::from_value::<LiveResultDeliveryState>(serde_json::to_value(state)?)?,
            state
        );
    }
    for invalid in [
        "provider_processed",
        "consumed",
        "heard",
        "playback_complete",
    ] {
        assert!(serde_json::from_value::<LiveResultDeliveryState>(json!(invalid)).is_err());
    }
    Ok(())
}

#[test]
fn continuation_eligibility_is_separate_from_attempt_delivery() -> TestResult {
    let eligibility = LiveContinuationBatchEligibility::Spent;
    let outcome = LiveContinuationState::WrittenConsumptionUnconfirmed;
    assert_eq!(serde_json::to_value(eligibility)?, json!("spent"));
    assert_eq!(
        serde_json::to_value(outcome)?,
        json!("written_consumption_unconfirmed")
    );
    assert!(
        serde_json::from_value::<LiveContinuationBatchEligibility>(serde_json::to_value(outcome)?)
            .is_err()
    );
    assert!(
        serde_json::from_value::<LiveContinuationState>(serde_json::to_value(eligibility)?)
            .is_err()
    );
    Ok(())
}

#[test]
fn context_write_and_correlated_injection_are_not_the_same_fact() -> TestResult {
    for state in [
        LiveContextChunkDeliveryState::NotClaimed,
        LiveContextChunkDeliveryState::Claimed,
        LiveContextChunkDeliveryState::NotEnqueued,
        LiveContextChunkDeliveryState::WrittenInjectionUnconfirmed,
        LiveContextChunkDeliveryState::CorrelatedInjectionObserved,
        LiveContextChunkDeliveryState::AmbiguousUnfenced,
        LiveContextChunkDeliveryState::AmbiguousFenced,
        LiveContextChunkDeliveryState::AbandonedBeforeWrite,
    ] {
        assert_eq!(
            serde_json::from_value::<LiveContextChunkDeliveryState>(serde_json::to_value(state)?)?,
            state
        );
    }
    assert_ne!(
        LiveContextChunkDeliveryState::WrittenInjectionUnconfirmed,
        LiveContextChunkDeliveryState::CorrelatedInjectionObserved
    );
    Ok(())
}

#[test]
fn uncorrelated_acknowledgment_has_no_chunk_identity() -> TestResult {
    let uncorrelated = serde_json::to_value(LiveContextAcknowledgmentAttribution::Uncorrelated {})?;
    assert_eq!(uncorrelated, json!({"kind": "uncorrelated"}));
    assert!(
        serde_json::from_value::<LiveContextAcknowledgmentAttribution>(
            json!({"kind": "uncorrelated", "client_event_id": "guess"})
        )
        .is_err()
    );
    let correlated = LiveContextAcknowledgmentAttribution::Correlated {
        client_event_id: "exact-observed-correlation".into(),
    };
    assert_eq!(
        serde_json::from_value::<LiveContextAcknowledgmentAttribution>(serde_json::to_value(
            &correlated
        )?)?,
        correlated
    );
    Ok(())
}

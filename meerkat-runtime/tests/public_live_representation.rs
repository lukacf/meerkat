use meerkat_runtime::live_delivery::{
    LiveContextAcknowledgmentAttribution, LiveContextChunkDeliveryState,
    LiveContinuationBatchEligibility, LiveContinuationState, LiveResultDeliveryState,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn live_input_reference_cannot_encode_as_operator_prompt_or_runless_run() -> TestResult {
    use meerkat_runtime::live_request::{
        InputRunIsolation, LiveAdmissionFateRecord, LiveExecutionRequestRecord,
    };
    let value = json!({
        "kind":"live_request",
        "provenance":{
            "request_id":uuid::Uuid::from_u128(1),
            "source":{"session_id":uuid::Uuid::from_u128(2),"channel_id":"voice",
                "source":{"kind":"client_delegation","delegation":"d"}},
            "evidence_kind":"application_snapshot","request_digest":vec![1;32]
        },
        "source_row":vec![2;32]
    });
    let record: LiveExecutionRequestRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(&record)?, value);
    assert!(matches!(
        record.isolation(),
        InputRunIsolation::ExclusiveLiveRequest { .. }
    ));
    for kind in ["operator", "prompt", "ordinary"] {
        let mut changed = value.clone();
        changed["kind"] = json!(kind);
        assert!(serde_json::from_value::<LiveExecutionRequestRecord>(changed).is_err());
    }
    for value in [
        json!({"kind":"unknown"}),
        json!({"kind":"cancelled_without_run","reason":"operator_requested"}),
        json!({"kind":"refused","reason":"permission_denied"}),
        json!({"kind":"terminal_without_delivery","input_id":uuid::Uuid::from_u128(5)}),
    ] {
        let fate: LiveAdmissionFateRecord = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(fate)?, value);
        let mut forged = value;
        forged["run_id"] = json!(uuid::Uuid::from_u128(6));
        assert!(serde_json::from_value::<LiveAdmissionFateRecord>(forged).is_err());
    }
    Ok(())
}

fn continuation_image() -> serde_json::Value {
    let batch = |response: &str| {
        json!({
            "channel_id":"voice","delegation":"d","response":response
        })
    };
    let output = |value: u128| {
        json!({
            "call_id":format!("call-{value}"),"attempt_id":uuid::Uuid::from_u128(value),
            "payload":{"sequence":value,"digest":vec![1;32]},
            "physical_write":"written_consumption_unconfirmed","later_rejection":null
        })
    };
    json!({
        "format":"live_ledger_v1","session_id":uuid::Uuid::from_u128(100),
        "channel_id":"voice","send_generation":1,
        "batches":[
            {"key":batch("a"),"outputs":[output(1)],"eligibility":"spent","claiming_attempt":uuid::Uuid::from_u128(10)},
            {"key":batch("b"),"outputs":[output(2)],"eligibility":"eligible_unclaimed","claiming_attempt":null}
        ],
        "claims":[{
            "attempt_id":uuid::Uuid::from_u128(10),"send_generation":1,
            "members":[batch("a")],"claim_commit":{"revision":1,"digest":vec![2;32]},
            "delivery":"written_consumption_unconfirmed"
        }],
        "not_attempted":[]
    })
}

#[test]
fn complete_continuation_join_roundtrips_with_written_not_consumed_truth() -> TestResult {
    use meerkat_runtime::live_ledger::attempt::LiveContinuationSnapshot;
    let value = continuation_image();
    let restored: LiveContinuationSnapshot = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(&restored)?, value);
    assert_eq!(restored.claims().len(), 1);
    assert_eq!(restored.batches().len(), 2);
    Ok(())
}

#[test]
fn empty_subset_superset_and_overlapping_claims_cannot_reuse_spent_members() -> TestResult {
    use meerkat_runtime::live_ledger::attempt::LiveContinuationSnapshot;
    let original = continuation_image();
    let a = original["batches"][0]["key"].clone();
    let b = original["batches"][1]["key"].clone();
    for members in [
        json!([]),
        json!([a.clone()]),
        json!([a.clone(), b]),
        json!([a.clone(), a]),
    ] {
        let mut value = original.clone();
        let mut another = value["claims"][0].clone();
        another["attempt_id"] = json!(uuid::Uuid::from_u128(11));
        another["members"] = members;
        another["delivery"] = json!("claimed");
        value["claims"]
            .as_array_mut()
            .ok_or("claims")?
            .push(another);
        assert!(serde_json::from_value::<LiveContinuationSnapshot>(value).is_err());
    }
    let mut disjoint = original;
    disjoint["batches"][1]["eligibility"] = json!("claimed_by_attempt");
    disjoint["batches"][1]["claiming_attempt"] = json!(uuid::Uuid::from_u128(11));
    let mut next = disjoint["claims"][0].clone();
    next["attempt_id"] = json!(uuid::Uuid::from_u128(11));
    next["members"] = json!([b]);
    next["delivery"] = json!("claimed");
    disjoint["claims"]
        .as_array_mut()
        .ok_or("claims")?
        .push(next);
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(disjoint.clone()).is_ok());
    disjoint["claims"][0]["delivery"] = json!("claimed");
    disjoint["batches"][0]["eligibility"] = json!("claimed_by_attempt");
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(disjoint).is_err());
    Ok(())
}

#[test]
fn continuation_requires_written_outputs_and_one_current_active_attempt() -> TestResult {
    use meerkat_runtime::live_ledger::attempt::LiveContinuationSnapshot;
    for state in [
        "not_sent",
        "not_enqueued",
        "rejected_after_write",
        "abandoned_by_close",
        "ambiguous_fenced",
    ] {
        let mut value = continuation_image();
        value["batches"][1]["outputs"][0]["physical_write"] = json!(state);
        assert!(serde_json::from_value::<LiveContinuationSnapshot>(value).is_err());
    }
    for field in ["claiming_attempt", "outputs"] {
        let mut value = continuation_image();
        value["batches"][0][field] = if field == "outputs" {
            json!([])
        } else {
            serde_json::Value::Null
        };
        assert!(serde_json::from_value::<LiveContinuationSnapshot>(value).is_err());
    }
    let mut stale = continuation_image();
    stale["send_generation"] = json!(2);
    stale["claims"][0]["delivery"] = json!("claimed");
    stale["batches"][0]["eligibility"] = json!("claimed_by_attempt");
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(stale).is_err());
    Ok(())
}

#[test]
fn late_rejection_supplements_spent_write_without_reopening_eligibility() -> TestResult {
    use meerkat_runtime::live_ledger::attempt::LiveContinuationSnapshot;
    let mut spent = continuation_image();
    spent["batches"][0]["outputs"][0]["later_rejection"] =
        json!({"sequence":3,"digest":vec![3;32]});
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(spent.clone()).is_ok());
    spent["batches"][0]["eligibility"] = json!("eligible_unclaimed");
    spent["batches"][0]["claiming_attempt"] = serde_json::Value::Null;
    spent["claims"] = json!([]);
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(spent).is_err());
    Ok(())
}

#[test]
fn impossible_output_has_explicit_fenced_not_attempted_image_without_attempt_id() -> TestResult {
    use meerkat_runtime::live_ledger::attempt::LiveContinuationSnapshot;
    let mut value = continuation_image();
    value["batches"][1]["eligibility"] = json!("abandoned");
    value["batches"][1]["outputs"][0]["physical_write"] = json!("not_sent");
    value["not_attempted"] = json!([{
        "members":[value["batches"][1]["key"].clone()],
        "missing_required_outputs":[uuid::Uuid::from_u128(2)],
        "fenced_send_generation":1
    }]);
    let decoded: LiveContinuationSnapshot = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, value);
    for missing in [
        json!([]),
        json!([uuid::Uuid::from_u128(99)]),
        json!([uuid::Uuid::from_u128(2), uuid::Uuid::from_u128(2)]),
    ] {
        let mut changed = value.clone();
        changed["not_attempted"][0]["missing_required_outputs"] = missing;
        assert!(serde_json::from_value::<LiveContinuationSnapshot>(changed).is_err());
    }
    value["not_attempted"][0]["attempt_id"] = json!(uuid::Uuid::from_u128(11));
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(value).is_err());
    Ok(())
}

#[test]
fn continuation_images_reject_unknown_versions_foreign_channels_and_extra_proof() -> TestResult {
    use meerkat_runtime::live_ledger::attempt::LiveContinuationSnapshot;
    for (pointer, replacement) in [
        ("/format", json!("live_ledger_v2")),
        ("/channel_id", json!("another-channel")),
        ("/batches/0/key/delegation", serde_json::Value::Null),
        ("/batches/0/key/response", json!("x".repeat(129))),
        ("/claims/0/claim_commit/revision", json!(0)),
        ("/claims/0/delivery", json!("provider_processed")),
    ] {
        let mut value = continuation_image();
        *value.pointer_mut(pointer).ok_or("fixture field")? = replacement;
        assert!(
            serde_json::from_value::<LiveContinuationSnapshot>(value).is_err(),
            "{pointer}"
        );
    }
    let mut value = continuation_image();
    value["claims"][0]["set_digest_is_permission"] = json!(true);
    assert!(serde_json::from_value::<LiveContinuationSnapshot>(value).is_err());
    Ok(())
}

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

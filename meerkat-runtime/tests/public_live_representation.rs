use meerkat_runtime::live_delivery::{
    LiveContextAcknowledgmentAttribution, LiveContextChunkDeliveryState,
    LiveContinuationBatchEligibility, LiveContinuationState, LiveResultDeliveryState,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn retention_image() -> serde_json::Value {
    use meerkat_runtime::live_resources::LiveActiveResource;
    let active = LiveActiveResource::ALL
        .into_iter()
        .map(|kind| {
            (
                kind,
                meerkat_runtime::live_resources::LiveResourceCharge::default(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    json!({
        "used":{"records":1000,"encoded_bytes":1048576},
        "reserved":{"records":10,"encoded_bytes":65536},
        "quota":{"records":1000000,"encoded_bytes":268435456},
        "active":active,
        "active_context_chunks":0
    })
}

#[test]
fn settled_history_does_not_occupy_unsettled_slots_or_reset_durable_charge() -> TestResult {
    use meerkat_runtime::live_resources::{LiveActiveResource, LiveRetentionAccounting};
    let image = retention_image();
    let accounting: LiveRetentionAccounting = serde_json::from_value(image.clone())?;
    assert_eq!(serde_json::to_value(&accounting)?, image);
    assert_eq!(accounting.used().records, 1000);
    for kind in LiveActiveResource::ALL {
        assert_eq!(accounting.active(kind).records, 0);
        let mut bounded = image.clone();
        let key = serde_json::to_value(kind)?
            .as_str()
            .ok_or("resource name")?
            .to_owned();
        bounded["active"][&key] = serde_json::to_value(kind.limit())?;
        let restored: LiveRetentionAccounting = serde_json::from_value(bounded.clone())?;
        assert_eq!(restored.active(kind), kind.limit());
        for field in ["records", "encoded_bytes"] {
            let mut overflow = bounded.clone();
            overflow["active"][&key][field] =
                json!(overflow["active"][&key][field].as_u64().ok_or("counter")? + 1);
            assert!(serde_json::from_value::<LiveRetentionAccounting>(overflow).is_err());
        }
    }
    Ok(())
}

#[test]
fn retention_requires_complete_counted_byte_and_shared_capacity_accounting() -> TestResult {
    use meerkat_runtime::live_resources::LiveRetentionAccounting;
    let original = retention_image();
    for changed in [
        json!({"records":1000000,"encoded_bytes":1048576}),
        json!({"records":1000,"encoded_bytes":268435456}),
        json!({"records":u64::MAX,"encoded_bytes":u64::MAX}),
    ] {
        let mut image = original.clone();
        image["used"] = changed;
        assert!(serde_json::from_value::<LiveRetentionAccounting>(image).is_err());
    }
    let mut incomplete = original.clone();
    incomplete["active"]
        .as_object_mut()
        .ok_or("active")?
        .remove("work");
    assert!(serde_json::from_value::<LiveRetentionAccounting>(incomplete).is_err());
    let mut shared = original.clone();
    shared["active"]["outputs"] = json!({"records":128,"encoded_bytes":4194304});
    shared["active"]["continuations"] = json!({"records":1,"encoded_bytes":1});
    assert!(serde_json::from_value::<LiveRetentionAccounting>(shared).is_err());
    for chunks in [1, 65] {
        let mut image = original.clone();
        image["active_context_chunks"] = json!(chunks);
        assert!(serde_json::from_value::<LiveRetentionAccounting>(image).is_err());
    }
    let mut context = original;
    context["active"]["context_plans"] = json!({"records":8,"encoded_bytes":65536});
    context["active_context_chunks"] = json!(64);
    assert!(serde_json::from_value::<LiveRetentionAccounting>(context).is_ok());
    Ok(())
}

fn send_attempt_image() -> serde_json::Value {
    json!({
        "format":"live_ledger_v1",
        "session_id":uuid::Uuid::from_u128(100),
        "channel_id":"voice",
        "attempt_id":uuid::Uuid::from_u128(200),
        "send_generation":3,
        "target":{"kind":"function_output","batch":{
            "channel_id":"voice","delegation":"d","response":"r"
        },"call_id":"call"},
        "payload":{"sequence":1,"digest":vec![1;32]},
        "disposition":{"kind":"authorized"},
        "later_rejection":null
    })
}

#[test]
fn send_attempt_images_preserve_claim_write_fence_and_abandonment_without_retry_authority()
-> TestResult {
    use meerkat_runtime::live_ledger::send_attempt::LiveSendAttemptRecord;
    let claim = json!({"revision":2,"digest":vec![2;32]});
    let feedback = json!({"sequence":3,"digest":vec![3;32]});
    let fence = json!({
        "fenced_generation":3,"successor_generation":4,
        "commit":{"revision":3,"digest":vec![4;32]}
    });
    for disposition in [
        json!({"kind":"authorized"}),
        json!({"kind":"claimed","claim":claim}),
        json!({"kind":"not_enqueued","claim":claim,"feedback":feedback}),
        json!({"kind":"written_consumption_unconfirmed","claim":claim,"feedback":feedback}),
        json!({"kind":"ambiguous_unfenced","claim":claim}),
        json!({"kind":"ambiguous_fenced","claim":claim,"fence":fence}),
        json!({"kind":"not_sent","claim":claim,"no_write_feedback":feedback}),
        json!({"kind":"rejected_before_write","no_write":{"kind":"before_claim"},"rejection":feedback}),
        json!({"kind":"rejected_before_write","no_write":{"kind":"not_enqueued","claim":claim,"feedback":feedback},"rejection":{"sequence":4,"digest":vec![4;32]}}),
        json!({"kind":"abandoned_before_claim","reason":"channel_closed"}),
        json!({"kind":"abandoned_before_claim","reason":"request_cancelled"}),
        json!({"kind":"abandoned_not_enqueued","claim":claim,"no_write_feedback":feedback,"reason":"channel_replaced"}),
    ] {
        let mut image = send_attempt_image();
        image["disposition"] = disposition;
        let bytes = serde_json::to_vec(&image)?;
        let record: LiveSendAttemptRecord = serde_json::from_slice(&bytes)?;
        assert_eq!(serde_json::to_value(&record)?, image);
        assert_eq!(
            serde_json::from_slice::<LiveSendAttemptRecord>(&serde_json::to_vec(&record)?)?,
            record,
        );
        for field in [
            "retry_authorized",
            "provider_processed",
            "private_bridge_proof",
        ] {
            let mut forged = image.clone();
            forged["disposition"][field] = json!(true);
            assert!(serde_json::from_value::<LiveSendAttemptRecord>(forged).is_err());
        }
    }
    Ok(())
}

#[test]
fn lost_feedback_cannot_encode_not_sent_or_fenced_without_exact_evidence() -> TestResult {
    use meerkat_runtime::live_ledger::send_attempt::LiveSendAttemptRecord;
    let claim = json!({"revision":2,"digest":vec![2;32]});
    for disposition in [
        json!({"kind":"claimed"}),
        json!({"kind":"not_sent","claim":claim}),
        json!({"kind":"rejected_before_write","rejection":{"sequence":3,"digest":vec![3;32]}}),
        json!({"kind":"rejected_before_write","no_write":{"kind":"not_enqueued","claim":claim},"rejection":{"sequence":3,"digest":vec![3;32]}}),
        json!({"kind":"rejected_before_write","no_write":{"kind":"before_claim"},"rejection":{"sequence":1,"digest":vec![3;32]}}),
        json!({"kind":"abandoned_not_enqueued","claim":claim,"reason":"channel_closed"}),
        json!({"kind":"written_consumption_unconfirmed","claim":claim}),
        json!({"kind":"written_consumption_unconfirmed","claim":claim,"feedback":{"sequence":1,"digest":vec![3;32]}}),
        json!({"kind":"ambiguous_fenced","claim":claim}),
        json!({"kind":"ambiguous_fenced","claim":claim,"fence":{
            "fenced_generation":2,"successor_generation":4,"commit":{"revision":3,"digest":vec![4;32]}
        }}),
        json!({"kind":"ambiguous_fenced","claim":claim,"fence":{
            "fenced_generation":3,"successor_generation":3,"commit":{"revision":3,"digest":vec![4;32]}
        }}),
        json!({"kind":"ambiguous_fenced","claim":claim,"fence":{
            "fenced_generation":3,"successor_generation":4,"commit":{"revision":2,"digest":vec![4;32]}
        }}),
        json!({"kind":"provider_processed","claim":claim}),
        json!({"kind":"playback_completed","claim":claim}),
    ] {
        let mut image = send_attempt_image();
        image["disposition"] = disposition;
        assert!(serde_json::from_value::<LiveSendAttemptRecord>(image).is_err());
    }
    Ok(())
}

#[test]
fn send_attempt_target_and_late_rejection_remain_exact_and_distinct_from_physical_write()
-> TestResult {
    use meerkat_runtime::live_ledger::send_attempt::LiveSendAttemptRecord;
    let original = send_attempt_image();
    let batch = original["target"]["batch"].clone();
    for target in [
        original["target"].clone(),
        json!({"kind":"continuation","members":[batch]}),
        json!({"kind":"context_chunk","plan_id":uuid::Uuid::from_u128(300),"chunk_index":63}),
    ] {
        let mut image = original.clone();
        image["target"] = target;
        let decoded: LiveSendAttemptRecord = serde_json::from_value(image.clone())?;
        assert_eq!(serde_json::to_value(decoded)?, image);
    }
    for target in [
        json!({"kind":"continuation","members":[]}),
        json!({"kind":"continuation","members":[batch,batch]}),
        json!({"kind":"continuation","members":[{"channel_id":"other","delegation":"d","response":"r"}]}),
        json!({"kind":"function_output","batch":batch,"call_id":""}),
        json!({"kind":"context_chunk","plan_id":uuid::Uuid::from_u128(300),"chunk_index":64}),
    ] {
        let mut image = original.clone();
        image["target"] = target;
        assert!(serde_json::from_value::<LiveSendAttemptRecord>(image).is_err());
    }
    let mut written = original;
    written["disposition"] = json!({
        "kind":"written_consumption_unconfirmed",
        "claim":{"revision":2,"digest":vec![2;32]},
        "feedback":{"sequence":3,"digest":vec![3;32]}
    });
    written["later_rejection"] = json!({"sequence":4,"digest":vec![4;32]});
    let decoded: LiveSendAttemptRecord = serde_json::from_value(written.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, written);
    let mut raced = written.clone();
    raced["later_rejection"]["sequence"] = json!(2);
    let decoded: LiveSendAttemptRecord = serde_json::from_value(raced.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, raced);
    for invalid_sequence in [1, 3] {
        let mut invalid = written.clone();
        invalid["later_rejection"]["sequence"] = json!(invalid_sequence);
        assert!(serde_json::from_value::<LiveSendAttemptRecord>(invalid).is_err());
    }
    written["disposition"] = json!({"kind":"authorized"});
    assert!(serde_json::from_value::<LiveSendAttemptRecord>(written).is_err());
    Ok(())
}

fn live_input_reference_image() -> serde_json::Value {
    json!({
        "kind":"live_request",
        "provenance":{
            "request_id":uuid::Uuid::from_u128(1),
            "source":{"session_id":uuid::Uuid::from_u128(2),"channel_id":"voice",
                "source":{"kind":"client_delegation","delegation":"d"}},
            "evidence_kind":"application_snapshot","request_digest":vec![1;32]
        },
        "source_row":vec![2;32]
    })
}

fn callback_input_reference_image() -> serde_json::Value {
    let mut image = live_input_reference_image();
    image["kind"] = json!("callback_continuation");
    image["continuation"] = json!({
        "target": {
            "session_id": uuid::Uuid::from_u128(2),
            "run_id": uuid::Uuid::from_u128(3),
            "execution_scope": uuid::Uuid::from_u128(4),
            "execution_boundary": uuid::Uuid::from_u128(5),
            "batch_digest": vec![3;32]
        },
        "results_digest": vec![4;32]
    });
    image["admission_commit"] = json!({"revision": 2, "digest": vec![5;32]});
    image
}

#[tokio::test]
async fn ordinary_ingress_refuses_unsealed_live_input_without_creating_a_row() -> TestResult {
    use meerkat_runtime::accept::{AcceptOutcome, RejectReason};
    use meerkat_runtime::driver::EphemeralRuntimeDriver;
    use meerkat_runtime::identifiers::{InputKind, LogicalRuntimeId};
    use meerkat_runtime::input::{
        Input, InputDurability, InputHeader, InputOrigin, InputVisibility, LiveRequestInput,
        PromptInput,
    };
    use meerkat_runtime::traits::RuntimeDriver;

    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("unsealed-live"));
    for (image, expected_kind) in [
        (live_input_reference_image(), InputKind::LiveRequest),
        (
            callback_input_reference_image(),
            InputKind::LiveCallbackContinuation,
        ),
    ] {
        for source in [InputOrigin::LiveRequest, InputOrigin::Operator] {
            for durability in [
                InputDurability::Durable,
                InputDurability::Ephemeral,
                InputDurability::Derived,
            ] {
                let input_id = meerkat_core::lifecycle::InputId::new();
                let input = Input::LiveRequest(LiveRequestInput {
                    header: InputHeader {
                        id: input_id.clone(),
                        timestamp: chrono::Utc::now(),
                        source: source.clone(),
                        durability,
                        visibility: InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    request: serde_json::from_value(image.clone())?,
                });
                let encoded = serde_json::to_value(&input)?;
                assert_eq!(encoded["input_type"], "live_request");
                assert!(encoded.get("content").is_none());
                let input: Input = serde_json::from_value(encoded)?;
                assert_eq!(input.kind(), expected_kind);
                assert!(matches!(
                    driver.accept_input(input).await?,
                    AcceptOutcome::Rejected {
                        reason: RejectReason::LiveRequestRequiresGrant,
                        ..
                    }
                ));
                assert!(driver.input_state(&input_id).is_none());
            }
        }
    }
    let mut forged = PromptInput::new("ordinary text cannot mint a Live admission", None);
    forged.header.source = InputOrigin::LiveRequest;
    assert!(matches!(
        driver.accept_input(Input::Prompt(forged)).await?,
        AcceptOutcome::Rejected {
            reason: RejectReason::LiveRequestRequiresGrant,
            ..
        }
    ));
    Ok(())
}

#[test]
fn live_input_reference_cannot_encode_as_operator_prompt_or_runless_run() -> TestResult {
    use meerkat_runtime::live_request::{
        InputRunIsolation, LiveAdmissionFateRecord, LiveExecutionRequestRecord,
    };
    let value = live_input_reference_image();
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

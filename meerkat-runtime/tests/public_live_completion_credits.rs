use meerkat_runtime::live_resources::{
    LiveCompletionObligation, LiveResourceArithmeticError, LiveResourceCharge,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn impossible_joint_spending_cannot_leave_unfunded_record_slots() -> TestResult {
    use meerkat_runtime::live_ledger::completion_budget::{
        CompletionCreditReservation, CompletionEnvelopeBudgetV1,
    };
    use meerkat_runtime::live_resources::LIVE_EVENT_STORAGE_ALLOWANCE_BYTES;
    let budget =
        CompletionEnvelopeBudgetV1::for_obligation(LiveCompletionObligation::RequestChain)?;
    let minimum = budget.minimum_encoded_record_bytes() + LIVE_EVENT_STORAGE_ALLOWANCE_BYTES;
    let maximum = budget.maximum_encoded_record_bytes() + LIVE_EVENT_STORAGE_ALLOWANCE_BYTES;
    for spent in [
        LiveResourceCharge {
            records: 0,
            encoded_bytes: budget.total().encoded_bytes,
        },
        LiveResourceCharge {
            records: 0,
            encoded_bytes: 1,
        },
        LiveResourceCharge {
            records: 1,
            encoded_bytes: 0,
        },
        LiveResourceCharge {
            records: 1,
            encoded_bytes: minimum - 1,
        },
        LiveResourceCharge {
            records: 1,
            encoded_bytes: maximum + 1,
        },
    ] {
        assert!(CompletionCreditReservation::from_record(budget, spent).is_err());
        assert!(
            serde_json::from_value::<CompletionCreditReservation>(json!({
                "budget": budget, "spent": spent
            }))
            .is_err()
        );
    }
    for encoded_bytes in [minimum, maximum] {
        let record = CompletionCreditReservation::from_record(
            budget,
            LiveResourceCharge {
                records: 1,
                encoded_bytes,
            },
        )?;
        let remaining = record.remaining();
        assert!(remaining.encoded_bytes >= remaining.records * maximum);
    }
    Ok(())
}

fn extra_field_at_each_object(value: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut variants = Vec::new();
    if let serde_json::Value::Object(object) = value {
        let mut changed = value.clone();
        changed["unexpected_authority"] = json!(true);
        variants.push(changed);
        for (key, child) in object {
            for changed_child in extra_field_at_each_object(child) {
                let mut changed = value.clone();
                changed[key] = changed_child;
                variants.push(changed);
            }
        }
    }
    variants
}

#[test]
fn all_valid_utf8_result_byte_extremes_fit_and_invalid_content_rejects_before_encoding()
-> TestResult {
    use meerkat_runtime::live_ledger::completion::{
        LIVE_RESULT_MAX_BYTES, LiveCompletionEvent, LiveCompletionText,
    };
    use meerkat_runtime::live_ledger::completion_budget::{
        CompletionEnvelopeBudgetV1, maximal_completion_record,
    };
    let budget =
        CompletionEnvelopeBudgetV1::for_obligation(LiveCompletionObligation::FunctionOutput)?;
    for unit in ["\0", "\n", "\\", "\"", "a", "\u{00e9}", "\u{1f98a}"] {
        let text = unit.repeat(LIVE_RESULT_MAX_BYTES / unit.len());
        let output = LiveCompletionText::new(text.clone())?;
        let record = maximal_completion_record(LiveCompletionEvent::FunctionOutput {
            attempt_id: meerkat_core::ops::OperationId(uuid::Uuid::nil()),
            request_id: meerkat_core::ops::OperationId(uuid::Uuid::nil()),
            output,
        })?;
        let encoded = record.encode()?;
        assert!(encoded.bytes().len() as u64 <= budget.maximum_encoded_record_bytes());
        let value: serde_json::Value = serde_json::from_slice(encoded.bytes())?;
        assert_eq!(value["event"]["output"].as_str(), Some(text.as_str()));
        assert!(!format!("{encoded:?}").contains(&text));
    }
    assert!(
        LiveCompletionText::<LIVE_RESULT_MAX_BYTES>::new("a".repeat(LIVE_RESULT_MAX_BYTES + 1))
            .is_err()
    );
    assert!(
        serde_json::from_value::<LiveCompletionText<LIVE_RESULT_MAX_BYTES>>(json!(
            "a".repeat(LIVE_RESULT_MAX_BYTES + 1)
        ))
        .is_err()
    );
    Ok(())
}

#[test]
fn persisted_credits_derive_remaining_capacity_and_reject_overdrawn_images() -> TestResult {
    use meerkat_runtime::live_ledger::completion_budget::{
        CompletionCreditReservation, CompletionEnvelopeBudgetV1,
    };
    let budget =
        CompletionEnvelopeBudgetV1::for_obligation(LiveCompletionObligation::RequestChain)?;
    for spent in [
        LiveResourceCharge::default(),
        LiveResourceCharge {
            records: 1,
            encoded_bytes: 4096,
        },
        budget.total(),
    ] {
        let record = CompletionCreditReservation::from_record(budget, spent)?;
        assert_eq!(
            record.spent().checked_add(record.remaining())?,
            budget.total()
        );
        let bytes = serde_json::to_vec(&record)?;
        assert_eq!(
            serde_json::from_slice::<CompletionCreditReservation>(&bytes)?,
            record
        );
        let mut value = serde_json::to_value(record)?;
        value["remaining"] = json!({"records": 100, "encoded_bytes": 1000000});
        assert!(serde_json::from_value::<CompletionCreditReservation>(value).is_err());
    }
    for increment in [
        LiveResourceCharge {
            records: 1,
            encoded_bytes: 0,
        },
        LiveResourceCharge {
            records: 0,
            encoded_bytes: 1,
        },
    ] {
        let spent = budget.total().checked_add(increment)?;
        assert!(CompletionCreditReservation::from_record(budget, spent).is_err());
        let value = json!({"budget": budget, "spent": spent});
        assert!(serde_json::from_value::<CompletionCreditReservation>(value).is_err());
    }
    Ok(())
}

#[test]
fn text_at_full_unreserved_quota_cannot_spend_capacity_owed_to_every_obligation_class() -> TestResult
{
    let mut reserved = LiveResourceCharge::default();
    for obligation in [
        LiveCompletionObligation::ChannelControl,
        LiveCompletionObligation::RequestChain,
        LiveCompletionObligation::EffectStart,
        LiveCompletionObligation::FunctionOutput,
        LiveCompletionObligation::Continuation,
        LiveCompletionObligation::ContextChunk,
        LiveCompletionObligation::CallbackContinuation,
    ] {
        reserved = reserved.checked_add(obligation.base_budget()?)?;
    }
    let quota = LiveResourceCharge {
        records: 1_000_000,
        encoded_bytes: 256 * 1024 * 1024,
    };
    let used = quota.checked_sub(reserved)?;
    assert_eq!(
        quota.checked_sub(used.checked_add(reserved)?)?,
        LiveResourceCharge::default()
    );
    let new_text = LiveResourceCharge::for_event_record(b"\"new text\"")?;
    assert!(
        quota
            .checked_sub(used.checked_add(reserved)?.checked_add(new_text)?)
            .is_err()
    );
    for extra in [
        LiveResourceCharge {
            records: u64::MAX,
            encoded_bytes: 1,
        },
        LiveResourceCharge {
            records: 1,
            encoded_bytes: u64::MAX,
        },
    ] {
        assert!(extra.checked_mul(2).is_err());
    }
    Ok(())
}

#[test]
fn observation_and_completion_charges_use_the_same_exact_encoded_unit() -> TestResult {
    use meerkat_contracts::wire::live_observation::{
        LiveObservationRecord, LiveObservationWireCodecV1,
    };
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_observation::{
        LiveObservationSeq, LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    use meerkat_runtime::live_ledger::transcript::StoredLiveObservation;
    let fit = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
        sequence: LiveObservationSeq::new(1)?,
        channel_id: LiveChannelId::new("channel"),
        observation: LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(0.0, 0.5)?,
            "\0".repeat(1024),
        ),
    })?;
    let stored = StoredLiveObservation::from_fit(&fit);
    let bytes = LiveObservationWireCodecV1::encode_ledger_record(&stored)?;
    assert_eq!(
        stored.encoded_charge()?,
        LiveResourceCharge::for_event_record(&bytes)?
    );
    assert!(stored.encoded_charge()?.encoded_bytes > 6 * 1024);
    Ok(())
}

#[test]
fn record_and_encoded_byte_units_roundtrip_without_token_or_text_substitution() -> TestResult {
    let charge = LiveResourceCharge {
        records: 3,
        encoded_bytes: 4096,
    };
    assert_eq!(
        serde_json::to_value(charge)?,
        json!({"records": 3, "encoded_bytes": 4096})
    );
    assert_eq!(
        serde_json::from_value::<LiveResourceCharge>(serde_json::to_value(charge)?)?,
        charge
    );
    for invalid in [
        json!({"records": 3, "text_bytes": 4096}),
        json!({"records": 3, "encoded_bytes": 4096, "tokens": 100}),
        json!({"records": -1, "encoded_bytes": 4096}),
    ] {
        assert!(serde_json::from_value::<LiveResourceCharge>(invalid).is_err());
    }
    Ok(())
}

#[test]
fn addition_and_subtraction_preserve_both_units() -> TestResult {
    let base = LiveResourceCharge {
        records: 3,
        encoded_bytes: 50,
    };
    let extra = LiveResourceCharge {
        records: 2,
        encoded_bytes: 70,
    };
    let sum = base.checked_add(extra)?;
    assert_eq!(
        sum,
        LiveResourceCharge {
            records: 5,
            encoded_bytes: 120,
        }
    );
    assert_eq!(sum.checked_sub(extra)?, base);
    Ok(())
}

#[test]
fn arithmetic_fails_closed_in_either_dimension() {
    for maximum in [
        LiveResourceCharge {
            records: u64::MAX,
            encoded_bytes: 1,
        },
        LiveResourceCharge {
            records: 1,
            encoded_bytes: u64::MAX,
        },
    ] {
        assert_eq!(
            maximum.checked_add(LiveResourceCharge {
                records: 1,
                encoded_bytes: 1,
            }),
            Err(LiveResourceArithmeticError::Overflow)
        );
    }
    for decrement in [
        LiveResourceCharge {
            records: 2,
            encoded_bytes: 0,
        },
        LiveResourceCharge {
            records: 0,
            encoded_bytes: 2,
        },
    ] {
        assert_eq!(
            LiveResourceCharge {
                records: 1,
                encoded_bytes: 1,
            }
            .checked_sub(decrement),
            Err(LiveResourceArithmeticError::Underflow)
        );
    }
}

#[test]
fn every_declared_completion_kind_has_a_bounded_nonzero_base_budget() -> TestResult {
    for obligation in [
        LiveCompletionObligation::ChannelControl,
        LiveCompletionObligation::RequestChain,
        LiveCompletionObligation::EffectStart,
        LiveCompletionObligation::FunctionOutput,
        LiveCompletionObligation::Continuation,
        LiveCompletionObligation::ContextChunk,
        LiveCompletionObligation::CallbackContinuation,
    ] {
        let budget = obligation.base_budget()?;
        println!(
            "{obligation:?}: records={}, accounted_bytes={}",
            budget.records, budget.encoded_bytes
        );
        assert!(budget.records > 0 && budget.encoded_bytes > 0);
        assert!(budget.records <= 64);
        assert_eq!(budget.records, obligation.record_limit());
        assert_eq!(
            serde_json::from_value::<LiveCompletionObligation>(serde_json::to_value(obligation)?)?,
            obligation
        );
    }
    Ok(())
}

#[test]
fn every_maximum_record_and_all_owed_slots_fit_measured_completion_capacity() -> TestResult {
    use meerkat_runtime::live_ledger::completion_budget::{
        CompletionEnvelopeBudgetV1, maximal_completion_events, maximal_completion_record,
    };
    for event in maximal_completion_events()? {
        let record = maximal_completion_record(event)?;
        let encoded = record.encode()?;
        let budget = CompletionEnvelopeBudgetV1::for_obligation(encoded.obligation())?;
        assert!(encoded.bytes().len() as u64 <= budget.maximum_encoded_record_bytes());
        let mut used = LiveResourceCharge::default();
        let mut reserved = budget.total();
        for _ in 0..budget.total().records {
            let charge = encoded.charge();
            reserved = reserved.checked_sub(charge)?;
            used = used.checked_add(charge)?;
            assert_eq!(used.checked_add(reserved)?, budget.total());
        }
        assert_eq!(reserved.records, 0);
        let restored: meerkat_runtime::live_ledger::completion::LiveCompletionRecord =
            serde_json::from_slice(encoded.bytes())?;
        assert_eq!(restored, record);
        assert_eq!(restored.encode()?.bytes(), encoded.bytes());
    }
    Ok(())
}

#[test]
fn escaped_sixteen_kib_result_cannot_be_stranded_by_the_old_48_kib_estimate() -> TestResult {
    use meerkat_runtime::live_ledger::completion::{
        LIVE_RESULT_MAX_BYTES, LiveCompletionEvent, LiveCompletionText,
    };
    use meerkat_runtime::live_ledger::completion_budget::maximal_completion_record;
    let text = "\0".repeat(LIVE_RESULT_MAX_BYTES);
    let record = maximal_completion_record(LiveCompletionEvent::FunctionOutput {
        attempt_id: meerkat_core::ops::OperationId(uuid::Uuid::nil()),
        request_id: meerkat_core::ops::OperationId(uuid::Uuid::nil()),
        output: LiveCompletionText::new(text.clone())?,
    })?;
    let encoded = record.encode()?;
    assert!(encoded.bytes().len() > 96 * 1024);
    let budget = LiveCompletionObligation::FunctionOutput.base_budget()?;
    assert!(budget.encoded_bytes >= encoded.charge().encoded_bytes * budget.records);
    let restored: meerkat_runtime::live_ledger::completion::LiveCompletionRecord =
        serde_json::from_slice(encoded.bytes())?;
    let LiveCompletionEvent::FunctionOutput { output, .. } = restored.event else {
        return Err("function output variant lost".into());
    };
    assert_eq!(output.as_str(), text);
    Ok(())
}

#[test]
fn completion_budget_restore_rejects_downgrade_and_unknown_version() -> TestResult {
    use meerkat_runtime::live_ledger::completion_budget::CompletionEnvelopeBudgetV1;
    let budget =
        CompletionEnvelopeBudgetV1::for_obligation(LiveCompletionObligation::FunctionOutput)?;
    let value = serde_json::to_value(budget)?;
    assert_eq!(
        serde_json::from_value::<CompletionEnvelopeBudgetV1>(value.clone())?,
        budget
    );
    for field in ["records", "encoded_bytes"] {
        let mut altered = value.clone();
        altered["total"][field] = json!(1);
        assert!(serde_json::from_value::<CompletionEnvelopeBudgetV1>(altered).is_err());
    }
    let mut altered = value;
    altered["schema"] = json!("completion_credits_v0");
    assert!(serde_json::from_value::<CompletionEnvelopeBudgetV1>(altered).is_err());
    Ok(())
}

#[test]
fn every_completion_and_runless_terminal_case_rejects_extra_authority_fields() -> TestResult {
    use meerkat_runtime::live_ledger::completion::LiveCompletionRecord;
    use meerkat_runtime::live_ledger::completion_budget::{
        maximal_completion_events, maximal_completion_record,
    };
    for event in maximal_completion_events()? {
        let value = serde_json::to_value(maximal_completion_record(event)?)?;
        for altered in extra_field_at_each_object(&value) {
            assert!(serde_json::from_value::<LiveCompletionRecord>(altered).is_err());
        }
    }
    for value in [
        json!({"kind":"refused", "reason":"permission_denied", "run_id":uuid::Uuid::nil()}),
        json!({"kind":"cancelled_without_run", "reason":"operator_requested", "run_id":uuid::Uuid::nil()}),
    ] {
        assert!(
            serde_json::from_value::<
                meerkat_runtime::live_ledger::completion::LiveRequestCompletionFact,
            >(value)
            .is_err()
        );
    }
    Ok(())
}

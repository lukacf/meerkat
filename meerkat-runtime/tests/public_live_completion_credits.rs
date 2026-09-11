use meerkat_runtime::live_resources::{
    LiveCompletionObligation, LiveResourceArithmeticError, LiveResourceCharge,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
        let budget = obligation.base_budget();
        assert!(budget.records > 0 && budget.encoded_bytes > 0);
        assert!(budget.records <= 64 && budget.encoded_bytes <= 160 * 1024);
        assert_eq!(
            serde_json::from_value::<LiveCompletionObligation>(serde_json::to_value(obligation)?)?,
            obligation
        );
    }
    Ok(())
}

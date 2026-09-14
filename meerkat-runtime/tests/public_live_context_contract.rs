use meerkat_runtime::live_ledger::context::{LiveContextPlanFrontiers, LiveContextPlanRecord};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn plan() -> Value {
    let chunk = |source, sequence, attempt| {
        json!({
            "source_ordinal":source,"source_chunk_index":0,"source_chunk_count":1,
            "payload":{"sequence":sequence,"digest":vec![1;32]},
            "content_digest":vec![2;32],"decoded_utf8_bytes":40,
            "attempt":attempt,"write":if source == 2 { "written_injection_unconfirmed" } else { "not_claimed" },
            "correlated_injection":null
        })
    };
    json!({
        "format":"live_ledger_v1","plan_id":uuid::Uuid::from_u128(10),
        "session_id":uuid::Uuid::from_u128(1),"channel_id":"destination",
        "origin":{"kind":"continuous_observations","head":{
            "format":"live_ledger_v1","session_id":uuid::Uuid::from_u128(1),
            "generation":1,"revision":2,"event_count":3,"prefix_digest":vec![3;32]
        }},
        "intent":"thinking","policy":"recent_authorized_suffix",
        "considered":{"after":0,"through":3},
        "omissions":[{"source":{"after":0,"through":1},"reason":"older_than_window"}],
        "chunks":[chunk(2,4,json!({
            "attempt_id":uuid::Uuid::from_u128(20),"event_id":"ctx-a",
            "claim_commit":{"revision":3,"digest":vec![4;32]}
        })),chunk(3,5,Value::Null)],
        "uncorrelated_injections":[{"start_ms":2.5,"end_ms":2.5}]
    })
}

#[test]
fn considered_attempted_and_correlated_frontiers_remain_independent() -> TestResult {
    let value = plan();
    let decoded: LiveContextPlanRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(&decoded)?, value);
    assert_eq!(
        decoded.frontiers(),
        LiveContextPlanFrontiers {
            considered_source_through: 3,
            attempted_chunk_count: 1,
            correlated_injection_chunk_count: 0,
        }
    );
    let mut correlated = value;
    correlated["chunks"][0]["correlated_injection"] = json!({
        "event_id":"ctx-a","estimated_range":{"start_ms":2.5,"end_ms":2.5}
    });
    let decoded: LiveContextPlanRecord = serde_json::from_value(correlated)?;
    assert_eq!(decoded.frontiers().attempted_chunk_count, 1);
    assert_eq!(decoded.frontiers().correlated_injection_chunk_count, 1);
    Ok(())
}

#[test]
fn no_id_ack_cannot_name_a_chunk_and_out_of_order_ack_cannot_advance_a_prefix() -> TestResult {
    let mut value = plan();
    value["uncorrelated_injections"][0]["chunk_index"] = json!(0);
    assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_err());
    let mut value = plan();
    value["chunks"][1]["attempt"] = json!({
        "attempt_id":uuid::Uuid::from_u128(21),"event_id":"ctx-b",
        "claim_commit":{"revision":4,"digest":vec![5;32]}
    });
    value["chunks"][1]["write"] = json!("written_injection_unconfirmed");
    value["chunks"][1]["correlated_injection"] = json!({
        "event_id":"ctx-b","estimated_range":{"start_ms":5.0,"end_ms":5.25}
    });
    let decoded: LiveContextPlanRecord = serde_json::from_value(value.clone())?;
    assert_eq!(decoded.frontiers().attempted_chunk_count, 2);
    assert_eq!(decoded.frontiers().correlated_injection_chunk_count, 0);
    value["chunks"][0]["correlated_injection"] = json!({
        "event_id":"ctx-a","estimated_range":{"start_ms":2.0,"end_ms":3.125}
    });
    let decoded: LiveContextPlanRecord = serde_json::from_value(value)?;
    assert_eq!(decoded.frontiers().correlated_injection_chunk_count, 2);
    Ok(())
}

#[test]
fn injection_needs_exact_correlation_and_a_retained_written_attempt() -> TestResult {
    for event in [json!("another-id"), Value::Null] {
        let mut value = plan();
        value["chunks"][0]["correlated_injection"] = json!({
            "event_id":event,"estimated_range":{"start_ms":0.0,"end_ms":1.0}
        });
        assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_err());
    }
    for state in [
        "not_claimed",
        "not_enqueued",
        "claimed",
        "ambiguous_unfenced",
        "abandoned_before_write",
    ] {
        let mut value = plan();
        value["chunks"][0]["write"] = json!(state);
        value["chunks"][0]["correlated_injection"] = json!({
            "event_id":"ctx-a","estimated_range":{"start_ms":0.0,"end_ms":1.0}
        });
        assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_err());
    }
    Ok(())
}

#[test]
fn whole_selected_sources_and_explicit_omissions_cover_the_considered_window() -> TestResult {
    for omission in [
        json!([]),
        json!([{"source":{"after":0,"through":2},"reason":"older_than_window"}]),
    ] {
        let mut value = plan();
        value["omissions"] = omission;
        assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_err());
    }
    let mut value = plan();
    value["chunks"][0]["source_chunk_count"] = json!(2);
    assert!(serde_json::from_value::<LiveContextPlanRecord>(value.clone()).is_err());
    let mut second_part = value["chunks"][0].clone();
    second_part["source_chunk_index"] = json!(1);
    second_part["attempt"] = Value::Null;
    second_part["write"] = json!("not_claimed");
    value["chunks"]
        .as_array_mut()
        .ok_or("chunks")?
        .insert(1, second_part);
    assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_ok());
    Ok(())
}

#[test]
fn reject_policy_does_not_silently_select_an_older_or_over_budget_suffix() -> TestResult {
    let mut value = plan();
    value["policy"] = json!("reject");
    assert!(serde_json::from_value::<LiveContextPlanRecord>(value.clone()).is_err());
    value["omissions"][0]["reason"] = json!("excluded_by_policy");
    assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_ok());
    Ok(())
}

#[test]
fn source_content_cannot_become_profile_instructions_or_cross_session_proof() -> TestResult {
    for (pointer, replacement) in [
        ("/intent", json!("instructions")),
        ("/session_id", json!(uuid::Uuid::from_u128(999))),
        ("/format", json!("live_ledger_v2")),
        ("/origin/head/revision", json!(0)),
        ("/chunks/0/decoded_utf8_bytes", json!(401)),
    ] {
        let mut value = plan();
        *value.pointer_mut(pointer).ok_or("fixture field")? = replacement;
        assert!(
            serde_json::from_value::<LiveContextPlanRecord>(value).is_err(),
            "{pointer}"
        );
    }
    let mut value = plan();
    value["chunks"][0]["provider_heard"] = json!(true);
    assert!(serde_json::from_value::<LiveContextPlanRecord>(value).is_err());
    Ok(())
}

use meerkat_runtime::live_grant::{LiveExecutionGrant, LiveExecutionGrantRecord};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn grant() -> Value {
    json!({
        "format":"v1",
        "grant_id":"00000000-0000-0000-0000-000000000001",
        "activation_id":"voice-activation",
        "declaration":{
            "issuer_realm":"owner", "profile_id":"voice", "profile_revision":vec![2;32],
            "requesting_realms":["caller"],
            "executor":{"kind":"session","session_id":"00000000-0000-0000-0000-000000000003"},
            "allowed_evidence":["application_snapshot"],
            "permission":{
                "allowed_mutations":["read_only"],
                "tools":{"kind":"allow_listed","names":["read"]},
                "limits":{"max_requests":128,"max_concurrent_requests":1,
                    "max_effects_per_request":10,"max_tokens_per_request":8192,"max_duration_ms":30000}
            },
            "expires_at":"2026-09-12T00:00:00Z",
            "generation":4,
            "revoke_policy":"cancel_pending_and_request_running_cancellation"
        },
        "requesting_realm":"caller",
        "executor":{
            "selector":{"kind":"session","session_id":"00000000-0000-0000-0000-000000000003"},
            "binding":{
                "session_id":"00000000-0000-0000-0000-000000000003",
                "realm":"executor", "runtime_epoch":"00000000-0000-0000-0000-000000000005",
                "binding_generation":6
            }
        },
        "issued_at":"2026-09-11T00:00:00Z"
    })
}

#[test]
fn exact_grant_record_keeps_issuer_requester_executor_and_permission_distinct() -> TestResult {
    let value = grant();
    let record: LiveExecutionGrantRecord<()> = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(&record)?, value);
    assert_eq!(record.grant_ref().issuer_realm.as_str(), "owner");
    assert_eq!(record.requesting_realm().as_str(), "caller");
    assert_eq!(record.executor().binding.realm.as_str(), "executor");
    assert_eq!(record.grant_ref().generation.get(), 4);
    let _: Option<LiveExecutionGrant<()>> = None;
    Ok(())
}

#[test]
fn wrong_requested_realm_selector_or_binding_cannot_enter_grant_record() -> TestResult {
    for (pointer, wrong) in [
        ("/requesting_realm", json!("not-listed")),
        (
            "/executor/selector/session_id",
            json!("00000000-0000-0000-0000-000000000099"),
        ),
        (
            "/executor/binding/session_id",
            json!("00000000-0000-0000-0000-000000000099"),
        ),
        ("/declaration/generation", json!(0)),
        ("/executor/binding/binding_generation", json!(0)),
    ] {
        let mut value = grant();
        *value.pointer_mut(pointer).ok_or("fixture field")? = wrong;
        assert!(
            serde_json::from_value::<LiveExecutionGrantRecord<()>>(value).is_err(),
            "{pointer}"
        );
    }
    Ok(())
}

#[test]
fn expiry_is_historical_record_content_not_a_deserialization_time_default() -> TestResult {
    let record: LiveExecutionGrantRecord<()> = serde_json::from_value(grant())?;
    assert!(record.declaration().expires_at.is_some());
    let mut value = grant();
    value["issued_at"] = json!("2026-09-12T00:00:00Z");
    assert!(serde_json::from_value::<LiveExecutionGrantRecord<()>>(value).is_err());
    let mut value = grant();
    value["declaration"]
        .as_object_mut()
        .ok_or("declaration")?
        .remove("expires_at");
    let no_expiry: LiveExecutionGrantRecord<()> = serde_json::from_value(value)?;
    assert!(no_expiry.declaration().expires_at.is_none());
    Ok(())
}

#[test]
fn record_schema_and_extra_authority_fields_fail_closed() -> TestResult {
    for field in ["api_key", "auth_binding", "effect_permit", "revoked"] {
        let mut value = grant();
        value[field] = json!(true);
        assert!(serde_json::from_value::<LiveExecutionGrantRecord<()>>(value).is_err());
    }
    let mut value = grant();
    value["format"] = json!("v2");
    assert!(serde_json::from_value::<LiveExecutionGrantRecord<()>>(value).is_err());
    Ok(())
}

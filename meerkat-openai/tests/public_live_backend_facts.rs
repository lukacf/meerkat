#![cfg(all(not(target_arch = "wasm32"), feature = "live"))]

use meerkat_core::live_execution::backend::{
    LiveBackendCandidates, LiveBackendOwnership, LiveBackendScope,
};
use meerkat_core::live_execution::request::LiveDelegationAttribution;
use meerkat_openai::public_live::backend::{observe_backend_scope, ready_function_batch};
use meerkat_openai::public_live::request::{INVOKE_MEERKAT, InvokeMeerkatRequest};
use oai_rt_rs::live::{Codec, FunctionCallTracker, ResponseKey, ServerFrame};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lifecycle(kind: &str, id: &str, status: &str) -> Value {
    json!({
        "type": kind, "sequence_number": 1,
        "response": { "id": id, "status": status, "created_at": 1.0,
            "output": [], "tools": [], "instructions": null }
    })
}

fn frame(scope: Option<Value>, event: Value) -> Result<ServerFrame, Box<dyn std::error::Error>> {
    let mut value = json!({"type": "response.event", "event_id": "outer", "event": event});
    if let Some(scope) = scope {
        value["delegation_id"] = scope;
    }
    Ok(Codec::default().decode_server(&value.to_string())?)
}

#[test]
fn null_and_absent_outer_scope_preserve_known_unscoped_response_identity() -> TestResult {
    for (outer, expected) in [
        (None, LiveDelegationAttribution::Absent {}),
        (
            Some(Value::Null),
            LiveDelegationAttribution::ExplicitNull {},
        ),
    ] {
        let mut tracker = FunctionCallTracker::default();
        let frame = frame(
            outer,
            lifecycle("response.created", "known-r2", "in_progress"),
        )?;
        let scope = observe_backend_scope(&mut tracker, &frame)?;
        assert_eq!(scope.envelope_attribution, expected);
        let LiveBackendOwnership::Owned { response } = &scope.ownership else {
            return Err("known response was discarded".into());
        };
        assert_eq!(response.response.as_str(), "known-r2");
        assert!(response.delegation.is_none());
        let key = ResponseKey {
            response_id: "known-r2".into(),
            delegation_id: None,
        };
        assert!(ready_function_batch(&tracker, &key)?.is_none());
        assert_eq!(
            serde_json::from_value::<LiveBackendScope>(serde_json::to_value(&scope)?)?,
            scope
        );
        let mut bad = serde_json::to_value(scope)?;
        bad["ownership"]["permission"] = json!("invented");
        assert!(serde_json::from_value::<LiveBackendScope>(bad).is_err());
    }
    Ok(())
}

#[test]
fn unknown_and_ambiguous_items_never_choose_the_last_response() -> TestResult {
    let mut tracker = FunctionCallTracker::default();
    let granular = frame(
        Some(json!("delegation")),
        json!({
            "type": "response.function_call_arguments.delta", "sequence_number": 3,
            "item_id": "unbound-item", "output_index": 0, "delta": "partial"
        }),
    )?;
    assert!(matches!(
        observe_backend_scope(&mut tracker, &granular)?.ownership,
        LiveBackendOwnership::Unowned {}
    ));
    for id in ["first", "second"] {
        let created = frame(
            Some(json!("delegation")),
            lifecycle("response.created", id, "in_progress"),
        )?;
        observe_backend_scope(&mut tracker, &created)?;
    }
    let scope = observe_backend_scope(&mut tracker, &granular)?;
    let LiveBackendOwnership::Ambiguous { candidates } = &scope.ownership else {
        return Err("concurrent candidates must remain ambiguous".into());
    };
    assert_eq!(candidates.iter().len(), 2);
    assert_eq!(
        candidates
            .iter()
            .map(|key| key.response.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(
        serde_json::from_value::<LiveBackendScope>(serde_json::to_value(&scope)?)?,
        scope
    );
    assert!(LiveBackendCandidates::new(Vec::new()).is_err());
    let candidate = candidates.iter().next().ok_or("candidate")?.clone();
    assert!(LiveBackendCandidates::new(vec![candidate.clone(), candidate]).is_err());
    Ok(())
}

#[test]
fn batch_projection_waits_for_tracker_barrier_and_keeps_invalid_arguments_as_content() -> TestResult
{
    let mut tracker = FunctionCallTracker::default();
    let key = ResponseKey {
        response_id: "response".into(),
        delegation_id: Some("delegation".into()),
    };
    let created = frame(
        Some(json!("delegation")),
        lifecycle("response.created", "response", "in_progress"),
    )?;
    observe_backend_scope(&mut tracker, &created)?;
    let item = frame(
        Some(json!("delegation")),
        json!({
            "type": "response.output_item.done", "sequence_number": 2, "output_index": 0,
            "item": { "type": "function_call", "id": "item", "call_id": "call",
                "name": INVOKE_MEERKAT, "arguments": " {malformed-exact-arguments " }
        }),
    )?;
    observe_backend_scope(&mut tracker, &item)?;
    assert!(ready_function_batch(&tracker, &key)?.is_none());
    let completed = frame(
        Some(json!("delegation")),
        lifecycle("response.completed", "response", "completed"),
    )?;
    observe_backend_scope(&mut tracker, &completed)?;
    let batch = ready_function_batch(&tracker, &key)?.ok_or("complete batch")?;
    let call = batch.calls().next().ok_or("completed call")?;
    assert_eq!(call.arguments, " {malformed-exact-arguments ");
    assert_eq!(
        call.arguments.as_ptr(),
        tracker.calls(&key).ok_or("calls")?[0].args.as_ptr()
    );
    assert!(InvokeMeerkatRequest::decode_arguments(call.name, call.arguments).is_err());
    assert!(!format!("{call:?}").contains("malformed-exact-arguments"));
    assert_eq!(batch.response().response.as_str(), "response");
    Ok(())
}

#[test]
fn confirmed_empty_batch_is_distinct_from_unknown_or_failed_batch() -> TestResult {
    for (kind, status, ready) in [
        ("response.completed", "completed", true),
        ("response.failed", "failed", false),
        ("response.incomplete", "incomplete", false),
    ] {
        let mut tracker = FunctionCallTracker::default();
        let key = ResponseKey {
            response_id: "response".into(),
            delegation_id: Some("delegation".into()),
        };
        assert!(ready_function_batch(&tracker, &key)?.is_none());
        for event in [
            lifecycle("response.created", "response", "in_progress"),
            lifecycle(kind, "response", status),
        ] {
            let frame = frame(Some(json!("delegation")), event)?;
            observe_backend_scope(&mut tracker, &frame)?;
        }
        let batch = ready_function_batch(&tracker, &key)?;
        assert_eq!(batch.is_some(), ready);
        if let Some(batch) = batch {
            assert_eq!(batch.calls().len(), 0);
        }
    }
    Ok(())
}

fn completed_item(arguments: &str, index: u64) -> Value {
    json!({
        "type":"response.output_item.done", "sequence_number":2, "output_index":index,
        "item":{"type":"function_call","id":"item","call_id":"call",
            "name":INVOKE_MEERKAT,"arguments":arguments}
    })
}

#[test]
fn duplicate_item_is_idempotent_but_conflicting_content_or_late_terminal_poison_readiness()
-> TestResult {
    let key = ResponseKey {
        response_id: "response".into(),
        delegation_id: Some("delegation".into()),
    };
    let arguments = r#"{"request":" exact content "}"#;
    for conflicting in [
        completed_item(r#"{"request":"changed"}"#, 0),
        completed_item(arguments, 1),
        lifecycle("response.failed", "response", "failed"),
        lifecycle("response.completed", "response", "in_progress"),
    ] {
        let mut tracker = FunctionCallTracker::default();
        for event in [
            lifecycle("response.created", "response", "in_progress"),
            completed_item(arguments, 0),
            lifecycle("response.completed", "response", "completed"),
        ] {
            observe_backend_scope(&mut tracker, &frame(Some(json!("delegation")), event)?)?;
        }
        let digest = ready_function_batch(&tracker, &key)?
            .ok_or("ready")?
            .digest()?;
        for event in [
            completed_item(arguments, 0),
            lifecycle("response.completed", "response", "completed"),
        ] {
            observe_backend_scope(&mut tracker, &frame(Some(json!("delegation")), event)?)?;
            let batch = ready_function_batch(&tracker, &key)?.ok_or("idempotent ready")?;
            assert_eq!(batch.calls().len(), 1);
            assert_eq!(batch.digest()?, digest);
        }
        assert!(
            observe_backend_scope(
                &mut tracker,
                &frame(Some(json!("delegation")), conflicting)?
            )
            .is_err()
        );
        assert!(ready_function_batch(&tracker, &key)?.is_none());
        assert_eq!(
            tracker.calls(&key).ok_or("retained calls")?[0].args,
            arguments
        );
    }
    Ok(())
}

#[test]
fn malformed_owned_evidence_prevents_later_completed_from_claiming_a_complete_batch() -> TestResult
{
    let mut tracker = FunctionCallTracker::default();
    let key = ResponseKey {
        response_id: "response".into(),
        delegation_id: Some("delegation".into()),
    };
    observe_backend_scope(
        &mut tracker,
        &frame(
            Some(json!("delegation")),
            lifecycle("response.created", "response", "in_progress"),
        )?,
    )?;
    let malformed = frame(
        Some(json!("delegation")),
        json!({
            "type":"response.output_item.done","output_index":0,"item":null
        }),
    )?;
    assert!(observe_backend_scope(&mut tracker, &malformed).is_err());
    observe_backend_scope(
        &mut tracker,
        &frame(
            Some(json!("delegation")),
            lifecycle("response.completed", "response", "completed"),
        )?,
    )?;
    assert!(ready_function_batch(&tracker, &key)?.is_none());
    assert!(tracker.terminal(&key).is_some());
    Ok(())
}

#[test]
fn diagnostic_encoding_has_no_raw_payload_and_enforces_escaped_public_byte_ceiling() -> TestResult {
    use meerkat_core::live_execution::backend::{
        LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES, LiveProviderDiagnostic,
    };
    for category in [
        "backend_advisory_error",
        "protocol_inconsistency",
        "accounting_unmeasured",
        "accounting_disputed",
        "unsupported_provider_event",
        "uncorrelated_context_acknowledgment",
    ] {
        let image = json!({
            "category":category,"attribution":{"kind":"unowned"},"occurrences":u64::MAX
        });
        let diagnostic: LiveProviderDiagnostic = serde_json::from_value(image.clone())?;
        assert_eq!(serde_json::to_value(&diagnostic)?, image);
        for field in [
            "raw",
            "instructions",
            "message",
            "arguments",
            "provider_snapshot",
            "terminal",
        ] {
            let mut unsafe_image = image.clone();
            unsafe_image[field] = json!("private-sentinel");
            assert!(serde_json::from_value::<LiveProviderDiagnostic>(unsafe_image).is_err());
        }
    }
    let mut largest = 0;
    let mut refused = 0;
    for bytes in 1..=128 {
        let image = json!({
            "category":"protocol_inconsistency",
            "attribution":{"kind":"owned","response":{
                "response":"\0".repeat(bytes),"delegation":"\0".repeat(bytes)
            }},
            "occurrences":u64::MAX
        });
        let expected_bytes = serde_json::to_vec(&image)?.len();
        match serde_json::from_value::<LiveProviderDiagnostic>(image) {
            Ok(record) => {
                let encoded = serde_json::to_vec(&record)?;
                assert_eq!(encoded.len(), expected_bytes);
                assert!(encoded.len() <= LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES);
                largest = largest.max(encoded.len());
            }
            Err(_) => {
                assert!(expected_bytes > LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES);
                refused += 1;
            }
        }
    }
    assert!(largest > 1000 && refused > 0);
    let mut exact = json!({
        "category":"protocol_inconsistency",
        "attribution":{"kind":"owned","response":{
            "response":"\0".repeat(70),"delegation":"\0".repeat(70)
        }},
        "occurrences":u64::MAX
    });
    let padding = LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES
        .checked_sub(serde_json::to_vec(&exact)?.len())
        .ok_or("fixture already exceeds diagnostic budget")?;
    assert!(70 + padding < 128);
    exact["attribution"]["response"]["response"] =
        json!(format!("{}{}", "\0".repeat(70), "x".repeat(padding)));
    let accepted: LiveProviderDiagnostic = serde_json::from_value(exact.clone())?;
    assert_eq!(
        serde_json::to_vec(&accepted)?.len(),
        LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES
    );
    exact["attribution"]["response"]["response"] =
        json!(format!("{}{}", "\0".repeat(70), "x".repeat(padding + 1)));
    assert_eq!(
        serde_json::to_vec(&exact)?.len(),
        LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES + 1
    );
    assert!(serde_json::from_value::<LiveProviderDiagnostic>(exact).is_err());
    Ok(())
}

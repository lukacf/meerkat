use chrono::Utc;
use meerkat_core::ops::{OpEvent, OperationId, WorkKind};
use meerkat_runtime::{
    ContinuationInput, ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, OperationInput, PeerConvention, PeerInput,
};
use serde_json::json;

fn header(source: InputOrigin) -> InputHeader {
    InputHeader {
        id: meerkat_core::lifecycle::InputId::new(),
        timestamp: Utc::now(),
        source,
        durability: InputDurability::Durable,
        visibility: InputVisibility::default(),
        idempotency_key: None,
        supersession_key: None,
        correlation_id: None,
    }
}

fn assert_unknown_field_rejected(mut value: serde_json::Value, field: &str) {
    value
        .as_object_mut()
        .expect("input serializes as object")
        .insert(field.to_string(), json!({ "model": "stale-model" }));

    let err = serde_json::from_value::<Input>(value)
        .expect_err("runtime input must reject stale split metadata fields");
    let message = err.to_string();
    assert!(
        message.contains(field) || message.contains("unknown field"),
        "unexpected error for {field}: {message}"
    );
}

#[test]
fn runtime_non_prompt_inputs_reject_stale_metadata_fields() {
    let peer = Input::Peer(PeerInput {
        header: header(InputOrigin::Peer {
            peer_id: "peer-1".to_string(),
            display_identity: None,
            runtime_id: None,
        }),
        convention: Some(PeerConvention::Message),
        body: "hello".to_string(),
        payload: None,
        blocks: None,
        handling_mode: None,
    });
    assert_unknown_field_rejected(
        serde_json::to_value(peer).expect("serialize peer"),
        "turn_metadata",
    );

    let external = Input::ExternalEvent(ExternalEventInput {
        header: header(InputOrigin::External {
            source_name: "webhook".to_string(),
        }),
        event_type: "webhook.created".to_string(),
        payload: json!({ "ok": true }),
        blocks: None,
        handling_mode: Default::default(),
        render_metadata: None,
    });
    assert_unknown_field_rejected(
        serde_json::to_value(external).expect("serialize external event"),
        "provider_params",
    );

    let continuation = Input::Continuation(ContinuationInput {
        header: header(InputOrigin::System),
        reason: "wake".to_string(),
        handling_mode: Default::default(),
        request_id: None,
    });
    assert_unknown_field_rejected(
        serde_json::to_value(continuation).expect("serialize continuation"),
        "model",
    );

    let operation_id = OperationId::new();
    let operation = Input::Operation(OperationInput {
        header: header(InputOrigin::System),
        operation_id: operation_id.clone(),
        event: OpEvent::Started {
            id: operation_id,
            kind: WorkKind::ToolCall,
        },
    });
    assert_unknown_field_rejected(
        serde_json::to_value(operation).expect("serialize operation"),
        "flow_tool_overlay",
    );
}

#[test]
fn runtime_peer_convention_rejects_unknown_fields() {
    let peer = Input::Peer(PeerInput {
        header: header(InputOrigin::Peer {
            peer_id: "peer-1".to_string(),
            display_identity: None,
            runtime_id: None,
        }),
        convention: Some(PeerConvention::Request {
            request_id: "req-1".to_string(),
            intent: "lookup".to_string(),
        }),
        body: "hello".to_string(),
        payload: None,
        blocks: None,
        handling_mode: None,
    });
    let mut value = serde_json::to_value(peer).expect("serialize peer");
    value
        .get_mut("convention")
        .and_then(serde_json::Value::as_object_mut)
        .expect("peer convention serializes as object")
        .insert("provider_params".to_string(), json!({ "effort": "stale" }));

    let err = serde_json::from_value::<Input>(value)
        .expect_err("peer convention must reject stale split metadata fields");
    let message = err.to_string();
    assert!(
        message.contains("provider_params") || message.contains("unknown field"),
        "unexpected error: {message}"
    );
}

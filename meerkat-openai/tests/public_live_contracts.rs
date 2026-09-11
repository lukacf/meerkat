#![cfg(not(target_arch = "wasm32"))]

use oai_rt_rs::live::{
    AudioFormat, ClientEvent, Codec, Command, ConnectionRole, CreateResponse, Field,
    FunctionCallTracker, Nullable, ResponseAttribution, ResponseEvent, ResponseKey, ServerEvent,
    SessionConfig, SessionStatus,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn response_event(event: Value) -> Result<ResponseEvent, Box<dyn std::error::Error>> {
    Codec::default()
        .decode_server(
            &json!({
                "type": "response.event",
                "event_id": "outer-event",
                "delegation_id": "delegation-a",
                "event": event,
            })
            .to_string(),
        )?
        .response_event()?
        .ok_or_else(|| "expected nested response event".into())
}

fn lifecycle(kind: &str, response_id: &str, status: &str) -> Value {
    json!({
        "type": kind,
        "sequence_number": 1,
        "response": {
            "id": response_id,
            "created_at": 1.0,
            "status": status,
            "output": [],
            "tools": [],
            "instructions": null,
        },
    })
}

#[test]
fn public_start_uses_session_model_without_private_session_type() -> TestResult {
    let event = ClientEvent::new(Command::Start {
        session: SessionConfig::default(),
    });
    let encoded: Value = serde_json::from_str(&Codec::default().encode(&event)?)?;
    assert_eq!(
        encoded,
        json!({"type": "session.start", "session": {"model": "gpt-live-1"}})
    );
    assert!(
        Codec::default()
            .decode_client(
                r#"{"type":"session.start","session":{"model":"gpt-live-1","type":"live"}}"#
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn context_requires_explicit_nullable_delegation() -> TestResult {
    let codec = Codec::default();
    let encoded: Value =
        serde_json::from_str(&codec.encode(&ClientEvent::new(Command::ThinkingAppend {
            content: "A factual progress update.".into(),
            delegation_id: Nullable(None),
        }))?)?;
    assert_eq!(
        encoded,
        json!({
            "type": "session.thinking.append",
            "content": "A factual progress update.",
            "delegation_id": null,
        })
    );
    assert!(
        codec
            .decode_client(r#"{"type":"session.thinking.append","content":"progress"}"#)
            .is_err()
    );
    Ok(())
}

#[test]
fn sparse_fields_preserve_absent_null_and_value() -> TestResult {
    let absent: SessionConfig = serde_json::from_value(json!({"model": "gpt-live-1"}))?;
    let null: SessionConfig =
        serde_json::from_value(json!({"model": "gpt-live-1", "delegation": null}))?;
    assert!(matches!(absent.delegation, Field::Absent));
    assert!(matches!(null.delegation, Field::Null));
    assert!(serde_json::to_value(absent)?.get("delegation").is_none());
    assert_eq!(serde_json::to_value(null)?["delegation"], Value::Null);
    let value = Field::Value("context".to_string());
    assert_eq!(serde_json::to_value(value)?, json!("context"));
    Ok(())
}

#[test]
fn legacy_voice_control_commands_are_not_public_live_commands() {
    for kind in [
        "input_audio_buffer.commit",
        "input_audio_buffer.clear",
        "conversation.item.create",
        "conversation.item.truncate",
        "response.cancel",
        "session.input_text.append",
    ] {
        assert!(
            Codec::default()
                .decode_client(&json!({"type": kind}).to_string())
                .is_err(),
            "{kind} must not become a public Live command"
        );
    }
}

#[test]
fn continuation_has_no_model_or_delegation_override() -> TestResult {
    let codec = Codec::default();
    let encoded: Value =
        serde_json::from_str(&codec.encode(&ClientEvent::new(Command::ResponseCreate))?)?;
    assert_eq!(encoded, json!({"type": "response.create"}));
    for field in ["model", "delegation_id", "response"] {
        let mut invalid = encoded.clone();
        invalid[field] = json!("not-authorized");
        assert!(codec.decode_client(&invalid.to_string()).is_err());
    }
    Ok(())
}

#[test]
fn webrtc_creation_returns_only_identity_and_answer() -> TestResult {
    let created: CreateResponse = serde_json::from_value(json!({
        "session": {"id": "live-created"},
        "transport": {"type": "webrtc", "sdp": "answer"},
    }))?;
    assert_eq!(created.session.id, "live-created");
    assert_eq!(created.transport.sdp(), "answer");
    assert!(
        Codec::default()
            .decode_server(
                &json!({
                    "type": "session.started",
                    "event_id": "start",
                    "session": {"id": "live-created"},
                })
                .to_string()
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn client_delegation_contains_metadata_not_task_or_final_turn() -> TestResult {
    let frame = Codec::default().decode_server(
        &json!({
            "type": "session.delegation.created",
            "event_id": "delegated",
            "offset_ms": 123.5,
            "delegation": {"id": "client-a", "type": "delegation", "target": "client"},
        })
        .to_string(),
    )?;
    assert!(matches!(frame.event, ServerEvent::DelegationCreated { .. }));
    for field in ["task", "arguments", "handoff_id", "user_bidi_turn_id"] {
        assert!(frame.raw.get(field).is_none());
        assert!(frame.raw["delegation"].get(field).is_none());
    }
    Ok(())
}

#[test]
fn transcript_preserves_whitespace_and_fractional_observation_interval() -> TestResult {
    let frame = Codec::default().decode_server(
        &json!({
            "type": "session.input_transcript.delta",
            "event_id": "transcript",
            "delta": " unfinished\n",
            "start_ms": 10.25,
            "end_ms": 20.5,
        })
        .to_string(),
    )?;
    let ServerEvent::InputTranscriptDelta {
        delta,
        start_ms,
        end_ms,
        ..
    } = frame.event
    else {
        return Err("expected input transcript observation".into());
    };
    assert_eq!(delta, " unfinished\n");
    assert_eq!(start_ms.to_bits(), 10.25_f64.to_bits());
    assert_eq!(end_ms.to_bits(), 20.5_f64.to_bits());
    Ok(())
}

#[test]
fn primary_audio_does_not_invent_timing_or_turn_identity() -> TestResult {
    let frame = Codec::default()
        .decode_server(r#"{"type":"session.output_audio.delta","delta":"AAA="}"#)?;
    let audio = frame
        .audio(ConnectionRole::Primary, AudioFormat::default())?
        .ok_or("expected output audio")?;
    assert_eq!(audio.bytes, [0, 0]);
    assert!(audio.interval.is_none());
    for field in ["event_id", "response_id", "item_id", "start_ms", "end_ms"] {
        assert!(frame.raw.get(field).is_none());
    }
    assert!(
        frame
            .audio(ConnectionRole::Sideband, AudioFormat::default())
            .is_err()
    );
    Ok(())
}

#[test]
fn context_ack_may_be_uncorrelated_and_have_zero_width() -> TestResult {
    let codec = Codec::default();
    let uncorrelated = codec.decode_server(
        r#"{"type":"session.thinking.appended","event_id":"ack","start_ms":5,"end_ms":5}"#,
    )?;
    assert!(uncorrelated.client_event_id.is_none());
    let correlated = codec.decode_server(
        r#"{"type":"session.thinking.appended","event_id":"ack","client_event_id":"sent","start_ms":5,"end_ms":5}"#,
    )?;
    assert_eq!(correlated.client_event_id.as_deref(), Some("sent"));
    Ok(())
}

#[test]
fn completed_empty_snapshot_does_not_erase_finished_function_items() -> TestResult {
    let mut tracker = FunctionCallTracker::default();
    let key = ResponseKey {
        delegation_id: Some("delegation-a".into()),
        response_id: "response-a".into(),
    };
    let created = response_event(lifecycle("response.created", "response-a", "in_progress"))?;
    assert!(matches!(
        tracker.observe(Some("delegation-a"), &created)?,
        ResponseAttribution::Owned(ref observed) if observed == &key
    ));
    let item = response_event(json!({
        "type": "response.output_item.done",
        "sequence_number": 2,
        "output_index": 0,
        "item": {
            "type": "function_call",
            "id": "item-a",
            "call_id": "call-a",
            "name": "invoke_meerkat",
            "arguments": "{\"request\":\"Perform the authorized task.\"}",
        },
    }))?;
    tracker.observe(Some("delegation-a"), &item)?;
    assert_eq!(tracker.calls(&key).map(<[_]>::len), Some(1));
    assert!(tracker.ready_calls(&key).is_none());
    let completed = response_event(lifecycle("response.completed", "response-a", "completed"))?;
    tracker.observe(Some("delegation-a"), &completed)?;
    assert_eq!(tracker.ready_calls(&key).map(<[_]>::len), Some(1));
    tracker.observe(Some("delegation-a"), &item)?;
    assert_eq!(tracker.ready_calls(&key).map(<[_]>::len), Some(1));
    Ok(())
}

#[test]
fn unscoped_lifecycle_retains_identity_without_ready_permission() -> TestResult {
    let mut tracker = FunctionCallTracker::default();
    let created = response_event(lifecycle(
        "response.created",
        "response-unscoped",
        "in_progress",
    ))?;
    let attribution = tracker.observe(None, &created)?;
    let ResponseAttribution::Owned(key) = attribution else {
        return Err("known lifecycle response identity must be retained".into());
    };
    assert_eq!(key.response_id, "response-unscoped");
    assert!(key.delegation_id.is_none());
    let completed = response_event(lifecycle(
        "response.completed",
        "response-unscoped",
        "completed",
    ))?;
    tracker.observe(None, &completed)?;
    assert!(tracker.terminal(&key).is_some());
    assert!(tracker.ready_calls(&key).is_none());
    Ok(())
}

#[test]
fn contradictory_lifecycle_status_cannot_close_a_ready_batch() -> TestResult {
    let mut tracker = FunctionCallTracker::default();
    let key = ResponseKey {
        delegation_id: Some("delegation-a".into()),
        response_id: "response-a".into(),
    };
    tracker.observe(
        Some("delegation-a"),
        &response_event(lifecycle("response.created", "response-a", "in_progress"))?,
    )?;
    let contradiction =
        response_event(lifecycle("response.completed", "response-a", "in_progress"))?;
    assert!(
        tracker
            .observe(Some("delegation-a"), &contradiction)
            .is_err()
    );
    assert!(tracker.ready_calls(&key).is_none());
    Ok(())
}

#[test]
fn valid_closed_event_is_terminal_even_with_active_snapshot() -> TestResult {
    let frame = Codec::default().decode_server(
        &json!({
            "type": "session.closed",
            "event_id": "closed",
            "session": {
                "id": "live-a",
                "model": "gpt-live-1",
                "status": "active",
                "expires_at": 1000,
            },
            "reason": "close_requested",
            "usage": {"seconds": 12.5},
        })
        .to_string(),
    )?;
    let ServerEvent::Closed { session, usage, .. } = frame.event else {
        return Err("expected provider closed event".into());
    };
    assert_eq!(session.status, SessionStatus::Active);
    assert_eq!(usage.seconds.to_bits(), 12.5_f64.to_bits());
    Ok(())
}

#[test]
fn unknown_event_remains_explicit_and_debug_redacts_content() -> TestResult {
    let frame = Codec::default()
        .decode_server(r#"{"type":"future.observation","payload":"private-sentinel"}"#)?;
    assert!(matches!(frame.event, ServerEvent::Unknown));
    assert_eq!(frame.raw["payload"], json!("private-sentinel"));
    assert!(!format!("{frame:?}").contains("private-sentinel"));
    let event = ClientEvent::new(Command::ThinkingAppend {
        content: "private-sentinel".into(),
        delegation_id: Nullable(None),
    });
    assert!(!format!("{event:?}").contains("private-sentinel"));
    Ok(())
}

#[test]
fn command_byte_bound_counts_json_escaping() {
    let codec = Codec {
        max_event_bytes: 256,
    };
    let event = ClientEvent::new(Command::ThinkingAppend {
        content: "\0".repeat(64),
        delegation_id: Nullable(None),
    });
    assert!(codec.encode(&event).is_err());
}

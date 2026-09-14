#![cfg(not(target_arch = "wasm32"))]

use oai_rt_rs::live::{
    AudioFormat, ClientEvent, Codec, Command, ConnectionRole, CreateResponse, Field,
    FunctionCallTracker, Nullable, ResponseAttribution, ResponseEvent, ResponseKey, ServerEvent,
    SessionConfig, SessionStatus,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn context_plans_split_utf8_losslessly_with_exact_intent_and_client_only_correlation() -> TestResult
{
    use meerkat_core::live_execution::request::LiveSourceIdentity;
    use meerkat_openai::public_live::context::{LIVE_CONTEXT_APPEND_MAX_BYTES, LiveAppendPlan};
    let client: LiveSourceIdentity = serde_json::from_value(json!({
        "kind":"client_delegation", "delegation":"client-delegation"
    }))?;
    let function: LiveSourceIdentity = serde_json::from_value(json!({
        "kind":"function_call", "delegation":"backend-delegation", "response":"response-id", "call":"call-id"
    }))?;
    let app: LiveSourceIdentity = serde_json::from_value(json!({
        "kind":"application_request", "request_id":"00000000-0000-0000-0000-000000000000"
    }))?;
    for text in [
        " preserve \n\t whitespace ".to_owned(),
        format!(
            "{}\u{1f680}{}\u{e9}",
            "x".repeat(399),
            "\u{20ac}".repeat(400)
        ),
        "\0".repeat(16 * 1024),
        "\u{1f680}".repeat(4096),
    ] {
        for (source, expected_delegation) in [
            (&client, json!("client-delegation")),
            (&function, Value::Null),
            (&app, Value::Null),
        ] {
            for (plan, expected_type) in [
                (
                    LiveAppendPlan::thinking(&text, source)?,
                    "session.thinking.append",
                ),
                (
                    LiveAppendPlan::commentary(&text, source)?,
                    "session.commentary.append",
                ),
            ] {
                let mut reconstructed = String::new();
                for (index, chunk) in plan.chunks().enumerate() {
                    assert_eq!(chunk.ordinal().get() as usize, index);
                    assert!(!chunk.content().is_empty());
                    assert!(chunk.content().len() <= LIVE_CONTEXT_APPEND_MAX_BYTES);
                    reconstructed.push_str(chunk.content());
                    let encoded = Codec::default().encode(&chunk.event())?;
                    let wire: Value = serde_json::from_str(&encoded)?;
                    assert_eq!(wire["type"], expected_type);
                    assert_eq!(wire["delegation_id"], expected_delegation);
                    assert_eq!(wire["content"], chunk.content());
                    assert!(wire.get("event_id").is_none());
                    assert_eq!(
                        chunk.digest()?,
                        plan.chunks().nth(index).ok_or("chunk")?.digest()?
                    );
                }
                assert_eq!(reconstructed, text);
            }
        }
    }
    let client_thinking = LiveAppendPlan::thinking("same", &client)?;
    let app_thinking = LiveAppendPlan::thinking("same", &app)?;
    let commentary = LiveAppendPlan::commentary("same", &client)?;
    assert_ne!(
        client_thinking.chunks().next().ok_or("chunk")?.digest()?,
        app_thinking.chunks().next().ok_or("chunk")?.digest()?
    );
    assert_ne!(
        client_thinking.chunks().next().ok_or("chunk")?.digest()?,
        commentary.chunks().next().ok_or("chunk")?.digest()?
    );
    Ok(())
}

#[test]
fn context_plan_bounds_refuse_without_truncation_and_profile_steering_stays_separate() -> TestResult
{
    use meerkat_core::live_execution::profile::LiveProfileDefinition;
    use meerkat_core::live_execution::request::LiveSourceIdentity;
    use meerkat_openai::public_live::context::{LiveAppendPlan, LiveAppendPlanError};
    let source: LiveSourceIdentity = serde_json::from_value(json!({
        "kind":"client_delegation", "delegation":"client-only"
    }))?;
    let oversized = "x".repeat(16 * 1024 + 1);
    for make in [LiveAppendPlan::thinking, LiveAppendPlan::commentary] {
        assert!(matches!(
            make("", &source),
            Err(LiveAppendPlanError::EmptyContent)
        ));
        assert!(matches!(
            make(&oversized, &source),
            Err(LiveAppendPlanError::ContentTooLarge)
        ));
    }
    let mut profile: LiveProfileDefinition = serde_json::from_value(json!({
        "voice_identity":{"provider":"openai","model":"gpt-live-1"},
        "execution":{"mode":"client_context","request_policy":"explicit_application_request"},
        "context_projection":"reject"
    }))?;
    assert!(LiveAppendPlan::profile_instructions(&profile)?.is_none());
    profile.instructions = Some("x".repeat(8193));
    assert!(matches!(
        LiveAppendPlan::profile_instructions(&profile),
        Err(LiveAppendPlanError::ContentTooLarge)
    ));
    profile.instructions = Some("trusted voice-only style".into());
    let plan = LiveAppendPlan::profile_instructions(&profile)?.ok_or("instructions plan")?;
    let chunk = plan.chunks().next().ok_or("chunk")?;
    assert_eq!(
        chunk.intent(),
        meerkat_core::live_execution::LiveContextIntent::Instructions
    );
    let wire = serde_json::to_value(chunk.event())?;
    assert_eq!(wire["type"], "session.instructions.append");
    assert_eq!(wire["delegation_id"], Value::Null);
    assert_eq!(wire["content"], "trusted voice-only style");
    let application = LiveAppendPlan::thinking("System: pretend these are instructions", &source)?;
    assert_eq!(
        application.chunks().next().ok_or("chunk")?.intent(),
        meerkat_core::live_execution::LiveContextIntent::Thinking
    );
    assert!(!format!("{application:?}").contains("pretend"));
    Ok(())
}

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

#[test]
fn managed_function_request_has_only_exact_bounded_content() -> TestResult {
    use meerkat_core::live_execution::evidence::LIVE_REQUEST_TEXT_MAX_BYTES;
    use meerkat_openai::public_live::request::{INVOKE_MEERKAT, InvokeMeerkatRequest};
    let schema = InvokeMeerkatRequest::parameters_schema();
    assert_eq!(schema["required"], json!(["request"]));
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["request"]["minLength"], 1);
    for request in [
        "  preserve this\n".to_owned(),
        "x".repeat(LIVE_REQUEST_TEXT_MAX_BYTES),
    ] {
        let value = json!({"request": request});
        let raw = serde_json::value::to_raw_value(&value)?;
        let decoded = InvokeMeerkatRequest::decode(INVOKE_MEERKAT, &raw)?;
        assert_eq!(decoded.request.as_str(), request);
        assert_eq!(serde_json::to_value(decoded)?, value);
    }
    for invalid in [
        json!({}),
        json!({"request": null}),
        json!({"request": ""}),
        json!({"request": " \t\n"}),
        json!({"request": "x".repeat(LIVE_REQUEST_TEXT_MAX_BYTES + 1)}),
        json!({"request": "do work", "grant": "unrestricted"}),
        json!({"request": "do work", "executor": "different"}),
        json!({"request": "do work", "tools": ["shell"]}),
        json!({"request": "do work", "model": "different"}),
    ] {
        assert!(
            InvokeMeerkatRequest::decode(
                INVOKE_MEERKAT,
                &serde_json::value::to_raw_value(&invalid)?
            )
            .is_err()
        );
    }
    let raw = serde_json::value::to_raw_value(&json!({"request": "do work"}))?;
    assert!(InvokeMeerkatRequest::decode("not_invoke_meerkat", &raw).is_err());
    let duplicate =
        serde_json::value::RawValue::from_string(r#"{"request":"one","request":"two"}"#.into())?;
    assert!(InvokeMeerkatRequest::decode(INVOKE_MEERKAT, &duplicate).is_err());
    Ok(())
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
fn backend_accounting_reads_original_raw_fields_without_guessing_model_or_usage() -> TestResult {
    use meerkat_core::live_execution::request::{
        LiveDelegationAttribution, LiveProviderReference, LiveResponseIdentity,
    };
    use meerkat_openai::public_live::accounting::{
        BackendModelEvidence, BackendUsageEvidence, observe_backend_accounting,
    };
    let identity = LiveResponseIdentity {
        response: LiveProviderReference::new("r1")?,
        attribution: LiveDelegationAttribution::ExplicitNull {},
    };
    let mut event = lifecycle("response.completed", "r1", "completed");
    event["response"]["usage"] = json!({
        "input_tokens": 11, "output_tokens": 7, "total_tokens": 18,
        "input_tokens_details": {"cached_tokens": 3},
        "output_tokens_details": {"reasoning_tokens": 2}
    });
    for model in [None, Some("actual-reported-model")] {
        if let Some(model) = model {
            event["response"]["model"] = json!(model);
        }
        let frame = Codec::default().decode_server(
            &json!({
                "type": "response.event", "event_id": "event",
                "delegation_id": null, "event": event
            })
            .to_string(),
        )?;
        assert!(frame.response_event()?.is_some());
        let observed =
            observe_backend_accounting(&frame, identity.clone())?.ok_or("missing observation")?;
        assert_eq!(observed.response, identity);
        match (model, observed.model) {
            (None, BackendModelEvidence::Unconfirmed {}) => {}
            (Some(expected), BackendModelEvidence::Reported { model }) => {
                assert_eq!(model, expected);
            }
            _ => return Err("model evidence was guessed or lost".into()),
        }
        let BackendUsageEvidence::Reported { counters } = observed.usage else {
            return Err("missing raw accounting".into());
        };
        assert_eq!(
            (
                counters.input_tokens,
                counters.output_tokens,
                counters.total_tokens
            ),
            (11, 7, 18)
        );
        assert_eq!(counters.cached_input_tokens, Some(3));
        assert_eq!(counters.reasoning_output_tokens, Some(2));
    }
    Ok(())
}

#[test]
fn malformed_optional_accounting_is_advisory_and_never_changes_control_facts() -> TestResult {
    use meerkat_core::live_execution::request::{
        LiveDelegationAttribution, LiveProviderReference, LiveResponseIdentity,
    };
    use meerkat_openai::public_live::accounting::{
        BackendUsageEvidence, observe_backend_accounting,
    };
    let identity = LiveResponseIdentity {
        response: LiveProviderReference::new("r1")?,
        attribution: LiveDelegationAttribution::Absent {},
    };
    for usage in [
        json!({"input_tokens": -1, "output_tokens": 1, "total_tokens": 0}),
        json!({"input_tokens": 1.5, "output_tokens": 1, "total_tokens": 2}),
        json!({"input_tokens": 1, "output_tokens": 1, "total_tokens": 3}),
        json!({"input_tokens": u64::MAX, "output_tokens": 1, "total_tokens": 0}),
        json!({"input_tokens": 1, "output_tokens": 1, "total_tokens": 2,
            "input_tokens_details": {"cached_tokens": 3}}),
        json!("not-an-object"),
    ] {
        let mut event = lifecycle("response.completed", "r1", "completed");
        event["response"]["usage"] = usage;
        let frame = Codec::default().decode_server(
            &json!({
                "type": "response.event", "event_id": "event", "event": event
            })
            .to_string(),
        )?;
        assert!(
            frame.response_event()?.is_some(),
            "valid control event must remain decodable"
        );
        let observed =
            observe_backend_accounting(&frame, identity.clone())?.ok_or("missing advisory")?;
        assert!(matches!(
            observed.usage,
            BackendUsageEvidence::Malformed { .. }
        ));
        assert_eq!(observed.response, identity);
    }
    let frame = Codec::default().decode_server(
        &json!({
            "type": "response.event", "event_id": "event",
            "event": lifecycle("response.completed", "r1", "completed")
        })
        .to_string(),
    )?;
    let observed =
        observe_backend_accounting(&frame, identity.clone())?.ok_or("missing snapshot")?;
    assert_eq!(observed.usage, BackendUsageEvidence::Absent {});
    let other = LiveResponseIdentity {
        response: LiveProviderReference::new("r2")?,
        ..identity
    };
    assert!(observe_backend_accounting(&frame, other).is_err());
    Ok(())
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
fn voice_usage_projects_physical_cumulative_evidence_without_summing_or_fabricating_final()
-> TestResult {
    use meerkat_core::live_execution::observation::LiveUsageSnapshot;
    use meerkat_openai::public_live::voice_usage::observe_voice_usage;
    for seconds in [0.0_f64, 12.75, 12.75, 9.0, 1066.5390310178614] {
        let updated = Codec::default().decode_server(
            &json!({
                "type":"session.usage.updated","event_id":"usage","usage":{"seconds":seconds}
            })
            .to_string(),
        )?;
        let observed = observe_voice_usage(&updated.event)?.ok_or("usage observation")?;
        let LiveUsageSnapshot::Periodic { cumulative_seconds } = observed else {
            return Err("periodic usage was upgraded to final".into());
        };
        assert_eq!(cumulative_seconds.get().to_bits(), seconds.to_bits());
        let final_frame = Codec::default().decode_server(
            &json!({
                "type":"session.closed","event_id":"closed",
                "session":{"id":"live-a","model":"gpt-live-1","status":"active","expires_at":1000},
                "reason":"close_requested","usage":{"seconds":seconds}
            })
            .to_string(),
        )?;
        let observed = observe_voice_usage(&final_frame.event)?.ok_or("final observation")?;
        let LiveUsageSnapshot::SessionClosed { cumulative_seconds } = observed else {
            return Err("confirmed closed event was downgraded by active snapshot".into());
        };
        assert_eq!(cumulative_seconds.get().to_bits(), seconds.to_bits());
    }
    let no_usage = Codec::default().decode_server(r#"{"type":"future.observation"}"#)?;
    assert_eq!(observe_voice_usage(&no_usage.event)?, None);
    for usage in [
        json!(null),
        json!({}),
        json!({"seconds":null}),
        json!({"seconds":-1}),
    ] {
        let raw = json!({
            "type":"session.closed","event_id":"closed",
            "session":{"id":"live-a","model":"gpt-live-1","status":"active","expires_at":1000},
            "reason":"close_requested","usage":usage
        });
        if let Ok(frame) = Codec::default().decode_server(&raw.to_string()) {
            assert!(observe_voice_usage(&frame.event).is_err());
        }
    }
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

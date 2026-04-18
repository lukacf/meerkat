use oai_rt_rs::protocol::client_events::ClientEvent;
use oai_rt_rs::protocol::models::{
    AudioConfig, AudioFormat, ConversationMode, Infinite, InputAudioConfig,
    InputAudioTranscription, InputItem, ItemStatus, MaxTokens, Nullable, OutputModalities,
    ResponseStatus, Role, Session, SessionConfig, SessionKind, SessionUpdate, SessionUpdateConfig,
    TurnDetection,
};
use oai_rt_rs::protocol::server_events::ServerEvent;
use serde_json::json;

#[test]
fn test_session_update_deserialization() {
    let json = json!({
        "type": "session.update",
        "session": {
            "instructions": "You are a helpful assistant.",
            "turn_detection": {
                "type": "server_vad",
                "threshold": 0.5,
                "create_response": true
            },
            "output_modalities": ["audio"],
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": 24000 }
                }
            }
        }
    });

    let event: ClientEvent =
        serde_json::from_value(json).expect("Failed to deserialize session.update");
    match event {
        ClientEvent::SessionUpdate { session, .. } => {
            let session = session.as_ref();
            assert_eq!(
                session.config.output_modalities,
                Some(OutputModalities::Audio)
            );

            // Check nested audio config
            if let Some(audio) = &session.config.audio {
                if let Some(input) = &audio.input {
                    if let Some(AudioFormat::Pcm { .. }) = &input.format {
                        // Correct variant
                    } else {
                        panic!("Missing or wrong format: {:?}", input.format);
                    }
                } else {
                    panic!("Missing input");
                }
            } else {
                panic!("Missing audio config");
            }
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_response_create_with_input_and_metadata() {
    let json = json!({
        "type": "response.create",
        "response": {
            "conversation": "none",
            "metadata": { "topic": "pizza" },
            "input": [
                {
                    "type": "item_reference",
                    "id": "item_ref_1"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": "Pineapple?" }
                    ]
                }
            ]
        }
    });

    let event: ClientEvent =
        serde_json::from_value(json).expect("Failed to deserialize response.create");
    match event {
        ClientEvent::ResponseCreate { response, .. } => {
            let config = response.expect("Missing response config");
            let config = config.as_ref();
            assert_eq!(config.conversation, Some(ConversationMode::None));

            let metadata = config.metadata.as_ref().expect("Missing metadata");
            assert_eq!(
                metadata.get("topic").and_then(|s| s.as_str()),
                Some("pizza")
            );

            let input = config.input.as_ref().expect("Missing input");
            assert_eq!(input.len(), 2);

            match &input[0] {
                InputItem::ItemReference { id } => assert_eq!(id, "item_ref_1"),
                InputItem::Message { .. } => panic!("Wrong item type at index 0"),
            }

            match &input[1] {
                InputItem::Message { role, .. } => assert!(matches!(role, Role::User)),
                InputItem::ItemReference { .. } => panic!("Wrong item type at index 1"),
            }
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_response_create_omits_none_optionals_instead_of_serializing_nulls() {
    let event = ClientEvent::ResponseCreate {
        event_id: None,
        response: Some(Box::new(oai_rt_rs::protocol::models::ResponseConfig {
            conversation: Some(ConversationMode::Auto),
            output_modalities: Some(OutputModalities::Audio),
            instructions: Some("Respond with audio.".to_string()),
            ..oai_rt_rs::protocol::models::ResponseConfig::default()
        })),
    };

    let serialized = serde_json::to_value(&event).expect("serialize response.create");
    let response = serialized
        .get("response")
        .and_then(|value| value.as_object())
        .expect("response object");

    assert_eq!(
        response
            .get("conversation")
            .and_then(|value| value.as_str()),
        Some("auto")
    );
    assert_eq!(response.get("output_modalities"), Some(&json!(["audio"])));
    assert_eq!(
        response
            .get("instructions")
            .and_then(|value| value.as_str()),
        Some("Respond with audio.")
    );
    assert!(
        !response.contains_key("tool_choice"),
        "response.create should omit tool_choice when it is not set"
    );
    assert!(
        !response.contains_key("tools"),
        "response.create should omit tools when they are not set"
    );
    assert!(
        !response.contains_key("audio"),
        "response.create should omit audio when it is not set"
    );
    assert!(
        !response.contains_key("temperature"),
        "response.create should omit temperature when it is not set"
    );
}

#[test]
fn test_session_update_omits_none_optionals_instead_of_serializing_nulls() {
    let event = ClientEvent::SessionUpdate {
        event_id: None,
        session: Box::new(SessionUpdate {
            config: SessionUpdateConfig {
                instructions: Some("hello".to_string()),
                output_modalities: Some(OutputModalities::Audio),
                ..SessionUpdateConfig::default()
            },
        }),
    };

    let serialized = serde_json::to_value(&event).expect("serialize session.update");
    let session = serialized
        .get("session")
        .and_then(|value| value.as_object())
        .expect("session object");
    assert_eq!(
        session.get("instructions").and_then(|value| value.as_str()),
        Some("hello")
    );
    assert_eq!(session.get("output_modalities"), Some(&json!(["audio"])));
    assert!(
        !session.contains_key("audio"),
        "session.update should omit audio when it is not set"
    );
    assert!(
        !session.contains_key("tools"),
        "session.update should omit tools when they are not set"
    );
}

#[test]
fn test_session_update_omits_nested_audio_turn_detection_null_fields() {
    let event = ClientEvent::SessionUpdate {
        event_id: None,
        session: Box::new(SessionUpdate {
            config: SessionUpdateConfig {
                output_modalities: Some(OutputModalities::Audio),
                audio: Some(AudioConfig {
                    input: Some(InputAudioConfig {
                        format: Some(AudioFormat::pcm_24khz()),
                        turn_detection: Some(Nullable::Value(TurnDetection::ServerVad {
                            threshold: None,
                            prefix_padding_ms: None,
                            silence_duration_ms: None,
                            idle_timeout_ms: None,
                            create_response: Some(true),
                            interrupt_response: Some(true),
                        })),
                        transcription: Some(Nullable::Value(InputAudioTranscription {
                            model: Some("gpt-4o-mini-transcribe".to_string()),
                            language: None,
                            prompt: None,
                        })),
                        noise_reduction: None,
                    }),
                    output: None,
                }),
                ..SessionUpdateConfig::default()
            },
        }),
    };

    let serialized = serde_json::to_value(&event).expect("serialize nested audio session.update");
    let turn_detection = serialized
        .get("session")
        .and_then(|value| value.get("audio"))
        .and_then(|value| value.get("input"))
        .and_then(|value| value.get("turn_detection"))
        .and_then(|value| value.as_object())
        .expect("turn_detection object");

    assert_eq!(
        turn_detection.get("type").and_then(|value| value.as_str()),
        Some("server_vad")
    );
    assert_eq!(
        turn_detection
            .get("create_response")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        turn_detection
            .get("interrupt_response")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(
        !turn_detection.contains_key("threshold"),
        "turn_detection.threshold should be omitted when unset"
    );
    assert!(
        !turn_detection.contains_key("prefix_padding_ms"),
        "turn_detection.prefix_padding_ms should be omitted when unset"
    );
    assert!(
        !turn_detection.contains_key("silence_duration_ms"),
        "turn_detection.silence_duration_ms should be omitted when unset"
    );
    assert!(
        !turn_detection.contains_key("idle_timeout_ms"),
        "turn_detection.idle_timeout_ms should be omitted when unset"
    );
}

#[test]
fn test_infinite_tokens_roundtrip() {
    let inf = MaxTokens::Infinite(Infinite::Inf);
    let serialized = serde_json::to_string(&inf).expect("Serialize inf");
    assert_eq!(serialized, "\"inf\"");

    let deserialized: MaxTokens = serde_json::from_str(&serialized).expect("Deserialize inf");
    assert!(matches!(deserialized, MaxTokens::Infinite(Infinite::Inf)));
}

#[test]
fn test_server_event_flat_deserialization() {
    let json = json!({
        "type": "response.output_text.delta",
        "event_id": "evt_1",
        "response_id": "resp_1",
        "item_id": "item_1",
        "output_index": 0,
        "content_index": 0,
        "delta": "hello"
    });

    let event: ServerEvent = serde_json::from_value(json).expect("Deserialize flat event");
    assert_eq!(event.event_id(), Some("evt_1"));
    match event {
        ServerEvent::ResponseOutputTextDelta { delta, .. } => assert_eq!(delta, "hello"),
        _ => panic!("Wrong variant"),
    }
}

#[test]
fn test_serialization_roundtrip() {
    let original = json!({
        "type": "conversation.item.create",
        "item": {
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": "Hello world"
                }
            ]
        }
    });

    let event: ClientEvent =
        serde_json::from_value(original.clone()).expect("Deserialize in roundtrip");
    let serialized = serde_json::to_value(&event).expect("Serialize in roundtrip");

    assert_eq!(serialized.get("type"), original.get("type"));
}

#[test]
fn test_item_status_copy() {
    let s = ItemStatus::Completed;
    let _ = s; // Should be Copy
    assert_eq!(s, ItemStatus::Completed);
}

#[test]
fn test_session_struct_update() {
    let mut config = SessionConfig::new(
        SessionKind::Realtime,
        "gpt-realtime",
        OutputModalities::Audio,
    );
    config.instructions = Some("Test instructions".to_string());

    let session = Session {
        id: "sess_123".to_string(),
        object: "realtime.session".to_string(),
        expires_at: 123,
        config,
    };

    assert_eq!(session.config.model.as_str(), "gpt-realtime");
    assert_eq!(
        session.config.instructions.as_deref(),
        Some("Test instructions")
    );
    assert_eq!(session.config.output_modalities, OutputModalities::Audio);
}

#[test]
fn test_response_status_enum() {
    let json = json!("cancelled");
    let status: ResponseStatus = serde_json::from_value(json).unwrap();
    assert_eq!(status, ResponseStatus::Cancelled);
}

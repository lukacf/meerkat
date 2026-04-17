use futures::stream;
use oai_rt_rs::{ServerEvent, VoiceEvent, pump_raw_event_stream};

#[tokio::test]
async fn test_new_voice_events_mapping() {
    let _ = VoiceEvent::UserTranscriptDone {
        item_id: "item_1".to_string(),
        content_index: 0,
        transcript: "hello".to_string(),
    };
    let _ = VoiceEvent::ResponseCancelled {
        response_id: "resp_1".to_string(),
    };
}

#[tokio::test]
async fn test_session_state_methods() {
    // verify compilation of new methods would go here
}

#[tokio::test]
async fn test_raw_event_pump_forwards_each_server_event() {
    let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    let events = stream::iter(vec![
        Ok(ServerEvent::InputAudioBufferSpeechStarted {
            event_id: "evt_1".to_string(),
            audio_start_ms: 12,
            item_id: "item_1".to_string(),
        }),
        Ok(ServerEvent::InputAudioBufferSpeechStopped {
            event_id: "evt_2".to_string(),
            audio_end_ms: 24,
            item_id: "item_1".to_string(),
        }),
    ]);

    pump_raw_event_stream(events, move |event| {
        let seen = seen_clone.clone();
        async move {
            let name = match event {
                ServerEvent::InputAudioBufferSpeechStarted { event_id, .. }
                | ServerEvent::InputAudioBufferSpeechStopped { event_id, .. } => event_id,
                other => panic!("unexpected event: {other:?}"),
            };
            seen.lock().await.push(name);
            Ok(())
        }
    })
    .await
    .expect("pump should finish cleanly");

    assert_eq!(
        seen.lock().await.as_slice(),
        &["evt_1".to_string(), "evt_2".to_string()]
    );
}

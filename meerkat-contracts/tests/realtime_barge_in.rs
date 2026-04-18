#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Wire shape for the barge-in truncation frames (Item 5a).

use meerkat_contracts::{
    RealtimeBargeInTruncateFrame, RealtimeChannelEventFrame, RealtimeClientFrame, RealtimeEvent,
    RealtimeServerFrame,
};

#[test]
fn barge_in_truncate_client_frame_roundtrips_over_the_wire() {
    let frame = RealtimeClientFrame::ChannelBargeInTruncate(RealtimeBargeInTruncateFrame {
        item_id: "item_99".to_string(),
        content_index: 0,
        audio_played_ms: 512,
    });
    let value = serde_json::to_value(&frame).expect("frame serializes");
    assert_eq!(value["type"], "channel.barge_in_truncate");
    assert_eq!(value["item_id"], "item_99");
    assert_eq!(value["content_index"], 0);
    assert_eq!(value["audio_played_ms"], 512);

    let roundtrip: RealtimeClientFrame = serde_json::from_value(value).expect("frame deserializes");
    assert_eq!(roundtrip, frame);
}

#[test]
fn assistant_transcript_truncated_event_roundtrips_with_and_without_text() {
    let with_text = RealtimeEvent::AssistantTranscriptTruncated {
        item_id: "item_99".to_string(),
        audio_played_ms: 512,
        truncated_text: Some("hello wor".to_string()),
    };
    let value = serde_json::to_value(&with_text).expect("event serializes");
    assert_eq!(value["type"], "assistant_transcript_truncated");
    assert_eq!(value["audio_played_ms"], 512);
    assert_eq!(value["truncated_text"], "hello wor");

    let without_text = RealtimeEvent::AssistantTranscriptTruncated {
        item_id: "item_99".to_string(),
        audio_played_ms: 512,
        truncated_text: None,
    };
    let value_bare = serde_json::to_value(&without_text).expect("bare event serializes");
    assert!(
        value_bare.get("truncated_text").is_none() || value_bare["truncated_text"].is_null(),
        "truncated_text must be omitted when None so legacy consumers do not \
         treat `null` as an authoritative empty string",
    );

    let roundtrip: RealtimeServerFrame = serde_json::from_value(
        serde_json::to_value(RealtimeServerFrame::ChannelEvent(
            RealtimeChannelEventFrame {
                event: with_text.clone(),
            },
        ))
        .unwrap(),
    )
    .expect("server frame deserializes");
    match roundtrip {
        RealtimeServerFrame::ChannelEvent(f) => assert_eq!(f.event, with_text),
        other => panic!("expected ChannelEvent, got {other:?}"),
    }
}

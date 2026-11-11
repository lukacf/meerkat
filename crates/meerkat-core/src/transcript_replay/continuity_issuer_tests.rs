//! The continuity issuer recorded on OpenAI-family provider metadata: how it
//! is stored, what an earlier version reads, and what the replay owner admits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Message;
use serde_json::{Value, json};

/// One tool turn and its answer exactly as 0.9.12 stored them after an
/// OpenAI Responses backend returned them (`store: false`,
/// `include: reasoning.encrypted_content`): the bytes are what the 0.9.12
/// serializer writes for these messages. No issuer is recorded.
const STORED_BY_0_9_12: &str = r#"[
  {
    "role": "user",
    "content": "What is the weather in Oslo?",
    "created_at": "2026-11-10T09:00:00Z"
  },
  {
    "role": "block_assistant",
    "blocks": [
      {
        "block_type": "reasoning",
        "data": {
          "text": "",
          "meta": {
            "provider": "open_ai",
            "id": "rs_0a1b",
            "encrypted_content": "gAAAAAB-ciphertext-1",
            "response_id": "resp_0001"
          }
        }
      },
      {
        "block_type": "tool_use",
        "data": {
          "id": "call_weather_1",
          "name": "get_weather",
          "args": {"city":"Oslo"},
          "meta": {
            "provider": "open_ai_response",
            "response_id": "resp_0001"
          }
        }
      }
    ],
    "stop_reason": "tool_use",
    "created_at": "2026-11-10T09:00:00Z"
  },
  {
    "role": "tool_results",
    "results": [
      {
        "tool_use_id": "call_weather_1",
        "content": "4 C, light rain",
        "is_error": false
      }
    ],
    "created_at": "2026-11-10T09:00:00Z"
  },
  {
    "role": "block_assistant",
    "blocks": [
      {
        "block_type": "reasoning",
        "data": {
          "text": "Summarize the reading.",
          "meta": {
            "provider": "open_ai",
            "id": "rs_0c2d",
            "encrypted_content": "gAAAAAB-ciphertext-2",
            "response_id": "resp_0002"
          }
        }
      },
      {
        "block_type": "text",
        "data": {
          "text": "It is 4 C with light rain in Oslo.",
          "meta": {
            "provider": "open_ai_assistant_message",
            "id": "msg_0e3f",
            "phase": "final_answer",
            "response_id": "resp_0002"
          }
        }
      }
    ],
    "stop_reason": "end_turn",
    "created_at": "2026-11-10T09:00:00Z"
  }
]"#;

/// The transcript revision 0.9.12 computes for [`STORED_BY_0_9_12`].
const REVISION_COMPUTED_BY_0_9_12: &str =
    "sha256:9f59d47acab90d62ae31188c037883327763eca5203d5df37a1b28ccdca44e21";

/// [`STORED_BY_0_9_12`] with `issuer` recorded on every OpenAI-family
/// provider metadata object, as a version that records issuers stores it.
fn stored_with_issuer(issuer: Value) -> Value {
    let mut stored: Value = serde_json::from_str(STORED_BY_0_9_12).unwrap();
    let mut stamped = 0;
    for message in stored.as_array_mut().unwrap() {
        let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in blocks {
            if let Some(meta) = block["data"].get_mut("meta") {
                meta["issuer"] = issuer.clone();
                stamped += 1;
            }
        }
    }
    assert_eq!(stamped, 4, "every provider metadata object is stamped");
    stored
}

fn messages(stored: Value) -> Vec<Message> {
    serde_json::from_value(stored).expect("stored transcript decodes")
}

/// The fixture is exactly what the serializer writes for an issuer-less
/// transcript: decoding it and writing it back changes nothing.
#[test]
fn an_issuer_less_transcript_is_written_back_unchanged() {
    let stored: Value = serde_json::from_str(STORED_BY_0_9_12).unwrap();
    let written = serde_json::to_value(messages(stored.clone())).unwrap();
    assert_eq!(written, stored);
}

/// Rolling back: a reader decodes a transcript whose provider metadata names
/// an issuer, whether or not it knows the key. (On the commit that adds this
/// row the metadata type is 0.9.12's, which does not know it.)
#[test]
fn a_transcript_that_names_issuers_decodes() {
    for issuer in [
        json!({"kind": "openai_api"}),
        json!({"kind": "copilot"}),
        json!({"kind": "self_hosted", "server": "local"}),
    ] {
        let decoded = messages(stored_with_issuer(issuer.clone()));
        assert_eq!(decoded.len(), 4, "{issuer}");
    }
}

/// Rolling back: the transcript revision is a content address the stores
/// compare on read and append. A reader that drops the `issuer` key must
/// compute the revision the writer recorded, so the issuer is not part of
/// it: a transcript digests to the revision 0.9.12 computes for the same
/// rows without the key.
#[test]
fn the_transcript_revision_does_not_cover_the_issuer() {
    let without = messages(serde_json::from_str(STORED_BY_0_9_12).unwrap());
    assert_eq!(
        crate::transcript_messages_digest(&without).unwrap(),
        REVISION_COMPUTED_BY_0_9_12,
        "the issuer-less rows keep the revision 0.9.12 computed"
    );
    for issuer in [
        json!({"kind": "openai_api"}),
        json!({"kind": "copilot"}),
        json!({"kind": "self_hosted", "server": "local"}),
    ] {
        let with = messages(stored_with_issuer(issuer.clone()));
        assert_eq!(
            crate::transcript_messages_digest(&with).unwrap(),
            REVISION_COMPUTED_BY_0_9_12,
            "rows that name the issuer {issuer} keep the revision a reader \
             without the key computes"
        );
    }
}

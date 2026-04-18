# Rust OpenAI Realtime SDK

A Rust client for the [OpenAI Realtime API](https://platform.openai.com/docs/guides/realtime).

[![Crates.io](https://img.shields.io/crates/v/oai-rt-rs.svg)](https://crates.io/crates/oai-rt-rs)
[![Documentation](https://docs.rs/oai-rt-rs/badge.svg)](https://docs.rs/oai-rt-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Features

- GA-aligned Realtime API protocol models (WebSocket + REST).
- GA-only behavior: beta headers/events are not supported (e.g., `response.output_item.created`).
- Voice-first SDK with full-duplex audio streaming, VAD, and barge-in helpers.
- Strongly typed `ClientEvent` and `ServerEvent` enums.
- WebRTC SDP signaling, SIP control endpoints, and call hangup (low-level REST).
- Async interface using `tokio` and `tokio-tungstenite`.
- Client-side validation for GA constraints (PCM 24kHz, output modalities, 15MB audio chunks).

## Quickstart (Voice-first SDK)

```rust
#[tokio::main]
async fn main() -> oai_rt_rs::Result<()> {
    let mut session = Realtime::builder()
        .api_key("your-api-key")
        .model("gpt-realtime")
        .voice_session()
        .voice("alloy")
        .vad_server_default()
        .transcription("gpt-4o-transcribe")
        .auto_barge_in(true)
        .connect_ws()
        .await?;

    // Stream voice events (audio deltas + transcripts).
    while let Some(evt) = session.next_voice_event().await? {
        match evt {
            oai_rt_rs::VoiceEvent::AudioDelta { pcm, .. } => {
                // play PCM16 @ 24kHz
                println!("audio bytes: {}", pcm.len());
            }
            oai_rt_rs::VoiceEvent::TranscriptDone { transcript, .. } => {
                println!("assistant: {transcript}");
            }
            _ => {}
        }
    }
    Ok(())
}
```

## Barge-in

```rust
# async fn demo(session: &oai_rt_rs::RealtimeSession) -> oai_rt_rs::Result<()> {
// Manually barge-in (clear output + cancel active response).
session.barge_in().await?;
# Ok(())
# }
```

## Convenience audio/transcript streams

```rust
# async fn demo(mut session: oai_rt_rs::RealtimeSession) -> oai_rt_rs::Result<()> {
if let Some(chunk) = session.next_audio_chunk().await? {
    println!("audio bytes: {}", chunk.pcm.len());
}
if let Some(tx) = session.next_transcript().await? {
    println!("transcript: {}", tx.text);
}
# Ok(())
# }
```

## Response builder (high-level)

```rust
use oai_rt_rs::ResponseBuilder;

# async fn demo(session: &oai_rt_rs::RealtimeSession) -> oai_rt_rs::Result<()> {
ResponseBuilder::new()
    .output_text()
    .instructions("Be concise.")
    .input_text("Summarize this.")
    .send(session)
    .await?;
# Ok(())
# }
```

## Sending microphone audio

```rust
# async fn demo(mut session: oai_rt_rs::RealtimeSession) -> oai_rt_rs::Result<()> {
let pcm_samples: Vec<i16> = vec![0; 2400]; // 100ms @ 24kHz
session.audio().send_pcm16(&pcm_samples).await?;
# Ok(())
# }
```

## Streaming microphone audio

```rust
# async fn demo(session: oai_rt_rs::RealtimeSession) -> oai_rt_rs::Result<()> {
let chunks = vec![vec![0i16; 480], vec![1i16; 480]];
let stream = futures::stream::iter(chunks);
session.stream_audio_pcm16(stream).await?;
# Ok(())
# }
```

## Typed tools (simple)

```rust
use oai_rt_rs::Realtime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SumArgs {
    pub a: i32,
    pub b: i32,
}

#[derive(Debug, Serialize)]
pub struct SumResp {
    pub sum: i32,
}

# async fn demo() -> oai_rt_rs::Result<()> {
let _session = Realtime::builder()
    .api_key("your-api-key")
    .model("gpt-realtime")
    .voice_session()
    .tool_desc("sum", "Add two integers.", |args: SumArgs| async move {
        Ok(SumResp { sum: args.a + args.b })
    })
    .connect_ws()
    .await?;
# // By default, tool success triggers an automatic response.create.
# // Disable with Realtime::builder().auto_tool_response(false).
# Ok(())
# }
```

## Low-level protocol (full control)

```rust
use oai_rt_rs::RealtimeClient;
use oai_rt_rs::protocol::client_events::ClientEvent;
use oai_rt_rs::protocol::models::{SessionUpdate, SessionUpdateConfig, OutputModalities};

#[tokio::main]
async fn main() -> oai_rt_rs::Result<()> {
    let mut client = RealtimeClient::connect("your-api-key", None, None).await?;

    let session = SessionUpdate {
        config: SessionUpdateConfig {
            output_modalities: Some(OutputModalities::Audio),
            instructions: Some("You are a helpful assistant.".to_string()),
            ..SessionUpdateConfig::default()
        },
    };
    client
        .send(ClientEvent::SessionUpdate { event_id: None, session: Box::new(session) })
        .await?;

    while let Some(event) = client.next_event().await? {
        println!("Received event: {:?}", event);
    }
    Ok(())
}
```

## REST helpers (WebRTC/SIP)

Use the low-level REST adapter for call control:

```rust
use oai_rt_rs::transport::rest::RealtimeRestAdapter;
use oai_rt_rs::protocol::models::{SessionConfig, SessionKind, OutputModalities};

# async fn demo() -> oai_rt_rs::Result<()> {
let rest = RealtimeRestAdapter::new("your-api-key")?;
let session = SessionConfig::new(
    SessionKind::Realtime,
    "gpt-realtime",
    OutputModalities::Audio,
);

// WebRTC (raw SDP) + call_id capture
let resp = rest.post_sdp_offer_raw_with_call_id("v=0...".to_string()).await?;
println!("call_id: {:?}", resp.call_id);

// Hang up
if let Some(call_id) = resp.call_id.as_deref() {
    rest.hangup(call_id).await?;
}
# Ok(())
# }
```

## GA constraints (no beta)

- `output_modalities` must be exactly one of `audio` or `text`.
- `audio/pcm` rate is fixed at 24 kHz.
- `input_audio_buffer.append` chunks must be ≤ 15 MB (base64-decoded).
- Invalid GA inputs are rejected client-side with `Error::InvalidClientEvent`.

## MCP example

```rust
use oai_rt_rs::protocol::models::{
    McpToolConfig, RequireApproval, ApprovalMode, Tool, ToolChoice, ToolChoiceMode, SessionUpdate, SessionUpdateConfig,
};
use oai_rt_rs::protocol::client_events::ClientEvent;
use oai_rt_rs::protocol::models::SessionUpdate;

# async fn demo(mut client: oai_rt_rs::RealtimeClient) -> oai_rt_rs::Result<()> {
let mcp = Tool::Mcp(McpToolConfig {
    server_label: "weather".to_string(),
    server_url: Some("https://mcp.example.com".to_string()),
    require_approval: Some(RequireApproval::Mode(ApprovalMode::Always)),
    ..McpToolConfig::default()
});

let session = SessionUpdate {
    config: SessionUpdateConfig {
        tools: Some(vec![mcp]),
        tool_choice: Some(ToolChoice::Mode(ToolChoiceMode::Auto)),
        ..SessionUpdateConfig::default()
    },
};

client.send(ClientEvent::SessionUpdate { event_id: None, session: Box::new(session) }).await?;
# Ok(())
# }
```

## MCP approval flow (items)

```rust
use oai_rt_rs::protocol::models::{Item, ItemStatus};
use oai_rt_rs::protocol::client_events::ClientEvent;

# async fn demo(mut client: oai_rt_rs::RealtimeClient) -> oai_rt_rs::Result<()> {
let request = Item::McpApprovalRequest {
    id: Some("item_req_1".to_string()),
    status: Some(ItemStatus::InProgress),
    server_label: "weather".to_string(),
    name: "get_forecast".to_string(),
    arguments: r#"{"city":"Paris"}"#.to_string(),
};

client.send(ClientEvent::ConversationItemCreate {
    event_id: None,
    previous_item_id: None,
    item: Box::new(request),
}).await?;

let response = Item::McpApprovalResponse {
    id: Some("item_resp_1".to_string()),
    status: Some(ItemStatus::Completed),
    approval_request_id: "item_req_1".to_string(),
    approve: true,
    reason: None,
};

client.send(ClientEvent::ConversationItemCreate {
    event_id: None,
    previous_item_id: None,
    item: Box::new(response),
}).await?;
# Ok(())
# }
```

## Response creation

```rust
use oai_rt_rs::protocol::client_events::ClientEvent;
use oai_rt_rs::protocol::models::{ResponseConfig, InputItem, ContentPart, OutputModalities};

# async fn demo(mut client: oai_rt_rs::RealtimeClient) -> oai_rt_rs::Result<()> {
let response = ResponseConfig {
    output_modalities: Some(OutputModalities::Text),
    input: Some(vec![InputItem::Message {
        id: None,
        role: oai_rt_rs::protocol::models::Role::User,
        content: vec![ContentPart::InputText { text: "Hello".to_string() }],
    }]),
    ..ResponseConfig::default()
};

client.send(ClientEvent::ResponseCreate {
    event_id: None,
    response: Some(Box::new(response)),
}).await?;
# Ok(())
# }
```

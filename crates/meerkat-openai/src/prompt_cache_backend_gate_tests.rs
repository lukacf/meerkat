//! Prompt-cache fields reach only a backend that has admitted them (#1669).
//!
//! The public OpenAI API admits Meerkat's prompt-cache fields; the ChatGPT
//! backend and Azure OpenAI have not. Whoever set the fields (the factory's
//! model defaults or a host's explicit `provider_params`), a request on a
//! backend that has not admitted them carries none of them: no
//! `prompt_cache_*` body field, no `prompt_cache_breakpoint` input marker and
//! no authored cache breakpoint claim.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::lifecycle::run_primitive::{
    OpenAiPromptCacheOptions, OpenAiPromptCacheRetention, OpenAiProviderTag, ProviderTag,
};
use meerkat_core::model_profile::capabilities::{OpenAiPromptCacheMode, OpenAiPromptCacheTtl};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, SystemMessage, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmRequest};
use serde_json::Value;

use crate::{AzureOpenAiWireConfig, OpenAiClient};

/// Its catalog row admits explicit prompt caching with a 30-minute TTL.
const MODEL: &str = "gpt-5.6-sol";

fn request(tag: OpenAiProviderTag) -> LlmRequest {
    let messages = vec![
        Message::System(SystemMessage::new("You are a helpful assistant.")),
        Message::User(UserMessage::text("First question")),
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "First answer".into(),
                meta: None,
            }],
            StopReason::EndTurn,
        )),
        Message::User(UserMessage::text("Second question")),
    ];
    let mut request = LlmRequest::new(MODEL, messages);
    request.provider_params = Some(ProviderTag::OpenAi(tag));
    request
}

fn explicit_cache_tag() -> OpenAiProviderTag {
    OpenAiProviderTag {
        prompt_cache_enabled: Some(true),
        prompt_cache_key: Some("host:explicit:key".into()),
        prompt_cache_options: Some(OpenAiPromptCacheOptions {
            mode: Some(OpenAiPromptCacheMode::Explicit),
            ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
        }),
        ..Default::default()
    }
}

fn chatgpt() -> OpenAiClient {
    OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire()
}

fn azure() -> OpenAiClient {
    OpenAiClient::new("test-key".to_string())
        .with_azure_openai_wire(AzureOpenAiWireConfig::default())
}

fn public() -> OpenAiClient {
    OpenAiClient::new("test-key".to_string())
}

fn cache_body_fields(body: &Value) -> Vec<&'static str> {
    [
        "prompt_cache_key",
        "prompt_cache_options",
        "prompt_cache_retention",
    ]
    .into_iter()
    .filter(|field| body.get(*field).is_some())
    .collect()
}

fn breakpoint_markers(body: &Value) -> usize {
    serde_json::to_string(body)
        .unwrap()
        .matches("prompt_cache_breakpoint")
        .count()
}

fn assert_no_cache_on_wire(client: &OpenAiClient, tag: OpenAiProviderTag, wire: &str) {
    let request = request(tag);
    let body = client.build_request_body(&request).expect("request body");
    assert!(
        cache_body_fields(&body).is_empty(),
        "{wire}: cache fields reached the wire: {:?}",
        cache_body_fields(&body)
    );
    assert_eq!(
        breakpoint_markers(&body),
        0,
        "{wire}: cache breakpoints reached the wire"
    );
    let claims = client
        .authored_cache_breakpoints(&request, &request.messages)
        .expect("authored breakpoints");
    assert!(
        claims.is_empty(),
        "{wire}: no cache breakpoint may be claimed: {claims:?}"
    );
}

#[test]
fn explicit_cache_params_never_reach_the_chatgpt_backend_wire() {
    assert_no_cache_on_wire(&chatgpt(), explicit_cache_tag(), "chatgpt backend");
}

#[test]
fn explicit_cache_params_never_reach_the_azure_openai_wire() {
    assert_no_cache_on_wire(&azure(), explicit_cache_tag(), "azure openai");
}

#[test]
fn a_bare_prompt_cache_enabled_is_not_lowered_to_implicit_mode_on_an_unadmitted_backend() {
    // The issue's case: `prompt_cache_enabled: true` alone became
    // `prompt_cache_options: {mode: "implicit"}` on every backend.
    for (client, wire) in [(chatgpt(), "chatgpt backend"), (azure(), "azure openai")] {
        assert_no_cache_on_wire(
            &client,
            OpenAiProviderTag {
                prompt_cache_enabled: Some(true),
                ..Default::default()
            },
            wire,
        );
    }
}

#[test]
fn a_cache_opt_out_and_a_retention_send_no_cache_field_on_an_unadmitted_backend() {
    for (client, wire) in [(chatgpt(), "chatgpt backend"), (azure(), "azure openai")] {
        assert_no_cache_on_wire(
            &client,
            OpenAiProviderTag {
                prompt_cache_enabled: Some(false),
                ..Default::default()
            },
            wire,
        );
        assert_no_cache_on_wire(
            &client,
            OpenAiProviderTag {
                prompt_cache_retention: Some(OpenAiPromptCacheRetention::InMemory),
                ..Default::default()
            },
            wire,
        );
    }
}

#[test]
fn the_public_openai_wire_keeps_explicit_cache_params() {
    let client = public();
    let request = request(explicit_cache_tag());
    let body = client.build_request_body(&request).expect("request body");
    assert_eq!(body["prompt_cache_key"], "host:explicit:key");
    assert_eq!(body["prompt_cache_options"]["mode"], "explicit");
    assert!(
        breakpoint_markers(&body) > 0,
        "explicit mode authors input breakpoints"
    );
    let claims = client
        .authored_cache_breakpoints(&request, &request.messages)
        .expect("authored breakpoints");
    assert!(
        !claims.is_empty(),
        "explicit mode on the public API claims its breakpoints"
    );
}

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

// ---------------------------------------------------------------------------
// The OpenAI-compatible and GitHub Copilot routes
// ---------------------------------------------------------------------------
//
// Every route that lowers an OpenAI tag applies the same rule: a self-hosted
// server through the compatible client (Chat Completions and Responses), and
// the Copilot `/responses` and `/chat/completions` routes. None of those
// backends has admitted the fields, so a request through them carries none,
// a disabled cache included.

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
use futures::StreamExt;
use tokio::net::TcpListener;

use crate::client_compatible::OpenAiCompatibleMode;
use crate::{OpenAiCompatibleClient, OpenAiCompatibleClientOptions};

const RESPONSES_SSE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",",
    "\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

const CHAT_SSE: &str = concat!(
    "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},",
    "\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
    "\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1}}\n\n",
    "data: [DONE]\n\n",
);

/// A remote model outside the catalog, as a self-hosted server serves one: no
/// catalog row refuses a cache mode or retention before the request is sent.
const SELF_HOSTED_REMOTE_MODEL: &str = "served-model";

/// Every way a tag asks for caching: explicit params, a bare enable (lowered
/// to implicit mode on an admitted backend), the durable opt-out
/// (explicit-only mode on an admitted backend) and a retention.
fn cache_tags() -> Vec<(&'static str, OpenAiProviderTag)> {
    vec![
        ("explicit params", explicit_cache_tag()),
        (
            "bare enable",
            OpenAiProviderTag {
                prompt_cache_enabled: Some(true),
                ..Default::default()
            },
        ),
        (
            "disabled cache",
            OpenAiProviderTag {
                prompt_cache_enabled: Some(false),
                ..Default::default()
            },
        ),
        (
            "retention",
            OpenAiProviderTag {
                prompt_cache_retention: Some(OpenAiPromptCacheRetention::TwentyFourHours),
                ..Default::default()
            },
        ),
    ]
}

/// Assert that no request body carries a cache field or breakpoint, naming
/// every tag whose request did.
fn assert_no_cache_on_the_wire(wire: &str, bodies: &[(&'static str, Value)]) {
    let leaks: Vec<String> = bodies
        .iter()
        .filter_map(|(label, body)| {
            let fields = cache_body_fields(body);
            let markers = breakpoint_markers(body);
            (!fields.is_empty() || markers > 0)
                .then(|| format!("{label}: fields {fields:?}, {markers} breakpoint(s)"))
        })
        .collect();
    assert!(
        leaks.is_empty(),
        "{wire}: cache params reached the wire: {leaks:#?}"
    );
}

/// A self-hosted client as the factory builds one.
fn self_hosted(mode: OpenAiCompatibleMode, base_url: String) -> OpenAiCompatibleClient {
    OpenAiCompatibleClient::new_with_options(
        mode,
        SELF_HOSTED_REMOTE_MODEL.to_string(),
        base_url,
        None,
        OpenAiCompatibleClientOptions {
            supports_temperature: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_image_tool_results: false,
        },
    )
    .with_image_input_support(true)
    .with_static_credential()
}

#[derive(Clone)]
struct BodyCapture {
    bodies: Arc<Mutex<Vec<Value>>>,
    payload: &'static str,
}

async fn capture_body(
    State(capture): State<BodyCapture>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    capture.bodies.lock().expect("capture lock").push(body);
    ([("content-type", "text/event-stream")], capture.payload)
}

/// Send one request per tag through the client `build` makes for a local
/// server answering `path`, and return the body each request put on the wire.
async fn bodies_on_the_wire(
    path: &str,
    payload: &'static str,
    build: impl Fn(String) -> Arc<dyn LlmClient>,
) -> Vec<(&'static str, Value)> {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(path, post(capture_body))
        .with_state(BodyCapture {
            bodies: Arc::clone(&bodies),
            payload,
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let client = build(format!("http://{addr}"));
    let tags = cache_tags();
    for (_, tag) in &tags {
        let request = request(tag.clone());
        let mut stream = client.stream(&request);
        while stream.next().await.is_some() {}
    }
    server.abort();
    let bodies = std::mem::take(&mut *bodies.lock().expect("capture lock"));
    assert_eq!(
        bodies.len(),
        tags.len(),
        "every request reached the wire: {bodies:?}"
    );
    tags.into_iter()
        .map(|(label, _)| label)
        .zip(bodies)
        .collect()
}

#[test]
fn cache_params_never_reach_a_self_hosted_chat_completions_wire() {
    let client = self_hosted(
        OpenAiCompatibleMode::ChatCompletions,
        "http://127.0.0.1:9/v1".to_string(),
    );
    let bodies: Vec<(&'static str, Value)> = cache_tags()
        .into_iter()
        .map(|(label, tag)| {
            let body = client
                .build_chat_completions_body(&request(tag))
                .expect("chat completions body");
            (label, body)
        })
        .collect();
    assert_no_cache_on_the_wire("self-hosted chat completions", &bodies);
}

#[tokio::test]
async fn cache_params_never_reach_a_self_hosted_responses_wire() {
    let bodies = bodies_on_the_wire("/v1/responses", RESPONSES_SSE, |base| {
        Arc::new(self_hosted(
            OpenAiCompatibleMode::Responses,
            format!("{base}/v1"),
        ))
    })
    .await;
    assert_no_cache_on_the_wire("self-hosted responses", &bodies);
}

#[cfg(feature = "copilot")]
mod copilot_routes {
    use super::*;
    use crate::runtime::{CopilotOpenAiRouteTarget, copilot_openai_route_client};

    struct CopilotTokenAuthorizer;

    #[async_trait::async_trait]
    impl meerkat_core::HttpAuthorizer for CopilotTokenAuthorizer {
        async fn authorize(
            &self,
            request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
        ) -> Result<(), meerkat_core::AuthError> {
            request.headers.push((
                "Authorization".to_string(),
                "Bearer copilot-token".to_string(),
            ));
            Ok(())
        }

        fn label(&self) -> &str {
            meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL
        }
    }

    /// The client the OpenAI Copilot route builds for `endpoint`.
    fn copilot_route(
        endpoint: meerkat_copilot::CopilotEndpoint,
        api_base: String,
    ) -> Arc<dyn LlmClient> {
        copilot_openai_route_client(
            endpoint,
            api_base,
            Arc::new(CopilotTokenAuthorizer),
            &CopilotOpenAiRouteTarget {
                model: MODEL.to_string(),
                supports_temperature: false,
                supports_image_input: true,
                supports_image_tool_results: true,
            },
        )
        .expect("Copilot route client")
    }

    #[tokio::test]
    async fn cache_params_never_reach_the_copilot_responses_wire() {
        let bodies = bodies_on_the_wire("/responses", RESPONSES_SSE, |base| {
            copilot_route(meerkat_copilot::CopilotEndpoint::Responses, base)
        })
        .await;
        assert_no_cache_on_the_wire("copilot responses", &bodies);
    }

    #[tokio::test]
    async fn cache_params_never_reach_the_copilot_chat_completions_wire() {
        let bodies = bodies_on_the_wire("/chat/completions", CHAT_SSE, |base| {
            copilot_route(meerkat_copilot::CopilotEndpoint::ChatCompletions, base)
        })
        .await;
        assert_no_cache_on_the_wire("copilot chat completions", &bodies);
    }
}

/// The Copilot `/responses` wire also authors no cache breakpoint claim.
#[test]
fn explicit_cache_params_never_reach_the_copilot_responses_wire_or_its_claims() {
    assert_no_cache_on_wire(
        &OpenAiClient::new("test-key".to_string()).with_copilot_backend_wire(),
        explicit_cache_tag(),
        "copilot responses",
    );
}

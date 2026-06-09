//! OpenAI-native web-search fallback executor.

use async_trait::async_trait;
use meerkat_core::lifecycle::run_primitive::{
    ModelId, OpaqueProviderBody, OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
};
use meerkat_core::web_search::{
    WebSearchEvidence, WebSearchNativeEvent, WebSearchRequest, WebSearchResult, WebSearchStatus,
};
use meerkat_core::{AgentLlmClient, AssistantBlock, Message, Provider, SystemMessage, UserMessage};
use meerkat_llm_core::{LlmError, WebSearchExecutor};
use std::sync::Arc;

#[derive(Clone)]
pub struct OpenAiWebSearchExecutor {
    model: String,
    client: Arc<dyn AgentLlmClient>,
}

impl OpenAiWebSearchExecutor {
    pub fn new(model: String, client: Arc<dyn AgentLlmClient>) -> Self {
        Self { model, client }
    }

    fn provider_tag(provider_params: Option<serde_json::Value>) -> ProviderTag {
        let mut body = serde_json::json!({"type": "web_search"});
        merge_object(&mut body, provider_params);
        ProviderTag::OpenAi(OpenAiProviderTag {
            web_search: Some(OpaqueProviderBody::from_value(&body)),
            ..Default::default()
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WebSearchExecutor for OpenAiWebSearchExecutor {
    async fn execute_web_search(
        &self,
        request: WebSearchRequest,
    ) -> Result<WebSearchResult, LlmError> {
        if let Some(requested_provider) = request.provider
            && requested_provider != Provider::OpenAI
        {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "web_search fallback is configured for '{}', not '{}'",
                    Provider::OpenAI.as_str(),
                    requested_provider.as_str()
                ),
            });
        }

        execute_native_web_search(
            self.client.as_ref(),
            Provider::OpenAI,
            &self.model,
            request,
            Self::provider_tag,
        )
        .await
    }
}

async fn execute_native_web_search(
    client: &dyn AgentLlmClient,
    provider: Provider,
    model: &str,
    request: WebSearchRequest,
    provider_tag: impl FnOnce(Option<serde_json::Value>) -> ProviderTag,
) -> Result<WebSearchResult, LlmError> {
    let prompt = web_search_user_prompt(&request);
    let messages = vec![
        Message::System(SystemMessage::new(
            "You are a web-search execution adapter for Meerkat. Use your \
             provider-native web search/grounding tool to answer the query. \
             Return a concise answer and include cited URLs when available. \
             Do not call non-search tools.",
        )),
        Message::User(UserMessage::text(prompt)),
    ];
    let provider_params = ProviderParamsOverride {
        max_output_tokens: Some(2048),
        provider_tag: Some(provider_tag(request.provider_params.clone())),
        ..Default::default()
    };
    // Propagate the typed provider failure class instead of laundering every
    // failure into a stringly-typed LlmError::Unknown that erases whether the
    // search was rate-limited, auth-rejected, content-filtered, etc.
    let result = client
        .stream_response(&messages, &[], 2048, None, Some(&provider_params))
        .await
        .map_err(map_agent_error_to_llm_error)?;

    let mut answer_parts = Vec::new();
    let mut native_events = Vec::new();
    let mut evidence = Vec::new();
    for block in result.blocks() {
        match block {
            AssistantBlock::Text { text, .. } | AssistantBlock::Transcript { text, .. } => {
                if !text.trim().is_empty() {
                    answer_parts.push(text.trim().to_string());
                }
            }
            AssistantBlock::ServerToolContent { kind, content, .. } => {
                collect_evidence(content, &mut evidence);
                native_events.push(WebSearchNativeEvent {
                    // Derived projection of the typed kind owner.
                    name: kind.provider_name().to_string(),
                    content: content.clone(),
                });
            }
            _ => {}
        }
    }

    // Gate Completed on a typed witness that the provider actually ran its
    // native search: a `ServerToolContent` native-search event must have been
    // observed. A stream that produced only assistant prose (no native search
    // event) is NOT verified evidence of a search having run, so it reports
    // Unavailable rather than laundering an unverified turn into Completed.
    if native_events.is_empty() {
        return Ok(WebSearchResult {
            status: WebSearchStatus::Unavailable,
            query: request.query,
            provider: Some(provider),
            model: Some(ModelId::new(model.to_string())),
            answer: (!answer_parts.is_empty()).then(|| answer_parts.join("\n\n")),
            evidence,
            native_events,
            error: Some(
                "provider stream produced no native web-search event; \
                 search execution is unverified"
                    .to_string(),
            ),
            checked_at: chrono::Utc::now(),
        });
    }

    Ok(WebSearchResult {
        status: WebSearchStatus::Completed,
        query: request.query,
        provider: Some(provider),
        model: Some(ModelId::new(model.to_string())),
        answer: (!answer_parts.is_empty()).then(|| answer_parts.join("\n\n")),
        evidence,
        native_events,
        error: None,
        checked_at: chrono::Utc::now(),
    })
}

/// Translate the agent-loop [`AgentError`] returned by `stream_response` into a
/// typed [`LlmError`], preserving the failure class instead of collapsing every
/// failure into [`LlmError::Unknown`]. There is intentionally no blanket
/// `From<AgentError> for LlmError`: the web-search executor owns this narrow
/// projection at its provider seam.
fn map_agent_error_to_llm_error(err: meerkat_core::AgentError) -> LlmError {
    use meerkat_core::AgentError;
    use meerkat_core::error::{LlmFailureReason, LlmProviderErrorKind};

    match err {
        AgentError::Llm {
            reason, message, ..
        } => match reason {
            LlmFailureReason::RateLimited { retry_after } => LlmError::RateLimited {
                retry_after_ms: retry_after.map(|d| d.as_millis() as u64),
            },
            LlmFailureReason::ContextExceeded { max, requested } => {
                LlmError::ContextLengthExceeded {
                    max: max as usize,
                    requested: requested as usize,
                }
            }
            LlmFailureReason::AuthError => LlmError::AuthenticationFailed { message },
            LlmFailureReason::InvalidModel(model) => LlmError::ModelNotFound { model },
            LlmFailureReason::NetworkTimeout { duration_ms }
            | LlmFailureReason::CallTimeout { duration_ms } => {
                LlmError::NetworkTimeout { duration_ms }
            }
            LlmFailureReason::ProviderError(provider_error) => match provider_error.kind {
                LlmProviderErrorKind::InvalidRequest => LlmError::InvalidRequest { message },
                LlmProviderErrorKind::ContentFiltered => {
                    LlmError::ContentFiltered { reason: message }
                }
                LlmProviderErrorKind::ServerError => LlmError::ServerError {
                    status: 500,
                    message,
                },
                LlmProviderErrorKind::ServerOverloaded => LlmError::ServerOverloaded,
                LlmProviderErrorKind::ConnectionReset => LlmError::ConnectionReset,
                LlmProviderErrorKind::StreamParseError => LlmError::StreamParseError { message },
                LlmProviderErrorKind::IncompleteResponse => {
                    LlmError::IncompleteResponse { message }
                }
                LlmProviderErrorKind::Unknown => LlmError::Unknown { message },
            },
            // LlmFailureReason is #[non_exhaustive]: a future reason maps to a
            // typed Unknown carrying the provider message, never silently dropped.
            _ => LlmError::Unknown { message },
        },
        AgentError::TokenBudgetExceeded { .. }
        | AgentError::TimeBudgetExceeded { .. }
        | AgentError::ToolCallBudgetExceeded { .. }
        | AgentError::MaxTurnsReached { .. }
        | AgentError::MaxTokensReached { .. } => LlmError::InvalidRequest {
            message: err.to_string(),
        },
        AgentError::ContentFiltered { .. } => LlmError::ContentFiltered {
            reason: err.to_string(),
        },
        other => LlmError::Unknown {
            message: other.to_string(),
        },
    }
}

fn merge_object(body: &mut serde_json::Value, provider_params: Option<serde_json::Value>) {
    if let Some(extra) = provider_params
        && let (Some(base), Some(extra)) = (body.as_object_mut(), extra.as_object())
    {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
}

fn web_search_user_prompt(request: &WebSearchRequest) -> String {
    let mut prompt = format!("Search query:\n{}", request.query);
    if let Some(context) = request
        .context
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        prompt.push_str("\n\nConversation context:\n");
        prompt.push_str(context.trim());
    }
    prompt
}

fn collect_evidence(value: &serde_json::Value, out: &mut Vec<WebSearchEvidence>) {
    match value {
        serde_json::Value::Object(map) => {
            let url = map
                .get("url")
                .or_else(|| map.get("uri"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            let title = map
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            let summary = map
                .get("summary")
                .or_else(|| map.get("snippet"))
                .or_else(|| map.get("text"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            if url.is_some() || title.is_some() || summary.is_some() {
                out.push(WebSearchEvidence {
                    title,
                    url,
                    summary,
                });
            }
            for child in map.values() {
                collect_evidence(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_evidence(child, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::ServerToolKind;
    use meerkat_core::agent::LlmStreamResult;
    use meerkat_core::error::LlmFailureReason;
    use meerkat_core::types::Usage;
    use meerkat_core::{AgentError, StopReason, ToolDef};
    use std::sync::Mutex;

    /// A fake LLM client that yields one canned outcome per call. The outcome
    /// is moved out so a single configured result is consumed exactly once.
    struct FakeClient {
        outcome: Mutex<Option<Result<LlmStreamResult, AgentError>>>,
    }

    impl FakeClient {
        fn ok(blocks: Vec<AssistantBlock>) -> Arc<dyn AgentLlmClient> {
            Arc::new(Self {
                outcome: Mutex::new(Some(Ok(LlmStreamResult::new(
                    blocks,
                    StopReason::EndTurn,
                    Usage::default(),
                )))),
            })
        }

        fn err(err: AgentError) -> Arc<dyn AgentLlmClient> {
            Arc::new(Self {
                outcome: Mutex::new(Some(Err(err))),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for FakeClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<LlmStreamResult, AgentError> {
            self.outcome
                .lock()
                .expect("fake outcome lock")
                .take()
                .expect("FakeClient outcome consumed more than once")
        }

        fn provider(&self) -> &'static str {
            "openai"
        }

        fn model(&self) -> &'static str {
            "gpt-realtime-1.5"
        }
    }

    fn request() -> WebSearchRequest {
        WebSearchRequest {
            query: "what is meerkat".to_string(),
            provider: None,
            provider_params: None,
            context: None,
        }
    }

    fn server_tool_block() -> AssistantBlock {
        AssistantBlock::ServerToolContent {
            id: Some("ws_1".to_string()),
            kind: ServerToolKind::WebSearch,
            content: serde_json::json!({
                "url": "https://example.com",
                "title": "Meerkat",
                "summary": "A small mammal."
            }),
            meta: None,
        }
    }

    fn text_block(text: &str) -> AssistantBlock {
        AssistantBlock::Text {
            text: text.to_string(),
            meta: None,
        }
    }

    // Row #180: a stream that produced only assistant prose (no native
    // search event) is NOT verified evidence of a search and must NOT report
    // Completed.
    #[tokio::test]
    async fn stream_without_native_search_event_is_not_completed() {
        let client = FakeClient::ok(vec![text_block("I think meerkats are small.")]);
        let executor = OpenAiWebSearchExecutor::new("gpt-realtime-1.5".to_string(), client);
        let result = executor
            .execute_web_search(request())
            .await
            .expect("a successful stream must still resolve to a typed result");
        assert_ne!(
            result.status,
            WebSearchStatus::Completed,
            "no native web-search event ran; status must not be Completed"
        );
        assert_eq!(result.status, WebSearchStatus::Unavailable);
        assert!(
            result.error.is_some(),
            "an unverified search must carry an explanatory witness"
        );
    }

    #[tokio::test]
    async fn stream_with_native_search_event_is_completed() {
        let client = FakeClient::ok(vec![text_block("Meerkats are small."), server_tool_block()]);
        let executor = OpenAiWebSearchExecutor::new("gpt-realtime-1.5".to_string(), client);
        let result = executor
            .execute_web_search(request())
            .await
            .expect("native search event present");
        assert_eq!(result.status, WebSearchStatus::Completed);
        assert!(
            !result.native_events.is_empty(),
            "the native-search witness must be retained"
        );
    }

    // Row #180: a stream error must propagate the typed failure class, not be
    // laundered into LlmError::Unknown.
    #[tokio::test]
    async fn stream_error_propagates_typed_llm_error() {
        let client = FakeClient::err(AgentError::Llm {
            provider: "openai",
            reason: LlmFailureReason::RateLimited { retry_after: None },
            message: "rate limited".to_string(),
        });
        let executor = OpenAiWebSearchExecutor::new("gpt-realtime-1.5".to_string(), client);
        let err = executor
            .execute_web_search(request())
            .await
            .expect_err("a stream error must surface as a typed LlmError");
        assert!(
            matches!(err, LlmError::RateLimited { .. }),
            "expected the typed RateLimited class to survive, got Unknown laundering: {err:?}"
        );
    }
}

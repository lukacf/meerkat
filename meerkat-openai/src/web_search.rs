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
    let result = client
        .stream_response(&messages, &[], 2048, None, Some(&provider_params))
        .await
        .map_err(|err| LlmError::Unknown {
            message: err.to_string(),
        })?;

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
            AssistantBlock::ServerToolContent { name, content, .. } => {
                collect_evidence(content, &mut evidence);
                native_events.push(WebSearchNativeEvent {
                    name: name.clone(),
                    content: content.clone(),
                });
            }
            _ => {}
        }
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

//! Builtin Meerkat-owned web-search fallback tool.

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use meerkat_core::types::{ToolDef, ToolProvenance, ToolSourceKind};
use meerkat_core::web_search::{
    WEB_SEARCH_TOOL_NAME, WebSearchRequest, WebSearchResult, WebSearchStatus,
};
use meerkat_core::{Provider, WebSearchEvidence, WebSearchNativeEvent};
use meerkat_llm_core::WebSearchExecutor;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

const WEB_SEARCH_TOOL_DOCUMENTATION: &str = r#"Search the web through Meerkat when the active model does not have provider-native web search.

Use this tool when the user asks for current, recent, online, or cited information and no native provider web-search tool is available in this session.

Request shape:
{"query":"latest Meerkat release notes","provider":"openai"}

Fields:
- query: required natural-language search/research query.
- provider: optional provider override: "openai", "gemini", or "anthropic". If omitted, Meerkat chooses the first configured search provider in OpenAI, Gemini, Anthropic order.
- provider_params: optional provider-native search-tool options. These are merged into the selected provider's native web-search body.
- context: optional brief context from the conversation to help disambiguate the query.

Result shape follows the common provider-native search idea: status, provider, model, answer, evidence, and native_events. Treat native_events as provider-observed evidence, not Meerkat-owned truth."#;

#[derive(Debug, Deserialize, JsonSchema)]
struct WebSearchToolArgs {
    #[schemars(description = "Natural-language query to search for.")]
    query: String,
    #[serde(default)]
    #[schemars(description = "Optional provider override: openai, gemini, or anthropic.")]
    provider: Option<String>,
    #[serde(default)]
    #[schemars(description = "Optional provider-native search-tool options.")]
    provider_params: Option<Value>,
    #[serde(default)]
    #[schemars(description = "Optional brief conversation context to disambiguate the search.")]
    context: Option<String>,
}

#[derive(Clone)]
pub struct WebSearchTool {
    executor: Arc<dyn WebSearchExecutor>,
}

impl WebSearchTool {
    pub fn new(executor: Arc<dyn WebSearchExecutor>) -> Self {
        Self { executor }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for WebSearchTool {
    fn name(&self) -> &'static str {
        WEB_SEARCH_TOOL_NAME
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: WEB_SEARCH_TOOL_DOCUMENTATION.to_string(),
            input_schema: crate::schema::schema_for::<WebSearchToolArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: WebSearchToolArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?;
        let query = args.query.trim().to_string();
        if query.is_empty() {
            return Err(BuiltinToolError::invalid_args("query must not be empty"));
        }
        let provider = args
            .provider
            .as_deref()
            .map(parse_search_provider)
            .transpose()?;
        let result = self
            .executor
            .execute_web_search(WebSearchRequest {
                query,
                provider,
                provider_params: args.provider_params,
                context: args.context.filter(|value| !value.trim().is_empty()),
            })
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        serde_json::to_value(result)
            .map(ToolOutput::Json)
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))
    }
}

fn parse_search_provider(value: &str) -> Result<Provider, BuiltinToolError> {
    match Provider::parse_strict(value) {
        Some(provider @ (Provider::OpenAI | Provider::Gemini | Provider::Anthropic)) => {
            Ok(provider)
        }
        Some(other) => Err(BuiltinToolError::invalid_args(format!(
            "provider '{}' does not support Meerkat web_search fallback",
            other.as_str()
        ))),
        None => Err(BuiltinToolError::invalid_args(format!(
            "unknown provider '{value}' (expected openai, gemini, or anthropic)"
        ))),
    }
}

#[derive(Default)]
pub struct EmptyWebSearchExecutor;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WebSearchExecutor for EmptyWebSearchExecutor {
    async fn execute_web_search(
        &self,
        request: WebSearchRequest,
    ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
        Ok(WebSearchResult {
            status: WebSearchStatus::Unavailable,
            query: request.query,
            provider: request.provider,
            model: None,
            answer: None,
            evidence: Vec::<WebSearchEvidence>::new(),
            native_events: Vec::<WebSearchNativeEvent>::new(),
            error: Some("no configured provider supports Meerkat web_search fallback".to_string()),
            checked_at: chrono::Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::run_primitive::ModelId;
    use meerkat_core::{WebSearchStatus, web_search::WebSearchResult};
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct RecordingExecutor {
        seen: Mutex<Vec<WebSearchRequest>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl WebSearchExecutor for RecordingExecutor {
        async fn execute_web_search(
            &self,
            request: WebSearchRequest,
        ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
            self.seen.lock().await.push(request.clone());
            Ok(WebSearchResult {
                status: WebSearchStatus::Completed,
                query: request.query,
                provider: Some(request.provider.unwrap_or(Provider::OpenAI)),
                model: Some(ModelId::new("gpt-5.5")),
                answer: Some("answer".to_string()),
                evidence: Vec::new(),
                native_events: Vec::new(),
                error: None,
                checked_at: chrono::Utc::now(),
            })
        }
    }

    #[tokio::test]
    async fn web_search_tool_accepts_provider_override() -> Result<(), String> {
        let executor = Arc::new(RecordingExecutor::default());
        let tool = WebSearchTool::new(executor.clone());
        let output = tool
            .call(serde_json::json!({
                "query": "today's news",
                "provider": "gemini",
                "provider_params": {"allowed_domains": ["example.com"]},
                "context": "smoke test"
            }))
            .await
            .expect("tool call should succeed");
        let value = match output {
            ToolOutput::Json(value) => value,
            other => return Err(format!("expected JSON output, got {other:?}")),
        };
        assert_eq!(value["status"], "completed");
        assert_eq!(value["provider"], "gemini");
        let seen = executor.seen.lock().await;
        assert_eq!(seen[0].provider, Some(Provider::Gemini));
        assert_eq!(
            seen[0].provider_params,
            Some(serde_json::json!({"allowed_domains": ["example.com"]}))
        );
        Ok(())
    }

    #[tokio::test]
    async fn web_search_tool_rejects_non_search_provider() {
        let tool = WebSearchTool::new(Arc::new(RecordingExecutor::default()));
        let err = tool
            .call(serde_json::json!({"query": "x", "provider": "self_hosted"}))
            .await
            .expect_err("self-hosted cannot own fallback search");
        assert!(err.to_string().contains("does not support"));
    }
}

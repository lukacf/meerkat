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
{"query":"latest Meerkat release notes"}

Fields:
- query: required natural-language search/research query.
- provider: optional provider assertion: "openai", "gemini", or "anthropic". If omitted, Meerkat uses the session's configured fallback provider. If present, it must match that provider.
- context: optional brief context from the conversation to help disambiguate the query.

Result shape follows the common provider-native search idea: status, provider, model, answer, evidence, and native_events. Treat native_events as provider-observed evidence, not Meerkat-owned truth."#;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WebSearchToolArgs {
    #[schemars(description = "Natural-language query to search for.")]
    query: String,
    #[serde(default)]
    #[schemars(description = "Optional provider override: openai, gemini, or anthropic.")]
    provider: Option<String>,
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

    /// Retrieval-only outbound query; it publishes nothing and writes no local state.
    fn mutation_class(&self) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::ReadOnly
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        self.search(args, None).await
    }

    /// A governed turn carries its admitted work authorization into the
    /// helper's own model request. The outer `ReadOnly` admission and the tool
    /// name check do not authorize the helper's model, account, endpoint or
    /// hosted search; the executor prepares those facts for the actual target.
    async fn call_with_context(
        &self,
        _call: meerkat_core::ToolCallView<'_>,
        args: Value,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolOutput, BuiltinToolError> {
        let authorization = context.work_authorization().map(|work| {
            meerkat_core::LlmRequestAuthorization::new(
                work.clone(),
                meerkat_core::OperationId::new(),
                meerkat_core::authorization::ModelAuthorizationUse::Inference,
            )
            .with_coordinates(context.run_id().cloned(), None)
        });
        self.search(args, authorization).await
    }
}

impl WebSearchTool {
    async fn search(
        &self,
        args: Value,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<ToolOutput, BuiltinToolError> {
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
            .execute_web_search_authorized(
                WebSearchRequest {
                    query,
                    provider,
                    provider_params: None,
                    context: args.context.filter(|value| !value.trim().is_empty()),
                },
                authorization,
            )
            .await
            .map_err(|err| match err {
                meerkat_llm_core::LlmError::OperationRefused { refusal } => {
                    BuiltinToolError::OperationRefused { refusal }
                }
                meerkat_llm_core::LlmError::OperationObservationUnavailable => {
                    BuiltinToolError::OperationObservationUnavailable
                }
                meerkat_llm_core::LlmError::OperationAuthorizationUnavailable => {
                    BuiltinToolError::OperationAuthorizationUnavailable
                }
                meerkat_llm_core::LlmError::OperationReviewRefused { refusal } => {
                    BuiltinToolError::EntryRefused(Box::new(refusal.into()))
                }
                other => BuiltinToolError::execution_failed(other.to_string()),
            })?;
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

    /// Reports unavailability without any model request, so there is no
    /// helper operation for the companion to govern.
    async fn execute_web_search_authorized(
        &self,
        request: WebSearchRequest,
        _authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
        self.execute_web_search(request).await
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
        assert_eq!(seen[0].provider_params, None);
        Ok(())
    }

    #[tokio::test]
    async fn web_search_tool_rejects_provider_native_params() {
        let tool = WebSearchTool::new(Arc::new(RecordingExecutor::default()));
        let err = tool
            .call(serde_json::json!({
                "query": "today's news",
                "provider_params": {"allowed_domains": ["example.com"]}
            }))
            .await
            .expect_err("model-callable fallback search must not accept provider-native params");
        assert!(err.to_string().contains("unknown field"));
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

    struct UnusedPolicy;
    impl meerkat_core::authorization::WorkAuthorization for UnusedPolicy {
        fn prepare(
            &self,
            _: &meerkat_core::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Err(meerkat_core::authorization::OperationRefused::new(
                meerkat_core::authorization::OperationRefusalKind::Denied,
            )
            .into())
        }
    }

    fn governed_context() -> meerkat_core::ToolDispatchContext {
        meerkat_core::ToolDispatchContext::default().with_work_authorization(Some(
            meerkat_core::WorkAuthorizationContext::new(
                Arc::new(UnusedPolicy),
                meerkat_core::exact_operation::OperationExecutionScope::Domain,
            ),
        ))
    }

    async fn call_in(
        tool: &WebSearchTool,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolOutput, BuiltinToolError> {
        let args = serde_json::json!({"query": "fixed query"});
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        tool.call_with_context(
            meerkat_core::ToolCallView {
                id: "search-call",
                name: WEB_SEARCH_TOOL_NAME,
                args: &raw,
            },
            args,
            context,
        )
        .await
    }

    /// Stands in for a helper whose own model operation is refused.
    #[derive(Default)]
    struct RefusedHelper {
        authorized: Mutex<Vec<bool>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl WebSearchExecutor for RefusedHelper {
        async fn execute_web_search(
            &self,
            _request: WebSearchRequest,
        ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
            Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: "the tool always uses the authorized seam".to_string(),
            })
        }

        async fn execute_web_search_authorized(
            &self,
            _request: WebSearchRequest,
            authorization: Option<meerkat_core::LlmRequestAuthorization>,
        ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
            self.authorized.lock().await.push(authorization.is_some());
            Err(meerkat_llm_core::LlmError::operation_refused(
                meerkat_core::authorization::OperationRefusalKind::Denied,
            ))
        }
    }

    #[tokio::test]
    async fn governed_turn_forwards_work_authorization_and_keeps_refusal_typed() {
        let executor = Arc::new(RefusedHelper::default());
        let tool = WebSearchTool::new(executor.clone());
        let error = call_in(&tool, &governed_context())
            .await
            .expect_err("a refused helper is not a search result");
        assert!(
            matches!(
                &error,
                BuiltinToolError::OperationRefused { refusal }
                    if refusal.kind() == meerkat_core::authorization::OperationRefusalKind::Denied
            ),
            "refusal must not flatten into ExecutionFailed: {error:?}"
        );
        assert_eq!(*executor.authorized.lock().await, vec![true]);
    }

    #[tokio::test]
    async fn executor_without_an_authorized_seam_refuses_governed_work_without_searching() {
        let executor = Arc::new(RecordingExecutor::default());
        let tool = WebSearchTool::new(executor.clone());
        let error = call_in(&tool, &governed_context())
            .await
            .expect_err("an executor that cannot bind the helper must refuse");
        assert!(
            matches!(error, BuiltinToolError::OperationRefused { .. }),
            "{error:?}"
        );
        assert!(executor.seen.lock().await.is_empty(), "zero searches");

        // Without admitted work the legacy contract is unchanged.
        call_in(&tool, &meerkat_core::ToolDispatchContext::default())
            .await
            .expect("ungoverned search keeps its existing behavior");
        assert_eq!(executor.seen.lock().await.len(), 1);
    }
}

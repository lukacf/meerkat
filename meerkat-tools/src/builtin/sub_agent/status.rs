//! agent_status tool - Get status and output of a sub-agent

use super::state::SubAgentToolState;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::ops::{OperationId, SubAgentState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

/// Parameters for agent_status tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct StatusParams {
    /// Sub-agent ID to query
    #[schemars(description = "The unique identifier of the sub-agent (UUID format)")]
    agent_id: String,
}

/// Response from agent_status tool
#[derive(Debug, Serialize)]
struct StatusResponse {
    /// Sub-agent ID
    agent_id: String,
    /// Current state
    state: String,
    /// Output content (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Whether this is the final result
    is_final: bool,
    /// Duration in milliseconds (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    /// Tokens used (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_used: Option<u64>,
}

/// Tool for getting status and output of a sub-agent
pub struct AgentStatusTool {
    state: Arc<SubAgentToolState>,
}

impl AgentStatusTool {
    /// Create a new agent_status tool
    pub fn new(state: Arc<SubAgentToolState>) -> Self {
        Self { state }
    }

    fn parse_agent_id(id_str: &str) -> Result<OperationId, BuiltinToolError> {
        Uuid::parse_str(id_str)
            .map(OperationId)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid agent_id format: {}", e)))
    }

    async fn get_status(&self, params: StatusParams) -> Result<StatusResponse, BuiltinToolError> {
        let op_id = Self::parse_agent_id(&params.agent_id)?;

        // Get agent info from manager
        let info = self
            .state
            .manager
            .get_agent_info(&op_id)
            .await
            .ok_or_else(|| {
                BuiltinToolError::execution_failed(format!(
                    "Sub-agent not found: {}",
                    params.agent_id
                ))
            })?;

        let state_str = match &info.state {
            SubAgentState::Running => "running",
            SubAgentState::Completed => "completed",
            SubAgentState::Failed => "failed",
            SubAgentState::Cancelled => "cancelled",
        };

        let is_final = info.state != SubAgentState::Running;

        // Get result details if available
        if let Some(result) = info.result.as_ref() {
            let content = result.content.clone();
            let (output, error) = if result.is_error {
                (None, Some(content))
            } else {
                (Some(content), None)
            };

            return Ok(StatusResponse {
                agent_id: params.agent_id,
                state: state_str.to_string(),
                output,
                error,
                is_final,
                duration_ms: Some(result.duration_ms),
                tokens_used: Some(result.tokens_used),
            });
        }

        Ok(StatusResponse {
            agent_id: params.agent_id,
            state: state_str.to_string(),
            output: None,
            error: None,
            is_final,
            duration_ms: Some(info.running_ms),
            tokens_used: None,
        })
    }
}

#[async_trait]
impl BuiltinTool for AgentStatusTool {
    fn name(&self) -> &'static str {
        "agent_status"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "agent_status".into(),
            description: "Get status and output of a sub-agent by ID. Returns the current state (running, completed, failed, cancelled) and output when available.".into(),
            input_schema: crate::schema::schema_for::<StatusParams>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Sub-agent tools are disabled by default
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: StatusParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid parameters: {}", e)))?;

        let response = self.get_status(params).await?;
        serde_json::to_value(response).map_err(|e| {
            BuiltinToolError::execution_failed(format!("Failed to serialize response: {}", e))
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::sub_agent::config::SubAgentConfig;
    use meerkat_client::{FactoryError, LlmClient, LlmClientFactory, LlmProvider};
    use meerkat_core::error::{AgentError, ToolError};
    use meerkat_core::ops::ConcurrencyLimits;
    use meerkat_core::session::Session;
    use meerkat_core::sub_agent::SubAgentManager;
    use meerkat_core::{AgentSessionStore, AgentToolDispatcher, ToolCallView, ToolResult};
    use serde_json::json;
    use tokio::sync::RwLock;

    struct MockClientFactory;

    impl LlmClientFactory for MockClientFactory {
        fn create_client(
            &self,
            _provider: LlmProvider,
            _api_key: Option<String>,
        ) -> Result<Arc<dyn LlmClient>, FactoryError> {
            Err(FactoryError::MissingApiKey("mock".into()))
        }

        fn supported_providers(&self) -> Vec<LlmProvider> {
            vec![]
        }
    }

    struct MockToolDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    struct MockSessionStore;

    #[async_trait]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    fn create_test_state() -> Arc<SubAgentToolState> {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        Arc::new(SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
        ))
    }

    #[test]
    fn test_tool_name() {
        let state = create_test_state();
        let tool = AgentStatusTool::new(state);
        assert_eq!(tool.name(), "agent_status");
    }

    #[test]
    fn test_tool_def() {
        let state = create_test_state();
        let tool = AgentStatusTool::new(state);
        let def = tool.def();

        assert_eq!(def.name, "agent_status");
        assert!(def.description.contains("status"));

        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent_id"].is_object());
        assert_eq!(schema["required"], json!(["agent_id"]));
    }

    #[test]
    fn test_default_disabled() {
        let state = create_test_state();
        let tool = AgentStatusTool::new(state);
        assert!(!tool.default_enabled());
    }

    #[tokio::test]
    async fn test_status_not_found() {
        let state = create_test_state();
        let tool = AgentStatusTool::new(state);

        let result = tool
            .call(json!({
                "agent_id": "019467d9-7e3a-7000-8000-000000000000"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_status_invalid_uuid() {
        let state = create_test_state();
        let tool = AgentStatusTool::new(state);

        let result = tool
            .call(json!({
                "agent_id": "not-a-uuid"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid agent_id format"));
    }

    #[tokio::test]
    async fn test_status_missing_agent_id() {
        let state = create_test_state();
        let tool = AgentStatusTool::new(state);

        let result = tool.call(json!({})).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid parameters"));
    }

    #[test]
    fn test_parse_agent_id_valid() {
        let uuid = "019467d9-7e3a-7000-8000-000000000000";
        let result = AgentStatusTool::parse_agent_id(uuid);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_agent_id_invalid() {
        let result = AgentStatusTool::parse_agent_id("invalid");
        assert!(result.is_err());
    }
}

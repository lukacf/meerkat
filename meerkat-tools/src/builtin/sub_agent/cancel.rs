//! agent_cancel tool - Cancel a running sub-agent

use super::state::SubAgentToolState;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::ops::{OperationId, SubAgentState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

/// Parameters for agent_cancel tool
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CancelParams {
    /// Sub-agent ID to cancel
    #[schemars(description = "The unique identifier of the sub-agent to cancel (UUID format)")]
    agent_id: String,
}

/// Response from agent_cancel tool
#[derive(Debug, Serialize)]
struct CancelResponse {
    /// Sub-agent ID
    agent_id: String,
    /// Whether cancellation was successful
    success: bool,
    /// Previous state before cancellation
    previous_state: String,
    /// Message describing the result
    message: String,
}

/// Tool for cancelling a running sub-agent
pub struct AgentCancelTool {
    state: Arc<SubAgentToolState>,
}

impl AgentCancelTool {
    /// Create a new agent_cancel tool
    pub fn new(state: Arc<SubAgentToolState>) -> Self {
        Self { state }
    }

    fn parse_agent_id(id_str: &str) -> Result<OperationId, BuiltinToolError> {
        Uuid::parse_str(id_str)
            .map(OperationId)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid agent_id format: {}", e)))
    }

    async fn cancel_agent(&self, params: CancelParams) -> Result<CancelResponse, BuiltinToolError> {
        let op_id = Self::parse_agent_id(&params.agent_id)?;

        // Get current state
        let state = self.state.manager.get_state(&op_id).await.ok_or_else(|| {
            BuiltinToolError::execution_failed(format!("Sub-agent not found: {}", params.agent_id))
        })?;

        let previous_state = match state {
            SubAgentState::Running => "running",
            SubAgentState::Completed => "completed",
            SubAgentState::Failed => "failed",
            SubAgentState::Cancelled => "cancelled",
        };

        // Can only cancel running agents
        if state != SubAgentState::Running {
            return Ok(CancelResponse {
                agent_id: params.agent_id,
                success: false,
                previous_state: previous_state.to_string(),
                message: format!(
                    "Cannot cancel sub-agent: already {} (only running agents can be cancelled)",
                    previous_state
                ),
            });
        }

        // Perform cancellation
        self.state.manager.cancel(&op_id).await;

        Ok(CancelResponse {
            agent_id: params.agent_id,
            success: true,
            previous_state: previous_state.to_string(),
            message: "Sub-agent cancelled successfully".to_string(),
        })
    }
}

#[async_trait]
impl BuiltinTool for AgentCancelTool {
    fn name(&self) -> &'static str {
        "agent_cancel"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "agent_cancel".into(),
            description: "Cancel a running sub-agent. Only agents in the 'running' state can be cancelled. Completed, failed, or already cancelled agents will return an error.".into(),
            input_schema: crate::schema::schema_for::<CancelParams>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Sub-agent tools are disabled by default
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: CancelParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid parameters: {}", e)))?;

        let response = self.cancel_agent(params).await?;
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
    use meerkat_core::{AgentSessionStore, AgentToolDispatcher};
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
        async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
            Err(ToolError::not_found("mock"))
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
        let tool = AgentCancelTool::new(state);
        assert_eq!(tool.name(), "agent_cancel");
    }

    #[test]
    fn test_tool_def() {
        let state = create_test_state();
        let tool = AgentCancelTool::new(state);
        let def = tool.def();

        assert_eq!(def.name, "agent_cancel");
        assert!(def.description.contains("Cancel"));

        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent_id"].is_object());
        assert_eq!(schema["required"], json!(["agent_id"]));
    }

    #[test]
    fn test_default_disabled() {
        let state = create_test_state();
        let tool = AgentCancelTool::new(state);
        assert!(!tool.default_enabled());
    }

    #[tokio::test]
    async fn test_cancel_not_found() {
        let state = create_test_state();
        let tool = AgentCancelTool::new(state);

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
    async fn test_cancel_running_agent() {
        let state = create_test_state();

        // Register an agent
        let op_id = meerkat_core::ops::OperationId::new();
        state
            .manager
            .register(op_id.clone(), "test-agent".to_string())
            .await
            .unwrap();

        let tool = AgentCancelTool::new(state.clone());
        let result = tool
            .call(json!({
                "agent_id": op_id.to_string()
            }))
            .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response["success"], true);
        assert_eq!(response["previous_state"], "running");
        assert!(
            response["message"]
                .as_str()
                .unwrap()
                .contains("successfully")
        );

        // Verify state changed to cancelled
        let new_state = state.manager.get_state(&op_id).await;
        assert_eq!(new_state, Some(SubAgentState::Cancelled));
    }

    #[tokio::test]
    async fn test_cancel_already_cancelled() {
        let state = create_test_state();

        // Register and cancel an agent
        let op_id = meerkat_core::ops::OperationId::new();
        state
            .manager
            .register(op_id.clone(), "test-agent".to_string())
            .await
            .unwrap();
        state.manager.cancel(&op_id).await;

        let tool = AgentCancelTool::new(state);
        let result = tool
            .call(json!({
                "agent_id": op_id.to_string()
            }))
            .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response["success"], false);
        assert_eq!(response["previous_state"], "cancelled");
    }

    #[tokio::test]
    async fn test_cancel_invalid_uuid() {
        let state = create_test_state();
        let tool = AgentCancelTool::new(state);

        let result = tool
            .call(json!({
                "agent_id": "not-a-uuid"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid agent_id format"));
    }
}

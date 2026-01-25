//! agent_steer tool - Send guidance message to running sub-agent

use super::state::SubAgentToolState;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::ops::OperationId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

/// Parameters for agent_steer tool
#[derive(Debug, Deserialize)]
struct SteerParams {
    /// Sub-agent ID to send steering to
    agent_id: String,
    /// Guidance message to send
    message: String,
}

/// Response from agent_steer tool
#[derive(Debug, Serialize)]
struct SteerResponse {
    /// Sub-agent ID
    agent_id: String,
    /// Whether steering was successfully queued
    success: bool,
    /// Steering message ID
    steering_id: String,
    /// Sequence number of this steering message
    seq: u64,
    /// Current status of the steering message
    status: String,
    /// Additional message
    #[serde(skip_serializing_if = "Option::is_none")]
    info: Option<String>,
}

/// Tool for sending guidance messages to running sub-agents
pub struct AgentSteerTool {
    state: Arc<SubAgentToolState>,
}

impl AgentSteerTool {
    /// Create a new agent_steer tool
    pub fn new(state: Arc<SubAgentToolState>) -> Self {
        Self { state }
    }

    fn parse_agent_id(id_str: &str) -> Result<OperationId, BuiltinToolError> {
        Uuid::parse_str(id_str)
            .map(OperationId)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid agent_id format: {}", e)))
    }

    async fn steer_agent(&self, params: SteerParams) -> Result<SteerResponse, BuiltinToolError> {
        let op_id = Self::parse_agent_id(&params.agent_id)?;

        // Validate message is not empty
        if params.message.trim().is_empty() {
            return Err(BuiltinToolError::invalid_args(
                "Steering message cannot be empty",
            ));
        }

        // Send steering message
        let handle = self
            .state
            .manager
            .steer(&op_id, params.message)
            .await
            .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;

        let status = match handle.status {
            meerkat_core::ops::SteeringStatus::Queued => "queued",
            meerkat_core::ops::SteeringStatus::Applied => "applied",
            meerkat_core::ops::SteeringStatus::Expired => "expired",
            meerkat_core::ops::SteeringStatus::Failed => "failed",
        };

        Ok(SteerResponse {
            agent_id: params.agent_id,
            success: true,
            steering_id: handle.id.to_string(),
            seq: handle.seq,
            status: status.to_string(),
            info: Some("Steering message queued for delivery at next turn boundary".to_string()),
        })
    }
}

#[async_trait]
impl BuiltinTool for AgentSteerTool {
    fn name(&self) -> &'static str {
        "agent_steer"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "agent_steer".to_string(),
            description: "Send a guidance message to a running sub-agent. The message will be injected into the sub-agent's context at the next turn boundary. Only works for agents in the 'running' state.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "description": "The unique identifier of the sub-agent (UUID format)"
                    },
                    "message": {
                        "type": "string",
                        "description": "The guidance message to send to the sub-agent. This will appear in the sub-agent's context as a steering instruction."
                    }
                },
                "required": ["agent_id", "message"]
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Sub-agent tools are disabled by default
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: SteerParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid parameters: {}", e)))?;

        let response = self.steer_agent(params).await?;
        serde_json::to_value(response).map_err(|e| {
            BuiltinToolError::execution_failed(format!("Failed to serialize response: {}", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::sub_agent::config::SubAgentConfig;
    use meerkat_client::{FactoryError, LlmClient, LlmClientFactory, LlmProvider};
    use meerkat_core::error::{AgentError, ToolError};
    use meerkat_core::ops::ConcurrencyLimits;
    use meerkat_core::session::Session;
    use meerkat_core::sub_agent::SubAgentManager;
    use meerkat_core::{AgentSessionStore, AgentToolDispatcher};
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
        fn tools(&self) -> Vec<ToolDef> {
            vec![]
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
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
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
        let tool = AgentSteerTool::new(state);
        assert_eq!(tool.name(), "agent_steer");
    }

    #[test]
    fn test_tool_def() {
        let state = create_test_state();
        let tool = AgentSteerTool::new(state);
        let def = tool.def();

        assert_eq!(def.name, "agent_steer");
        assert!(def.description.contains("guidance"));

        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent_id"].is_object());
        assert!(schema["properties"]["message"].is_object());
        assert_eq!(schema["required"], json!(["agent_id", "message"]));
    }

    #[test]
    fn test_default_disabled() {
        let state = create_test_state();
        let tool = AgentSteerTool::new(state);
        assert!(!tool.default_enabled());
    }

    #[tokio::test]
    async fn test_steer_not_found() {
        let state = create_test_state();
        let tool = AgentSteerTool::new(state);

        let result = tool
            .call(json!({
                "agent_id": "019467d9-7e3a-7000-8000-000000000000",
                "message": "Focus on security"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("SubAgentNotFound")
        );
    }

    #[tokio::test]
    async fn test_steer_running_agent() {
        let state = create_test_state();

        // Register an agent
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let op_id = meerkat_core::ops::OperationId::new();
        state
            .manager
            .register(op_id.clone(), "test-agent".to_string(), tx)
            .await
            .unwrap();

        let tool = AgentSteerTool::new(state);
        let result = tool
            .call(json!({
                "agent_id": op_id.to_string(),
                "message": "Focus on security aspects"
            }))
            .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response["success"], true);
        assert_eq!(response["status"], "queued");
        assert!(response["steering_id"].is_string());
    }

    #[tokio::test]
    async fn test_steer_empty_message() {
        let state = create_test_state();

        // Register an agent
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        let op_id = meerkat_core::ops::OperationId::new();
        state
            .manager
            .register(op_id.clone(), "test-agent".to_string(), tx)
            .await
            .unwrap();

        let tool = AgentSteerTool::new(state);
        let result = tool
            .call(json!({
                "agent_id": op_id.to_string(),
                "message": "   "
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[tokio::test]
    async fn test_steer_invalid_uuid() {
        let state = create_test_state();
        let tool = AgentSteerTool::new(state);

        let result = tool
            .call(json!({
                "agent_id": "not-a-uuid",
                "message": "Test"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid agent_id format"));
    }

    #[tokio::test]
    async fn test_steer_missing_message() {
        let state = create_test_state();
        let tool = AgentSteerTool::new(state);

        let result = tool
            .call(json!({
                "agent_id": "019467d9-7e3a-7000-8000-000000000000"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid parameters"));
    }
}

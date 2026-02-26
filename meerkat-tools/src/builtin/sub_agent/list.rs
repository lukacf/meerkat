//! agent_list tool - List all sub-agents and their states

use super::state::SubAgentToolState;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::ops::SubAgentState;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// Information about a single sub-agent (view for serialization)
#[derive(Debug, Serialize)]
struct SubAgentInfoView {
    /// Sub-agent ID
    id: String,
    /// Name/identifier
    name: String,
    /// Current state
    state: String,
    /// Nesting depth
    depth: u32,
    /// Duration running in milliseconds
    running_ms: u64,
}

/// Response from agent_list tool
#[derive(Debug, Serialize)]
struct ListResponse {
    /// List of all sub-agents
    agents: Vec<SubAgentInfoView>,
    /// Count of running agents
    running_count: usize,
    /// Count of completed agents
    completed_count: usize,
    /// Count of failed agents
    failed_count: usize,
    /// Total count
    total_count: usize,
}

/// Tool for listing all sub-agents and their states
pub struct AgentListTool {
    state: Arc<SubAgentToolState>,
}

impl AgentListTool {
    /// Create a new agent_list tool
    pub fn new(state: Arc<SubAgentToolState>) -> Self {
        Self { state }
    }

    async fn list_agents(&self) -> Result<ListResponse, BuiltinToolError> {
        let infos = self.state.manager.list_agents().await;

        let mut agent_list = Vec::new();
        let mut running_count = 0;
        let mut completed_count = 0;
        let mut failed_count = 0;

        for info in infos {
            let state_str = match info.state {
                SubAgentState::Running => {
                    running_count += 1;
                    "running"
                }
                SubAgentState::Completed => {
                    completed_count += 1;
                    "completed"
                }
                SubAgentState::Failed => {
                    failed_count += 1;
                    "failed"
                }
                SubAgentState::Cancelled => {
                    failed_count += 1; // Count cancelled with failed
                    "cancelled"
                }
            };

            agent_list.push(SubAgentInfoView {
                id: info.id.to_string(),
                name: info.name,
                state: state_str.to_string(),
                depth: info.depth,
                running_ms: info.running_ms,
            });
        }

        // Sort by start time (ID contains timestamp in UUID v7)
        agent_list.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(ListResponse {
            total_count: agent_list.len(),
            agents: agent_list,
            running_count,
            completed_count,
            failed_count,
        })
    }
}

#[async_trait]
impl BuiltinTool for AgentListTool {
    fn name(&self) -> &'static str {
        "agent_list"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "agent_list".into(),
            description: "List all sub-agents spawned by this agent with their current states. Shows running, completed, and failed agents with their IDs, names, and execution times.".into(),
            input_schema: crate::schema::empty_object_schema(),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Sub-agent tools are disabled by default
    }

    async fn call(&self, _args: Value) -> Result<Value, BuiltinToolError> {
        let response = self.list_agents().await?;
        serde_json::to_value(response).map_err(|e| {
            BuiltinToolError::execution_failed(format!("Failed to serialize response: {e}"))
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
        let tool = AgentListTool::new(state);
        assert_eq!(tool.name(), "agent_list");
    }

    #[test]
    fn test_tool_def() {
        let state = create_test_state();
        let tool = AgentListTool::new(state);
        let def = tool.def();

        assert_eq!(def.name, "agent_list");
        assert!(def.description.contains("List"));

        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], serde_json::json!([]));
    }

    #[test]
    fn test_default_disabled() {
        let state = create_test_state();
        let tool = AgentListTool::new(state);
        assert!(!tool.default_enabled());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let state = create_test_state();
        let tool = AgentListTool::new(state);

        let result = tool.call(json!({})).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response["agents"], json!([]));
        assert_eq!(response["total_count"], 0);
        assert_eq!(response["running_count"], 0);
        assert_eq!(response["completed_count"], 0);
        assert_eq!(response["failed_count"], 0);
    }

    #[tokio::test]
    async fn test_list_with_registered_agent() {
        let state = create_test_state();

        // Register an agent
        let op_id = meerkat_core::ops::OperationId::new();
        state
            .manager
            .register(op_id.clone(), "test-agent".to_string())
            .await
            .unwrap();

        let tool = AgentListTool::new(state);
        let result = tool.call(json!({})).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response["total_count"], 1);
        assert_eq!(response["running_count"], 1);
        assert_eq!(response["agents"][0]["name"], "test-agent");
        assert_eq!(response["agents"][0]["state"], "running");
    }
}

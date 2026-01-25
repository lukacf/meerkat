//! SubAgentToolSet - bundles all sub-agent tools together

use super::cancel::AgentCancelTool;
use super::fork::AgentForkTool;
use super::list::AgentListTool;
use super::spawn::AgentSpawnTool;
use super::state::SubAgentToolState;
use super::status::AgentStatusTool;
use super::steer::AgentSteerTool;
use crate::builtin::BuiltinTool;
use std::sync::Arc;

/// A set of all sub-agent tools that share common state
///
/// This struct bundles all sub-agent tools together, allowing them to share
/// the same `SubAgentToolState` for coordinated sub-agent management.
pub struct SubAgentToolSet {
    /// Tool for spawning new sub-agents with clean context
    pub spawn: AgentSpawnTool,
    /// Tool for forking with continued context
    pub fork: AgentForkTool,
    /// Tool for sending guidance to running sub-agents
    pub steer: AgentSteerTool,
    /// Tool for checking sub-agent status
    pub status: AgentStatusTool,
    /// Tool for cancelling running sub-agents
    pub cancel: AgentCancelTool,
    /// Tool for listing all sub-agents
    pub list: AgentListTool,
    /// Shared state for all tools
    state: Arc<SubAgentToolState>,
}

impl SubAgentToolSet {
    /// Create a new SubAgentToolSet with the given shared state
    pub fn new(state: Arc<SubAgentToolState>) -> Self {
        Self {
            spawn: AgentSpawnTool::new(state.clone()),
            fork: AgentForkTool::new(state.clone()),
            steer: AgentSteerTool::new(state.clone()),
            status: AgentStatusTool::new(state.clone()),
            cancel: AgentCancelTool::new(state.clone()),
            list: AgentListTool::new(state.clone()),
            state,
        }
    }

    /// Get references to all tools in this set
    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![
            &self.spawn as &dyn BuiltinTool,
            &self.fork as &dyn BuiltinTool,
            &self.steer as &dyn BuiltinTool,
            &self.status as &dyn BuiltinTool,
            &self.cancel as &dyn BuiltinTool,
            &self.list as &dyn BuiltinTool,
        ]
    }

    /// Get the shared state
    pub fn state(&self) -> &Arc<SubAgentToolState> {
        &self.state
    }

    /// Get tool names in this set
    pub fn tool_names(&self) -> Vec<&'static str> {
        vec![
            "agent_spawn",
            "agent_fork",
            "agent_steer",
            "agent_status",
            "agent_cancel",
            "agent_list",
        ]
    }
}

impl std::fmt::Debug for SubAgentToolSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentToolSet")
            .field("tool_names", &self.tool_names())
            .field("state", &self.state)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::sub_agent::config::SubAgentConfig;
    use async_trait::async_trait;
    use meerkat_client::{FactoryError, LlmClient, LlmClientFactory, LlmProvider};
    use meerkat_core::error::{AgentError, ToolError};
    use meerkat_core::ops::ConcurrencyLimits;
    use meerkat_core::session::Session;
    use meerkat_core::sub_agent::SubAgentManager;
    use meerkat_core::{AgentSessionStore, AgentToolDispatcher, ToolDef};
    use serde_json::Value;
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
    fn test_tool_set_creation() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);

        assert_eq!(tool_set.spawn.name(), "agent_spawn");
        assert_eq!(tool_set.fork.name(), "agent_fork");
        assert_eq!(tool_set.steer.name(), "agent_steer");
        assert_eq!(tool_set.status.name(), "agent_status");
        assert_eq!(tool_set.cancel.name(), "agent_cancel");
        assert_eq!(tool_set.list.name(), "agent_list");
    }

    #[test]
    fn test_tool_set_tools() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let tools = tool_set.tools();

        assert_eq!(tools.len(), 6);

        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"agent_spawn"));
        assert!(names.contains(&"agent_fork"));
        assert!(names.contains(&"agent_steer"));
        assert!(names.contains(&"agent_status"));
        assert!(names.contains(&"agent_cancel"));
        assert!(names.contains(&"agent_list"));
    }

    #[test]
    fn test_tool_set_tool_names() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let names = tool_set.tool_names();

        assert_eq!(names.len(), 6);
        assert!(names.contains(&"agent_spawn"));
        assert!(names.contains(&"agent_fork"));
        assert!(names.contains(&"agent_steer"));
        assert!(names.contains(&"agent_status"));
        assert!(names.contains(&"agent_cancel"));
        assert!(names.contains(&"agent_list"));
    }

    #[test]
    fn test_tool_set_shared_state() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state.clone());

        // Verify state is shared
        assert!(Arc::ptr_eq(&state, tool_set.state()));
    }

    #[test]
    fn test_tool_set_all_disabled_by_default() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let tools = tool_set.tools();

        for tool in tools {
            assert!(
                !tool.default_enabled(),
                "Tool {} should be disabled by default",
                tool.name()
            );
        }
    }

    #[test]
    fn test_tool_set_debug() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let debug = format!("{:?}", tool_set);

        assert!(debug.contains("SubAgentToolSet"));
        assert!(debug.contains("agent_spawn"));
    }

    #[test]
    fn test_tool_set_definitions_valid() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let tools = tool_set.tools();

        for tool in tools {
            let def = tool.def();

            // All definitions should have valid structure
            assert!(!def.name.is_empty(), "Tool {} has empty name", tool.name());
            assert!(
                !def.description.is_empty(),
                "Tool {} has empty description",
                tool.name()
            );
            assert!(
                def.input_schema.get("type").is_some(),
                "Tool {} has no type in schema",
                tool.name()
            );
        }
    }
}

//! SubAgentToolSet - bundles all sub-agent tools together

use super::cancel::AgentCancelTool;
use super::fork::AgentForkTool;
use super::list::AgentListTool;
use super::spawn::AgentSpawnTool;
use super::state::SubAgentToolState;
use super::status::AgentStatusTool;
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
            "agent_status",
            "agent_cancel",
            "agent_list",
        ]
    }

    /// Get usage instructions for the LLM on how to properly use sub-agent tools
    ///
    /// These instructions should be injected into the system prompt when
    /// sub-agent tools are enabled.
    pub fn usage_instructions() -> &'static str {
        r#"# Sub-Agent Tools

You have access to tools for spawning and managing sub-agents. Sub-agents are independent LLM agents that can work in parallel on subtasks.

## Available Tools
- `agent_spawn` - Create a sub-agent with clean context (just your prompt)
- `agent_fork` - Create a sub-agent that inherits your full conversation history
- `agent_status` - Check the status and output of a sub-agent
- `agent_cancel` - Cancel a running sub-agent
- `agent_list` - List all sub-agents and their states

## Communicating with Sub-Agents
To send messages to running sub-agents, use the `comms_send` tool. Sub-agents automatically
trust their parent and can receive messages at turn boundaries.

## Best Practices

### Spawning Sub-Agents
- Give sub-agents clear, self-contained tasks with all necessary context in the prompt
- Sub-agents inherit your tools by default (tool_access policy can restrict this)
- Use different providers/models for different strengths (e.g., gemini-3-pro-preview for analysis, gpt-5.2 for coding)
- Set appropriate budgets (max_turns, max_tokens, max_tool_calls) to prevent runaway costs

### Monitoring Sub-Agents
- **CRITICAL: DO NOT poll agent_status repeatedly** - this wastes tokens and provides no benefit
- Sub-agents run asynchronously and take time to complete (typically 10-120 seconds)
- **Instead of polling**: do other useful work, then check status once or twice near the end
- Use `agent_list` to see all sub-agents at once - much more efficient than individual status checks
- When `is_final: true`, the sub-agent is done and `output` contains the result
- If you find yourself checking status more than 3 times total per agent, you're polling too much

### When to Use Sub-Agents
- Parallel independent tasks (e.g., analyze multiple files simultaneously)
- Tasks requiring different model strengths
- Long-running work you want to delegate while doing other things
- Breaking complex problems into specialized subtasks

### When NOT to Use Sub-Agents
- Simple tasks you can do directly
- Tasks requiring tight coordination (use sequential steps instead)
- When you need immediate results (sub-agents add latency)"#
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
    fn test_tool_set_creation() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);

        assert_eq!(tool_set.spawn.name(), "agent_spawn");
        assert_eq!(tool_set.fork.name(), "agent_fork");
        assert_eq!(tool_set.status.name(), "agent_status");
        assert_eq!(tool_set.cancel.name(), "agent_cancel");
        assert_eq!(tool_set.list.name(), "agent_list");
    }

    #[test]
    fn test_tool_set_tools() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let tools = tool_set.tools();

        assert_eq!(tools.len(), 5);

        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"agent_spawn"));
        assert!(names.contains(&"agent_fork"));
        assert!(names.contains(&"agent_status"));
        assert!(names.contains(&"agent_cancel"));
        assert!(names.contains(&"agent_list"));
    }

    #[test]
    fn test_tool_set_tool_names() {
        let state = create_test_state();
        let tool_set = SubAgentToolSet::new(state);
        let names = tool_set.tool_names();

        assert_eq!(names.len(), 5);
        assert!(names.contains(&"agent_spawn"));
        assert!(names.contains(&"agent_fork"));
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

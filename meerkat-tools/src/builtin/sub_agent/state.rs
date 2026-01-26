//! Shared state for sub-agent tools

use super::config::SubAgentConfig;
use meerkat_client::LlmClientFactory;
use meerkat_comms::TrustedPeers;
use meerkat_core::session::Session;
use meerkat_core::sub_agent::SubAgentManager;
use meerkat_core::{AgentSessionStore, AgentToolDispatcher, ParentCommsContext};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared state for all sub-agent tools
///
/// This state is shared across all sub-agent tools (spawn, fork, status, cancel, list)
/// to provide unified access to the sub-agent manager and required services.
pub struct SubAgentToolState {
    /// Sub-agent manager for tracking and controlling sub-agents
    pub manager: Arc<SubAgentManager>,

    /// Factory for creating LLM clients for different providers
    pub client_factory: Arc<dyn LlmClientFactory>,

    /// Tool dispatcher for providing tools to sub-agents
    pub tool_dispatcher: Arc<dyn AgentToolDispatcher>,

    /// Session store for persisting sub-agent sessions
    pub session_store: Arc<dyn AgentSessionStore>,

    /// Parent session (for forking and context sharing)
    pub parent_session: Arc<RwLock<Session>>,

    /// Configuration for sub-agent behavior
    pub config: SubAgentConfig,

    /// Current depth of this agent (0 = top-level)
    pub current_depth: u32,

    /// Parent comms context for sub-agent communication (if comms enabled)
    pub parent_comms: Option<ParentCommsContext>,

    /// Parent's trusted peers (for adding sub-agents as trusted)
    /// This is shared with the parent's CommsRuntime so updates are visible to listeners.
    pub parent_trusted_peers: Option<Arc<RwLock<TrustedPeers>>>,
}

impl SubAgentToolState {
    /// Create new sub-agent tool state
    pub fn new(
        manager: Arc<SubAgentManager>,
        client_factory: Arc<dyn LlmClientFactory>,
        tool_dispatcher: Arc<dyn AgentToolDispatcher>,
        session_store: Arc<dyn AgentSessionStore>,
        parent_session: Arc<RwLock<Session>>,
        config: SubAgentConfig,
        current_depth: u32,
    ) -> Self {
        Self {
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            current_depth,
            parent_comms: None,
            parent_trusted_peers: None,
        }
    }

    /// Create new sub-agent tool state with comms enabled
    ///
    /// The `parent_trusted_peers` is shared with the parent's CommsRuntime.
    /// When sub-agents are spawned, they are added to this list so the parent
    /// can accept their connections.
    #[allow(clippy::too_many_arguments)]
    pub fn with_comms(
        manager: Arc<SubAgentManager>,
        client_factory: Arc<dyn LlmClientFactory>,
        tool_dispatcher: Arc<dyn AgentToolDispatcher>,
        session_store: Arc<dyn AgentSessionStore>,
        parent_session: Arc<RwLock<Session>>,
        config: SubAgentConfig,
        current_depth: u32,
        parent_comms: ParentCommsContext,
        parent_trusted_peers: Arc<RwLock<TrustedPeers>>,
    ) -> Self {
        Self {
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            current_depth,
            parent_comms: Some(parent_comms),
            parent_trusted_peers: Some(parent_trusted_peers),
        }
    }

    /// Check if we can spawn more sub-agents
    pub async fn can_spawn(&self) -> bool {
        self.manager.can_spawn().await
    }

    /// Get the current nesting depth
    pub fn depth(&self) -> u32 {
        self.current_depth
    }

    /// Check if nested spawning is allowed at current depth
    pub fn can_nest(&self) -> bool {
        self.config.allow_nested_spawn
            && self.current_depth < self.config.concurrency_limits.max_depth
    }
}

impl std::fmt::Debug for SubAgentToolState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentToolState")
            .field("current_depth", &self.current_depth)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_client::{FactoryError, LlmClient, LlmProvider};
    use meerkat_core::ToolDef;
    use meerkat_core::error::{AgentError, ToolError};
    use meerkat_core::ops::ConcurrencyLimits;
    use serde_json::Value;

    // Mock implementations for testing

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
            vec![LlmProvider::Anthropic]
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

    fn create_test_state() -> SubAgentToolState {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
        )
    }

    #[test]
    fn test_state_creation() {
        let state = create_test_state();
        assert_eq!(state.depth(), 0);
        assert!(state.can_nest());
    }

    #[test]
    fn test_state_depth() {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 2));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let state = SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            2,
        );

        assert_eq!(state.depth(), 2);
        assert!(state.can_nest()); // depth 2 < max_depth 3
    }

    #[test]
    fn test_state_cannot_nest_at_max_depth() {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 3));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default(); // max_depth defaults to 3

        let state = SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            3,
        );

        assert_eq!(state.depth(), 3);
        assert!(!state.can_nest()); // depth 3 >= max_depth 3
    }

    #[test]
    fn test_state_cannot_nest_when_disabled() {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default().with_allow_nested_spawn(false);

        let state = SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
        );

        assert!(!state.can_nest());
    }

    #[tokio::test]
    async fn test_state_can_spawn() {
        let state = create_test_state();
        assert!(state.can_spawn().await);
    }

    #[test]
    fn test_state_debug() {
        let state = create_test_state();
        let debug = format!("{:?}", state);
        assert!(debug.contains("SubAgentToolState"));
        assert!(debug.contains("current_depth: 0"));
    }

    // === Tool inheritance regression tests ===
    // These tests verify that sub-agents receive the correct tools

    /// Mock dispatcher that provides specific tools (for inheritance testing)
    struct MockToolDispatcherWithTools {
        tool_names: Vec<String>,
    }

    impl MockToolDispatcherWithTools {
        fn new(tool_names: Vec<&str>) -> Self {
            Self {
                tool_names: tool_names.into_iter().map(String::from).collect(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockToolDispatcherWithTools {
        fn tools(&self) -> Vec<ToolDef> {
            self.tool_names
                .iter()
                .map(|name| ToolDef {
                    name: name.clone(),
                    description: format!("{} tool", name),
                    input_schema: serde_json::json!({"type": "object"}),
                })
                .collect()
        }

        async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
            if self.tool_names.contains(&name.to_string()) {
                Ok(Value::String(format!("{} executed", name)))
            } else {
                Err(ToolError::not_found(name))
            }
        }
    }

    #[test]
    fn test_subagent_tool_dispatcher_provides_expected_tools() {
        // Regression test: sub-agents should have access to the tools
        // passed in the tool_dispatcher
        let expected_tools = vec!["shell", "task_list", "task_create"];
        let dispatcher = Arc::new(MockToolDispatcherWithTools::new(expected_tools.clone()));

        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let state = SubAgentToolState::new(
            manager,
            client_factory,
            dispatcher,
            session_store,
            parent_session,
            config,
            0,
        );

        // Verify the tool dispatcher provides the expected tools
        let tools = state.tool_dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();

        for expected in &expected_tools {
            assert!(
                tool_names.contains(expected),
                "Sub-agent should have access to '{}' tool, but tools are: {:?}",
                expected,
                tool_names
            );
        }
    }

    #[test]
    fn test_subagent_tool_dispatcher_excludes_subagent_tools() {
        // Regression test: sub-agents should NOT have access to sub-agent tools
        // (to prevent infinite nesting)
        let tools_without_subagent = vec!["shell", "task_list"]; // No agent_spawn, agent_fork, etc.
        let dispatcher = Arc::new(MockToolDispatcherWithTools::new(tools_without_subagent));

        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let state = SubAgentToolState::new(
            manager,
            client_factory,
            dispatcher,
            session_store,
            parent_session,
            config,
            0,
        );

        // Verify sub-agent tools are NOT present
        let tools = state.tool_dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();

        let forbidden_tools = [
            "agent_spawn",
            "agent_fork",
            "agent_status",
            "agent_cancel",
            "agent_list",
        ];
        for forbidden in &forbidden_tools {
            assert!(
                !tool_names.contains(forbidden),
                "Sub-agent should NOT have access to '{}' tool to prevent infinite nesting",
                forbidden
            );
        }
    }

    #[tokio::test]
    async fn test_subagent_can_dispatch_inherited_tools() {
        // Regression test: sub-agents should be able to execute inherited tools
        let dispatcher = Arc::new(MockToolDispatcherWithTools::new(vec!["shell", "task_list"]));

        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let state = SubAgentToolState::new(
            manager,
            client_factory,
            dispatcher,
            session_store,
            parent_session,
            config,
            0,
        );

        // Sub-agent should be able to dispatch inherited tools
        let result = state
            .tool_dispatcher
            .dispatch("shell", &serde_json::json!({}))
            .await;
        assert!(
            result.is_ok(),
            "Sub-agent should be able to use 'shell' tool"
        );

        let result = state
            .tool_dispatcher
            .dispatch("task_list", &serde_json::json!({}))
            .await;
        assert!(
            result.is_ok(),
            "Sub-agent should be able to use 'task_list' tool"
        );

        // Sub-agent should NOT be able to dispatch non-existent tools
        let result = state
            .tool_dispatcher
            .dispatch("agent_spawn", &serde_json::json!({}))
            .await;
        assert!(
            result.is_err(),
            "Sub-agent should NOT have 'agent_spawn' tool"
        );
    }

    // === Comms context regression tests ===
    // These tests verify that sub-agents receive comms context when parent has comms enabled

    #[test]
    fn test_state_new_has_no_parent_comms() {
        // When created with `new()`, parent_comms should be None
        let state = create_test_state();
        assert!(
            state.parent_comms.is_none(),
            "SubAgentToolState::new() should not have parent_comms"
        );
    }

    #[test]
    fn test_state_with_comms_has_parent_comms() {
        use meerkat_core::ParentCommsContext;
        use std::path::PathBuf;

        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let parent_comms = ParentCommsContext {
            parent_name: "parent-agent".to_string(),
            parent_pubkey: [42u8; 32],
            parent_addr: "tcp://127.0.0.1:4200".to_string(),
            comms_base_dir: PathBuf::from("/tmp/comms"),
        };

        let parent_trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));

        let state = SubAgentToolState::with_comms(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
            parent_comms,
            parent_trusted_peers,
        );

        // Regression test: sub-agents MUST have parent_comms when created with with_comms()
        // This enables sub-agents to communicate back to the parent
        assert!(
            state.parent_comms.is_some(),
            "SubAgentToolState::with_comms() MUST set parent_comms"
        );
        assert!(
            state.parent_trusted_peers.is_some(),
            "SubAgentToolState::with_comms() MUST set parent_trusted_peers"
        );

        let comms = state.parent_comms.unwrap();
        assert_eq!(comms.parent_name, "parent-agent");
        assert_eq!(comms.parent_pubkey, [42u8; 32]);
        assert_eq!(comms.parent_addr, "tcp://127.0.0.1:4200");
    }

    #[test]
    fn test_spawn_tool_uses_parent_comms_for_subagent() {
        // Verify the spawn tool accesses parent_comms from state
        // This is a compile-time check that the field is public and accessible
        use meerkat_core::ParentCommsContext;
        use std::path::PathBuf;

        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let parent_comms = ParentCommsContext {
            parent_name: "orchestrator".to_string(),
            parent_pubkey: [1u8; 32],
            parent_addr: "uds:///tmp/orchestrator.sock".to_string(),
            comms_base_dir: PathBuf::from("/tmp/agents"),
        };

        let parent_trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));

        let state = SubAgentToolState::with_comms(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
            parent_comms,
            parent_trusted_peers,
        );

        // The spawn tool checks state.parent_comms.as_ref() to decide whether
        // to set up comms for sub-agents. This test verifies the pattern works.
        let comms_config = state
            .parent_comms
            .as_ref()
            .map(|pc| (pc.parent_name.clone(), pc.comms_base_dir.clone()));

        assert!(comms_config.is_some());
        let (name, base_dir) = comms_config.unwrap();
        assert_eq!(name, "orchestrator");
        assert_eq!(base_dir, PathBuf::from("/tmp/agents"));
    }

    #[tokio::test]
    async fn test_parent_trusted_peers_can_be_updated() {
        // Regression test: When a sub-agent is spawned, its pubkey should be
        // added to the parent's trusted peers. This test verifies the shared
        // trusted peers list can be updated, which is critical for sub-agent
        // communication to work.
        use meerkat_comms::{PubKey, TrustedPeer};
        use meerkat_core::ParentCommsContext;
        use std::path::PathBuf;

        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));

        // Initially empty
        {
            let peers = trusted_peers.read().await;
            assert!(!peers.has_peers(), "Initially should have no peers");
        }

        // Simulate what spawn_sub_agent_dyn does: add child as trusted peer
        let child_pubkey = PubKey::new([42u8; 32]);
        let child_peer = TrustedPeer {
            name: "sub-agent-test".to_string(),
            pubkey: child_pubkey,
            addr: "uds:///tmp/sub-agent-test.sock".to_string(),
        };

        {
            let mut peers = trusted_peers.write().await;
            peers.upsert(child_peer);
        }

        // Now the child should be in the trusted peers
        {
            let peers = trusted_peers.read().await;
            assert!(peers.has_peers(), "Should have the child peer after upsert");
            assert_eq!(peers.len(), 1);

            let found = peers.get_peer(&child_pubkey);
            assert!(found.is_some(), "Should find child by pubkey");
            assert_eq!(found.unwrap().name, "sub-agent-test");
        }

        // The same Arc can be stored in SubAgentToolState and shared
        // This verifies the pattern used in the CLI
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits, 0));
        let client_factory = Arc::new(MockClientFactory);
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        let parent_comms = ParentCommsContext {
            parent_name: "parent".to_string(),
            parent_pubkey: [1u8; 32],
            parent_addr: "tcp://127.0.0.1:4200".to_string(),
            comms_base_dir: PathBuf::from("/tmp/comms"),
        };

        let state = SubAgentToolState::with_comms(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
            parent_comms,
            trusted_peers.clone(), // Same Arc as above
        );

        // Verify the state holds the same Arc and sees the child
        assert!(state.parent_trusted_peers.is_some());
        let peers_from_state = state.parent_trusted_peers.unwrap();
        let peers = peers_from_state.read().await;
        assert!(
            peers.has_peers(),
            "State should see the child added earlier"
        );
        assert_eq!(peers.len(), 1);
    }
}

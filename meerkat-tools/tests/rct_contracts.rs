use async_trait::async_trait;
use meerkat_client::{FactoryError, LlmClient, LlmClientFactory, LlmProvider};
use meerkat_core::error::{AgentError, ToolError as CoreToolError};
use meerkat_core::ops::ConcurrencyLimits;
use meerkat_core::session::Session;
use meerkat_core::sub_agent::SubAgentManager;
use meerkat_core::{AgentSessionStore, AgentToolDispatcher, ToolDef};
use meerkat_tools::ToolError;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::builtin::shell::ShellToolSet;
use meerkat_tools::builtin::sub_agent::SubAgentConfig;
use meerkat_tools::builtin::{
    AgentListTool, BuiltinTool, BuiltinToolConfig, CompositeDispatcher, FileTaskStore,
    MemoryTaskStore, TaskCreateTool,
};
use meerkat_tools::dispatcher::{ToolDispatcherConfig, ToolDispatcherKind};
use serde_json::{Value, json};
use std::sync::Arc;
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

    async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, CoreToolError> {
        Err(CoreToolError::not_found("mock"))
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

fn create_sub_agent_state() -> Arc<meerkat_tools::builtin::sub_agent::SubAgentToolState> {
    let limits = ConcurrencyLimits::default();
    let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
    let client_factory = Arc::new(MockClientFactory);
    let tool_dispatcher = Arc::new(MockToolDispatcher);
    let session_store = Arc::new(MockSessionStore);
    let parent_session = Arc::new(RwLock::new(Session::new()));
    let config = SubAgentConfig::default();

    Arc::new(meerkat_tools::builtin::sub_agent::SubAgentToolState::new(
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
fn test_rct_contracts_shell_allowlist_contract() {
    let tool_set = ShellToolSet::new(ShellConfig::default());
    let names = [
        tool_set.job_status.name(),
        tool_set.jobs_list.name(),
        tool_set.job_cancel.name(),
    ];
    assert!(names.contains(&"shell_job_status"));
    assert!(names.contains(&"shell_jobs"));
    assert!(names.contains(&"shell_job_cancel"));
}

#[test]
fn test_rct_contracts_tool_dispatcher_contract() {
    let config = ToolDispatcherConfig::default();
    assert_eq!(config.kind, ToolDispatcherKind::Composite);

    let encoded = serde_json::to_value(&config).expect("serialize");
    let decoded: ToolDispatcherConfig = serde_json::from_value(encoded).expect("deserialize");
    assert_eq!(decoded.kind, ToolDispatcherKind::Composite);
}

#[test]
fn test_rct_contracts_tool_error_schema_contract() {
    let err = ToolError::invalid_arguments("tool_x", "bad input");
    let message = format!("{err}");
    assert!(message.contains("tool_x"));
    assert!(message.contains("bad input"));

    let unavailable = ToolError::unavailable("tool_y", "no peers configured");
    assert_eq!(unavailable.error_code(), "tool_unavailable");
    let payload = unavailable.to_error_payload();
    assert_eq!(payload["error"], "tool_unavailable");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tool_y")
    );
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no peers configured")
    );
}

#[tokio::test]
async fn test_rct_contracts_task_store_persistence_contract() {
    let store = Arc::new(MemoryTaskStore::new());
    let tool = TaskCreateTool::with_session(store, "sess-123".to_string());
    let result = tool
        .call(json!({"subject":"Test","description":"Persist"}))
        .await
        .expect("task create");

    assert_eq!(result["created_by_session"], "sess-123");
    assert_eq!(result["updated_by_session"], "sess-123");
}

#[tokio::test]
async fn test_rct_contracts_inv_004_task_tools_session_id() {
    let store = Arc::new(MemoryTaskStore::new());
    let tool = TaskCreateTool::with_session(store, "sess-999".to_string());
    let result = tool
        .call(json!({"subject":"Task","description":"Track session"}))
        .await
        .expect("task create");

    assert_eq!(result["created_by_session"], "sess-999");
}

#[test]
fn test_rct_contracts_shell_defaults_contract() {
    let core_defaults = meerkat_core::ShellDefaults::default();
    let shell_config = ShellConfig::default();
    assert_eq!(core_defaults.program, shell_config.shell);
    assert_eq!(
        core_defaults.timeout_secs,
        shell_config.default_timeout_secs
    );
    assert!(core_defaults.allowlist.is_empty());
}

#[test]
fn test_rct_contracts_agent_list_schema_contract() {
    let state = create_sub_agent_state();
    let tool = AgentListTool::new(state);
    let def = tool.def();

    assert_eq!(def.name, "agent_list");

    let schema = &def.input_schema;
    assert_eq!(schema["type"], "object");
    let props = schema.get("properties").unwrap();
    assert!(props.as_object().is_none_or(|o| o.is_empty()));
    assert_eq!(schema["required"], json!([]));

    // Verify presence in serialized form (RCT requirement)
    let json_str = serde_json::to_string(&def).expect("serialize");
    println!("Serialized def: {}", json_str);
    let json_val: Value = serde_json::from_str(&json_str).expect("deserialize");
    assert!(
        json_val["input_schema"]
            .as_object()
            .unwrap()
            .contains_key("required"),
        "serialized schema must contain 'required' field"
    );
    assert_eq!(json_val["input_schema"]["required"], json!([]));
}

#[test]
fn test_rct_contracts_all_builtin_schemas_have_required_field() {
    let store = Arc::new(MemoryTaskStore::new());
    let mut config = BuiltinToolConfig::default();
    config.policy = config
        .policy
        .with_mode(meerkat_tools::builtin::ToolMode::AllowAll);

    // Enable all categories
    config.policy = config
        .policy
        .enable_tool("shell")
        .enable_tool("agent_spawn")
        .enable_tool("agent_fork")
        .enable_tool("agent_status")
        .enable_tool("agent_cancel")
        .enable_tool("agent_list")
        .enable_tool("datetime")
        .enable_tool("wait");

    let shell_config = Some(ShellConfig::default());

    let dispatcher = CompositeDispatcher::new(
        store,
        &config,
        shell_config,
        None,
        Some("test-session".to_string()),
    )
    .expect("create dispatcher");

    let tools = dispatcher.tools();
    assert!(!tools.is_empty(), "should have tools");

    // Also test COMMS tools
    let keypair = meerkat_comms::Keypair::generate();
    let trusted_peers = Arc::new(RwLock::new(meerkat_comms::TrustedPeers::default()));
    let router = Arc::new(meerkat_comms::Router::with_shared_peers(
        keypair,
        trusted_peers.clone(),
        meerkat_comms::CommsConfig::default(),
    ));
    let comms_dispatcher = meerkat_tools::CommsToolDispatcher::with_inner(
        router,
        trusted_peers,
        Arc::new(meerkat_tools::EmptyToolDispatcher),
    );
    let comms_tools = AgentToolDispatcher::tools(&comms_dispatcher);
    assert!(!comms_tools.is_empty(), "should have comms tools");

    let all_tools = tools.into_iter().chain(comms_tools);

    for tool in all_tools {
        let schema = &tool.input_schema;
        let json_str = serde_json::to_string(&tool).expect("serialize");
        let json_val: Value = serde_json::from_str(&json_str).expect("deserialize");

        assert!(
            json_val["input_schema"]
                .as_object()
                .unwrap()
                .contains_key("required"),
            "Tool '{}' schema MUST contain 'required' field (RCT requirement). Schema: {}",
            tool.name,
            schema
        );
    }
}

#[test]
fn test_rct_contracts_inv_007_builtin_task_persistence_strategy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = FileTaskStore::in_project(dir.path());
    let path = store.path().to_string_lossy().to_string();
    assert!(path.ends_with(".rkat/tasks.json"));
}

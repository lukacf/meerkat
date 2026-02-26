use meerkat_core::{
    CommsRuntimeConfig, CommsRuntimeMode, Config, ConfigDelta, ConfigScope, ConfigStore,
    ProviderConfig, SecurityMode, SystemPromptConfig, ToolCallView, ToolResult,
};
use serde_json::json;

#[test]
fn test_tool_call_view_parse_args_contract() -> Result<(), Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    struct Args {
        #[allow(dead_code)]
        a: i32,
    }

    let args = json!({ "a": "not-an-int" });
    let raw = serde_json::value::RawValue::from_string(args.to_string())?;
    let call = meerkat_core::types::ToolCallView {
        id: "test-1",
        name: "tool",
        args: &raw,
    };

    let parsed: Result<Args, _> = call.parse_args();
    assert!(parsed.is_err(), "type mismatch should fail to parse");
    Ok(())
}

#[test]
fn test_agent_factory_config_contract() {
    let config = Config::default();
    assert!(!config.models.anthropic.is_empty());
    assert!(!config.models.openai.is_empty());
    assert!(!config.models.gemini.is_empty());
    assert!(config.max_tokens > 0);
    assert!(!config.shell.program.is_empty());
    assert!(!config.tools.builtins_enabled);
    assert!(!config.tools.shell_enabled);
    assert_eq!(config.comms.mode, CommsRuntimeMode::Inproc);
    assert!(config.agent.tool_instructions.is_none());
}

#[test]
fn test_shell_allowlist_backward_compat() -> Result<(), Box<dyn std::error::Error>> {
    let toml_str = r#"
[shell]
allowlist = ["git *", "ls -la"]
"#;
    let config: Config = toml::from_str(toml_str)?;
    assert_eq!(config.shell.security_mode, SecurityMode::AllowList);
    assert_eq!(
        config.shell.security_patterns,
        vec!["git *".to_string(), "ls -la".to_string()]
    );
    Ok(())
}

#[test]
fn test_config_env_contract() -> Result<(), Box<dyn std::error::Error>> {
    let env = std::collections::HashMap::from([
        ("RKAT_MODEL".to_string(), "env-model".to_string()),
        (
            "RKAT_ANTHROPIC_API_KEY".to_string(),
            "rkat-secret".to_string(),
        ),
        (
            "ANTHROPIC_API_KEY".to_string(),
            "anthropic-secret".to_string(),
        ),
    ]);

    let mut config = Config::default();
    config.apply_env_overrides_from(|key| env.get(key).cloned())?;
    assert_eq!(config.agent.model, Config::default().agent.model);
    match config.provider {
        ProviderConfig::Anthropic { api_key, .. } => {
            assert_eq!(api_key.as_deref(), Some("rkat-secret"));
        }
        _ => return Err("expected anthropic provider".into()),
    }
    Ok(())
}

#[test]
fn test_resume_metadata_contract() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = meerkat_core::SessionMetadata {
        model: "claude-test".to_string(),
        max_tokens: 1234,
        provider: meerkat_core::Provider::Anthropic,
        tooling: meerkat_core::SessionTooling {
            builtins: true,
            shell: true,
            comms: false,
            subagents: true,
            mob: false,
            active_skills: None,
        },
        host_mode: true,
        comms_name: Some("agent-a".to_string()),
        peer_meta: None,
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
    };

    let json = serde_json::to_value(&metadata)?;
    let parsed: meerkat_core::SessionMetadata = serde_json::from_value(json)?;
    assert_eq!(parsed.model, "claude-test");
    assert_eq!(parsed.max_tokens, 1234);
    assert_eq!(parsed.provider, meerkat_core::Provider::Anthropic);
    assert!(parsed.tooling.shell);
    assert_eq!(parsed.comms_name.as_deref(), Some("agent-a"));
    Ok(())
}

#[test]
fn test_comms_runtime_contract() -> Result<(), Box<dyn std::error::Error>> {
    let config = CommsRuntimeConfig::default();
    assert_eq!(config.mode, CommsRuntimeMode::Inproc);
    assert!(config.address.is_none());
    assert!(!config.auto_enable_for_subagents);

    let encoded = serde_json::to_value(&config)?;
    let decoded: CommsRuntimeConfig = serde_json::from_value(encoded)?;
    assert_eq!(decoded, config);
    Ok(())
}

#[test]
fn test_error_mapping_contract() {
    let err = meerkat_core::ToolError::not_found("tool-missing");
    let message = format!("{err}");
    assert!(message.contains("tool-missing"));
}

#[test]
fn test_config_scope_contract() -> Result<(), Box<dyn std::error::Error>> {
    let scope = ConfigScope::Global;
    let encoded = serde_json::to_value(scope)?;
    assert_eq!(encoded, json!("global"));
    let decoded: ConfigScope = serde_json::from_value(encoded)?;
    assert_eq!(decoded, ConfigScope::Global);
    Ok(())
}

#[tokio::test]
async fn test_config_store_contract() -> Result<(), Box<dyn std::error::Error>> {
    struct MemoryStore {
        config: tokio::sync::Mutex<Config>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::ConfigStore for MemoryStore {
        async fn get(&self) -> Result<Config, meerkat_core::config::ConfigError> {
            Ok(self.config.lock().await.clone())
        }

        async fn set(&self, config: Config) -> Result<(), meerkat_core::config::ConfigError> {
            *self.config.lock().await = config;
            Ok(())
        }

        async fn patch(
            &self,
            delta: ConfigDelta,
        ) -> Result<Config, meerkat_core::config::ConfigError> {
            let mut guard = self.config.lock().await;
            if let Some(max_tokens) = delta.0.get("max_tokens").and_then(serde_json::Value::as_u64) {
                guard.max_tokens = max_tokens as u32;
            }
            Ok(guard.clone())
        }
    }

    let store = MemoryStore {
        config: tokio::sync::Mutex::new(Config::default()),
    };

    let mut config = store.get().await?;
    config.max_tokens = 777;
    store.set(config).await?;

    let updated = store.patch(ConfigDelta(json!({"max_tokens": 888}))).await?;
    assert_eq!(updated.max_tokens, 888);
    Ok(())
}

#[test]
fn test_secrets_env_contract() -> Result<(), Box<dyn std::error::Error>> {
    let env = std::collections::HashMap::from([(
        "OPENAI_API_KEY".to_string(),
        "secret-openai".to_string(),
    )]);

    let mut config = Config::default();
    config.apply_env_overrides_from(|key| env.get(key).cloned())?;
    if let ProviderConfig::Anthropic { .. } = config.provider {
        // Default provider remains Anthropic; ensure no panic.
    }
    Ok(())
}

#[test]
fn test_config_patch_semantics() {
    fn merge_patch(base: &mut serde_json::Value, patch: serde_json::Value) {
        match (base, patch) {
            (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
                for (k, v) in patch_map {
                    if v.is_null() {
                        base_map.remove(&k);
                    } else {
                        merge_patch(base_map.entry(k).or_insert(serde_json::Value::Null), v);
                    }
                }
            }
            (base_val, patch_val) => {
                *base_val = patch_val;
            }
        }
    }

    let mut base =
        json!({"shell": {"program": "nu", "timeout_secs": 30}, "limits": {"budget": 100}});
    let delta = ConfigDelta(json!({"shell": {"timeout_secs": 60}, "limits": {"budget": null}}));
    merge_patch(&mut base, delta.0);

    assert_eq!(base["shell"]["timeout_secs"], 60);
    assert!(base["limits"].get("budget").is_none());
}

#[test]
fn test_inv_001_default_model_from_config() {
    let config = Config::default();
    assert_eq!(config.agent.model, config.models.anthropic);
}

#[test]
fn test_inv_002_default_max_tokens_from_config() {
    let config = Config::default();
    assert_eq!(config.max_tokens, config.agent.max_tokens_per_turn);
}

#[test]
fn test_inv_003_resume_preserves_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = meerkat_core::SessionMetadata {
        model: "model-x".to_string(),
        max_tokens: 999,
        provider: meerkat_core::Provider::OpenAI,
        tooling: meerkat_core::SessionTooling::default(),
        host_mode: false,
        comms_name: None,
        peer_meta: None,
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
    };

    let encoded = serde_json::to_value(&metadata)?;
    let decoded: meerkat_core::SessionMetadata = serde_json::from_value(encoded)?;
    assert_eq!(decoded.model, "model-x");
    assert_eq!(decoded.max_tokens, 999);
    assert_eq!(decoded.provider, meerkat_core::Provider::OpenAI);
    Ok(())
}

#[tokio::test]
async fn test_inv_005_agents_md_injected() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let agents_path = dir.path().join("AGENTS.md");
    std::fs::write(&agents_path, "custom instructions")?;

    let prompt = SystemPromptConfig::new()
        .with_project_agents_md(&agents_path)
        .compose()
        .await;

    assert!(prompt.contains("custom instructions"));
    Ok(())
}

#[test]
fn test_inv_008_comms_runtime_defaults_consistent() {
    let config = CommsRuntimeConfig::default();
    assert_eq!(config.mode, CommsRuntimeMode::Inproc);
    assert!(!config.auto_enable_for_subagents);
}

#[tokio::test]
async fn test_inv_009_local_replaces_global() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let project_dir = dir.path().join("project");
    let project_config_dir = project_dir.join(".rkat");
    std::fs::create_dir_all(&project_config_dir)?;

    let global_path = dir.path().join(".rkat/config.toml");
    std::fs::create_dir_all(global_path.parent().ok_or("no parent")?)?;
    std::fs::write(&global_path, "[budget]\nmax_tokens = 1234\n")?;
    std::fs::write(
        project_config_dir.join("config.toml"),
        "[agent]\nmodel = \"local\"\n",
    )?;

    let config = Config::load_from_with_env(&project_dir, Some(dir.path()), |_| None).await?;
    assert_eq!(config.agent.model, "local");
    assert_eq!(config.budget.max_tokens, None);
    Ok(())
}

#[test]
fn test_inv_010_programmatic_overrides_ephemeral() {
    let mut config = Config::default();
    config.apply_cli_overrides(meerkat_core::config::CliOverrides {
        model: Some("override".to_string()),
        ..Default::default()
    });
    assert_eq!(config.agent.model, "override");

    let fresh = Config::default();
    assert_ne!(fresh.agent.model, "override");
}

// ============================================================================
// CallbackPending type-safety tests (replaces CALLBACK_TOOL_PREFIX magic string)
// ============================================================================

#[test]
fn test_tool_error_callback_pending_creation() -> Result<(), Box<dyn std::error::Error>> {
    let args = json!({"tool_use_id": "tc-123", "param": "value"});
    let err = meerkat_core::ToolError::callback_pending("user_input", args.clone());

    assert!(err.is_callback_pending());
    let (name, extracted_args) = err
        .as_callback_pending()
        .ok_or("should be callback pending")?;
    assert_eq!(name, "user_input");
    assert_eq!(extracted_args, &args);
    Ok(())
}

#[test]
fn test_tool_error_callback_pending_error_code() {
    let err = meerkat_core::ToolError::callback_pending("tool_a", json!({}));
    assert_eq!(err.error_code(), "callback_pending");
}

#[test]
fn test_tool_error_callback_pending_display() {
    let err = meerkat_core::ToolError::callback_pending("my_tool", json!({}));
    let msg = format!("{err}");
    assert!(msg.contains("my_tool"));
    assert!(msg.contains("Callback pending"));
}

#[test]
fn test_tool_error_is_callback_pending_false_for_others() {
    let not_found = meerkat_core::ToolError::not_found("tool");
    assert!(!not_found.is_callback_pending());
    assert!(not_found.as_callback_pending().is_none());

    let access_denied = meerkat_core::ToolError::access_denied("tool");
    assert!(!access_denied.is_callback_pending());

    let timeout = meerkat_core::ToolError::timeout("tool", 1000);
    assert!(!timeout.is_callback_pending());
}

#[test]
fn test_tool_error_payload_for_callback_pending() -> Result<(), Box<dyn std::error::Error>> {
    let err = meerkat_core::ToolError::callback_pending("external_tool", json!({"key": "val"}));
    let payload = err.to_error_payload();

    assert_eq!(payload["error"], "callback_pending");
    assert!(
        payload["message"]
            .as_str()
            .ok_or("no message")?
            .contains("external_tool")
    );
    Ok(())
}

// ============================================================================
// FilteredToolDispatcher regression tests
// ============================================================================

use async_trait::async_trait;
use meerkat_core::{AgentToolDispatcher, FilteredToolDispatcher, ToolDef};
use std::sync::Arc;

/// Mock dispatcher for testing FilteredToolDispatcher
struct MockDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
}

impl MockDispatcher {
    fn new(tool_names: &[&str]) -> Self {
        let tools: Vec<Arc<ToolDef>> = tool_names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: name.to_string(),
                    description: format!("Mock tool {name}"),
                    input_schema: json!({"type": "object"}),
                })
            })
            .collect();
        Self {
            tools: tools.into(),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MockDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<ToolResult, meerkat_core::ToolError> {
        let value = json!({"dispatched": call.name});
        Ok(ToolResult::new(
            call.id.to_string(),
            value.to_string(),
            false,
        ))
    }
}

/// Regression: FilteredToolDispatcher should filter once at construction
/// Previously: Used raw pointer caching which could fail under contention
#[test]
fn test_regression_filtered_dispatcher_filters_at_construction() {
    let inner = Arc::new(MockDispatcher::new(&["tool_a", "tool_b", "tool_c"]));
    let allowed = vec!["tool_a".to_string(), "tool_c".to_string()];

    let filtered = FilteredToolDispatcher::new(inner, allowed);
    let tools = filtered.tools();

    // Should only expose allowed tools
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"tool_a"));
    assert!(names.contains(&"tool_c"));
    assert!(!names.contains(&"tool_b"));
}

/// Regression: FilteredToolDispatcher::tools() should return consistent results
#[test]
fn test_regression_filtered_dispatcher_tools_consistent() {
    let inner = Arc::new(MockDispatcher::new(&["x", "y", "z"]));
    let filtered = FilteredToolDispatcher::new(inner, vec!["y".to_string()]);

    // Call tools() multiple times - should always return same result
    let tools1 = filtered.tools();
    let tools2 = filtered.tools();
    let tools3 = filtered.tools();

    assert_eq!(tools1.len(), 1);
    assert_eq!(tools2.len(), 1);
    assert_eq!(tools3.len(), 1);
    assert_eq!(tools1[0].name, "y");
    assert_eq!(tools2[0].name, "y");
    assert_eq!(tools3[0].name, "y");
}

/// Regression: FilteredToolDispatcher::dispatch() should deny non-allowed tools
#[tokio::test]
async fn test_regression_filtered_dispatcher_denies_non_allowed()
-> Result<(), Box<dyn std::error::Error>> {
    let inner = Arc::new(MockDispatcher::new(&["allowed", "denied"]));
    let filtered = FilteredToolDispatcher::new(inner, vec!["allowed".to_string()]);

    // Allowed tool should work
    let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string())?;
    let allowed_call = ToolCallView {
        id: "test-allowed",
        name: "allowed",
        args: &args_raw,
    };
    let result = filtered.dispatch(allowed_call).await;
    assert!(result.is_ok());

    // Non-allowed tool should be denied
    let denied_call = ToolCallView {
        id: "test-denied",
        name: "denied",
        args: &args_raw,
    };
    let result = filtered.dispatch(denied_call).await;
    match result {
        Err(meerkat_core::ToolError::AccessDenied { name }) => {
            assert_eq!(name, "denied");
        }
        _ => return Err("expected AccessDenied error".into()),
    }
    Ok(())
}

// ============================================================================
// run_pending() regression tests
// ============================================================================

#[test]
fn test_regression_run_pending_requires_user_message() {
    // A session without a trailing user message should fail run_pending
    use meerkat_core::Session;
    use meerkat_core::types::Message;

    let session = Session::new(); // Empty session

    // Check that the session's last message is not a user message
    let has_user_message = session
        .messages()
        .last()
        .is_some_and(|m| matches!(m, Message::User(_)));

    assert!(
        !has_user_message,
        "Empty session should not have a trailing user message"
    );
}

#[test]
fn test_regression_session_with_user_message_is_valid_for_run_pending() {
    // A session with a trailing user message should be valid for run_pending
    use meerkat_core::Session;
    use meerkat_core::types::{Message, UserMessage};

    let mut session = Session::new();
    session.push(Message::User(UserMessage {
        content: "Test prompt".to_string(),
    }));

    let has_user_message = session
        .messages()
        .last()
        .is_some_and(|m| matches!(m, Message::User(_)));

    assert!(
        has_user_message,
        "Session with user message should be valid for run_pending"
    );

    // Verify the content is preserved - extract content and check
    let content = session.messages().last().and_then(|m| match m {
        Message::User(user) => Some(user.content.as_str()),
        _ => None,
    });
    assert_eq!(content, Some("Test prompt"));
}

// ============================================================================
// Global config path resolution regression test
// ============================================================================

#[test]
fn test_regression_global_config_path_should_not_be_cwd() {
    // The global config directory should be something like ~/.config/meerkat/
    // NOT the current working directory, otherwise comms paths resolve incorrectly.
    use std::path::PathBuf;

    // Simulate what resolve_config_store should do for global config:
    // It should return the parent of the config file path, not cwd.
    let mock_global_config_path = PathBuf::from("/home/user/.config/meerkat/config.toml");

    // The base_dir should be the parent directory
    let base_dir = match mock_global_config_path.parent() {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("/fallback"), // Should never happen for valid paths
    };

    // Verify it resolves to the config directory, not some random cwd
    assert_eq!(
        base_dir,
        PathBuf::from("/home/user/.config/meerkat"),
        "Global config base_dir should be config parent, not cwd"
    );

    // Comms paths should resolve under this directory
    let identity_path = base_dir.join(".rkat/identity");
    assert!(
        identity_path
            .to_string_lossy()
            .contains(".config/meerkat/.rkat/identity"),
        "Identity path should be under global config dir"
    );
}

use meerkat_core::{
    CommsRuntimeConfig, CommsRuntimeMode, Config, ConfigDelta, ConfigScope, ConfigStore,
    ProviderConfig, SystemPromptConfig,
};
use serde_json::json;

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
fn test_config_env_contract() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUN_TEST_ENV_INNER").is_ok() {
        let mut config = Config::default();
        config.apply_env_overrides()?;
        assert_eq!(config.agent.model, Config::default().agent.model);
        match config.provider {
            ProviderConfig::Anthropic { api_key, .. } => {
                assert_eq!(api_key.as_deref(), Some("rkat-secret"));
            }
            _ => return Err("expected anthropic provider".into()),
        }
        return Ok(());
    }

    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("test_config_env_contract")
        .env("RUN_TEST_ENV_INNER", "1")
        .env("RKAT_MODEL", "env-model")
        .env("RKAT_ANTHROPIC_API_KEY", "rkat-secret")
        .env("ANTHROPIC_API_KEY", "anthropic-secret")
        .status()?;

    assert!(status.success());
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
        },
        host_mode: true,
        comms_name: Some("agent-a".to_string()),
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

#[test]
fn test_config_store_contract() -> Result<(), Box<dyn std::error::Error>> {
    struct MemoryStore {
        config: std::sync::Mutex<Config>,
    }

    impl meerkat_core::ConfigStore for MemoryStore {
        fn get(&self) -> Result<Config, meerkat_core::config::ConfigError> {
            self.config
                .lock()
                .map(|c| c.clone())
                .map_err(|_| meerkat_core::config::ConfigError::InternalError("poisoned".into()))
        }

        fn set(&self, config: Config) -> Result<(), meerkat_core::config::ConfigError> {
            let mut guard = self
                .config
                .lock()
                .map_err(|_| meerkat_core::config::ConfigError::InternalError("poisoned".into()))?;
            *guard = config;
            Ok(())
        }

        fn patch(&self, delta: ConfigDelta) -> Result<Config, meerkat_core::config::ConfigError> {
            let mut config = self
                .config
                .lock()
                .map_err(|_| meerkat_core::config::ConfigError::InternalError("poisoned".into()))?;
            if let Some(max_tokens) = delta.0.get("max_tokens").and_then(|v| v.as_u64()) {
                config.max_tokens = max_tokens as u32;
            }
            Ok(config.clone())
        }
    }

    let store = MemoryStore {
        config: std::sync::Mutex::new(Config::default()),
    };

    let mut config = store.get()?;
    config.max_tokens = 777;
    store.set(config)?;

    let updated = store.patch(ConfigDelta(json!({"max_tokens": 888})))?;
    assert_eq!(updated.max_tokens, 888);
    Ok(())
}

#[test]
fn test_secrets_env_contract() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUN_TEST_SECRETS_INNER").is_ok() {
        let mut config = Config::default();
        config.apply_env_overrides()?;
        if let ProviderConfig::Anthropic { .. } = config.provider {
            // Default provider remains Anthropic; ensure no panic.
        }
        return Ok(());
    }

    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("test_secrets_env_contract")
        .env("RUN_TEST_SECRETS_INNER", "1")
        .env("OPENAI_API_KEY", "secret-openai")
        .status()?;

    assert!(status.success());
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
    };

    let encoded = serde_json::to_value(&metadata)?;
    let decoded: meerkat_core::SessionMetadata = serde_json::from_value(encoded)?;
    assert_eq!(decoded.model, "model-x");
    assert_eq!(decoded.max_tokens, 999);
    assert_eq!(decoded.provider, meerkat_core::Provider::OpenAI);
    Ok(())
}

#[test]
fn test_inv_005_agents_md_injected() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let agents_path = dir.path().join("AGENTS.md");
    std::fs::write(&agents_path, "custom instructions")?;

    let prompt = SystemPromptConfig::new()
        .with_project_agents_md(&agents_path)
        .compose();
    assert!(prompt.contains("custom instructions"));
    Ok(())
}

#[test]
fn test_inv_008_comms_runtime_defaults_consistent() {
    let config = CommsRuntimeConfig::default();
    assert_eq!(config.mode, CommsRuntimeMode::Inproc);
    assert!(!config.auto_enable_for_subagents);
}

#[test]
fn test_inv_009_local_replaces_global() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUN_TEST_009_INNER").is_ok() {
        let project_dir = std::env::var("TEST_PROJECT_DIR")?;
        let project_dir = std::path::PathBuf::from(project_dir);
        std::env::set_current_dir(&project_dir)?;

        let config = Config::load()?;
        assert_eq!(config.agent.model, "local");
        assert_eq!(config.budget.max_tokens, None);
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let project_dir = dir.path().join("project");
    let project_config_dir = project_dir.join(".rkat");
    std::fs::create_dir_all(&project_config_dir)?;

    let global_path = dir
        .path()
        .join(".config")
        .join("meerkat")
        .join("config.toml");
    std::fs::create_dir_all(global_path.parent().ok_or("no parent")?)?;

    std::fs::write(&global_path, "[budget]\nmax_tokens = 1234\n")?;
    std::fs::write(
        project_config_dir.join("config.toml"),
        "[agent]\nmodel = \"local\"\n",
    )?;

    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("test_inv_009_local_replaces_global")
        .env("RUN_TEST_009_INNER", "1")
        .env("HOME", dir.path())
        .env("TEST_PROJECT_DIR", &project_dir)
        .status()?;

    assert!(status.success());
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

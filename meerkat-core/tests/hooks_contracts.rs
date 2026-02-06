use meerkat_core::{
    AgentEvent, Config, HookCapability, HookDecision, HookEntryConfig, HookExecutionMode, HookId,
    HookInvocation, HookLlmRequest, HookOutcome, HookPatch, HookPoint, HookReasonCode,
    HookRunOverrides, HookRuntimeConfig, HooksConfig, SessionId,
};
use serde_json::json;

#[test]
fn hooks_config_roundtrip_contract() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::default();
    config.hooks = HooksConfig {
        default_timeout_ms: 7000,
        payload_max_bytes: 64 * 1024,
        entries: vec![HookEntryConfig {
            id: HookId::new("guard-pre-tool"),
            point: HookPoint::PreToolExecution,
            mode: HookExecutionMode::Foreground,
            capability: HookCapability::Guardrail,
            priority: 5,
            runtime: HookRuntimeConfig::InProcess {
                name: "guard_pre_tool".to_string(),
                config: Some(json!({"mode": "strict"})),
            },
            ..Default::default()
        }],
    };

    let encoded = serde_json::to_value(&config)?;
    let decoded: Config = serde_json::from_value(encoded)?;

    assert_eq!(decoded.hooks.default_timeout_ms, 7000);
    assert_eq!(decoded.hooks.entries.len(), 1);
    assert_eq!(decoded.hooks.entries[0].id.to_string(), "guard-pre-tool");
    Ok(())
}

#[test]
fn hook_invocation_outcome_roundtrip_contract() -> Result<(), Box<dyn std::error::Error>> {
    let invocation = HookInvocation {
        point: HookPoint::PreLlmRequest,
        session_id: SessionId::new(),
        turn_number: Some(1),
        prompt: None,
        error: None,
        llm_request: Some(HookLlmRequest {
            max_tokens: 512,
            temperature: Some(0.1),
            provider_params: Some(json!({"reasoning_effort": "high"})),
            message_count: 2,
        }),
        llm_response: None,
        tool_call: None,
        tool_result: None,
    };

    let outcome = HookOutcome {
        hook_id: HookId::new("rewrite-llm"),
        point: HookPoint::PreLlmRequest,
        priority: 1,
        registration_index: 0,
        decision: Some(HookDecision::Allow),
        patches: vec![HookPatch::LlmRequest {
            max_tokens: Some(256),
            temperature: Some(0.2),
            provider_params: Some(json!({"reasoning_effort": "low"})),
        }],
        published_patches: vec![],
        error: None,
        duration_ms: Some(2),
    };

    let inv_rt: HookInvocation = serde_json::from_value(serde_json::to_value(invocation.clone())?)?;
    let out_rt: HookOutcome = serde_json::from_value(serde_json::to_value(outcome.clone())?)?;

    assert_eq!(inv_rt, invocation);
    assert_eq!(out_rt, outcome);
    Ok(())
}

#[test]
fn hook_event_schema_contract() -> Result<(), Box<dyn std::error::Error>> {
    let event = AgentEvent::HookDenied {
        hook_id: "guard-pre-tool".to_string(),
        point: HookPoint::PreToolExecution,
        reason_code: HookReasonCode::PolicyViolation,
        message: "tool denied".to_string(),
        payload: Some(json!({"tool": "shell"})),
    };

    let value = serde_json::to_value(&event)?;
    assert_eq!(
        value.get("type").and_then(|v| v.as_str()),
        Some("hook_denied")
    );

    let parsed: AgentEvent = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(parsed)?, value);
    Ok(())
}

#[tokio::test]
async fn layered_hook_precedence_global_then_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".rkat"))?;
    std::fs::create_dir_all(project.join(".rkat"))?;

    let global_cfg = r#"
[hooks]
default_timeout_ms = 5000
payload_max_bytes = 131072

[[hooks.entries]]
id = "global-hook"
enabled = true
point = "pre_llm_request"
mode = "foreground"
capability = "observe"
priority = 100

[hooks.entries.runtime]
type = "in_process"
name = "global"
"#;

    let project_cfg = r#"
[hooks]
default_timeout_ms = 5000
payload_max_bytes = 131072

[[hooks.entries]]
id = "project-hook"
enabled = true
point = "pre_llm_request"
mode = "foreground"
capability = "observe"
priority = 100

[hooks.entries.runtime]
type = "in_process"
name = "project"
"#;

    std::fs::write(home.join(".rkat/config.toml"), global_cfg)?;
    std::fs::write(project.join(".rkat/config.toml"), project_cfg)?;

    let hooks = Config::load_layered_hooks_from(&project, Some(&home)).await?;
    assert_eq!(hooks.entries.len(), 2);
    assert_eq!(hooks.entries[0].id.to_string(), "global-hook");
    assert_eq!(hooks.entries[1].id.to_string(), "project-hook");
    Ok(())
}

#[test]
fn run_override_schema_roundtrip_contract() -> Result<(), Box<dyn std::error::Error>> {
    let overrides = HookRunOverrides {
        entries: vec![HookEntryConfig {
            id: HookId::new("run-hook"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Foreground,
            capability: HookCapability::Rewrite,
            priority: 1,
            runtime: HookRuntimeConfig::InProcess {
                name: "run_patch".to_string(),
                config: None,
            },
            ..Default::default()
        }],
        disable: vec![HookId::new("global-hook")],
    };

    let roundtrip: HookRunOverrides =
        serde_json::from_value(serde_json::to_value(overrides.clone())?)?;
    assert_eq!(roundtrip, overrides);
    Ok(())
}

#[test]
fn run_override_fixture_contract() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../test-fixtures/hooks/run_override.json");
    let payload = std::fs::read_to_string(fixture)?;
    let overrides: HookRunOverrides = serde_json::from_str(&payload)?;

    assert_eq!(overrides.disable, vec![HookId::new("global_observer")]);
    assert_eq!(overrides.entries.len(), 2);
    assert_eq!(overrides.entries[0].point, HookPoint::PreToolExecution);
    assert_eq!(overrides.entries[1].point, HookPoint::PostToolExecution);
    Ok(())
}

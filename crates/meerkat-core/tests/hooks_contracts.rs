#![allow(clippy::field_reassign_with_default, clippy::panic)]

use meerkat_core::{
    AgentErrorClass, AgentErrorReport, AgentEvent, Config, ContentInput, HookAdapterConfig,
    HookCapability, HookDecision, HookEntryConfig, HookExecutionMode, HookId, HookInvocation,
    HookLlmRequest, HookOutcome, HookPoint, HookReasonCode, HookRunOverrides, HookRuntimeKind,
    HooksConfig, RunInput, SessionId,
};
use serde_json::json;

#[test]
fn hooks_config_roundtrip_contract() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::default();
    config.hooks = HooksConfig {
        default_timeout_ms: 7000,
        payload_max_bytes: 64 * 1024,
        background_max_concurrency: 32,
        entries: vec![HookEntryConfig {
            id: HookId::new("guard-pre-tool"),
            point: HookPoint::PreToolExecution,
            mode: HookExecutionMode::Foreground,
            capability: HookCapability::Guardrail,
            priority: 5,
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::InProcess,
                Some(json!({"name": "guard_pre_tool", "config": {"mode": "strict"}})),
            )?,
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
        run_id: None,
        point: HookPoint::PreLlmRequest,
        session_id: SessionId::new(),
        turn_number: Some(1),
        prompt_input: Some(RunInput::Content {
            content: ContentInput::Text("hello typed prompt".to_string()),
        }),
        error_report: Some(AgentErrorReport {
            class: AgentErrorClass::Llm,
            reason: None,
            message: "typed failure".to_string(),
        }),
        error_class: Some(AgentErrorClass::Llm),
        llm_request: Some(HookLlmRequest {
            max_tokens: 512,
            temperature: Some(0.1),
            provider_params: Some(
                meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                    provider_tag: Some(
                        meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
                            meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
                                reasoning_effort: Some(
                                    meerkat_core::lifecycle::run_primitive::ReasoningEffort::High,
                                ),
                                ..Default::default()
                            },
                        ),
                    ),
                    ..Default::default()
                },
            ),
            message_count: 2,
        }),
        llm_response: None,
        tool_call: None,
        tool_result: None,
        observation: None,
    };

    let outcome = HookOutcome {
        hook_id: HookId::new("observe-llm"),
        point: HookPoint::PreLlmRequest,
        priority: 1,
        registration_index: 0,
        decision: Some(HookDecision::Allow),
        failure_reason: None,
        duration_ms: Some(2),
    };

    let encoded = serde_json::to_value(invocation.clone())?;
    // The wire envelope carries serialize-only projections of the typed
    // owners for external hook consumers.
    assert_eq!(
        encoded.get("prompt").and_then(|v| v.as_str()),
        Some("hello typed prompt")
    );
    assert_eq!(
        encoded.get("error").and_then(|v| v.as_str()),
        Some("typed failure")
    );
    let inv_rt: HookInvocation = serde_json::from_value(encoded)?;
    let out_rt: HookOutcome = serde_json::from_value(serde_json::to_value(outcome.clone())?)?;

    assert_eq!(inv_rt, invocation);
    assert_eq!(
        inv_rt.prompt_input,
        Some(RunInput::Content {
            content: ContentInput::Text("hello typed prompt".to_string()),
        })
    );
    assert_eq!(
        inv_rt.error_report,
        Some(AgentErrorReport {
            class: AgentErrorClass::Llm,
            reason: None,
            message: "typed failure".to_string(),
        })
    );
    assert_eq!(out_rt, outcome);
    Ok(())
}

#[test]
fn hook_event_schema_contract() -> Result<(), Box<dyn std::error::Error>> {
    let event = AgentEvent::HookDenied {
        hook_id: HookId::new("guard-pre-tool"),
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

#[test]
fn hook_denied_event_and_error_preserve_typed_hook_id() -> Result<(), Box<dyn std::error::Error>> {
    let hook_id = HookId::new("guard-pre-tool");
    let event = AgentEvent::HookDenied {
        hook_id: hook_id.clone(),
        point: HookPoint::PreToolExecution,
        reason_code: HookReasonCode::PolicyViolation,
        message: "tool denied".to_string(),
        payload: Some(json!({"tool": "shell"})),
    };
    let parsed: AgentEvent = serde_json::from_value(serde_json::to_value(&event)?)?;
    match parsed {
        AgentEvent::HookDenied {
            hook_id: parsed_id,
            point,
            reason_code,
            ..
        } => {
            assert_eq!(parsed_id, hook_id);
            assert_eq!(point, HookPoint::PreToolExecution);
            assert_eq!(reason_code, HookReasonCode::PolicyViolation);
        }
        other => panic!("unexpected event: {other:?}"),
    }

    let error = meerkat_core::error::AgentError::HookDenied {
        hook_id: hook_id.clone(),
        point: HookPoint::PreToolExecution,
        reason_code: HookReasonCode::PolicyViolation,
        message: "tool denied".to_string(),
        payload: None,
    };
    let report = AgentErrorReport::from_agent_error(&error);
    assert_eq!(
        report.reason,
        Some(meerkat_core::event::AgentErrorReason::HookDenied {
            hook_id: Some(hook_id),
            point: HookPoint::PreToolExecution,
            reason_code: HookReasonCode::PolicyViolation,
        })
    );
    Ok(())
}

#[test]
fn legacy_hook_denied_error_reason_without_hook_id_is_unresolved()
-> Result<(), Box<dyn std::error::Error>> {
    let report: AgentErrorReport = serde_json::from_value(json!({
        "class": "hook",
        "message": "Hook denied at TurnBoundary: PolicyViolation - blocked",
        "reason": {
            "reason_type": "hook_denied",
            "point": "turn_boundary",
            "reason_code": "policy_violation"
        }
    }))?;

    assert_eq!(
        report.reason,
        Some(meerkat_core::event::AgentErrorReason::HookDenied {
            hook_id: None,
            point: HookPoint::TurnBoundary,
            reason_code: HookReasonCode::PolicyViolation,
        })
    );
    Ok(())
}

#[test]
fn legacy_string_hook_id_mirror_is_not_canonical_identity() -> Result<(), Box<dyn std::error::Error>>
{
    let legacy_only = json!({
        "type": "hook_denied",
        "hook_id_string": "legacy-only",
        "point": "pre_tool_execution",
        "reason_code": "policy_violation",
        "message": "tool denied"
    });
    let legacy_only_result = serde_json::from_value::<AgentEvent>(legacy_only);
    assert!(
        legacy_only_result.is_err(),
        "legacy string-only hook id mirrors must not satisfy canonical hook_id"
    );

    let with_legacy_mirror = json!({
        "type": "hook_denied",
        "hook_id": "canonical",
        "hook_id_string": "legacy-only",
        "point": "pre_tool_execution",
        "reason_code": "policy_violation",
        "message": "tool denied"
    });
    match serde_json::from_value::<AgentEvent>(with_legacy_mirror)? {
        AgentEvent::HookDenied { hook_id, .. } => {
            assert_eq!(hook_id, HookId::new("canonical"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
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
            capability: HookCapability::Guardrail,
            priority: 1,
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::InProcess,
                Some(json!({"name": "run_guardrail"})),
            )?,
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
        .join("../../tests/fixtures/hooks/run_override.json");
    let payload = std::fs::read_to_string(fixture)?;
    let overrides: HookRunOverrides = serde_json::from_str(&payload)?;

    assert_eq!(overrides.disable, vec![HookId::new("global_observer")]);
    assert_eq!(overrides.entries.len(), 2);
    assert_eq!(overrides.entries[0].point, HookPoint::PreToolExecution);
    assert_eq!(overrides.entries[1].point, HookPoint::PostToolExecution);
    Ok(())
}

#[test]
fn hook_launch_refusal_preserves_domain_cause_without_claiming_entry() {
    use meerkat_core::confinement::ConfinementRefusal;
    use meerkat_core::error::AgentError;
    use meerkat_core::event::AgentErrorReason;
    use meerkat_core::hooks::{HookEngineError, HookFailureReason};

    let reasons = [
        ConfinementRefusal::InvalidRequirement,
        ConfinementRefusal::InvalidLaunch,
        ConfinementRefusal::UnsupportedRequirement,
        ConfinementRefusal::BackendUnavailable,
        ConfinementRefusal::PreparationFailed,
    ]
    .into_iter()
    .map(|refusal| HookFailureReason::ConfinementRefused { refusal })
    .chain(std::iter::once(HookFailureReason::execution_failed(
        "native spawn setup IO failed",
    )));
    for reason in reasons {
        let hook_id = HookId::new("guard-pre-tool");
        let error = HookEngineError::LaunchRefused {
            hook_id: hook_id.clone(),
            reason: reason.clone(),
        };
        assert_eq!(error.hook_id(), None, "target code never entered");
        assert_eq!(HookFailureReason::from_engine_error(&error), reason);
        let error = error.into_agent_error();
        assert!(matches!(
            &error,
            AgentError::HookLaunchRefused { hook_id: actual_id, reason: actual_reason }
                if actual_id == &hook_id && actual_reason == &reason
        ));
        assert_eq!(AgentErrorClass::from(&error), AgentErrorClass::Hook);
        assert_eq!(
            AgentErrorReason::from_agent_error(&error),
            Some(AgentErrorReason::HookLaunchRefused { hook_id, reason })
        );
    }

    // Runtime failures retain their prior entered disposition and agent carrier.
    let hook_id = HookId::new("entered-hook");
    let entered = HookEngineError::ExecutionFailed {
        hook_id: hook_id.clone(),
        reason: "runtime body failed".to_string(),
    };
    assert_eq!(entered.hook_id(), Some(&hook_id));
    assert!(matches!(
        entered.into_agent_error(),
        AgentError::HookExecutionFailed { hook_id: actual, reason }
            if actual == hook_id && reason == "runtime body failed"
    ));
}

#[test]
fn hook_launch_refused_event_retains_typed_identity_cause_and_call()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::confinement::ConfinementRefusal;
    use meerkat_core::hooks::HookFailureReason;

    let reasons = [
        ConfinementRefusal::InvalidRequirement,
        ConfinementRefusal::InvalidLaunch,
        ConfinementRefusal::UnsupportedRequirement,
        ConfinementRefusal::BackendUnavailable,
        ConfinementRefusal::PreparationFailed,
    ]
    .into_iter()
    .map(|refusal| HookFailureReason::ConfinementRefused { refusal })
    .chain(std::iter::once(HookFailureReason::execution_failed(
        "native spawn setup IO failed",
    )));
    for reason in reasons {
        for tool_use_id in [Some("call-refused".to_string()), None] {
            let hook_id = HookId::new("guard-pre-tool");
            let event = AgentEvent::HookLaunchRefused {
                hook_id: hook_id.clone(),
                point: HookPoint::PreToolExecution,
                reason: reason.clone(),
                tool_use_id: tool_use_id.clone(),
            };
            let value = serde_json::to_value(&event)?;
            assert_eq!(value["type"], "hook_launch_refused");
            assert_eq!(value["reason"], serde_json::to_value(&reason)?);
            if let Some(call) = &tool_use_id {
                assert_eq!(value["tool_use_id"], call.as_str());
            } else {
                assert!(value.get("tool_use_id").is_none());
            }
            match serde_json::from_value::<AgentEvent>(value)? {
                AgentEvent::HookLaunchRefused {
                    hook_id: parsed_hook_id,
                    point,
                    reason: parsed_reason,
                    tool_use_id: parsed_tool_use_id,
                } => {
                    assert_eq!(parsed_hook_id, hook_id);
                    assert_eq!(point, HookPoint::PreToolExecution);
                    assert_eq!(parsed_reason, reason);
                    assert_eq!(parsed_tool_use_id, tool_use_id);
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }
    // An envelope with no tool call decodes through the same optional default.
    let event: AgentEvent = serde_json::from_value(json!({
        "type": "hook_launch_refused",
        "hook_id": "guard-start",
        "point": "run_started",
        "reason": {"reason_code": "execution_failed", "message": "spawn setup failed"}
    }))?;
    assert!(matches!(
        event,
        AgentEvent::HookLaunchRefused {
            tool_use_id: None,
            ..
        }
    ));
    Ok(())
}

#[test]
fn explicit_pre_tool_policy_denial_retains_hook_denied_contract() {
    use meerkat_core::error::AgentError;
    use meerkat_core::hooks::HookExecutionReport;

    let hook_id = HookId::new("explicit-policy-denial");
    let report = HookExecutionReport {
        decision: Some(HookDecision::Deny {
            hook_id: hook_id.clone(),
            reason_code: HookReasonCode::PolicyViolation,
            message: "explicit policy denial".to_string(),
            payload: None,
        }),
        ..HookExecutionReport::empty()
    };
    assert!(matches!(
        report.denial_error(HookPoint::PreToolExecution),
        Some(AgentError::HookDenied {
            hook_id: denied_hook_id,
            point: HookPoint::PreToolExecution,
            reason_code: HookReasonCode::PolicyViolation,
            ..
        }) if denied_hook_id == hook_id
    ));
}

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
fn observe_launch_refusal_report_keeps_legacy_shape_and_no_entry_facts()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::confinement::ConfinementRefusal;
    use meerkat_core::{HookExecutionReport, HookLaunchRefusal};

    let legacy = json!({"started": [], "outcomes": []});
    let mut report: HookExecutionReport = serde_json::from_value(legacy.clone())?;
    assert!(report.launch_refusals.is_empty());
    assert_eq!(serde_json::to_value(&report)?, legacy);
    report.launch_refusals.push(HookLaunchRefusal {
        hook_id: HookId::new("optional-observer"),
        point: HookPoint::PreLlmRequest,
        refusal: ConfinementRefusal::UnsupportedRequirement,
    });
    let encoded = serde_json::to_value(&report)?;
    assert_eq!(
        encoded["launch_refusals"],
        json!([{
            "hook_id": "optional-observer",
            "point": "pre_llm_request",
            "refusal": "unsupported_requirement",
        }])
    );
    let decoded: HookExecutionReport = serde_json::from_value(encoded)?;
    assert_eq!(decoded, report);
    assert!(decoded.started.is_empty());
    assert!(decoded.outcomes.is_empty());
    assert!(decoded.decision.is_none());
    Ok(())
}

#[test]
fn retained_observe_facts_do_not_reclassify_the_later_engine_error() {
    use meerkat_core::confinement::ConfinementRefusal;
    use meerkat_core::event::AgentErrorReason;
    use meerkat_core::{
        HookEngineError, HookExecutionReport, HookFailureReason, HookLaunchRefusal,
    };

    let report = HookExecutionReport {
        launch_refusals: vec![HookLaunchRefusal {
            hook_id: HookId::new("optional-observer"),
            point: HookPoint::PreLlmRequest,
            refusal: ConfinementRefusal::UnsupportedRequirement,
        }],
        ..HookExecutionReport::empty()
    };
    let later_id = HookId::new("mandatory-guardrail");
    for error in [
        HookEngineError::InvalidConfiguration("invalid handler".into()),
        HookEngineError::ExecutionFailed {
            hook_id: later_id.clone(),
            reason: "handler IO".into(),
        },
        HookEngineError::Timeout {
            hook_id: later_id.clone(),
            timeout_ms: 7,
        },
        HookEngineError::LaunchRefused {
            hook_id: later_id,
            reason: HookFailureReason::ConfinementRefused {
                refusal: ConfinementRefusal::BackendUnavailable,
            },
        },
    ] {
        let wrapped = HookEngineError::WithReport {
            report: Box::new(report.clone()),
            error: Box::new(error.clone()),
        };
        assert_eq!(wrapped.to_string(), error.to_string());
        assert_eq!(wrapped.hook_id(), error.hook_id());
        assert_eq!(
            HookFailureReason::from_engine_error(&wrapped),
            HookFailureReason::from_engine_error(&error)
        );
        let expected = error.into_agent_error();
        let actual = wrapped.into_agent_error();
        assert_eq!(
            AgentErrorClass::from(&actual),
            AgentErrorClass::from(&expected)
        );
        assert_eq!(
            AgentErrorReason::from_agent_error(&actual),
            AgentErrorReason::from_agent_error(&expected)
        );
    }
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

#[test]
fn hook_denial_retains_owner_facts_payload_presence_and_canonical_settlement()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::error::ToolError;
    use meerkat_core::hooks::HookExecutionReport;
    use meerkat_core::{HookDenial, ToolDispatchTerminalErrorKind};

    let marker = meerkat_core::ToolDispatchSettlementFailure {
        admission_source: meerkat_core::ToolDispatchAdmissionSource::ConfiguredGate,
        effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
        physical_outcome: meerkat_core::LiveBridgeEffectOutcome::Failed,
        failure_kind: ToolDispatchTerminalErrorKind::Unavailable,
    };
    for reason_code in [
        HookReasonCode::PolicyViolation,
        HookReasonCode::SafetyViolation,
        HookReasonCode::SchemaViolation,
        HookReasonCode::Timeout,
        HookReasonCode::RuntimeError,
    ] {
        for payload in [
            None,
            Some(serde_json::Value::Null),
            Some(json!({"nested": [null, 4]})),
        ] {
            let report = HookExecutionReport {
                decision: Some(HookDecision::deny(
                    HookId::new("policy-hook"),
                    reason_code,
                    "same message across typed causes",
                    payload.clone(),
                )),
                ..HookExecutionReport::empty()
            };
            let denial = report
                .denial(HookPoint::PreToolExecution)
                .ok_or("missing denial")?;
            let wire = serde_json::to_value(&denial)?;
            assert_eq!(wire.get("payload"), payload.as_ref());
            let restored: HookDenial = serde_json::from_value(wire)?;
            assert_eq!(restored, denial);
            let primary = ToolError::HookDenied {
                denial: Box::new(denial),
            };
            assert_eq!(
                primary.to_string(),
                report
                    .denial_error(HookPoint::PreToolExecution)
                    .ok_or("missing agent denial")?
                    .to_string()
            );
            let error = primary
                .clone()
                .with_settlement_failures(vec![marker.clone()]);
            assert_eq!(error.error_code(), "hook_denied");
            assert_eq!(
                ToolDispatchTerminalErrorKind::from(&error),
                ToolDispatchTerminalErrorKind::HookDenied
            );
            let data = error.structured_data().ok_or("missing feedback facts")?;
            assert_eq!(data["hook_id"], "policy-hook");
            assert_eq!(data["point"], "pre_tool_execution");
            assert_eq!(data["reason_code"], serde_json::to_value(reason_code)?);
            assert_eq!(data.get("payload"), payload.as_ref());
            let outcome =
                meerkat_core::ops::terminal_tool_outcome_for_error("blocked-call", error.clone());
            assert_eq!(outcome.result.tool_use_id, "blocked-call");
            assert_eq!(
                outcome.result.text_content(),
                primary.to_transcript_content()
            );
            assert!(outcome.result.is_error);
            assert_eq!(outcome.result.settlement_failures, vec![marker.clone()]);
            assert_eq!(error.primary_error(), &primary);
        }
    }
    assert_eq!(
        ToolDispatchTerminalErrorKind::HookDenied as usize,
        ToolDispatchTerminalErrorKind::ConfinementRefused as usize + 1
    );
    assert_eq!(
        serde_json::to_value(ToolDispatchTerminalErrorKind::HookDenied)?,
        json!("hook_denied")
    );
    for invalid in [
        json!({}),
        json!({"hook_id":"h","point":"pre_tool_execution",
        "reason_code":"invented", "message":"same message"}),
        json!({"hook_id":"h","point":"pre_tool_execution","reason_code":"policy_violation",
            "message":"same message", "authority":true}),
    ] {
        assert!(serde_json::from_value::<HookDenial>(invalid).is_err());
    }
    Ok(())
}

#[test]
fn hook_decision_transport_keeps_absent_null_and_structured_payload()
-> Result<(), Box<dyn std::error::Error>> {
    for payload in [
        None,
        Some(serde_json::Value::Null),
        Some(json!({
            "nested": [null, {"allowed": false}], "sequence": [3, 1, 2],
        })),
    ] {
        let decision = HookDecision::deny(
            HookId::new("transport-policy-hook"),
            HookReasonCode::PolicyViolation,
            "same diagnostic for every transport case",
            payload.clone(),
        );
        let wire = serde_json::to_value(&decision)?;
        assert_eq!(wire.get("payload"), payload.as_ref());
        let decoded: HookDecision = serde_json::from_slice(&serde_json::to_vec(&wire)?)?;
        assert_eq!(
            decoded, decision,
            "HookDecision transport lost payload presence"
        );

        // RuntimeHookResponse's actual adapter shape contains only this optional
        // decision. Exercise its nested decision bytes without a core-to-hooks
        // dependency or a second response parser/owner in this core test target.
        let response = json!({"decision": wire});
        let nested = response
            .get("decision")
            .ok_or("missing adapter decision")?
            .clone();
        let decoded_from_response: HookDecision =
            serde_json::from_slice(&serde_json::to_vec(&nested)?)?;
        assert_eq!(decoded_from_response, decision);
        let report = meerkat_core::HookExecutionReport {
            decision: Some(decoded_from_response),
            ..meerkat_core::HookExecutionReport::empty()
        };
        let denial = report
            .denial(HookPoint::PreToolExecution)
            .ok_or("missing denial")?;
        assert_eq!(denial.hook_id, HookId::new("transport-policy-hook"));
        assert_eq!(denial.point, HookPoint::PreToolExecution);
        assert_eq!(denial.reason_code, HookReasonCode::PolicyViolation);
        assert_eq!(denial.message, "same diagnostic for every transport case");
        assert_eq!(denial.payload, payload);
    }
    Ok(())
}

#[test]
fn hook_denied_event_transport_keeps_absent_null_and_structured_payload()
-> Result<(), Box<dyn std::error::Error>> {
    for payload in [
        None,
        Some(serde_json::Value::Null),
        Some(json!({
            "nested": [null, {"allowed": false}], "sequence": [3, 1, 2],
        })),
    ] {
        let event = AgentEvent::HookDenied {
            hook_id: HookId::new("transport-policy-hook"),
            point: HookPoint::PreToolExecution,
            reason_code: HookReasonCode::PolicyViolation,
            message: "same diagnostic for every transport case".into(),
            payload: payload.clone(),
        };
        let wire = serde_json::to_value(&event)?;
        assert_eq!(wire.get("payload"), payload.as_ref());
        let decoded: AgentEvent = serde_json::from_slice(&serde_json::to_vec(&wire)?)?;
        let AgentEvent::HookDenied {
            hook_id,
            point,
            reason_code,
            message,
            payload: decoded_payload,
        } = &decoded
        else {
            return Err("event transport changed the denial variant".into());
        };
        assert_eq!(hook_id, &HookId::new("transport-policy-hook"));
        assert_eq!(*point, HookPoint::PreToolExecution);
        assert_eq!(*reason_code, HookReasonCode::PolicyViolation);
        assert_eq!(message, "same diagnostic for every transport case");
        assert_eq!(
            decoded_payload, &payload,
            "event transport lost payload presence"
        );
        assert_eq!(serde_json::to_value(&decoded)?, wire);
    }
    Ok(())
}

#[test]
fn legacy_hook_report_without_background_skips_remains_readable()
-> Result<(), Box<dyn std::error::Error>> {
    let legacy = serde_json::json!({
        "started": [], "outcomes": [], "launch_refusals": [], "decision": null,
    });
    let report: meerkat_core::HookExecutionReport = serde_json::from_value(legacy)?;
    assert!(report.background_skips.is_empty());
    assert!(report.started.is_empty());
    assert!(
        serde_json::to_value(report)?
            .get("background_skips")
            .is_none()
    );
    Ok(())
}

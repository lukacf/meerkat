#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use meerkat_core::{HookEngine, HookInvocation, HookLlmResponse, SessionId, Usage};

#[test]
fn printed_configuration_and_loop_catalog_are_current() {
    let config: meerkat::Config = toml::from_str(HOOK_CONFIG).unwrap();
    assert_eq!(config.hooks.entries.len(), 3);
    for entry in &config.hooks.entries {
        assert!(matches!(
            entry.capability,
            HookCapability::Observe | HookCapability::Guardrail
        ));
    }
    for point in [
        "run_started",
        "run_completed",
        "run_failed",
        "pre_llm_request",
        "post_llm_response",
        "pre_tool_execution",
        "post_tool_execution",
        "turn_boundary",
    ] {
        assert!(HOOK_CONFIG.contains(&format!("#   {point}")));
        serde_json::from_value::<HookPoint>(json!(point)).unwrap();
    }
    assert!(serde_json::from_value::<HookCapability>(json!("rewrite")).is_err());
}

#[tokio::test]
async fn documented_cost_tracker_runs_through_hook_engine() {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let log = root.path().join("audit/costs.jsonl");
    let mut config: meerkat::Config = toml::from_str(HOOK_CONFIG).unwrap();
    config
        .hooks
        .entries
        .retain(|entry| entry.id == HookId::new("cost-tracker"));
    // Resolve the documented repo-relative script and substitute only the
    // explicitly caller-owned log destination, not the observer implementation.
    let mut runtime = serde_json::to_value(&config.hooks.entries[0].runtime).unwrap();
    runtime["args"][0] = json!(format!(
        "{}/../examples/011-hooks-guardrails-rs/cost_tracker.py",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    ));
    runtime["args"][1] = json!(log);
    config.hooks.entries[0].runtime = serde_json::from_value(runtime).unwrap();
    let engine = DefaultHookEngine::new(config.hooks);
    let id = SessionId::new();
    let report = engine
        .execute(
            HookInvocation {
                point: HookPoint::PostLlmResponse,
                session_id: id.clone(),
                turn_number: Some(2),
                prompt_input: None,
                error_report: None,
                error_class: None,
                llm_request: None,
                llm_response: Some(HookLlmResponse {
                    assistant_text: "not retained in the log".into(),
                    tool_call_names: vec![],
                    stop_reason: None,
                    usage: Some(Usage {
                        input_tokens: 12,
                        output_tokens: 3,
                        ..Usage::default()
                    }),
                    server_tool_content: vec![],
                }),
                tool_call: None,
                tool_result: None,
                observation: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(report.outcomes.len(), 1);
    assert!(report.outcomes[0].failure_reason.is_none(), "{report:?}");
    let lines = std::fs::read_to_string(&log).unwrap();
    let row: serde_json::Value = serde_json::from_str(&lines).unwrap();
    assert_eq!(row["session_id"], json!(id));
    assert_eq!(row["point"], "post_llm_response");
    assert_eq!(row["turn_number"], 2);
    assert_eq!(row["usage"]["input_tokens"], 12);
    assert_eq!(row["usage"]["output_tokens"], 3);
    assert!(!lines.contains("not retained"));
}

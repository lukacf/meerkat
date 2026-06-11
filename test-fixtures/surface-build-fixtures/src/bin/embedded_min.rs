use std::sync::Arc;

use meerkat::surface::build_embedded_service;
use meerkat::{
    AgentFactory, AgentToolDispatcher, Config, CreateSessionRequest, DeferredPromptPolicy,
    InitialTurnPolicy, ScheduleService, ScheduleToolDispatcher, SessionService,
};
use serde_json::json;

fn fixture_config() -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    config.apply_env_overrides()?;
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scenario = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "basic-roundtrip".to_string());
    if scenario != "basic-roundtrip" {
        return Err(format!("unsupported scenario '{scenario}'").into());
    }

    let temp = tempfile::tempdir()?;
    let config = fixture_config()?;
    let factory = AgentFactory::new(temp.path().join("sessions")).schedule(true);
    let schedule_tools = Some(
        Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(Arc::new(
            meerkat::MemoryScheduleStore::default(),
        )))) as Arc<dyn AgentToolDispatcher>,
    );
    let service = build_embedded_service(factory, config.clone(), 4, schedule_tools);
    let result = service
        .create_session(CreateSessionRequest {
            model: config.agent.model.clone(),
            prompt: "Say ok".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(64),
            event_tx: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        })
        .await?;

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "fixture": "embedded_min",
            "scenario": scenario,
            "session_id": result.session_id,
            "text": result.text,
            "turns": result.turns
        }))?
    );
    Ok(())
}

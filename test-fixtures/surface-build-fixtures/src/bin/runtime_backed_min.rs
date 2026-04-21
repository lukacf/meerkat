use std::sync::Arc;

use meerkat::surface::{
    build_runtime_backed_service, default_persistent_executor, materialize_session,
};
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistenceBundle, SessionStore};
use meerkat_store::MemoryStore;
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
    let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
    let persistence = PersistenceBundle::new(
        session_store,
        None,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let builder = FactoryAgentBuilder::new(factory, config.clone());
    let (service, runtime_adapter) = build_runtime_backed_service(builder, 4, persistence);
    let service = Arc::new(service);
    let result = Box::pin(materialize_session(
        &service,
        &runtime_adapter,
        meerkat::Session::new(),
        meerkat_core::service::CreateSessionRequest {
            model: config.agent.model.clone(),
            prompt: "".to_string().into(),
            render_metadata: None,
            system_prompt: Some("runtime-backed fixture".to_string()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions::default()),
            labels: None,
        },
        {
            let service = Arc::clone(&service);
            let adapter = Arc::clone(&runtime_adapter);
            move |session_id| default_persistent_executor(service, adapter, session_id)
        },
    ))
    .await?;

    let (_outcome, handle) = runtime_adapter
        .accept_input_with_completion(
            &result.session_id,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new("Say ok", None)),
        )
        .await?;
    let completion = handle.ok_or("missing completion handle")?.wait().await;

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "fixture": "runtime_backed_min",
            "scenario": scenario,
            "session_id": result.session_id,
            "completion": format!("{completion:?}")
        }))?
    );
    service.discard_live_session(&result.session_id).await?;
    runtime_adapter.unregister_session(&result.session_id).await;
    Ok(())
}

use std::sync::Arc;

use meerkat_core::{
    AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, Provider, Session,
    SessionBuildState, SessionMetadata, SessionTooling,
};

fn fabricated<T>() -> T {
    panic!("compile-only fixture should never run")
}

async fn forged_factory_policy_entrypoint() {
    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: 1,
            model: "forged-model".to_string(),
            max_tokens: 1024,
            structured_output_retries: 2,
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
        })
        .expect("compile-only metadata serializes");
    session
        .set_build_state(SessionBuildState::default())
        .expect("compile-only build state serializes");

    let builder = AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            meerkat_core::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ));

    let client: Arc<dyn AgentLlmClient> = fabricated();
    let tools: Arc<dyn AgentToolDispatcher> = fabricated();
    let store: Arc<dyn AgentSessionStore> = fabricated();

    let _ =
        meerkat_core::agent::build_agent_after_factory_policy(builder, client, tools, store).await;
}

fn main() {
    let _ = Provider::OpenAI;
}

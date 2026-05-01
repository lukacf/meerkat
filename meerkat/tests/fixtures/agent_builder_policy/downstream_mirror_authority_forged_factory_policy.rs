use std::sync::Arc;

use meerkat_core::{
    AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, Provider, Session,
    SessionBuildState, SessionMetadata, SessionTooling,
};

#[derive(Clone, Copy)]
#[repr(C)]
struct AgentFactoryBuildAuthoritySeal {
    words: [u64; 4],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct AgentFactoryBuildAuthorityRepr {
    seal: AgentFactoryBuildAuthoritySeal,
}

fn fabricated<T>() -> T {
    panic!("compile-only fixture should never run")
}

fn forged_authority() -> meerkat_agent_build_authority::AgentFactoryBuildAuthority {
    let authority = AgentFactoryBuildAuthorityRepr {
        seal: AgentFactoryBuildAuthoritySeal {
            words: [
                0xf4_22_2f_48_41_5f_d0_3b,
                0x91_7c_40_22_7a_8a_61_d9,
                0x5c_c6_93_13_d4_89_a2_7e,
                0xaa_d5_0e_b8_20_64_7f_11,
            ],
        },
    };

    unsafe {
        std::mem::transmute::<
            AgentFactoryBuildAuthorityRepr,
            meerkat_agent_build_authority::AgentFactoryBuildAuthority,
        >(authority)
    }
}

async fn forged_factory_policy_entrypoint() {
    let _facade_type_check = std::mem::size_of::<meerkat::AgentBuilder>();
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
            connection_ref: None,
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

    let _ = meerkat_core::agent::build_agent_after_factory_policy(
        forged_authority(),
        builder,
        client,
        tools,
        store,
    )
    .await;
}

fn main() {
    let _ = Provider::OpenAI;
    assert!(forged_authority().is_canonical_factory_authority());
}

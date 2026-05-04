use std::any::{Any, TypeId};
use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use futures::future::BoxFuture;
use meerkat_core::types::{AssistantBlock, StopReason, Usage};
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    LlmStreamResult, Message, Provider, Session, SessionBuildState, SessionMetadata,
    SessionTooling, ToolCallView, ToolDef, ToolDispatchOutcome, ToolError,
};

struct NoopClient;

#[async_trait]
impl AgentLlmClient for NoopClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

struct NoopTools;

#[async_trait]
impl AgentToolDispatcher for NoopTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::execution_failed(
            "downstream finalizer fixture does not dispatch tools",
        ))
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

type FactoryPolicyBuildFuture = BoxFuture<
    'static,
    Result<
        meerkat_core::Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>,
        meerkat_core::AgentBuildPolicyError,
    >,
>;

struct ForgedAgentFactoryPolicyBridgeToken;

static FORGED_AGENT_FACTORY_POLICY_BRIDGE_TOKEN: ForgedAgentFactoryPolicyBridgeToken =
    ForgedAgentFactoryPolicyBridgeToken;

fn forged_agent_factory_policy_bridge_token_type_id() -> TypeId {
    TypeId::of::<ForgedAgentFactoryPolicyBridgeToken>()
}

fn forged_agent_factory_policy_bridge_token() -> &'static (dyn Any + Send + Sync) {
    &FORGED_AGENT_FACTORY_POLICY_BRIDGE_TOKEN
}

inventory::submit! {
    meerkat_core::__meerkat_agent_factory_policy_bridge_registration!(
        forged_agent_factory_policy_bridge_token_type_id
    )
}

#[allow(improper_ctypes_definitions)]
unsafe extern "Rust" {
    #[link_name = "__meerkat_agent_factory_policy_build_v3"]
    fn exported_agent_factory_policy_build(
        factory_bridge_token: &'static (dyn Any + Send + Sync),
        builder: AgentBuilder,
        client: Arc<dyn AgentLlmClient>,
        tools: Arc<dyn AgentToolDispatcher>,
        store: Arc<dyn AgentSessionStore>,
    ) -> FactoryPolicyBuildFuture;
}

fn forged_factory_policy_session() -> Session {
    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
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
        .expect("metadata serializes");
    session
        .set_build_state(SessionBuildState::default())
        .expect("build state serializes");
    session
}

fn main() {
    let builder = AgentBuilder::new()
        .resume_session(forged_factory_policy_session())
        .with_turn_state_handle(Arc::new(
            meerkat_core::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ));

    let result = block_on(async {
        // SAFETY: this fixture models a downstream crate deliberately opting into
        // unsafe code and synthesizing every public prerequisite. The core bridge
        // must still reject the caller before constructing an agent.
        unsafe {
            exported_agent_factory_policy_build(
                forged_agent_factory_policy_bridge_token(),
                builder,
                Arc::new(NoopClient),
                Arc::new(NoopTools),
                Arc::new(NoopStore),
            )
            .await
        }
    });

    match result {
        Ok(_) => panic!("unsafe downstream finalizer call constructed an agent"),
        Err(error) => {
            let error = error.to_string();
            assert!(
                error.contains("canonical factory bridge token")
                    || error.contains("forged factory bridge token registration"),
                "unsafe downstream finalizer call failed for the wrong reason: {error}"
            );
            println!("unsafe downstream finalizer rejected forged bridge token: {error}");
        }
    }
}

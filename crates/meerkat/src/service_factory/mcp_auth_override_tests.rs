//! A member build's own MCP credential source
//! (`SessionBuildOptions::mcp_auth_resolver_override`): every MCP connection
//! the build opens asks it, never the factory's; without one the factory's
//! source applies; a payload that is not a credential source refuses every
//! credential and never falls back to the factory's.

use super::*;
use async_trait::async_trait;
use meerkat_client::{LlmEvent, LlmRequest};
use meerkat_core::auth::{McpServiceSecretRefusal, McpServiceSecretValues};
use meerkat_core::service::SessionBuildOptions;
use std::collections::{BTreeSet, HashMap};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A model the build needs and no test turn calls.
struct UnusedModel;

#[async_trait]
impl LlmClient for UnusedModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        Box::pin(futures::stream::empty())
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A credential source that counts the service-secret reads of the stdio
/// servers its connections launch, and holds no custody.
#[derive(Default)]
struct CountingSource {
    secret_reads: AtomicUsize,
}

impl CountingSource {
    fn reads(&self) -> usize {
        self.secret_reads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl meerkat_mcp::McpAuthResolver for CountingSource {
    async fn stored_bearer_token(
        &self,
        _target: &crate::McpServerIdentity,
    ) -> Result<Option<String>, crate::McpOAuthError> {
        Ok(None)
    }

    async fn interactive_login(
        &self,
        target: &crate::McpServerIdentity,
        _www_authenticate: Option<&str>,
    ) -> Result<String, crate::McpOAuthError> {
        Err(crate::McpOAuthError::HumanAuthorizationRequired {
            server_name: target.server_name().to_owned(),
        })
    }

    async fn stdio_service_secrets(
        &self,
        _server: &meerkat_core::McpServerConfig,
    ) -> Result<McpServiceSecretValues, McpServiceSecretRefusal> {
        self.secret_reads.fetch_add(1, Ordering::SeqCst);
        Err(McpServiceSecretRefusal::NoCustody)
    }
}

/// A stdio server that declares one service secret, so its launch asks the
/// build's credential source before anything is spawned.
fn secret_server() -> meerkat_core::McpServerConfig {
    let mut server = meerkat_core::McpServerConfig::stdio(
        "vault",
        "/nonexistent/vault-server",
        Vec::new(),
        HashMap::new(),
    );
    if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut server.transport {
        stdio.secret_env = BTreeSet::from(["VAULT_TOKEN".to_string()]);
    }
    server
}

fn request(override_source: Option<Arc<dyn std::any::Any + Send + Sync>>) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "claude-sonnet-4-5".to_string(),
        prompt: "hello".to_string().into(),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            mcp_servers: vec![secret_server()],
            mcp_auth_resolver_override: override_source,
            ..SessionBuildOptions::default()
        }),
        labels: None,
        injected_context: Vec::new(),
    }
}

/// A builder whose factory's own credential source is `factory_source`, and
/// which waits for each build's MCP servers to connect or fail.
fn builder(factory_source: &Arc<CountingSource>) -> FactoryAgentBuilder {
    let factory = AgentFactory::minimal()
        .mcp_auth_resolver(Arc::clone(factory_source) as Arc<dyn meerkat_mcp::McpAuthResolver>);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(UnusedModel));
    builder.wait_for_mcp = true;
    builder
}

async fn build(builder: &FactoryAgentBuilder, request: &CreateSessionRequest) {
    let (event_tx, _event_rx) = mpsc::channel(8);
    let agent = builder
        .build_agent(request, event_tx)
        .await
        .expect("the build succeeds; its failed server is reported to the model");
    drop(agent);
}

#[tokio::test]
async fn a_build_routes_its_mcp_credentials_through_its_own_source() {
    let factory_source = Arc::new(CountingSource::default());
    let builder = builder(&factory_source);

    let member_source = Arc::new(CountingSource::default());
    build(
        &builder,
        &request(Some(crate::encode_mcp_auth_resolver_override_for_service(
            Arc::clone(&member_source) as Arc<dyn meerkat_mcp::McpAuthResolver>,
        ))),
    )
    .await;
    assert!(
        member_source.reads() > 0,
        "the build's own source served its MCP connection"
    );
    assert_eq!(
        factory_source.reads(),
        0,
        "the factory's source was never asked for that build"
    );

    build(&builder, &request(None)).await;
    assert!(
        factory_source.reads() > 0,
        "a build without its own source uses the factory's"
    );

    let before = factory_source.reads();
    build(
        &builder,
        &request(Some(
            Arc::new("not a credential source") as Arc<dyn std::any::Any + Send + Sync>
        )),
    )
    .await;
    assert_eq!(
        factory_source.reads(),
        before,
        "a payload that is not a credential source never falls back to the factory's"
    );
}

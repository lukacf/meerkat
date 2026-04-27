//! Factory-backed SessionAgent and SessionAgentBuilder implementations.
//!
//! Bridges `AgentFactory::build_agent()` into the `SessionAgent`/`SessionAgentBuilder`
//! traits so any surface can create a `SessionService` backed by the standard factory.

use async_trait::async_trait;
use meerkat_core::Config;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ConfigStore;
use meerkat_core::Session;
use meerkat_core::comms::{CommsCommand, PeerDirectoryEntry, SendError, SendReceipt};
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{CreateSessionRequest, SessionError, TurnToolOverlay};
use meerkat_core::types::{
    AssistantBlock, HandlingMode, Message, RenderMetadata, RunResult, SessionId, StopReason, Usage,
};
use meerkat_session::EphemeralSessionService;
use meerkat_session::ephemeral::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "session-store")]
use crate::PersistenceBundle;
use crate::{AgentBuildConfig, AgentFactory, DynAgent};
use meerkat_client::LlmClient;

/// Wrapper around [`DynAgent`] implementing [`SessionAgent`].
pub struct FactoryAgent {
    agent: DynAgent,
    /// Session-context DSL handle, carried through from `SessionRuntimeBindings`
    /// when the factory runs in `SessionOwned` mode. The session task uses it
    /// to fire `AdvanceSessionContext` on every canonical session-truth
    /// mutation (W2-E / issue #264). `None` on `StandaloneEphemeral` builds.
    session_context: Option<Arc<dyn meerkat_core::handles::SessionContextHandle>>,
}

impl FactoryAgent {
    /// Access the underlying agent.
    pub fn agent(&self) -> &DynAgent {
        &self.agent
    }

    /// Access the underlying agent mutably.
    pub fn agent_mut(&mut self) -> &mut DynAgent {
        &mut self.agent
    }

    /// Access the current session for inspection.
    pub fn session(&self) -> &Session {
        self.agent.session()
    }

    /// Send a canonical comms command through the wrapped agent runtime.
    pub async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        let runtime = self
            .agent
            .comms()
            .ok_or_else(|| SendError::Unsupported("comms runtime is not configured".to_string()))?;
        runtime.send(cmd).await
    }

    /// List peers discoverable to this agent runtime.
    pub async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        match self.agent.comms() {
            Some(runtime) => runtime.peers().await,
            None => Vec::new(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgent for FactoryAgent {
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_with_events(prompt, event_tx).await
    }

    async fn run_turn_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
        execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        // handling_mode and render_metadata are runtime-owned semantics.
        // The runtime routes Queue/Steer BEFORE calling the executor, so by
        // the time this method runs the routing decision is already made.
        // Reject if a non-runtime caller attempts to use these — they cannot
        // be honored on the direct path and silent flattening violates §5.
        if handling_mode != HandlingMode::Queue {
            return Err(meerkat_core::error::AgentError::ConfigError(format!(
                "handling_mode {handling_mode:?} requires a runtime-backed surface; direct session-service path supports Queue only",
            )));
        }
        if render_metadata.is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "render_metadata requires a runtime-backed surface; direct session-service path does not support it".to_string(),
            ));
        }
        self.agent.set_runtime_execution_kind(execution_kind);
        self.agent.run_with_events(prompt, event_tx).await
    }

    async fn run_pending_with_events(
        &mut self,
        execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.set_runtime_execution_kind(execution_kind);
        self.agent.run_pending_with_events(event_tx).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.agent.pending_skill_references = refs;
    }

    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .set_flow_tool_overlay(overlay)
            .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))
    }

    fn apply_pending_tool_results(
        &mut self,
        results: Vec<meerkat_core::ToolResult>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        if results.is_empty() {
            return Ok(());
        }
        self.agent
            .session_mut()
            .push(Message::ToolResults { results });
        Ok(())
    }

    fn replace_client(&mut self, client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>) {
        self.agent.replace_client(client);
    }

    fn hot_swap_llm_identity(
        &mut self,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        // Atomically update the live client and the session's durable
        // LLM request policy/identity so subsequent turns run against the
        // new model/provider/provider_params/connection_ref and persisted
        // recovery sees the swap.
        let previous_connection_ref = self
            .agent
            .session()
            .session_metadata()
            .and_then(|metadata| metadata.connection_ref);
        self.agent
            .replace_client_with_request_policy(client, request_policy);
        self.agent
            .rotate_auth_lease_connection_ref(
                previous_connection_ref.as_ref(),
                identity.connection_ref.as_ref(),
            )
            .map_err(|e| {
                meerkat_core::error::AgentError::ConfigError(format!(
                    "failed to rotate auth lease during llm identity hot-swap: {e}"
                ))
            })?;
        if let Some(mut metadata) = self.agent.session().session_metadata() {
            metadata.apply_llm_identity(&identity);
            self.agent
                .session_mut()
                .set_session_metadata(metadata)
                .map_err(|e| {
                    meerkat_core::error::AgentError::ConfigError(format!(
                        "failed to apply hot-swapped llm identity to session metadata: {e}"
                    ))
                })?;
        }
        Ok(())
    }

    fn update_keep_alive(&mut self, keep_alive: bool) {
        if let Some(mut metadata) = self.agent.session().session_metadata() {
            metadata.keep_alive = keep_alive;
            if let Err(e) = self.agent.session_mut().set_session_metadata(metadata) {
                tracing::warn!(error = %e, "failed to update keep_alive in session metadata");
            }
        }
    }

    fn update_mob_tool_authority_context(
        &mut self,
        authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .session_mut()
            .set_mob_tool_authority_context(authority_context)
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update mob tool authority context in session metadata: {err}"
                ))
            })
    }

    fn update_system_prompt(
        &mut self,
        system_prompt: String,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent.session_mut().set_system_prompt(system_prompt);
        Ok(())
    }

    fn stage_external_tool_filter(
        &mut self,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .stage_external_tool_filter(filter)
            .map(|_| ())
            .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))
    }

    fn set_tool_visibility_state(
        &mut self,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        let visibility_state = state.clone().unwrap_or_default();
        self.agent
            .tool_scope()
            .set_visibility_state(visibility_state)
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to replace tool visibility state on live scope: {err}"
                ))
            })?;
        if let Some(state) = state {
            self.agent
                .session_mut()
                .set_tool_visibility_state(state)
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to persist tool visibility state into session metadata: {err}"
                    ))
                })
        } else {
            self.agent
                .session_mut()
                .remove_metadata(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY);
            Ok(())
        }
    }

    async fn dispatch_external_tool_call(
        &mut self,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        self.agent.dispatch_external_tool_call(call).await
    }

    fn sync_system_context_state(&mut self) {
        self.agent.sync_system_context_state_to_session();
    }

    fn cancel(&mut self) {
        self.agent.cancel();
    }

    fn cancel_after_boundary_handle(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        Some(self.agent.cancel_after_boundary_handle())
    }

    fn session_context_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::SessionContextHandle>> {
        self.session_context.as_ref().map(Arc::clone)
    }

    fn session_id(&self) -> SessionId {
        self.agent.session().id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        let s = self.agent.session();
        SessionSnapshot {
            created_at: s.created_at(),
            updated_at: s.updated_at(),
            message_count: s.messages().len(),
            total_tokens: s.total_tokens(),
            usage: s.total_usage(),
            last_assistant_text: s.last_assistant_text(),
        }
    }

    fn execution_snapshot(&self) -> Option<meerkat_core::AgentExecutionSnapshot> {
        self.agent.execution_snapshot()
    }

    fn tool_scope_snapshot(&self) -> Option<meerkat_core::ToolScopeSnapshot> {
        self.agent.tool_scope_snapshot()
    }

    fn visible_tool_defs(&self) -> Vec<meerkat_core::ToolDef> {
        self.agent
            .tool_scope()
            .visible_tools()
            .iter()
            .map(|tool| tool.as_ref().clone())
            .collect()
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.agent.external_tool_surface_snapshot()
    }

    fn session_clone(&self) -> Session {
        self.agent.session_with_system_context_state()
    }

    fn has_pending_boundary(&self) -> bool {
        self.agent.session().has_pending_boundary()
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.agent
            .session_mut()
            .append_system_context_blocks(appends);
    }

    fn append_external_user_content(
        &mut self,
        content: meerkat_core::types::ContentInput,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .session_mut()
            .append_external_user_content(content);
        Ok(())
    }

    fn append_external_assistant_output(
        &mut self,
        blocks: Vec<AssistantBlock>,
        stop_reason: StopReason,
        usage: Usage,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent.session_mut().append_external_assistant_blocks(
            blocks,
            stop_reason,
            usage.clone(),
        );
        self.agent.budget().record_usage(&usage);
        Ok(())
    }

    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
        self.agent.system_context_state()
    }

    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.agent.comms_arc()?.event_injector()
    }

    #[doc(hidden)]
    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.agent.comms_arc()?.interaction_event_injector()
    }

    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.agent.comms_arc()
    }
}

/// Implements [`SessionAgentBuilder`] by delegating to [`AgentFactory::build_agent()`].
pub struct FactoryAgentBuilder {
    factory: AgentFactory,
    config_snapshot: Config,
    #[cfg(not(target_arch = "wasm32"))]
    config_store: Option<Arc<dyn ConfigStore>>,
    /// Optional default LLM client injected into all builds (for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
    /// Optional default tool dispatcher injected when no override is provided.
    /// Used on wasm32 where filesystem-based tool resolution is unavailable.
    pub default_tool_dispatcher: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    /// Optional default session store injected when no override is provided.
    /// Used on wasm32 where filesystem-based stores are unavailable.
    pub default_session_store: Option<Arc<dyn meerkat_core::AgentSessionStore>>,
    /// Default mob tools factory injected into all builds.
    /// Wrapped in `Arc<StdRwLock<...>>` so surfaces can set it after the builder
    /// is consumed into a session service (breaking the circular dependency between
    /// session service construction and mob state creation).
    pub default_mob_tools:
        Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>,
    /// Default scheduler tools injected into all builds.
    ///
    /// Wrapped in `Arc<StdRwLock<...>>` so surfaces can attach scheduler tool
    /// visibility after the builder is consumed into a session service, while
    /// keeping runtime/service ownership in the surface.
    pub default_schedule_tools:
        Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::AgentToolDispatcher>>>>,
    /// Default blob store injected into all builds.
    pub default_blob_store: Option<Arc<dyn meerkat_core::BlobStore>>,
    /// Default image-generation executor injected into all builds.
    pub default_image_generation_executor:
        Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
}

impl FactoryAgentBuilder {
    /// Create a new builder backed by the given factory and config.
    pub fn new(factory: AgentFactory, config: Config) -> Self {
        Self {
            factory,
            config_snapshot: config,
            #[cfg(not(target_arch = "wasm32"))]
            config_store: None,
            default_llm_client: None,
            default_tool_dispatcher: None,
            default_session_store: None,
            default_mob_tools: Arc::new(std::sync::RwLock::new(None)),
            default_schedule_tools: Arc::new(std::sync::RwLock::new(None)),
            default_blob_store: None,
            default_image_generation_executor: None,
        }
    }

    /// Create a new builder that resolves config from a store on each build.
    ///
    /// If the store read fails, the builder falls back to `initial_config`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_with_config_store(
        factory: AgentFactory,
        initial_config: Config,
        config_store: Arc<dyn ConfigStore>,
    ) -> Self {
        Self {
            factory,
            config_snapshot: initial_config,
            config_store: Some(config_store),
            default_llm_client: None,
            default_tool_dispatcher: None,
            default_session_store: None,
            default_mob_tools: Arc::new(std::sync::RwLock::new(None)),
            default_schedule_tools: Arc::new(std::sync::RwLock::new(None)),
            default_blob_store: None,
            default_image_generation_executor: None,
        }
    }

    pub fn with_image_generation_machine(
        mut self,
        machine: Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>,
    ) -> Self {
        self.factory = self.factory.with_image_generation_machine(machine);
        self
    }

    async fn resolve_config(&self) -> Config {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = &self.config_store {
            match store.get().await {
                Ok(config) => return config,
                Err(err) => {
                    tracing::warn!("Failed to read latest config from store: {err}");
                }
            }
        }
        self.config_snapshot.clone()
    }

    /// Get a reference to the factory.
    pub fn factory(&self) -> &AgentFactory {
        &self.factory
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &Config {
        &self.config_snapshot
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgentBuilder for FactoryAgentBuilder {
    type Agent = FactoryAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut build_config = AgentBuildConfig::from_create_session_request(req, event_tx);

        // Inject default LLM client if none provided.
        if build_config.llm_client_override.is_none()
            && let Some(ref client) = self.default_llm_client
        {
            build_config.llm_client_override = Some(client.clone());
        }

        // Inject default tool dispatcher if none provided.
        if build_config.tool_dispatcher_override.is_none()
            && let Some(ref dispatcher) = self.default_tool_dispatcher
        {
            build_config.tool_dispatcher_override = Some(dispatcher.clone());
        }

        // Inject default session store if none provided.
        if build_config.session_store_override.is_none()
            && let Some(ref store) = self.default_session_store
        {
            build_config.session_store_override = Some(store.clone());
        }

        // Inject default mob tools factory if none provided.
        if build_config.mob_tools.is_none()
            && let Some(mob_factory) = self
                .default_mob_tools
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        {
            build_config.mob_tools = Some(mob_factory);
        }

        // Inject default scheduler tools if none provided.
        if build_config.schedule_tools.is_none()
            && let Some(schedule_dispatcher) = self
                .default_schedule_tools
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        {
            build_config.schedule_tools = Some(schedule_dispatcher);
        }

        if build_config.blob_store_override.is_none()
            && let Some(blob_store) = self.default_blob_store.clone()
        {
            build_config.blob_store_override = Some(blob_store);
        }
        if build_config.image_generation_executor_override.is_none()
            && let Some(executor) = self.default_image_generation_executor.clone()
        {
            build_config.image_generation_executor_override = Some(executor);
        }

        let config = self.resolve_config().await;

        // Capture the session_context handle before build_agent consumes the
        // RuntimeBuildMode. Needed so the session task can fire
        // `AdvanceSessionContext` on every session-truth mutation (W2-E).
        let session_context = match req.build.as_ref().map(|opts| &opts.runtime_build_mode) {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                Some(Arc::clone(&bindings.session_context))
            }
            _ => None,
        };

        let agent = self
            .factory
            .build_agent(build_config, &config)
            .await
            .map_err(|e| {
                SessionError::Agent(meerkat_core::error::AgentError::BuildError(e.to_string()))
            })?;

        Ok(FactoryAgent {
            agent,
            session_context,
        })
    }
}

/// Convenience: build an `EphemeralSessionService` backed by `AgentFactory`.
pub fn build_ephemeral_service(
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    let builder = FactoryAgentBuilder::new(factory, config);
    crate::surface::build_embedded_service_from_builder(builder, max_sessions)
}

/// Convenience: build a `PersistentSessionService` backed by `AgentFactory`.
#[cfg(feature = "session-store")]
pub fn build_persistent_service_with_runtime_adapter(
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
    persistence: PersistenceBundle,
) -> (
    meerkat_session::PersistentSessionService<FactoryAgentBuilder>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let builder = FactoryAgentBuilder::new(factory, config);
    crate::surface::build_runtime_backed_service(builder, max_sessions, persistence)
}

/// Convenience: build a `PersistentSessionService` backed by `AgentFactory`.
#[cfg(feature = "session-store")]
pub fn build_persistent_service(
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
    persistence: PersistenceBundle,
) -> meerkat_session::PersistentSessionService<FactoryAgentBuilder> {
    build_persistent_service_with_runtime_adapter(factory, config, max_sessions, persistence).0
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
    use meerkat_core::Config;
    use meerkat_core::comms::InputSource;
    use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
    use meerkat_core::service::SessionBuildOptions;
    use meerkat_core::{
        Provider, ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolResult,
    };
    use meerkat_llm_core::{
        ImageGenerationExecutor, ProviderImageGenerationOutput, ProviderImageGenerationRequest,
    };
    use meerkat_runtime::MeerkatMachine;
    use meerkat_schedule::{MemoryScheduleStore, ScheduleService, ScheduleToolDispatcher};
    use meerkat_session::ephemeral::SessionAgent;
    use meerkat_store::MemoryBlobStore;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    struct MockLlmClient {
        delta: &'static str,
    }

    impl Default for MockLlmClient {
        fn default() -> Self {
            Self { delta: "ok" }
        }
    }

    #[derive(Default)]
    struct CaptureToolClient {
        inner: meerkat_client::TestClient,
        seen_tools: Mutex<Vec<String>>,
    }

    impl CaptureToolClient {
        fn tool_names(&self) -> Vec<String> {
            self.seen_tools.lock().expect("capture lock").clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl LlmClient for CaptureToolClient {
        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
        > {
            *self.seen_tools.lock().expect("capture lock") = request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            self.inner.stream(request)
        }

        fn provider(&self) -> &'static str {
            self.inner.provider()
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            self.inner.health_check().await
        }
    }

    struct FakeImageGenerationExecutor;

    #[async_trait]
    impl ImageGenerationExecutor for FakeImageGenerationExecutor {
        async fn execute_image_generation(
            &self,
            request: ProviderImageGenerationRequest,
        ) -> Result<ProviderImageGenerationOutput, meerkat_llm_core::LlmError> {
            Ok(ProviderImageGenerationOutput {
                operation_id: request.operation_id,
                terminal: meerkat_core::ImageOperationTerminalClass::Failed,
                images: Vec::new(),
                provider_text: None,
                revised_prompt: meerkat_core::RevisedPromptDisposition::NotRequested,
                native_metadata: meerkat_core::ProviderImageMetadata::NotEmitted,
                warnings: Vec::new(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: self.delta.to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RegistryBindingProbe {
        bound: AtomicBool,
        seen_registry: Mutex<Option<Arc<dyn OpsLifecycleRegistry>>>,
        seen_session_id: Mutex<Option<SessionId>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl meerkat_core::AgentToolDispatcher for RegistryBindingProbe {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "noop".to_string(), false).into())
        }

        fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
            meerkat_core::agent::DispatcherCapabilities {
                ops_lifecycle: true,
            }
        }

        fn bind_ops_lifecycle(
            self: Arc<Self>,
            registry: Arc<dyn OpsLifecycleRegistry>,
            owner_bridge_session_id: SessionId,
        ) -> Result<meerkat_core::agent::BindOutcome, meerkat_core::agent::OpsLifecycleBindError>
        {
            self.bound.store(true, Ordering::SeqCst);
            *self.seen_registry.lock().expect("probe lock") = Some(registry);
            *self.seen_session_id.lock().expect("probe lock") = Some(owner_bridge_session_id);
            Ok(meerkat_core::agent::BindOutcome::Bound(self))
        }
    }

    async fn build_factory_agent_with_mock(
        temp: &TempDir,
        mut build_config: AgentBuildConfig,
    ) -> Result<FactoryAgent, String> {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        build_config.llm_client_override = Some(Arc::new(MockLlmClient::default()));
        let agent = factory
            .build_agent(build_config, &Config::default())
            .await
            .map_err(|err| format!("{err}"))?;
        Ok(FactoryAgent {
            agent,
            session_context: None,
        })
    }

    #[tokio::test]
    async fn factory_builder_uses_runtime_session_registry_override() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient { delta: "ok" }));

        let probe = Arc::new(RegistryBindingProbe::default());
        let probe_dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> = probe.clone();
        builder.default_tool_dispatcher = Some(probe_dispatcher);

        let runtime_adapter = MeerkatMachine::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();
        runtime_adapter.register_session(session_id.clone()).await;
        let expected_registry = runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .ok_or_else(|| "missing runtime registry".to_string())?
            as Arc<dyn OpsLifecycleRegistry>;

        let bindings = meerkat_core::SessionRuntimeBindings {
            session_id: session_id.clone(),
            epoch_id: meerkat_core::runtime_epoch::RuntimeEpochId::new(),
            ops_lifecycle: expected_registry.clone(),
            cursor_state: Arc::new(meerkat_core::EpochCursorState::new()),
            tool_visibility_owner: Arc::new(meerkat_core::LocalToolVisibilityOwner::new()),
            turn_state: Arc::new(meerkat_runtime::RuntimeTurnStateHandle::ephemeral()),
            comms_drain: Arc::new(meerkat_runtime::RuntimeCommsDrainHandle::ephemeral()),
            external_tool_surface: Arc::new(
                meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral(),
            ),
            peer_comms: Arc::new(meerkat_runtime::RuntimePeerCommsHandle::ephemeral()),
            session_admission: Arc::new(meerkat_runtime::RuntimeSessionAdmissionHandle::ephemeral()),
            auth_lease: Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::ephemeral()),
            mcp_server_lifecycle: Arc::new(
                meerkat_runtime::RuntimeMcpServerLifecycleHandle::ephemeral(),
            ),
            peer_interaction: Some(Arc::new(
                meerkat_runtime::RuntimePeerInteractionHandle::ephemeral(),
            )),
            session_context: Arc::new(meerkat_runtime::RuntimeSessionContextHandle::ephemeral()),
            session_claim_handle: runtime_adapter.session_claim_handle(),
            interaction_stream: Some(Arc::new(
                meerkat_runtime::RuntimeInteractionStreamHandle::ephemeral(),
            )),
            realtime_product_turn: Arc::new(
                meerkat_runtime::RuntimeRealtimeProductTurnHandle::ephemeral(),
            ),
        };

        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,

            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        let (event_tx, _event_rx) = mpsc::channel(8);
        let agent = builder
            .build_agent(&req, event_tx)
            .await
            .map_err(|err| err.to_string())?;
        drop(agent);

        assert!(
            probe.bound.load(Ordering::SeqCst),
            "dispatcher should receive ops lifecycle binding"
        );
        let seen_registry = probe
            .seen_registry
            .lock()
            .expect("probe lock")
            .clone()
            .ok_or_else(|| "dispatcher did not record registry".to_string())?;
        let seen_session_id = probe
            .seen_session_id
            .lock()
            .expect("probe lock")
            .clone()
            .ok_or_else(|| "dispatcher did not record session id".to_string())?;

        assert!(
            Arc::ptr_eq(&seen_registry, &expected_registry),
            "factory should use runtime adapter's canonical registry, not a fresh fallback"
        );
        assert_eq!(seen_session_id, session_id);

        Ok(())
    }

    fn mock_input_cmd(session_id: &SessionId) -> CommsCommand {
        CommsCommand::Input {
            session_id: session_id.clone(),
            body: "hello".to_string(),
            blocks: None,
            source: InputSource::Rpc,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            stream: meerkat_core::comms::InputStreamMode::None,
            allow_self_session: true,
        }
    }

    #[tokio::test]
    async fn test_factory_agent_send_without_comms_runtime_is_unsupported() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;
        let session_id = agent.session().id().clone();
        let result = agent.send(mock_input_cmd(&session_id)).await;
        assert!(matches!(result, Err(SendError::Unsupported(_))));

        Ok(())
    }

    #[tokio::test]
    async fn test_session_llm_override_is_applied_end_to_end() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient { delta: "default" }));

        let build = SessionBuildOptions {
            llm_client_override: Some(crate::encode_llm_client_override_for_service(Arc::new(
                MockLlmClient { delta: "override" },
            ))),
            ..SessionBuildOptions::default()
        };
        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "ignored".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,

            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };

        let (build_event_tx, _build_event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&req, build_event_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        let (run_event_tx, _run_event_rx) = mpsc::channel(8);
        let result =
            SessionAgent::run_with_events(&mut agent, "hello".to_string().into(), run_event_tx)
                .await
                .map_err(|err| format!("{err}"))?;
        assert_eq!(result.text, "override");
        Ok(())
    }

    #[tokio::test]
    async fn test_default_llm_override_allows_unknown_model_names() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient { delta: "override" }));

        let (event_tx, _event_rx) = mpsc::channel(8);
        let agent = builder
            .build_agent(&make_session_request("mock-model"), event_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        let metadata = agent
            .session()
            .session_metadata()
            .ok_or_else(|| "missing session metadata".to_string())?;
        assert_eq!(metadata.provider, Provider::Other);
        Ok(())
    }

    #[tokio::test]
    async fn factory_builder_uses_default_schedule_tools_on_runtime_backed_resume()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions")).schedule(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        let capture: Arc<CaptureToolClient> = Arc::new(CaptureToolClient::default());
        builder.default_llm_client = Some(capture.clone());
        *builder
            .default_schedule_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(MemoryScheduleStore::default()),
            ))));

        let runtime_adapter = MeerkatMachine::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();
        runtime_adapter.register_session(session_id.clone()).await;
        let bindings = meerkat_core::SessionRuntimeBindings {
            session_id,
            epoch_id: meerkat_core::runtime_epoch::RuntimeEpochId::new(),
            ops_lifecycle: runtime_adapter
                .ops_lifecycle_registry(session.id())
                .await
                .ok_or_else(|| "missing runtime registry".to_string())?,
            cursor_state: Arc::new(meerkat_core::EpochCursorState::new()),
            tool_visibility_owner: Arc::new(meerkat_core::LocalToolVisibilityOwner::new()),
            turn_state: Arc::new(meerkat_runtime::RuntimeTurnStateHandle::ephemeral()),
            comms_drain: Arc::new(meerkat_runtime::RuntimeCommsDrainHandle::ephemeral()),
            external_tool_surface: Arc::new(
                meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral(),
            ),
            peer_comms: Arc::new(meerkat_runtime::RuntimePeerCommsHandle::ephemeral()),
            session_admission: Arc::new(meerkat_runtime::RuntimeSessionAdmissionHandle::ephemeral()),
            auth_lease: Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::ephemeral()),
            mcp_server_lifecycle: Arc::new(
                meerkat_runtime::RuntimeMcpServerLifecycleHandle::ephemeral(),
            ),
            peer_interaction: Some(Arc::new(
                meerkat_runtime::RuntimePeerInteractionHandle::ephemeral(),
            )),
            session_context: Arc::new(meerkat_runtime::RuntimeSessionContextHandle::ephemeral()),
            session_claim_handle: runtime_adapter.session_claim_handle(),
            interaction_stream: Some(Arc::new(
                meerkat_runtime::RuntimeInteractionStreamHandle::ephemeral(),
            )),
            realtime_product_turn: Arc::new(
                meerkat_runtime::RuntimeRealtimeProductTurnHandle::ephemeral(),
            ),
        };

        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&req, event_tx)
            .await
            .map_err(|err| format!("{err}"))?;
        let (run_tx, _run_rx) = mpsc::channel(8);
        SessionAgent::run_with_events(&mut agent, "inspect".to_string().into(), run_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        let tool_names = capture.tool_names();
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_create")
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_list")
        );
        Ok(())
    }

    #[tokio::test]
    async fn factory_builder_wires_generate_image_on_runtime_backed_path() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let runtime_adapter = Arc::new(MeerkatMachine::ephemeral());
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(true)
            .with_image_generation_machine(runtime_adapter.clone());
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        let capture: Arc<CaptureToolClient> = Arc::new(CaptureToolClient::default());
        builder.default_llm_client = Some(capture.clone());
        builder.default_blob_store = Some(Arc::new(MemoryBlobStore::default()));
        builder.default_image_generation_executor = Some(Arc::new(FakeImageGenerationExecutor));

        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime_adapter
            .prepare_bindings(session_id)
            .await
            .map_err(|err| format!("prepare bindings: {err}"))?;
        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&req, event_tx)
            .await
            .map_err(|err| format!("{err}"))?;
        let (run_tx, _run_rx) = mpsc::channel(8);
        SessionAgent::run_with_events(&mut agent, "inspect".to_string().into(), run_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        let tool_names = capture.tool_names();
        assert!(tool_names.iter().any(|name| name == "generate_image"));
        Ok(())
    }

    #[tokio::test]
    async fn factory_agent_execution_snapshot_forwards_core_state() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;

        let snapshot = SessionAgent::execution_snapshot(&agent)
            .ok_or_else(|| "factory agent should expose execution snapshot".to_string())?;

        assert_eq!(
            snapshot.loop_state,
            meerkat_core::state::LoopState::CallingLlm
        );
        assert_eq!(
            snapshot.turn_phase,
            meerkat_core::turn_execution_authority::TurnPhase::Ready
        );
        assert_eq!(snapshot.active_run_id, None);
        assert_eq!(snapshot.applied_cursor, 0);

        Ok(())
    }

    #[tokio::test]
    async fn factory_agent_tool_scope_snapshot_forwards_core_state() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;

        let snapshot = SessionAgent::tool_scope_snapshot(&agent)
            .ok_or_else(|| "factory agent should expose tool-scope snapshot".to_string())?;

        assert_eq!(snapshot.base_filter, meerkat_core::ToolFilter::All);
        assert_eq!(
            snapshot.active_external_filter,
            meerkat_core::ToolFilter::All
        );
        assert_eq!(
            snapshot.staged_external_filter,
            meerkat_core::ToolFilter::All
        );
        assert_eq!(snapshot.active_revision, meerkat_core::ToolScopeRevision(0));
        assert_eq!(snapshot.staged_revision, meerkat_core::ToolScopeRevision(0));
        assert_eq!(snapshot.known_base_names, snapshot.visible_names);

        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn factory_agent_external_tool_surface_snapshot_forwards_core_state() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let mut router = meerkat_mcp::McpRouter::new();
        router.stage_add(meerkat_core::McpServerConfig::stdio(
            "planner",
            "/bin/echo",
            Vec::<String>::new(),
            std::collections::HashMap::new(),
        ));
        let dispatcher = Arc::new(meerkat_mcp::McpRouterAdapter::new(router))
            as Arc<dyn meerkat_core::AgentToolDispatcher>;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                tool_dispatcher_override: Some(dispatcher),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;

        let snapshot = SessionAgent::external_tool_surface_snapshot(&agent).ok_or_else(|| {
            "factory agent should expose external-tool surface snapshot".to_string()
        })?;

        assert_eq!(
            snapshot.phase,
            meerkat_core::ExternalToolSurfaceGlobalPhase::Operating
        );
        assert_eq!(snapshot.snapshot_epoch, 0);
        assert_eq!(snapshot.snapshot_aligned_epoch, 0);
        assert_eq!(snapshot.entries.len(), 1);

        let entry = &snapshot.entries[0];
        assert_eq!(entry.surface_id, "planner");
        assert!(!entry.visible);
        assert_eq!(
            entry.base_state,
            meerkat_core::ExternalToolSurfaceBaseState::Absent
        );
        assert_eq!(
            entry.pending_op,
            meerkat_core::ExternalToolSurfacePendingOp::None
        );
        assert_eq!(
            entry.staged_op,
            meerkat_core::ExternalToolSurfaceStagedOp::Add
        );
        assert_eq!(entry.staged_intent_sequence, 1);
        assert_eq!(entry.pending_task_sequence, 0);
        assert_eq!(entry.pending_lineage_sequence, 0);
        assert_eq!(entry.inflight_call_count, 0);
        assert_eq!(
            entry.last_delta_operation,
            meerkat_core::ExternalToolSurfaceDeltaOperation::None
        );
        assert_eq!(
            entry.last_delta_phase,
            meerkat_core::ExternalToolSurfaceDeltaPhase::None
        );

        Ok(())
    }

    #[test]
    fn test_session_build_options_preserve_keep_alive_flag() {
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.apply_session_build_options(&SessionBuildOptions {
            keep_alive: true,
            ..SessionBuildOptions::default()
        });
        assert!(build.keep_alive);
    }

    // ── Per-agent provider resolution via realm config ──
    //
    // These tests verify the architectural invariant that per-agent provider
    // agnosticity works via Config — the same code path used by WASM when
    // default_llm_client is NOT set.

    /// Helper: build a request with the given model (deferred — no initial turn).
    fn make_session_request(model: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            model: model.to_string(),
            prompt: "test".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,

            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        }
    }

    fn make_session_request_with_connection(model: &str, binding: &str) -> CreateSessionRequest {
        let mut req = make_session_request(model);
        let build = meerkat_core::service::SessionBuildOptions {
            connection_ref: Some(meerkat_core::ConnectionRef {
                realm: meerkat_core::RealmId::parse("default").expect("valid test realm"),
                binding: meerkat_core::BindingId::parse(binding).expect("valid test binding"),
                profile: None,
            }),
            ..Default::default()
        };
        req.build = Some(build);
        req
    }

    /// When realm config is populated and default_llm_client is None,
    /// build_agent() resolves the correct provider per-model from the Config map.
    /// This is the WASM code path after the default_llm_client fix.
    #[tokio::test]
    async fn test_config_api_keys_resolve_different_providers_per_model() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        let mut section = meerkat_core::RealmConfigSection::from_inline_api_keys(&[
            ("anthropic", "test-anthropic-key"),
            ("openai", "test-openai-key"),
            ("gemini", "test-gemini-key"),
        ]);
        // First-entry default_binding = anthropic by the helper's
        // sorted order. All three bindings are resolvable by provider.
        // The test asserts multi-provider dispatch, so we need a
        // binding per provider.
        let _ = &mut section;
        config.realm.insert("default".to_string(), section);
        let builder = FactoryAgentBuilder::new(factory, config);

        let (tx1, _rx1) = mpsc::channel(8);
        builder
            .build_agent(
                &make_session_request_with_connection("claude-sonnet-4-5", "default_anthropic"),
                tx1,
            )
            .await
            .map_err(|e| format!("anthropic model should build: {e}"))?;

        let (tx2, _rx2) = mpsc::channel(8);
        builder
            .build_agent(
                &make_session_request_with_connection("gpt-5.4", "default_openai"),
                tx2,
            )
            .await
            .map_err(|e| format!("openai model should build: {e}"))?;

        let (tx3, _rx3) = mpsc::channel(8);
        builder
            .build_agent(
                &make_session_request_with_connection("gemini-3-flash-preview", "default_gemini"),
                tx3,
            )
            .await
            .map_err(|e| format!("gemini model should build: {e}"))?;

        Ok(())
    }

    #[tokio::test]
    async fn test_default_llm_client_takes_precedence_over_config_keys() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let mut builder = FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(MockLlmClient {
            delta: "from-default",
        }));

        let (tx, _rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&make_session_request("claude-sonnet-4-5"), tx)
            .await
            .map_err(|e| format!("build failed: {e}"))?;

        let (run_tx, _run_rx) = mpsc::channel(8);
        let result = SessionAgent::run_with_events(&mut agent, "hello".into(), run_tx)
            .await
            .map_err(|e| format!("run failed: {e}"))?;
        assert_eq!(result.text, "from-default");
        Ok(())
    }

    #[tokio::test]
    async fn test_unknown_model_prefix_fails_even_with_config_keys() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        let section =
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[("anthropic", "test-key")]);
        config.realm.insert("default".to_string(), section);
        let builder = FactoryAgentBuilder::new(factory, config);

        let (tx, _rx) = mpsc::channel(8);
        let result = builder
            .build_agent(&make_session_request("llama-3.1-70b"), tx)
            .await;
        assert!(result.is_err(), "unknown model prefix should fail");
        Ok(())
    }
}

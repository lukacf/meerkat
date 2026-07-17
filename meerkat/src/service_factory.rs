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
    AssistantBlock, HandlingMode, Message, RunResult, SessionId, StopReason, Usage,
};
use meerkat_session::EphemeralSessionService;
use meerkat_session::ephemeral::{
    ObservedSessionTailKind, SessionAgent, SessionAgentBuilder, SessionAgentTurnInput,
    SessionSnapshot,
};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "session-store")]
use crate::PersistenceBundle;
use crate::{AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent};
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

fn build_agent_error_to_session_error(error: BuildAgentError) -> SessionError {
    match error {
        #[cfg(feature = "comms")]
        BuildAgentError::SessionIdentityInUse(session_id) => SessionError::Agent(
            meerkat_core::error::AgentError::SessionIdentityInUse(session_id),
        ),
        BuildAgentError::LlmClient(error) => {
            SessionError::build_llm_identity_unresolvable(error.to_string())
        }
        other => SessionError::Agent(meerkat_core::error::AgentError::BuildError(
            other.to_string(),
        )),
    }
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

    async fn reconcile_runtime_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .reconcile_runtime_compaction_projections(intents)
            .await
    }

    async fn settle_inflight_sticky_model_fallback(
        &mut self,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent.settle_inflight_sticky_model_fallback().await
    }

    async fn abort_uncommitted_compaction_projections(
        &mut self,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent.abort_uncommitted_compaction_projections().await
    }

    fn take_runtime_terminal_failure_witness(
        &mut self,
    ) -> Result<Option<meerkat_core::TurnErrorMetadata>, meerkat_core::error::AgentError> {
        self.agent.take_runtime_terminal_failure_witness()
    }

    async fn run_turn_with_events(
        &mut self,
        input: SessionAgentTurnInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        // handling_mode and render_metadata are runtime-owned semantics.
        // The runtime routes Queue/Steer BEFORE calling the executor, so by
        // the time this method runs the routing decision is already made.
        // Reject if a non-runtime caller attempts to use these — they cannot
        // be honored on the direct path and silent flattening violates §5.
        if input.handling_mode != HandlingMode::Queue {
            return Err(meerkat_core::error::AgentError::ConfigError(format!(
                "handling_mode {:?} requires a runtime-backed surface; direct session-service path supports Queue only",
                input.handling_mode,
            )));
        }
        if input.render_metadata.is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "render_metadata requires a runtime-backed surface; direct session-service path does not support it".to_string(),
            ));
        }
        self.agent.set_runtime_execution_kind(input.execution_kind);
        if input.typed_turn_appends.is_empty()
            && input.transcript_identity.is_none()
            && input.injected_context.is_empty()
        {
            self.agent.run_with_events(input.prompt, event_tx).await
        } else {
            self.agent
                .run_with_events_and_typed_turn_appends(
                    input.prompt,
                    input.typed_turn_appends,
                    input.injected_context,
                    input.transcript_identity,
                    event_tx,
                )
                .await
        }
    }

    async fn run_pending_with_events(
        &mut self,
        transcript_identity: Option<meerkat_core::types::TranscriptMessageIdentity>,
        execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.set_runtime_execution_kind(execution_kind);
        self.agent
            .set_active_transcript_identity(transcript_identity);
        self.agent.run_pending_with_events(event_tx).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.agent.pending_skill_references = refs;
    }

    fn set_turn_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .set_turn_tool_overlay(overlay)
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
            .push(Message::tool_results(results));
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
        self.agent
            .hot_swap_llm_identity(client, identity, request_policy)
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
        let mut build_state = self
            .agent
            .session()
            .try_build_state()
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to restore session build state for system prompt update: {err}"
                ))
            })?
            .ok_or_else(|| {
                meerkat_core::error::AgentError::InternalError(
                    "session is missing build state for system prompt update".to_string(),
                )
            })?;
        self.agent
            .session_mut()
            .set_system_prompt_with_source(
                system_prompt.clone(),
                meerkat_core::session_durable_config_authority::SessionSystemPromptSource::DirectMutation,
            )
            .map_err(|err| {
                meerkat_core::error::AgentError::ConfigError(format!(
                    "system prompt update was rejected by durable-config authority: {err}"
                ))
            })?;
        // A deferred first-turn override changes the canonical base itself.
        // Keep the exact bytes and their typed intent together so live
        // open/refresh never has to reconstruct either fact from transcript
        // shape or stale create-time policy.
        build_state.system_prompt = meerkat_core::SystemPromptOverride::Set(system_prompt.clone());
        build_state.assembled_system_prompt = Some(system_prompt);
        self.agent
            .session_mut()
            .set_build_state(build_state)
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to persist canonical system prompt update: {err}"
                ))
            })?;
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
            let authorized_state = self
                .agent
                .tool_scope()
                .authorized_visibility_state()
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to authorize tool visibility state for persistence: {err}"
                    ))
                })?;
            debug_assert_eq!(authorized_state.as_state(), &state);
            self.agent
                .session_mut()
                .set_tool_visibility_state(authorized_state)
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to persist tool visibility state into session metadata: {err}"
                    ))
                })
        } else {
            let authorized_state = self
                .agent
                .tool_scope()
                .authorized_visibility_state()
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to authorize default tool visibility state for persistence: {err}"
                    ))
                })?;
            debug_assert_eq!(
                authorized_state.as_state(),
                &meerkat_core::SessionToolVisibilityState::default()
            );
            self.agent
                .session_mut()
                .set_tool_visibility_state(authorized_state)
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to persist default tool visibility state into session metadata: {err}"
                    ))
                })
        }
    }

    async fn dispatch_external_tool_call(
        &mut self,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        self.agent.dispatch_external_tool_call(call).await
    }

    async fn dispatch_external_tool_call_with_timeout_policy(
        &mut self,
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        self.agent
            .dispatch_external_tool_call_with_timeout_policy(call, timeout_policy)
            .await
    }

    fn sync_system_context_state(&mut self) -> Result<(), meerkat_core::SystemContextStateError> {
        self.agent.sync_system_context_state_to_session()
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn sync_session_from_durable_snapshot(
        &mut self,
        mut session: Session,
    ) -> Result<(), meerkat_core::error::AgentError> {
        if session.id() != self.agent.session().id() {
            return Err(meerkat_core::error::AgentError::InternalError(format!(
                "durable snapshot session id {} does not match live session {}",
                session.id(),
                self.agent.session().id()
            )));
        }

        let system_context_state = session
            .try_system_context_state()
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to restore durable system-context state during live session sync: {err}"
                ))
            })?
            .unwrap_or_default();
        let deferred_turn_state = session.try_deferred_turn_state().map_err(|err| {
            meerkat_core::error::AgentError::InternalError(format!(
                "failed to restore durable deferred-turn state during live session sync: {err}"
            ))
        })?;
        if let Some(state) = deferred_turn_state {
            session.set_deferred_turn_state(state).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize restored durable deferred-turn state during live session sync: {err}"
                ))
            })?;
        }
        let visibility_state = session
            .tool_visibility_state()
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to decode durable tool visibility state during live session sync: {err}"
                ))
            })?
            .unwrap_or_default();
        self.agent
            .tool_scope()
            .set_visibility_state(visibility_state)
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to synchronize live tool visibility state from durable session: {err}"
                ))
            })?;
        *self.agent.session_mut() = session;

        let state_handle = self.agent.system_context_state();
        state_handle
            .replace_from_generated_restore(system_context_state)
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "generated system-context authority rejected durable session sync: {err}"
                ))
            })?;
        Ok(())
    }

    fn cancel(&mut self) {
        self.agent.cancel();
    }

    fn cancel_after_boundary_handle(&self) -> Option<meerkat_core::CancelAfterBoundarySender> {
        Some(self.agent.cancel_after_boundary_handle())
    }

    fn turn_state_handle(&self) -> Option<Arc<dyn meerkat_core::TurnStateHandle>> {
        self.agent.turn_state_handle()
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

    fn execution_snapshot(
        &self,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, meerkat_core::SnapshotProjectionError>
    {
        self.agent.execution_snapshot()
    }

    fn tool_scope_snapshot(&self) -> Option<meerkat_core::ToolScopeSnapshot> {
        self.agent.tool_scope_snapshot()
    }

    fn visible_tool_defs(&self) -> Vec<meerkat_core::ToolDef> {
        self.agent.visible_tool_defs()
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.agent.external_tool_surface_snapshot()
    }

    fn session_clone(&self) -> Result<Session, meerkat_core::SystemContextStateError> {
        self.agent.session_with_system_context_state()
    }

    fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
        self.agent
            .session()
            .session_metadata()
            .map(|metadata| metadata.llm_identity())
    }

    fn observed_session_tail(&self) -> ObservedSessionTailKind {
        meerkat_core::pending_continuation::observe_session_tail(self.agent.session().messages())
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.agent
            .session_mut()
            .append_system_context_blocks(appends);
        let state = self
            .agent
            .session()
            .system_context_state()
            .unwrap_or_default();
        let state_handle = self.agent.system_context_state();
        if let Err(err) = state_handle.replace_from_generated_restore(state) {
            tracing::warn!(
                error = %err,
                "generated system-context authority rejected runtime context restore"
            );
        }
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

    fn append_realtime_transcript_event(
        &mut self,
        event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, meerkat_core::error::AgentError> {
        let outcome = self
            .agent
            .session_mut()
            .append_realtime_transcript_event(event);
        for materialized in &outcome.materialized_messages {
            if let meerkat_core::RealtimeTranscriptMaterializedMessage::Assistant {
                usage, ..
            } = materialized
            {
                self.agent.budget().record_usage(usage);
            }
        }
        Ok(outcome)
    }

    fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
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
/// Parent-chain inheritance inputs for [`FactoryAgentBuilder`].
///
/// Holds the realm config source (ancestors) and the head realm whose durable
/// document the surface owns. Kept as one value so the builder cannot be left
/// with a source but no head (or vice versa). Surfaces that learn their realm
/// source after the builder is consumed (e.g. the RPC `SessionRuntime`) set it
/// through the shared slot on [`FactoryAgentBuilder::realm_inheritance`].
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct RealmInheritance {
    source: Arc<dyn meerkat_core::RealmConfigSource>,
    head: meerkat_core::connection::RealmId,
}

#[cfg(not(target_arch = "wasm32"))]
impl RealmInheritance {
    /// Build the inheritance inputs from a realm config `source` (supplies the
    /// ancestor chain + the implicit `global` tail) and the `head` realm whose
    /// durable config the surface owns.
    pub fn new(
        source: Arc<dyn meerkat_core::RealmConfigSource>,
        head: meerkat_core::connection::RealmId,
    ) -> Self {
        Self { source, head }
    }

    /// Compose the parent chain + `global` tail over `head_config` (the surface's
    /// authoritative head config), producing the effective config the agent
    /// build AND the model hot-swap / reconfigure path consume.
    ///
    /// Fail-closed: a compose error is returned, never masked behind the raw head.
    pub async fn compose_over(
        &self,
        head_config: Config,
    ) -> Result<Config, meerkat_core::ConfigError> {
        meerkat_core::EffectiveConfigReader::new(self.source.clone())
            .effective_config_over_head(&self.head, head_config)
            .await
    }
}

pub struct FactoryAgentBuilder {
    factory: AgentFactory,
    config_snapshot: Config,
    #[cfg(not(target_arch = "wasm32"))]
    config_store: Option<Arc<dyn ConfigStore>>,
    /// Realm parent-chain inheritance slot.
    ///
    /// When populated, [`Self::resolve_config`] folds the head realm's durable
    /// config (the live `config_store`, or the `config_snapshot` when no store is
    /// configured) with its ancestor chain so top-level fields (models, mcp,
    /// hooks, skills, limits) inherit from parent realms on every agent build —
    /// not just the auth-binding path.
    ///
    /// A shared slot (rather than a plain field) so surfaces that learn their
    /// realm source after the builder is consumed into a session service — the
    /// RPC `SessionRuntime` sets it from `set_realm_config_source` — can populate
    /// it through the same `Arc` the live builder reads.
    #[cfg(not(target_arch = "wasm32"))]
    pub realm_inheritance: Arc<std::sync::RwLock<Option<RealmInheritance>>>,
    /// Optional default LLM client injected into all builds (for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
    /// Optional default wrapper applied to every final agent-facing LLM client.
    pub default_agent_llm_client_decorator:
        Arc<std::sync::RwLock<Option<meerkat_core::AgentLlmClientDecorator>>>,
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
    /// Default WorkGraph tools injected into all builds.
    pub default_workgraph_tools:
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
            #[cfg(not(target_arch = "wasm32"))]
            realm_inheritance: Arc::new(std::sync::RwLock::new(None)),
            default_llm_client: None,
            default_agent_llm_client_decorator: Arc::new(std::sync::RwLock::new(None)),
            default_tool_dispatcher: None,
            default_session_store: None,
            default_mob_tools: Arc::new(std::sync::RwLock::new(None)),
            default_schedule_tools: Arc::new(std::sync::RwLock::new(None)),
            default_workgraph_tools: Arc::new(std::sync::RwLock::new(None)),
            default_blob_store: None,
            default_image_generation_executor: None,
        }
    }

    /// Create a new builder that resolves config from a store on each build.
    ///
    /// A store read failure is propagated as a typed error on build; the
    /// builder never serves the stale `initial_config` behind a store failure.
    /// `initial_config` is the snapshot returned by [`Self::config`] and is used
    /// only when no config store is configured.
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
            realm_inheritance: Arc::new(std::sync::RwLock::new(None)),
            default_llm_client: None,
            default_agent_llm_client_decorator: Arc::new(std::sync::RwLock::new(None)),
            default_tool_dispatcher: None,
            default_session_store: None,
            default_mob_tools: Arc::new(std::sync::RwLock::new(None)),
            default_schedule_tools: Arc::new(std::sync::RwLock::new(None)),
            default_workgraph_tools: Arc::new(std::sync::RwLock::new(None)),
            default_blob_store: None,
            default_image_generation_executor: None,
        }
    }

    /// Attach realm parent-chain inheritance to this builder.
    ///
    /// `source` supplies ancestor realm documents (parent chain + the implicit
    /// `global` tail); `head` is the realm whose durable config is owned by the
    /// `config_store` passed to [`Self::new_with_config_store`]. Has no effect
    /// unless a config store is also configured: with no store, builds serve the
    /// in-memory `config_snapshot` unchanged.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_realm_inheritance(
        self,
        source: Arc<dyn meerkat_core::RealmConfigSource>,
        head: meerkat_core::connection::RealmId,
    ) -> Self {
        *self
            .realm_inheritance
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(RealmInheritance::new(source, head));
        self
    }

    pub fn with_image_generation_machine(
        mut self,
        machine: Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>,
    ) -> Self {
        self.factory = self.factory.with_image_generation_machine(machine);
        self
    }

    /// Resolve the effective config for this build.
    ///
    /// When a config store is configured, the latest durable config is read
    /// from it and a read failure is propagated as a typed error — the builder
    /// never serves a stale `config_snapshot` behind a store failure. The
    /// in-memory `config_snapshot` is returned only when no store is configured.
    async fn resolve_config(&self) -> Result<Config, SessionError> {
        // The head config is the surface's authoritative document: the live
        // `config_store` when one is configured (a read failure is propagated,
        // never masked behind a stale snapshot), otherwise the in-memory
        // `config_snapshot`.
        #[cfg(not(target_arch = "wasm32"))]
        let head_config = if let Some(store) = &self.config_store {
            store.get().await.map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::ConfigError(format!(
                    "failed to read latest config from store: {err}"
                )))
            })?
        } else {
            self.config_snapshot.clone()
        };
        #[cfg(target_arch = "wasm32")]
        let head_config = self.config_snapshot.clone();

        // When realm inheritance is configured, fold the head realm's config with
        // its ancestor chain so top-level fields (models, mcp, hooks, skills,
        // limits) inherit on every build — not just the auth-binding path. The
        // head config keeps coming from the surface's store/snapshot; inheritance
        // only ADDS ancestor docs, so an inherited entry is never durably
        // flattened into the child realm's document.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inheritance = self
                .realm_inheritance
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(inheritance) = inheritance {
                let reader = meerkat_core::EffectiveConfigReader::new(inheritance.source.clone());
                return reader
                    .effective_config_over_head(&inheritance.head, head_config)
                    .await
                    .map_err(|err| {
                        SessionError::Agent(meerkat_core::error::AgentError::ConfigError(format!(
                            "failed to compose effective config for realm '{}': {err}",
                            inheritance.head
                        )))
                    });
            }
        }
        Ok(head_config)
    }

    /// Get a reference to the factory.
    pub fn factory(&self) -> &AgentFactory {
        &self.factory
    }

    /// Clone the canonical head-config source used by runtime-owned consumers.
    ///
    /// Store-backed builders share the exact store that [`Self::resolve_config`]
    /// reads on every build. Snapshot-only builders lower their immutable
    /// snapshot into a private memory store so downstream runtime components can
    /// consume one uniform [`ConfigStore`] seam without inventing a second live
    /// source of truth.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) fn runtime_config_store(&self) -> Arc<dyn ConfigStore> {
        self.config_store.clone().unwrap_or_else(|| {
            Arc::new(meerkat_core::MemoryConfigStore::new(
                self.config_snapshot.clone(),
                meerkat_models::canonical(),
            ))
        })
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

    async fn abort_absent_session_compaction_stages(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(all(feature = "memory-store-session", not(target_arch = "wasm32")))]
        {
            use meerkat_core::memory::{MemoryOwner, MemoryStore};

            let memory_dir = self.factory.store_path.join("memory");
            let memory_store_exists = memory_dir.try_exists().map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to inspect canonical memory store at {} before session materialization: {error}",
                    memory_dir.display()
                )))
            })?;
            if !memory_store_exists {
                // No durable backend exists, so an empty runtime outbox has no
                // stage owner to reconcile. Avoid creating a memory database
                // for sessions whose memory capability is disabled.
                return Ok(());
            }

            let store = meerkat_memory::HnswMemoryStore::open(&memory_dir).map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to open canonical memory store at {} before session materialization: {error}",
                    memory_dir.display()
                )))
            })?;
            let receipt = store
                .reconcile_compaction_stages(
                    &MemoryOwner::canonical_session(session_id.clone()),
                    &[],
                )
                .await
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to reconcile empty runtime compaction authority for absent session {session_id}: {error}"
                    )))
                })?;
            tracing::debug!(
                %session_id,
                aborted_orphans = receipt.aborted_orphans,
                retained_committed = receipt.retained_committed,
                "reconciled durable compaction stages before SessionTask materialization"
            );
            return Ok(());
        }

        #[cfg(not(all(feature = "memory-store-session", not(target_arch = "wasm32"))))]
        {
            let _ = session_id;
            // This concrete builder has no durable staged-memory backend in
            // this build, so the empty outbox is already fully reconciled.
            Ok(())
        }
    }

    async fn model_supports_inline_video(
        &self,
        identity: &meerkat_core::SessionLlmIdentity,
    ) -> Option<bool> {
        // A config-store read failure yields `None` (capability unknown) rather
        // than a stale-snapshot assertion: we never claim inline-video support
        // from a config we could not freshly read.
        self.resolve_config()
            .await
            .ok()?
            .model_registry(meerkat_models::canonical())
            .ok()
            .and_then(|registry| registry.profile_for_provider(identity.provider, &identity.model))
            .map(|profile| profile.inline_video)
    }

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

        if build_config.agent_llm_client_decorator.is_none()
            && let Some(decorator) = self
                .default_agent_llm_client_decorator
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        {
            build_config.agent_llm_client_decorator = Some(decorator);
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

        if build_config.workgraph_tools.is_none()
            && let Some(workgraph_dispatcher) = self
                .default_workgraph_tools
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        {
            build_config.workgraph_tools = Some(workgraph_dispatcher);
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

        let config = self.resolve_config().await?;

        // Capture the session_context handle before build_agent consumes the
        // RuntimeBuildMode. Needed so the session task can fire
        // `AdvanceSessionContext` on every session-truth mutation (W2-E).
        let session_context = match req.build.as_ref().map(|opts| &opts.runtime_build_mode) {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                Some(Arc::clone(bindings.session_context()))
            }
            _ => None,
        };

        let agent = self
            .factory
            .build_agent(build_config, &config)
            .await
            .map_err(build_agent_error_to_session_error)?;

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
fn set_default_workgraph_tools_from_persistence(
    builder: &FactoryAgentBuilder,
    persistence: &PersistenceBundle,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(manifest) = persistence.manifest() {
        let service = crate::WorkGraphService::with_scope(
            persistence.workgraph_store(),
            manifest.realm.as_str().to_owned(),
            crate::WorkNamespace::default(),
        );
        crate::surface::set_default_workgraph_tools(
            builder,
            Some(Arc::new(crate::WorkGraphToolSurface::new(service))),
        );
    }
}

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
    set_default_workgraph_tools_from_persistence(&builder, &persistence);
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
    use meerkat_core::service::{SessionBuildOptions, SessionService};
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[test]
    fn llm_client_build_failure_keeps_typed_session_cause() {
        let error = build_agent_error_to_session_error(BuildAgentError::LlmClient(
            meerkat_client::FactoryError::ClientCreationFailed(
                "backend diagnostic that callers must not classify by text".to_string(),
            ),
        ));

        assert!(error.is_build_llm_identity_unresolvable());
    }

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
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

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

        fn provider(&self) -> meerkat_core::Provider {
            self.inner.provider()
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            self.inner.health_check().await
        }
    }

    struct FakeImageGenerationExecutor;

    #[cfg(not(target_arch = "wasm32"))]
    fn self_hosted_inline_video_config(inline_video: bool) -> Config {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            "local".to_string(),
            meerkat_core::SelfHostedServerConfig {
                transport: meerkat_core::SelfHostedTransport::OpenAiCompatible,
                base_url: "http://127.0.0.1:11434".to_string(),
                api_style: meerkat_core::SelfHostedApiStyle::Responses,
            },
        );
        config.self_hosted.models.insert(
            "video-alias".to_string(),
            meerkat_core::SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "video-model".to_string(),
                display_name: "Video Alias".to_string(),
                family: "video-family".to_string(),
                tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
                context_window: None,
                max_output_tokens: None,
                vision: true,
                image_tool_results: true,
                inline_video,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: None,
            },
        );
        config
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn inline_video_capability_reads_current_config_store() {
        let initial_config = self_hosted_inline_video_config(false);
        let current_config = self_hosted_inline_video_config(true);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(current_config, meerkat_models::canonical()),
        );
        let builder = FactoryAgentBuilder::new_with_config_store(
            AgentFactory::minimal(),
            initial_config,
            store,
        );
        let identity = meerkat_core::SessionLlmIdentity {
            model: "video-alias".to_string(),
            provider: Provider::SelfHosted,
            self_hosted_server_id: Some("local".to_string()),
            provider_params: None,
            auth_binding: None,
        };

        assert_eq!(
            builder.model_supports_inline_video(&identity).await,
            Some(true)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct FailingConfigStore;

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait]
    impl meerkat_core::ConfigStore for FailingConfigStore {
        async fn get(&self) -> Result<Config, meerkat_core::config::ConfigError> {
            Err(meerkat_core::config::ConfigError::InternalError(
                "store unavailable".to_string(),
            ))
        }
        async fn set(&self, _config: Config) -> Result<(), meerkat_core::config::ConfigError> {
            Ok(())
        }
        async fn patch(
            &self,
            _delta: meerkat_core::config::ConfigDelta,
        ) -> Result<Config, meerkat_core::config::ConfigError> {
            Err(meerkat_core::config::ConfigError::InternalError(
                "store unavailable".to_string(),
            ))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn build_agent_fails_closed_on_config_store_read_error() {
        let initial_config = self_hosted_inline_video_config(false);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(FailingConfigStore);
        let mut builder = FactoryAgentBuilder::new_with_config_store(
            AgentFactory::minimal(),
            initial_config,
            store,
        );
        builder.default_llm_client = Some(Arc::new(MockLlmClient::default()));
        let (event_tx, _event_rx) = mpsc::channel(8);
        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "video-alias".to_string(),
            prompt: "fail closed on stale config".to_string().into(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        };

        let result = builder.build_agent(&req, event_tx).await;
        // `FactoryAgent` is not `Debug`, so match the error out rather than
        // `expect_err` (which would require the `Ok` type to be `Debug`).
        let Err(err) = result else {
            panic!("build must fail closed on config-store read error");
        };
        assert!(
            err.to_string()
                .contains("failed to read latest config from store"),
            "expected a typed config-store error, got: {err}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn session_service_live_identity_matches_factory_resolved_metadata() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, self_hosted_inline_video_config(true));
        builder.default_llm_client = Some(Arc::new(MockLlmClient::default()));
        let service = EphemeralSessionService::new(builder, 10);

        let result = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "video-alias".to_string(),
                prompt: "defer identity parity".to_string().into(),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .map_err(|err| err.to_string())?;

        let live_identity = service
            .live_session_llm_identity(&result.session_id)
            .await
            .map_err(|err| err.to_string())?;
        assert_eq!(live_identity.model, "video-alias");
        assert_eq!(live_identity.provider, Provider::SelfHosted);
        assert_eq!(
            live_identity.self_hosted_server_id.as_deref(),
            Some("local")
        );

        let exported = service
            .export_session(&result.session_id)
            .await
            .map_err(|err| err.to_string())?;
        let metadata = exported
            .session_metadata()
            .ok_or_else(|| "missing session metadata".to_string())?;
        assert_eq!(metadata.llm_identity(), live_identity);

        Ok(())
    }

    #[async_trait]
    impl ImageGenerationExecutor for FakeImageGenerationExecutor {
        async fn execute_image_generation(
            &self,
            request: ProviderImageGenerationRequest,
        ) -> Result<ProviderImageGenerationOutput, meerkat_llm_core::LlmError> {
            Ok(ProviderImageGenerationOutput {
                operation_id: request.operation_id,
                terminal_observation:
                    meerkat_core::ImageProviderTerminalObservation::ExecutionFailed,
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
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

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

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    struct CountingAgentLlmClient {
        inner: Arc<dyn meerkat_core::AgentLlmClient>,
        stream_calls: Arc<AtomicUsize>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl meerkat_core::AgentLlmClient for CountingAgentLlmClient {
        async fn stream_response(
            &self,
            messages: &[meerkat_core::Message],
            tools: &[Arc<meerkat_core::ToolDef>],
            max_tokens: u32,
            temperature: Option<f32>,
            provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            self.stream_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .stream_response(messages, tools, max_tokens, temperature, provider_params)
                .await
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.inner.provider()
        }

        fn model(&self) -> &str {
            self.inner.model()
        }

        fn compile_schema(
            &self,
            output_schema: &meerkat_core::OutputSchema,
        ) -> Result<meerkat_core::CompiledSchema, meerkat_core::SchemaError> {
            self.inner.compile_schema(output_schema)
        }
    }

    fn counting_agent_llm_client_decorator(
        constructions: Arc<AtomicUsize>,
        stream_calls: Arc<AtomicUsize>,
    ) -> meerkat_core::AgentLlmClientDecorator {
        Arc::new(move |client| {
            constructions.fetch_add(1, Ordering::SeqCst);
            Arc::new(CountingAgentLlmClient {
                inner: client,
                stream_calls: Arc::clone(&stream_calls),
            })
        })
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

    fn session_with_raw_metadata(
        session: Session,
        key: &'static str,
        value: serde_json::Value,
    ) -> Session {
        let mut raw = serde_json::to_value(session).expect("session should serialize");
        raw.get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .expect("session metadata should be an object")
            .insert(key.to_string(), value);
        serde_json::from_value(raw).expect("session should deserialize with raw metadata")
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn factory_agent_sync_rejects_malformed_deferred_turn_state() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let mut agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;
        let durable = session_with_raw_metadata(
            agent.session().clone(),
            meerkat_core::SESSION_DEFERRED_TURN_STATE_KEY,
            serde_json::json!("not-a-deferred-turn-state"),
        );

        let err = SessionAgent::sync_session_from_durable_snapshot(&mut agent, durable)
            .expect_err("malformed deferred-turn state must fail closed");

        assert!(
            err.to_string().contains("deferred-turn state"),
            "unexpected error: {err}"
        );
        assert!(
            agent.session().try_deferred_turn_state().unwrap().is_none(),
            "failed sync must not install raw deferred-turn metadata into the live session"
        );
        Ok(())
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
        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| format!("prepare bindings: {err}"))?;
        let expected_registry = Arc::clone(bindings.ops_lifecycle());

        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,

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
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "ignored".to_string().into(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,

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
    async fn factory_builder_default_agent_llm_client_decorator_wraps_default_llm_client()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient { delta: "default" }));
        let constructions = Arc::new(AtomicUsize::new(0));
        let stream_calls = Arc::new(AtomicUsize::new(0));
        *builder
            .default_agent_llm_client_decorator
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(counting_agent_llm_client_decorator(
                Arc::clone(&constructions),
                Arc::clone(&stream_calls),
            ));

        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&make_session_request("mock-model"), event_tx)
            .await
            .map_err(|err| format!("{err}"))?;
        let (run_tx, _run_rx) = mpsc::channel(8);
        let result = SessionAgent::run_with_events(&mut agent, "hello".to_string().into(), run_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        assert_eq!(result.text, "default");
        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
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
        let bindings = runtime_adapter
            .prepare_bindings(session_id)
            .await
            .map_err(|err| format!("prepare bindings: {err}"))?;

        let req = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
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
        SessionAgent::run_turn_with_events(
            &mut agent,
            SessionAgentTurnInput {
                prompt: "inspect".to_string().into(),
                injected_context: Vec::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
                typed_turn_appends: Vec::new(),
                transcript_identity: None,
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            },
            run_tx,
        )
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
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
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
        SessionAgent::run_turn_with_events(
            &mut agent,
            SessionAgentTurnInput {
                prompt: "inspect".to_string().into(),
                injected_context: Vec::new(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
                typed_turn_appends: Vec::new(),
                transcript_identity: None,
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            },
            run_tx,
        )
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
            .map_err(|err| format!("snapshot should project: {err}"))?
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
        let runtime_adapter = MeerkatMachine::ephemeral();
        let session = Session::new();
        let bindings = runtime_adapter
            .prepare_bindings(session.id().clone())
            .await
            .map_err(|err| format!("prepare bindings: {err}"))?;
        let mut router = meerkat_mcp::McpRouter::new_with_surface_handle(Arc::clone(
            bindings.external_tool_surface(),
        ));
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "planner",
                "/bin/echo",
                Vec::<String>::new(),
                std::collections::HashMap::new(),
            ))
            .map_err(|error| error.to_string())?;
        let dispatcher = Arc::new(meerkat_mcp::McpRouterAdapter::new(router))
            as Arc<dyn meerkat_core::AgentToolDispatcher>;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                tool_dispatcher_override: Some(dispatcher),
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
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

    /// M2 acceptance: an embedder attaches an MCP stdio server to a
    /// factory-built agent DECLARATIVELY (`mcp_servers` build option) — no
    /// hand-wired router, no ephemeral-handle incantation — and the built
    /// agent exposes the MCP surface through the composed dispatcher chain.
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn factory_builds_declarative_mcp_servers_into_the_dispatcher() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                mcp_servers: vec![meerkat_core::McpServerConfig::stdio(
                    "planner",
                    "/bin/echo",
                    Vec::<String>::new(),
                    std::collections::HashMap::new(),
                )],
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;

        let snapshot = SessionAgent::external_tool_surface_snapshot(&agent).ok_or_else(|| {
            "declarative mcp_servers must surface through the composed dispatcher".to_string()
        })?;
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.surface_id == "planner"),
            "the declared MCP server must be staged on the session surface: {snapshot:?}"
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
            injected_context: Vec::new(),
            model: model.to_string(),
            prompt: "test".to_string().into(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,

            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        }
    }

    fn make_session_request_with_connection(model: &str, binding: &str) -> CreateSessionRequest {
        let mut req = make_session_request(model);
        let build = meerkat_core::service::SessionBuildOptions {
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("default").expect("valid test realm"),
                binding: meerkat_core::BindingId::parse(binding).expect("valid test binding"),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            }),
            ..Default::default()
        };
        req.build = Some(build);
        req
    }

    #[tokio::test]
    async fn deferred_first_turn_prompt_update_refreshes_canonical_assembled_bytes()
    -> Result<(), String> {
        let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient::default()));
        let mut request = make_session_request("claude-sonnet-4-5");
        request.system_prompt =
            meerkat_core::SystemPromptOverride::Set("create-time prompt".to_string());
        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&request, event_tx)
            .await
            .map_err(|error| format!("build failed: {error}"))?;

        SessionAgent::update_system_prompt(&mut agent, "deferred override".to_string())
            .map_err(|error| format!("prompt update failed: {error}"))?;

        let build_state = agent
            .session()
            .build_state()
            .ok_or_else(|| "updated agent lost session build state".to_string())?;
        assert_eq!(
            build_state.system_prompt,
            meerkat_core::SystemPromptOverride::Set("deferred override".to_string())
        );
        assert_eq!(
            build_state.assembled_system_prompt.as_deref(),
            Some("deferred override")
        );
        assert!(matches!(
            agent.session().messages().first(),
            Some(Message::System(system)) if system.content == "deferred override"
        ));
        Ok(())
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
                &make_session_request_with_connection("gemini-3.5-flash", "default_gemini"),
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

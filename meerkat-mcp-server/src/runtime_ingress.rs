use async_trait::async_trait;
#[cfg(feature = "comms")]
use meerkat::surface::configure_peer_ingress;
use meerkat::{
    CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService, RunResult, Session,
    SessionServiceControlExt,
    surface::{
        BUILD_ONLY_RECOVERY_OVERRIDE_ERROR, CONTEXT_ONLY_MATERIALIZATION_METADATA_ERROR,
        CONTEXT_ONLY_TURN_METADATA_ERROR, SurfaceRuntimeMaterializeError,
        SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides, build_recovered_session,
        has_build_only_turn_overrides, has_context_only_materialization_metadata,
        has_context_only_unapplied_turn_metadata,
        materialization_recovery_overrides_from_runtime_turn, materialize_session,
        recovery_overrides_from_runtime_turn, rematerialize_session_for_runtime_turn,
    },
};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::error::AgentError;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunPrimitive, RuntimeTurnMetadata,
};
use meerkat_core::service::{SessionError, SessionService, StartTurnRequest};
use meerkat_core::types::SessionId;
use meerkat_core::{ConfigRuntime, EventEnvelope, PendingSystemContextAppend};
use meerkat_mcp::McpRouterAdapter;
use meerkat_runtime::completion::CompletionHandle;
use meerkat_runtime::{AcceptOutcome, Input, MeerkatMachine};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};

type RequestEventTx = mpsc::Sender<EventEnvelope<AgentEvent>>;

pub(crate) type SharedMcpRuntimeSessions =
    Arc<RwLock<HashMap<SessionId, Arc<McpRuntimeSessionState>>>>;
pub(crate) type SharedMcpAdapters = Arc<Mutex<HashMap<String, Arc<McpRouterAdapter>>>>;

pub(crate) struct McpRuntimeIngressResources {
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    pub runtime_adapter: Arc<MeerkatMachine>,
    pub config_runtime: Arc<ConfigRuntime>,
    pub realm_id: meerkat_core::connection::RealmId,
    pub instance_id: Option<String>,
    pub backend: String,
    pub mcp_adapters: SharedMcpAdapters,
    pub runtime_sessions: SharedMcpRuntimeSessions,
}

#[derive(Clone)]
pub(crate) struct McpRuntimeIngressContext {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    config_runtime: Arc<ConfigRuntime>,
    realm_id: meerkat_core::connection::RealmId,
    instance_id: Option<String>,
    backend: String,
    mcp_adapters: SharedMcpAdapters,
    runtime_sessions: SharedMcpRuntimeSessions,
}

impl McpRuntimeIngressContext {
    pub(crate) fn new(resources: McpRuntimeIngressResources) -> Self {
        Self {
            service: resources.service,
            runtime_adapter: resources.runtime_adapter,
            config_runtime: resources.config_runtime,
            realm_id: resources.realm_id,
            instance_id: resources.instance_id,
            backend: resources.backend,
            mcp_adapters: resources.mcp_adapters,
            runtime_sessions: resources.runtime_sessions,
        }
    }

    async fn runtime_session_state(&self, session_id: &SessionId) -> Arc<McpRuntimeSessionState> {
        if let Some(existing) = self.runtime_sessions.read().await.get(session_id).cloned() {
            if self.runtime_adapter.session_has_executor(session_id).await {
                return existing;
            }
            existing.clear_queued_turns().await;
            let executor = Box::new(McpSessionRuntimeExecutor::new(
                self.clone(),
                session_id.clone(),
                existing.clone(),
            ));
            self.runtime_adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
            return existing;
        }

        let state = Arc::new(McpRuntimeSessionState::default());
        let executor = Box::new(McpSessionRuntimeExecutor::new(
            self.clone(),
            session_id.clone(),
            state.clone(),
        ));
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        state
    }

    pub(crate) async fn ensure_session(
        &self,
        session_id: &SessionId,
    ) -> Arc<McpRuntimeSessionState> {
        self.runtime_session_state(session_id).await
    }

    pub(crate) async fn current_callback_tools(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn AgentToolDispatcher>> {
        let state = self
            .runtime_sessions
            .read()
            .await
            .get(session_id)
            .cloned()?;
        state.callback_tools().await
    }

    pub(crate) async fn configure_session(
        &self,
        session_id: &SessionId,
        callback_tools: Option<Arc<dyn AgentToolDispatcher>>,
        replace_runtime_attachment: bool,
    ) -> Arc<McpRuntimeSessionState> {
        if replace_runtime_attachment {
            self.runtime_adapter.unregister_session(session_id).await;
        }
        let state = self.runtime_session_state(session_id).await;
        state.set_callback_tools(callback_tools).await;
        state
    }

    pub(crate) async fn clear_session(&self, session_id: &SessionId) {
        self.runtime_adapter.unregister_session(session_id).await;
        if let Some(state) = self.runtime_sessions.write().await.remove(session_id) {
            state.clear_queued_turns().await;
        }
        self.mcp_adapters
            .lock()
            .await
            .remove(&session_id.to_string());
    }

    async fn clear_session_after_materialize_error(&self, session_id: &SessionId) {
        match self.service.export_live_session(session_id).await {
            Ok(_) => {}
            Err(SessionError::NotFound { .. }) => {
                self.clear_session(session_id).await;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "preserving MCP runtime ingress state after materialize error because live session state is unknown"
                );
            }
        }
    }

    pub(crate) async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<RequestEventTx>,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), meerkat_runtime::RuntimeDriverError>
    {
        let state = self.runtime_session_state(session_id).await;
        let requested_input_id = input.id().clone();
        let mut context_input_id = requested_input_id.clone();
        let queued_context = state
            .enqueue_turn_context(requested_input_id.clone(), event_tx)
            .await;

        let (outcome, handle) = match self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if queued_context {
                    let _ = state.discard_turn_context(&requested_input_id).await;
                }
                return Err(error);
            }
        };

        if queued_context {
            let canonical_input_id = match &outcome {
                AcceptOutcome::Accepted { input_id, .. } => Some(input_id),
                AcceptOutcome::Deduplicated { existing_id, .. } => Some(existing_id),
                _ => None,
            };
            if let Some(input_id) = canonical_input_id
                && input_id != &requested_input_id
            {
                let rekeyed = state
                    .rekey_turn_context(&requested_input_id, input_id.clone())
                    .await;
                if rekeyed {
                    context_input_id = input_id.clone();
                }
            }
        }

        if handle.is_none() && queued_context {
            let _ = state.discard_turn_context(&context_input_id).await;
        }

        Ok((outcome, handle))
    }

    async fn external_tools_for_session(
        &self,
        session_id: &SessionId,
        state: &McpRuntimeSessionState,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, SessionError> {
        let callback_tools = state.callback_tools().await;
        let mcp_tools = self
            .mcp_adapters
            .lock()
            .await
            .get(&session_id.to_string())
            .cloned()
            .map(|adapter| adapter as Arc<dyn AgentToolDispatcher>);
        crate::compose_external_tool_dispatchers(callback_tools, mcp_tools)
            .map_err(|error| SessionError::Agent(AgentError::InternalError(error)))
    }

    async fn materialize_with_state(
        &self,
        session: Session,
        request: CreateSessionRequest,
        keep_alive: bool,
        state: Arc<McpRuntimeSessionState>,
    ) -> Result<RunResult, SessionError> {
        let session_id = session.id().clone();
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        let context = self.clone();
        let result = Box::pin(materialize_session(
            &self.service,
            &self.runtime_adapter,
            session,
            request,
            move |runtime_session_id| {
                Box::new(McpSessionRuntimeExecutor::new(
                    context,
                    runtime_session_id,
                    state,
                ))
            },
        ))
        .await;

        match result {
            Ok(result) => {
                #[cfg(feature = "comms")]
                configure_peer_ingress(
                    &self.runtime_adapter,
                    &self.service,
                    &result.session_id,
                    keep_alive,
                )
                .await;
                Ok(result)
            }
            Err(error) => {
                self.clear_session_after_materialize_error(&session_id)
                    .await;
                Err(surface_materialize_session_error(error))
            }
        }
    }

    async fn rematerialize_persisted_session(
        &self,
        session_id: &SessionId,
        state: Arc<McpRuntimeSessionState>,
        recovery_overrides: SurfaceSessionRecoveryOverrides,
    ) -> Result<SessionId, SessionError> {
        let session = self
            .service
            .load_authoritative_session(session_id)
            .await?
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.clone(),
            })?;
        let current_snapshot = self.config_runtime.get().await.ok();
        let current_generation = current_snapshot
            .as_ref()
            .map(|snapshot| snapshot.generation)
            .or_else(|| {
                session
                    .session_metadata()
                    .as_ref()
                    .and_then(|meta| meta.config_generation)
            });
        let external_tools = self.external_tools_for_session(session_id, &state).await?;
        let recovered = build_recovered_session(
            session.clone(),
            &recovery_overrides,
            SurfaceSessionRecoveryContext {
                llm_client_override: None,
                external_tools,
                realm_id: Some(self.realm_id.to_string()),
                instance_id: self.instance_id.clone(),
                backend: Some(self.backend.clone()),
                config_generation: current_generation,
                runtime_owned_recovery: true,
                ..Default::default()
            },
        )
        .map_err(surface_recovery_session_error)?;
        let keep_alive = recovered.keep_alive;
        let result = Box::pin(self.materialize_with_state(
            session,
            recovered.into_deferred_create_request(),
            keep_alive,
            state,
        ))
        .await?;
        Ok(result.session_id)
    }

    async fn rematerialize_live_session_for_turn_metadata(
        &self,
        session_id: &SessionId,
        state: Arc<McpRuntimeSessionState>,
        recovery_overrides: SurfaceSessionRecoveryOverrides,
        keep_alive_override: Option<bool>,
    ) -> Result<SessionId, SessionError> {
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        let current_generation = self
            .config_runtime
            .get()
            .await
            .ok()
            .map(|snapshot| snapshot.generation);
        let external_tools = self.external_tools_for_session(session_id, &state).await?;
        let context = self.clone();
        let state_for_executor = state.clone();
        let (result, _rematerialized_keep_alive) =
            Box::pin(rematerialize_session_for_runtime_turn(
                &self.service,
                &self.runtime_adapter,
                session_id,
                recovery_overrides,
                SurfaceSessionRecoveryContext {
                    llm_client_override: None,
                    external_tools,
                    realm_id: Some(self.realm_id.to_string()),
                    instance_id: self.instance_id.clone(),
                    backend: Some(self.backend.clone()),
                    config_generation: current_generation,
                    runtime_owned_recovery: true,
                    ..Default::default()
                },
                move |runtime_session_id| {
                    Box::new(McpSessionRuntimeExecutor::new(
                        context,
                        runtime_session_id,
                        state_for_executor,
                    ))
                },
            ))
            .await
            .map_err(surface_materialize_session_error)?;
        #[cfg(feature = "comms")]
        let keep_alive = keep_alive_override.unwrap_or(_rematerialized_keep_alive);
        #[cfg(feature = "comms")]
        configure_peer_ingress(
            &self.runtime_adapter,
            &self.service,
            &result.session_id,
            keep_alive,
        )
        .await;
        Ok(result.session_id)
    }
}

fn surface_materialize_session_error(error: SurfaceRuntimeMaterializeError) -> SessionError {
    match error {
        SurfaceRuntimeMaterializeError::Session(error) => error,
        SurfaceRuntimeMaterializeError::Recovery(
            meerkat::surface::SurfaceSessionRecoveryError::InvalidOverride(message),
        ) => SessionError::Unsupported(message),
        other => SessionError::Agent(AgentError::InternalError(other.to_string())),
    }
}

fn surface_recovery_session_error(
    error: meerkat::surface::SurfaceSessionRecoveryError,
) -> SessionError {
    match error {
        meerkat::surface::SurfaceSessionRecoveryError::InvalidOverride(message) => {
            SessionError::Unsupported(message)
        }
        other => SessionError::Agent(AgentError::InternalError(other.to_string())),
    }
}

#[derive(Default)]
struct RuntimeTurnQueue {
    entries: HashMap<meerkat_core::lifecycle::InputId, QueuedTurnContext>,
}

struct QueuedTurnContext {
    event_tx: RequestEventTx,
}

pub(crate) struct McpRuntimeSessionState {
    queued_turns: Mutex<RuntimeTurnQueue>,
    callback_tools: RwLock<Option<Arc<dyn AgentToolDispatcher>>>,
}

impl Default for McpRuntimeSessionState {
    fn default() -> Self {
        Self {
            queued_turns: Mutex::new(RuntimeTurnQueue::default()),
            callback_tools: RwLock::new(None),
        }
    }
}

impl McpRuntimeSessionState {
    async fn set_callback_tools(&self, callback_tools: Option<Arc<dyn AgentToolDispatcher>>) {
        *self.callback_tools.write().await = callback_tools;
    }

    async fn callback_tools(&self) -> Option<Arc<dyn AgentToolDispatcher>> {
        self.callback_tools.read().await.clone()
    }

    async fn enqueue_turn_context(
        &self,
        input_id: meerkat_core::lifecycle::InputId,
        event_tx: Option<RequestEventTx>,
    ) -> bool {
        let Some(event_tx) = event_tx else {
            return false;
        };
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns
            .entries
            .insert(input_id, QueuedTurnContext { event_tx });
        true
    }

    async fn take_turn_context_for_inputs(
        &self,
        contributing_input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Option<QueuedTurnContext> {
        let mut queued_turns = self.queued_turns.lock().await;
        let mut selected = None;
        for input_id in contributing_input_ids {
            if let Some(context) = queued_turns.entries.remove(input_id) {
                selected = Some(context);
            }
        }
        selected
    }

    async fn rekey_turn_context(
        &self,
        from_input_id: &meerkat_core::lifecycle::InputId,
        to_input_id: meerkat_core::lifecycle::InputId,
    ) -> bool {
        if from_input_id == &to_input_id {
            return true;
        }
        let mut queued_turns = self.queued_turns.lock().await;
        if queued_turns.entries.contains_key(&to_input_id) {
            queued_turns.entries.remove(from_input_id);
            return true;
        }
        let Some(context) = queued_turns.entries.remove(from_input_id) else {
            return false;
        };
        queued_turns.entries.insert(to_input_id, context);
        true
    }

    async fn discard_turn_context(&self, input_id: &meerkat_core::lifecycle::InputId) -> bool {
        self.queued_turns
            .lock()
            .await
            .entries
            .remove(input_id)
            .is_some()
    }

    async fn clear_queued_turns(&self) {
        self.queued_turns.lock().await.entries.clear();
    }
}

struct McpSessionRuntimeExecutor {
    context: McpRuntimeIngressContext,
    session_id: SessionId,
    state: Arc<McpRuntimeSessionState>,
}

struct McpSessionRuntimeBoundaryHandle {
    context: McpRuntimeIngressContext,
    session_id: SessionId,
}

#[async_trait]
impl CoreExecutorBoundaryHandle for McpSessionRuntimeBoundaryHandle {
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .cancel_after_boundary(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

struct McpSessionRuntimeInterruptHandle {
    context: McpRuntimeIngressContext,
    session_id: SessionId,
}

#[async_trait]
impl CoreExecutorInterruptHandle for McpSessionRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .interrupt(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

impl McpSessionRuntimeExecutor {
    fn new(
        context: McpRuntimeIngressContext,
        session_id: SessionId,
        state: Arc<McpRuntimeSessionState>,
    ) -> Self {
        Self {
            context,
            session_id,
            state,
        }
    }
}

fn render_context_append_text(content: &CoreRenderable) -> String {
    match content {
        CoreRenderable::Text { text } => text.clone(),
        CoreRenderable::Blocks { blocks } => meerkat_core::types::text_content(blocks),
        CoreRenderable::Json { value } => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        CoreRenderable::Reference { uri, label } => match label {
            Some(label) if !label.trim().is_empty() => format!("[Reference] {label} ({uri})"),
            _ => format!("[Reference] {uri}"),
        },
        _ => String::new(),
    }
}

fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            text: render_context_append_text(&append.content),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
        })
        .collect()
}

fn should_apply_context_without_turn(primitive: &RunPrimitive) -> bool {
    primitive.is_context_only_apply_without_turn()
}

async fn apply_runtime_turn(
    context: &McpRuntimeIngressContext,
    state: &Arc<McpRuntimeSessionState>,
    session_id: &SessionId,
    run_id: meerkat_core::lifecycle::RunId,
    primitive: &RunPrimitive,
) -> Result<CoreApplyOutput, SessionError> {
    if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
        return Err(SessionError::Agent(AgentError::InternalError(
            reason.to_string(),
        )));
    }
    let recovery_overrides = recovery_overrides_from_runtime_turn(
        primitive.turn_metadata(),
        primitive.build_only_overrides(),
    );

    // Context-only staged primitives may land directly as runtime
    // system-context appends, but terminal peer responses carry a typed apply
    // intent that requires a requester reaction turn.
    if primitive.is_context_only_apply_without_turn() {
        if has_build_only_turn_overrides(&recovery_overrides) {
            return Err(SessionError::Unsupported(
                meerkat::surface::BUILD_ONLY_RECOVERY_OVERRIDE_ERROR.to_string(),
            ));
        }
        if has_context_only_materialization_metadata(&recovery_overrides) {
            return Err(SessionError::Unsupported(
                CONTEXT_ONLY_MATERIALIZATION_METADATA_ERROR.to_string(),
            ));
        }
        if has_context_only_unapplied_turn_metadata(&recovery_overrides) {
            return Err(SessionError::Unsupported(
                CONTEXT_ONLY_TURN_METADATA_ERROR.to_string(),
            ));
        }
        let RunPrimitive::StagedInput(staged) = primitive else {
            unreachable!("context-only apply without turn only matches staged primitives");
        };
        let appends = pending_system_context_appends(&staged.context_appends);
        return match context
            .service
            .apply_runtime_context_appends_with_boundary(
                session_id,
                run_id.clone(),
                appends.clone(),
                primitive.apply_boundary(),
                staged.contributing_input_ids.clone(),
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(SessionError::NotFound { .. }) => {
                Box::pin(context.rematerialize_persisted_session(
                    session_id,
                    state.clone(),
                    recovery_overrides.clone(),
                ))
                .await
                .map(|_| ())?;
                context
                    .service
                    .apply_runtime_context_appends_with_boundary(
                        session_id,
                        run_id,
                        appends,
                        primitive.apply_boundary(),
                        staged.contributing_input_ids.clone(),
                    )
                    .await
            }
            Err(error) => Err(error),
        };
    }

    let prompt = primitive.extract_content_input();
    let boundary = primitive.apply_boundary();
    let contributing_input_ids = primitive.contributing_input_ids().to_vec();
    let pre_turn_context_appends = match primitive {
        RunPrimitive::StagedInput(staged)
            if primitive.is_peer_response_terminal_context_and_run() =>
        {
            pending_system_context_appends(&staged.context_appends)
        }
        _ => Vec::new(),
    };
    let queued_context = state
        .take_turn_context_for_inputs(&contributing_input_ids)
        .await;
    let event_tx = queued_context
        .as_ref()
        .map(|context| context.event_tx.clone());

    let turn_request = StartTurnRequest {
        prompt: prompt.clone(),
        system_prompt: primitive
            .build_only_overrides()
            .and_then(|overrides| overrides.system_prompt.clone()),
        event_tx: event_tx.clone(),
        pre_turn_context_appends: pre_turn_context_appends.clone(),
        turn_metadata: primitive.turn_metadata().cloned(),
    };

    let build_only_requires_recovery = primitive
        .build_only_overrides()
        .is_some_and(|overrides| overrides.requires_materialization_recovery());
    let live_materialization_requires_recovery = primitive
        .turn_metadata()
        .is_some_and(|metadata| metadata.has_materialization_recovery_fields());
    if live_materialization_requires_recovery && !build_only_requires_recovery {
        let materialization_overrides =
            materialization_recovery_overrides_from_runtime_turn(primitive.turn_metadata());
        let keep_alive_override = match primitive
            .turn_metadata()
            .and_then(|metadata| metadata.keep_alive.as_ref())
        {
            Some(meerkat_core::lifecycle::run_primitive::TurnMetadataOverride::Set(_)) => {
                Some(true)
            }
            Some(meerkat_core::lifecycle::run_primitive::TurnMetadataOverride::Clear) => {
                Some(false)
            }
            None => None,
        };
        Box::pin(context.rematerialize_live_session_for_turn_metadata(
            session_id,
            state.clone(),
            materialization_overrides,
            keep_alive_override,
        ))
        .await
        .map(|_| ())?;
    }
    let live_result = if build_only_requires_recovery {
        Err(SessionError::NotFound {
            id: session_id.clone(),
        })
    } else {
        context
            .service
            .apply_runtime_turn(
                session_id,
                run_id.clone(),
                turn_request,
                boundary,
                contributing_input_ids.clone(),
            )
            .await
    };

    match live_result {
        Ok(output) => Ok(output),
        Err(SessionError::NotFound { .. }) => {
            Box::pin(context.rematerialize_persisted_session(
                session_id,
                state.clone(),
                recovery_overrides.clone(),
            ))
            .await
            .map(|_| ())?;
            context
                .service
                .apply_runtime_turn_outcome(
                    session_id,
                    run_id,
                    StartTurnRequest {
                        prompt,
                        system_prompt: None,
                        event_tx,
                        pre_turn_context_appends,
                        turn_metadata: recovery_overrides.turn_metadata.clone(),
                    },
                    boundary,
                    contributing_input_ids,
                )
                .await
        }
        Err(error) => Err(error),
    }
}

#[async_trait]
impl CoreExecutor for McpSessionRuntimeExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(McpSessionRuntimeBoundaryHandle {
            context: self.context.clone(),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(McpSessionRuntimeInterruptHandle {
            context: self.context.clone(),
            session_id: self.session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        Box::pin(apply_runtime_turn(
            &self.context,
            &self.state,
            &self.session_id,
            run_id,
            &primitive,
        ))
        .await
        .map_err(|error| CoreExecutorError::apply_failed_runtime_turn(error.to_string()))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .cancel_after_boundary(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        let discard_result = self
            .context
            .service
            .discard_live_session(&self.session_id)
            .await;
        self.context.clear_session(&self.session_id).await;
        match discard_result {
            Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
            Err(error) => Err(CoreExecutorError::control_failed_runtime(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat::surface::build_runtime_backed_service;
    use meerkat::{AgentFactory, Config, PersistenceBundle};
    use meerkat_client::TestClient;
    use meerkat_core::lifecycle::run_primitive::{
        PeerResponseTerminalApplyIntent, RunApplyBoundary, RuntimeBuildOnlyTurnOverrides,
        RuntimeExecutionKind, RuntimeTurnMetadata, StagedRunInput,
    };
    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
    use meerkat_core::{MemoryConfigStore, RealmId};
    use meerkat_store::{JsonlStore, MemoryBlobStore, SessionStoreError};
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn build_test_context(temp: &tempfile::TempDir) -> McpRuntimeIngressContext {
        let session_store = Arc::new(JsonlStore::new(temp.path().join("sessions")));
        session_store.init().await.expect("init jsonl store");
        let session_store: Arc<dyn meerkat::SessionStore> = session_store;
        build_test_context_with_store(temp, session_store).await
    }

    async fn build_test_context_with_store(
        temp: &tempfile::TempDir,
        session_store: Arc<dyn meerkat::SessionStore>,
    ) -> McpRuntimeIngressContext {
        let persistence =
            PersistenceBundle::new(session_store, None, Arc::new(MemoryBlobStore::new()));

        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, runtime_adapter) = build_runtime_backed_service(builder, 4, persistence);
        let config_store = Arc::new(MemoryConfigStore::new(Config::default()));

        McpRuntimeIngressContext::new(McpRuntimeIngressResources {
            service: Arc::new(service),
            runtime_adapter,
            config_runtime: Arc::new(ConfigRuntime::new(
                config_store,
                temp.path().join("config-state.json"),
            )),
            realm_id: RealmId::parse("mcp-runtime-test").expect("valid realm id"),
            instance_id: None,
            backend: "test".to_string(),
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            runtime_sessions: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    struct FailingSaveSessionStore {
        inner: meerkat::MemoryStore,
        fail_save: AtomicBool,
    }

    impl FailingSaveSessionStore {
        fn new() -> Self {
            Self {
                inner: meerkat::MemoryStore::new(),
                fail_save: AtomicBool::new(false),
            }
        }

        fn set_fail_save(&self, fail: bool) {
            self.fail_save.store(fail, Ordering::Release);
        }
    }

    #[async_trait::async_trait]
    impl meerkat::SessionStore for FailingSaveSessionStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            if self.fail_save.load(Ordering::Acquire) {
                return Err(SessionStoreError::Internal(
                    "forced save failure".to_string(),
                ));
            }
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }
    }

    fn context_append() -> ConversationContextAppend {
        ConversationContextAppend {
            key: "peer_response_terminal:analyst:req-1".to_string(),
            content: CoreRenderable::Text {
                text: "done".to_string(),
            },
        }
    }

    fn deferred_create_request(prompt: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "test".to_string(),
            prompt: prompt.to_string().into(),
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            build: None,
            labels: None,
        }
    }

    #[tokio::test]
    async fn mcp_runtime_ingress_rejects_malformed_terminal_peer_response_intent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let state = Arc::new(McpRuntimeSessionState::default());
        let session_id = SessionId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: Vec::new(),
            context_appends: vec![ConversationContextAppend {
                key: "peer_response_terminal:analyst-rt:req-invalid".to_string(),
                content: CoreRenderable::Text {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] invalid".to_string(),
                },
            }],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                peer_response_terminal_apply_intent: Some(
                    PeerResponseTerminalApplyIntent::AppendContextAndRun,
                ),
                ..Default::default()
            }),
            build_only_overrides: None,
        });

        let error = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect_err("malformed terminal peer-response intent must be rejected");

        assert!(
            error.to_string().contains("requires RunStart boundary"),
            "unexpected rejection reason: {error}"
        );
    }

    #[test]
    fn runtime_rematerialization_uses_primitive_turn_metadata_for_recovery() {
        let source = include_str!("runtime_ingress.rs");
        let empty_recovery = concat!("&SurfaceSessionRecoveryOverrides", "::default(),");
        assert!(
            !source.contains(empty_recovery),
            "runtime rematerialization must not rebuild with empty recovery overrides"
        );
        assert!(
            source.contains("recovery_overrides_from_runtime_turn(")
                && source.contains("primitive.build_only_overrides()")
                && source.contains(
                    "build_recovered_session(\n            session.clone(),\n            &recovery_overrides,"
                ),
            "runtime rematerialization must pass primitive turn metadata and build-only overrides into recovery"
        );
    }

    #[tokio::test]
    async fn mcp_context_only_runtime_apply_rejects_build_only_overrides() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let state = Arc::new(McpRuntimeSessionState::default());
        let session_id = SessionId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![context_append()],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
            build_only_overrides: Some(RuntimeBuildOnlyTurnOverrides {
                system_prompt: Some("context-only system".to_string()),
                ..Default::default()
            }),
        });

        let error = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect_err("context-only runtime applies must not drop build-only overrides");

        assert!(
            error
                .to_string()
                .contains(BUILD_ONLY_RECOVERY_OVERRIDE_ERROR),
            "unexpected rejection reason: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_context_only_runtime_apply_rejects_materialization_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let state = Arc::new(McpRuntimeSessionState::default());
        let session_id = SessionId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![context_append()],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "gpt-context-only",
                )),
                ..Default::default()
            }),
            build_only_overrides: None,
        });

        let error = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect_err("context-only runtime applies must reject materialization metadata");

        assert!(
            error.to_string().contains("context-only")
                && error.to_string().contains("materialization"),
            "unexpected rejection reason: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_context_only_runtime_apply_rejects_turn_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let state = Arc::new(McpRuntimeSessionState::default());
        let session_id = SessionId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![context_append()],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::HandlingMode::Steer),
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
            build_only_overrides: None,
        });

        let error = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect_err("context-only runtime applies must reject unapplied turn metadata");

        assert!(
            error.to_string().contains("context-only")
                && error.to_string().contains("turn metadata"),
            "unexpected rejection reason: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_context_only_rematerialization_retry_preserves_apply_boundary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let state = Arc::new(McpRuntimeSessionState::default());
        let session = context
            .service
            .create_session(deferred_create_request("seed prompt"))
            .await
            .expect("create deferred session");
        let session_id = session.session_id;
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session before context-only apply");
        context
            .runtime_adapter
            .unregister_session(&session_id)
            .await;

        let run_id = RunId::new();
        let contributing_input_id = InputId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![context_append()],
            contributing_input_ids: vec![contributing_input_id.clone()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
            build_only_overrides: None,
        });

        let output = apply_runtime_turn(&context, &state, &session_id, run_id.clone(), &primitive)
            .await
            .expect("context-only rematerialization retry should apply");

        assert_eq!(output.receipt.run_id, run_id);
        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(
            output.receipt.contributing_input_ids,
            vec![contributing_input_id]
        );
        assert!(
            output.terminal.is_none(),
            "context-only apply must not synthesize a turn terminal"
        );
    }

    #[tokio::test]
    async fn mcp_ingress_post_commit_materialize_error_preserves_live_runtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fail_store = Arc::new(FailingSaveSessionStore::new());
        let context = build_test_context_with_store(
            &temp,
            Arc::clone(&fail_store) as Arc<dyn meerkat::SessionStore>,
        )
        .await;
        let created = context
            .service
            .create_session(deferred_create_request("seed prompt"))
            .await
            .expect("create deferred persisted session");
        let session_id = created.session_id;
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session before rematerialization");
        context
            .runtime_adapter
            .unregister_session(&session_id)
            .await;

        fail_store.set_fail_save(true);
        let state = Arc::new(McpRuntimeSessionState::default());
        let error = Box::pin(context.rematerialize_persisted_session(
            &session_id,
            state,
            SurfaceSessionRecoveryOverrides::default(),
        ))
        .await
        .expect_err("forced post-commit save failure should surface");

        assert!(
            error.to_string().contains("forced save failure"),
            "unexpected error: {error}"
        );
        assert!(
            context
                .service
                .export_live_session(&session_id)
                .await
                .is_ok(),
            "post-commit create error should leave the committed live session"
        );
        assert!(
            context.runtime_adapter.contains_session(&session_id).await,
            "MCP ingress cleanup must preserve committed runtime registration"
        );
        assert!(
            context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "MCP ingress cleanup must preserve committed runtime session state"
        );
    }

    #[test]
    fn mcp_context_only_shortcut_excludes_terminal_peer_response() {
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunStart,
            appends: Vec::new(),
            context_appends: vec![context_append()],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                peer_response_terminal_apply_intent: Some(
                    PeerResponseTerminalApplyIntent::AppendContextAndRun,
                ),
                ..Default::default()
            }),
            build_only_overrides: None,
        });

        assert!(
            !should_apply_context_without_turn(&primitive),
            "terminal peer responses must append context and run a requester reaction turn"
        );
    }

    #[test]
    fn mcp_context_only_shortcut_keeps_plain_context_append() {
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![context_append()],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
            build_only_overrides: None,
        });

        assert!(should_apply_context_without_turn(&primitive));
    }
}

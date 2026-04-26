use async_trait::async_trait;
#[cfg(feature = "comms")]
use meerkat::surface::configure_peer_ingress;
use meerkat::{
    CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService, RunResult, Session,
    surface::{
        SurfaceRuntimeMaterializeError, SurfaceSessionRecoveryContext,
        SurfaceSessionRecoveryOverrides, build_recovered_session, materialize_session,
    },
};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::error::AgentError;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunPrimitive,
};
use meerkat_core::service::{SessionError, SessionService, StartTurnRequest};
use meerkat_core::types::{HandlingMode, SessionId};
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
                self.clear_session(&session_id).await;
                Err(surface_materialize_session_error(error))
            }
        }
    }

    async fn rematerialize_persisted_session(
        &self,
        session_id: &SessionId,
        state: Arc<McpRuntimeSessionState>,
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
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext {
                llm_client_override: None,
                external_tools,
                realm_id: Some(self.realm_id.to_string()),
                instance_id: self.instance_id.clone(),
                backend: Some(self.backend.clone()),
                config_generation: current_generation,
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
}

fn surface_materialize_session_error(error: SurfaceRuntimeMaterializeError) -> SessionError {
    match error {
        SurfaceRuntimeMaterializeError::Session(error) => error,
        other => SessionError::Agent(AgentError::InternalError(other.to_string())),
    }
}

fn surface_recovery_session_error(
    error: meerkat::surface::SurfaceSessionRecoveryError,
) -> SessionError {
    SessionError::Agent(AgentError::InternalError(error.to_string()))
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

async fn apply_runtime_turn(
    context: &McpRuntimeIngressContext,
    state: &Arc<McpRuntimeSessionState>,
    session_id: &SessionId,
    run_id: meerkat_core::lifecycle::RunId,
    primitive: &RunPrimitive,
) -> Result<CoreApplyOutput, SessionError> {
    // Context-only staged primitive — no conversation appends, just context
    // (e.g. peer_response_terminal). The runtime boundary for these is
    // Steer-derived (RunCheckpoint), so the stricter `is_context_only_
    // immediate` gate (boundary == Immediate) doesn't match. Relaxing to
    // "no appends + has context_appends" is the correct trigger.
    if let RunPrimitive::StagedInput(staged) = primitive
        && staged.appends.is_empty()
        && !staged.context_appends.is_empty()
    {
        return context
            .service
            .apply_runtime_context_appends(
                session_id,
                run_id,
                pending_system_context_appends(&staged.context_appends),
                staged.contributing_input_ids.clone(),
            )
            .await;
    }

    let prompt = primitive.extract_content_input();
    let boundary = primitive.apply_boundary();
    let contributing_input_ids = primitive.contributing_input_ids().to_vec();
    let queued_context = state
        .take_turn_context_for_inputs(&contributing_input_ids)
        .await;
    let event_tx = queued_context
        .as_ref()
        .map(|context| context.event_tx.clone());

    let turn_request = StartTurnRequest {
        prompt: prompt.clone(),
        system_prompt: None,
        render_metadata: None,
        handling_mode: HandlingMode::Queue,
        event_tx: event_tx.clone(),
        skill_references: primitive
            .turn_metadata()
            .and_then(|meta| meta.skill_references.clone()),
        flow_tool_overlay: primitive
            .turn_metadata()
            .and_then(|meta| meta.flow_tool_overlay.clone()),
        turn_metadata: primitive.turn_metadata().cloned(),
    };

    match context
        .service
        .apply_runtime_turn(
            session_id,
            run_id.clone(),
            turn_request,
            boundary,
            contributing_input_ids.clone(),
        )
        .await
    {
        Ok(output) => Ok(output),
        Err(SessionError::NotFound { .. }) => {
            Box::pin(context.rematerialize_persisted_session(session_id, state.clone()))
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
                        render_metadata: None,
                        handling_mode: HandlingMode::Queue,
                        event_tx,
                        skill_references: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.skill_references.clone()),
                        flow_tool_overlay: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.flow_tool_overlay.clone()),
                        turn_metadata: primitive.turn_metadata().cloned(),
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
        .map_err(|error| CoreExecutorError::ApplyFailed {
            reason: error.to_string(),
        })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .context
                .service
                .interrupt(&self.session_id)
                .await
                .map_err(|error| CoreExecutorError::ControlFailed {
                    reason: error.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self
                    .context
                    .service
                    .discard_live_session(&self.session_id)
                    .await;
                self.context.clear_session(&self.session_id).await;
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(error) => Err(CoreExecutorError::ControlFailed {
                        reason: error.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

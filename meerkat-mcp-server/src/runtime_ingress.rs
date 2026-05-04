use async_trait::async_trait;
#[cfg(feature = "comms")]
use meerkat::surface::configure_peer_ingress;
use meerkat::{
    CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService, RunResult, Session,
    SessionServiceControlExt,
    surface::{
        SurfaceRuntimeMaterializeError, SurfaceSessionRecoveryContext,
        SurfaceSessionRecoveryOverrides, build_recovered_session, materialize_session,
        materialize_session_with_reserved_admission,
    },
};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::error::AgentError;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreApplyTerminal, CoreExecutor, CoreExecutorBoundaryHandle,
    CoreExecutorError, CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunPrimitive,
};
use meerkat_core::service::{SessionError, SessionService, StartTurnRequest};
use meerkat_core::types::{HandlingMode, SessionId};
use meerkat_core::{ConfigRuntime, EventEnvelope, PendingSystemContextAppend};
use meerkat_mcp::McpRouterAdapter;
use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_runtime::completion::{CompletionHandle, CompletionOutcome};
use meerkat_runtime::{AcceptOutcome, Input, MeerkatMachine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::{Mutex, RwLock, mpsc};

type RequestEventTx = mpsc::Sender<EventEnvelope<AgentEvent>>;

pub(crate) type SharedMcpRuntimeSessions =
    Arc<RwLock<HashMap<SessionId, Arc<McpRuntimeSessionState>>>>;
pub(crate) type SharedMcpAdapters = Arc<Mutex<HashMap<String, Arc<McpRouterAdapter>>>>;
pub(crate) type SharedMcpRuntimePreAdmissions =
    Arc<Mutex<HashMap<SessionId, Vec<McpRuntimePreAdmissionEntry>>>>;
pub(crate) type SharedMcpRuntimeRegistrationLocks =
    Arc<StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>>;

pub(crate) struct McpRuntimePreAdmissionEntry {
    input_id: meerkat_core::lifecycle::InputId,
    admission: meerkat::RuntimeContextAdmissionGuard,
}

struct McpRuntimePreAdmissionRegistration {
    pre_admissions: SharedMcpRuntimePreAdmissions,
    session_id: SessionId,
    input_id: meerkat_core::lifecycle::InputId,
    release_on_drop: bool,
}

pub(crate) struct McpRuntimeRegistrationLockLease {
    locks: SharedMcpRuntimeRegistrationLocks,
    session_id: SessionId,
    lock: Arc<Mutex<()>>,
}

impl McpRuntimePreAdmissionRegistration {
    fn new(
        pre_admissions: SharedMcpRuntimePreAdmissions,
        session_id: SessionId,
        input_id: meerkat_core::lifecycle::InputId,
    ) -> Self {
        Self {
            pre_admissions,
            session_id,
            input_id,
            release_on_drop: true,
        }
    }

    fn disarm(mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for McpRuntimePreAdmissionRegistration {
    fn drop(&mut self) {
        if !self.release_on_drop {
            return;
        }
        let pre_admissions = Arc::clone(&self.pre_admissions);
        let session_id = self.session_id.clone();
        let input_id = self.input_id.clone();
        tokio::spawn(async move {
            discard_mcp_runtime_pre_admission(&pre_admissions, &session_id, &input_id).await;
        });
    }
}

impl McpRuntimeRegistrationLockLease {
    pub(crate) fn mutex(&self) -> &Mutex<()> {
        &self.lock
    }
}

impl Drop for McpRuntimeRegistrationLockLease {
    fn drop(&mut self) {
        if Arc::strong_count(&self.lock) != 1 {
            return;
        }
        let this_lock = Arc::downgrade(&self.lock);
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locks
            .get(&self.session_id)
            .is_some_and(|registered| registered.ptr_eq(&this_lock))
        {
            locks.remove(&self.session_id);
        }
    }
}

async fn discard_mcp_runtime_pre_admission(
    pre_admissions: &SharedMcpRuntimePreAdmissions,
    session_id: &SessionId,
    input_id: &meerkat_core::lifecycle::InputId,
) {
    let mut pre_admissions = pre_admissions.lock().await;
    let Some(entries) = pre_admissions.get_mut(session_id) else {
        return;
    };
    if let Some(index) = entries.iter().position(|entry| &entry.input_id == input_id) {
        entries.remove(index);
    }
    if entries.is_empty() {
        pre_admissions.remove(session_id);
    }
}

async fn rekey_mcp_runtime_pre_admission(
    pre_admissions: &SharedMcpRuntimePreAdmissions,
    session_id: &SessionId,
    from_input_id: &meerkat_core::lifecycle::InputId,
    to_input_id: meerkat_core::lifecycle::InputId,
) {
    if from_input_id == &to_input_id {
        return;
    }
    let mut pre_admissions = pre_admissions.lock().await;
    if let Some(entries) = pre_admissions.get_mut(session_id)
        && let Some(entry) = entries
            .iter_mut()
            .find(|entry| &entry.input_id == from_input_id)
    {
        entry.input_id = to_input_id;
    }
}

fn spawn_mcp_runtime_pre_admission_rekey(
    pre_admissions: SharedMcpRuntimePreAdmissions,
    session_id: SessionId,
    from_input_id: meerkat_core::lifecycle::InputId,
    to_input_id: meerkat_core::lifecycle::InputId,
) {
    tokio::spawn(async move {
        rekey_mcp_runtime_pre_admission(&pre_admissions, &session_id, &from_input_id, to_input_id)
            .await;
    });
}

fn wrap_mcp_runtime_pre_admission_cleanup(
    context: McpRuntimeIngressContext,
    session_id: SessionId,
    requested_input_id: meerkat_core::lifecycle::InputId,
    accepted_input_id: meerkat_core::lifecycle::InputId,
    handle: CompletionHandle,
) -> CompletionHandle {
    handle.with_outcome_cleanup(move |outcome| async move {
        context
            .cleanup_runtime_after_completion_outcome(&session_id, &outcome)
            .await;
        context
            .discard_runtime_pre_admission(&session_id, &requested_input_id)
            .await;
        context
            .discard_runtime_pre_admission(&session_id, &accepted_input_id)
            .await;
        outcome
    })
}

fn completion_outcome_requires_mcp_runtime_cleanup(outcome: &CompletionOutcome) -> bool {
    match outcome {
        CompletionOutcome::Abandoned(reason) | CompletionOutcome::RuntimeTerminated(reason) => {
            reason.contains("runtime boundary commit failed")
                || reason.contains("runtime loop commit failed")
        }
        CompletionOutcome::Completed(_)
        | CompletionOutcome::CompletedWithoutResult
        | CompletionOutcome::Cancelled
        | CompletionOutcome::CallbackPending { .. } => false,
    }
}

fn completion_outcome_is_mcp_apply_failure(outcome: &CompletionOutcome) -> bool {
    match outcome {
        CompletionOutcome::Abandoned(reason) | CompletionOutcome::RuntimeTerminated(reason) => {
            reason.starts_with("apply failed:")
        }
        CompletionOutcome::Completed(_)
        | CompletionOutcome::CompletedWithoutResult
        | CompletionOutcome::Cancelled
        | CompletionOutcome::CallbackPending { .. } => false,
    }
}

pub(crate) struct McpRuntimeIngressResources {
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    pub runtime_adapter: Arc<MeerkatMachine>,
    pub config_runtime: Arc<ConfigRuntime>,
    pub realm_id: meerkat_core::connection::RealmId,
    pub instance_id: Option<String>,
    pub backend: String,
    pub mcp_adapters: SharedMcpAdapters,
    pub runtime_sessions: SharedMcpRuntimeSessions,
    pub runtime_pre_admissions: SharedMcpRuntimePreAdmissions,
    pub runtime_registration_locks: SharedMcpRuntimeRegistrationLocks,
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
    runtime_pre_admissions: SharedMcpRuntimePreAdmissions,
    runtime_registration_locks: SharedMcpRuntimeRegistrationLocks,
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
            runtime_pre_admissions: resources.runtime_pre_admissions,
            runtime_registration_locks: resources.runtime_registration_locks,
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
        #[cfg(feature = "comms")]
        self.runtime_adapter.abort_comms_drain(session_id).await;
        self.runtime_adapter.unregister_session(session_id).await;
        if let Some(state) = self.runtime_sessions.write().await.remove(session_id) {
            state.clear_queued_turns().await;
        }
        self.discard_all_runtime_pre_admissions_for_session(session_id)
            .await;
        self.mcp_adapters
            .lock()
            .await
            .remove(&session_id.to_string());
    }

    async fn cleanup_runtime_after_completion_outcome(
        &self,
        session_id: &SessionId,
        outcome: &CompletionOutcome,
    ) {
        let archived_now = self
            .service
            .load_authoritative_session(session_id)
            .await
            .ok()
            .flatten()
            .as_ref()
            .is_some_and(session_metadata_marks_archived);
        if archived_now {
            let _ = self.service.discard_live_session(session_id).await;
            self.clear_session(session_id).await;
            return;
        }

        let live_present = self
            .service
            .has_live_session(session_id)
            .await
            .unwrap_or(false);
        if completion_outcome_requires_mcp_runtime_cleanup(outcome)
            || (!live_present && completion_outcome_is_mcp_apply_failure(outcome))
        {
            let _ = self.service.discard_live_session(session_id).await;
            self.clear_session(session_id).await;
        }
    }

    async fn insert_runtime_pre_admission(
        &self,
        session_id: SessionId,
        input_id: meerkat_core::lifecycle::InputId,
        admission: meerkat::RuntimeContextAdmissionGuard,
    ) -> Result<(), SessionError> {
        let mut pre_admissions = self.runtime_pre_admissions.lock().await;
        let entries = pre_admissions.entry(session_id.clone()).or_default();
        if entries.iter().any(|entry| entry.input_id == input_id) {
            return Err(SessionError::Busy { id: session_id });
        }
        entries.push(McpRuntimePreAdmissionEntry {
            input_id,
            admission,
        });
        Ok(())
    }

    async fn take_runtime_pre_admission(
        &self,
        session_id: &SessionId,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Option<meerkat::RuntimeContextAdmissionGuard> {
        if input_ids.is_empty() {
            return None;
        }
        let mut pre_admissions = self.runtime_pre_admissions.lock().await;
        let entries = pre_admissions.get_mut(session_id)?;
        let index = entries
            .iter()
            .position(|entry| input_ids.contains(&entry.input_id))?;
        let entry = entries.remove(index);
        if entries.is_empty() {
            pre_admissions.remove(session_id);
        }
        Some(entry.admission)
    }

    async fn discard_runtime_pre_admission(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) {
        discard_mcp_runtime_pre_admission(&self.runtime_pre_admissions, session_id, input_id).await;
    }

    async fn discard_all_runtime_pre_admissions_for_session(&self, session_id: &SessionId) {
        self.runtime_pre_admissions.lock().await.remove(session_id);
    }

    pub(crate) fn runtime_registration_lock(
        &self,
        session_id: &SessionId,
    ) -> McpRuntimeRegistrationLockLease {
        let lock = {
            let mut locks = self
                .runtime_registration_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(lock) = locks.get(session_id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(session_id.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        McpRuntimeRegistrationLockLease {
            locks: Arc::clone(&self.runtime_registration_locks),
            session_id: session_id.clone(),
            lock,
        }
    }

    pub(crate) async fn clear_session_if_new_locked(
        &self,
        session_id: &SessionId,
        runtime_was_registered: bool,
        runtime_state_existed: bool,
    ) {
        let has_active_inputs = self
            .runtime_adapter
            .list_active_inputs(session_id)
            .await
            .is_ok_and(|inputs| !inputs.is_empty());
        if has_active_inputs {
            return;
        }
        if self
            .runtime_pre_admissions
            .lock()
            .await
            .contains_key(session_id)
        {
            return;
        }
        if !runtime_was_registered {
            self.runtime_adapter.unregister_session(session_id).await;
        }
        if !runtime_state_existed {
            if let Some(state) = self.runtime_sessions.write().await.remove(session_id) {
                state.clear_queued_turns().await;
            }
            self.mcp_adapters
                .lock()
                .await
                .remove(&session_id.to_string());
        }
    }

    pub(crate) async fn clear_session_if_new(
        &self,
        session_id: &SessionId,
        runtime_was_registered: bool,
        runtime_state_existed: bool,
    ) {
        let lock = self.runtime_registration_lock(session_id);
        let _guard = lock.mutex().lock().await;
        self.clear_session_if_new_locked(session_id, runtime_was_registered, runtime_state_existed)
            .await;
    }

    pub(crate) async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<RequestEventTx>,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), meerkat_runtime::RuntimeDriverError>
    {
        if self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(|error| meerkat_runtime::RuntimeDriverError::Internal(error.to_string()))?
            .as_ref()
            .is_some_and(session_metadata_marks_archived)
        {
            self.clear_session(session_id).await;
            return Err(meerkat_runtime::RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Destroyed,
            });
        }

        let requested_input_id = input.id().clone();
        let runtime_registration_lock = self.runtime_registration_lock(session_id);
        let runtime_registration_guard = runtime_registration_lock.mutex().lock().await;
        let pre_admission = match self
            .service
            .reserve_runtime_turn_admission(session_id)
            .await
        {
            Ok(admission) => Some(admission),
            Err(error) => {
                return Err(meerkat_runtime::RuntimeDriverError::ValidationFailed {
                    reason: error.to_string(),
                });
            }
        };
        let mut pre_admission_registration = None;
        if let Some(admission) = pre_admission {
            if let Err(error) = self
                .insert_runtime_pre_admission(
                    session_id.clone(),
                    requested_input_id.clone(),
                    admission,
                )
                .await
            {
                return Err(meerkat_runtime::RuntimeDriverError::ValidationFailed {
                    reason: error.to_string(),
                });
            }
            pre_admission_registration = Some(McpRuntimePreAdmissionRegistration::new(
                Arc::clone(&self.runtime_pre_admissions),
                session_id.clone(),
                requested_input_id.clone(),
            ));
        }

        let runtime_was_registered = self.runtime_adapter.contains_session(session_id).await;
        let runtime_state_existed = self.runtime_sessions.read().await.contains_key(session_id);
        let state = self.runtime_session_state(session_id).await;
        let mut context_input_id = requested_input_id.clone();
        let queued_context = state
            .enqueue_turn_context(requested_input_id.clone(), event_tx)
            .await;

        let (outcome, mut handle) = match self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if queued_context {
                    let _ = state.discard_turn_context(&requested_input_id).await;
                }
                self.discard_runtime_pre_admission(session_id, &requested_input_id)
                    .await;
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
                self.clear_session_if_new_locked(
                    session_id,
                    runtime_was_registered,
                    runtime_state_existed,
                )
                .await;
                return Err(error);
            }
        };

        match &outcome {
            AcceptOutcome::Accepted { input_id, .. } if handle.is_some() => {
                if let Some(raw_handle) = handle.take() {
                    let cleanup_handle = wrap_mcp_runtime_pre_admission_cleanup(
                        self.clone(),
                        session_id.clone(),
                        requested_input_id.clone(),
                        input_id.clone(),
                        raw_handle,
                    );
                    spawn_mcp_runtime_pre_admission_rekey(
                        Arc::clone(&self.runtime_pre_admissions),
                        session_id.clone(),
                        requested_input_id.clone(),
                        input_id.clone(),
                    );
                    handle = Some(cleanup_handle);
                }
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
            }
            AcceptOutcome::Accepted { input_id, .. } => {
                self.discard_runtime_pre_admission(session_id, &requested_input_id)
                    .await;
                self.discard_runtime_pre_admission(session_id, input_id)
                    .await;
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
            }
            _ => {
                self.discard_runtime_pre_admission(session_id, &requested_input_id)
                    .await;
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
            }
        }

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

        if !matches!(&outcome, AcceptOutcome::Accepted { .. }) || handle.is_none() {
            self.clear_session_if_new_locked(
                session_id,
                runtime_was_registered,
                runtime_state_existed,
            )
            .await;
        }

        drop(runtime_registration_guard);
        drop(runtime_registration_lock);
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
        admission: Option<meerkat::RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, SessionError> {
        let session_id = session.id().clone();
        if session_metadata_marks_archived(&session) {
            return Err(SessionError::NotFound { id: session_id });
        }
        let runtime_was_registered = self.runtime_adapter.contains_session(&session_id).await;
        let runtime_state_existed = self.runtime_sessions.read().await.contains_key(&session_id);
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        let context = self.clone();
        let result = if let Some(admission) = admission {
            Box::pin(materialize_session_with_reserved_admission(
                &self.service,
                &self.runtime_adapter,
                session,
                request,
                admission,
                move |runtime_session_id| {
                    Box::new(McpSessionRuntimeExecutor::new(
                        context,
                        runtime_session_id,
                        state,
                    ))
                },
            ))
            .await
        } else {
            Box::pin(materialize_session(
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
            .await
        };

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
                self.clear_session_if_new(
                    &session_id,
                    runtime_was_registered,
                    runtime_state_existed,
                )
                .await;
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
        if session_metadata_marks_archived(&session) {
            return Err(SessionError::NotFound {
                id: session_id.clone(),
            });
        }
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
            None,
        ))
        .await?;
        Ok(result.session_id)
    }

    async fn rematerialize_persisted_session_with_admission(
        &self,
        session_id: &SessionId,
        state: Arc<McpRuntimeSessionState>,
        admission: meerkat::RuntimeContextAdmissionGuard,
    ) -> Result<SessionId, SessionError> {
        let session = self
            .service
            .load_authoritative_session(session_id)
            .await?
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.clone(),
            })?;
        if session_metadata_marks_archived(&session) {
            return Err(SessionError::NotFound {
                id: session_id.clone(),
            });
        }
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
            Some(admission),
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

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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

    if context
        .service
        .load_authoritative_session(session_id)
        .await?
        .as_ref()
        .is_some_and(session_metadata_marks_archived)
    {
        context.clear_session(session_id).await;
        return Err(SessionError::NotFound {
            id: session_id.clone(),
        });
    }

    if primitive.is_context_only_apply_without_turn() {
        let RunPrimitive::StagedInput(staged) = primitive else {
            unreachable!("context-only apply helper only matches staged inputs");
        };
        let appends = pending_system_context_appends(&staged.context_appends);
        let boundary = primitive.apply_boundary();
        let contributing_input_ids = staged.contributing_input_ids.clone();
        let mut pre_admission = context
            .take_runtime_pre_admission(session_id, &contributing_input_ids)
            .await;
        let apply_result = if let Some(admission) = pre_admission.take() {
            match context
                .service
                .apply_runtime_context_appends_with_recoverable_reserved_admission(
                    session_id,
                    run_id.clone(),
                    appends.clone(),
                    boundary,
                    contributing_input_ids.clone(),
                    admission,
                )
                .await
            {
                Ok(output) => Ok(output),
                Err((error, admission)) => {
                    pre_admission = admission;
                    Err(error)
                }
            }
        } else {
            context
                .service
                .apply_runtime_context_appends_with_boundary(
                    session_id,
                    run_id.clone(),
                    appends.clone(),
                    boundary,
                    contributing_input_ids.clone(),
                )
                .await
        };
        return match apply_result {
            Ok(output) => Ok(output),
            Err(SessionError::NotFound { .. }) => {
                if let Some(admission) = pre_admission.take() {
                    if let Err(error) =
                        Box::pin(context.rematerialize_persisted_session_with_admission(
                            session_id,
                            state.clone(),
                            admission,
                        ))
                        .await
                    {
                        if matches!(error, SessionError::NotFound { .. }) {
                            context.clear_session(session_id).await;
                        }
                        return Err(error);
                    }
                } else if let Err(error) =
                    Box::pin(context.rematerialize_persisted_session(session_id, state.clone()))
                        .await
                {
                    if matches!(error, SessionError::NotFound { .. }) {
                        context.clear_session(session_id).await;
                    }
                    return Err(error);
                }
                context
                    .service
                    .apply_runtime_context_appends_with_boundary(
                        session_id,
                        run_id,
                        appends,
                        boundary,
                        contributing_input_ids,
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
        pre_turn_context_appends: pre_turn_context_appends.clone(),
        turn_metadata: primitive.turn_metadata().cloned(),
    };

    let mut pre_admission = context
        .take_runtime_pre_admission(session_id, &contributing_input_ids)
        .await;
    let apply_result = if let Some(admission) = pre_admission.take() {
        match context
            .service
            .apply_runtime_turn_with_recoverable_reserved_admission(
                session_id,
                run_id.clone(),
                turn_request,
                boundary,
                contributing_input_ids.clone(),
                admission,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err((error, admission)) => {
                pre_admission = admission;
                Err(error)
            }
        }
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

    match apply_result {
        Ok(output) => Ok(output),
        Err(SessionError::NotFound { .. }) => {
            if let Some(admission) = pre_admission.take() {
                if let Err(error) =
                    Box::pin(context.rematerialize_persisted_session_with_admission(
                        session_id,
                        state.clone(),
                        admission,
                    ))
                    .await
                {
                    if matches!(error, SessionError::NotFound { .. }) {
                        context.clear_session(session_id).await;
                    }
                    return Err(error);
                }
            } else if let Err(error) =
                Box::pin(context.rematerialize_persisted_session(session_id, state.clone())).await
            {
                if matches!(error, SessionError::NotFound { .. }) {
                    context.clear_session(session_id).await;
                }
                return Err(error);
            }
            let output = context
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
                        pre_turn_context_appends,
                        turn_metadata: primitive.turn_metadata().cloned(),
                    },
                    boundary,
                    contributing_input_ids,
                )
                .await?;
            if matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)) {
                let _ = context.service.discard_live_session(session_id).await;
                context.clear_session(session_id).await;
            }
            Ok(output)
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
        .map_err(CoreExecutorError::apply_failed_from_session_error)
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
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use meerkat::surface::build_runtime_backed_service;
    use meerkat::{AgentFactory, Config, PersistenceBundle};
    use meerkat_client::TestClient;
    use meerkat_core::lifecycle::run_primitive::{
        PeerResponseTerminalApplyIntent, RunApplyBoundary, RuntimeExecutionKind,
        RuntimeTurnMetadata, StagedRunInput,
    };
    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::{MemoryConfigStore, RealmId};
    use meerkat_runtime::SessionServiceRuntimeExt as _;
    use meerkat_store::{JsonlStore, MemoryBlobStore};

    async fn build_test_context(temp: &tempfile::TempDir) -> McpRuntimeIngressContext {
        build_test_context_with_capacity(temp, 4).await
    }

    async fn build_test_context_with_capacity(
        temp: &tempfile::TempDir,
        active_session_capacity: usize,
    ) -> McpRuntimeIngressContext {
        let session_store = Arc::new(JsonlStore::new(temp.path().join("sessions")));
        session_store.init().await.expect("init jsonl store");
        let session_store: Arc<dyn meerkat::SessionStore> = session_store;
        let persistence =
            PersistenceBundle::new(session_store, None, Arc::new(MemoryBlobStore::new()));

        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, runtime_adapter) =
            build_runtime_backed_service(builder, active_session_capacity, persistence);
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
            runtime_pre_admissions: Arc::new(Mutex::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
        })
    }

    fn create_request(
        prompt: &str,
        initial_turn: meerkat_core::service::InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: prompt.to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: Some(1024),
            event_tx: None,
            skill_references: None,
            initial_turn,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions::default()),
            labels: None,
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
        });

        let error = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect_err("malformed terminal peer-response intent must be rejected");

        assert!(
            error.to_string().contains("requires RunStart boundary"),
            "unexpected rejection reason: {error}"
        );
    }

    #[tokio::test]
    async fn mcp_runtime_rematerialize_rejects_archived_session_before_binding() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(1024),
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        context
            .service
            .archive(&session_id)
            .await
            .expect("archive should succeed");
        let state = Arc::new(McpRuntimeSessionState::default());

        let rejected = context
            .rematerialize_persisted_session(&session_id, state)
            .await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived MCP runtime rematerialization should reject before binding: {rejected:?}"
        );
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "archived MCP runtime rematerialization must not leave a runtime registration"
        );
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "archived MCP runtime rematerialization must not install runtime session state"
        );
    }

    #[tokio::test]
    async fn mcp_runtime_apply_recovers_persisted_session_for_context_only_apply() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(1024),
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        context.clear_session(&session_id).await;
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "test must remove runtime registration before context-only recovery"
        );

        let state = context.ensure_session(&session_id).await;
        let input_id = InputId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![ConversationContextAppend {
                key: "mcp-context-recovery".to_string(),
                content: CoreRenderable::Text {
                    text: "mcp recovered context".to_string(),
                },
            }],
            contributing_input_ids: vec![input_id.clone()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
        });

        let output = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect("context-only apply should recover persisted MCP session");

        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(output.receipt.contributing_input_ids, vec![input_id]);
        assert!(context.runtime_adapter.contains_session(&session_id).await);
        assert!(
            context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "context-only recovery should install MCP runtime session state"
        );

        let exported = context
            .service
            .export_live_session(&session_id)
            .await
            .expect("export recovered live session");
        let system_context = exported
            .messages()
            .iter()
            .find_map(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or("");
        assert!(
            system_context.contains("mcp-context-recovery")
                && system_context.contains("mcp recovered context"),
            "context-only recovery should persist MCP runtime context append: {system_context}"
        );
    }

    #[tokio::test]
    async fn mcp_runtime_accept_clears_state_for_archived_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(1024),
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        context.ensure_session(&session_id).await;
        assert!(context.runtime_adapter.contains_session(&session_id).await);

        context
            .service
            .archive(&session_id)
            .await
            .expect("archive should succeed");
        let rejected = context
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(meerkat_runtime::PromptInput::new("after archive", None)),
                None,
            )
            .await;
        assert!(
            matches!(
                rejected,
                Err(meerkat_runtime::RuntimeDriverError::NotReady {
                    state: meerkat_runtime::RuntimeState::Destroyed
                })
            ),
            "archived MCP accept should reject as not ready: {rejected:?}"
        );
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "archived MCP accept must clear runtime registration"
        );
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "archived MCP accept must clear runtime session state"
        );
    }

    #[tokio::test]
    async fn mcp_runtime_pre_admission_completion_cleanup_clears_boundary_failed_runtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let created = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let session_id = created.session_id;
        context.ensure_session(&session_id).await;
        context.mcp_adapters.lock().await.insert(
            session_id.to_string(),
            Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new())),
        );
        assert!(context.runtime_adapter.contains_session(&session_id).await);
        assert!(
            context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id)
        );
        assert!(
            context
                .mcp_adapters
                .lock()
                .await
                .contains_key(&session_id.to_string())
        );

        let requested_input_id = InputId::new();
        let accepted_input_id = InputId::new();
        let admission = context
            .service
            .reserve_runtime_turn_admission(&session_id)
            .await
            .expect("reserve runtime turn admission");
        context
            .insert_runtime_pre_admission(session_id.clone(), requested_input_id.clone(), admission)
            .await
            .expect("insert pending pre-admission");

        let handle = wrap_mcp_runtime_pre_admission_cleanup(
            context.clone(),
            session_id.clone(),
            requested_input_id,
            accepted_input_id,
            CompletionHandle::already_resolved(CompletionOutcome::Abandoned(
                "runtime boundary commit failed: injected failure".to_string(),
            )),
        );
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
            .await
            .expect("cleanup handle should finish");

        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "boundary failure cleanup must unregister the runtime"
        );
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "boundary failure cleanup must remove MCP runtime session state"
        );
        assert!(
            !context
                .mcp_adapters
                .lock()
                .await
                .contains_key(&session_id.to_string()),
            "boundary failure cleanup must remove MCP adapter bindings"
        );
        assert!(
            context
                .runtime_pre_admissions
                .lock()
                .await
                .get(&session_id)
                .is_none(),
            "boundary failure cleanup must release MCP runtime pre-admission"
        );

        context
            .service
            .create_session(create_request(
                "new deferred capacity user",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("cleanup must release active capacity");
    }

    #[tokio::test]
    async fn mcp_runtime_accept_capacity_full_rejects_before_input_accept() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context.ensure_session(&target_session_id).await;

        let _blocker = context
            .service
            .create_session(create_request(
                "capacity blocker",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("deferred blocker should fill active capacity");

        let input = Input::Prompt(meerkat_runtime::PromptInput::new(
            "must reject before runtime accept",
            None,
        ));
        let rejected = context
            .accept_input_with_completion(&target_session_id, input, None)
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "capacity-full MCP runtime accept should reject before input admission: {rejected:?}"
        );

        let active_inputs = context
            .runtime_adapter
            .list_active_inputs(&target_session_id)
            .await
            .expect("list active inputs");
        assert!(
            active_inputs.is_empty(),
            "capacity rejection must not enqueue MCP runtime input: {active_inputs:?}"
        );
    }

    #[tokio::test]
    async fn mcp_runtime_accept_waits_for_registration_lock_before_pre_admission() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context.ensure_session(&target_session_id).await;

        let registration_lock = context.runtime_registration_lock(&target_session_id);
        let registration_guard = registration_lock.mutex().lock().await;
        let accept_context = context.clone();
        let accept_session_id = target_session_id.clone();
        let accept_task = tokio::spawn(async move {
            accept_context
                .accept_input_with_completion(
                    &accept_session_id,
                    Input::Prompt(meerkat_runtime::PromptInput::new("locked accept", None)),
                    None,
                )
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            context
                .runtime_pre_admissions
                .lock()
                .await
                .get(&target_session_id)
                .is_none(),
            "accept_input must not reserve active capacity while registration mutation is locked"
        );

        drop(registration_guard);
        drop(registration_lock);
        let accepted = tokio::time::timeout(std::time::Duration::from_secs(5), accept_task)
            .await
            .expect("locked accept should finish after registration lock is released")
            .expect("locked accept task should not panic")
            .expect("locked accept should succeed once registration lock is released");
        if let Some(handle) = accepted.1 {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
                .await
                .expect("locked accept completion should finish");
        }
    }

    #[tokio::test]
    async fn mcp_new_runtime_cleanup_preserves_pending_pre_admission() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context
            .runtime_adapter
            .unregister_session(&target_session_id)
            .await;
        let runtime_was_registered = context
            .runtime_adapter
            .contains_session(&target_session_id)
            .await;
        let runtime_state_existed = context
            .runtime_sessions
            .read()
            .await
            .contains_key(&target_session_id);
        context
            .runtime_adapter
            .prepare_bindings(target_session_id.clone())
            .await
            .expect("prepare new runtime binding");
        let input_id = InputId::new();
        let admission = context
            .service
            .reserve_runtime_turn_admission(&target_session_id)
            .await
            .expect("reserve pending runtime admission");
        context
            .insert_runtime_pre_admission(target_session_id.clone(), input_id.clone(), admission)
            .await
            .expect("insert pending pre-admission");

        context
            .clear_session_if_new(
                &target_session_id,
                runtime_was_registered,
                runtime_state_existed,
            )
            .await;
        assert!(
            context
                .runtime_adapter
                .contains_session(&target_session_id)
                .await,
            "cleanup must preserve a new runtime registration with pending active admission"
        );

        context
            .discard_runtime_pre_admission(&target_session_id, &input_id)
            .await;
    }

    #[tokio::test]
    async fn mcp_runtime_accept_terminal_no_handle_clears_new_runtime_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context
            .service
            .discard_live_session(&target_session_id)
            .await
            .expect("discard live target session");
        context
            .runtime_adapter
            .unregister_session(&target_session_id)
            .await;
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&target_session_id),
            "test requires no preexisting MCP runtime state"
        );

        let operation_id = meerkat_core::OperationId::new();
        let input = Input::Operation(meerkat_runtime::OperationInput {
            header: meerkat_runtime::InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: meerkat_core::types::message_timestamp_now(),
                source: meerkat_runtime::InputOrigin::System,
                durability: meerkat_runtime::InputDurability::Derived,
                visibility: meerkat_runtime::InputVisibility {
                    transcript_eligible: false,
                    operator_eligible: false,
                },
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            operation_id: operation_id.clone(),
            event: meerkat_core::OpEvent::Cancelled { id: operation_id },
        });

        let (outcome, handle) = context
            .accept_input_with_completion(&target_session_id, input, None)
            .await
            .expect("terminal operation input should be accepted");
        assert!(
            matches!(outcome, AcceptOutcome::Accepted { .. }),
            "terminal operation input should be accepted: {outcome:?}"
        );
        assert!(
            handle.is_none(),
            "terminal operation input should not return a completion handle"
        );
        assert!(
            !context
                .runtime_adapter
                .contains_session(&target_session_id)
                .await,
            "terminal no-handle accept must unregister newly-created runtime registration"
        );
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&target_session_id),
            "terminal no-handle accept must clear newly-created MCP runtime state"
        );
        assert!(
            context
                .runtime_pre_admissions
                .lock()
                .await
                .get(&target_session_id)
                .is_none(),
            "terminal no-handle accept must release pre-admission"
        );
        context
            .service
            .create_session(create_request(
                "replacement",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("terminal no-handle accept should release active capacity");
    }

    #[tokio::test]
    async fn mcp_runtime_accept_recovers_persisted_live_missing_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        let state = context.ensure_session(&target_session_id).await;
        context
            .runtime_adapter
            .ensure_session_with_executor(
                target_session_id.clone(),
                Box::new(McpSessionRuntimeExecutor::new(
                    context.clone(),
                    target_session_id.clone(),
                    state.clone(),
                )),
            )
            .await;
        context
            .service
            .discard_live_session(&target_session_id)
            .await
            .expect("discard live target session");
        assert!(
            !context
                .service
                .has_live_session(&target_session_id)
                .await
                .expect("check live session"),
            "test requires a persisted-only target session"
        );

        let input = Input::Prompt(meerkat_runtime::PromptInput::new(
            "recover persisted-only target",
            None,
        ));
        let (outcome, handle) = context
            .accept_input_with_completion(&target_session_id, input, None)
            .await
            .expect("persisted-only MCP runtime accept should reserve and accept");
        assert!(
            matches!(outcome, AcceptOutcome::Accepted { .. }),
            "persisted-only MCP input should be accepted: {outcome:?}"
        );
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
                .await
                .expect("persisted-only MCP input should complete");
        }

        for _ in 0..200 {
            let pre_admission_cleared = context
                .runtime_pre_admissions
                .lock()
                .await
                .get(&target_session_id)
                .is_none();
            let active_inputs = context
                .runtime_adapter
                .list_active_inputs(&target_session_id)
                .await
                .unwrap_or_default();
            if pre_admission_cleared && active_inputs.is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("persisted-only MCP input did not finish and clear pre-admission");
    }

    #[tokio::test]
    async fn mcp_runtime_recovery_no_pending_releases_capacity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = context
            .service
            .create_session(create_request(
                "completed target",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("completed target create should succeed");
        let session_id = target.session_id;
        let state = context.ensure_session(&session_id).await;
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard completed live session before recovery");
        context
            .runtime_adapter
            .unregister_session(&session_id)
            .await;

        let input_id = InputId::new();
        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: Vec::new(),
            context_appends: Vec::new(),
            contributing_input_ids: vec![input_id],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ResumePending),
                ..Default::default()
            }),
        });
        let output = apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive)
            .await
            .expect("live-missing resume-pending recovery should return no-op output");
        assert!(
            matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)),
            "expected no-pending terminal from recovered completed session: {output:?}"
        );
        assert!(
            !context
                .service
                .has_live_session(&session_id)
                .await
                .expect("check recovered live session"),
            "no-op recovery should discard the rematerialized live session"
        );
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "no-op recovery should unregister the rematerialized runtime"
        );

        context
            .service
            .create_session(create_request(
                "new deferred capacity user",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("no-op recovery must release active capacity");
    }

    #[tokio::test]
    async fn mcp_runtime_apply_clears_state_for_archived_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(1024),
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        let state = context.ensure_session(&session_id).await;
        assert!(context.runtime_adapter.contains_session(&session_id).await);

        context
            .service
            .archive(&session_id)
            .await
            .expect("archive should succeed");
        let primitive = RunPrimitive::ImmediateAppend(
            meerkat_core::lifecycle::run_primitive::ConversationAppend {
                role: meerkat_core::lifecycle::run_primitive::ConversationAppendRole::User,
                content: CoreRenderable::Text {
                    text: "after archive".to_string(),
                },
            },
        );
        let rejected =
            apply_runtime_turn(&context, &state, &session_id, RunId::new(), &primitive).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived MCP apply should reject as not found: {rejected:?}"
        );
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "archived MCP apply must clear runtime registration"
        );
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "archived MCP apply must clear runtime session state"
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
        });

        assert!(should_apply_context_without_turn(&primitive));
    }
}

use async_trait::async_trait;
use meerkat::session_runtime::admission::{
    RuntimePreAdmissionEntry, RuntimePreAdmissionRegistration, RuntimePreAdmissionRestore,
    RuntimeRegistrationLockLease,
};
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
#[cfg(test)]
use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunPrimitive,
};
use meerkat_core::service::{SessionError, SessionService, StartTurnRequest};
use meerkat_core::types::{HandlingMode, SessionId};
use meerkat_core::{ConfigRuntime, EventEnvelope, PendingSystemContextAppend};
use meerkat_mcp::McpRouterAdapter;
use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_runtime::completion::CompletionHandle;
use meerkat_runtime::{AcceptOutcome, Input, MeerkatMachine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::{Mutex, RwLock, mpsc};

type RequestEventTx = mpsc::Sender<EventEnvelope<AgentEvent>>;

pub(crate) type SharedMcpRuntimeSessions =
    Arc<RwLock<HashMap<SessionId, Arc<McpRuntimeSessionState>>>>;
pub(crate) type SharedMcpAdapters = Arc<Mutex<HashMap<String, Arc<McpRouterAdapter>>>>;
pub(crate) type SharedMcpRuntimePreAdmissions =
    Arc<StdMutex<HashMap<SessionId, Vec<RuntimePreAdmissionEntry>>>>;
pub(crate) type SharedMcpRuntimeRegistrationLocks =
    Arc<StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>>;

/// Sync restore hook backing the shared [`RuntimePreAdmissionRegistration`]
/// RAII seam for MCP (defined in `meerkat::session_runtime::admission`, the
/// same seam RPC consumes). When an un-disarmed registration drops, the shared
/// guard calls `restore_or_release`, which removes the pre-admission entry —
/// dropping its `RuntimePreAdmission` and releasing the reserved capacity.
struct McpPreAdmissionLedger {
    pre_admissions: SharedMcpRuntimePreAdmissions,
}

impl RuntimePreAdmissionRestore for McpPreAdmissionLedger {
    fn restore_or_release(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) {
        discard_mcp_runtime_pre_admission(&self.pre_admissions, session_id, input_id);
    }
}

fn discard_mcp_runtime_pre_admission(
    pre_admissions: &SharedMcpRuntimePreAdmissions,
    session_id: &SessionId,
    input_id: &meerkat_core::lifecycle::InputId,
) {
    let mut pre_admissions = pre_admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

fn rekey_mcp_runtime_pre_admission(
    pre_admissions: &SharedMcpRuntimePreAdmissions,
    session_id: &SessionId,
    from_input_id: &meerkat_core::lifecycle::InputId,
    to_input_id: meerkat_core::lifecycle::InputId,
) {
    if from_input_id == &to_input_id {
        return;
    }
    let mut pre_admissions = pre_admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entries) = pre_admissions.get_mut(session_id)
        && let Some(entry) = entries
            .iter_mut()
            .find(|entry| &entry.input_id == from_input_id)
    {
        entry.input_id = to_input_id;
    }
}

fn wrap_mcp_runtime_pre_admission_cleanup(
    context: McpRuntimeIngressContext,
    session_id: SessionId,
    requested_input_id: meerkat_core::lifecycle::InputId,
    accepted_input_id: meerkat_core::lifecycle::InputId,
    handle: CompletionHandle,
) -> CompletionHandle {
    handle.with_resultful_completion_cleanup(move |completion| async move {
        let runtime_registration_lock = context.runtime_registration_lock(&session_id);
        let _runtime_registration_guard = runtime_registration_lock.mutex().lock().await;
        let release_pre_admission = match completion {
            Ok(cleanup_observation) => {
                context
                    .cleanup_runtime_after_completion_outcome(&session_id, cleanup_observation)
                    .await?
            }
            Err(error) => {
                context
                    .runtime_wait_failure_releases_pre_admission(&session_id, &error)
                    .await?
            }
        };
        if release_pre_admission {
            context
                .discard_runtime_pre_admission(&session_id, &requested_input_id)
                .await;
            context
                .discard_runtime_pre_admission(&session_id, &accepted_input_id)
                .await;
        }
        Ok(())
    })
}

pub(crate) struct McpRuntimeIngressResources {
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    pub runtime_adapter: Arc<MeerkatMachine>,
    pub workgraph_service: meerkat::WorkGraphService,
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
    workgraph_service: meerkat::WorkGraphService,
    config_runtime: Arc<ConfigRuntime>,
    realm_id: meerkat_core::connection::RealmId,
    instance_id: Option<String>,
    backend: String,
    mcp_adapters: SharedMcpAdapters,
    runtime_sessions: SharedMcpRuntimeSessions,
    runtime_pre_admissions: SharedMcpRuntimePreAdmissions,
    runtime_registration_locks: SharedMcpRuntimeRegistrationLocks,
    #[cfg(test)]
    fail_external_cleanup_discard_once: Arc<std::sync::atomic::AtomicBool>,
}

impl McpRuntimeIngressContext {
    pub(crate) fn new(resources: McpRuntimeIngressResources) -> Self {
        Self {
            service: resources.service,
            runtime_adapter: resources.runtime_adapter,
            workgraph_service: resources.workgraph_service,
            config_runtime: resources.config_runtime,
            realm_id: resources.realm_id,
            instance_id: resources.instance_id,
            backend: resources.backend,
            mcp_adapters: resources.mcp_adapters,
            runtime_sessions: resources.runtime_sessions,
            runtime_pre_admissions: resources.runtime_pre_admissions,
            runtime_registration_locks: resources.runtime_registration_locks,
            #[cfg(test)]
            fail_external_cleanup_discard_once: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    async fn runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRuntimeSessionState>, SessionError> {
        if let Some(existing) = self.runtime_sessions.read().await.get(session_id).cloned() {
            if self
                .runtime_adapter
                .session_has_executor(session_id)
                .await
                .map_err(runtime_driver_error_to_session_error)?
            {
                return Ok(existing);
            }
            existing.clear_queued_turns().await;
            let executor = Box::new(McpSessionRuntimeExecutor::new(
                self.clone(),
                session_id.clone(),
                existing.clone(),
            ));
            self.runtime_adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await
                .map_err(runtime_driver_error_to_session_error)?;
            return Ok(existing);
        }

        let state = Arc::new(McpRuntimeSessionState::default());
        let executor = Box::new(McpSessionRuntimeExecutor::new(
            self.clone(),
            session_id.clone(),
            state.clone(),
        ));
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await
            .map_err(runtime_driver_error_to_session_error)?;
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        Ok(state)
    }

    pub(crate) async fn ensure_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRuntimeSessionState>, SessionError> {
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
    ) -> Result<Arc<McpRuntimeSessionState>, SessionError> {
        if replace_runtime_attachment {
            self.runtime_adapter
                .unregister_session(session_id)
                .await
                .map_err(runtime_driver_error_to_session_error)?;
        }
        let state = self.runtime_session_state(session_id).await?;
        state.set_callback_tools(callback_tools).await;
        Ok(state)
    }

    async fn clear_runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        // unregister_session owns comms-drain quiescence. Do not consume the
        // MCP state/adapters until that authoritative teardown succeeds.
        self.unregister_runtime_session_if_present(session_id)
            .await?;
        if let Some(state) = self.runtime_sessions.write().await.remove(session_id) {
            state.clear_queued_turns().await;
        }
        self.mcp_adapters
            .lock()
            .await
            .remove(&session_id.to_string());
        Ok(())
    }

    /// Clear only MCP/session projections after the machine-owned unregister
    /// saga has authorized external cleanup. Runtime registration remains the
    /// saga's responsibility; calling `unregister_session` here would recurse
    /// into and deadlock on that same coordinator.
    async fn clear_external_runtime_session_state(&self, session_id: &SessionId) {
        if let Some(state) = self.runtime_sessions.write().await.remove(session_id) {
            state.clear_queued_turns().await;
        }
        self.mcp_adapters
            .lock()
            .await
            .remove(&session_id.to_string());
        self.discard_all_runtime_pre_admissions_for_session(session_id)
            .await;
    }

    async fn clear_runtime_session_state_after_verified_termination(
        &self,
        session_id: &SessionId,
        cleanup_observation: &meerkat_runtime::CompletionCleanupObservation,
    ) -> Result<(), SessionError> {
        if !cleanup_observation.proves_runtime_termination_for(session_id) {
            return Err(SessionError::Unsupported(format!(
                "MCP runtime termination cleanup for {session_id} lacks machine-owned termination proof"
            )));
        }
        if self.runtime_adapter.contains_session(session_id).await {
            return Err(SessionError::Unsupported(format!(
                "MCP runtime termination cleanup for {session_id} expected an already-absent registration"
            )));
        }
        self.clear_runtime_session_state(session_id).await
    }

    /// Idempotently drive machine-owned unregister to verified absence.
    ///
    /// A deeper compensation path may already have completed unregister before
    /// an outer MCP cleanup owner runs. Verified absence is success; an error
    /// that leaves the registration present is still propagated and retains
    /// the MCP retry anchors.
    async fn unregister_runtime_session_if_present(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        if !self.runtime_adapter.contains_session(session_id).await {
            return Ok(());
        }
        match self.runtime_adapter.unregister_session(session_id).await {
            Ok(()) => Ok(()),
            Err(_) if !self.runtime_adapter.contains_session(session_id).await => Ok(()),
            Err(error) => Err(runtime_driver_error_to_session_error(error)),
        }
    }

    pub(crate) async fn clear_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.clear_runtime_session_state(session_id).await?;
        self.discard_all_runtime_pre_admissions_for_session(session_id)
            .await;
        Ok(())
    }

    async fn session_archived_by_authority(
        &self,
        session_id: &SessionId,
        session: &Session,
    ) -> Result<bool, SessionError> {
        self.service
            .session_archived_by_authority(session_id, session)
            .await
    }

    async fn authoritative_session_archived(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        let Some(session) = self.service.load_authoritative_session(session_id).await? else {
            return Ok(false);
        };
        self.session_archived_by_authority(session_id, &session)
            .await
    }

    async fn cleanup_runtime_after_completion_outcome(
        &self,
        session_id: &SessionId,
        cleanup_observation: meerkat_runtime::completion::CompletionCleanupObservation,
    ) -> Result<bool, meerkat_runtime::completion::CompletionWaitError> {
        let archived_now = match self.authoritative_session_archived(session_id).await {
            Ok(archived_now) => archived_now,
            Err(error) => {
                return Err(
                    meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                        format!(
                            "MCP runtime completion archive authority unavailable for {session_id}: {error}"
                        ),
                    ),
                );
            }
        };
        let live_session = if archived_now {
            meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::NotObserved
        } else {
            match self.service.has_live_session(session_id).await {
                Ok(true) => {
                    meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::Present
                }
                Ok(false) => {
                    meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::Absent
                }
                Err(error) => {
                    return Err(meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                        format!("MCP runtime completion live-session authority unavailable for {session_id}: {error}"),
                    ));
                }
            }
        };
        let runtime_termination_cleanup = cleanup_observation
            .proves_runtime_termination_for(session_id)
            && matches!(
                live_session,
                meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::Absent
            );
        let runtime_termination_proof = cleanup_observation.clone();
        let cleanup_authority = match self
            .runtime_adapter
            .resolve_runtime_completion_cleanup(
                session_id,
                cleanup_observation,
                archived_now,
                live_session,
            )
            .await
        {
            Ok(authority) => authority,
            Err(error) => {
                if archived_now && !self.runtime_adapter.contains_session(session_id).await {
                    // The only benign resolution race: durable archive is
                    // proven and the runtime registration is already gone.
                    return Ok(true);
                }
                if runtime_termination_cleanup
                    && !self.runtime_adapter.contains_session(session_id).await
                {
                    self.clear_runtime_session_state_after_verified_termination(
                        session_id,
                        &runtime_termination_proof,
                    )
                    .await
                    .map_err(|cleanup_error| {
                        meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                            format!(
                                "MCP runtime completion cleanup failed for {session_id}: {cleanup_error}"
                            ),
                        )
                    })?;
                    return Ok(true);
                }
                return Err(
                    meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                        format!(
                            "MCP generated runtime completion cleanup authority unavailable for {session_id}: {error}"
                        ),
                    ),
                );
            }
        };
        let release_pre_admission = cleanup_authority.releases_pre_admission();
        if cleanup_authority.requires_runtime_cleanup() {
            let discard_error = match self.service.discard_live_session(session_id).await {
                Ok(()) | Err(SessionError::NotFound { .. }) => None,
                Err(error) => Some(error),
            };
            let clear_error = if runtime_termination_cleanup
                && !self.runtime_adapter.contains_session(session_id).await
            {
                self.clear_runtime_session_state_after_verified_termination(
                    session_id,
                    &runtime_termination_proof,
                )
                .await
                .err()
            } else {
                self.clear_runtime_session_state(session_id).await.err()
            };
            if discard_error.is_some() || clear_error.is_some() {
                let mut causes = Vec::new();
                if let Some(error) = discard_error {
                    causes.push(format!("live-session discard failed: {error}"));
                }
                if let Some(error) = clear_error {
                    causes.push(format!("runtime unregister failed: {error}"));
                }
                return Err(
                    meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                        format!(
                            "MCP runtime completion cleanup failed for {session_id}: {}",
                            causes.join("; ")
                        ),
                    ),
                );
            }
        }
        Ok(release_pre_admission)
    }

    async fn runtime_wait_failure_releases_pre_admission(
        &self,
        session_id: &SessionId,
        error: &meerkat_runtime::completion::CompletionWaitError,
    ) -> Result<bool, meerkat_runtime::completion::CompletionWaitError> {
        match self
            .runtime_adapter
            .resolve_runtime_completion_wait_failure(session_id, error)
            .await
        {
            Ok(authority) => Ok(authority.releases_pre_admission()),
            Err(authority_error) => Err(
                meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(format!(
                    "{error}; MCP runtime wait-failure cleanup authority unavailable for {session_id}: {authority_error}"
                )),
            ),
        }
    }

    async fn insert_runtime_pre_admission(
        &self,
        session_id: SessionId,
        input_id: meerkat_core::lifecycle::InputId,
        admission: meerkat::RuntimeContextAdmissionGuard,
    ) -> Result<(), SessionError> {
        let mut pre_admissions = self
            .runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = pre_admissions.entry(session_id.clone()).or_default();
        if entries.iter().any(|entry| entry.input_id == input_id) {
            return Err(SessionError::Busy { id: session_id });
        }
        entries.push(RuntimePreAdmissionEntry {
            input_id,
            admission: admission.into(),
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
        let mut pre_admissions = self
            .runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = pre_admissions.get_mut(session_id)?;
        let index = entries
            .iter()
            .position(|entry| input_ids.contains(&entry.input_id))?;
        let entry = entries.remove(index);
        if entries.is_empty() {
            pre_admissions.remove(session_id);
        }
        Some(entry.admission.into_admission())
    }

    async fn discard_runtime_pre_admission(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) {
        discard_mcp_runtime_pre_admission(&self.runtime_pre_admissions, session_id, input_id);
    }

    async fn discard_all_runtime_pre_admissions_for_session(&self, session_id: &SessionId) {
        self.runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    pub(crate) fn runtime_registration_lock(
        &self,
        session_id: &SessionId,
    ) -> RuntimeRegistrationLockLease {
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
        RuntimeRegistrationLockLease {
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
    ) -> Result<(), SessionError> {
        let runtime_is_registered = self.runtime_adapter.contains_session(session_id).await;
        let has_active_inputs = if runtime_is_registered {
            !self
                .runtime_adapter
                .list_active_inputs(session_id)
                .await
                .map_err(runtime_driver_error_to_session_error)?
                .is_empty()
        } else {
            false
        };
        if has_active_inputs {
            return Ok(());
        }
        if self
            .runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id)
        {
            return Ok(());
        }
        if !runtime_was_registered {
            self.unregister_runtime_session_if_present(session_id)
                .await?;
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
        Ok(())
    }

    pub(crate) async fn clear_session_if_new(
        &self,
        session_id: &SessionId,
        runtime_was_registered: bool,
        runtime_state_existed: bool,
    ) -> Result<(), SessionError> {
        let lock = self.runtime_registration_lock(session_id);
        let _guard = lock.mutex().lock().await;
        self.clear_session_if_new_locked(session_id, runtime_was_registered, runtime_state_existed)
            .await
    }

    pub(crate) async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<RequestEventTx>,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), meerkat_runtime::RuntimeDriverError>
    {
        if self
            .authoritative_session_archived(session_id)
            .await
            .map_err(|error| meerkat_runtime::RuntimeDriverError::Internal(error.to_string()))?
        {
            self.clear_session(session_id).await.map_err(|error| {
                meerkat_runtime::RuntimeDriverError::Internal(error.to_string())
            })?;
            return Err(meerkat_runtime::RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Retired,
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
            pre_admission_registration = Some(RuntimePreAdmissionRegistration::new(
                Arc::new(McpPreAdmissionLedger {
                    pre_admissions: Arc::clone(&self.runtime_pre_admissions),
                }),
                session_id.clone(),
                requested_input_id.clone(),
            ));
        }

        let runtime_was_registered = self.runtime_adapter.contains_session(session_id).await;
        let runtime_state_existed = self.runtime_sessions.read().await.contains_key(session_id);
        let state = match self.runtime_session_state(session_id).await {
            Ok(state) => state,
            Err(error) => {
                self.discard_runtime_pre_admission(session_id, &requested_input_id)
                    .await;
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
                if let Err(cleanup) = self
                    .clear_session_if_new_locked(
                        session_id,
                        runtime_was_registered,
                        runtime_state_existed,
                    )
                    .await
                {
                    return Err(meerkat_runtime::RuntimeDriverError::Internal(format!(
                        "{error}; additionally failed to clear newly prepared MCP runtime session: {cleanup}"
                    )));
                }
                return Err(meerkat_runtime::RuntimeDriverError::Internal(
                    error.to_string(),
                ));
            }
        };
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
                if let Err(cleanup) = self
                    .clear_session_if_new_locked(
                        session_id,
                        runtime_was_registered,
                        runtime_state_existed,
                    )
                    .await
                {
                    return Err(meerkat_runtime::RuntimeDriverError::Internal(format!(
                        "{error}; additionally failed to clear newly prepared MCP runtime session: {cleanup}"
                    )));
                }
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
                    rekey_mcp_runtime_pre_admission(
                        &self.runtime_pre_admissions,
                        session_id,
                        &requested_input_id,
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
            .await
            .map_err(|error| meerkat_runtime::RuntimeDriverError::Internal(error.to_string()))?;
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
        if self
            .session_archived_by_authority(&session_id, &session)
            .await?
        {
            return Err(SessionError::NotFound { id: session_id });
        }
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
                .await
                .map_err(runtime_driver_error_to_session_error)?;
                Ok(result)
            }
            Err(error) => Err(surface_materialize_session_error(error)),
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
        if self
            .session_archived_by_authority(session_id, &session)
            .await?
        {
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
                realm_id: Some(self.realm_id.clone()),
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
        if self
            .session_archived_by_authority(session_id, &session)
            .await?
        {
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
                realm_id: Some(self.realm_id.clone()),
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

fn runtime_driver_error_to_session_error(
    error: meerkat_runtime::RuntimeDriverError,
) -> SessionError {
    SessionError::Agent(AgentError::InternalError(error.to_string()))
}

fn combine_session_cleanup_error(
    primary: impl std::fmt::Display,
    cleanup: impl std::fmt::Display,
    context: &str,
) -> SessionError {
    SessionError::Agent(AgentError::InternalError(format!(
        "{primary}; additionally failed to {context}: {cleanup}"
    )))
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
            .cancel_after_boundary_with_machine_authority(
                &self.session_id,
                self.context.runtime_adapter.session_control_authority(),
            )
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
            .interrupt_with_machine_authority(
                &self.session_id,
                self.context.runtime_adapter.session_control_authority(),
            )
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

fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            content: append.content.clone(),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            peer_response_terminal: None,
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

    if context.authoritative_session_archived(session_id).await? {
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
                        return Err(error);
                    }
                } else if let Err(error) =
                    Box::pin(context.rematerialize_persisted_session(session_id, state.clone()))
                        .await
                {
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

    let boundary = primitive.apply_boundary();
    let contributing_input_ids = primitive.contributing_input_ids().to_vec();
    let typed_turn_appends = primitive.typed_turn_appends();
    let prompt = primitive.extract_content_input();
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

    let mut turn_request = StartTurnRequest {
        injected_context: Vec::new(),
        prompt: prompt.clone(),
        system_prompt: None,
        event_tx: event_tx.clone(),
        runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
            HandlingMode::Queue,
            primitive
                .turn_metadata()
                .and_then(|meta| meta.turn_tool_overlay.clone()),
            pre_turn_context_appends.clone(),
            primitive.turn_metadata().cloned(),
        )
        .with_typed_turn_appends(typed_turn_appends.clone()),
    };
    meerkat::surface::inject_workgraph_attention_turn_overlay(
        context.service.as_ref(),
        Some(&context.workgraph_service),
        session_id,
        &mut turn_request,
    )
    .await
    .map_err(|error| {
        SessionError::Agent(meerkat_core::AgentError::InternalError(error.to_string()))
    })?;

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
                    return Err(error);
                }
            } else if let Err(error) =
                Box::pin(context.rematerialize_persisted_session(session_id, state.clone())).await
            {
                return Err(error);
            }
            let mut recovered_turn_request = StartTurnRequest {
                injected_context: Vec::new(),
                prompt,
                system_prompt: None,
                event_tx,
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                    HandlingMode::Queue,
                    primitive
                        .turn_metadata()
                        .and_then(|meta| meta.turn_tool_overlay.clone()),
                    pre_turn_context_appends,
                    primitive.turn_metadata().cloned(),
                )
                .with_typed_turn_appends(typed_turn_appends),
            };
            meerkat::surface::inject_workgraph_attention_turn_overlay(
                context.service.as_ref(),
                Some(&context.workgraph_service),
                session_id,
                &mut recovered_turn_request,
            )
            .await
            .map_err(|error| {
                SessionError::Agent(meerkat_core::AgentError::InternalError(error.to_string()))
            })?;
            context
                .service
                .apply_runtime_turn_outcome(
                    session_id,
                    run_id,
                    recovered_turn_request,
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
        match Box::pin(apply_runtime_turn(
            &self.context,
            &self.state,
            &self.session_id,
            run_id,
            &primitive,
        ))
        .await
        {
            Ok(output) => Ok(output),
            Err(error @ SessionError::NotFound { .. }) => {
                let archived = self
                    .context
                    .authoritative_session_archived(&self.session_id)
                    .await
                    .map_err(CoreExecutorError::apply_failed_from_session_error)?;
                if archived {
                    Err(CoreExecutorError::archived_session_requires_teardown(
                        error.to_string(),
                    ))
                } else {
                    Err(CoreExecutorError::session_unavailable_requires_teardown(
                        error.to_string(),
                    ))
                }
            }
            Err(error) => Err(CoreExecutorError::apply_failed_from_session_error(error)),
        }
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .reconcile_runtime_compaction_projections(&self.session_id, intents.to_vec())
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .abort_uncommitted_compaction_projections(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .cancel_after_boundary_with_machine_authority(
                &self.session_id,
                self.context.runtime_adapter.session_control_authority(),
            )
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        #[cfg(test)]
        let injected_discard_failure = self
            .context
            .fail_external_cleanup_discard_once
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        #[cfg(test)]
        let discard_result = if injected_discard_failure {
            Err(SessionError::Unsupported(
                "synthetic fail-once MCP live-session discard".to_string(),
            ))
        } else {
            self.context
                .service
                .discard_live_session(&self.session_id)
                .await
        };
        #[cfg(not(test))]
        let discard_result = self
            .context
            .service
            .discard_live_session(&self.session_id)
            .await;
        match discard_result {
            Ok(()) | Err(SessionError::NotFound { .. }) => {
                self.context
                    .clear_external_runtime_session_state(&self.session_id)
                    .await;
                Ok(())
            }
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

    async fn pending_completion_handle(
        adapter: &meerkat_runtime::meerkat_machine::MeerkatMachine,
        session_id: &SessionId,
    ) -> meerkat_runtime::CompletionHandle {
        adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("test machine should prepare runtime bindings");
        let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
            "pending completion fixture",
            None,
        ));
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, input)
            .await
            .expect("test machine should accept pending completion input");
        handle.expect("pending completion input should return a completion handle")
    }

    async fn runtime_terminated_completion_handle(
        adapter: &meerkat_runtime::meerkat_machine::MeerkatMachine,
        session_id: &SessionId,
        reason: &str,
    ) -> meerkat_runtime::CompletionHandle {
        let handle = pending_completion_handle(adapter, session_id).await;
        adapter
            .stop_runtime_executor(session_id, reason)
            .await
            .expect("test machine should resolve pending completion as runtime terminated");
        handle
    }

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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let persistence = PersistenceBundle::new(
            session_store,
            runtime_store,
            Arc::new(MemoryBlobStore::new()),
        );

        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, runtime_adapter) =
            build_runtime_backed_service(builder, active_session_capacity, persistence);
        let config_store = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));

        McpRuntimeIngressContext::new(McpRuntimeIngressResources {
            service: Arc::new(service),
            runtime_adapter,
            workgraph_service: meerkat::WorkGraphService::with_scope(
                Arc::new(meerkat::MemoryWorkGraphStore::new()),
                "mcp-runtime-test".to_string(),
                meerkat::WorkNamespace::default(),
            ),
            config_runtime: Arc::new(ConfigRuntime::new(
                config_store,
                temp.path().join("config-state.json"),
            )),
            realm_id: RealmId::parse("mcp-runtime-test").expect("valid realm id"),
            instance_id: None,
            backend: "test".to_string(),
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            runtime_sessions: Arc::new(RwLock::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(StdMutex::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
        })
    }

    async fn archive_with_test_machine_authority(
        context: &McpRuntimeIngressContext,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        context
            .service
            .archive_with_machine_protocol(
                session_id,
                meerkat::MachineSessionArchiveProtocol::from_machine(
                    context.runtime_adapter.as_ref(),
                ),
            )
            .await
    }

    #[test]
    fn mcp_context_system_notice_projects_via_typed_notice() {
        let blocks = vec![meerkat_core::types::SystemNoticeBlock::Comms {
            sender_taint: None,
            kind: meerkat_core::types::CommsNoticeKind::ResponseTerminal,
            direction: meerkat_core::types::SystemNoticeDirection::Incoming,
            peer: None,
            request_id: Some("req-1".to_string()),
            intent: Some("checksum_token".to_string()),
            status: Some("completed".to_string()),
            summary: Some("Peer terminal response".to_string()),
            payload: None,
            content: Vec::new(),
        }];
        let content = CoreRenderable::SystemNotice {
            kind: meerkat_core::types::SystemNoticeKind::Comms,
            body: Some("Peer terminal response context".to_string()),
            blocks: blocks.clone(),
        };

        assert_eq!(
            content.render_text(),
            meerkat_core::SystemNoticeMessage::with_blocks(
                meerkat_core::types::SystemNoticeKind::Comms,
                Some("Peer terminal response context".to_string()),
                blocks,
            )
            .model_projection_text()
        );
    }

    fn create_request(
        prompt: &str,
        initial_turn: meerkat_core::service::InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: prompt.to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(1024),
            event_tx: None,
            initial_turn,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions::default()),
            labels: None,
        }
    }

    async fn create_session_with_test_machine(
        context: &McpRuntimeIngressContext,
        prompt: &str,
        initial_turn: meerkat_core::service::InitialTurnPolicy,
    ) -> Result<RunResult, SessionError> {
        context
            .materialize_with_state(
                Session::new(),
                create_request(prompt, initial_turn),
                false,
                Arc::new(McpRuntimeSessionState::default()),
                None,
            )
            .await
    }

    async fn create_deferred_session_with_generated_authority(
        context: &McpRuntimeIngressContext,
        prompt: &str,
    ) -> SessionId {
        context
            .service
            .create_session(create_request(
                prompt,
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("deferred session create should seed generated authority")
            .session_id
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
                key: "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-invalid"
                    .to_string(),
                content: CoreRenderable::Text {
                    text: "Peer terminal response from 550e8400-e29b-41d4-a716-446655440000\nRequest ID: req-invalid\nStatus: completed\ninvalid".to_string(),
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
    async fn mcp_runtime_rematerialize_rejects_archived_without_pre_handoff_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(1024),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        archive_with_test_machine_authority(&context, &session_id)
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
            context.runtime_adapter.contains_session(&session_id).await,
            "archived MCP runtime rematerialization must retain registration for canonical post-handoff teardown"
        );
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "archived MCP runtime rematerialization must not install runtime session state"
        );
        context
            .clear_session(&session_id)
            .await
            .expect("test cleanup should run outside CoreExecutor::apply");
    }

    #[tokio::test]
    async fn mcp_runtime_apply_recovers_persisted_session_for_context_only_apply() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(1024),
                event_tx: None,
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
        context
            .clear_session(&session_id)
            .await
            .expect("test runtime session should clear");
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "test must remove runtime registration before context-only recovery"
        );

        let state = context
            .ensure_session(&session_id)
            .await
            .expect("MCP runtime executor should attach");
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

        let session_snapshot: Session = serde_json::from_slice(
            output
                .session_snapshot
                .as_deref()
                .expect("context-only apply should return a machine commit snapshot"),
        )
        .expect("machine commit snapshot should deserialize");
        let system_context = session_snapshot
            .system_context_state()
            .expect("context-only recovery should persist system-context state");
        assert!(
            system_context.applied().iter().any(|append| {
                append.source.as_deref() == Some("mcp-context-recovery")
                    && append
                        .content
                        .render_text()
                        .contains("mcp recovered context")
            }),
            "context-only recovery should persist MCP runtime context append: {system_context:?}"
        );
    }

    #[tokio::test]
    async fn mcp_runtime_accept_clears_state_for_archived_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(1024),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        context
            .ensure_session(&session_id)
            .await
            .expect("MCP runtime executor should attach");
        assert!(context.runtime_adapter.contains_session(&session_id).await);

        archive_with_test_machine_authority(&context, &session_id)
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
                    state: meerkat_runtime::RuntimeState::Retired
                })
            ),
            "archived MCP accept should reject as not ready: {rejected:?}"
        );
        assert!(
            !context.runtime_adapter.contains_session(&session_id).await,
            "archived MCP accept may clear local runtime registration after reporting Retired"
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
    async fn mcp_runtime_pre_admission_completion_cleanup_clears_terminated_runtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session_id =
            create_deferred_session_with_generated_authority(&context, "runtime cleanup target")
                .await;
        let completion_handle = runtime_terminated_completion_handle(
            context.runtime_adapter.as_ref(),
            &session_id,
            "runtime stopped during cleanup",
        )
        .await;
        context.runtime_sessions.write().await.insert(
            session_id.clone(),
            Arc::new(McpRuntimeSessionState::default()),
        );
        context.mcp_adapters.lock().await.insert(
            session_id.to_string(),
            Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new())),
        );
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
            .reserve_create_session_admission()
            .await
            .expect("reserve active admission");
        context
            .insert_runtime_pre_admission(session_id.clone(), requested_input_id.clone(), admission)
            .await
            .expect("insert pending pre-admission");

        let handle = wrap_mcp_runtime_pre_admission_cleanup(
            context.clone(),
            session_id.clone(),
            requested_input_id,
            accepted_input_id,
            completion_handle,
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
            .await
            .expect("cleanup handle should finish")
            .expect("authorized cleanup should preserve the completion outcome");

        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "runtime termination cleanup must remove MCP runtime session state"
        );
        assert!(
            !context
                .mcp_adapters
                .lock()
                .await
                .contains_key(&session_id.to_string()),
            "runtime termination cleanup must remove MCP adapter bindings"
        );
        assert!(
            context
                .runtime_pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&session_id)
                .is_none(),
            "runtime termination cleanup must release MCP runtime pre-admission"
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
    async fn mcp_runtime_completion_preserves_outcome_after_machine_saga_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session_id = create_deferred_session_with_generated_authority(
            &context,
            "MCP machine-saga cleanup target",
        )
        .await;
        let completion_handle = runtime_terminated_completion_handle(
            context.runtime_adapter.as_ref(),
            &session_id,
            "MCP external cleanup precedes machine saga",
        )
        .await;
        let executor_state = Arc::new(McpRuntimeSessionState::default());
        context
            .runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::clone(&executor_state));
        let mut executor =
            McpSessionRuntimeExecutor::new(context.clone(), session_id.clone(), executor_state);
        meerkat_core::lifecycle::CoreExecutor::cleanup_after_runtime_stop_terminalized(
            &mut executor,
        )
        .await
        .expect("MCP executor external cleanup should succeed");
        assert!(
            context.runtime_adapter.contains_session(&session_id).await,
            "MCP external cleanup must not recursively unregister machine authority"
        );
        context
            .runtime_adapter
            .unregister_session(&session_id)
            .await
            .expect("machine-owned saga should remove the stopped MCP runtime");
        assert!(!context.runtime_adapter.contains_session(&session_id).await);

        let handle = wrap_mcp_runtime_pre_admission_cleanup(
            context,
            session_id,
            InputId::new(),
            InputId::new(),
            completion_handle,
        );
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
            .await
            .expect("machine-saga MCP cleanup should finish")
            .expect("verified machine-saga cleanup should preserve the completion outcome");
        assert!(
            matches!(
                outcome,
                meerkat_runtime::CompletionOutcome::RuntimeTerminated { .. }
            ),
            "MCP machine-saga cleanup must publish RuntimeTerminated: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn mcp_external_cleanup_failure_retains_all_projection_retry_anchors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session_id = create_deferred_session_with_generated_authority(
            &context,
            "MCP external cleanup retry anchor",
        )
        .await;
        let executor_state = Arc::new(McpRuntimeSessionState::default());
        context
            .runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::clone(&executor_state));
        context.mcp_adapters.lock().await.insert(
            session_id.to_string(),
            Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new())),
        );
        let input_id = InputId::new();
        let admission = context
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve cleanup retry admission");
        context
            .insert_runtime_pre_admission(session_id.clone(), input_id, admission)
            .await
            .expect("insert cleanup retry pre-admission");
        context
            .fail_external_cleanup_discard_once
            .store(true, std::sync::atomic::Ordering::Release);
        let mut executor =
            McpSessionRuntimeExecutor::new(context.clone(), session_id.clone(), executor_state);

        let error = meerkat_core::lifecycle::CoreExecutor::cleanup_after_runtime_stop_terminalized(
            &mut executor,
        )
        .await
        .expect_err("injected live-session discard failure must remain visible");
        assert!(
            error
                .to_string()
                .contains("synthetic fail-once MCP live-session discard")
        );
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
        assert!(
            context
                .runtime_pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&session_id),
            "failed discard must retain the pre-admission retry anchor"
        );
        assert!(
            context
                .service
                .has_live_session(&session_id)
                .await
                .expect("inspect live session after failed cleanup")
        );

        meerkat_core::lifecycle::CoreExecutor::cleanup_after_runtime_stop_terminalized(
            &mut executor,
        )
        .await
        .expect("retry should clear projections after discard succeeds");
        assert!(
            !context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id)
        );
        assert!(
            !context
                .mcp_adapters
                .lock()
                .await
                .contains_key(&session_id.to_string())
        );
        assert!(
            !context
                .runtime_pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&session_id)
        );
    }

    #[tokio::test]
    async fn mcp_runtime_completion_cleanup_preserves_unrelated_pre_admission() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 3).await;
        let session_id = create_deferred_session_with_generated_authority(
            &context,
            "runtime unrelated cleanup target",
        )
        .await;
        let completion_handle = runtime_terminated_completion_handle(
            context.runtime_adapter.as_ref(),
            &session_id,
            "runtime stopped with unrelated admission",
        )
        .await;

        let requested_input_id = InputId::new();
        let accepted_input_id = InputId::new();
        let unrelated_input_id = InputId::new();
        let requested_admission = context
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve requested active admission");
        let unrelated_admission = context
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve unrelated active admission");
        context
            .insert_runtime_pre_admission(
                session_id.clone(),
                requested_input_id.clone(),
                requested_admission,
            )
            .await
            .expect("insert requested pre-admission");
        context
            .insert_runtime_pre_admission(
                session_id.clone(),
                unrelated_input_id.clone(),
                unrelated_admission,
            )
            .await
            .expect("insert unrelated pre-admission");

        let handle = wrap_mcp_runtime_pre_admission_cleanup(
            context.clone(),
            session_id.clone(),
            requested_input_id.clone(),
            accepted_input_id,
            completion_handle,
        );
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
            .await
            .expect("cleanup handle should finish");

        {
            let pre_admissions = context
                .runtime_pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entries = pre_admissions
                .get(&session_id)
                .expect("unrelated pre-admission must remain after completion cleanup");
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.input_id != requested_input_id),
                "generated cleanup release must remove only the completed input pre-admission"
            );
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.input_id == unrelated_input_id),
                "runtime cleanup must not release unrelated MCP pre-admission"
            );
        }

        context
            .discard_runtime_pre_admission(&session_id, &unrelated_input_id)
            .await;
    }

    #[tokio::test]
    async fn mcp_runtime_completion_cleanup_retains_pre_admission_on_authority_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let source_session_id = SessionId::new();
        let target_session_id = SessionId::new();
        let completion_handle = runtime_terminated_completion_handle(
            context.runtime_adapter.as_ref(),
            &source_session_id,
            "runtime stopped during mismatched cleanup",
        )
        .await;

        let requested_input_id = InputId::new();
        let accepted_input_id = InputId::new();
        let admission = context
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve active admission");
        context
            .insert_runtime_pre_admission(
                target_session_id.clone(),
                requested_input_id.clone(),
                admission,
            )
            .await
            .expect("insert pending pre-admission");

        let handle = wrap_mcp_runtime_pre_admission_cleanup(
            context.clone(),
            target_session_id.clone(),
            requested_input_id.clone(),
            accepted_input_id,
            completion_handle,
        );
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
            .await
            .expect("cleanup handle should finish")
            .expect_err("authority mismatch must withhold the completion outcome");
        assert!(
            error.to_string().contains("cleanup authority unavailable"),
            "unexpected cleanup error: {error}"
        );

        assert!(
            context
                .runtime_pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&target_session_id)
                .is_some_and(|entries| entries
                    .iter()
                    .any(|entry| entry.input_id == requested_input_id)),
            "MCP runtime pre-admission must be retained when generated cleanup authority rejects the observation"
        );

        context
            .discard_runtime_pre_admission(&target_session_id, &requested_input_id)
            .await;
    }

    #[tokio::test]
    async fn mcp_runtime_wait_failure_authority_controls_pre_admission_release() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let session_id = SessionId::new();
        context
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare generated runtime authority");

        assert!(
            context
                .runtime_wait_failure_releases_pre_admission(
                    &session_id,
                    &meerkat_runtime::completion::CompletionWaitError::ChannelClosed,
                )
                .await
                .expect("generated wait-failure authority should resolve"),
            "generated MCP waiter-failure authority should release pre-admission for channel-close failures"
        );

        let missing_session_id = SessionId::new();
        let error = context
            .runtime_wait_failure_releases_pre_admission(
                &missing_session_id,
                &meerkat_runtime::completion::CompletionWaitError::ChannelClosed,
            )
            .await
            .expect_err(
                "MCP waiter-failure cleanup must fail closed when generated authority is absent",
            );
        assert!(error.to_string().contains("authority unavailable"));
    }

    #[tokio::test]
    async fn mcp_runtime_accept_capacity_full_rejects_before_input_accept() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let target = create_session_with_test_machine(
            &context,
            "completed target",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        )
        .await
        .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context
            .ensure_session(&target_session_id)
            .await
            .expect("MCP runtime executor should attach");

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
        let target = create_session_with_test_machine(
            &context,
            "completed target",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        )
        .await
        .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context
            .ensure_session(&target_session_id)
            .await
            .expect("MCP runtime executor should attach");

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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let target = create_session_with_test_machine(
            &context,
            "completed target",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        )
        .await
        .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        context
            .runtime_adapter
            .unregister_session(&target_session_id)
            .await
            .expect("target runtime session should unregister cleanly");
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
            .await
            .expect("pending pre-admission should make cleanup a no-op");
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
        let target = create_session_with_test_machine(
            &context,
            "completed target",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        )
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
            .await
            .expect("target runtime session should unregister cleanly");
        context
            .clear_session(&target_session_id)
            .await
            .expect("test target runtime session should clear");
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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let target = create_session_with_test_machine(
            &context,
            "completed target",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        )
        .await
        .expect("completed target create should succeed");
        let target_session_id = target.session_id;
        let state = context
            .ensure_session(&target_session_id)
            .await
            .expect("MCP runtime executor should attach");
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
            .await
            .expect("runtime executor registration should succeed");
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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let target = create_session_with_test_machine(
            &context,
            "completed target",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        )
        .await
        .expect("completed target create should succeed");
        let session_id = target.session_id;
        let state = context
            .ensure_session(&session_id)
            .await
            .expect("MCP runtime executor should attach");
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard completed live session before recovery");
        context
            .runtime_adapter
            .unregister_session(&session_id)
            .await
            .expect("runtime session should unregister cleanly");

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
            context.runtime_adapter.contains_session(&session_id).await,
            "direct apply helper must retain runtime authority until post-handoff unregister"
        );

        context
            .runtime_adapter
            .unregister_session(&session_id)
            .await
            .expect("test cleanup should drive canonical unregister");

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
    async fn mcp_runtime_apply_requests_post_handoff_teardown_for_archived_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(1024),
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id;
        let state = context
            .ensure_session(&session_id)
            .await
            .expect("MCP runtime executor should attach");
        assert!(context.runtime_adapter.contains_session(&session_id).await);

        archive_with_test_machine_authority(&context, &session_id)
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
        let mut executor =
            McpSessionRuntimeExecutor::new(context.clone(), session_id.clone(), Arc::clone(&state));
        let rejected = CoreExecutor::apply(&mut executor, RunId::new(), primitive).await;
        assert!(matches!(
            rejected,
            Err(CoreExecutorError::TeardownRequired {
                reason: meerkat_core::lifecycle::core_executor::CoreExecutorTeardownReason::ArchivedSession,
                ..
            })
        ));
        assert!(
            context.runtime_adapter.contains_session(&session_id).await,
            "archived MCP apply must retain registration until exact executor handoff"
        );
        assert!(
            context
                .runtime_sessions
                .read()
                .await
                .contains_key(&session_id),
            "archived MCP apply must retain runtime session state as cleanup retry anchor"
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

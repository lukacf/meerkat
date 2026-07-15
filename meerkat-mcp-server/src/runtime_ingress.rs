use async_trait::async_trait;
use meerkat::session_runtime::admission::{
    RuntimePreAdmissionEntry, RuntimePreAdmissionRegistration, RuntimePreAdmissionRestore,
    RuntimeRegistrationLockLease,
};
#[cfg(test)]
use meerkat::{CreateSessionRequest, RunResult};
use meerkat::{FactoryAgentBuilder, PersistentSessionService, Session, SessionServiceControlExt};
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::error::AgentError;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle, CoreExecutorPostStopCleanupHandle, CoreExecutorPublicationHandle,
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
use std::sync::{
    Arc, Mutex as StdMutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, RwLock, mpsc};

type RequestEventTx = mpsc::Sender<EventEnvelope<AgentEvent>>;

#[derive(Debug)]
pub(crate) enum McpRuntimeIngressError {
    Session(SessionError),
    Runtime(meerkat_runtime::RuntimeDriverError),
}

impl std::fmt::Display for McpRuntimeIngressError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for McpRuntimeIngressError {}

impl From<SessionError> for McpRuntimeIngressError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<meerkat_runtime::RuntimeDriverError> for McpRuntimeIngressError {
    fn from(error: meerkat_runtime::RuntimeDriverError) -> Self {
        Self::Runtime(error)
    }
}

impl McpRuntimeIngressError {
    pub(crate) fn into_tool_error(self, context: impl std::fmt::Display) -> crate::ToolCallError {
        match self {
            Self::Session(error) => session_error_to_tool_error(error, context),
            Self::Runtime(error) => crate::ToolCallError::internal(format!("{context}: {error}")),
        }
    }
}

pub(crate) type SharedMcpSidecars = Arc<RwLock<HashMap<SessionId, McpSessionSidecar>>>;
pub(crate) type SharedMcpRuntimeRegistrationLocks =
    Arc<StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>>;

/// Process-local MCP configuration for one logical session.
///
/// Router connections and callback handlers survive an executor attachment
/// replacement in this process, but they are intentionally not durable across
/// an MCP host restart.
#[derive(Clone)]
pub(crate) struct McpLogicalSessionConfig {
    incarnation: uuid::Uuid,
    revision: u64,
    router: Option<Arc<McpRouterAdapter>>,
    callback_tools: Option<Arc<dyn AgentToolDispatcher>>,
}

impl McpLogicalSessionConfig {
    fn fresh() -> Self {
        Self {
            incarnation: uuid::Uuid::new_v4(),
            revision: 0,
            router: None,
            callback_tools: None,
        }
    }
}

/// The one MCP sidecar slot for a logical session.
///
/// `logical` is session-scoped. `attachment` is exact-executor-scoped and is
/// compare-and-removed by the machine-minted attachment witness.
pub(crate) struct McpSessionSidecar {
    logical: McpLogicalSessionConfig,
    attachment: Option<Arc<McpRuntimeAttachmentState>>,
}

#[derive(Clone)]
struct McpLogicalConfigCandidate {
    previous: Option<McpLogicalSessionConfig>,
    base_incarnation: Option<uuid::Uuid>,
    base_revision: Option<u64>,
    desired: McpLogicalSessionConfig,
}

struct McpAttachmentAcquisition {
    attachment: Arc<McpRuntimeAttachmentState>,
    _registration_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl McpAttachmentAcquisition {
    fn existing(
        attachment: Arc<McpRuntimeAttachmentState>,
        registration_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Self {
        Self {
            attachment,
            _registration_guard: registration_guard,
        }
    }

    fn into_attachment(self) -> Arc<McpRuntimeAttachmentState> {
        self.attachment
    }
}
impl McpLogicalConfigCandidate {
    fn with_router(mut self, router: Arc<McpRouterAdapter>) -> Self {
        self.desired.router = Some(router);
        self.bump_revision();
        self
    }

    fn with_callback_tools(mut self, callback_tools: Option<Arc<dyn AgentToolDispatcher>>) -> Self {
        self.desired.callback_tools = callback_tools;
        self.bump_revision();
        self
    }

    fn bump_revision(&mut self) {
        self.desired.revision = self.desired.revision.saturating_add(1);
    }
}

pub(crate) enum McpCallbackConfig {
    Preserve,
    Replace(Option<Arc<dyn AgentToolDispatcher>>),
}

#[derive(Clone)]
pub(crate) struct McpLogicalConfigSnapshot {
    pub(crate) sidecar_exists: bool,
    pub(crate) router: Option<Arc<McpRouterAdapter>>,
    pub(crate) callback_tools: Option<Arc<dyn AgentToolDispatcher>>,
    pub(crate) attachment_witness: Option<meerkat_runtime::RuntimeExecutorAttachmentWitness>,
}

/// Serialized lease for one live logical-session router.
///
/// Field order is intentional: Rust drops fields in declaration order, so the
/// owned mutex guard releases its Arc before the registration lease performs
/// the Weak-map eviction check.
pub(crate) struct McpLiveRouterLease {
    _operation_guard: tokio::sync::OwnedMutexGuard<()>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
    _lease: RuntimeRegistrationLockLease,
    router: Arc<McpRouterAdapter>,
    _witness: meerkat_runtime::RuntimeExecutorAttachmentWitness,
    _actor_lease: meerkat::LiveSessionActorTurnBoundaryLease,
}

impl McpLiveRouterLease {
    pub(crate) fn router(&self) -> Arc<McpRouterAdapter> {
        Arc::clone(&self.router)
    }
}

/// Sync restore hook backing the shared [`RuntimePreAdmissionRegistration`]
/// RAII seam for MCP (defined in `meerkat::session_runtime::admission`, the
/// same seam RPC consumes). When an un-disarmed registration drops, the shared
/// guard calls `restore_or_release`, which removes the pre-admission entry —
/// dropping its `RuntimePreAdmission` and releasing the reserved capacity.
struct McpPreAdmissionLedger {
    attachment: Arc<McpRuntimeAttachmentState>,
}

impl RuntimePreAdmissionRestore for McpPreAdmissionLedger {
    fn restore_or_release(
        &self,
        _session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) {
        self.attachment.discard_pre_admission(input_id);
    }
}

struct McpQueuedTurnRegistration {
    attachment: Arc<McpRuntimeAttachmentState>,
    input_id: meerkat_core::lifecycle::InputId,
    armed: bool,
}

impl McpQueuedTurnRegistration {
    fn rekey(&mut self, input_id: meerkat_core::lifecycle::InputId) -> bool {
        let rekeyed = self
            .attachment
            .rekey_turn_context(&self.input_id, input_id.clone());
        if rekeyed {
            self.input_id = input_id;
        }
        rekeyed
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for McpQueuedTurnRegistration {
    fn drop(&mut self) {
        if self.armed {
            self.attachment.discard_turn_context(&self.input_id);
        }
    }
}

fn wrap_mcp_runtime_pre_admission_cleanup(
    context: McpRuntimeIngressContext,
    session_id: SessionId,
    attachment: Arc<McpRuntimeAttachmentState>,
    requested_input_id: meerkat_core::lifecycle::InputId,
    accepted_input_id: meerkat_core::lifecycle::InputId,
    handle: CompletionHandle,
) -> CompletionHandle {
    handle.with_resultful_completion_cleanup(move |completion| async move {
        // Completion cleanup is attachment-exact. It must not hold the
        // surface registration gate while shared retirement establishes B;
        // every removal below compare-checks the attachment witness.
        let cleanup = match completion {
            Ok(cleanup_observation) => {
                context
                    .cleanup_runtime_after_completion_outcome_locked(
                        &session_id,
                        &attachment,
                        cleanup_observation,
                    )
                    .await?
            }
            Err(error) if error.is_attachment_replaced() => {
                McpRuntimeCompletionCleanup::StaleAttachment { error }
            }
            Err(error) => {
                let release_pre_admission = context
                    .runtime_wait_failure_releases_pre_admission(&session_id, &error)
                    .await?;
                McpRuntimeCompletionCleanup::Current {
                    release_pre_admission,
                }
            }
        };
        let (release_pre_admission, stale_error) = match cleanup {
            McpRuntimeCompletionCleanup::Current {
                release_pre_admission,
            } => (release_pre_admission, None),
            McpRuntimeCompletionCleanup::StaleAttachment { error } => (true, Some(error)),
        };
        if release_pre_admission {
            attachment.discard_pre_admission(&requested_input_id);
            attachment.discard_pre_admission(&accepted_input_id);
        }
        if let Some(error) = stale_error {
            attachment.discard_turn_context(&requested_input_id);
            attachment.discard_turn_context(&accepted_input_id);
            return Err(error);
        }
        Ok(())
    })
}

enum McpRuntimeCompletionCleanup {
    Current {
        release_pre_admission: bool,
    },
    /// The waiter belongs to an attachment that no longer owns this session.
    /// Its request-local admission is releasable, but its successful outcome
    /// must be withheld so replacement B can never be observed as A's work.
    StaleAttachment {
        error: meerkat_runtime::completion::CompletionWaitError,
    },
}

impl McpRuntimeCompletionCleanup {
    fn stale() -> Self {
        Self::StaleAttachment {
            error: meerkat_runtime::completion::CompletionWaitError::AttachmentReplaced,
        }
    }
}

pub(crate) struct McpRuntimeIngressResources {
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    pub runtime_adapter: Arc<MeerkatMachine>,
    pub workgraph_service: meerkat::WorkGraphService,
    pub config_runtime: Arc<ConfigRuntime>,
    pub realm_id: meerkat_core::connection::RealmId,
    pub instance_id: Option<String>,
    pub backend: String,
    pub sidecars: SharedMcpSidecars,
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
    sidecars: SharedMcpSidecars,
    runtime_registration_locks: SharedMcpRuntimeRegistrationLocks,
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
            sidecars: resources.sidecars,
            runtime_registration_locks: resources.runtime_registration_locks,
        }
    }

    async fn snapshot_config_candidate(&self, session_id: &SessionId) -> McpLogicalConfigCandidate {
        let existing = self
            .sidecars
            .read()
            .await
            .get(session_id)
            .map(|sidecar| sidecar.logical.clone());
        let desired = existing
            .clone()
            .unwrap_or_else(McpLogicalSessionConfig::fresh);
        McpLogicalConfigCandidate {
            previous: existing.clone(),
            base_incarnation: existing.as_ref().map(|config| config.incarnation),
            base_revision: existing.as_ref().map(|config| config.revision),
            desired,
        }
    }

    async fn current_attachment_exact(
        &self,
        session_id: &SessionId,
        witness: &meerkat_runtime::RuntimeExecutorAttachmentWitness,
    ) -> Option<Arc<McpRuntimeAttachmentState>> {
        self.sidecars
            .read()
            .await
            .get(session_id)
            .and_then(|sidecar| sidecar.attachment.as_ref())
            .filter(|attachment| {
                attachment.accepts_input_plumbing() && attachment.witness() == witness
            })
            .cloned()
    }

    pub(crate) async fn current_attachment_witness_for_actor_exact(
        &self,
        session_id: &SessionId,
        actor_witness: &meerkat::LiveSessionActorWitness,
    ) -> Option<meerkat_runtime::RuntimeExecutorAttachmentWitness> {
        self.sidecars
            .read()
            .await
            .get(session_id)
            .and_then(|sidecar| sidecar.attachment.as_ref())
            .filter(|attachment| attachment.belongs_to_actor(actor_witness))
            .map(|attachment| attachment.witness().clone())
    }

    /// Configure peer ingress only for the already-materialized exact MCP
    /// incarnation. This method never reconstructs a missing actor/executor.
    #[cfg(feature = "comms")]
    pub(crate) async fn configure_peer_ingress_exact(
        &self,
        session_id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(session_id)
            .await
            .map_err(|error| match error {
                SessionError::NotFound { .. } => {
                    explicit_resume_required(session_id, "live actor is absent")
                }
                other => other,
            })?;
        let registration_lease = self.runtime_registration_lock(session_id);
        let _registration_guard = Arc::clone(&registration_lease.lock).lock_owned().await;
        let attachment = self
            .current_attachment_exact(
                session_id,
                &self
                    .runtime_adapter
                    .current_executor_attachment_witness(session_id)
                    .await
                    .ok_or_else(|| {
                        explicit_resume_required(session_id, "runtime attachment is absent")
                    })?,
            )
            .await
            .filter(|attachment| attachment.belongs_to_actor(actor_lease.witness()))
            .ok_or_else(|| {
                explicit_resume_required(
                    session_id,
                    "live actor has no matching exact MCP attachment",
                )
            })?;
        let comms_runtime = self.service.comms_runtime(session_id).await;
        self.runtime_adapter
            .update_peer_ingress_context_if_current(attachment.witness(), keep_alive, comms_runtime)
            .await
            .map_err(runtime_driver_error_to_session_error)
            .map(|_| ())
    }

    /// Install one inactive exact attachment sidecar while the machine still
    /// retains its pending serving fence. The final active-bit flip happens in
    /// the machine commit hook, so no serving executor can exist without its
    /// mechanical MCP owner.
    async fn stage_attachment_publication(
        &self,
        session_id: &SessionId,
        candidate: &McpLogicalConfigCandidate,
        attachment: Arc<McpRuntimeAttachmentState>,
    ) -> Result<(), SessionError> {
        let mut sidecars = self.sidecars.write().await;
        let can_publish = match (candidate.base_incarnation, sidecars.get(session_id)) {
            (None, None) => true,
            (Some(expected_incarnation), Some(sidecar)) => {
                sidecar.logical.incarnation == expected_incarnation
                    && Some(sidecar.logical.revision) == candidate.base_revision
                    && sidecar.attachment.as_ref().is_none_or(|current| {
                        current.witness() == attachment.witness() || current.is_retired()
                    })
            }
            _ => false,
        };
        if !can_publish {
            return Err(explicit_resume_required(
                session_id,
                "logical config or exact attachment changed before sidecar commit",
            ));
        }

        match sidecars.get_mut(session_id) {
            Some(sidecar) => {
                sidecar.logical = candidate.desired.clone();
                sidecar.attachment = Some(Arc::clone(&attachment));
            }
            None => {
                sidecars.insert(
                    session_id.clone(),
                    McpSessionSidecar {
                        logical: candidate.desired.clone(),
                        attachment: Some(Arc::clone(&attachment)),
                    },
                );
            }
        }
        Ok(())
    }

    /// Undo only the exact actor/attachment whose machine commit succeeded
    /// but whose post-commit MCP ownership validation failed. This is a
    /// call-local compare-and-remove cleanup: it never archives durable session
    /// state and can never retire or detach a same-SessionId replacement.
    async fn cleanup_failed_committed_attachment(
        &self,
        session_id: &SessionId,
        attachment_witness: &meerkat_runtime::RuntimeExecutorAttachmentWitness,
        actor_witness: &meerkat::LiveSessionActorWitness,
    ) -> Result<(), SessionError> {
        let retirement = self
            .runtime_adapter
            .unregister_executor_attachment_if_current(attachment_witness)
            .await;
        self.finish_failed_committed_attachment_cleanup(
            session_id,
            attachment_witness,
            actor_witness,
            retirement,
        )
        .await
    }

    /// Finish MCP-local cleanup only after the machine proves exact A is no
    /// longer serving. A retirement error preserves the actor and sidecar as
    /// one coherent retryable assembly; tearing them down would strand a live
    /// machine executor without its owners.
    async fn finish_failed_committed_attachment_cleanup(
        &self,
        session_id: &SessionId,
        attachment_witness: &meerkat_runtime::RuntimeExecutorAttachmentWitness,
        actor_witness: &meerkat::LiveSessionActorWitness,
        retirement: Result<bool, meerkat_runtime::RuntimeDriverError>,
    ) -> Result<(), SessionError> {
        let retired = retirement.map_err(runtime_driver_error_to_session_error)?;
        let current = self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await;
        if !retired && current.as_ref() == Some(attachment_witness) {
            return Err(SessionError::Agent(AgentError::InternalError(format!(
                "machine refused exact retirement while MCP attachment {attachment_witness:?} remains current"
            ))));
        }
        let session_has_executor = self
            .runtime_adapter
            .session_has_executor(session_id)
            .await
            .map_err(runtime_driver_error_to_session_error)?;

        // Exact A is now absent. Compare-detach only A; replacement B's
        // sidecar is untouched. Canonical machine teardown normally already
        // performed this through the executor's post-stop cleanup handle.
        self.detach_exact(session_id, attachment_witness).await;

        // If another executor exists it may legitimately retain this service
        // actor, so never discard the actor in that case. With no executor,
        // exact actor cleanup is safe and closes a partially committed create.
        if !session_has_executor
            && let Some(actor_lease) = self
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(actor_witness)
                .await?
        {
            self.service
                .discard_live_session_actor(&actor_lease)
                .await?;
        }
        Ok(())
    }

    async fn fail_committed_attachment(
        &self,
        session_id: &SessionId,
        attachment_witness: &meerkat_runtime::RuntimeExecutorAttachmentWitness,
        actor_witness: &meerkat::LiveSessionActorWitness,
        primary: SessionError,
    ) -> SessionError {
        match self
            .cleanup_failed_committed_attachment(session_id, attachment_witness, actor_witness)
            .await
        {
            Ok(()) => primary,
            Err(cleanup_error) => SessionError::Agent(AgentError::InternalError(format!(
                "{primary}; exact cleanup after failed MCP attachment commit validation also failed: {cleanup_error}"
            ))),
        }
    }

    async fn detach_exact(
        &self,
        session_id: &SessionId,
        witness: &meerkat_runtime::RuntimeExecutorAttachmentWitness,
    ) -> bool {
        let attachment = {
            let mut sidecars = self.sidecars.write().await;
            let Some(sidecar) = sidecars.get_mut(session_id) else {
                return false;
            };
            if !sidecar
                .attachment
                .as_ref()
                .is_some_and(|attachment| attachment.witness() == witness)
            {
                return false;
            }
            sidecar.attachment.take()
        };
        if let Some(attachment) = attachment {
            attachment.retire().await;
            true
        } else {
            false
        }
    }

    async fn remove_unattached_logical_exact(
        &self,
        session_id: &SessionId,
        incarnation: uuid::Uuid,
        revision: u64,
    ) -> bool {
        let mut sidecars = self.sidecars.write().await;
        if !sidecars.get(session_id).is_some_and(|sidecar| {
            sidecar.logical.incarnation == incarnation
                && sidecar.logical.revision == revision
                && sidecar.attachment.is_none()
        }) {
            return false;
        }
        sidecars.remove(session_id);
        true
    }

    async fn ensure_attachment_for_live_session(
        &self,
        session_id: &SessionId,
        candidate: McpLogicalConfigCandidate,
        actor_lease: &meerkat::LiveSessionActorTurnBoundaryLease,
        registration_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<McpAttachmentAcquisition, SessionError> {
        if actor_lease.session_id() != session_id {
            return Err(explicit_resume_required(
                session_id,
                "live actor lease belongs to a different session",
            ));
        }
        if candidate.base_revision != Some(candidate.desired.revision) {
            return Err(explicit_resume_required(
                session_id,
                "logical configuration changed outside an explicit resume",
            ));
        }

        let witness = self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .ok_or_else(|| explicit_resume_required(session_id, "runtime attachment is absent"))?;
        let attachment = self
            .current_attachment_exact(session_id, &witness)
            .await
            .ok_or_else(|| {
                explicit_resume_required(
                    session_id,
                    "exact MCP sidecar for the current runtime attachment is absent",
                )
            })?;

        // Machine-owned teardown can run independently of the surface lock.
        // Revalidate after capturing the sidecar so attachment A can never be
        // returned after replacement B became current.
        if self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .as_ref()
            != Some(&witness)
            || !attachment.belongs_to_actor(actor_lease.witness())
            || !attachment.accepts_input_plumbing()
        {
            return Err(explicit_resume_required(
                session_id,
                "live actor, runtime attachment, and MCP sidecar are not one exact incarnation",
            ));
        }

        Ok(McpAttachmentAcquisition::existing(
            attachment,
            registration_guard,
        ))
    }

    async fn runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(session_id)
            .await
            .map_err(|error| match error {
                SessionError::NotFound { .. } => {
                    explicit_resume_required(session_id, "live actor is absent")
                }
                other => other,
            })?;
        self.runtime_session_state_locked_acquisition(session_id, &actor_lease, None)
            .await
            .map(McpAttachmentAcquisition::into_attachment)
    }

    async fn runtime_session_state_locked_acquisition(
        &self,
        session_id: &SessionId,
        actor_lease: &meerkat::LiveSessionActorTurnBoundaryLease,
        registration_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<McpAttachmentAcquisition, SessionError> {
        let candidate = self.snapshot_config_candidate(session_id).await;
        self.ensure_attachment_for_live_session(
            session_id,
            candidate,
            actor_lease,
            registration_guard,
        )
        .await
    }

    pub(crate) async fn ensure_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        self.runtime_session_state(session_id).await
    }

    pub(crate) async fn current_callback_tools(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn AgentToolDispatcher>> {
        self.sidecars
            .read()
            .await
            .get(session_id)
            .and_then(|sidecar| sidecar.logical.callback_tools.clone())
    }

    /// Capture session-scoped logical MCP configuration for an explicit
    /// materialization. The eventual attachment commit compare-checks this
    /// revision; this snapshot itself carries no lifecycle authority.
    pub(crate) async fn logical_config_for_prepared_actor(
        &self,
        session_id: &SessionId,
        prepared: &crate::McpPreparedActorMaterialization,
    ) -> Result<McpLogicalConfigSnapshot, SessionError> {
        if prepared.session_id() != session_id {
            return Err(SessionError::Agent(AgentError::InternalError(format!(
                "MCP logical configuration snapshot belongs to {}, not {session_id}",
                prepared.session_id()
            ))));
        }
        let sidecars = self.sidecars.read().await;
        let sidecar = sidecars.get(session_id);
        Ok(McpLogicalConfigSnapshot {
            sidecar_exists: sidecar.is_some(),
            router: sidecar.and_then(|sidecar| sidecar.logical.router.clone()),
            callback_tools: sidecar.and_then(|sidecar| sidecar.logical.callback_tools.clone()),
            attachment_witness: sidecar
                .and_then(|sidecar| sidecar.attachment.as_ref())
                .filter(|attachment| attachment.accepts_input_plumbing())
                .map(|attachment| attachment.witness().clone()),
        })
    }

    /// Exact logical/attachment snapshot for a non-rebuild resume. The lease
    /// retains the service actor's B boundary so concurrent configure or
    /// replacement cannot interleave a different router with this actor.
    pub(crate) async fn logical_config_for_live_actor(
        &self,
        session_id: &SessionId,
        actor_lease: &meerkat::LiveSessionActorTurnBoundaryLease,
    ) -> Result<McpLogicalConfigSnapshot, SessionError> {
        if actor_lease.session_id() != session_id {
            return Err(SessionError::Agent(AgentError::InternalError(format!(
                "MCP logical configuration snapshot actor belongs to {}, not {session_id}",
                actor_lease.session_id()
            ))));
        }
        let sidecars = self.sidecars.read().await;
        let sidecar = sidecars.get(session_id);
        Ok(McpLogicalConfigSnapshot {
            sidecar_exists: sidecar.is_some(),
            router: sidecar.and_then(|sidecar| sidecar.logical.router.clone()),
            callback_tools: sidecar.and_then(|sidecar| sidecar.logical.callback_tools.clone()),
            attachment_witness: sidecar
                .and_then(|sidecar| sidecar.attachment.as_ref())
                .filter(|attachment| {
                    attachment.accepts_input_plumbing()
                        && attachment.belongs_to_actor(actor_lease.witness())
                })
                .map(|attachment| attachment.witness().clone()),
        })
    }

    pub(crate) async fn logical_router(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<McpRouterAdapter>> {
        self.sidecars
            .read()
            .await
            .get(session_id)
            .and_then(|sidecar| sidecar.logical.router.clone())
    }

    pub(crate) async fn current_attachment_witness(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_runtime::RuntimeExecutorAttachmentWitness> {
        self.sidecars
            .read()
            .await
            .get(session_id)
            .and_then(|sidecar| sidecar.attachment.as_ref())
            .filter(|attachment| attachment.accepts_input_plumbing())
            .map(|attachment| attachment.witness().clone())
    }

    pub(crate) async fn has_sidecar(&self, session_id: &SessionId) -> bool {
        self.sidecars.read().await.contains_key(session_id)
    }

    pub(crate) async fn live_router_lease(
        &self,
        session_id: &SessionId,
    ) -> Result<McpLiveRouterLease, SessionError> {
        // Canonical lock order is B before any MCP mechanical gate. Retaining
        // this exact actor lease keeps router configuration A from being used
        // with a same-SessionId replacement actor B.
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(session_id)
            .await
            .map_err(|error| match error {
                SessionError::NotFound { .. } => {
                    explicit_resume_required(session_id, "live actor is absent")
                }
                other => other,
            })?;
        let lease = self.runtime_registration_lock(session_id);
        let guard = Arc::clone(&lease.lock).lock_owned().await;
        let (router, attachment) = {
            let sidecars = self.sidecars.read().await;
            let sidecar = sidecars.get(session_id).ok_or_else(|| {
                explicit_resume_required(session_id, "logical MCP configuration is absent")
            })?;
            let attachment = sidecar
                .attachment
                .as_ref()
                .filter(|attachment| attachment.accepts_input_plumbing())
                .ok_or_else(|| {
                    explicit_resume_required(session_id, "exact MCP attachment is absent")
                })?;
            let router = sidecar.logical.router.clone().ok_or_else(|| {
                explicit_resume_required(session_id, "logical MCP router is absent")
            })?;
            (router, Arc::clone(attachment))
        };
        let operation_guard = Arc::clone(&attachment.operation_gate).lock_owned().await;
        if !attachment.accepts_input_plumbing() {
            return Err(explicit_resume_required(
                session_id,
                "exact MCP attachment retired during router acquisition",
            ));
        }
        if self.authoritative_session_archived(session_id).await? {
            return Err(SessionError::NotFound {
                id: session_id.clone(),
            });
        }
        let witness = attachment.witness().clone();
        let machine_witness = self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .ok_or_else(|| explicit_resume_required(session_id, "runtime attachment is absent"))?;
        if machine_witness != witness || !attachment.belongs_to_actor(actor_lease.witness()) {
            return Err(explicit_resume_required(
                session_id,
                "actor, sidecar, and runtime attachment are not one exact incarnation",
            ));
        }
        Ok(McpLiveRouterLease {
            _operation_guard: operation_guard,
            _guard: guard,
            _lease: lease,
            router,
            _witness: witness,
            _actor_lease: actor_lease,
        })
    }

    pub(crate) async fn configure_session(
        &self,
        session_id: &SessionId,
        callback_config: McpCallbackConfig,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        self.configure_session_with_optional_router(session_id, callback_config, None)
            .await
    }

    pub(crate) async fn configure_session_with_router(
        &self,
        session_id: &SessionId,
        callback_config: McpCallbackConfig,
        router: Arc<McpRouterAdapter>,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        self.configure_session_with_optional_router(session_id, callback_config, Some(router))
            .await
    }

    async fn configure_session_with_optional_router(
        &self,
        session_id: &SessionId,
        callback_config: McpCallbackConfig,
        router: Option<Arc<McpRouterAdapter>>,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(session_id)
            .await
            .map_err(|error| match error {
                SessionError::NotFound { .. } => {
                    explicit_resume_required(session_id, "live actor is absent")
                }
                other => other,
            })?;
        let lock = self.runtime_registration_lock(session_id);
        let guard = Arc::clone(&lock.lock).lock_owned().await;
        self.configure_session_with_optional_router_locked(
            session_id,
            callback_config,
            router,
            actor_lease,
            Some(guard),
        )
        .await
    }

    pub(crate) async fn commit_prepared_session(
        &self,
        session_id: &SessionId,
        mut prepared: crate::McpPreparedActorMaterialization,
        callback_config: McpCallbackConfig,
        router: Option<Arc<McpRouterAdapter>>,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        if prepared.session_id() != session_id {
            return Err(SessionError::Agent(AgentError::InternalError(format!(
                "prepared MCP materialization belongs to {}, not {session_id}",
                prepared.session_id()
            ))));
        }
        let actor_witness = prepared.actor_witness_slot().witness().ok_or_else(|| {
            explicit_resume_required(
                session_id,
                "prepared actor has not published an exact service witness",
            )
        })?;
        let replaces_predecessor = prepared.replaces_predecessor();
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor_witness)
            .await?
            .ok_or_else(|| {
                explicit_resume_required(
                    session_id,
                    "prepared service actor was replaced before attachment commit",
                )
            })?;
        drop(actor_lease);

        let created_attachment = Arc::new(StdMutex::new(None));
        let created_attachment_for_factory = Arc::clone(&created_attachment);
        let context = self.clone();
        let executor_session_id = session_id.clone();
        let actor_witness_slot = prepared.actor_witness_slot().clone();
        let attachment_actor_witness = actor_witness.clone();
        let outcome = prepared
            .ensure_executor_attachment(move |witness| {
                let attachment = Arc::new(McpRuntimeAttachmentState::new_for_actor(
                    witness,
                    attachment_actor_witness,
                ));
                *created_attachment_for_factory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(Arc::clone(&attachment));
                Box::new(McpSessionRuntimeExecutor::new_exact(
                    context,
                    executor_session_id,
                    attachment,
                    actor_witness_slot,
                )) as Box<dyn CoreExecutor>
            })
            .await
            .map_err(runtime_driver_error_to_session_error)?;
        let pending = match outcome {
            meerkat_runtime::EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            meerkat_runtime::EnsureRuntimeExecutorAttachment::Existing(witness) => {
                return Err(SessionError::Agent(AgentError::InternalError(format!(
                    "prepared MCP materialization unexpectedly found committed attachment {witness:?} for {session_id}"
                ))));
            }
        };
        let created_attachment_state = {
            created_attachment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        let attachment = match created_attachment_state {
            Some(attachment) => attachment,
            None => {
                let primary = SessionError::Agent(AgentError::InternalError(format!(
                    "prepared MCP executor factory did not publish attachment state for {session_id}"
                )));
                return Err(abort_pending_mcp_attachment_after_error(pending, primary).await);
            }
        };
        if attachment.witness() != pending.witness() {
            let primary = SessionError::Agent(AgentError::InternalError(format!(
                "MCP executor factory witness diverged before commit for {session_id}"
            )));
            return Err(abort_pending_mcp_attachment_after_error(pending, primary).await);
        }

        // The aggregate map update is its own compare-and-swap. Do not acquire
        // the surface registration gate while the pending attachment retains
        // machine mutation authority: ordinary input paths hold that gate
        // while entering the machine, and cleanup may acquire the service
        // boundary before returning here. The logical revision plus exact
        // attachment witness make a stale publication fail closed without a
        // cross-owner lock transaction.
        let mut candidate = self.snapshot_config_candidate(session_id).await;
        if let Some(router) = router {
            candidate = candidate.with_router(router);
        }
        if let McpCallbackConfig::Replace(callback_tools) = callback_config {
            candidate = candidate.with_callback_tools(callback_tools);
        }
        let stage_result = self
            .stage_attachment_publication(session_id, &candidate, Arc::clone(&attachment))
            .await;
        if let Err(primary) = stage_result {
            return Err(abort_pending_mcp_attachment_after_error(pending, primary).await);
        }

        // Test-only startup failure is injected after sidecar staging but
        // before serving publication. This is the precise window where a
        // surface registration gate must not be retained while pending-loop
        // abort cleanup waits for the service-owned boundary.
        #[cfg(test)]
        if let Err(error) = self
            .runtime_adapter
            .run_executor_attach_post_ensure_test_hook(session_id)
            .await
        {
            let primary = SessionError::Agent(AgentError::InternalError(format!(
                "MCP executor attachment post-ensure hook failed for {session_id}: {error}"
            )));
            return Err(abort_pending_mcp_attachment_after_error(pending, primary).await);
        }

        let expected_witness = attachment.witness().clone();
        let attachment_for_activation = Arc::clone(&attachment);
        let activate = move |committed: &meerkat_runtime::RuntimeExecutorAttachmentWitness| {
            if committed != &expected_witness {
                return Err(meerkat_runtime::RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "MCP sidecar staged for {expected_witness:?}, machine committed {committed:?}"
                    ),
                });
            }
            attachment_for_activation.activate()
        };
        let committed = if replaces_predecessor {
            pending.commit_with_replacing_predecessor(activate).await
        } else {
            pending.commit_with(activate).await
        }
        .map_err(runtime_driver_error_to_session_error)?;
        if &committed != attachment.witness() {
            let primary = explicit_resume_required(
                session_id,
                "machine committed a different executor attachment witness",
            );
            return Err(self
                .fail_committed_attachment(
                    session_id,
                    attachment.witness(),
                    &actor_witness,
                    primary,
                )
                .await);
        }

        // Do not retain the mechanical registration gate while acquiring the
        // exact service actor boundary. A concurrent explicit resume may win;
        // the compare checks below then fail closed without publishing over it.
        let actor_lease = match self
            .service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor_witness)
            .await
        {
            Ok(Some(actor_lease)) => actor_lease,
            Ok(None) => {
                let primary = explicit_resume_required(
                    session_id,
                    "service actor changed after executor attachment commit",
                );
                return Err(self
                    .fail_committed_attachment(
                        session_id,
                        attachment.witness(),
                        &actor_witness,
                        primary,
                    )
                    .await);
            }
            Err(primary) => {
                return Err(self
                    .fail_committed_attachment(
                        session_id,
                        attachment.witness(),
                        &actor_witness,
                        primary,
                    )
                    .await);
            }
        };
        let registration_lease = self.runtime_registration_lock(session_id);
        let _registration_guard = Arc::clone(&registration_lease.lock).lock_owned().await;
        if self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .as_ref()
            != Some(attachment.witness())
            || self.current_attachment_witness(session_id).await.as_ref()
                != Some(attachment.witness())
            || !attachment.belongs_to_actor(actor_lease.witness())
        {
            let primary = explicit_resume_required(
                session_id,
                "actor or executor attachment changed after exact MCP sidecar activation",
            );
            drop(_registration_guard);
            drop(registration_lease);
            drop(actor_lease);
            return Err(self
                .fail_committed_attachment(
                    session_id,
                    attachment.witness(),
                    &actor_witness,
                    primary,
                )
                .await);
        }

        Ok(attachment)
    }

    async fn configure_session_with_optional_router_locked(
        &self,
        session_id: &SessionId,
        callback_config: McpCallbackConfig,
        router: Option<Arc<McpRouterAdapter>>,
        actor_lease: meerkat::LiveSessionActorTurnBoundaryLease,
        registration_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<Arc<McpRuntimeAttachmentState>, SessionError> {
        let base_candidate = self.snapshot_config_candidate(session_id).await;
        let mut candidate = base_candidate.clone();
        if let Some(router) = router {
            candidate = candidate.with_router(router);
        }
        if let McpCallbackConfig::Replace(callback_tools) = callback_config {
            candidate = candidate.with_callback_tools(callback_tools);
        }
        let acquisition = self
            .ensure_attachment_for_live_session(
                session_id,
                base_candidate,
                &actor_lease,
                registration_guard,
            )
            .await?;
        let attachment = Arc::clone(&acquisition.attachment);
        let operation_guard = Arc::clone(&attachment.operation_gate).lock_owned().await;
        if !attachment.accepts_input_plumbing()
            || self
                .runtime_adapter
                .current_executor_attachment_witness(session_id)
                .await
                .as_ref()
                != Some(attachment.witness())
        {
            return Err(explicit_resume_required(
                session_id,
                "exact attachment changed before logical configuration commit",
            ));
        }

        {
            let mut sidecars = self.sidecars.write().await;
            let sidecar = sidecars.get_mut(session_id).ok_or_else(|| {
                explicit_resume_required(session_id, "logical MCP sidecar is absent")
            })?;
            if sidecar.logical.incarnation
                != candidate.base_incarnation.ok_or_else(|| {
                    explicit_resume_required(
                        session_id,
                        "logical MCP sidecar incarnation is absent",
                    )
                })?
                || Some(sidecar.logical.revision) != candidate.base_revision
                || !sidecar
                    .attachment
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &attachment))
            {
                return Err(explicit_resume_required(
                    session_id,
                    "logical MCP sidecar changed before configuration commit",
                ));
            }
            sidecar.logical = candidate.desired.clone();
        }

        if !attachment.accepts_input_plumbing()
            || self
                .runtime_adapter
                .current_executor_attachment_witness(session_id)
                .await
                .as_ref()
                != Some(attachment.witness())
        {
            let mut sidecars = self.sidecars.write().await;
            if let Some(sidecar) = sidecars.get_mut(session_id)
                && sidecar.logical.incarnation == candidate.desired.incarnation
                && sidecar.logical.revision == candidate.desired.revision
                && sidecar
                    .attachment
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &attachment))
                && let Some(previous) = candidate.previous
            {
                sidecar.logical = previous;
            }
            return Err(explicit_resume_required(
                session_id,
                "exact attachment changed during logical configuration commit",
            ));
        }
        drop(operation_guard);
        Ok(acquisition.into_attachment())
    }

    async fn clear_runtime_session_state_locked(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        let (logical_identity, attachment) = self
            .sidecars
            .read()
            .await
            .get(session_id)
            .map(|sidecar| {
                (
                    Some((sidecar.logical.incarnation, sidecar.logical.revision)),
                    sidecar.attachment.clone(),
                )
            })
            .unwrap_or((None, None));
        let Some(attachment) = attachment else {
            if self.runtime_adapter.contains_session(session_id).await {
                return Err(explicit_resume_required(
                    session_id,
                    "runtime registration exists without an exact MCP sidecar cleanup witness",
                ));
            }
            if let Some((incarnation, revision)) = logical_identity {
                self.remove_unattached_logical_exact(session_id, incarnation, revision)
                    .await;
            }
            return Ok(());
        };

        self.runtime_adapter
            .unregister_executor_attachment_if_current(attachment.witness())
            .await
            .map_err(runtime_driver_error_to_session_error)?;
        self.detach_exact(session_id, attachment.witness()).await;
        if self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .as_ref()
            == Some(attachment.witness())
        {
            return Err(SessionError::Unsupported(format!(
                "exact MCP attachment cleanup did not retire witness {:?} for session {session_id}",
                attachment.witness()
            )));
        }
        if let Some((incarnation, revision)) = logical_identity {
            self.remove_unattached_logical_exact(session_id, incarnation, revision)
                .await;
        }
        Ok(())
    }

    /// Clear only MCP/session projections after the machine-owned unregister
    /// saga has authorized external cleanup. Runtime registration remains the
    /// saga's responsibility; calling `unregister_session` here would recurse
    /// into and deadlock on that same coordinator.
    async fn clear_external_runtime_attachment_state_under_retirement(
        &self,
        session_id: &SessionId,
        attachment: &Arc<McpRuntimeAttachmentState>,
        _retirement_guard: tokio::sync::OwnedMutexGuard<()>,
    ) {
        {
            let mut sidecars = self.sidecars.write().await;
            if let Some(sidecar) = sidecars.get_mut(session_id)
                && sidecar
                    .attachment
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, attachment))
            {
                sidecar.attachment.take();
            }
        }
        attachment.finish_retirement().await;
    }

    async fn clear_runtime_session_state_after_verified_termination_locked(
        &self,
        session_id: &SessionId,
        attachment: &McpRuntimeAttachmentState,
        cleanup_observation: &meerkat_runtime::CompletionCleanupObservation,
        remove_logical_sidecar: bool,
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
        let logical_identity = if remove_logical_sidecar {
            self.sidecars
                .read()
                .await
                .get(session_id)
                .map(|sidecar| (sidecar.logical.incarnation, sidecar.logical.revision))
        } else {
            None
        };
        self.detach_exact(session_id, attachment.witness()).await;
        if let Some((incarnation, revision)) = logical_identity {
            self.remove_unattached_logical_exact(session_id, incarnation, revision)
                .await;
        }
        Ok(())
    }

    pub(crate) async fn clear_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        // Exact machine retirement owns its own mutation fence;
        // compare-and-remove by attachment witness protects a concurrent
        // replacement without transferring cleanup to a detached task.
        self.clear_runtime_session_state_locked(session_id).await
    }

    async fn clear_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
        attachment: &Arc<McpRuntimeAttachmentState>,
        retirement_guard: tokio::sync::OwnedMutexGuard<()>,
    ) {
        // The runtime-owned executor decorator closes the exact adapter
        // attachment after this service/surface cleanup returns. Calling the
        // ordinary unregister path here would await this loop's own handle.
        self.clear_external_runtime_attachment_state_under_retirement(
            session_id,
            attachment,
            retirement_guard,
        )
        .await;
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

    /// Resolve completion cleanup from one exact attachment observation.
    async fn cleanup_runtime_after_completion_outcome_locked(
        &self,
        session_id: &SessionId,
        attachment: &McpRuntimeAttachmentState,
        cleanup_observation: meerkat_runtime::completion::CompletionCleanupObservation,
    ) -> Result<McpRuntimeCompletionCleanup, meerkat_runtime::completion::CompletionWaitError> {
        if let Some(_current) = self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .filter(|current| current != attachment.witness())
        {
            // The completion belongs to stale attachment A. It may release
            // A-local request resources, but it must never ask the
            // session-scoped DSL to classify or clean replacement B, nor may
            // its caller observe B's result as a successful completion of A.
            return Ok(McpRuntimeCompletionCleanup::stale());
        }
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
        let archived_logical_identity = if archived_now {
            self.sidecars
                .read()
                .await
                .get(session_id)
                .and_then(|sidecar| {
                    sidecar
                        .attachment
                        .as_ref()
                        .is_some_and(|current| current.witness() == attachment.witness())
                        .then_some((sidecar.logical.incarnation, sidecar.logical.revision))
                })
        } else {
            None
        };
        let live_session = if archived_now {
            meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::NotObserved
        } else {
            let actor_witness = attachment.actor_witness.get().ok_or_else(|| {
                meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(format!(
                    "MCP runtime completion attachment for {session_id} lacks an exact actor witness"
                ))
            })?;
            match self
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(actor_witness)
                .await
            {
                Ok(Some(_actor_lease)) => {
                    meerkat_runtime::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::Present
                }
                Ok(None) => {
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
                    self.detach_exact(session_id, attachment.witness()).await;
                    if let Some((incarnation, revision)) = archived_logical_identity {
                        self.remove_unattached_logical_exact(session_id, incarnation, revision)
                            .await;
                    }
                    return Ok(McpRuntimeCompletionCleanup::Current {
                        release_pre_admission: true,
                    });
                }
                if runtime_termination_cleanup
                    && !self.runtime_adapter.contains_session(session_id).await
                {
                    self.clear_runtime_session_state_after_verified_termination_locked(
                        session_id,
                        attachment,
                        &runtime_termination_proof,
                        archived_now,
                    )
                    .await
                    .map_err(|cleanup_error| {
                        meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                            format!(
                                "MCP runtime completion cleanup failed for {session_id}: {cleanup_error}"
                            ),
                        )
                    })?;
                    return Ok(McpRuntimeCompletionCleanup::Current {
                        release_pre_admission: true,
                    });
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
            if runtime_termination_cleanup
                && !self.runtime_adapter.contains_session(session_id).await
            {
                self.clear_runtime_session_state_after_verified_termination_locked(
                    session_id,
                    attachment,
                    &runtime_termination_proof,
                    archived_now,
                )
                .await
                .map_err(|error| {
                    meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                        format!(
                            "MCP runtime completion exact termination cleanup failed for {session_id}: {error}"
                        ),
                    )
                })?;
            } else {
                let removed = self
                    .runtime_adapter
                    .unregister_executor_attachment_if_current(attachment.witness())
                    .await
                    .map_err(runtime_driver_error_to_session_error)
                    .map_err(|error| {
                        meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                            format!(
                                "MCP runtime completion exact actor-and-attachment retirement failed for {session_id}: {error}"
                            ),
                        )
                    })?;
                if !removed {
                    // A delayed completion for attachment A has no authority
                    // over a same-SessionId replacement B's actor, machine
                    // registration, sidecar, or returned completion result.
                    return Ok(McpRuntimeCompletionCleanup::stale());
                }
                if archived_now {
                    self.detach_exact(session_id, attachment.witness()).await;
                    if let Some((incarnation, revision)) = archived_logical_identity {
                        self.remove_unattached_logical_exact(session_id, incarnation, revision)
                            .await;
                    }
                } else {
                    self.detach_exact(session_id, attachment.witness()).await;
                }
            }
            if archived_now {
                self.detach_exact(session_id, attachment.witness()).await;
                if let Some((incarnation, revision)) = archived_logical_identity {
                    self.remove_unattached_logical_exact(session_id, incarnation, revision)
                        .await;
                }
            }
        }
        Ok(McpRuntimeCompletionCleanup::Current {
            release_pre_admission,
        })
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
        attachment: &McpRuntimeAttachmentState,
        session_id: SessionId,
        input_id: meerkat_core::lifecycle::InputId,
        admission: meerkat::RuntimeContextAdmissionGuard,
    ) -> Result<(), SessionError> {
        attachment.insert_pre_admission(session_id, input_id, admission)
    }

    async fn take_runtime_pre_admission(
        &self,
        attachment: &McpRuntimeAttachmentState,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Option<meerkat::RuntimeContextAdmissionGuard> {
        attachment.take_pre_admission(input_ids)
    }

    async fn discard_runtime_pre_admission(
        &self,
        attachment: &McpRuntimeAttachmentState,
        input_id: &meerkat_core::lifecycle::InputId,
    ) {
        attachment.discard_pre_admission(input_id);
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

    pub(crate) async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<RequestEventTx>,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), McpRuntimeIngressError> {
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease(session_id)
            .await
            .map_err(|error| match error {
                SessionError::NotFound { .. } => McpRuntimeIngressError::Session(
                    explicit_resume_required(session_id, "live actor is absent"),
                ),
                other => McpRuntimeIngressError::Session(other),
            })?;
        if self
            .authoritative_session_archived(session_id)
            .await
            .map_err(McpRuntimeIngressError::Session)?
        {
            return Err(McpRuntimeIngressError::Runtime(
                meerkat_runtime::RuntimeDriverError::NotReady {
                    state: meerkat_runtime::RuntimeState::Retired,
                },
            ));
        }

        // B is the authority boundary; only after capturing the exact actor do
        // we serialize MCP attachment plumbing. Retain this guard through
        // pre-admission insertion and machine accept so clear/config cannot
        // tear the aggregate between those steps.
        let registration_lease = self.runtime_registration_lock(session_id);
        let registration_guard = Arc::clone(&registration_lease.lock).lock_owned().await;

        let requested_input_id = input.id().clone();
        let acquisition = match self
            .runtime_session_state_locked_acquisition(
                session_id,
                &actor_lease,
                Some(registration_guard),
            )
            .await
        {
            Ok(acquisition) => acquisition,
            Err(error) => return Err(McpRuntimeIngressError::Session(error)),
        };
        let attachment = Arc::clone(&acquisition.attachment);
        let attachment_operation_guard = Arc::clone(&attachment.operation_gate).lock_owned().await;
        let machine_witness = self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await;
        if !attachment.accepts_input_plumbing()
            || machine_witness.as_ref() != Some(attachment.witness())
        {
            drop(attachment_operation_guard);
            return Err(McpRuntimeIngressError::Runtime(
                meerkat_runtime::RuntimeDriverError::NotReady {
                    state: meerkat_runtime::RuntimeState::Retired,
                },
            ));
        }
        let pre_admission = match self
            .service
            .reserve_runtime_turn_admission_for_actor(&actor_lease)
            .await
        {
            Ok(admission) => Some(admission),
            Err(error) => return Err(McpRuntimeIngressError::Session(error)),
        };
        let mut pre_admission_registration = None;
        if let Some(admission) = pre_admission {
            if let Err(error) = self
                .insert_runtime_pre_admission(
                    &attachment,
                    session_id.clone(),
                    requested_input_id.clone(),
                    admission,
                )
                .await
            {
                drop(attachment_operation_guard);
                return Err(McpRuntimeIngressError::Session(error));
            }
            pre_admission_registration = Some(RuntimePreAdmissionRegistration::new(
                Arc::new(McpPreAdmissionLedger {
                    attachment: Arc::clone(&attachment),
                }),
                session_id.clone(),
                requested_input_id.clone(),
            ));
        }

        let mut queued_context_registration =
            attachment.register_turn_context(requested_input_id.clone(), event_tx);
        let queued_context = queued_context_registration.is_some();

        let (outcome, mut handle) = match self
            .runtime_adapter
            .accept_input_with_completion_for_attachment(attachment.witness(), input)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                drop(queued_context_registration.take());
                self.discard_runtime_pre_admission(&attachment, &requested_input_id)
                    .await;
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
                drop(attachment_operation_guard);
                return Err(McpRuntimeIngressError::Runtime(error));
            }
        };

        match (&outcome, handle.take()) {
            (
                AcceptOutcome::Accepted { input_id, .. }
                | AcceptOutcome::Deduplicated {
                    existing_id: input_id,
                    ..
                },
                Some(raw_handle),
            ) => {
                let cleanup_handle = wrap_mcp_runtime_pre_admission_cleanup(
                    self.clone(),
                    session_id.clone(),
                    Arc::clone(&attachment),
                    requested_input_id.clone(),
                    input_id.clone(),
                    raw_handle,
                );
                attachment.rekey_pre_admission(&requested_input_id, input_id.clone());
                handle = Some(cleanup_handle);
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
            }
            (AcceptOutcome::Accepted { input_id, .. }, None) => {
                self.discard_runtime_pre_admission(&attachment, &requested_input_id)
                    .await;
                self.discard_runtime_pre_admission(&attachment, input_id)
                    .await;
                if let Some(registration) = pre_admission_registration.take() {
                    registration.disarm();
                }
            }
            (_, returned_handle) => {
                debug_assert!(returned_handle.is_none());
                self.discard_runtime_pre_admission(&attachment, &requested_input_id)
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
                let _ = queued_context_registration
                    .as_mut()
                    .is_some_and(|registration| registration.rekey(input_id.clone()));
            }
        }

        if handle.is_some()
            && let Some(registration) = queued_context_registration.take()
        {
            registration.disarm();
        }
        drop(queued_context_registration);

        drop(attachment_operation_guard);
        drop(acquisition);
        drop(registration_lease);

        Ok((outcome, handle))
    }
}

pub(crate) fn resume_required_error(session_id: &SessionId, reason: &str) -> SessionError {
    SessionError::FailedWithData {
        message: format!(
            "MCP ingress for session {session_id} is unavailable because {reason}; call meerkat_resume explicitly"
        ),
        data: serde_json::json!({
            "kind": "session_resume_required",
            "session_resume_required": true,
            "session_id": session_id.to_string(),
            "reason": reason,
        }),
    }
}

pub(crate) fn resume_required_tool_error(
    session_id: &SessionId,
    reason: &str,
) -> crate::ToolCallError {
    crate::ToolCallError::new(
        meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code(),
        format!(
            "MCP ingress for session {session_id} is unavailable because {reason}; call meerkat_resume explicitly"
        ),
        Some(serde_json::json!({
            "kind": "session_resume_required",
            "session_resume_required": true,
            "session_id": session_id.to_string(),
            "reason": reason,
        })),
    )
}

pub(crate) fn session_error_to_tool_error(
    error: SessionError,
    context: impl std::fmt::Display,
) -> crate::ToolCallError {
    match error {
        SessionError::FailedWithData { message, data }
            if data
                .get("session_resume_required")
                .and_then(|value| value.as_bool())
                == Some(true) =>
        {
            crate::ToolCallError::new(
                meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code(),
                message,
                Some(data),
            )
        }
        SessionError::NotFound { id } => crate::ToolCallError::new(
            meerkat_contracts::ErrorCode::SessionNotFound.jsonrpc_code(),
            format!("session not found: {id}"),
            Some(serde_json::json!({ "session_id": id.to_string() })),
        ),
        other => crate::ToolCallError::internal(format!("{context}: {other}")),
    }
}

fn explicit_resume_required(session_id: &SessionId, missing: &str) -> SessionError {
    resume_required_error(session_id, missing)
}

fn runtime_driver_error_to_session_error(
    error: meerkat_runtime::RuntimeDriverError,
) -> SessionError {
    SessionError::Agent(AgentError::InternalError(error.to_string()))
}

async fn abort_pending_mcp_attachment_after_error(
    pending: meerkat_runtime::PendingRuntimeExecutorAttachment,
    primary: SessionError,
) -> SessionError {
    match pending.abort().await {
        Ok(()) => primary,
        Err(cleanup_error) => SessionError::Agent(AgentError::InternalError(format!(
            "{primary}; exact pending MCP attachment abort also failed: {cleanup_error}"
        ))),
    }
}

#[derive(Default)]
struct RuntimeTurnQueue {
    entries: HashMap<meerkat_core::lifecycle::InputId, QueuedTurnContext>,
}

struct QueuedTurnContext {
    event_tx: RequestEventTx,
}

pub(crate) struct McpRuntimeAttachmentState {
    witness: meerkat_runtime::RuntimeExecutorAttachmentWitness,
    actor_witness: OnceLock<meerkat::LiveSessionActorWitness>,
    active: AtomicBool,
    retired: AtomicBool,
    operation_gate: Arc<Mutex<()>>,
    queued_turns: StdMutex<RuntimeTurnQueue>,
    pre_admissions: StdMutex<Vec<RuntimePreAdmissionEntry>>,
}

impl std::fmt::Debug for McpRuntimeAttachmentState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpRuntimeAttachmentState")
            .field("witness", &self.witness)
            .field("active", &self.active.load(Ordering::Acquire))
            .field("retired", &self.retired.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl McpRuntimeAttachmentState {
    fn new_for_actor(
        witness: meerkat_runtime::RuntimeExecutorAttachmentWitness,
        actor_witness: meerkat::LiveSessionActorWitness,
    ) -> Self {
        let actor_witness_slot = OnceLock::new();
        if actor_witness_slot.set(actor_witness).is_err() {
            unreachable!("fresh MCP attachment actor-witness slot must be empty");
        }
        Self {
            witness,
            actor_witness: actor_witness_slot,
            active: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            operation_gate: Arc::new(Mutex::new(())),
            queued_turns: StdMutex::new(RuntimeTurnQueue::default()),
            pre_admissions: StdMutex::new(Vec::new()),
        }
    }

    fn witness(&self) -> &meerkat_runtime::RuntimeExecutorAttachmentWitness {
        &self.witness
    }

    fn belongs_to_actor(&self, actor_witness: &meerkat::LiveSessionActorWitness) -> bool {
        self.actor_witness.get() == Some(actor_witness)
    }

    /// Activate this exact sidecar at the machine's serving linearization
    /// point. A later runtime-loop serving-release failure leaves the sidecar
    /// briefly marked active while the machine's owned exact-abort path tears
    /// it down. That state is fail-closed: every MCP acquisition also requires
    /// the same witness to be current in the machine, so it cannot serve or
    /// publish over a replacement attachment.
    fn activate(&self) -> Result<(), meerkat_runtime::RuntimeDriverError> {
        if self.is_retired() {
            return Err(meerkat_runtime::RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "cannot activate retired MCP attachment for {}",
                    self.witness.session_id()
                ),
            });
        }
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| meerkat_runtime::RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "MCP attachment for {} was already active before exact publication",
                    self.witness.session_id()
                ),
            })?;
        if self.is_retired() {
            self.active.store(false, Ordering::Release);
            return Err(meerkat_runtime::RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "MCP attachment for {} retired during exact publication",
                    self.witness.session_id()
                ),
            });
        }
        Ok(())
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    fn accepts_input_plumbing(&self) -> bool {
        self.active.load(Ordering::Acquire) && !self.is_retired()
    }

    fn insert_pre_admission(
        &self,
        session_id: SessionId,
        input_id: meerkat_core::lifecycle::InputId,
        admission: meerkat::RuntimeContextAdmissionGuard,
    ) -> Result<(), SessionError> {
        let mut entries = self
            .pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.accepts_input_plumbing() {
            return Err(SessionError::NotRunning { id: session_id });
        }
        if entries.iter().any(|entry| entry.input_id == input_id) {
            return Err(SessionError::Busy { id: session_id });
        }
        entries.push(RuntimePreAdmissionEntry {
            input_id,
            admission: admission.into(),
        });
        Ok(())
    }

    fn take_pre_admission(
        &self,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Option<meerkat::RuntimeContextAdmissionGuard> {
        if input_ids.is_empty() {
            return None;
        }
        let mut entries = self
            .pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = entries
            .iter()
            .position(|entry| input_ids.contains(&entry.input_id))?;
        Some(entries.remove(index).admission.into_admission())
    }

    fn discard_pre_admission(&self, input_id: &meerkat_core::lifecycle::InputId) {
        let mut entries = self
            .pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = entries.iter().position(|entry| &entry.input_id == input_id) {
            entries.remove(index);
        }
    }

    fn rekey_pre_admission(
        &self,
        from_input_id: &meerkat_core::lifecycle::InputId,
        to_input_id: meerkat_core::lifecycle::InputId,
    ) {
        if from_input_id == &to_input_id {
            return;
        }
        let mut entries = self
            .pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| &entry.input_id == from_input_id)
        {
            entry.input_id = to_input_id;
        }
    }

    fn has_pre_admissions(&self) -> bool {
        !self
            .pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    fn register_turn_context(
        self: &Arc<Self>,
        input_id: meerkat_core::lifecycle::InputId,
        event_tx: Option<RequestEventTx>,
    ) -> Option<McpQueuedTurnRegistration> {
        let Some(event_tx) = event_tx else {
            return None;
        };
        let mut queued_turns = self
            .queued_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.accepts_input_plumbing() {
            return None;
        }
        queued_turns
            .entries
            .insert(input_id.clone(), QueuedTurnContext { event_tx });
        Some(McpQueuedTurnRegistration {
            attachment: Arc::clone(self),
            input_id,
            armed: true,
        })
    }

    fn take_turn_context_for_inputs(
        &self,
        contributing_input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Option<QueuedTurnContext> {
        let mut queued_turns = self
            .queued_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut selected = None;
        for input_id in contributing_input_ids {
            if let Some(context) = queued_turns.entries.remove(input_id) {
                selected = Some(context);
            }
        }
        selected
    }

    fn rekey_turn_context(
        &self,
        from_input_id: &meerkat_core::lifecycle::InputId,
        to_input_id: meerkat_core::lifecycle::InputId,
    ) -> bool {
        if from_input_id == &to_input_id {
            return true;
        }
        let mut queued_turns = self
            .queued_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    fn discard_turn_context(&self, input_id: &meerkat_core::lifecycle::InputId) -> bool {
        self.queued_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .remove(input_id)
            .is_some()
    }

    async fn retire(&self) {
        let _operation_guard = self.begin_retirement().await;
        self.finish_retirement().await;
    }

    async fn begin_retirement(&self) -> tokio::sync::OwnedMutexGuard<()> {
        let operation_guard = Arc::clone(&self.operation_gate).lock_owned().await;
        self.retired.store(true, Ordering::Release);
        operation_guard
    }

    async fn finish_retirement(&self) {
        self.queued_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .clear();
        self.pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

struct McpSessionRuntimeExecutor {
    context: McpRuntimeIngressContext,
    session_id: SessionId,
    attachment: Arc<McpRuntimeAttachmentState>,
    actor_witness_slot: Option<meerkat::LiveSessionActorWitnessSlot>,
}

struct McpSessionRuntimePostStopCleanupHandle {
    context: McpRuntimeIngressContext,
    session_id: SessionId,
    attachment: Arc<McpRuntimeAttachmentState>,
    actor_witness_slot: Option<meerkat::LiveSessionActorWitnessSlot>,
}

#[async_trait]
impl CoreExecutorPostStopCleanupHandle for McpSessionRuntimePostStopCleanupHandle {
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        // Canonical order is service turn-finalization boundary B before the
        // attachment-local operation gate. Live router/config paths use the
        // same B -> operation order, so post-stop cleanup cannot deadlock them.
        let _turn_finalization_guard = self
            .context
            .service
            .acquire_runtime_turn_finalization_guard(&self.session_id)
            .await;
        let retirement_guard = self.attachment.begin_retirement().await;
        let cleanup = match self.actor_witness_slot.as_ref() {
            Some(actor_witness_slot) => {
                meerkat::surface::persistent_runtime_post_stop_cleanup_handle_for_actor_slot(
                    Arc::clone(&self.context.service),
                    self.session_id.clone(),
                    actor_witness_slot.clone(),
                )
            }
            None => meerkat::surface::persistent_runtime_post_stop_cleanup_handle(
                Arc::clone(&self.context.service),
                self.session_id.clone(),
            ),
        };
        cleanup
            .cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary()
            .await?;
        self.context
            .clear_session_after_runtime_stop_terminalized(
                &self.session_id,
                &self.attachment,
                retirement_guard,
            )
            .await;
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        let retirement_guard = self.attachment.begin_retirement().await;
        let cleanup = match self.actor_witness_slot.as_ref() {
            Some(actor_witness_slot) => {
                meerkat::surface::persistent_runtime_post_stop_cleanup_handle_for_actor_slot(
                    Arc::clone(&self.context.service),
                    self.session_id.clone(),
                    actor_witness_slot.clone(),
                )
            }
            None => meerkat::surface::persistent_runtime_post_stop_cleanup_handle(
                Arc::clone(&self.context.service),
                self.session_id.clone(),
            ),
        };
        cleanup
            .cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary()
            .await?;
        self.context
            .clear_session_after_runtime_stop_terminalized(
                &self.session_id,
                &self.attachment,
                retirement_guard,
            )
            .await;
        Ok(())
    }
}

struct McpSessionRuntimeBoundaryHandle {
    context: McpRuntimeIngressContext,
    session_id: SessionId,
}

#[async_trait]
impl CoreExecutorBoundaryHandle for McpSessionRuntimeBoundaryHandle {
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .cancel_after_boundary_with_machine_authority(
                &self.session_id,
                expected_run_id,
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
    fn new_exact(
        context: McpRuntimeIngressContext,
        session_id: SessionId,
        attachment: Arc<McpRuntimeAttachmentState>,
        actor_witness_slot: meerkat::LiveSessionActorWitnessSlot,
    ) -> Self {
        Self {
            context,
            session_id,
            attachment,
            actor_witness_slot: Some(actor_witness_slot),
        }
    }

    #[cfg(test)]
    fn new(
        context: McpRuntimeIngressContext,
        session_id: SessionId,
        attachment: Arc<McpRuntimeAttachmentState>,
    ) -> Self {
        Self {
            context,
            session_id,
            attachment,
            actor_witness_slot: None,
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

/// Apply callback invoked by `McpSessionRuntimeExecutor::apply` after the
/// runtime loop has acquired the non-reentrant service turn-finalization
/// boundary, including recovery and NoPending cleanup.
async fn apply_runtime_turn_under_runtime_turn_boundary(
    context: &McpRuntimeIngressContext,
    attachment: &Arc<McpRuntimeAttachmentState>,
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
        let pre_admission = context
            .take_runtime_pre_admission(attachment, &contributing_input_ids)
            .await;
        return if let Some(admission) = pre_admission {
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
                    drop(admission);
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
    let queued_context = attachment.take_turn_context_for_inputs(&contributing_input_ids);
    let event_tx = queued_context
        .as_ref()
        .map(|context| context.event_tx.clone());

    let mut turn_request = StartTurnRequest {
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
        &mut turn_request,
    )
    .await
    .map_err(|error| {
        SessionError::Agent(meerkat_core::AgentError::InternalError(error.to_string()))
    })?;

    let pre_admission = context
        .take_runtime_pre_admission(attachment, &contributing_input_ids)
        .await;
    if let Some(admission) = pre_admission {
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
                drop(admission);
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
    }
}

#[cfg(test)]
async fn apply_runtime_turn(
    context: &McpRuntimeIngressContext,
    attachment: &Arc<McpRuntimeAttachmentState>,
    session_id: &SessionId,
    run_id: meerkat_core::lifecycle::RunId,
    primitive: &RunPrimitive,
) -> Result<CoreApplyOutput, SessionError> {
    let _turn_finalization_guard = context
        .service
        .acquire_runtime_turn_finalization_guard(session_id)
        .await;
    apply_runtime_turn_under_runtime_turn_boundary(
        context, attachment, session_id, run_id, primitive,
    )
    .await
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

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        Some(meerkat::surface::persistent_runtime_publication_handle(
            Arc::clone(&self.context.service),
            self.session_id.clone(),
        ))
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(Arc::new(McpSessionRuntimePostStopCleanupHandle {
            context: self.context.clone(),
            session_id: self.session_id.clone(),
            attachment: Arc::clone(&self.attachment),
            actor_witness_slot: self.actor_witness_slot.clone(),
        }))
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(
            meerkat::surface::persistent_runtime_turn_finalization_boundary_handle(
                Arc::clone(&self.context.service),
                self.session_id.clone(),
            ),
        )
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        match Box::pin(apply_runtime_turn_under_runtime_turn_boundary(
            &self.context,
            &self.attachment,
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

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: &[u8],
    ) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .checkpoint_committed_runtime_session_snapshot_under_runtime_turn_boundary(
                &self.session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.context
            .service
            .publish_interaction_terminals_exact_batch(&self.session_id, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.context
            .service
            .cancel_current_after_boundary_with_machine_authority(
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
        McpSessionRuntimePostStopCleanupHandle {
            context: self.context.clone(),
            session_id: self.session_id.clone(),
            attachment: Arc::clone(&self.attachment),
            actor_witness_slot: self.actor_witness_slot.clone(),
        }
        .cleanup_after_runtime_stop_terminalized()
        .await
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
            sidecars: Arc::new(RwLock::new(HashMap::new())),
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

    async fn materialize_test_session(
        context: &McpRuntimeIngressContext,
        session: Session,
        mut request: CreateSessionRequest,
        router: Option<Arc<McpRouterAdapter>>,
        callback_tools: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Result<(RunResult, Arc<McpRuntimeAttachmentState>), SessionError> {
        let session_id = session.id().clone();
        let prepared = crate::prepare_new_mcp_actor_materialization(
            &context.service,
            &context.runtime_adapter,
            &session_id,
        )
        .await
        .map_err(|error| SessionError::Agent(AgentError::InternalError(error.message)))?;
        let build = request.build.get_or_insert_with(Default::default);
        build.resume_session = Some(session);
        build.runtime_build_mode =
            meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
        let outcome = crate::create_runtime_backed_session_and_run_initial_turn_call_local(
            &context.service,
            &context.runtime_adapter,
            &session_id,
            request,
            None,
            prepared,
        )
        .await
        .map_err(|error| SessionError::Agent(AgentError::InternalError(error.message)))?;
        let crate::McpCallLocalActorCreateOutcome {
            result,
            transaction,
        } = outcome;
        let result = match result {
            Ok(result) => result,
            Err(
                crate::McpRuntimeBackedCreateError::Session { error, .. }
                | crate::McpRuntimeBackedCreateError::CreatedSessionIdentityMismatch(error)
                | crate::McpRuntimeBackedCreateError::InitialTurnIdentityMismatch(error),
            ) => {
                return Err(error);
            }
        };
        let attachment = context
            .commit_prepared_session(
                &session_id,
                transaction,
                McpCallbackConfig::Replace(callback_tools),
                router,
            )
            .await?;
        Ok((result, attachment))
    }

    async fn create_session_with_test_machine(
        context: &McpRuntimeIngressContext,
        prompt: &str,
        initial_turn: meerkat_core::service::InitialTurnPolicy,
    ) -> Result<RunResult, SessionError> {
        let session = Session::new();
        materialize_test_session(
            context,
            session,
            create_request(prompt, initial_turn),
            None,
            None,
        )
        .await
        .map(|(result, _)| result)
    }

    async fn create_session_with_test_mcp_config(
        context: &McpRuntimeIngressContext,
        prompt: &str,
        initial_turn: meerkat_core::service::InitialTurnPolicy,
        router: Arc<McpRouterAdapter>,
        callback_tools: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Result<(RunResult, Arc<McpRuntimeAttachmentState>), SessionError> {
        let session = Session::new();

        let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
        let external_tools: Arc<dyn AgentToolDispatcher> = match callback_tools.as_ref() {
            Some(callback_tools) => Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                Arc::clone(callback_tools),
                router_tools,
            ])),
            None => router_tools,
        };
        let mut request = create_request(prompt, initial_turn);
        request
            .build
            .get_or_insert_with(Default::default)
            .external_tools = Some(external_tools);

        materialize_test_session(context, session, request, Some(router), callback_tools).await
    }

    async fn create_deferred_session_with_generated_authority(
        context: &McpRuntimeIngressContext,
        prompt: &str,
    ) -> SessionId {
        create_session_with_test_machine(
            context,
            prompt,
            meerkat_core::service::InitialTurnPolicy::Defer,
        )
        .await
        .expect("deferred session materialization should seed generated authority")
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
        let session_id =
            create_deferred_session_with_generated_authority(&context, "invalid terminal").await;
        let state = context
            .ensure_session(&session_id)
            .await
            .expect("test attachment should exist");
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
    async fn persisted_only_ensure_requires_explicit_resume_without_minting_attachment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let created = context
            .service
            .create_session(create_request(
                "persisted only",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create persisted session");
        let session_id = created.session_id;
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live actor");
        let witness_before = context
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await;

        let error = context
            .ensure_session(&session_id)
            .await
            .expect_err("generic ensure must not rematerialize a persisted-only session");

        assert!(
            error.to_string().contains("meerkat_resume"),
            "unexpected fail-closed error: {error}"
        );
        assert_eq!(
            context
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await,
            witness_before,
            "generic ensure must not mint or replace a runtime attachment"
        );
        assert!(!context.has_sidecar(&session_id).await);
    }

    #[tokio::test]
    async fn mcp_apply_does_not_rematerialize_session_lost_after_locked_preparation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let session_id =
            create_deferred_session_with_generated_authority(&context, "lost after preparation")
                .await;
        let state = context
            .ensure_session(&session_id)
            .await
            .expect("locked preparation attaches executor");
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("simulate live session loss after preparation");

        let primitive = RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::RunCheckpoint,
            appends: Vec::new(),
            context_appends: vec![ConversationContextAppend {
                key: "lost-after-preparation".to_string(),
                content: CoreRenderable::Text {
                    text: "must not rematerialize from apply".to_string(),
                },
            }],
            contributing_input_ids: vec![InputId::new()],
            turn_metadata: Some(RuntimeTurnMetadata {
                execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            }),
        });
        let mut executor =
            McpSessionRuntimeExecutor::new(context.clone(), session_id.clone(), Arc::clone(&state));
        let error = CoreExecutor::apply(&mut executor, RunId::new(), primitive)
            .await
            .expect_err("post-preparation loss must surface to the runtime-loop handoff");
        assert!(error.requires_runtime_teardown());
        assert!(
            !context
                .service
                .has_live_session(&session_id)
                .await
                .expect("check live absence"),
            "apply must not recreate the missing live session"
        );
        assert!(context.runtime_adapter.contains_session(&session_id).await);
        context
            .runtime_adapter
            .unregister_executor_attachment_if_current(state.witness())
            .await
            .expect("external teardown owner unregisters after apply returns");
    }

    #[tokio::test]
    async fn mcp_runtime_accept_after_out_of_band_archive_requires_resume_without_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let session_id = create_deferred_session_with_generated_authority(&context, "hello").await;
        context
            .ensure_session(&session_id)
            .await
            .expect("MCP runtime executor should attach");
        assert!(context.runtime_adapter.contains_session(&session_id).await);

        // Deliberately bypass the public MCP archive wrapper. Generic input is
        // not the owner of the corresponding surface cleanup.
        archive_with_test_machine_authority(&context, &session_id)
            .await
            .expect("archive should succeed");
        let attachment_before_accept = context.current_attachment_witness(&session_id).await;
        let sidecar_before_accept = context.has_sidecar(&session_id).await;
        let error = context
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(meerkat_runtime::PromptInput::new("after archive", None)),
                None,
            )
            .await
            .expect_err("actor-missing generic input must require explicit resume");
        assert!(
            error.to_string().contains("meerkat_resume"),
            "out-of-band archive must use the typed resume-required contract: {error}"
        );
        let tool_error = error.into_tool_error("generic MCP accept failed");
        assert_eq!(
            tool_error.code,
            meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code()
        );
        assert_eq!(
            tool_error
                .data
                .as_ref()
                .and_then(|data| data.get("session_resume_required"))
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "resume-required ingress failure must retain structured MCP data"
        );
        assert_eq!(
            context.current_attachment_witness(&session_id).await,
            attachment_before_accept,
            "generic input must not change attachment state owned by archive cleanup"
        );
        assert_eq!(
            context.has_sidecar(&session_id).await,
            sidecar_before_accept,
            "generic input must not change sidecar state owned by archive cleanup"
        );
    }

    #[tokio::test]
    async fn exact_detach_retires_attachment_plumbing_but_retains_logical_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session_id =
            create_deferred_session_with_generated_authority(&context, "runtime cleanup target")
                .await;
        let router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let attachment = context
            .configure_session_with_router(
                &session_id,
                McpCallbackConfig::Replace(None),
                Arc::clone(&router),
            )
            .await
            .expect("exact MCP attachment should commit");

        let requested_input_id = InputId::new();
        let admission = context
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve active admission");
        context
            .insert_runtime_pre_admission(
                &attachment,
                session_id.clone(),
                requested_input_id,
                admission,
            )
            .await
            .expect("insert pending pre-admission");

        assert!(
            context
                .detach_exact(&session_id, attachment.witness())
                .await
        );

        assert!(context.has_sidecar(&session_id).await);
        assert!(
            context
                .current_attachment_witness(&session_id)
                .await
                .is_none()
        );
        assert!(Arc::ptr_eq(
            &context
                .logical_router(&session_id)
                .await
                .expect("router retained"),
            &router
        ));
        assert!(!attachment.has_pre_admissions());
        assert!(attachment.is_retired());
    }

    #[tokio::test]
    async fn normal_post_stop_cleanup_locks_boundary_before_attachment_operation_gate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let (created, attachment) = create_session_with_test_mcp_config(
            &context,
            "post-stop lock order",
            meerkat_core::service::InitialTurnPolicy::Defer,
            router,
            None,
        )
        .await
        .expect("materialize exact attachment");
        let session_id = created.session_id;
        let turn_boundary = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            context
                .service
                .acquire_runtime_turn_finalization_guard(&session_id),
        )
        .await
        .expect("pending materialization must not retain the service boundary");
        let cleanup = McpSessionRuntimePostStopCleanupHandle {
            context: context.clone(),
            session_id: session_id.clone(),
            attachment: Arc::clone(&attachment),
            actor_witness_slot: None,
        };
        let cleanup_task =
            tokio::spawn(async move { cleanup.cleanup_after_runtime_stop_terminalized().await });
        tokio::task::yield_now().await;

        let operation_guard = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            Arc::clone(&attachment.operation_gate).lock_owned(),
        )
        .await
        .expect("normal cleanup must wait on B before acquiring the operation gate");
        drop(operation_guard);
        drop(turn_boundary);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup_task)
            .await
            .expect("post-stop cleanup must finish after B is released")
            .expect("cleanup task joins");

        if let Some(witness) = context
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
        {
            context
                .runtime_adapter
                .unregister_executor_attachment_if_current(&witness)
                .await
                .expect("test cleanup unregisters any retained machine attachment");
        }
    }

    #[tokio::test]
    async fn materialization_does_not_retain_registration_gate_while_cleanup_waits_for_boundary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let prepared = crate::prepare_new_mcp_actor_materialization(
            &context.service,
            &context.runtime_adapter,
            &session_id,
        )
        .await
        .expect("prepare exact materialization");
        let mut request = create_request(
            "post-ensure cleanup lock order",
            meerkat_core::service::InitialTurnPolicy::Defer,
        );
        let build = request.build.get_or_insert_with(Default::default);
        build.resume_session = Some(session);
        build.runtime_build_mode =
            meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
        let create = crate::create_runtime_backed_session_and_run_initial_turn_call_local(
            &context.service,
            &context.runtime_adapter,
            &session_id,
            request,
            None,
            prepared,
        )
        .await
        .expect("call-local actor creation");
        let crate::McpCallLocalActorCreateOutcome {
            result,
            transaction,
        } = create;
        result.expect("deferred actor creation");

        context
            .runtime_adapter
            .test_pause_next_executor_after_ensure();
        let commit_context = context.clone();
        let commit_session_id = session_id.clone();
        let mut commit = tokio::spawn(async move {
            commit_context
                .commit_prepared_session(
                    &commit_session_id,
                    transaction,
                    McpCallbackConfig::Preserve,
                    None,
                )
                .await
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            context
                .runtime_adapter
                .test_wait_for_executor_after_ensure_pause(),
        )
        .await
        .expect("MCP materialization must reach the exact post-ensure hook");
        let turn_boundary = context
            .service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        context
            .runtime_adapter
            .test_release_executor_after_ensure_pause();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut commit)
                .await
                .is_err(),
            "faulted attachment cleanup must wait for the retained service boundary"
        );

        let registration_lease = context.runtime_registration_lock(&session_id);
        let registration_guard = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            Arc::clone(&registration_lease.lock).lock_owned(),
        )
        .await
        .expect("pending materialization must not retain the MCP registration gate");
        drop(registration_guard);
        drop(registration_lease);
        drop(turn_boundary);

        let error = tokio::time::timeout(std::time::Duration::from_secs(5), commit)
            .await
            .expect("faulted materialization cleanup must finish after B is released")
            .expect("materialization task joins")
            .expect_err("injected post-ensure stop must reject materialization");
        assert!(error.to_string().contains("post-ensure hook failed"));
    }

    #[tokio::test]
    async fn sidecar_cas_loss_aborts_before_attachment_serves() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let prepared = crate::prepare_new_mcp_actor_materialization(
            &context.service,
            &context.runtime_adapter,
            &session_id,
        )
        .await
        .expect("prepare fresh exact materialization");
        let mut request = create_request(
            "sidecar compare-and-swap loss",
            meerkat_core::service::InitialTurnPolicy::Defer,
        );
        let build = request.build.get_or_insert_with(Default::default);
        build.resume_session = Some(session);
        build.runtime_build_mode =
            meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
        let create = crate::create_runtime_backed_session_and_run_initial_turn_call_local(
            &context.service,
            &context.runtime_adapter,
            &session_id,
            request,
            None,
            prepared,
        )
        .await
        .expect("call-local actor creation");
        let crate::McpCallLocalActorCreateOutcome {
            result,
            mut transaction,
        } = create;
        result.expect("deferred actor creation");
        let actor_witness = transaction
            .actor_witness_slot()
            .witness()
            .expect("created actor witness");

        let candidate = context.snapshot_config_candidate(&session_id).await;
        let created_attachment = Arc::new(StdMutex::new(None));
        let created_attachment_for_factory = Arc::clone(&created_attachment);
        let executor_context = context.clone();
        let executor_session_id = session_id.clone();
        let actor_witness_slot = transaction.actor_witness_slot().clone();
        let attachment_actor_witness = actor_witness.clone();
        let pending = match transaction
            .ensure_executor_attachment(move |witness| {
                let attachment = Arc::new(McpRuntimeAttachmentState::new_for_actor(
                    witness,
                    attachment_actor_witness,
                ));
                *created_attachment_for_factory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(Arc::clone(&attachment));
                Box::new(McpSessionRuntimeExecutor::new_exact(
                    executor_context,
                    executor_session_id,
                    attachment,
                    actor_witness_slot,
                )) as Box<dyn CoreExecutor>
            })
            .await
            .expect("prepare pending exact attachment")
        {
            meerkat_runtime::EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            meerkat_runtime::EnsureRuntimeExecutorAttachment::Existing(_) => {
                panic!("fresh transaction must not find an existing attachment")
            }
        };
        let attachment = created_attachment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("executor factory publishes attachment state");

        // Invalidate the logical candidate after it was captured but before
        // the inactive sidecar is staged. The failure occurs while the machine
        // still withholds serving publication.
        assert!(
            context
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "pending attachment must not serve before sidecar staging"
        );
        context.sidecars.write().await.insert(
            session_id.clone(),
            McpSessionSidecar {
                logical: McpLogicalSessionConfig::fresh(),
                attachment: None,
            },
        );
        let primary = context
            .stage_attachment_publication(&session_id, &candidate, Arc::clone(&attachment))
            .await
            .expect_err("logical CAS loss must fail before serving publication");
        let error = abort_pending_mcp_attachment_after_error(pending, primary).await;
        assert!(error.to_string().contains("meerkat_resume"));
        assert!(
            context
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "failed sidecar staging must not leave its machine attachment serving"
        );
        assert!(
            context
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(&actor_witness)
                .await
                .expect("inspect exact actor")
                .is_none(),
            "failed sidecar staging must compare-remove its exact actor"
        );
        assert!(
            context
                .current_attachment_witness(&session_id)
                .await
                .is_none(),
            "failed attachment must never become surface-visible"
        );
    }

    #[tokio::test]
    async fn post_commit_validation_retirement_failure_preserves_coherent_assembly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let (created, attachment) = create_session_with_test_mcp_config(
            &context,
            "retirement failure coherence",
            meerkat_core::service::InitialTurnPolicy::Defer,
            router,
            None,
        )
        .await
        .expect("materialize coherent exact attachment");
        let session_id = created.session_id;
        let attachment_witness = attachment.witness().clone();
        let actor_witness = attachment
            .actor_witness
            .get()
            .expect("exact actor witness")
            .clone();

        let error = context
            .finish_failed_committed_attachment_cleanup(
                &session_id,
                &attachment_witness,
                &actor_witness,
                Err(meerkat_runtime::RuntimeDriverError::Internal(
                    "injected exact retirement failure after commit validation failure".to_string(),
                )),
            )
            .await
            .expect_err("retirement failure must fail without partial owner cleanup");
        assert!(
            error
                .to_string()
                .contains("injected exact retirement failure")
        );
        assert_eq!(
            context
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await,
            Some(attachment_witness.clone()),
            "machine attachment remains coherently owned when retirement fails"
        );
        assert_eq!(
            context.current_attachment_witness(&session_id).await,
            Some(attachment_witness),
            "retirement failure must not detach the exact sidecar"
        );
        assert!(
            context
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(&actor_witness)
                .await
                .expect("inspect exact actor")
                .is_some(),
            "retirement failure must not discard the serving attachment's actor"
        );
        assert!(!attachment.is_retired());
    }

    #[tokio::test]
    async fn actor_missing_router_update_fails_closed_without_split_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let session_id = create_deferred_session_with_generated_authority(
            &context,
            "actor-missing router update",
        )
        .await;
        let original_router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let attachment = context
            .configure_session_with_router(
                &session_id,
                McpCallbackConfig::Replace(None),
                Arc::clone(&original_router),
            )
            .await
            .expect("attach original router");
        let original_witness = attachment.witness().clone();
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("open actor-missing recovery window");

        let replacement_router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let error = context
            .configure_session_with_router(
                &session_id,
                McpCallbackConfig::Preserve,
                replacement_router,
            )
            .await
            .expect_err("actor-only recovery cannot commit a logical router update");
        assert!(error.to_string().contains("meerkat_resume"));
        assert!(Arc::ptr_eq(
            &context
                .logical_router(&session_id)
                .await
                .expect("original logical router remains"),
            &original_router,
        ));
        assert_eq!(
            context.current_attachment_witness(&session_id).await,
            Some(original_witness),
            "failed router update must preserve exact attachment A"
        );
        assert!(
            !context
                .service
                .has_live_session(&session_id)
                .await
                .expect("actor liveness lookup"),
            "failed router update must not construct an actor from unpublished config"
        );
    }

    #[tokio::test]
    async fn stale_attachment_cannot_publish_over_or_clean_up_replacement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 2).await;
        let router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let (created, attachment_a) = create_session_with_test_mcp_config(
            &context,
            "exact attachment replacement",
            meerkat_core::service::InitialTurnPolicy::Defer,
            Arc::clone(&router),
            None,
        )
        .await
        .expect("materialize attachment A");
        let session_id = created.session_id;
        let candidate_a = context.snapshot_config_candidate(&session_id).await;
        let witness_a = attachment_a.witness().clone();
        let queued_input = InputId::new();
        let (event_tx, _event_rx) = mpsc::channel(1);
        attachment_a
            .register_turn_context(queued_input.clone(), Some(event_tx))
            .expect("queue context belongs to attachment A")
            .disarm();

        let session = context
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative lookup")
            .expect("persisted replacement session");
        let prepared = crate::prepare_mcp_actor_materialization(
            &context.service,
            &context.runtime_adapter,
            &session_id,
        )
        .await
        .expect("explicit resume seam prepares replacement B");
        let mut request = create_request("", meerkat_core::service::InitialTurnPolicy::Defer);
        let build = request.build.get_or_insert_with(Default::default);
        build.resume_session = Some(session);
        build.runtime_build_mode =
            meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
        build.external_tools = Some(router.clone());
        let outcome = crate::create_runtime_backed_session_and_run_initial_turn_call_local(
            &context.service,
            &context.runtime_adapter,
            &session_id,
            request,
            None,
            prepared,
        )
        .await
        .expect("call-local replacement create");
        let crate::McpCallLocalActorCreateOutcome {
            result,
            transaction,
        } = outcome;
        result.expect("replacement actor B should resume");
        let attachment_b = context
            .commit_prepared_session(&session_id, transaction, McpCallbackConfig::Preserve, None)
            .await
            .expect("commit exact replacement B");
        assert_ne!(attachment_b.witness(), &witness_a);
        assert!(
            attachment_a
                .take_turn_context_for_inputs(std::slice::from_ref(&queued_input))
                .is_none(),
            "retiring A must cancel its queued request"
        );
        assert!(
            attachment_b
                .take_turn_context_for_inputs(std::slice::from_ref(&queued_input))
                .is_none(),
            "queued work owned by A must never migrate to B"
        );

        assert!(
            !context.detach_exact(&session_id, &witness_a).await,
            "cleanup owned by attachment A must not compare-remove B"
        );
        let stale_publish = context
            .stage_attachment_publication(&session_id, &candidate_a, attachment_a)
            .await
            .expect_err("delayed attachment A publication must fail closed");
        assert!(stale_publish.to_string().contains("meerkat_resume"));
        assert_eq!(
            context.current_attachment_witness(&session_id).await,
            Some(attachment_b.witness().clone()),
            "neither stale publication nor stale cleanup may disturb B"
        );
        assert!(Arc::ptr_eq(
            &context
                .logical_router(&session_id)
                .await
                .expect("session-scoped router survives replacement"),
            &router,
        ));
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
        let attachment = context
            .ensure_session(&session_id)
            .await
            .expect("attach pre-admission fixture");

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
                &attachment,
                session_id.clone(),
                requested_input_id.clone(),
                requested_admission,
            )
            .await
            .expect("insert requested pre-admission");
        context
            .insert_runtime_pre_admission(
                &attachment,
                session_id.clone(),
                unrelated_input_id.clone(),
                unrelated_admission,
            )
            .await
            .expect("insert unrelated pre-admission");

        attachment.discard_pre_admission(&requested_input_id);
        attachment.discard_pre_admission(&accepted_input_id);

        {
            let entries = attachment
                .pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            .discard_runtime_pre_admission(&attachment, &unrelated_input_id)
            .await;
    }

    #[tokio::test]
    async fn mcp_runtime_completion_cleanup_retains_pre_admission_on_authority_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 3).await;
        let source_session_id = SessionId::new();
        let target_session_id =
            create_deferred_session_with_generated_authority(&context, "mismatched cleanup target")
                .await;
        let attachment = context
            .ensure_session(&target_session_id)
            .await
            .expect("attach mismatched cleanup target");
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
                &attachment,
                target_session_id.clone(),
                requested_input_id.clone(),
                admission,
            )
            .await
            .expect("insert pending pre-admission");

        let handle = wrap_mcp_runtime_pre_admission_cleanup(
            context.clone(),
            target_session_id.clone(),
            Arc::clone(&attachment),
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
            attachment
                .pre_admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|entry| entry.input_id == requested_input_id),
            "MCP runtime pre-admission must be retained when generated cleanup authority rejects the observation"
        );

        context
            .discard_runtime_pre_admission(&attachment, &requested_input_id)
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
        let attachment = context
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
            !attachment.has_pre_admissions(),
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
    async fn mcp_runtime_accept_terminal_no_handle_retains_committed_runtime_state() {
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
        assert!(context.has_sidecar(&target_session_id).await);

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
            context
                .runtime_adapter
                .contains_session(&target_session_id)
                .await,
            "a successful no-handle request must not tear down the committed session attachment"
        );
        assert!(
            context.has_sidecar(&target_session_id).await,
            "a successful no-handle request must retain the committed MCP sidecar"
        );
        let attachment = context
            .sidecars
            .read()
            .await
            .get(&target_session_id)
            .and_then(|sidecar| sidecar.attachment.clone())
            .expect("committed attachment remains");
        assert!(!attachment.has_pre_admissions());
    }

    #[tokio::test]
    async fn actor_missing_accept_requires_explicit_resume_and_preserves_exact_sidecar() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context_with_capacity(&temp, 1).await;
        let router = Arc::new(McpRouterAdapter::new(meerkat_mcp::McpRouter::new()));
        let callback_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(crate::MpcToolDispatcher::new(&[]));
        let (created, attachment) = create_session_with_test_mcp_config(
            &context,
            "actor missing",
            meerkat_core::service::InitialTurnPolicy::Defer,
            Arc::clone(&router),
            Some(Arc::clone(&callback_tools)),
        )
        .await
        .expect("attach exact MCP session");
        let session_id = created.session_id;
        let witness = attachment.witness().clone();
        let revision = context
            .sidecars
            .read()
            .await
            .get(&session_id)
            .expect("configured sidecar")
            .logical
            .revision;
        context
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live actor");

        let error = context
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(meerkat_runtime::PromptInput::new("must fail closed", None)),
                None,
            )
            .await
            .expect_err("generic accept must not rebuild a missing actor");

        assert!(
            error.to_string().contains("meerkat_resume"),
            "unexpected fail-closed accept error: {error}"
        );
        let tool_error = error.into_tool_error("generic MCP accept failed");
        assert_eq!(
            tool_error.code,
            meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code()
        );
        assert_eq!(
            tool_error
                .data
                .as_ref()
                .and_then(|data| data.get("session_resume_required"))
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "resume-required ingress failure must retain structured MCP data"
        );
        let sidecars = context.sidecars.read().await;
        let sidecar = sidecars.get(&session_id).expect("sidecar is preserved");
        assert_eq!(sidecar.logical.revision, revision);
        assert!(Arc::ptr_eq(
            sidecar
                .attachment
                .as_ref()
                .expect("exact attachment remains"),
            &attachment,
        ));
        drop(sidecars);
        assert_eq!(
            context
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await,
            Some(witness),
            "fail-closed accept must not replace or unregister its attachment"
        );
        assert!(!attachment.has_pre_admissions());
        assert!(Arc::ptr_eq(
            &context
                .logical_router(&session_id)
                .await
                .expect("router remains"),
            &router,
        ));
        assert!(Arc::ptr_eq(
            &context
                .current_callback_tools(&session_id)
                .await
                .expect("callback config remains"),
            &callback_tools,
        ));
    }

    #[tokio::test]
    async fn mcp_runtime_apply_requests_post_handoff_teardown_for_archived_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let context = build_test_context(&temp).await;
        let session_id = create_deferred_session_with_generated_authority(&context, "hello").await;
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
                .current_attachment_witness(&session_id)
                .await
                .is_some(),
            "archived MCP apply must retain the exact attachment through executor cleanup handoff"
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

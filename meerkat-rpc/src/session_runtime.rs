//! SessionRuntime - keeps agents alive between turns.
//!
//! Delegates to [`PersistentSessionService`] for session lifecycle management.
//! `FactoryAgentBuilder` bridges `AgentFactory::build_agent()` into the
//! `SessionAgentBuilder` / `SessionAgent` traits used by the service.
//!
//! The runtime preserves the two-step create-then-run API required by the
//! JSON-RPC handlers: `create_session()` stages a build config and returns a
//! `SessionId`; `start_turn_via_runtime()` admits the first prompt through
//! `MeerkatMachine`, and the runtime executor materializes the session inside
//! the service.

#[path = "session_runtime/schedule_host.rs"]
mod schedule_host;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
#[cfg(feature = "mcp")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock, Weak};
#[cfg(feature = "mcp")]
use std::time::Duration;

use futures::StreamExt;
use meerkat::{
    AgentBuildConfig, AgentFactory, FactoryAgentBuilder, MachineServiceTurnCommitProtocol,
    MachineSessionArchiveProtocol, PersistenceBundle, PersistentSessionService, PromotingSlot,
    ScheduleService, ScheduleToolDispatcher, StagedPhase, StagedSessionRegistry, StagedSlot,
    encode_llm_client_override_for_service,
};
use meerkat_client::{LlmClient, realtime_session::RealtimeSessionOpenConfig};
#[cfg(all(test, feature = "mcp"))]
use meerkat_core::ToolConfigChangedPayload;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreApplyTerminal};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunApplyBoundary, RunPrimitive,
};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    DeferredPromptPolicy, InitialTurnPolicy, MobToolAuthorityContext, SessionControlError,
    SessionError, SessionForkAtRequest, SessionForkReplaceRequest, SessionForkResult,
    SessionHistoryPage, SessionHistoryQuery, SessionQuery, SessionService,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionServiceTranscriptEditExt,
    SessionSummary, SessionView, StageToolResultsRequest, StageToolResultsResult, StartTurnRequest,
    StartTurnRuntimeSemantics,
};
use meerkat_core::skills::{SkillError, SourceIdentityRegistry};
use meerkat_core::types::{Message, RunResult, SessionId};
use meerkat_core::{
    AgentExecutionSnapshot, Config, ConfigStore, ContentInput, ExternalToolSurfaceSnapshot,
    PeerIngressRuntimeSnapshot, PendingSystemContextAppend, Session, SessionLlmIdentity,
    SessionSystemContextState, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryError,
    SurfaceSessionRecoveryOverrides, SystemMessage, ToolScopeSnapshot, build_recovered_session,
};
#[cfg(feature = "mcp")]
use meerkat_core::{AgentToolDispatcher, ToolGateway};
use meerkat_core::{EventEnvelope, EventStream, InputId, RunId, StreamError};
use meerkat_runtime::{
    HydratedSessionLlmState, MeerkatMachine, ResolvedSessionLlmReconfigure, RuntimeDriverError,
    RuntimeState, SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus,
    SessionLlmReconfigureHost, SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};
use tokio::sync::{Mutex, Notify, RwLock, broadcast, mpsc};

use crate::error;
use crate::protocol::RpcError;
#[cfg(feature = "mcp")]
use meerkat::{
    McpLifecycleAction, McpLifecyclePhase, McpReloadTarget, McpRouter, McpRouterAdapter,
    McpServerConfig,
};
#[cfg(feature = "mcp")]
use meerkat_core::ToolConfigChangeOperation;

fn unknown_provider_message(provider: &str) -> String {
    format!("unknown provider '{provider}' (expected anthropic, openai, gemini, or self_hosted)")
}

fn parse_provider_override(provider: &str) -> Result<meerkat_core::Provider, String> {
    meerkat_core::Provider::parse_strict(provider).ok_or_else(|| unknown_provider_message(provider))
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

const PENDING_SESSION_EVENT_CHANNEL_CAPACITY: usize = 128;
const DEFAULT_RUNTIME_ARCHIVED_HISTORY_CAPACITY: usize = 1024;

#[cfg(test)]
type ServiceStartTurnResultReceiver =
    tokio::sync::oneshot::Receiver<Result<RunResult, SessionError>>;
type ServiceApplyRuntimeTurnResultReceiver =
    tokio::sync::oneshot::Receiver<Result<CoreApplyOutput, SessionError>>;
type RecoverableServiceApplyRuntimeTurnResultReceiver = tokio::sync::oneshot::Receiver<(
    Result<CoreApplyOutput, SessionError>,
    Option<ActiveCapacityGuard>,
)>;
type ActiveCapacityGuard = meerkat::RuntimeContextAdmissionGuard;
type StagedCapacityAdmissions = Arc<StdMutex<HashMap<SessionId, ActiveCapacityGuard>>>;

#[derive(Clone)]
struct StagedAdmissionRestore {
    admissions: StagedCapacityAdmissions,
    session_id: SessionId,
}

pub(crate) struct RuntimePreAdmission {
    admission: Option<ActiveCapacityGuard>,
    staged_restore: Option<StagedAdmissionRestore>,
}

#[derive(Debug)]
struct RecoveredCreateRequest {
    request: CreateSessionRequest,
    runtime_was_registered: bool,
}

#[derive(Clone, Copy, Debug)]
enum RecoveryRuntimeBindingMode {
    Authoritative,
    LocalResources,
}

#[derive(Clone)]
struct PendingSessionEventStreams {
    events: broadcast::Sender<EventEnvelope<AgentEvent>>,
    receiver_dropped: Arc<Notify>,
}

#[derive(Clone)]
struct ArchiveRuntimeCleanup {
    runtime_adapter: Arc<MeerkatMachine>,
    pending_session_event_streams:
        Option<Arc<Mutex<HashMap<SessionId, PendingSessionEventStreams>>>>,
    #[cfg(feature = "mcp")]
    mcp_sessions: Option<Arc<RwLock<std::collections::HashMap<SessionId, SessionMcpState>>>>,
    #[cfg(feature = "mob")]
    mob_state: Option<Arc<meerkat_mob_mcp::MobMcpState>>,
}

impl ArchiveRuntimeCleanup {
    async fn has_retained_mob_cleanup(&self, session_id: &SessionId) -> bool {
        #[cfg(feature = "mob")]
        if let Some(mob_state) = self.mob_state.as_ref()
            && mob_state
                .has_bridge_session_scoped_mobs(&session_id.to_string())
                .await
        {
            return true;
        }

        #[cfg(not(feature = "mob"))]
        let _ = session_id;

        false
    }

    async fn archive_service(
        &self,
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        service
            .archive_with_machine_protocol(
                session_id,
                MachineSessionArchiveProtocol::from_machine(self.runtime_adapter.as_ref()),
            )
            .await
    }

    async fn run(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.runtime_adapter.unregister_session(session_id).await;
        if let Some(streams) = self.pending_session_event_streams.as_ref() {
            streams.lock().await.remove(session_id);
        }
        #[cfg(feature = "mcp")]
        if let Some(mcp_sessions) = self.mcp_sessions.as_ref()
            && let Some(state) = mcp_sessions.write().await.remove(session_id)
        {
            state.adapter.shutdown().await;
        }
        #[cfg(feature = "mob")]
        if let Some(mob_state) = self.mob_state.as_ref() {
            mob_state
                .destroy_bridge_session_mobs(&session_id.to_string())
                .await
                .map_err(|error| {
                    error.into_session_error("mob cleanup during archive incomplete")
                })?;
        }
        #[cfg(feature = "comms")]
        self.runtime_adapter.abort_comms_drain(session_id).await;
        Ok(())
    }
}

struct PendingSessionEventStreamDrop {
    receiver_dropped: Arc<Notify>,
}

impl Drop for PendingSessionEventStreamDrop {
    fn drop(&mut self) {
        self.receiver_dropped.notify_one();
    }
}

fn realtime_projection_root_system_message(session: &Session) -> Option<Message> {
    let build_state = session.build_state().unwrap_or_default();
    let mut content = build_state
        .system_prompt
        .or_else(|| {
            session
                .messages()
                .first()
                .and_then(|message| match message {
                    Message::System(system) => Some(system.content.clone()),
                    Message::SystemNotice(notice) => Some(notice.rendered_text()),
                    _ => None,
                })
        })
        .unwrap_or_default();

    if let Some(additional_instructions) = build_state.additional_instructions
        && !additional_instructions.is_empty()
    {
        if !content.trim().is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("[Session Build Instructions]");
        for instruction in additional_instructions {
            let instruction = instruction.trim();
            if instruction.is_empty() {
                continue;
            }
            content.push_str("\n- ");
            content.push_str(instruction);
        }
    }

    if content.trim().is_empty() {
        None
    } else {
        Some(Message::System(SystemMessage::new(content)))
    }
}

fn realtime_projection_messages(session: &Session) -> Vec<Message> {
    let mut projected = session.messages().to_vec();
    if let Some(root_system) = realtime_projection_root_system_message(session) {
        match projected.first() {
            Some(Message::System(_) | Message::SystemNotice(_)) => projected[0] = root_system,
            _ => projected.insert(0, root_system),
        }
    }
    projected
}

fn realtime_projection_runtime_system_context(
    session: &Session,
) -> Vec<PendingSystemContextAppend> {
    let state = session.system_context_state().unwrap_or_default();
    state.applied.into_iter().chain(state.pending).collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
fn exported_tool_visibility_state(session: &Session) -> meerkat_core::SessionToolVisibilityState {
    session
        .tool_visibility_state()
        .expect("exported visibility state should decode")
        .unwrap_or_default()
}

#[cfg(test)]
fn builtin_tool_visibility_witness() -> meerkat_core::ToolVisibilityWitness {
    let provenance = meerkat_core::ToolProvenance {
        kind: meerkat_core::ToolSourceKind::Builtin,
        source_id: "builtin".into(),
    };
    meerkat_core::ToolVisibilityWitness {
        stable_owner_key: Some(
            meerkat_core::tool_catalog::stable_owner_key_from_provenance(&provenance),
        ),
        last_seen_provenance: Some(provenance),
    }
}

struct RuntimePreAdmissionGuard {
    admission: Option<RuntimePreAdmission>,
}

struct RuntimePreAdmissionEntry {
    input_id: InputId,
    admission: RuntimePreAdmission,
}

struct RuntimePreAdmissionRegistration {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
    input_id: InputId,
    release_on_drop: bool,
}

struct RuntimeRegistrationLockLease {
    locks: Arc<StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>>,
    session_id: SessionId,
    lock: Arc<Mutex<()>>,
}

struct StagedArchiveRollbackGuard {
    staged_sessions: Arc<StagedSessionRegistry>,
    session_id: SessionId,
    restore_on_drop: bool,
}

#[derive(Clone, Copy)]
enum GuardedSessionCleanupOperation {
    Archive,
    DiscardLive,
}

#[cfg(test)]
#[derive(Default)]
struct PendingPromotionPreTurnHook {
    reached_flag: std::sync::atomic::AtomicBool,
    reached: Notify,
    release: Notify,
}

impl RuntimePreAdmissionGuard {
    fn new(admission: impl Into<RuntimePreAdmission>) -> Self {
        Self {
            admission: Some(admission.into()),
        }
    }

    fn take(&mut self) -> Option<RuntimePreAdmission> {
        self.admission.take()
    }
}

impl RuntimePreAdmissionRegistration {
    fn new(runtime: Arc<SessionRuntime>, session_id: SessionId, input_id: InputId) -> Self {
        Self {
            runtime,
            session_id,
            input_id,
            release_on_drop: true,
        }
    }

    fn disarm(mut self) {
        self.release_on_drop = false;
    }
}

impl RuntimeRegistrationLockLease {
    fn mutex(&self) -> &Mutex<()> {
        &self.lock
    }
}

impl Drop for RuntimePreAdmissionRegistration {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.runtime
                .restore_or_release_runtime_pre_admission(&self.session_id, &self.input_id);
        }
    }
}

impl Drop for RuntimeRegistrationLockLease {
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

impl StagedArchiveRollbackGuard {
    fn new(staged_sessions: Arc<StagedSessionRegistry>, session_id: &SessionId) -> Self {
        Self {
            staged_sessions,
            session_id: session_id.clone(),
            restore_on_drop: true,
        }
    }

    fn disarm(&mut self) {
        self.restore_on_drop = false;
    }
}

impl Drop for StagedArchiveRollbackGuard {
    fn drop(&mut self) {
        if !self.restore_on_drop {
            return;
        }
        let staged_sessions = Arc::clone(&self.staged_sessions);
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let _ = staged_sessions.restore_archive(&session_id).await;
        });
    }
}

fn restore_staged_capacity_admission(
    admissions: &StagedCapacityAdmissions,
    session_id: SessionId,
    admission: ActiveCapacityGuard,
) {
    let mut admissions = admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    admissions.insert(session_id, admission);
}

impl RuntimePreAdmission {
    fn fresh(admission: ActiveCapacityGuard) -> Self {
        Self {
            admission: Some(admission),
            staged_restore: None,
        }
    }

    fn staged(
        session_id: SessionId,
        admissions: StagedCapacityAdmissions,
        admission: ActiveCapacityGuard,
    ) -> Self {
        Self {
            admission: Some(admission),
            staged_restore: Some(StagedAdmissionRestore {
                admissions,
                session_id,
            }),
        }
    }

    #[allow(clippy::expect_used)]
    fn into_admission(mut self) -> ActiveCapacityGuard {
        self.staged_restore = None;
        self.admission
            .take()
            .expect("runtime pre-admission should not be consumed twice")
    }
}

impl From<ActiveCapacityGuard> for RuntimePreAdmission {
    fn from(admission: ActiveCapacityGuard) -> Self {
        Self::fresh(admission)
    }
}

impl Drop for RuntimePreAdmission {
    fn drop(&mut self) {
        let Some(admission) = self.admission.take() else {
            return;
        };
        if let Some(restore) = self.staged_restore.take() {
            restore_staged_capacity_admission(&restore.admissions, restore.session_id, admission);
        } else {
            drop(admission);
        }
    }
}

async fn await_guarded_session_cleanup(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    session_id: SessionId,
    operation: GuardedSessionCleanupOperation,
    runtime_cleanup: Option<ArchiveRuntimeCleanup>,
) -> Result<(), SessionError> {
    let result_session_id = session_id.clone();
    let operation_name = match operation {
        GuardedSessionCleanupOperation::Archive => "archive",
        GuardedSessionCleanupOperation::DiscardLive => "discard_live_session",
    };
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = match operation {
            GuardedSessionCleanupOperation::Archive => match runtime_cleanup.as_ref() {
                Some(cleanup) => cleanup.archive_service(&service, &session_id).await,
                None => Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "runtime-backed archive cleanup requires MachineSessionArchiveProtocol"
                            .to_string(),
                    ),
                )),
            },
            GuardedSessionCleanupOperation::DiscardLive => {
                service.discard_live_session(&session_id).await
            }
        };
        if matches!(operation, GuardedSessionCleanupOperation::Archive)
            && matches!(result, Ok(()) | Err(SessionError::NotFound { .. }))
            && let Some(cleanup) = runtime_cleanup.as_ref()
            && let Err(error) = cleanup.run(&session_id).await
        {
            let _ = result_tx.send(Err(error));
            return;
        }
        let _ = result_tx.send(result);
    });
    result_rx
        .await
        .map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "session service {operation_name} task ended before reporting a result for {result_session_id}"
            )))
        })?
}

#[allow(clippy::too_many_arguments)]
async fn await_guarded_staged_archive(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    staged_capacity_admissions: StagedCapacityAdmissions,
    mut staged_capacity_admission: Option<ActiveCapacityGuard>,
    runtime_cleanup: ArchiveRuntimeCleanup,
    session_id: SessionId,
    mut staged_archive_guard: StagedArchiveRollbackGuard,
    #[cfg(test)] after_service_hook: Option<Arc<PendingPromotionPreTurnHook>>,
) -> Result<(), SessionError> {
    let result_session_id = session_id.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = match runtime_cleanup.archive_service(&service, &session_id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {
                if let Err(error) = runtime_cleanup.run(&session_id).await {
                    let _ = staged_sessions.restore_archive(&session_id).await;
                    staged_archive_guard.disarm();
                    if let Some(admission) = staged_capacity_admission.take() {
                        restore_staged_capacity_admission(
                            &staged_capacity_admissions,
                            session_id.clone(),
                            admission,
                        );
                    }
                    let _ = result_tx.send(Err(error));
                    return;
                }
                #[cfg(test)]
                if let Some(hook) = after_service_hook {
                    hook.reached_flag
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    hook.reached.notify_waiters();
                    hook.release.notified().await;
                }
                let _ = staged_sessions.finish_archive(&session_id).await;
                staged_archive_guard.disarm();
                drop(staged_capacity_admission.take());
                Ok(())
            }
            Err(err) => {
                let _ = staged_sessions.restore_archive(&session_id).await;
                staged_archive_guard.disarm();
                if let Some(admission) = staged_capacity_admission.take() {
                    restore_staged_capacity_admission(
                        &staged_capacity_admissions,
                        session_id.clone(),
                        admission,
                    );
                }
                Err(err)
            }
        };
        let _ = result_tx.send(result);
    });
    result_rx
        .await
        .map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "session service staged archive task ended before reporting a result for {result_session_id}"
            )))
        })?
}

async fn await_session_archive_with_runtime_cleanup(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_cleanup: ArchiveRuntimeCleanup,
    session_id: SessionId,
) -> Result<(), SessionError> {
    let result_session_id = session_id.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = runtime_cleanup.archive_service(&service, &session_id).await;
        let service_not_found = matches!(&result, Err(SessionError::NotFound { .. }));
        let retained_mob_cleanup = if service_not_found {
            runtime_cleanup.has_retained_mob_cleanup(&session_id).await
        } else {
            false
        };
        if (result.is_ok() || service_not_found)
            && let Err(error) = runtime_cleanup.run(&session_id).await
        {
            let _ = result_tx.send(Err(error));
            return;
        }
        if service_not_found && retained_mob_cleanup {
            let _ = result_tx.send(Ok(()));
            return;
        }
        let _ = result_tx.send(result);
    });
    result_rx.await.map_err(|_| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "session service archive task ended before reporting a result for {result_session_id}"
        )))
    })?
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingPromotionCleanupMode {
    Restore,
    Finish,
}

struct PendingPromotionCleanup {
    staged_sessions: Arc<StagedSessionRegistry>,
    staged_capacity_admissions: StagedCapacityAdmissions,
    session_id: SessionId,
    staged_capacity_admission: Option<ActiveCapacityGuard>,
    build_config: Option<AgentBuildConfig>,
    effective_llm_identity: SessionLlmIdentity,
    labels: Option<BTreeMap<String, String>>,
    deferred_prompt: Option<ContentInput>,
    created_at_secs: u64,
    updated_at_secs: u64,
    mode: PendingPromotionCleanupMode,
    machine_archived_resume_authorized: bool,
    armed: bool,
}

impl PendingPromotionCleanup {
    fn new(
        staged_sessions: Arc<StagedSessionRegistry>,
        staged_capacity_admissions: StagedCapacityAdmissions,
        session_id: &SessionId,
        slot: &PromotingSlot,
        staged_capacity_admission: Option<ActiveCapacityGuard>,
    ) -> Self {
        Self {
            staged_sessions,
            staged_capacity_admissions,
            session_id: session_id.clone(),
            staged_capacity_admission,
            build_config: Some((*slot.build_config).clone()),
            effective_llm_identity: slot.effective_llm_identity.clone(),
            labels: slot.labels.clone(),
            deferred_prompt: slot.deferred_prompt.clone(),
            created_at_secs: slot.created_at_secs,
            updated_at_secs: slot.updated_at_secs,
            mode: PendingPromotionCleanupMode::Restore,
            machine_archived_resume_authorized: slot.machine_archived_resume_authorized,
            armed: true,
        }
    }

    fn update_build_config(&mut self, build_config: &AgentBuildConfig) {
        if self.armed {
            self.build_config = Some(build_config.clone());
        }
    }

    fn update_effective_llm_identity(&mut self, effective_llm_identity: SessionLlmIdentity) {
        if self.armed {
            self.effective_llm_identity = effective_llm_identity;
        }
    }

    fn mark_materialized(&mut self) {
        if self.armed {
            self.mode = PendingPromotionCleanupMode::Finish;
            self.staged_capacity_admission = None;
        }
    }

    fn take_staged_capacity_admission(&mut self) -> Option<ActiveCapacityGuard> {
        self.staged_capacity_admission.take()
    }

    async fn replenish_staged_capacity_admission(
        &mut self,
        service: &PersistentSessionService<FactoryAgentBuilder>,
    ) -> Result<(), SessionError> {
        if self.staged_capacity_admission.is_none() {
            self.staged_capacity_admission =
                Some(service.reserve_create_session_admission().await?);
        }
        Ok(())
    }

    async fn recover_materialized_staged_capacity_admission(
        &mut self,
        service: &PersistentSessionService<FactoryAgentBuilder>,
    ) -> Result<(), SessionError> {
        if self.staged_capacity_admission.is_none() {
            self.staged_capacity_admission = Some(
                service
                    .reserve_runtime_turn_admission(&self.session_id)
                    .await?,
            );
        }
        Ok(())
    }

    async fn abort_restore_without_capacity(&mut self) {
        tracing::warn!(
            session_id = %self.session_id,
            "aborting staged-session restore without a capacity admission"
        );
        let _ = self
            .staged_sessions
            .take_promoting_system_context_state(&self.session_id)
            .await;
        self.armed = false;
    }

    async fn restore_after_materialized_failure(
        &mut self,
        service: &PersistentSessionService<FactoryAgentBuilder>,
        protocol: MachineSessionArchiveProtocol<'_>,
    ) -> Result<(), SessionError> {
        if let Err(error) = self
            .recover_materialized_staged_capacity_admission(service)
            .await
        {
            self.abort_restore_without_capacity().await;
            return Err(error);
        }

        if let Err(error) = service
            .archive_with_machine_protocol(&self.session_id, protocol)
            .await
        {
            let _ = service.discard_live_session(&self.session_id).await;
            self.restore_now().await;
            return Err(error);
        }
        self.finish_after_machine_archive().await;
        Ok(())
    }

    async fn finish_after_machine_archive(&mut self) {
        if !self.armed {
            return;
        }
        let _ = self
            .staged_sessions
            .take_promoting_system_context_state(&self.session_id)
            .await;
        let _ = self.staged_sessions.abandon(&self.session_id).await;
        self.build_config = None;
        drop(self.staged_capacity_admission.take());
        self.armed = false;
    }

    fn authorize_machine_archived_resume(&mut self) {
        if !self.armed {
            return;
        }
        self.machine_archived_resume_authorized = true;
    }

    async fn preserve_promoting_system_context_state(
        staged_sessions: &StagedSessionRegistry,
        session_id: &SessionId,
        build_config: &mut AgentBuildConfig,
    ) {
        let Some((_starting_system_context_state, current_system_context_state)) = staged_sessions
            .promoting_system_context_state(session_id)
            .await
        else {
            return;
        };
        let session = build_config
            .resume_session
            .get_or_insert_with(|| Session::with_id(session_id.clone()));
        if let Err(err) = session.set_system_context_state(current_system_context_state) {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to preserve promoting system-context state while restoring staged session"
            );
        }
    }

    async fn restore_now(&mut self) {
        if !self.armed {
            return;
        }
        let Some(mut build_config) = self.build_config.take() else {
            self.armed = false;
            return;
        };
        Self::preserve_promoting_system_context_state(
            &self.staged_sessions,
            &self.session_id,
            &mut build_config,
        )
        .await;
        let Some(admission) = self.staged_capacity_admission.take() else {
            self.abort_restore_without_capacity().await;
            return;
        };
        let restored = self
            .staged_sessions
            .abandon_promotion(
                self.session_id.clone(),
                build_config,
                self.effective_llm_identity.clone(),
                self.labels.clone(),
                self.deferred_prompt.clone(),
                self.created_at_secs,
                self.updated_at_secs,
                self.machine_archived_resume_authorized,
            )
            .await;
        if restored {
            restore_staged_capacity_admission(
                &self.staged_capacity_admissions,
                self.session_id.clone(),
                admission,
            );
        }
        self.armed = false;
    }

    async fn finish_now(
        &mut self,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        if !self.armed || self.mode != PendingPromotionCleanupMode::Finish {
            return None;
        }
        self.staged_sessions
            .take_promoting_system_context_state(&self.session_id)
            .await
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.build_config = None;
        self.staged_capacity_admission = None;
    }
}

impl Drop for PendingPromotionCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let staged_sessions = Arc::clone(&self.staged_sessions);
        let staged_capacity_admissions = Arc::clone(&self.staged_capacity_admissions);
        let session_id = self.session_id.clone();
        match self.mode {
            PendingPromotionCleanupMode::Restore => {
                let Some(mut build_config) = self.build_config.take() else {
                    return;
                };
                let staged_capacity_admission = self.staged_capacity_admission.take();
                let effective_llm_identity = self.effective_llm_identity.clone();
                let labels = self.labels.clone();
                let deferred_prompt = self.deferred_prompt.clone();
                let created_at_secs = self.created_at_secs;
                let updated_at_secs = self.updated_at_secs;
                let machine_archived_resume_authorized = self.machine_archived_resume_authorized;
                tokio::spawn(async move {
                    let Some(admission) = staged_capacity_admission else {
                        tracing::warn!(
                            session_id = %session_id,
                            "aborting staged-session drop restore without a capacity admission"
                        );
                        let _ = staged_sessions
                            .take_promoting_system_context_state(&session_id)
                            .await;
                        return;
                    };
                    Self::preserve_promoting_system_context_state(
                        staged_sessions.as_ref(),
                        &session_id,
                        &mut build_config,
                    )
                    .await;
                    let restored = staged_sessions
                        .abandon_promotion(
                            session_id.clone(),
                            build_config,
                            effective_llm_identity,
                            labels,
                            deferred_prompt,
                            created_at_secs,
                            updated_at_secs,
                            machine_archived_resume_authorized,
                        )
                        .await;
                    if restored {
                        restore_staged_capacity_admission(
                            &staged_capacity_admissions,
                            session_id,
                            admission,
                        );
                    }
                });
            }
            PendingPromotionCleanupMode::Finish => {
                tokio::spawn(async move {
                    let _ = staged_sessions
                        .take_promoting_system_context_state(&session_id)
                        .await;
                });
            }
        }
    }
}

#[cfg(feature = "mob")]
struct RpcMobSessionService {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    runtime_adapter: Arc<MeerkatMachine>,
    mob_state: Arc<StdRwLock<Option<Arc<meerkat_mob_mcp::MobMcpState>>>>,
    #[cfg(test)]
    direct_create_after_ack_hook: Option<Arc<PendingPromotionPreTurnHook>>,
}

#[cfg(feature = "mob")]
impl RpcMobSessionService {
    fn archive_runtime_cleanup(&self) -> ArchiveRuntimeCleanup {
        ArchiveRuntimeCleanup {
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            pending_session_event_streams: None,
            #[cfg(feature = "mcp")]
            mcp_sessions: None,
            #[cfg(feature = "mob")]
            mob_state: self.mob_state.read().ok().and_then(|slot| slot.clone()),
        }
    }

    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: Arc<StagedSessionRegistry>,
        runtime_adapter: Arc<MeerkatMachine>,
        mob_state: Arc<StdRwLock<Option<Arc<meerkat_mob_mcp::MobMcpState>>>>,
    ) -> Self {
        Self {
            service,
            staged_sessions,
            runtime_adapter,
            mob_state,
            #[cfg(test)]
            direct_create_after_ack_hook: None,
        }
    }

    #[cfg(test)]
    fn with_direct_create_after_ack_hook(mut self, hook: Arc<PendingPromotionPreTurnHook>) -> Self {
        self.direct_create_after_ack_hook = Some(hook);
        self
    }

    fn ensure_create_session_id(req: &mut CreateSessionRequest) -> SessionId {
        let build = req.build.get_or_insert_with(Default::default);
        if let Some(session) = build.resume_session.as_ref() {
            return session.id().clone();
        }
        let session = Session::new();
        let session_id = session.id().clone();
        build.resume_session = Some(session);
        session_id
    }

    fn is_pre_run_start_turn_failure(result: &Result<RunResult, SessionError>) -> bool {
        matches!(
            result,
            Err(SessionError::Agent(
                meerkat_core::error::AgentError::NoPendingBoundary
            ))
        )
    }

    pub(crate) fn is_pre_run_apply_runtime_turn_failure(
        result: &Result<CoreApplyOutput, SessionError>,
    ) -> bool {
        matches!(
            result,
            Err(SessionError::Agent(
                meerkat_core::error::AgentError::NoPendingBoundary
            ))
        ) || matches!(
            result,
            Ok(output) if matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary))
        )
    }

    fn is_archived_create_rejection(error: &SessionError) -> bool {
        matches!(error, SessionError::NotFound { .. })
    }

    async fn pending_live_first_turn_is_still_deferred(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
    ) -> bool {
        service
            .live_deferred_first_turn_pending(session_id)
            .await
            .unwrap_or(false)
    }

    async fn should_restore_deferred_start_turn_admission(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
        result: &Result<RunResult, SessionError>,
    ) -> bool {
        Self::is_pre_run_start_turn_failure(result)
            || (result.is_err()
                && Self::pending_live_first_turn_is_still_deferred(service, session_id).await)
    }

    async fn should_restore_deferred_apply_runtime_turn_admission(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
        result: &Result<CoreApplyOutput, SessionError>,
    ) -> bool {
        Self::is_pre_run_apply_runtime_turn_failure(result)
            || (result.is_err()
                && Self::pending_live_first_turn_is_still_deferred(service, session_id).await)
    }

    async fn reject_archived_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        let Some(session) = self.service.load_authoritative_session(session_id).await? else {
            return Ok(());
        };
        if self
            .service
            .session_archived_by_authority(session_id, &session)
            .await?
        {
            if self.staged_sessions.contains(session_id).await {
                return Ok(());
            }
            let _ = self.service.discard_live_session(session_id).await;
            return Err(SessionError::NotFound {
                id: session_id.clone(),
            });
        }
        Ok(())
    }

    async fn reserve_turn_admission(
        &self,
        session_id: &SessionId,
    ) -> Result<ActiveCapacityGuard, SessionError> {
        self.service
            .reserve_runtime_turn_admission(session_id)
            .await
    }

    async fn await_guarded_start_turn(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        runtime_adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        req: StartTurnRequest,
        admission: ActiveCapacityGuard,
    ) -> Result<RunResult, SessionError> {
        let result_session_id = session_id.clone();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = service
                .run_machine_committed_live_turn(
                    MachineServiceTurnCommitProtocol::from_machine(runtime_adapter.as_ref()),
                    &session_id,
                    req,
                    admission,
                )
                .await
                .map_err(|(error, _admission)| error);
            let _ = result_tx.send(result);
        });
        result_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "session service start_turn task ended before reporting a result for {result_session_id}"
                )))
            })?
    }

    async fn await_guarded_apply_runtime_turn(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        session_id: SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: ActiveCapacityGuard,
    ) -> Result<CoreApplyOutput, SessionError> {
        let result_session_id = session_id.clone();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = service
                .apply_runtime_turn_with_reserved_admission(
                    &session_id,
                    run_id,
                    req,
                    boundary,
                    contributing_input_ids,
                    admission,
                )
                .await;
            let _ = result_tx.send(result);
        });
        result_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "session service apply_runtime_turn task ended before reporting a result for {result_session_id}"
                )))
            })?
    }

    #[allow(clippy::too_many_arguments)]
    async fn await_guarded_apply_runtime_context_appends(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        _staged_sessions: Option<Arc<StagedSessionRegistry>>,
        session_id: SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: ActiveCapacityGuard,
    ) -> Result<CoreApplyOutput, SessionError> {
        let result_session_id = session_id.clone();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = service
                .apply_runtime_context_appends_with_reserved_admission(
                    &session_id,
                    run_id,
                    appends,
                    boundary,
                    contributing_input_ids,
                    admission,
                )
                .await;
            let _ = result_tx.send(result);
        });
        result_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "session service apply_runtime_context_appends task ended before reporting a result for {result_session_id}"
                )))
            })?
    }

    async fn finish_guarded_apply_runtime_context_appends_admission(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: &Arc<StagedSessionRegistry>,
        session_id: &SessionId,
        admission: ActiveCapacityGuard,
    ) {
        let _ = (service, staged_sessions, session_id);
        drop(admission);
    }

    async fn await_guarded_create_session(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        session_id: SessionId,
        _initial_turn: InitialTurnPolicy,
        req: CreateSessionRequest,
        admission: ActiveCapacityGuard,
        #[cfg(test)] after_ack_hook: Option<Arc<PendingPromotionPreTurnHook>>,
    ) -> Result<RunResult, SessionError> {
        let result_session_id = session_id.clone();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = service
                .create_session_with_reserved_admission(req, admission)
                .await;
            #[cfg(test)]
            if _initial_turn == InitialTurnPolicy::Defer
                && result.is_ok()
                && let Some(hook) = after_ack_hook
            {
                hook.reached_flag
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                hook.reached.notify_waiters();
                hook.release.notified().await;
                if result_tx.is_closed() {
                    let _ = service.discard_live_session(&session_id).await;
                    return;
                }
            }
            let _ = result_tx.send(result);
        });
        result_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "session service create_session task ended before reporting a result for {result_session_id}"
            )))
        })?
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl SessionService for RpcMobSessionService {
    async fn create_session(
        &self,
        mut req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        let initial_turn = req.initial_turn;
        let session_id = Self::ensure_create_session_id(&mut req);
        if let Some(session) = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            && self
                .service
                .session_archived_by_authority(session.id(), session)
                .await?
        {
            return Err(SessionError::NotFound { id: session_id });
        }
        self.reject_archived_persisted_session(&session_id).await?;
        let admission = self.service.reserve_create_session_admission().await?;
        Self::await_guarded_create_session(
            Arc::clone(&self.service),
            session_id,
            initial_turn,
            req,
            admission,
            #[cfg(test)]
            self.direct_create_after_ack_hook.clone(),
        )
        .await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.reject_archived_persisted_session(id).await?;
        if self.staged_sessions.contains(id).await {
            return Err(SessionError::Busy { id: id.clone() });
        }
        let admission = self.reserve_turn_admission(id).await?;
        Self::await_guarded_start_turn(
            Arc::clone(&self.service),
            Arc::clone(&self.runtime_adapter),
            id.clone(),
            req,
            admission,
        )
        .await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "interrupt for runtime-backed session {id} must route through SessionRuntime::interrupt / MeerkatMachine::hard_cancel_current_run"
        )))
    }

    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "cancel_after_boundary for runtime-backed session {id} must route through SessionRuntime::interrupt_yielding / MeerkatMachine::cancel_after_boundary"
        )))
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        self.service.set_session_client(id, client).await
    }

    async fn hot_swap_session_llm_identity(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        self.service
            .hot_swap_session_llm_identity(id, client, identity, request_policy)
            .await
    }

    async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        self.service
            .set_session_tool_visibility_state(id, state)
            .await
    }

    async fn update_session_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        self.service.update_session_keep_alive(id, keep_alive).await
    }

    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        self.service
            .update_session_mob_authority_context(id, authority_context)
            .await
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        self.service.has_live_session(id).await
    }

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        self.service.set_session_tool_filter(id, filter).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        self.service.read(id).await
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        self.service.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        if self.staged_sessions.contains(id).await {
            return Err(SessionError::Busy { id: id.clone() });
        }
        let cleanup = self.archive_runtime_cleanup();
        let result = self
            .service
            .archive_with_machine_protocol(
                id,
                MachineSessionArchiveProtocol::from_machine(self.runtime_adapter.as_ref()),
            )
            .await;
        let service_not_found = matches!(&result, Err(SessionError::NotFound { .. }));
        let retained_mob_cleanup = if service_not_found {
            cleanup.has_retained_mob_cleanup(id).await
        } else {
            false
        };
        if result.is_ok() || service_not_found {
            cleanup.run(id).await?;
        }
        if service_not_found && retained_mob_cleanup {
            return Ok(());
        }
        result
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        self.service.subscribe_session_events(id).await
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl meerkat_core::service::SessionServiceCommsExt for RpcMobSessionService {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.service.comms_runtime(session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.service.event_injector(session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.service.interaction_event_injector(session_id).await
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl SessionServiceControlExt for RpcMobSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.service.append_system_context(id, req).await
    }

    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        self.service.stage_tool_results(id, req).await
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl SessionServiceHistoryExt for RpcMobSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        self.service.read_history(id, query).await
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl meerkat_mob::MobSessionService for RpcMobSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<MeerkatMachine>> {
        Some(Arc::clone(&self.runtime_adapter))
    }

    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.service
            .interrupt_with_machine_authority(session_id, authority)
            .await
    }

    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.service
            .cancel_after_boundary_with_machine_authority(session_id, authority)
            .await
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let Some(session) = self.service.load_authoritative_session(session_id).await? else {
            return Ok(None);
        };
        if self
            .service
            .session_archived_by_authority(session_id, &session)
            .await?
        {
            return Ok(None);
        }
        Ok(Some(session))
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Self as SessionService>::archive(self, session_id).await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        self.service.execution_snapshot(session_id).await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        self.service.tool_scope_snapshot(session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        self.service
            .external_tool_surface_snapshot(session_id)
            .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.service.comms_runtime(session_id).await else {
            return Ok(None);
        };
        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(meerkat_core::CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        if self.staged_sessions.contains(session_id).await {
            return Err(SessionError::Busy {
                id: session_id.clone(),
            });
        }
        self.service.discard_live_session(session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.reject_archived_persisted_session(session_id).await?;
        let admission = self.reserve_turn_admission(session_id).await?;
        Self::await_guarded_apply_runtime_turn(
            Arc::clone(&self.service),
            session_id.clone(),
            run_id,
            req,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_boundary(
            session_id,
            run_id,
            appends,
            RunApplyBoundary::Immediate,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_context_appends_with_boundary(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.reject_archived_persisted_session(session_id).await?;
        let admission = self.reserve_turn_admission(session_id).await?;
        Self::await_guarded_apply_runtime_context_appends(
            Arc::clone(&self.service),
            Some(Arc::clone(&self.staged_sessions)),
            session_id.clone(),
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        session_id: &SessionId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        self.service
            .apply_runtime_system_context_for_turn(session_id, appends)
            .await
    }

    async fn cancel_all_checkpointers(&self) {
        self.service.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.service.rearm_all_checkpointers().await;
    }
}

fn session_error_to_runtime_driver(err: SessionError) -> RuntimeDriverError {
    match err {
        SessionError::NotFound { .. } => RuntimeDriverError::NotReady {
            state: meerkat_runtime::RuntimeState::Destroyed,
        },
        other => RuntimeDriverError::Internal(other.to_string()),
    }
}

fn runtime_driver_error_to_session_error(err: RuntimeDriverError) -> SessionError {
    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
        err.to_string(),
    ))
}

fn runtime_driver_error_to_rpc(err: RuntimeDriverError) -> RpcError {
    match err {
        RuntimeDriverError::ValidationFailed { reason } => RpcError {
            code: error::INVALID_PARAMS,
            message: reason,
            data: None,
        },
        RuntimeDriverError::NotReady { state } => RpcError {
            code: error::INVALID_REQUEST,
            message: format!("runtime not ready for llm reconfiguration: {state}"),
            data: None,
        },
        RuntimeDriverError::Destroyed => RpcError {
            code: error::SESSION_NOT_FOUND,
            message: "runtime destroyed".to_string(),
            data: None,
        },
        RuntimeDriverError::Internal(message) => RpcError {
            code: error::INTERNAL_ERROR,
            message,
            data: None,
        },
        other => RpcError {
            code: error::INTERNAL_ERROR,
            message: other.to_string(),
            data: None,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterruptNoopTarget {
    Present,
    Missing,
}

fn interrupt_noop_target_for_presence(present: bool) -> InterruptNoopTarget {
    if present {
        InterruptNoopTarget::Present
    } else {
        InterruptNoopTarget::Missing
    }
}

fn registered_model_provider_mismatch_reason(
    registry: &meerkat_core::ModelRegistry,
    provider: meerkat_core::Provider,
    model: &str,
) -> Option<String> {
    registry.provider_override_mismatch_reason(provider, model)
}

fn unsupported_model_capability_rpc_error(
    evidence: meerkat_core::UnsupportedModelCapabilityEvidence,
) -> RpcError {
    RpcError {
        code: error::INVALID_PARAMS,
        message: evidence.to_string(),
        data: Some(evidence.details()),
    }
}

fn profile_to_capability_surface(
    profile: &meerkat_models::profile::ModelProfile,
) -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: profile.supports_temperature,
        supports_thinking: profile.supports_thinking,
        supports_reasoning: profile.supports_reasoning,
        inline_video: profile.inline_video,
        vision: profile.vision,
        image_input: profile.image_input,
        image_tool_results: profile.image_tool_results,
        supports_web_search: profile.supports_web_search,
        image_generation: profile.image_generation,
        realtime: profile.realtime,
        call_timeout_secs: profile.call_timeout_secs,
    }
}

#[derive(Clone)]
struct SessionRuntimeLlmReconfigureHost {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    factory: AgentFactory,
    auth_lease: Arc<dyn meerkat_core::handles::AuthLeaseHandle>,
    default_llm_client: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
    agent_llm_client_decorator: Arc<StdRwLock<Option<meerkat_core::AgentLlmClientDecorator>>>,
    config_runtime: Arc<StdRwLock<Option<Arc<meerkat_core::ConfigRuntime>>>>,
}

impl SessionRuntimeLlmReconfigureHost {
    async fn capability_surface_for_identity(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<
        (
            Option<SessionLlmCapabilitySurface>,
            SessionLlmCapabilitySurfaceStatus,
        ),
        RuntimeDriverError,
    > {
        let registry = self.model_registry().await?;
        Ok(
            match registry.profile_for_provider(identity.provider, &identity.model) {
                Some(profile) => (
                    Some(profile_to_capability_surface(&profile)),
                    SessionLlmCapabilitySurfaceStatus::Resolved,
                ),
                None => (None, SessionLlmCapabilitySurfaceStatus::Unresolved),
            },
        )
    }

    async fn hydrate_staged_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<HydratedSessionLlmState>, RuntimeDriverError> {
        let Some(current_identity) = self
            .staged_sessions
            .effective_llm_identity(session_id)
            .await
        else {
            return Ok(None);
        };
        let (current_capability_surface, capability_surface_status) = self
            .capability_surface_for_identity(&current_identity)
            .await?;
        Ok(Some(HydratedSessionLlmState {
            current_identity,
            current_visibility_state: Default::default(),
            current_capability_surface,
            capability_surface_status,
            base_tool_names: std::collections::BTreeSet::new(),
        }))
    }

    async fn model_registry(&self) -> Result<meerkat_core::ModelRegistry, RuntimeDriverError> {
        let config_runtime = self
            .config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let config = if let Some(runtime) = config_runtime {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| RuntimeDriverError::Internal(format!("Failed to load config: {e}")))?
        } else {
            meerkat_core::Config::default()
        };

        config.model_registry().map_err(|e| {
            RuntimeDriverError::Internal(format!("Failed to resolve model registry: {e}"))
        })
    }

    async fn build_adapter_for_llm_identity(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmClient>, RuntimeDriverError> {
        let default_llm_client = self
            .default_llm_client
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let raw_client = if let Some(default) = default_llm_client {
            default
        } else {
            let config = self.load_config_for_hot_swap().await?;
            self.factory
                .build_llm_client_for_identity_with_auth_lease(
                    &config,
                    identity,
                    Some(Arc::clone(&self.auth_lease)),
                )
                .await
                .map_err(|e| {
                    RuntimeDriverError::Internal(format!(
                        "Failed to build LLM client for session identity hot-swap: {e}"
                    ))
                })?
        };

        let adapter = self
            .factory
            .build_llm_adapter(raw_client, identity.model.clone())
            .await;
        let adapter = Arc::new(adapter) as Arc<dyn meerkat_core::AgentLlmClient>;
        let decorator = self
            .agent_llm_client_decorator
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Ok(AgentFactory::decorate_agent_llm_client(
            adapter,
            decorator.as_ref(),
        ))
    }

    async fn load_config_for_hot_swap(&self) -> Result<Config, RuntimeDriverError> {
        let config_runtime = self
            .config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(runtime) = config_runtime {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| {
                    RuntimeDriverError::Internal(format!("Failed to load config for hot-swap: {e}"))
                })
        } else {
            Ok(meerkat_core::Config::default())
        }
    }

    async fn build_request_policy_for_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Result<meerkat_core::SessionLlmRequestPolicy, RuntimeDriverError> {
        let config = self.load_config_for_hot_swap().await?;
        self.factory
            .request_policy_for_llm_identity(&config, identity)
            .map_err(|e| {
                RuntimeDriverError::Internal(format!(
                    "Failed to build LLM request policy for session {session_id} identity hot-swap: {e}"
                ))
            })
    }

    async fn resolve_target_llm_identity(
        &self,
        current: &SessionLlmIdentity,
        request: &SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmIdentity, RuntimeDriverError> {
        if request.provider.is_some() && request.model.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider override requires model on an existing session".to_string(),
            });
        }
        if request.clear_provider_params && request.provider_params.is_some() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "clear_provider_params cannot be combined with provider_params".to_string(),
            });
        }
        if request.clear_auth_binding && request.auth_binding.is_some() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "clear_auth_binding cannot be combined with auth_binding".to_string(),
            });
        }

        let registry = self.model_registry().await?;
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| current.model.clone());
        let provider = if let Some(provider_name) = request.provider.as_ref() {
            parse_provider_override(provider_name)
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        } else {
            current.provider
        };
        if (request.model.is_some() || request.provider.is_some())
            && let Some(reason) =
                registered_model_provider_mismatch_reason(&registry, provider, &model)
        {
            return Err(RuntimeDriverError::ValidationFailed { reason });
        }
        let provider_params = if request.clear_provider_params {
            None
        } else {
            request
                .provider_params
                .clone()
                .or_else(|| current.provider_params.clone())
        };
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if request.model.is_none() {
                current.self_hosted_server_id.clone().or_else(|| {
                    registry
                        .entry_for_provider(meerkat_core::Provider::SelfHosted, &model)
                        .and_then(|entry| entry.self_hosted.as_ref())
                        .map(|server| server.server_id.clone())
                })
            } else {
                match registry.entry_for_provider(meerkat_core::Provider::SelfHosted, &model) {
                    Some(entry) => entry
                        .self_hosted
                        .as_ref()
                        .map(|server| server.server_id.clone()),
                    None => {
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: format!(
                                "self-hosted provider requires a registered model alias; '{model}' is not configured"
                            ),
                        });
                    }
                }
            }
        } else {
            None
        };

        let auth_binding = if request.clear_auth_binding {
            None
        } else {
            request
                .auth_binding
                .clone()
                .or_else(|| current.auth_binding.clone())
        };

        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params,
            auth_binding,
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionLlmReconfigureHost for SessionRuntimeLlmReconfigureHost {
    async fn hydrate_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        let current_identity = match self.service.live_session_llm_identity(session_id).await {
            Ok(identity) => identity,
            Err(err) => {
                if let Some(hydrated) = self.hydrate_staged_session_llm_state(session_id).await? {
                    return Ok(hydrated);
                }
                return Err(session_error_to_runtime_driver(err));
            }
        };
        let session = match self.service.export_live_session(session_id).await {
            Ok(session) => session,
            Err(err) => {
                if let Some(hydrated) = self.hydrate_staged_session_llm_state(session_id).await? {
                    return Ok(hydrated);
                }
                return Err(session_error_to_runtime_driver(err));
            }
        };
        let current_visibility_state = session
            .try_tool_visibility_state()
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "invalid canonical tool visibility state: {err}"
                ))
            })?
            .unwrap_or_default();
        let base_tool_names = self
            .service
            .tool_scope_snapshot(session_id)
            .await
            .map_err(session_error_to_runtime_driver)?
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "session {session_id} missing live tool scope snapshot during llm reconfiguration"
                ))
            })?
            .known_base_names
            .into_iter()
            .collect();

        let (current_capability_surface, capability_surface_status) = self
            .capability_surface_for_identity(&current_identity)
            .await?;

        Ok(HydratedSessionLlmState {
            current_identity,
            current_visibility_state,
            current_capability_surface,
            capability_surface_status,
            base_tool_names,
        })
    }

    async fn resolve_target_session_llm_identity(
        &self,
        request: &SessionLlmReconfigureRequest,
        current_identity: &SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        let target_identity = self
            .resolve_target_llm_identity(current_identity, request)
            .await?;
        let registry = self.model_registry().await?;
        let profile = registry
            .profile_for_provider(target_identity.provider, &target_identity.model)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "no capability profile is registered for provider '{}' and model '{}'",
                    target_identity.provider.as_str(),
                    target_identity.model
                ),
            })?;

        Ok(ResolvedSessionLlmReconfigure {
            target_identity,
            target_capability_surface: profile_to_capability_surface(&profile),
        })
    }

    async fn apply_live_session_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Result<(), RuntimeDriverError> {
        let adapter = self.build_adapter_for_llm_identity(identity).await?;
        let request_policy = self
            .build_request_policy_for_llm_identity(session_id, identity)
            .await?;
        self.service
            .apply_runtime_session_llm_identity(
                session_id,
                adapter,
                identity.clone(),
                request_policy,
            )
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        session_id: &SessionId,
        visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        self.service
            .set_session_tool_visibility_state(session_id, visibility_state)
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn persist_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .persist_live_session_now(session_id)
            .await
            .map(|_| ())
            .map_err(session_error_to_runtime_driver)
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .discard_live_session(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }
}

#[derive(Clone)]
struct SkillIdentityRegistryState {
    generation: u64,
    registry: SourceIdentityRegistry,
}

#[cfg(feature = "mcp")]
struct SessionMcpState {
    adapter: Arc<McpRouterAdapter>,
    turn_counter: u32,
    lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    lifecycle_rx: mpsc::UnboundedReceiver<McpLifecycleAction>,
    drain_task_running: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is idle and ready to accept a new turn.
    Idle,
    /// A turn is currently running.
    Running,
    /// The session is shutting down.
    ShuttingDown,
}

impl SessionState {
    /// Return a stable string representation matching the serde `rename_all` convention.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Summary information about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub state: SessionState,
    pub labels: BTreeMap<String, String>,
}

fn now_unix_secs() -> u64 {
    meerkat_core::time_compat::SystemTime::now()
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Merge a deferred prompt with a turn prompt, preserving multimodal content.
/// If both are plain text, concatenates with `\n\n`. Otherwise, converts both
/// to content blocks and concatenates (deferred blocks first, then turn blocks).
fn merge_content_inputs(deferred: ContentInput, turn: ContentInput) -> ContentInput {
    match (&deferred, &turn) {
        (ContentInput::Text(d), ContentInput::Text(t)) => ContentInput::Text(format!("{d}\n\n{t}")),
        _ => {
            let mut blocks = deferred.into_blocks();
            blocks.extend(turn.into_blocks());
            ContentInput::Blocks(blocks)
        }
    }
}

// FactoryAgent and FactoryAgentBuilder are imported from meerkat::service_factory.
// Staged-session authority lives in meerkat::StagedSessionRegistry.

// ---------------------------------------------------------------------------
// SessionRuntime
// ---------------------------------------------------------------------------

/// Core runtime that manages agent sessions.
///
/// Wraps [`PersistentSessionService`] for session lifecycle management while
/// preserving the two-step create-then-run API required by JSON-RPC handlers.
pub struct SessionRuntime {
    factory: AgentFactory,
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    schedule_service: ScheduleService,
    artifact_store: Arc<dyn meerkat_core::ArtifactStore>,
    schedule_host: Mutex<Option<meerkat::surface::ScheduleHostHandle>>,
    /// Canonical staged-session authority (facade-owned). Holds sessions
    /// that have been created (ID returned to caller) but not yet materialized
    /// in the service. The first `start_turn` call promotes them through
    /// `staged_sessions.begin_promotion()`.
    staged_sessions: Arc<StagedSessionRegistry>,
    /// Event streams opened while a deferred session is still staged. The
    /// first materializing turn fans out events here, then bridges the stream
    /// to the live session service after promotion.
    pending_session_event_streams: Arc<Mutex<HashMap<SessionId, PendingSessionEventStreams>>>,
    /// Service-owned capacity guards for staged sessions that have not yet
    /// materialized into live session handles.
    staged_capacity_admissions: StagedCapacityAdmissions,
    runtime_pre_admissions: Arc<StdMutex<HashMap<SessionId, Vec<RuntimePreAdmissionEntry>>>>,
    runtime_registration_locks: Arc<StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>>,
    #[cfg(test)]
    pending_promotion_pre_turn_hook: Arc<StdMutex<Option<Arc<PendingPromotionPreTurnHook>>>>,
    #[cfg(test)]
    runtime_routed_pre_promotion_hook: Arc<StdMutex<Option<Arc<PendingPromotionPreTurnHook>>>>,
    #[cfg(test)]
    staged_archive_after_service_hook: Arc<StdMutex<Option<Arc<PendingPromotionPreTurnHook>>>>,
    #[cfg(test)]
    create_session_after_prepare_bindings_error: Arc<StdMutex<Option<String>>>,
    #[cfg(test)]
    create_session_after_prepare_bindings_session_id: Arc<StdMutex<Option<SessionId>>>,
    /// Startup fallback capacity used when no live config runtime is attached.
    max_sessions: usize,
    /// Override LLM client for all sessions (primarily for testing).
    default_llm_client: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
    /// Default wrapper applied after all session LLM clients reach the agent boundary.
    agent_llm_client_decorator: Arc<StdRwLock<Option<meerkat_core::AgentLlmClientDecorator>>>,
    realm_id: Option<meerkat_core::connection::RealmId>,
    instance_id: Option<String>,
    backend: Option<String>,
    config_runtime: Arc<StdRwLock<Option<Arc<meerkat_core::ConfigRuntime>>>>,
    runtime_adapter: Arc<MeerkatMachine>,
    /// Notification sink for event forwarding to the RPC transport.
    /// Wrapped in `RwLock` so it can be updated when a new TCP client
    /// connects (each connection has its own transport sink).
    notification_sink: StdRwLock<crate::router::NotificationSink>,
    skill_identity_registry: Arc<StdRwLock<SkillIdentityRegistryState>>,
    skill_identity_context_root: Arc<StdRwLock<Option<PathBuf>>>,
    skill_identity_user_root: Arc<StdRwLock<Option<PathBuf>>>,
    #[cfg(feature = "mob")]
    mob_state: Arc<StdRwLock<Option<Arc<meerkat_mob_mcp::MobMcpState>>>>,
    #[cfg(feature = "mcp")]
    mcp_sessions: Arc<RwLock<std::collections::HashMap<SessionId, SessionMcpState>>>,
    /// Channel for sending callback tool requests to the RPC server loop.
    /// Wrapped in `RwLock` so it can be set after Arc wrapping (server construction).
    #[allow(clippy::type_complexity)]
    callback_request_tx: StdRwLock<
        Option<
            mpsc::Sender<(
                crate::protocol::RpcRequest,
                tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
            )>,
        >,
    >,
    /// Counter for generating unique server-originated callback request IDs.
    callback_id_counter_slot: StdRwLock<Arc<std::sync::atomic::AtomicU64>>,
    /// Globally registered callback tool definitions (via `tools/register`).
    registered_tools_slot: StdRwLock<Arc<StdRwLock<Vec<meerkat_core::ToolDef>>>>,
    /// Handle to the builder's mob tools slot inside the session service.
    /// Captured before the builder is consumed so `set_mob_tools` can write
    /// through to the actual builder that creates agents.
    pub builder_mob_tools_slot:
        Arc<StdRwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>,
    /// Captured before the builder is consumed so runtime construction can
    /// inject scheduler tools into resumed/runtime-backed agent builds.
    pub builder_schedule_tools_slot:
        Arc<StdRwLock<Option<Arc<dyn meerkat_core::AgentToolDispatcher>>>>,
    /// Handle to the builder's default agent LLM client decorator slot.
    pub builder_agent_llm_client_decorator_slot:
        Arc<StdRwLock<Option<meerkat_core::AgentLlmClientDecorator>>>,
    /// Runtime-owned approval records. Surfaces only project decisions into
    /// this service; approval status is service-owned.
    approval_service: meerkat_core::ApprovalService,
}

#[cfg(test)]
fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn session_metadata_marks_mob_member(session: &Session) -> bool {
    session
        .session_metadata()
        .and_then(|metadata| metadata.peer_meta)
        .is_some_and(|peer_meta| peer_meta.labels.contains_key("mob_id"))
}

fn approval_service_from_persistence(
    persistence: &PersistenceBundle,
) -> meerkat_core::ApprovalService {
    let Some(store_path) = persistence.store_path() else {
        return meerkat_core::ApprovalService::new();
    };
    let path = store_path.join("approvals.json");
    match meerkat_store::FileApprovalStore::open(&path) {
        Ok(store) => match meerkat_core::ApprovalService::with_store(Arc::new(store)) {
            Ok(service) => service,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    "failed to load approval records; falling back to process-local approvals"
                );
                meerkat_core::ApprovalService::new()
            }
        },
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "failed to open approval store; falling back to process-local approvals"
            );
            meerkat_core::ApprovalService::new()
        }
    }
}

impl SessionRuntime {
    fn take_runtime_pre_admission_guard(
        pre_admission: &mut RuntimePreAdmissionGuard,
        session_id: &SessionId,
    ) -> Result<RuntimePreAdmission, RpcError> {
        pre_admission.take().ok_or_else(|| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("runtime pre-admission guard already consumed for {session_id}"),
            data: None,
        })
    }

    fn turn_keep_alive_policy(
        requested: Option<bool>,
    ) -> Option<meerkat_core::lifecycle::run_primitive::KeepAlivePolicy> {
        requested.and_then(|keep_alive| {
            keep_alive.then(|| meerkat_core::lifecycle::run_primitive::KeepAlivePolicy {
                ttl: std::time::Duration::from_secs(30),
                policy: meerkat_core::lifecycle::run_primitive::KeepAliveMode::Pinned,
            })
        })
    }

    fn turn_additional_instructions(
        requested: Option<Vec<String>>,
    ) -> Option<Vec<meerkat_core::lifecycle::run_primitive::TurnInstruction>> {
        requested.map(|instructions| {
            instructions
                .into_iter()
                .map(
                    |body| meerkat_core::lifecycle::run_primitive::TurnInstruction {
                        kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::User,
                        body,
                    },
                )
                .collect()
        })
    }

    fn turn_metadata_from_overrides(
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
        provider_hint: Option<&str>,
    ) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
        use meerkat_core::lifecycle::run_primitive::{
            ProviderParamsOverride, TurnMetadataOverride,
        };

        let provider_params = overrides.and_then(|ov| {
            if ov.clear_provider_params {
                Some(TurnMetadataOverride::Clear)
            } else {
                let provider = ov
                    .provider
                    .as_deref()
                    .or(provider_hint)
                    .unwrap_or("unknown");
                ov.provider_params.clone().map(|params| {
                    TurnMetadataOverride::Set(ProviderParamsOverride::from_legacy_provider_value(
                        provider, &params,
                    ))
                })
            }
        });
        let auth_binding = overrides.and_then(|ov| {
            if ov.clear_auth_binding {
                Some(TurnMetadataOverride::Clear)
            } else {
                ov.auth_binding.clone().map(TurnMetadataOverride::Set)
            }
        });
        let metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            handling_mode: None,
            keep_alive: overrides.and_then(|ov| Self::turn_keep_alive_policy(ov.keep_alive)),
            skill_references,
            flow_tool_overlay,
            additional_instructions: Self::turn_additional_instructions(additional_instructions),
            model: overrides
                .and_then(|ov| ov.model.clone())
                .map(meerkat_core::lifecycle::run_primitive::ModelId::new),
            provider: overrides
                .and_then(|ov| ov.provider.as_ref())
                .and_then(|provider| parse_provider_override(provider).ok()),
            provider_params,
            render_metadata: None,
            auth_binding,
            execution_kind: None,
            peer_response_terminal_apply_intent: None,
        };
        (!metadata.is_empty()).then_some(metadata)
    }

    fn runtime_stamped_prompt_turn_metadata_from_overrides(
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
        provider_hint: Option<&str>,
    ) -> meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
        meerkat_runtime::runtime_stamped_prompt_turn_metadata(Self::turn_metadata_from_overrides(
            skill_references,
            flow_tool_overlay,
            additional_instructions,
            overrides,
            provider_hint,
        ))
    }

    pub(crate) fn turn_overrides_from_metadata(
        metadata: Option<&meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    ) -> Option<crate::handlers::turn::TurnOverrides> {
        use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;

        let metadata = metadata?;
        let (provider_params, clear_provider_params) = match &metadata.provider_params {
            Some(TurnMetadataOverride::Set(params)) => {
                (Some(params.to_legacy_provider_value()), false)
            }
            Some(TurnMetadataOverride::Clear) => (None, true),
            None => (None, false),
        };
        let (auth_binding, clear_auth_binding) = match &metadata.auth_binding {
            Some(TurnMetadataOverride::Set(auth_binding)) => (Some(auth_binding.clone()), false),
            Some(TurnMetadataOverride::Clear) => (None, true),
            None => (None, false),
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            keep_alive: metadata.keep_alive.as_ref().map(|_| true),
            model: metadata.model.as_ref().map(ToString::to_string),
            provider: metadata
                .provider
                .map(|provider| provider.as_str().to_string()),
            provider_params,
            clear_provider_params,
            auth_binding,
            clear_auth_binding,
            ..Default::default()
        };
        (!overrides.is_empty()).then_some(overrides)
    }

    #[cfg(feature = "comms")]
    async fn preserve_existing_peer_ingress(
        &self,
        session_id: &SessionId,
        requested_keep_alive: bool,
    ) -> bool {
        requested_keep_alive || self.runtime_adapter.session_has_comms(session_id).await
    }

    async fn live_session_is_stale(&self, session_id: &SessionId) -> Result<bool, RpcError> {
        if self
            .service
            .synchronize_live_session_from_durable_authority_if_needed(session_id)
            .await
            .map_err(session_error_to_rpc)?
        {
            return Ok(false);
        }

        let live = match self.service.export_live_session(session_id).await {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => {
                return Ok(self.load_persisted_session(session_id).await?.is_some());
            }
            Err(err) => return Err(session_error_to_rpc(err)),
        };
        let Some(stored) = self.load_persisted_session(session_id).await? else {
            return Ok(false);
        };
        Ok(stored.messages().len() > live.messages().len())
    }

    async fn archived_persisted_session_without_live(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RpcError> {
        let Some(stored) = self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(session_error_to_rpc)?
        else {
            return Ok(false);
        };
        if !self
            .session_archived_by_authority(session_id, &stored)
            .await?
        {
            return Ok(false);
        }
        if self.staged_sessions.contains(session_id).await {
            return Ok(false);
        }
        let _ = self.service.discard_live_session(session_id).await;
        self.discard_staged_capacity_admission(session_id);
        self.archive_runtime_cleanup()
            .run(session_id)
            .await
            .map_err(session_error_to_rpc)?;
        Ok(true)
    }

    pub async fn reject_archived_persisted_session_without_live(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RpcError> {
        if self
            .archived_persisted_session_without_live(session_id)
            .await?
        {
            return Err(Self::session_not_found_rpc(session_id));
        }
        Ok(())
    }

    fn session_not_found_rpc(session_id: &SessionId) -> RpcError {
        RpcError {
            code: error::SESSION_NOT_FOUND,
            message: format!("session not found: {session_id}"),
            data: None,
        }
    }

    /// Create a new session runtime.
    pub fn new(
        factory: AgentFactory,
        config: Config,
        max_sessions: usize,
        persistence: PersistenceBundle,
        notification_sink: crate::router::NotificationSink,
    ) -> Self {
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let artifact_store = persistence.artifact_store();
        let factory_clone = factory.clone();
        let builder = FactoryAgentBuilder::new(factory, config);
        let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        let builder_schedule_tools_slot = Arc::clone(&builder.default_schedule_tools);
        let builder_agent_llm_client_decorator_slot =
            Arc::clone(&builder.default_agent_llm_client_decorator);
        let default_llm_client = Arc::new(StdRwLock::new(None));
        let config_runtime = Arc::new(StdRwLock::new(None));
        let staged_sessions = Arc::new(StagedSessionRegistry::new());
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        let approval_service = approval_service_from_persistence(&persistence);
        let (service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service_with_capacities(
                builder,
                max_sessions,
                DEFAULT_RUNTIME_ARCHIVED_HISTORY_CAPACITY,
                persistence,
            );
        let service = Arc::new(service);
        let reconfigure_auth_lease = runtime_adapter.auth_lease_handle();
        runtime_adapter.set_session_llm_reconfigure_host(Arc::new(
            SessionRuntimeLlmReconfigureHost {
                service: Arc::clone(&service),
                staged_sessions: Arc::clone(&staged_sessions),
                factory: factory_clone.clone(),
                auth_lease: reconfigure_auth_lease,
                default_llm_client: Arc::clone(&default_llm_client),
                agent_llm_client_decorator: Arc::clone(&builder_agent_llm_client_decorator_slot),
                config_runtime: Arc::clone(&config_runtime),
            },
        ));

        Self {
            factory: factory_clone,
            service,
            schedule_service,
            artifact_store,
            schedule_host: Mutex::new(None),
            staged_sessions,
            pending_session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            staged_capacity_admissions: Arc::new(StdMutex::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(StdMutex::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            pending_promotion_pre_turn_hook: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            runtime_routed_pre_promotion_hook: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            staged_archive_after_service_hook: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            create_session_after_prepare_bindings_error: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            create_session_after_prepare_bindings_session_id: Arc::new(StdMutex::new(None)),
            max_sessions,
            default_llm_client,
            agent_llm_client_decorator: Arc::clone(&builder_agent_llm_client_decorator_slot),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime,
            runtime_adapter,
            notification_sink: StdRwLock::new(notification_sink),
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
            skill_identity_context_root: Arc::new(StdRwLock::new(None)),
            skill_identity_user_root: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "mob")]
            mob_state: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "mcp")]
            mcp_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            callback_request_tx: StdRwLock::new(None),
            callback_id_counter_slot: StdRwLock::new(Arc::new(std::sync::atomic::AtomicU64::new(
                0,
            ))),
            registered_tools_slot: StdRwLock::new(Arc::new(StdRwLock::new(Vec::new()))),
            builder_mob_tools_slot,
            builder_schedule_tools_slot,
            builder_agent_llm_client_decorator_slot,
            approval_service,
        }
    }

    /// Create a runtime that resolves config from a shared config store.
    pub fn new_with_config_store(
        factory: AgentFactory,
        initial_config: Config,
        config_store: Arc<dyn ConfigStore>,
        max_sessions: usize,
        persistence: PersistenceBundle,
        notification_sink: crate::router::NotificationSink,
    ) -> Self {
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let artifact_store = persistence.artifact_store();
        let factory_clone = factory.clone();
        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, initial_config, config_store);
        let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        let builder_schedule_tools_slot = Arc::clone(&builder.default_schedule_tools);
        let builder_agent_llm_client_decorator_slot =
            Arc::clone(&builder.default_agent_llm_client_decorator);
        let default_llm_client = Arc::new(StdRwLock::new(None));
        let config_runtime = Arc::new(StdRwLock::new(None));
        let staged_sessions = Arc::new(StagedSessionRegistry::new());
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        let approval_service = approval_service_from_persistence(&persistence);
        let (service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service_with_capacities(
                builder,
                max_sessions,
                DEFAULT_RUNTIME_ARCHIVED_HISTORY_CAPACITY,
                persistence,
            );
        let service = Arc::new(service);
        let reconfigure_auth_lease = runtime_adapter.auth_lease_handle();
        runtime_adapter.set_session_llm_reconfigure_host(Arc::new(
            SessionRuntimeLlmReconfigureHost {
                service: Arc::clone(&service),
                staged_sessions: Arc::clone(&staged_sessions),
                factory: factory_clone.clone(),
                auth_lease: reconfigure_auth_lease,
                default_llm_client: Arc::clone(&default_llm_client),
                agent_llm_client_decorator: Arc::clone(&builder_agent_llm_client_decorator_slot),
                config_runtime: Arc::clone(&config_runtime),
            },
        ));

        Self {
            factory: factory_clone,
            service,
            schedule_service,
            artifact_store,
            schedule_host: Mutex::new(None),
            staged_sessions,
            pending_session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            staged_capacity_admissions: Arc::new(StdMutex::new(HashMap::new())),
            runtime_pre_admissions: Arc::new(StdMutex::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            pending_promotion_pre_turn_hook: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            runtime_routed_pre_promotion_hook: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            staged_archive_after_service_hook: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            create_session_after_prepare_bindings_error: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            create_session_after_prepare_bindings_session_id: Arc::new(StdMutex::new(None)),
            max_sessions,
            default_llm_client,
            agent_llm_client_decorator: Arc::clone(&builder_agent_llm_client_decorator_slot),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime,
            runtime_adapter,
            notification_sink: StdRwLock::new(notification_sink),
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
            skill_identity_context_root: Arc::new(StdRwLock::new(None)),
            skill_identity_user_root: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "mob")]
            mob_state: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "mcp")]
            mcp_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            callback_request_tx: StdRwLock::new(None),
            callback_id_counter_slot: StdRwLock::new(Arc::new(std::sync::atomic::AtomicU64::new(
                0,
            ))),
            registered_tools_slot: StdRwLock::new(Arc::new(StdRwLock::new(Vec::new()))),
            builder_mob_tools_slot,
            builder_schedule_tools_slot,
            builder_agent_llm_client_decorator_slot,
            approval_service,
        }
    }

    /// Attach realm context defaults used for session metadata.
    pub fn set_realm_context(
        &mut self,
        realm_id: Option<meerkat_core::connection::RealmId>,
        instance_id: Option<String>,
        backend: Option<String>,
    ) {
        self.realm_id = realm_id;
        self.instance_id = instance_id;
        self.backend = backend;
    }

    pub fn set_skill_identity_roots(
        &mut self,
        context_root: Option<PathBuf>,
        user_root: Option<PathBuf>,
    ) {
        if let Ok(mut slot) = self.skill_identity_context_root.write() {
            *slot = context_root;
        }
        if let Ok(mut slot) = self.skill_identity_user_root.write() {
            *slot = user_root;
        }
    }

    pub fn skill_identity_roots(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        let context_root = self
            .skill_identity_context_root
            .read()
            .map(|slot| slot.clone())
            .unwrap_or(None);
        let user_root = self
            .skill_identity_user_root
            .read()
            .map(|slot| slot.clone())
            .unwrap_or(None);
        (context_root, user_root)
    }

    /// Set the default mob tools factory for all agents built by this runtime.
    ///
    /// Writes through the shared slot to the `FactoryAgentBuilder` inside the
    /// session service — the builder that actually creates agents. The runtime's
    /// own `factory` clone is NOT used for session creation.
    pub fn set_mob_tools(&mut self, factory: Arc<dyn meerkat_core::service::MobToolsFactory>) {
        *self
            .builder_mob_tools_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(factory);
    }

    /// Active realm id for this runtime, if configured.
    pub fn realm_id(&self) -> Option<&meerkat_core::connection::RealmId> {
        self.realm_id.as_ref()
    }

    /// Attach config runtime for generation stamping.
    pub fn set_config_runtime(&mut self, runtime: Arc<meerkat_core::ConfigRuntime>) {
        *self
            .config_runtime
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
    }

    /// Shared config runtime used by config handlers.
    pub fn config_runtime(&self) -> Option<Arc<meerkat_core::ConfigRuntime>> {
        self.config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn pending_live_first_turn_is_still_deferred(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
    ) -> bool {
        service
            .live_deferred_first_turn_pending(session_id)
            .await
            .unwrap_or(false)
    }

    #[cfg(test)]
    async fn should_restore_pending_after_start_turn(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
        result: &Result<RunResult, SessionError>,
    ) -> bool {
        Self::is_pre_run_start_turn_failure(result)
            || (result.is_err()
                && Self::pending_live_first_turn_is_still_deferred(service, session_id).await)
    }

    async fn should_restore_pending_after_apply_runtime_turn(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &SessionId,
        result: &Result<CoreApplyOutput, SessionError>,
    ) -> bool {
        Self::is_pre_run_apply_runtime_turn_failure(result)
            || (result.is_err()
                && Self::pending_live_first_turn_is_still_deferred(service, session_id).await)
    }

    async fn reserve_active_turn(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimePreAdmission, RpcError> {
        if let Some(admission) = self.take_staged_capacity_admission(session_id) {
            return Ok(RuntimePreAdmission::staged(
                session_id.clone(),
                Arc::clone(&self.staged_capacity_admissions),
                admission,
            ));
        }
        self.service
            .reserve_runtime_turn_admission(session_id)
            .await
            .map(RuntimePreAdmission::fresh)
            .map_err(session_error_to_rpc)
    }

    async fn reserve_or_join_active_turn(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimePreAdmission, RpcError> {
        if let Some(admission) = self.take_staged_capacity_admission(session_id) {
            return Ok(RuntimePreAdmission::staged(
                session_id.clone(),
                Arc::clone(&self.staged_capacity_admissions),
                admission,
            ));
        }
        self.service
            .reserve_runtime_turn_admission(session_id)
            .await
            .map(RuntimePreAdmission::fresh)
            .map_err(session_error_to_rpc)
    }

    fn insert_staged_capacity_admission(
        &self,
        session_id: SessionId,
        admission: ActiveCapacityGuard,
    ) -> Result<(), RpcError> {
        let mut admissions = self
            .staged_capacity_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if admissions.contains_key(&session_id) {
            return Err(RpcError {
                code: error::SESSION_BUSY,
                message: format!("session {session_id} already has staged capacity"),
                data: None,
            });
        }
        admissions.insert(session_id, admission);
        Ok(())
    }

    fn take_staged_capacity_admission(
        &self,
        session_id: &SessionId,
    ) -> Option<ActiveCapacityGuard> {
        self.staged_capacity_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id)
    }

    fn has_staged_capacity_admission(&self, session_id: &SessionId) -> bool {
        self.staged_capacity_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id)
    }

    fn discard_staged_capacity_admission(&self, session_id: &SessionId) {
        drop(self.take_staged_capacity_admission(session_id));
    }

    pub(crate) fn take_runtime_pre_admission(
        &self,
        session_id: &SessionId,
        input_ids: &[InputId],
    ) -> Option<RuntimePreAdmission> {
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
        Some(entry.admission)
    }

    fn insert_runtime_pre_admission(
        &self,
        session_id: SessionId,
        input_id: InputId,
        admission: impl Into<RuntimePreAdmission>,
    ) -> Result<(), RpcError> {
        let mut pre_admissions = self
            .runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = pre_admissions.entry(session_id.clone()).or_default();
        if entries.iter().any(|entry| entry.input_id == input_id) {
            return Err(RpcError {
                code: error::SESSION_BUSY,
                message: format!("session {session_id} already has queued runtime work"),
                data: None,
            });
        }
        entries.push(RuntimePreAdmissionEntry {
            input_id,
            admission: admission.into(),
        });
        Ok(())
    }

    fn restore_or_release_runtime_pre_admission(&self, session_id: &SessionId, input_id: &InputId) {
        let Some(admission) =
            self.take_runtime_pre_admission(session_id, std::slice::from_ref(input_id))
        else {
            return;
        };
        drop(admission);
    }

    fn register_runtime_pre_admission(
        self: &Arc<Self>,
        session_id: SessionId,
        input_id: InputId,
        admission: impl Into<RuntimePreAdmission>,
    ) -> Result<RuntimePreAdmissionRegistration, RpcError> {
        self.insert_runtime_pre_admission(session_id.clone(), input_id.clone(), admission)?;
        Ok(RuntimePreAdmissionRegistration::new(
            Arc::clone(self),
            session_id,
            input_id,
        ))
    }

    fn runtime_registration_lock(
        self: &Arc<Self>,
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

    async fn unregister_new_runtime_registration_if_idle(
        self: &Arc<Self>,
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        runtime_was_registered: bool,
        staged_session_existed: bool,
        protect_active_admission: bool,
    ) {
        if runtime_was_registered || staged_session_existed {
            return;
        }
        if self.staged_sessions.contains(session_id).await {
            return;
        }
        let _ = protect_active_admission;
        if adapter
            .list_active_inputs(session_id)
            .await
            .is_ok_and(|inputs| !inputs.is_empty())
        {
            return;
        }
        if self
            .service
            .has_live_session(session_id)
            .await
            .unwrap_or(false)
        {
            return;
        }
        adapter.unregister_session(session_id).await;
    }

    fn runtime_input_requires_active_pre_admission(input: &meerkat_runtime::Input) -> bool {
        let idle_policy = meerkat_runtime::DefaultPolicyTable::resolve(input, true);
        matches!(
            idle_policy.wake_mode,
            meerkat_runtime::policy::WakeMode::WakeIfIdle
        ) && matches!(
            idle_policy.apply_mode,
            meerkat_runtime::policy::ApplyMode::StageRunStart
                | meerkat_runtime::policy::ApplyMode::StageRunBoundary
                | meerkat_runtime::policy::ApplyMode::InjectNow
        )
    }

    fn completion_outcome_requires_runtime_cleanup(
        outcome: &meerkat_runtime::CompletionOutcome,
    ) -> bool {
        match outcome {
            meerkat_runtime::CompletionOutcome::Abandoned(reason)
            | meerkat_runtime::CompletionOutcome::AbandonedWithError { reason, .. }
            | meerkat_runtime::CompletionOutcome::RuntimeTerminated(reason) => {
                reason.contains("runtime boundary commit failed")
                    || reason.contains("runtime loop commit failed")
            }
            meerkat_runtime::CompletionOutcome::CompletedWithFinalizationFailure {
                error, ..
            } => error.detail.as_deref().is_some_and(|reason| {
                reason.contains("runtime boundary commit failed")
                    || reason.contains("runtime loop commit failed")
            }),
            meerkat_runtime::CompletionOutcome::Completed(_)
            | meerkat_runtime::CompletionOutcome::CompletedWithoutResult
            | meerkat_runtime::CompletionOutcome::Cancelled
            | meerkat_runtime::CompletionOutcome::CallbackPending { .. } => false,
        }
    }

    fn completion_outcome_is_apply_failure(outcome: &meerkat_runtime::CompletionOutcome) -> bool {
        match outcome {
            meerkat_runtime::CompletionOutcome::Abandoned(reason)
            | meerkat_runtime::CompletionOutcome::AbandonedWithError { reason, .. }
            | meerkat_runtime::CompletionOutcome::RuntimeTerminated(reason) => {
                reason.starts_with("apply failed:")
            }
            meerkat_runtime::CompletionOutcome::CompletedWithFinalizationFailure { .. } => false,
            meerkat_runtime::CompletionOutcome::Completed(_)
            | meerkat_runtime::CompletionOutcome::CompletedWithoutResult
            | meerkat_runtime::CompletionOutcome::Cancelled
            | meerkat_runtime::CompletionOutcome::CallbackPending { .. } => false,
        }
    }

    async fn cleanup_runtime_after_completion_outcome(
        &self,
        session_id: &SessionId,
        outcome: &meerkat_runtime::CompletionOutcome,
    ) {
        let archived_now = self
            .authoritative_session_archived(session_id)
            .await
            .unwrap_or(false);
        if archived_now {
            let _ = self.service.discard_live_session(session_id).await;
            let _ = self.archive_runtime_cleanup().run(session_id).await;
            return;
        }

        let live_present = self
            .service
            .has_live_session(session_id)
            .await
            .unwrap_or(false);
        if Self::completion_outcome_requires_runtime_cleanup(outcome)
            || (!live_present && Self::completion_outcome_is_apply_failure(outcome))
        {
            let _ = self.service.discard_live_session(session_id).await;
            let _ = self.archive_runtime_cleanup().run(session_id).await;
        }
    }

    fn spawn_runtime_pre_admission_cleanup(
        self: &Arc<Self>,
        session_id: SessionId,
        input_id: InputId,
        handle: meerkat_runtime::CompletionHandle,
    ) {
        drop(self.spawn_runtime_pre_admission_cleanup_with_outcome(session_id, input_id, handle));
    }

    fn spawn_runtime_pre_admission_cleanup_with_outcome(
        self: &Arc<Self>,
        session_id: SessionId,
        input_id: InputId,
        handle: meerkat_runtime::CompletionHandle,
    ) -> tokio::sync::oneshot::Receiver<meerkat_runtime::CompletionOutcome> {
        let runtime = Arc::clone(self);
        let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let outcome = handle.wait().await;
            runtime.restore_or_release_runtime_pre_admission(&session_id, &input_id);
            runtime
                .cleanup_runtime_after_completion_outcome(&session_id, &outcome)
                .await;
            let _ = outcome_tx.send(outcome);
        });
        outcome_rx
    }

    fn spawn_runtime_pre_admission_idle_cleanup(
        self: &Arc<Self>,
        session_id: SessionId,
        input_id: InputId,
    ) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let runtime_running = runtime
                    .runtime_adapter
                    .runtime_state(&session_id)
                    .await
                    .is_ok_and(|state| matches!(state, meerkat_runtime::RuntimeState::Running));
                let input_still_active = runtime
                    .runtime_adapter
                    .list_active_inputs(&session_id)
                    .await
                    .is_ok_and(|inputs| inputs.contains(&input_id));
                if !runtime_running || !input_still_active {
                    runtime.restore_or_release_runtime_pre_admission(&session_id, &input_id);
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        });
    }

    #[cfg(test)]
    fn is_pre_run_start_turn_failure(result: &Result<RunResult, SessionError>) -> bool {
        matches!(
            result,
            Err(SessionError::Agent(
                meerkat_core::error::AgentError::NoPendingBoundary
            ))
        )
    }

    pub(crate) fn is_pre_run_apply_runtime_turn_failure(
        result: &Result<CoreApplyOutput, SessionError>,
    ) -> bool {
        matches!(
            result,
            Err(SessionError::Agent(
                meerkat_core::error::AgentError::NoPendingBoundary
            ))
        ) || matches!(
            result,
            Ok(output) if matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary))
        )
    }

    fn is_archived_create_rejection(error: &SessionError) -> bool {
        matches!(error, SessionError::NotFound { .. })
    }

    #[cfg(test)]
    fn spawn_service_start_turn_with_admission_guard(
        &self,
        session_id: SessionId,
        req: StartTurnRequest,
        admission: ActiveCapacityGuard,
    ) -> ServiceStartTurnResultReceiver {
        let service = Arc::clone(&self.service);
        let runtime_adapter = Arc::clone(&self.runtime_adapter);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = service
                .run_machine_committed_live_turn(
                    MachineServiceTurnCommitProtocol::from_machine(runtime_adapter.as_ref()),
                    &session_id,
                    req,
                    admission,
                )
                .await
                .map_err(|(error, _admission)| error);
            let _ = result_tx.send(result);
        });
        result_rx
    }

    #[cfg(test)]
    async fn wait_pending_promotion_pre_turn_hook(
        hook_slot: &Arc<StdMutex<Option<Arc<PendingPromotionPreTurnHook>>>>,
    ) {
        let hook = hook_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(hook) = hook else {
            return;
        };
        hook.reached_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        hook.reached.notify_waiters();
        hook.release.notified().await;
    }

    #[cfg(test)]
    pub(crate) async fn wait_runtime_routed_pre_promotion_hook(&self, session_id: &SessionId) {
        if self
            .staged_sessions
            .info(session_id)
            .await
            .is_none_or(|info| info.is_promoting)
        {
            return;
        }
        if self.has_staged_capacity_admission(session_id) {
            return;
        }
        Self::wait_pending_promotion_pre_turn_hook(&self.runtime_routed_pre_promotion_hook).await;
    }

    #[cfg(test)]
    fn spawn_pending_create_and_start_turn_with_admission_guard(
        &self,
        session_id: SessionId,
        create_req: CreateSessionRequest,
        start_req: StartTurnRequest,
        mut promotion_cleanup: PendingPromotionCleanup,
        machine_archived_resume_authorized: bool,
    ) -> ServiceStartTurnResultReceiver {
        let service = Arc::clone(&self.service);
        let staged_sessions = Arc::clone(&self.staged_sessions);
        let runtime_adapter = Arc::clone(&self.runtime_adapter);
        #[cfg(test)]
        let pending_promotion_pre_turn_hook = Arc::clone(&self.pending_promotion_pre_turn_hook);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let create_result = match (
                promotion_cleanup.take_staged_capacity_admission(),
                machine_archived_resume_authorized,
            ) {
                (Some(admission), true) => {
                    service
                        .create_session_with_reserved_machine_archived_resume_admission(
                            create_req,
                            admission,
                            runtime_adapter.session_control_authority(),
                        )
                        .await
                }
                (Some(admission), false) => {
                    service
                        .create_session_with_reserved_admission(create_req, admission)
                        .await
                }
                (None, true) => Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "machine-authorized archived resume for session {session_id} is missing a reserved staged admission"
                    )),
                )),
                (None, false) => service.create_session(create_req).await,
            };
            match create_result {
                Ok(_) => {
                    promotion_cleanup.mark_materialized();
                }
                Err(err) => {
                    if Self::is_archived_create_rejection(&err) {
                        let _ = service.discard_live_session(&session_id).await;
                        runtime_adapter.unregister_session(&session_id).await;
                        promotion_cleanup.mark_materialized();
                        let _ = promotion_cleanup.finish_now().await;
                        promotion_cleanup.disarm();
                        let _ = result_tx.send(Err(err));
                        return;
                    }
                    if Self::pending_live_first_turn_is_still_deferred(&service, &session_id).await
                    {
                        promotion_cleanup.mark_materialized();
                        Self::finish_pending_promotion_after_service_turn(
                            Arc::clone(&service),
                            staged_sessions,
                            &session_id,
                            "create_session",
                        )
                        .await;
                    } else {
                        let materialized_after_error =
                            service.has_live_session(&session_id).await.unwrap_or(false);
                        if materialized_after_error {
                            promotion_cleanup.mark_materialized();
                            Self::finish_pending_promotion_after_service_turn(
                                Arc::clone(&service),
                                staged_sessions,
                                &session_id,
                                "create_session",
                            )
                            .await;
                        } else {
                            if let Err(error) = promotion_cleanup
                                .replenish_staged_capacity_admission(&service)
                                .await
                            {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to replenish staged capacity after create_session error"
                                );
                            }
                            promotion_cleanup.restore_now().await;
                        }
                    }
                    promotion_cleanup.disarm();
                    let _ = result_tx.send(Err(err));
                    return;
                }
            }

            #[cfg(test)]
            Self::wait_pending_promotion_pre_turn_hook(&pending_promotion_pre_turn_hook).await;

            let result = match service.reserve_runtime_turn_admission(&session_id).await {
                Ok(admission) => service
                    .run_machine_committed_live_turn(
                        MachineServiceTurnCommitProtocol::from_machine(runtime_adapter.as_ref()),
                        &session_id,
                        start_req,
                        admission,
                    )
                    .await
                    .map_err(|(error, _admission)| error),
                Err(error) => Err(error),
            };
            if Self::should_restore_pending_after_start_turn(&service, &session_id, &result).await {
                let restore_result = promotion_cleanup
                    .restore_after_materialized_failure(
                        &service,
                        MachineSessionArchiveProtocol::from_machine(runtime_adapter.as_ref()),
                    )
                    .await;
                promotion_cleanup.disarm();
                let _ = result_tx.send(match restore_result {
                    Ok(()) => result,
                    Err(error) => Err(error),
                });
                return;
            }

            Self::finish_pending_promotion_after_service_turn(
                Arc::clone(&service),
                staged_sessions,
                &session_id,
                "start_turn",
            )
            .await;
            promotion_cleanup.disarm();
            let _ = result_tx.send(result);
        });
        result_rx
    }

    #[cfg(test)]
    async fn await_service_start_turn(
        session_id: &SessionId,
        result_rx: ServiceStartTurnResultReceiver,
    ) -> Result<RunResult, RpcError> {
        result_rx
            .await
            .map_err(|_| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!(
                    "session service start_turn task ended before reporting a result for {session_id}"
                ),
                data: None,
            })?
            .map_err(session_error_to_rpc)
    }

    fn spawn_service_apply_runtime_turn_with_recoverable_admission_guard(
        &self,
        session_id: SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
        admission: ActiveCapacityGuard,
    ) -> RecoverableServiceApplyRuntimeTurnResultReceiver {
        let service = Arc::clone(&self.service);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = service
                .apply_runtime_turn_with_recoverable_reserved_admission(
                    &session_id,
                    run_id,
                    req,
                    boundary,
                    contributing_input_ids,
                    admission,
                )
                .await;
            match result {
                Ok(output) => {
                    let _ = result_tx.send((Ok(output), None));
                }
                Err((error, admission)) => {
                    let _ = result_tx.send((Err(error), admission));
                }
            }
        });
        result_rx
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_pending_create_and_apply_runtime_turn_with_admission_guard(
        &self,
        session_id: SessionId,
        create_req: CreateSessionRequest,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
        mut promotion_cleanup: PendingPromotionCleanup,
        keep_alive: bool,
        machine_archived_resume_authorized: bool,
    ) -> ServiceApplyRuntimeTurnResultReceiver {
        let service = Arc::clone(&self.service);
        let staged_sessions = Arc::clone(&self.staged_sessions);
        let runtime_adapter = Arc::clone(&self.runtime_adapter);
        #[cfg(test)]
        let pending_promotion_pre_turn_hook = Arc::clone(&self.pending_promotion_pre_turn_hook);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let create_result = match (
                promotion_cleanup.take_staged_capacity_admission(),
                machine_archived_resume_authorized,
            ) {
                (Some(admission), true) => {
                    service
                        .create_session_with_reserved_machine_archived_resume_admission(
                            create_req,
                            admission,
                            runtime_adapter.session_control_authority(),
                        )
                        .await
                }
                (Some(admission), false) => {
                    service
                        .create_session_with_reserved_admission(create_req, admission)
                        .await
                }
                (None, true) => Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "machine-authorized archived resume for session {session_id} is missing a reserved staged admission"
                    )),
                )),
                (None, false) => service.create_session(create_req).await,
            };
            match create_result {
                Ok(_) => {
                    promotion_cleanup.mark_materialized();
                }
                Err(err) => {
                    if Self::is_archived_create_rejection(&err) {
                        let _ = service.discard_live_session(&session_id).await;
                        runtime_adapter.unregister_session(&session_id).await;
                        promotion_cleanup.mark_materialized();
                        let _ = promotion_cleanup.finish_now().await;
                        promotion_cleanup.disarm();
                        let _ = result_tx.send(Err(err));
                        return;
                    }
                    if Self::pending_live_first_turn_is_still_deferred(&service, &session_id).await
                    {
                        promotion_cleanup.mark_materialized();
                        Self::finish_pending_promotion_after_service_turn(
                            Arc::clone(&service),
                            staged_sessions,
                            &session_id,
                            "create_session",
                        )
                        .await;
                    } else {
                        let materialized_after_error =
                            service.has_live_session(&session_id).await.unwrap_or(false);
                        if materialized_after_error {
                            promotion_cleanup.mark_materialized();
                            Self::finish_pending_promotion_after_service_turn(
                                Arc::clone(&service),
                                staged_sessions,
                                &session_id,
                                "create_session",
                            )
                            .await;
                        } else {
                            if let Err(error) = promotion_cleanup
                                .replenish_staged_capacity_admission(&service)
                                .await
                            {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to replenish staged capacity after create_session error"
                                );
                            }
                            promotion_cleanup.restore_now().await;
                        }
                    }
                    promotion_cleanup.disarm();
                    let _ = result_tx.send(Err(err));
                    return;
                }
            }

            #[cfg(test)]
            Self::wait_pending_promotion_pre_turn_hook(&pending_promotion_pre_turn_hook).await;

            let (turn_result_tx, turn_result_rx) = tokio::sync::oneshot::channel();
            let service_for_turn = Arc::clone(&service);
            let session_for_turn = session_id.clone();
            tokio::spawn(async move {
                let result = service_for_turn
                    .apply_runtime_turn(
                        &session_for_turn,
                        run_id,
                        req,
                        boundary,
                        contributing_input_ids,
                    )
                    .await;
                let _ = turn_result_tx.send(result);
            });

            #[cfg(feature = "comms")]
            {
                // W2-G: never reconfigure a mob-owned drain during runtime
                // materialization. See `start_turn_via_runtime`.
                let owner = runtime_adapter.peer_ingress_owner(&session_id).await;
                if !owner.is_mob_owned() {
                    let comms_rt = service.comms_runtime(&session_id).await;
                    let peer_ingress_enabled =
                        keep_alive || runtime_adapter.session_has_comms(&session_id).await;
                    runtime_adapter
                        .update_peer_ingress_context(&session_id, peer_ingress_enabled, comms_rt)
                        .await;
                }
            }

            let result = turn_result_rx.await.unwrap_or_else(|_| {
                Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "session service apply_runtime_turn task ended before reporting a result for {session_id}"
                    )),
                ))
            });
            if Self::should_restore_pending_after_apply_runtime_turn(&service, &session_id, &result)
                .await
            {
                let restore_result = promotion_cleanup
                    .restore_after_materialized_failure(
                        &service,
                        MachineSessionArchiveProtocol::from_machine(runtime_adapter.as_ref()),
                    )
                    .await;
                promotion_cleanup.disarm();
                let _ = result_tx.send(match restore_result {
                    Ok(()) => result,
                    Err(error) => Err(error),
                });
                return;
            }

            Self::finish_pending_promotion_after_service_turn(
                Arc::clone(&service),
                staged_sessions,
                &session_id,
                "apply_runtime_turn",
            )
            .await;
            promotion_cleanup.disarm();
            let _ = result_tx.send(result);
        });
        result_rx
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_recovered_create_and_apply_runtime_turn_with_admission_guard(
        &self,
        session_id: SessionId,
        create_req: CreateSessionRequest,
        runtime_was_registered: bool,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
        admission: ActiveCapacityGuard,
        keep_alive: bool,
    ) -> ServiceApplyRuntimeTurnResultReceiver {
        #[cfg(not(feature = "comms"))]
        let _ = keep_alive;
        let service = Arc::clone(&self.service);
        let runtime_adapter = Arc::clone(&self.runtime_adapter);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = match service
                .create_session_with_reserved_admission(create_req, admission)
                .await
            {
                Ok(_) => {
                    let (turn_result_tx, turn_result_rx) = tokio::sync::oneshot::channel();
                    let service_for_turn = Arc::clone(&service);
                    let session_for_turn = session_id.clone();
                    tokio::spawn(async move {
                        let result = service_for_turn
                            .apply_runtime_turn(
                                &session_for_turn,
                                run_id,
                                req,
                                boundary,
                                contributing_input_ids,
                            )
                            .await;
                        let _ = turn_result_tx.send(result);
                    });

                    #[cfg(feature = "comms")]
                    {
                        // W2-G: never reconfigure a mob-owned drain during recovery
                        // materialization. See `start_turn_via_runtime`.
                        let owner = runtime_adapter.peer_ingress_owner(&session_id).await;
                        if !owner.is_mob_owned() {
                            let comms_rt = service.comms_runtime(&session_id).await;
                            let peer_ingress_enabled =
                                keep_alive || runtime_adapter.session_has_comms(&session_id).await;
                            runtime_adapter
                                .update_peer_ingress_context(
                                    &session_id,
                                    peer_ingress_enabled,
                                    comms_rt,
                                )
                                .await;
                        }
                    }

                    let result = turn_result_rx.await.unwrap_or_else(|_| {
                        Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "session service apply_runtime_turn task ended before reporting a result for {session_id}"
                            )),
                        ))
                    });
                    if Self::should_restore_pending_after_apply_runtime_turn(
                        &service,
                        &session_id,
                        &result,
                    )
                    .await
                    {
                        let _ = service.discard_live_session(&session_id).await;
                        runtime_adapter.unregister_session(&session_id).await;
                    }
                    result
                }
                Err(err) => {
                    if !runtime_was_registered {
                        runtime_adapter.unregister_session(&session_id).await;
                    }
                    Err(err)
                }
            };
            let _ = result_tx.send(result);
        });
        result_rx
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_recovered_create_and_apply_runtime_context_appends_with_admission_guard(
        &self,
        session_id: SessionId,
        create_req: CreateSessionRequest,
        runtime_was_registered: bool,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
        admission: ActiveCapacityGuard,
    ) -> ServiceApplyRuntimeTurnResultReceiver {
        let service = Arc::clone(&self.service);
        let runtime_adapter = Arc::clone(&self.runtime_adapter);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = match service
                .create_session_with_reserved_admission(create_req, admission)
                .await
            {
                Ok(_) => {
                    service
                        .apply_runtime_context_appends_with_boundary(
                            &session_id,
                            run_id,
                            appends,
                            boundary,
                            contributing_input_ids,
                        )
                        .await
                }
                Err(err) => {
                    if !runtime_was_registered {
                        runtime_adapter.unregister_session(&session_id).await;
                    }
                    Err(err)
                }
            };
            let _ = result_tx.send(result);
        });
        result_rx
    }

    async fn await_service_apply_runtime_turn(
        session_id: &SessionId,
        result_rx: ServiceApplyRuntimeTurnResultReceiver,
    ) -> Result<CoreApplyOutput, RpcError> {
        result_rx
            .await
            .map_err(|_| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!(
                    "session service apply_runtime_turn task ended before reporting a result for {session_id}"
                ),
                data: None,
            })?
            .map_err(session_error_to_rpc)
    }

    async fn await_service_apply_runtime_turn_with_recoverable_admission(
        session_id: &SessionId,
        result_rx: RecoverableServiceApplyRuntimeTurnResultReceiver,
    ) -> Result<
        (
            Result<CoreApplyOutput, SessionError>,
            Option<ActiveCapacityGuard>,
        ),
        RpcError,
    > {
        result_rx.await.map_err(|_| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!(
                "session service apply_runtime_turn task ended before reporting a result for {session_id}"
            ),
            data: None,
        })
    }

    async fn finish_pending_promotion_after_service_turn(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        staged_sessions: Arc<StagedSessionRegistry>,
        session_id: &SessionId,
        operation: &'static str,
    ) {
        let Some((starting_system_context_state, current_system_context_state)) = staged_sessions
            .take_promoting_system_context_state(session_id)
            .await
        else {
            return;
        };
        if let Err(err) = Self::replay_promoted_system_context_on_service(
            service,
            session_id,
            &starting_system_context_state,
            &current_system_context_state,
        )
        .await
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err.message,
                operation,
                "failed to replay promoted system-context state after service turn"
            );
        }
    }

    /// Persistent TokenStore used by OAuth-backed bindings (shared with
    /// the AgentFactory so login writes + resolve reads see the same
    /// credentials).
    pub fn token_store(&self) -> Option<Arc<dyn meerkat_providers::auth_store::TokenStore>> {
        self.factory.token_store.clone()
    }

    pub fn auth_lease_handle(&self) -> Arc<dyn meerkat_core::handles::AuthLeaseHandle> {
        self.runtime_adapter.auth_lease_handle()
    }

    pub fn oauth_flow_authority(
        &self,
    ) -> Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority> {
        self.runtime_adapter.oauth_flow_authority()
    }

    /// Override the shared default LLM client used by this runtime.
    pub fn set_default_llm_client(&mut self, client: Option<Arc<dyn LlmClient>>) {
        *self
            .default_llm_client
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = client;
    }

    /// Set the provider-agnostic wrapper applied to every agent LLM client.
    pub fn set_agent_llm_client_decorator(
        &mut self,
        decorator: Option<meerkat_core::AgentLlmClientDecorator>,
    ) {
        *self
            .agent_llm_client_decorator
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = decorator.clone();
        *self
            .builder_agent_llm_client_decorator_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = decorator;
    }

    /// Build the runtime adapter appropriate for this runtime's persistence mode.
    pub fn runtime_adapter(&self) -> Arc<MeerkatMachine> {
        self.runtime_adapter.clone()
    }

    pub(crate) fn core_session_service(&self) -> Arc<dyn SessionService> {
        self.service.clone()
    }

    pub(crate) async fn interrupt_live_with_machine_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.service
            .interrupt_with_machine_authority(
                session_id,
                self.runtime_adapter.session_control_authority(),
            )
            .await
    }

    pub(crate) async fn cancel_after_boundary_live_with_machine_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.service
            .cancel_after_boundary_with_machine_authority(
                session_id,
                self.runtime_adapter.session_control_authority(),
            )
            .await
    }

    fn archive_runtime_cleanup(&self) -> ArchiveRuntimeCleanup {
        ArchiveRuntimeCleanup {
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            pending_session_event_streams: Some(Arc::clone(&self.pending_session_event_streams)),
            #[cfg(feature = "mcp")]
            mcp_sessions: Some(Arc::clone(&self.mcp_sessions)),
            #[cfg(feature = "mob")]
            mob_state: self.mob_state(),
        }
    }

    async fn cleanup_recovered_runtime_if_new(
        &self,
        session_id: &SessionId,
        runtime_was_registered: bool,
    ) {
        if !runtime_was_registered {
            let _ = self.archive_runtime_cleanup().run(session_id).await;
        }
    }

    pub fn approval_service(&self) -> meerkat_core::ApprovalService {
        self.approval_service.clone()
    }

    async fn recover_live_session_for_realtime_open(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        if self.service.has_live_session(session_id).await? {
            return Ok(());
        }

        let session = self
            .load_persisted_session(session_id)
            .await
            .map_err(|err| rpc_error_to_session_error(err, session_id))?
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.clone(),
            })?;
        let keep_alive = session
            .session_metadata()
            .map(|metadata| metadata.keep_alive)
            .unwrap_or(false);
        let recovery_overrides = self
            .recovery_overrides_from_turn(None, keep_alive)
            .map_err(|err| rpc_error_to_session_error(err, session_id))?;
        let recovered = self
            .recovered_create_request_with_runtime_binding_mode(
                session_id,
                session,
                recovery_overrides,
                RecoveryRuntimeBindingMode::LocalResources,
            )
            .await
            .map_err(|err| rpc_error_to_session_error(err, session_id))?;
        let runtime_was_registered = recovered.runtime_was_registered;
        let admission = self.service.reserve_create_session_admission().await?;
        if let Err(error) = self
            .service
            .create_session_with_reserved_admission(recovered.request, admission)
            .await
        {
            self.cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                .await;
            return Err(error);
        }

        Ok(())
    }

    /// Project the owning live session into the provider-backed realtime open seam.
    pub async fn realtime_session_open_config(
        &self,
        session_id: &SessionId,
        turning_mode: meerkat_contracts::RealtimeTurningMode,
    ) -> Result<RealtimeSessionOpenConfig, SessionError> {
        self.recover_live_session_for_realtime_open(session_id)
            .await?;
        let session = match self
            .service
            .export_realtime_open_session_snapshot(session_id)
            .await
        {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => {
                self.recover_live_session_for_realtime_open(session_id)
                    .await?;
                self.service
                    .export_realtime_open_session_snapshot(session_id)
                    .await?
            }
            Err(error) => return Err(error),
        };
        let llm_identity = self.service.live_session_llm_identity(session_id).await?;
        let visible_tools = self.service.live_visible_tool_defs(session_id).await?;
        Ok(RealtimeSessionOpenConfig::new(
            turning_mode,
            llm_identity,
            visible_tools,
            realtime_projection_messages(&session),
        )
        .with_runtime_system_context(realtime_projection_runtime_system_context(&session)))
    }

    fn recovery_overrides_from_turn(
        &self,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
        keep_alive: bool,
    ) -> Result<SurfaceSessionRecoveryOverrides, RpcError> {
        let output_schema = match overrides.and_then(|ov| ov.output_schema.clone()) {
            Some(value) => Some(meerkat_core::OutputSchema::from_json_value(value).map_err(
                |e| RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!("Invalid output_schema override: {e}"),
                    data: None,
                },
            )?),
            None => None,
        };

        Ok(SurfaceSessionRecoveryOverrides {
            model: overrides.and_then(|ov| ov.model.clone()),
            provider: overrides
                .and_then(|ov| ov.provider.as_ref())
                .map(|provider| parse_provider_override(provider))
                .transpose()
                .map_err(|message| RpcError {
                    code: error::INVALID_PARAMS,
                    message,
                    data: None,
                })?,
            provider_params: overrides.and_then(|ov| ov.provider_params.clone()),
            clear_provider_params: overrides.is_some_and(|ov| ov.clear_provider_params),
            auth_binding: overrides.and_then(|ov| ov.auth_binding.clone()),
            clear_auth_binding: overrides.is_some_and(|ov| ov.clear_auth_binding),
            max_tokens: overrides.and_then(|ov| ov.max_tokens),
            system_prompt: overrides.and_then(|ov| ov.system_prompt.clone()),
            output_schema,
            structured_output_retries: overrides.and_then(|ov| ov.structured_output_retries),
            keep_alive: Some(keep_alive),
            ..Default::default()
        })
    }

    fn recovery_external_tools(&self) -> Option<Arc<dyn meerkat_core::AgentToolDispatcher>> {
        let tx = self.callback_request_tx()?;
        Some(
            Arc::new(crate::callback_dispatcher::CallbackToolDispatcher::new(
                self.registered_tools(),
                tx,
                self.callback_id_counter(),
                vec![],
            )) as Arc<dyn meerkat_core::AgentToolDispatcher>,
        )
    }

    fn recovery_error_to_rpc(error: SurfaceSessionRecoveryError) -> RpcError {
        match error {
            SurfaceSessionRecoveryError::InvalidOverride(message) => RpcError {
                code: error::INVALID_PARAMS,
                message,
                data: None,
            },
            other => RpcError {
                code: error::INTERNAL_ERROR,
                message: other.to_string(),
                data: None,
            },
        }
    }

    async fn recovered_create_request(
        &self,
        session_id: &SessionId,
        session: Session,
        overrides: SurfaceSessionRecoveryOverrides,
    ) -> Result<RecoveredCreateRequest, RpcError> {
        self.recovered_create_request_with_runtime_binding_mode(
            session_id,
            session,
            overrides,
            RecoveryRuntimeBindingMode::Authoritative,
        )
        .await
    }

    async fn recovered_create_request_with_runtime_binding_mode(
        &self,
        session_id: &SessionId,
        session: Session,
        overrides: SurfaceSessionRecoveryOverrides,
        binding_mode: RecoveryRuntimeBindingMode,
    ) -> Result<RecoveredCreateRequest, RpcError> {
        let current_generation = match self.config_runtime() {
            Some(runtime) => runtime.get().await.ok().map(|snapshot| snapshot.generation),
            None => None,
        };
        let runtime_was_registered = self.runtime_adapter.contains_session(session_id).await;
        let bindings = match binding_mode {
            RecoveryRuntimeBindingMode::Authoritative => {
                self.runtime_adapter
                    .prepare_bindings(session_id.clone())
                    .await
            }
            RecoveryRuntimeBindingMode::LocalResources => {
                self.runtime_adapter
                    .prepare_local_session_bindings(session_id.clone())
                    .await
            }
        }
        .map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("failed to prepare runtime bindings for session {session_id}: {e}"),
            data: None,
        })?;
        let agent_llm_client_decorator = {
            self.agent_llm_client_decorator
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let recovered = match build_recovered_session(
            session,
            &overrides,
            SurfaceSessionRecoveryContext {
                llm_client_override: self
                    .default_llm_client()
                    .map(encode_llm_client_override_for_service),
                agent_llm_client_decorator,
                external_tools: self.recovery_external_tools(),
                checkpointer: None,
                runtime_build_mode: Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)),
                require_runtime_build_mode: true,
                realm_id: self.realm_id.as_ref().map(ToString::to_string),
                instance_id: self.instance_id.clone(),
                backend: self.backend.clone(),
                config_generation: current_generation,
            },
        ) {
            Ok(recovered) => recovered,
            Err(error) => {
                if !runtime_was_registered {
                    self.runtime_adapter.unregister_session(session_id).await;
                }
                return Err(Self::recovery_error_to_rpc(error));
            }
        };
        Ok(RecoveredCreateRequest {
            request: recovered.into_deferred_create_request(),
            runtime_was_registered,
        })
    }

    pub fn schedule_service(&self) -> ScheduleService {
        self.schedule_service.clone()
    }

    pub fn blob_store(&self) -> Arc<dyn meerkat_core::BlobStore> {
        self.service.blob_store()
    }

    pub fn artifact_store(&self) -> Arc<dyn meerkat_core::ArtifactStore> {
        self.artifact_store.clone()
    }

    pub fn default_llm_client(&self) -> Option<Arc<dyn LlmClient>> {
        self.default_llm_client
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn model_registry(&self) -> Result<meerkat_core::ModelRegistry, RpcError> {
        let config = if let Some(runtime) = self.config_runtime() {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("Failed to load config: {e}"),
                    data: None,
                })?
        } else {
            meerkat_core::Config::default()
        };

        config.model_registry().map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("Failed to resolve model registry: {e}"),
            data: None,
        })
    }

    async fn wire_resolved_capabilities_for_identity(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Option<meerkat_contracts::WireResolvedModelCapabilities> {
        let registry = self.model_registry().await.ok()?;
        let profile = registry.profile_for_provider(identity.provider, &identity.model)?;
        Some(profile_to_capability_surface(&profile).to_wire_resolved())
    }

    async fn wire_resolved_capabilities_for_session(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Option<meerkat_contracts::WireResolvedModelCapabilities> {
        if let Ok(Some(surface)) = self
            .runtime_adapter
            .resolved_session_llm_capabilities(session_id)
            .await
        {
            return Some(surface.to_wire_resolved());
        }

        self.wire_resolved_capabilities_for_identity(identity).await
    }

    async fn attach_resolved_capabilities_to_wire_info(
        &self,
        info: &mut meerkat_contracts::WireSessionInfo,
    ) {
        let provider = meerkat_core::Provider::parse_strict(&info.provider)
            .unwrap_or(meerkat_core::Provider::Other);
        let identity = SessionLlmIdentity {
            model: info.model.clone(),
            provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        info.resolved_capabilities = self
            .wire_resolved_capabilities_for_session(&info.session_id, &identity)
            .await;
    }

    async fn llm_identity_from_pending_build(
        &self,
        build_config: &AgentBuildConfig,
    ) -> Result<SessionLlmIdentity, RpcError> {
        let registry = self.model_registry().await?;
        let model = build_config.model.clone();
        let provider = if let Some(provider) = build_config.provider {
            if let Some(reason) =
                registered_model_provider_mismatch_reason(&registry, provider, &model)
            {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: reason,
                    data: None,
                });
            }
            provider
        } else {
            registry
                .entry(&model)
                .map(|entry| entry.provider)
                .unwrap_or(meerkat_core::Provider::Other)
        };
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if let Some(server_id) = build_config.self_hosted_server_id.clone() {
                Some(server_id)
            } else if let Some(metadata) = build_config
                .resume_session
                .as_ref()
                .and_then(Session::session_metadata)
            {
                metadata.self_hosted_server_id
            } else if let Some(entry) =
                registry.entry_for_provider(meerkat_core::Provider::SelfHosted, &model)
            {
                entry
                    .self_hosted
                    .as_ref()
                    .map(|server| server.server_id.clone())
            } else {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "self-hosted provider requires a registered model alias; '{model}' is not configured"
                    ),
                    data: None,
                });
            }
        } else {
            None
        };
        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params: build_config.provider_params.clone(),
            auth_binding: build_config.auth_binding.clone(),
        })
    }

    async fn provider_supports_inline_video(&self, identity: &SessionLlmIdentity) -> bool {
        self.require_inline_video_support(identity).await.is_ok()
    }

    async fn require_inline_video_support(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<(), meerkat_core::UnsupportedModelCapabilityEvidence> {
        let Ok(registry) = self.model_registry().await else {
            return Err(
                meerkat_core::UnsupportedModelCapabilityEvidence::inline_video(
                    identity.provider,
                    identity.model.clone(),
                    meerkat_core::UnsupportedModelCapabilityReason::CapabilityRegistryUnavailable,
                ),
            );
        };

        registry.require_inline_video_for_provider(identity.provider, &identity.model)
    }

    async fn validate_prompt_video_input(
        &self,
        prompt: &ContentInput,
        identity: &SessionLlmIdentity,
    ) -> Result<(), RpcError> {
        let blocks = match prompt {
            ContentInput::Text(_) => return Ok(()),
            ContentInput::Blocks(blocks) => blocks,
        };

        meerkat_core::validate_inline_video_blocks(blocks).map_err(|message| RpcError {
            code: error::INVALID_PARAMS,
            message,
            data: None,
        })?;

        if meerkat_core::has_video(blocks)
            && let Err(evidence) = self.require_inline_video_support(identity).await
        {
            return Err(unsupported_model_capability_rpc_error(evidence));
        }

        Ok(())
    }

    async fn resolve_target_llm_identity(
        &self,
        current: &SessionLlmIdentity,
        ov: &crate::handlers::turn::TurnOverrides,
    ) -> Result<SessionLlmIdentity, RpcError> {
        if ov.provider.is_some() && ov.model.is_none() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "provider override requires model on an existing session".to_string(),
                data: None,
            });
        }
        if ov.clear_provider_params && ov.provider_params.is_some() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "clear_provider_params cannot be combined with provider_params"
                    .to_string(),
                data: None,
            });
        }
        if ov.clear_auth_binding && ov.auth_binding.is_some() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "clear_auth_binding cannot be combined with auth_binding".to_string(),
                data: None,
            });
        }

        let registry = self.model_registry().await?;
        let model = ov.model.clone().unwrap_or_else(|| current.model.clone());
        let provider = if let Some(provider_name) = ov.provider.as_ref() {
            parse_provider_override(provider_name).map_err(|message| RpcError {
                code: error::INVALID_PARAMS,
                message,
                data: None,
            })?
        } else {
            current.provider
        };
        if (ov.model.is_some() || ov.provider.is_some())
            && let Some(reason) =
                registered_model_provider_mismatch_reason(&registry, provider, &model)
        {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: reason,
                data: None,
            });
        }
        let provider_params = if ov.clear_provider_params {
            None
        } else {
            ov.provider_params
                .clone()
                .or_else(|| current.provider_params.clone())
        };
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if ov.model.is_none() {
                current.self_hosted_server_id.clone().or_else(|| {
                    registry
                        .entry_for_provider(meerkat_core::Provider::SelfHosted, &model)
                        .and_then(|entry| entry.self_hosted.as_ref())
                        .map(|server| server.server_id.clone())
                })
            } else {
                match registry.entry_for_provider(meerkat_core::Provider::SelfHosted, &model) {
                    Some(entry) => entry
                        .self_hosted
                        .as_ref()
                        .map(|server| server.server_id.clone()),
                    None => {
                        return Err(RpcError {
                            code: error::INVALID_PARAMS,
                            message: format!(
                                "self-hosted provider requires a registered model alias; '{model}' is not configured"
                            ),
                            data: None,
                        });
                    }
                }
            }
        } else {
            None
        };

        let auth_binding = if ov.clear_auth_binding {
            None
        } else {
            ov.auth_binding
                .clone()
                .or_else(|| current.auth_binding.clone())
        };

        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params,
            auth_binding,
        })
    }

    fn apply_llm_identity_to_build_config(
        build_config: &mut AgentBuildConfig,
        identity: &SessionLlmIdentity,
    ) {
        build_config.model = identity.model.clone();
        build_config.provider = Some(identity.provider);
        build_config.self_hosted_server_id = identity.self_hosted_server_id.clone();
        build_config.provider_params = identity.provider_params.clone();
        build_config.auth_binding = identity.auth_binding.clone();
    }

    async fn current_materialized_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, RpcError> {
        match self.service.live_session_llm_identity(session_id).await {
            Ok(identity) => Ok(identity),
            Err(SessionError::NotFound { .. }) => {
                let session = self
                    .load_persisted_session(session_id)
                    .await?
                    .ok_or_else(|| RpcError {
                        code: error::SESSION_NOT_FOUND,
                        message: format!("session not found: {session_id}"),
                        data: None,
                    })?;
                let metadata = session.session_metadata().ok_or_else(|| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!(
                        "session {session_id} is missing durable llm identity metadata"
                    ),
                    data: None,
                })?;
                Ok(metadata.llm_identity())
            }
            Err(err) => Err(session_error_to_rpc(err)),
        }
    }

    async fn effective_llm_identity_for_turn(
        &self,
        session_id: &SessionId,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
    ) -> Result<SessionLlmIdentity, RpcError> {
        let pending_identity = self
            .staged_sessions
            .effective_llm_identity(session_id)
            .await;
        if let Some(pending_identity) = pending_identity {
            return match overrides {
                Some(ov) => {
                    self.resolve_target_llm_identity(&pending_identity, ov)
                        .await
                }
                None => Ok(pending_identity),
            };
        }

        let current = self.current_materialized_llm_identity(session_id).await?;
        match overrides {
            Some(ov) => self.resolve_target_llm_identity(&current, ov).await,
            None => Ok(current),
        }
    }

    fn llm_reconfigure_host(&self) -> SessionRuntimeLlmReconfigureHost {
        SessionRuntimeLlmReconfigureHost {
            service: Arc::clone(&self.service),
            staged_sessions: Arc::clone(&self.staged_sessions),
            factory: self.factory.clone(),
            auth_lease: self.runtime_adapter.auth_lease_handle(),
            default_llm_client: Arc::clone(&self.default_llm_client),
            agent_llm_client_decorator: Arc::clone(&self.agent_llm_client_decorator),
            config_runtime: Arc::clone(&self.config_runtime),
        }
    }

    fn committed_visibility_allows(
        base_tool_names: &std::collections::BTreeSet<String>,
        visibility_state: &meerkat_core::SessionToolVisibilityState,
        tool_name: &str,
    ) -> bool {
        if !base_tool_names.contains(tool_name) {
            return false;
        }

        meerkat_core::ToolScope::compose(&[
            visibility_state.capability_base_filter.clone(),
            visibility_state.inherited_base_filter.clone(),
            visibility_state.active_filter.clone(),
        ])
        .allows(tool_name)
    }

    fn derive_reconfigured_visibility_state(
        current: &meerkat_core::SessionToolVisibilityState,
        target_capability_surface: &SessionLlmCapabilitySurface,
        base_tool_names: &std::collections::BTreeSet<String>,
    ) -> meerkat_core::SessionToolVisibilityState {
        let current_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            current,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );

        let mut next = current.clone();
        next.capability_base_filter = meerkat_core::capability_base_filter_for_image_tool_results(
            target_capability_surface.image_tool_results,
        );

        let next_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            &next,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );
        if current_view_image_visible != next_view_image_visible {
            next.active_revision = current.active_revision.max(current.staged_revision) + 1;
        }

        next
    }

    async fn rollback_idle_hot_swap_failure(
        &self,
        host: &SessionRuntimeLlmReconfigureHost,
        session_id: &SessionId,
        previous_identity: &SessionLlmIdentity,
        previous_visibility_state: &meerkat_core::SessionToolVisibilityState,
        original_error: RuntimeDriverError,
    ) -> Result<(), RuntimeDriverError> {
        let rollback_result = async {
            host.apply_live_session_llm_identity(session_id, previous_identity)
                .await?;
            host.apply_live_session_tool_visibility_state(
                session_id,
                Some(previous_visibility_state.clone()),
            )
            .await?;
            Ok::<(), RuntimeDriverError>(())
        }
        .await;

        match rollback_result {
            Ok(()) => Err(original_error),
            Err(rollback_error) => {
                let _ = host.discard_live_session(session_id).await;
                Err(RuntimeDriverError::Internal(format!(
                    "failed to rollback idle live llm reconfiguration after error ({original_error}): {rollback_error}"
                )))
            }
        }
    }

    async fn hot_swap_llm_client_on_idle_session(
        &self,
        session_id: &SessionId,
        request: &SessionLlmReconfigureRequest,
    ) -> Result<(), RuntimeDriverError> {
        let host = self.llm_reconfigure_host();
        let hydrated = host.hydrate_session_llm_state(session_id).await?;
        let resolved = host
            .resolve_target_session_llm_identity(request, &hydrated.current_identity)
            .await?;
        let next_visibility_state = Self::derive_reconfigured_visibility_state(
            &hydrated.current_visibility_state,
            &resolved.target_capability_surface,
            &hydrated.base_tool_names,
        );

        host.apply_live_session_llm_identity(session_id, &resolved.target_identity)
            .await?;
        if let Err(error) = host
            .apply_live_session_tool_visibility_state(session_id, Some(next_visibility_state))
            .await
        {
            return self
                .rollback_idle_hot_swap_failure(
                    &host,
                    session_id,
                    &hydrated.current_identity,
                    &hydrated.current_visibility_state,
                    error,
                )
                .await;
        }
        if let Err(error) = host.persist_live_session(session_id).await {
            return self
                .rollback_idle_hot_swap_failure(
                    &host,
                    session_id,
                    &hydrated.current_identity,
                    &hydrated.current_visibility_state,
                    error,
                )
                .await;
        }

        Ok(())
    }

    /// Hot-swap the LLM client on a materialized session.
    ///
    /// The machine owns the semantic transition; RPC only adapts the request.
    async fn hot_swap_llm_client(
        &self,
        session_id: &SessionId,
        ov: &crate::handlers::turn::TurnOverrides,
    ) -> Result<(), RpcError> {
        let request = SessionLlmReconfigureRequest {
            model: ov.model.clone(),
            provider: ov.provider.clone(),
            provider_params: ov.provider_params.clone(),
            clear_provider_params: ov.clear_provider_params,
            auth_binding: ov.auth_binding.clone(),
            clear_auth_binding: ov.clear_auth_binding,
        };

        if !self.runtime_adapter.contains_session(session_id).await
            && self.service.read(session_id).await.is_ok()
        {
            self.runtime_adapter
                .prepare_bindings(session_id.clone())
                .await
                .map_err(|e| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!(
                        "failed to prepare runtime bindings for live session {session_id}: {e}"
                    ),
                    data: None,
                })?;
        }

        match self
            .runtime_adapter
            .reconfigure_session_llm_identity(session_id, request.clone())
            .await
        {
            Ok(_) => Ok(()),
            Err(RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Idle,
            }) => self
                .hot_swap_llm_client_on_idle_session(session_id, &request)
                .await
                .map_err(runtime_driver_error_to_rpc),
            Err(err) => Err(runtime_driver_error_to_rpc(err)),
        }
    }

    pub async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        let result = self.service.discard_live_session(session_id).await;
        if result.is_ok() && !self.staged_sessions.contains(session_id).await {
            self.discard_staged_capacity_admission(session_id);
        }
        result
    }

    async fn discard_stale_live_session(&self, session_id: &SessionId) {
        let _ = self.discard_live_session(session_id).await;
        self.runtime_adapter.unregister_session(session_id).await;
    }

    pub async fn dispatch_external_tool_call(
        &self,
        session_id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        self.dispatch_external_tool_call_with_timeout_policy(
            session_id,
            call,
            meerkat_core::ToolDispatchTimeoutPolicy::Disabled,
        )
        .await
    }

    pub async fn dispatch_external_tool_call_with_timeout_policy(
        &self,
        session_id: &SessionId,
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        self.service
            .dispatch_external_tool_call_with_timeout_policy(session_id, call, timeout_policy)
            .await
    }

    pub async fn append_external_user_content(
        &self,
        session_id: &SessionId,
        content: meerkat_core::types::ContentInput,
    ) -> Result<(), SessionError> {
        self.service
            .append_external_user_content(session_id, content)
            .await
    }

    pub async fn append_external_assistant_output(
        &self,
        session_id: &SessionId,
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: meerkat_core::types::Usage,
    ) -> Result<(), SessionError> {
        self.service
            .append_external_assistant_output(session_id, blocks, stop_reason, usage)
            .await
    }

    pub async fn append_realtime_transcript_event(
        &self,
        session_id: &SessionId,
        event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, SessionError> {
        self.service
            .append_realtime_transcript_event(session_id, event)
            .await
    }

    #[cfg(feature = "mob")]
    pub fn session_service(&self) -> Arc<dyn meerkat_mob::MobSessionService> {
        Arc::new(RpcMobSessionService::new(
            Arc::clone(&self.service),
            Arc::clone(&self.staged_sessions),
            Arc::clone(&self.runtime_adapter),
            Arc::clone(&self.mob_state),
        ))
    }

    /// Apply runtime-owned system context appends to the canonical session
    /// transcript through the persistent session service. Used by the
    /// CoreExecutor context-only-immediate short-circuit so peer terminal
    /// responses (and other context-only primitives) land as system-context
    /// blocks rather than triggering a turn.
    pub(crate) async fn apply_runtime_context_appends_via_service(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::lifecycle::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::lifecycle::InputId>,
        pre_admission: Option<RuntimePreAdmission>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        self.apply_runtime_context_appends_with_recovery(
            session_id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            None,
            pre_admission,
        )
        .await
        .map_err(|err| rpc_error_to_session_error(err, session_id))
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_runtime_context_appends_with_recovery(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::lifecycle::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::lifecycle::InputId>,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
        pre_admission: Option<RuntimePreAdmission>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, crate::protocol::RpcError>
    {
        if self
            .archived_persisted_session_without_live(session_id)
            .await?
        {
            return Err(Self::session_not_found_rpc(session_id));
        }
        if self.live_session_is_stale(session_id).await? {
            self.discard_stale_live_session(session_id).await;
        }
        let admission = match pre_admission {
            Some(admission) => admission.into_admission(),
            None => self.reserve_active_turn(session_id).await?.into_admission(),
        };
        let result = self
            .service
            .apply_runtime_context_appends_with_recoverable_reserved_admission(
                session_id,
                run_id.clone(),
                appends.clone(),
                boundary,
                contributing_input_ids.clone(),
                admission,
            )
            .await;
        let admission = match result {
            Ok(output) => return Ok(output),
            Err((SessionError::NotFound { .. }, Some(admission)))
                if !self.staged_sessions.contains(session_id).await =>
            {
                // Preserve the accepted-input admission for recovery. Dropping
                // it here would make recovery re-admit against global capacity
                // even though this input has already been accepted.
                admission
            }
            Err((error @ SessionError::NotFound { .. }, Some(admission)))
                if self.staged_sessions.contains(session_id).await =>
            {
                restore_staged_capacity_admission(
                    &self.staged_capacity_admissions,
                    session_id.clone(),
                    admission,
                );
                return Err(session_error_to_rpc(error));
            }
            Err((error, admission)) => {
                drop(admission);
                return Err(session_error_to_rpc(error));
            }
        };

        let stored_session = self
            .load_persisted_session(session_id)
            .await?
            .ok_or_else(|| Self::session_not_found_rpc(session_id))?;
        let keep_alive = stored_session
            .session_metadata()
            .map(|metadata| metadata.keep_alive)
            .unwrap_or(false);
        let recovery_overrides = self.recovery_overrides_from_turn(overrides, keep_alive)?;
        let recovered_create = self
            .recovered_create_request(session_id, stored_session, recovery_overrides)
            .await?;
        let result_rx = self
            .spawn_recovered_create_and_apply_runtime_context_appends_with_admission_guard(
                session_id.clone(),
                recovered_create.request,
                recovered_create.runtime_was_registered,
                run_id,
                appends,
                boundary,
                contributing_input_ids,
                admission,
            );
        Self::await_service_apply_runtime_turn(session_id, result_rx).await
    }

    /// Pre-initialize the callback channel and return the receiver half.
    ///
    /// Call this before any code that reads `callback_request_tx()` (e.g.
    /// mob resume that invokes an `ExternalToolsProvider`). The server
    /// constructor that accepts a pre-created rx will reuse this channel
    /// instead of creating a new one.
    pub fn init_callback_channel(
        &self,
    ) -> mpsc::Receiver<(
        crate::protocol::RpcRequest,
        tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
    )> {
        let (tx, rx) = mpsc::channel(crate::NOTIFICATION_CHANNEL_CAPACITY);
        let id_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let registered_tools = Arc::new(StdRwLock::new(Vec::new()));
        self.set_callback_channel(tx, id_counter, registered_tools);
        rx
    }

    /// Set the callback request channel for tool callbacks.
    ///
    /// Takes `&self` so it can be called after the runtime is wrapped in `Arc`
    /// (e.g. during `RpcServer` construction).
    pub fn set_callback_channel(
        &self,
        tx: mpsc::Sender<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
        id_counter: Arc<std::sync::atomic::AtomicU64>,
        registered_tools: Arc<StdRwLock<Vec<meerkat_core::ToolDef>>>,
    ) {
        if let Ok(mut slot) = self.callback_request_tx.write() {
            *slot = Some(tx);
        }
        // id_counter and registered_tools are already Arc-shared — the server
        // passes its own Arcs into the runtime at construction time. When using
        // the &self path (after Arc wrapping), we store them atomically.
        if let Ok(mut c) = self.callback_id_counter_slot.write() {
            *c = id_counter;
        }
        if let Ok(mut t) = self.registered_tools_slot.write() {
            *t = registered_tools;
        }
    }

    /// Get a clone of the callback request sender, if configured.
    pub fn callback_request_tx(
        &self,
    ) -> Option<
        mpsc::Sender<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
    > {
        self.callback_request_tx
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Get the callback ID counter.
    pub fn callback_id_counter(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.callback_id_counter_slot
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Get the globally registered callback tool definitions.
    pub fn registered_tools(&self) -> Arc<StdRwLock<Vec<meerkat_core::ToolDef>>> {
        self.registered_tools_slot
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_else(|| Arc::new(StdRwLock::new(Vec::new())))
    }

    #[cfg(feature = "mob")]
    pub fn set_mob_state(&self, mob_state: Arc<meerkat_mob_mcp::MobMcpState>) {
        if let Ok(mut slot) = self.mob_state.write() {
            *slot = Some(mob_state);
        }
    }

    #[cfg(feature = "mob")]
    pub fn mob_state(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        self.mob_state.read().ok().and_then(|slot| slot.clone())
    }

    /// Replace the notification sink used by lazily-created session executors.
    ///
    /// Called by `serve_tcp_connection` when a new TCP client connects — each
    /// connection has its own transport writer so the sink must be updated to
    /// route events to the currently-connected client.
    /// Read the current notification sink (for use by executors at apply time).
    pub fn current_notification_sink(&self) -> crate::router::NotificationSink {
        self.notification_sink
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_notification_sink(&self, sink: crate::router::NotificationSink) {
        if let Ok(mut slot) = self.notification_sink.write() {
            *slot = sink;
        }
    }

    /// Wire an external comms runtime as the peer-ingress source for a session.
    ///
    /// This enables the session to receive incoming comms messages (peer
    /// requests/responses) between turns. The session must already be
    /// registered via `create_session` (which calls `prepare_bindings`).
    ///
    /// The comms drain starts once an executor is attached (i.e., on the first
    /// `turn/start`). Calling this before the first turn is safe — the drain
    /// context is stored and will be reconciled when the executor materializes.
    ///
    /// Used by the kennel hive agent where the comms runtime is shared at the
    /// factory level (not per-session via `comms_name`).
    #[cfg(feature = "comms")]
    pub async fn enable_comms_drain(
        self: &Arc<Self>,
        session_id: &meerkat_core::types::SessionId,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) {
        // Only store the comms context — don't create an executor yet.
        // The executor is created lazily on first turn/start, and it needs
        // the per-connection notification sink (not the startup noop sink).
        self.runtime_adapter
            .update_peer_ingress_context(session_id, true, Some(comms_runtime))
            .await;
    }

    /// Wire an external comms runtime and eagerly attach the runtime executor.
    ///
    /// Long-lived autonomous hosts, such as remote TUX targets, must drain
    /// incoming peer requests even when no user-driven `turn/start` is
    /// currently active. This keeps peer ingress on the same runtime surface
    /// used by RPC/TUX turns instead of relying on a separate helper surface.
    #[cfg(feature = "comms")]
    pub async fn enable_autonomous_comms_drain(
        self: &Arc<Self>,
        session_id: &meerkat_core::types::SessionId,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), RpcError> {
        self.ensure_runtime_executor(session_id).await?;
        self.runtime_adapter
            .update_peer_ingress_context(session_id, true, Some(comms_runtime))
            .await;
        Ok(())
    }

    pub fn set_skill_identity_registry(&self, registry: SourceIdentityRegistry) {
        if let Ok(mut slot) = self.skill_identity_registry.write() {
            slot.registry = registry;
        }
    }

    pub fn set_skill_identity_registry_for_generation(
        &self,
        generation: u64,
        registry: SourceIdentityRegistry,
    ) {
        if let Ok(mut slot) = self.skill_identity_registry.write()
            && generation >= slot.generation
        {
            slot.generation = generation;
            slot.registry = registry;
        }
    }

    pub(crate) async fn ensure_runtime_executor(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<(), RpcError> {
        self.ensure_runtime_executor_on_adapter(&self.runtime_adapter, session_id)
            .await
    }

    async fn ensure_runtime_executor_on_adapter(
        self: &Arc<Self>,
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
    ) -> Result<(), RpcError> {
        if self
            .archived_persisted_session_without_live(session_id)
            .await?
        {
            return Err(Self::session_not_found_rpc(session_id));
        }
        // Check for a live executor (not just registration). Sessions
        // registered via `prepare_bindings()` exist in the adapter map but
        // have no RuntimeLoop — inputs would queue without being processed.
        if !adapter.session_has_executor(session_id).await {
            let persisted_session = self.load_persisted_session(session_id).await?;
            let persisted_session_exists = persisted_session.is_some();
            let adapter_registration_exists = adapter.contains_session(session_id).await;
            let session_exists = adapter_registration_exists
                || self.staged_sessions.contains(session_id).await
                || self.service.read(session_id).await.is_ok()
                || persisted_session_exists;
            if !session_exists {
                return Err(RpcError {
                    code: error::SESSION_NOT_FOUND,
                    message: format!("session not found: {session_id}"),
                    data: None,
                });
            }
            #[cfg(feature = "mob")]
            let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> =
                match self.mob_state().as_ref() {
                    Some(mob_state)
                        if mob_state.owns_live_bridge_session(session_id).await
                            || mob_state.owns_persisted_bridge_session(session_id).await =>
                    {
                        let sink = self
                            .notification_sink
                            .read()
                            .ok()
                            .map(|slot| slot.clone())
                            .unwrap_or_else(crate::router::NotificationSink::noop);
                        Box::new(crate::session_executor::MobRpcRuntimeExecutor::new(
                            mob_state.session_service(),
                            Some(Arc::clone(self)),
                            session_id.clone(),
                            sink,
                        ))
                    }
                    _ => Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                        Arc::clone(self),
                        session_id.clone(),
                    )),
                };
            #[cfg(not(feature = "mob"))]
            let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> =
                Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                    Arc::clone(self),
                    session_id.clone(),
                ));
            adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
        }
        Ok(())
    }

    /// W3-H: install the correct runtime executor for the session, picking
    /// between `SessionRuntimeExecutor` (standalone sessions) and
    /// `MobRpcRuntimeExecutor` (mob-owned bridge sessions). Mirrors the
    /// `MethodRouter::ensure_runtime_session_registered` logic — used by the
    /// realtime WS observer after a MobMember channel's binding rotates to a
    /// new bridge session id (no RPC path crosses, so the pre-dispatch
    /// registration gate doesn't fire; we need to register the replacement
    /// session here so the next status poll does not see NotReady/Destroyed).
    ///
    /// Returns `Ok(())` if the session is already registered, newly registered,
    /// or not found (the observer closes the channel separately on terminal
    /// retire, so "not found after rotation" surfaces as a downstream status
    /// error on the next poll tick).
    #[cfg(feature = "mob")]
    pub async fn ensure_runtime_session_for_rotation(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.reject_archived_persisted_session_without_live(session_id)
            .await
            .map_err(|_| meerkat_core::service::SessionError::NotFound {
                id: session_id.clone(),
            })?;
        if self.runtime_adapter.session_has_executor(session_id).await {
            return Ok(());
        }
        let mob_state = self.mob_state();
        let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> = match mob_state.as_ref() {
            Some(mob_state)
                if mob_state.owns_live_bridge_session(session_id).await
                    || mob_state.owns_persisted_bridge_session(session_id).await =>
            {
                let sink = self
                    .notification_sink
                    .read()
                    .ok()
                    .map(|slot| slot.clone())
                    .unwrap_or_else(crate::router::NotificationSink::noop);
                Box::new(crate::session_executor::MobRpcRuntimeExecutor::new(
                    mob_state.session_service(),
                    Some(Arc::clone(self)),
                    session_id.clone(),
                    sink,
                ))
            }
            _ => Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                Arc::clone(self),
                session_id.clone(),
            )),
        };
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        Ok(())
    }

    /// Start a turn by routing through the runtime input/completion waiter path.
    ///
    /// Instead of calling `SessionService::start_turn()` directly, this method:
    /// 1. Creates an `Input::Prompt` from the request parameters
    /// 2. Accepts it via `MeerkatMachine::accept_input_with_completion()`
    /// 3. Awaits the completion handle
    /// 4. Returns the `RunResult` produced by the executor
    ///
    /// This ensures all session-driving work flows through the single runtime
    /// authority (the RuntimeLoop → CoreExecutor pipeline).
    #[allow(clippy::too_many_arguments, unused_variables)]
    pub async fn start_turn_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        prompt: ContentInput,
        mcp_event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        use meerkat_runtime::accept::AcceptOutcome;
        use meerkat_runtime::input::{Input, PromptInput};
        #[allow(unused_mut)]
        let mut prompt = prompt;
        self.reject_archived_persisted_session_without_live(session_id)
            .await?;
        let effective_identity = self
            .effective_llm_identity_for_turn(session_id, overrides.as_ref())
            .await?;
        self.validate_prompt_video_input(&prompt, &effective_identity)
            .await?;

        if self.live_session_is_stale(session_id).await? {
            self.discard_stale_live_session(session_id).await;
        }

        // Reject build-only overrides that cannot be applied via runtime turn
        // metadata before taking active capacity. Silently dropping them would
        // violate the surface contract, and invalid params are not active work.
        if let Some(ref ov) = overrides {
            let rejected = [
                ov.max_tokens.map(|_| "max_tokens"),
                ov.system_prompt.as_ref().map(|_| "system_prompt"),
                ov.output_schema.as_ref().map(|_| "output_schema"),
                ov.structured_output_retries
                    .map(|_| "structured_output_retries"),
            ];
            let rejected: Vec<&str> = rejected.into_iter().flatten().collect();
            if !rejected.is_empty() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "Cannot override {} on a runtime-routed turn; \
                         set these at session/create time or use a deferred session",
                        rejected.join(", ")
                    ),
                    data: None,
                });
            }
        }

        let runtime_registration_lock = self.runtime_registration_lock(session_id);
        let runtime_registration_guard = runtime_registration_lock.mutex().lock().await;
        let runtime_was_registered = self.runtime_adapter.contains_session(session_id).await;
        let staged_session_existed = self.staged_sessions.contains(session_id).await;
        let mut pre_admission =
            RuntimePreAdmissionGuard::new(self.reserve_active_turn(session_id).await?);

        // Apply MCP boundary before building the input — staged MCP ops
        // (from mcp/add, mcp/remove, mcp/reload) must be connected and made
        // visible to the agent before the turn starts.
        #[cfg(feature = "mcp")]
        {
            let mut mcp_text = String::new();
            self.apply_mcp_boundary(session_id, &mcp_event_tx, &mut mcp_text)
                .await?;
            if !mcp_text.is_empty() {
                let mut blocks = prompt.into_blocks();
                blocks.insert(
                    0,
                    meerkat_core::types::ContentBlock::Text { text: mcp_text },
                );
                prompt = ContentInput::Blocks(blocks);
            }
        }

        let turn_metadata = Self::turn_metadata_from_overrides(
            skill_references,
            flow_tool_overlay,
            additional_instructions,
            overrides.as_ref(),
            Some(effective_identity.provider.as_str()),
        );

        let input = Input::Prompt(PromptInput::from_content_input(prompt, turn_metadata));
        let input_id = input.id().clone();

        self.ensure_runtime_executor(session_id).await?;

        // Manage comms drain lifecycle based on keep_alive override.
        #[cfg(feature = "comms")]
        {
            let keep_alive_override = overrides.as_ref().and_then(|ov| ov.keep_alive);
            let pending_keep_alive_override_applied = if let Some(keep_alive) = keep_alive_override
            {
                match self
                    .apply_pending_keep_alive_override(session_id, keep_alive)
                    .await
                {
                    Ok(applied) => applied,
                    Err(err) => {
                        self.unregister_new_runtime_registration_if_idle(
                            &self.runtime_adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            false,
                        )
                        .await;
                        return Err(err);
                    }
                }
            } else {
                false
            };
            let keep_alive = match keep_alive_override {
                Some(val) => val,
                None => match self.load_persisted_session(session_id).await {
                    Ok(session) => session
                        .and_then(|s| s.session_metadata().map(|m| m.keep_alive))
                        .unwrap_or(false),
                    Err(err) => {
                        self.unregister_new_runtime_registration_if_idle(
                            &self.runtime_adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            false,
                        )
                        .await;
                        return Err(err);
                    }
                },
            };
            if !pending_keep_alive_override_applied {
                let comms_rt = self.service.comms_runtime(session_id).await;
                if keep_alive && comms_rt.is_none() {
                    // Check if the runtime adapter already has comms configured
                    // for this session (e.g., via enable_comms_drain). If so,
                    // the session-service comms check is not authoritative.
                    let adapter_has_comms =
                        self.runtime_adapter.session_has_comms(session_id).await;
                    if !adapter_has_comms {
                        self.unregister_new_runtime_registration_if_idle(
                            &self.runtime_adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            false,
                        )
                        .await;
                        return Err(RpcError {
                            code: error::INVALID_PARAMS,
                            message: "keep_alive requires a session created with comms_name"
                                .to_string(),
                            data: None,
                        });
                    }
                }
                // Persist explicit override so subsequent inheriting calls observe it.
                if keep_alive_override.is_some()
                    && let Err(err) = self
                        .service
                        .apply_runtime_session_keep_alive(session_id, keep_alive)
                        .await
                {
                    self.unregister_new_runtime_registration_if_idle(
                        &self.runtime_adapter,
                        session_id,
                        runtime_was_registered,
                        staged_session_existed,
                        false,
                    )
                    .await;
                    return Err(session_error_to_rpc(err));
                }
                // W2-G: never reconfigure a mob-owned drain from the session-runtime
                // turn-start path. The mob provisioner owns peer-ingress for its
                // members; the session runtime has no visibility into the mob's
                // comms context and must not substitute its own (this is the
                // s71 silent-downgrade class). The DSL's
                // `AttachSessionIngress` guard would already reject the swap,
                // but we short-circuit here so the turn-start path doesn't emit
                // a spurious WARN for every turn on every mob-owned member.
                let owner = self.runtime_adapter.peer_ingress_owner(session_id).await;
                if owner.is_mob_owned() {
                    tracing::debug!(
                        %session_id,
                        ?owner,
                        "start_turn_via_runtime: mob-owned peer ingress — skipping drain reconfigure"
                    );
                } else {
                    let peer_ingress_enabled = self
                        .preserve_existing_peer_ingress(session_id, keep_alive)
                        .await;
                    // Preserve an already-active peer ingress channel for
                    // externally-enabled sessions even when the persisted
                    // keep_alive bit is false. Without this, ordinary turn
                    // routing can accidentally tear down the persistent comms
                    // drain that autonomous peers rely on for between-turn
                    // responses.
                    if comms_rt.is_some() || peer_ingress_enabled {
                        self.runtime_adapter
                            .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                            .await;
                    }
                }
            }
        }

        let pre_admission_guard =
            match Self::take_runtime_pre_admission_guard(&mut pre_admission, session_id) {
                Ok(admission) => admission,
                Err(err) => {
                    self.unregister_new_runtime_registration_if_idle(
                        &self.runtime_adapter,
                        session_id,
                        runtime_was_registered,
                        staged_session_existed,
                        false,
                    )
                    .await;
                    return Err(err);
                }
            };
        let pre_admission_registration = match self.register_runtime_pre_admission(
            session_id.clone(),
            input_id.clone(),
            pre_admission_guard,
        ) {
            Ok(registration) => registration,
            Err(err) => {
                self.unregister_new_runtime_registration_if_idle(
                    &self.runtime_adapter,
                    session_id,
                    runtime_was_registered,
                    staged_session_existed,
                    false,
                )
                .await;
                return Err(err);
            }
        };
        let (outcome, handle) = match self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                self.unregister_new_runtime_registration_if_idle(
                    &self.runtime_adapter,
                    session_id,
                    runtime_was_registered,
                    staged_session_existed,
                    false,
                )
                .await;
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("runtime accept failed: {e}"),
                    data: None,
                });
            }
        };
        // Forward events while waiting for completion
        // (Events are forwarded by the executor's forwarder task,
        // which is spawned inside SessionRuntimeExecutor::apply())

        let Some(handle) = handle else {
            self.unregister_new_runtime_registration_if_idle(
                &self.runtime_adapter,
                session_id,
                runtime_was_registered,
                staged_session_existed,
                false,
            )
            .await;
            // Input already terminal (dedup of completed input)
            let existing_id = match outcome {
                AcceptOutcome::Deduplicated { existing_id, .. } => existing_id.to_string(),
                _ => "unknown".to_string(),
            };
            return Err(RpcError {
                code: error::DUPLICATE_INPUT,
                message: "input already processed".to_string(),
                data: Some(serde_json::json!({ "existing_id": existing_id })),
            });
        };

        let completion_rx = self.spawn_runtime_pre_admission_cleanup_with_outcome(
            session_id.clone(),
            input_id,
            handle,
        );
        pre_admission_registration.disarm();
        drop(runtime_registration_guard);
        drop(runtime_registration_lock);
        let outcome = completion_rx.await.unwrap_or_else(|_| {
            meerkat_runtime::CompletionOutcome::RuntimeTerminated(
                "runtime pre-admission cleanup task ended before reporting completion".to_string(),
            )
        });
        completion_outcome_to_rpc_result(outcome, session_id)
    }

    /// Admit a canonical external event through the runtime-backed path.
    pub async fn accept_external_event_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        event_type: String,
        payload: serde_json::Value,
        blocks: Option<Vec<meerkat_contracts::WireContentBlock>>,
    ) -> Result<meerkat_runtime::AcceptOutcome, RpcError> {
        use meerkat_runtime::input::{
            ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
        };

        self.reject_archived_persisted_session_without_live(session_id)
            .await?;

        if event_type.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "event_type cannot be empty".to_string(),
                data: None,
            });
        }

        let blocks = decode_wire_content_blocks(blocks)?;

        let input = Input::ExternalEvent(ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::External {
                    source_name: event_type.clone(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type,
            payload,
            blocks,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let input_id = input.id().clone();

        let runtime_registration_lock = self.runtime_registration_lock(session_id);
        let runtime_registration_guard = runtime_registration_lock.mutex().lock().await;
        let runtime_was_registered = self.runtime_adapter.contains_session(session_id).await;
        let staged_session_existed = self.staged_sessions.contains(session_id).await;
        let runtime_is_already_running = self
            .runtime_adapter
            .runtime_state(session_id)
            .await
            .is_ok_and(|state| matches!(state, meerkat_runtime::RuntimeState::Running));
        let mut pre_admission = if runtime_is_already_running {
            Some(RuntimePreAdmissionGuard::new(
                self.reserve_or_join_active_turn(session_id).await?,
            ))
        } else {
            None
        };
        let cleanup_protects_active_admission = pre_admission.is_none();

        if self.live_session_is_stale(session_id).await? {
            self.discard_stale_live_session(session_id).await;
        }
        self.ensure_runtime_executor(session_id).await?;

        let mut pre_admission_registration = None;
        if let Some(admission) = pre_admission.as_mut() {
            let admission_guard =
                match Self::take_runtime_pre_admission_guard(admission, session_id) {
                    Ok(admission) => admission,
                    Err(err) => {
                        self.unregister_new_runtime_registration_if_idle(
                            &self.runtime_adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            cleanup_protects_active_admission,
                        )
                        .await;
                        return Err(err);
                    }
                };
            pre_admission_registration = Some(
                match self.register_runtime_pre_admission(
                    session_id.clone(),
                    input_id.clone(),
                    admission_guard,
                ) {
                    Ok(registration) => registration,
                    Err(err) => {
                        self.unregister_new_runtime_registration_if_idle(
                            &self.runtime_adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            false,
                        )
                        .await;
                        return Err(err);
                    }
                },
            );
        }

        let result = self
            .runtime_adapter
            .accept_input_without_wake(session_id, input)
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("runtime accept failed: {e}"),
                data: None,
            });
        if result.is_err() {
            self.unregister_new_runtime_registration_if_idle(
                &self.runtime_adapter,
                session_id,
                runtime_was_registered,
                staged_session_existed,
                cleanup_protects_active_admission,
            )
            .await;
        }
        if let Some(registration) = pre_admission_registration {
            let keep_admission_for_active_input =
                matches!(result, Ok(meerkat_runtime::AcceptOutcome::Accepted { .. }))
                    && self
                        .runtime_adapter
                        .list_active_inputs(session_id)
                        .await
                        .is_ok_and(|inputs| inputs.contains(&input_id));
            if keep_admission_for_active_input {
                self.spawn_runtime_pre_admission_idle_cleanup(session_id.clone(), input_id.clone());
                registration.disarm();
            }
        }
        drop(runtime_registration_guard);
        drop(runtime_registration_lock);
        result
    }

    pub(crate) async fn accept_runtime_input_with_active_admission(
        self: &Arc<Self>,
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<meerkat_runtime::AcceptOutcome, RpcError> {
        self.reject_archived_persisted_session_without_live(session_id)
            .await?;
        let input_id = input.id().clone();
        let should_pre_admit = Self::runtime_input_requires_active_pre_admission(&input);
        let cleanup_protects_active_admission = !should_pre_admit;
        let mut pre_admission = if should_pre_admit {
            Some(RuntimePreAdmissionGuard::new(
                self.reserve_or_join_active_turn(session_id).await?,
            ))
        } else {
            None
        };

        if self.live_session_is_stale(session_id).await? {
            self.discard_stale_live_session(session_id).await;
        }

        let runtime_registration_lock = self.runtime_registration_lock(session_id);
        let runtime_registration_guard = runtime_registration_lock.mutex().lock().await;
        let mut pre_admission_registration = None;

        let runtime_was_registered = adapter.contains_session(session_id).await;
        let staged_session_existed = self.staged_sessions.contains(session_id).await;
        self.ensure_runtime_executor_on_adapter(adapter, session_id)
            .await?;

        if let Some(admission) = pre_admission.as_mut() {
            let admission_guard =
                match Self::take_runtime_pre_admission_guard(admission, session_id) {
                    Ok(admission) => admission,
                    Err(err) => {
                        self.unregister_new_runtime_registration_if_idle(
                            adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            cleanup_protects_active_admission,
                        )
                        .await;
                        return Err(err);
                    }
                };
            pre_admission_registration = Some(
                match self.register_runtime_pre_admission(
                    session_id.clone(),
                    input_id.clone(),
                    admission_guard,
                ) {
                    Ok(registration) => registration,
                    Err(err) => {
                        self.unregister_new_runtime_registration_if_idle(
                            adapter,
                            session_id,
                            runtime_was_registered,
                            staged_session_existed,
                            cleanup_protects_active_admission,
                        )
                        .await;
                        return Err(err);
                    }
                },
            );
        }

        let accepted = match adapter
            .accept_input_with_completion(session_id, input)
            .await
        {
            Ok(pair) => pair,
            Err(error) => {
                self.unregister_new_runtime_registration_if_idle(
                    adapter,
                    session_id,
                    runtime_was_registered,
                    staged_session_existed,
                    cleanup_protects_active_admission,
                )
                .await;
                return Err(runtime_driver_error_to_rpc(error));
            }
        };

        let (outcome, handle) = accepted;
        let accepted_with_completion =
            matches!(outcome, meerkat_runtime::AcceptOutcome::Accepted { .. }) && handle.is_some();
        if matches!(outcome, meerkat_runtime::AcceptOutcome::Accepted { .. })
            && let Some(registration) = pre_admission_registration.take()
        {
            if let Some(handle) = handle {
                self.spawn_runtime_pre_admission_cleanup(
                    session_id.clone(),
                    input_id.clone(),
                    handle,
                );
                registration.disarm();
            } else {
                drop(registration);
            }
        }
        if !accepted_with_completion {
            self.unregister_new_runtime_registration_if_idle(
                adapter,
                session_id,
                runtime_was_registered,
                staged_session_existed,
                cleanup_protects_active_admission,
            )
            .await;
        }
        drop(runtime_registration_guard);
        drop(runtime_registration_lock);
        Ok(outcome)
    }

    /// Admit a typed correlated terminal peer response through the
    /// runtime-backed path.
    pub async fn accept_peer_response_terminal_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        peer_id: meerkat_core::comms::PeerId,
        display_name: Option<meerkat_core::comms::PeerName>,
        request_id: meerkat_core::PeerCorrelationId,
        status: meerkat_contracts::PeerResponseTerminalStatusWire,
        result: serde_json::Value,
    ) -> Result<meerkat_runtime::AcceptOutcome, RpcError> {
        let input = meerkat_runtime::peer_response_terminal_input(
            peer_id,
            display_name,
            request_id,
            status,
            result,
        );

        let adapter = Arc::clone(&self.runtime_adapter);
        self.accept_runtime_input_with_active_admission(&adapter, session_id, input)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        primitive: &RunPrimitive,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        _additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<CoreApplyOutput, RpcError> {
        self.apply_runtime_turn_with_pre_admission(
            session_id,
            run_id,
            primitive,
            prompt,
            event_tx,
            skill_references,
            flow_tool_overlay,
            _additional_instructions,
            overrides,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn apply_runtime_turn_with_pre_admission(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        primitive: &RunPrimitive,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        _additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
        pre_admission: Option<RuntimePreAdmission>,
    ) -> Result<CoreApplyOutput, RpcError> {
        let mut pre_admission = pre_admission.map(RuntimePreAdmissionGuard::new);
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: reason.to_string(),
                data: None,
            });
        }

        if self
            .archived_persisted_session_without_live(session_id)
            .await?
        {
            return Err(Self::session_not_found_rpc(session_id));
        }

        // Context-only staged primitives may land directly as runtime
        // system-context appends, but terminal peer responses carry a typed
        // apply intent that requires a requester reaction turn.
        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = primitive else {
                unreachable!("context-only apply without turn only matches staged primitives");
            };
            let context_pre_admission = match pre_admission.as_mut() {
                Some(admission) => Some(Self::take_runtime_pre_admission_guard(
                    admission, session_id,
                )?),
                None => None,
            };
            return self
                .apply_runtime_context_appends_with_recovery(
                    session_id,
                    run_id,
                    pending_system_context_appends(&staged.context_appends),
                    primitive.apply_boundary(),
                    staged.contributing_input_ids.clone(),
                    overrides.as_ref(),
                    context_pre_admission,
                )
                .await;
        }

        if self
            .archived_persisted_session_without_live(session_id)
            .await?
        {
            return Err(Self::session_not_found_rpc(session_id));
        }

        let pre_turn_context_appends = match primitive {
            RunPrimitive::StagedInput(staged)
                if primitive.is_peer_response_terminal_context_and_run() =>
            {
                pending_system_context_appends(&staged.context_appends)
            }
            _ => Vec::new(),
        };

        let effective_identity = self
            .effective_llm_identity_for_turn(session_id, overrides.as_ref())
            .await?;
        self.validate_prompt_video_input(&prompt, &effective_identity)
            .await?;

        let pending_session = match self.staged_sessions.begin_promotion(session_id).await {
            Ok(slot) => slot,
            Err(meerkat::StagedLifecycleError::AlreadyPromoting(_)) => {
                return Err(RpcError {
                    code: error::SESSION_BUSY,
                    message: format!("session {session_id} is already being materialized"),
                    data: None,
                });
            }
            Err(e) => {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("staged session lifecycle error: {e}"),
                    data: None,
                });
            }
        };

        let persisted_keep_alive = if pending_session.is_none() {
            self.load_persisted_session(session_id)
                .await?
                .and_then(|session| session.session_metadata().map(|meta| meta.keep_alive))
        } else {
            None
        };
        let keep_alive = overrides
            .as_ref()
            .and_then(|ov| ov.keep_alive)
            .or_else(|| {
                pending_session
                    .as_ref()
                    .map(|slot| slot.build_config.keep_alive)
            })
            .or(persisted_keep_alive)
            .unwrap_or(false);

        if pending_session.is_none() && !self.live_session_is_stale(session_id).await? {
            let active_turn = match pre_admission.as_mut() {
                Some(admission) => {
                    Self::take_runtime_pre_admission_guard(admission, session_id)?.into_admission()
                }
                None => self.reserve_active_turn(session_id).await?.into_admission(),
            };
            // Hot-swap LLM client if model/provider overrides are present.
            if let Some(ref ov) = overrides
                && (ov.model.is_some()
                    || ov.provider.is_some()
                    || ov.provider_params.is_some()
                    || ov.clear_provider_params
                    || ov.auth_binding.is_some()
                    || ov.clear_auth_binding)
            {
                self.hot_swap_llm_client(session_id, ov).await?;
            }

            let req = StartTurnRequest {
                prompt: prompt.clone(),
                system_prompt: None,
                event_tx: Some(event_tx.clone()),
                runtime: StartTurnRuntimeSemantics::new(
                    None,
                    meerkat_core::types::HandlingMode::Queue,
                    skill_references.clone(),
                    flow_tool_overlay.clone(),
                    pre_turn_context_appends.clone(),
                    primitive.turn_metadata().cloned(),
                ),
            };

            let result_rx = self.spawn_service_apply_runtime_turn_with_recoverable_admission_guard(
                session_id.clone(),
                run_id.clone(),
                req,
                match primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                primitive.contributing_input_ids().to_vec(),
                active_turn,
            );
            let (result, recovered_admission) =
                Self::await_service_apply_runtime_turn_with_recoverable_admission(
                    session_id, result_rx,
                )
                .await?;
            match result {
                Ok(output) => return Ok(output),
                Err(SessionError::NotFound { .. }) => {
                    if let Some(admission) = recovered_admission {
                        pre_admission = Some(RuntimePreAdmissionGuard::new(admission));
                    }
                }
                Err(err) => {
                    drop(recovered_admission);
                    return Err(session_error_to_rpc(err));
                }
            }
        }

        if let Some(slot) = pending_session {
            let staged_capacity_admission = match pre_admission.as_mut() {
                Some(admission) => Some(
                    Self::take_runtime_pre_admission_guard(admission, session_id)?.into_admission(),
                ),
                None => self.take_staged_capacity_admission(session_id),
            };
            let mut promotion_cleanup = PendingPromotionCleanup::new(
                Arc::clone(&self.staged_sessions),
                Arc::clone(&self.staged_capacity_admissions),
                session_id,
                &slot,
                staged_capacity_admission,
            );
            let PromotingSlot {
                build_config,
                labels,
                deferred_prompt,
                machine_archived_resume_authorized,
                ..
            } = slot;
            let mut build_config = *build_config;
            let mut runtime_prompt: ContentInput = prompt.clone();
            if let Some(deferred) = deferred_prompt {
                runtime_prompt = merge_content_inputs(deferred, runtime_prompt);
            }

            let mut resolved_override_identity = None;
            if let Some(ref ov) = overrides {
                if ov.provider.is_some() && ov.model.is_none() {
                    promotion_cleanup.restore_now().await;
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: "provider override requires model on a session turn".to_string(),
                        data: None,
                    });
                }
                let base_identity = match self.llm_identity_from_pending_build(&build_config).await
                {
                    Ok(identity) => identity,
                    Err(err) => {
                        promotion_cleanup.restore_now().await;
                        return Err(err);
                    }
                };
                let resolved_identity =
                    match self.resolve_target_llm_identity(&base_identity, ov).await {
                        Ok(identity) => identity,
                        Err(err) => {
                            promotion_cleanup.restore_now().await;
                            return Err(err);
                        }
                    };
                Self::apply_llm_identity_to_build_config(&mut build_config, &resolved_identity);
                resolved_override_identity = Some(resolved_identity);
                if let Some(max_tokens) = ov.max_tokens {
                    build_config.max_tokens = Some(max_tokens);
                }
                if let Some(ref system_prompt) = ov.system_prompt {
                    build_config.system_prompt = Some(system_prompt.clone());
                }
                if let Some(ref output_schema) = ov.output_schema {
                    match meerkat_core::OutputSchema::from_json_value(output_schema.clone()) {
                        Ok(os) => build_config.output_schema = Some(os),
                        Err(e) => {
                            promotion_cleanup.restore_now().await;
                            return Err(RpcError {
                                code: error::INVALID_PARAMS,
                                message: format!("Invalid output_schema override: {e}"),
                                data: None,
                            });
                        }
                    }
                }
                if let Some(retries) = ov.structured_output_retries {
                    build_config.structured_output_retries = retries;
                }
                if let Some(keep_alive) = ov.keep_alive {
                    build_config.keep_alive = keep_alive;
                }
            }

            if let Err(err) = Self::validate_keep_alive_has_comms_name(&build_config) {
                promotion_cleanup.restore_now().await;
                return Err(err);
            }
            if let Some(identity) = resolved_override_identity {
                promotion_cleanup.update_effective_llm_identity(identity);
            }
            if overrides.is_some() {
                promotion_cleanup.update_build_config(&build_config);
            }
            if build_config.llm_client_override.is_none()
                && let Some(client) = self.default_llm_client()
            {
                build_config.llm_client_override = Some(client);
                promotion_cleanup.update_build_config(&build_config);
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = self.config_runtime() {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            let mut build = build_config.to_session_build_options();
            build.realm_id = build
                .realm_id
                .or_else(|| self.realm_id.as_ref().map(ToString::to_string));
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);
            let event_tx = self
                .pending_session_event_fanout_tx(session_id, event_tx)
                .await;
            let create_req = CreateSessionRequest {
                model: build_config.model.clone(),
                prompt: runtime_prompt.clone(),
                render_metadata: None,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: None,

                skill_references: skill_references.clone(),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: labels.clone(),
            };
            let output_rx = self.spawn_pending_create_and_apply_runtime_turn_with_admission_guard(
                session_id.clone(),
                create_req,
                run_id,
                StartTurnRequest {
                    prompt: runtime_prompt,
                    system_prompt: None,
                    event_tx: Some(event_tx),
                    runtime: StartTurnRuntimeSemantics::new(
                        None,
                        meerkat_core::types::HandlingMode::Queue,
                        skill_references,
                        flow_tool_overlay,
                        pre_turn_context_appends.clone(),
                        primitive.turn_metadata().cloned(),
                    ),
                },
                match primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                primitive.contributing_input_ids().to_vec(),
                promotion_cleanup,
                build_config.keep_alive,
                machine_archived_resume_authorized,
            );
            let output = Self::await_service_apply_runtime_turn(session_id, output_rx).await;
            self.bridge_pending_session_event_streams(session_id).await;
            return output;
        }

        let stored_session = self
            .load_persisted_session(session_id)
            .await?
            .ok_or_else(|| RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("session not found: {session_id}"),
                data: None,
            })?;
        let recovery_overrides =
            self.recovery_overrides_from_turn(overrides.as_ref(), keep_alive)?;
        let active_turn = match pre_admission.as_mut() {
            Some(admission) => {
                Self::take_runtime_pre_admission_guard(admission, session_id)?.into_admission()
            }
            None => self.reserve_active_turn(session_id).await?.into_admission(),
        };
        let recovered_create = self
            .recovered_create_request(session_id, stored_session, recovery_overrides)
            .await?;
        let output_rx = self.spawn_recovered_create_and_apply_runtime_turn_with_admission_guard(
            session_id.clone(),
            recovered_create.request,
            recovered_create.runtime_was_registered,
            run_id,
            StartTurnRequest {
                prompt,
                system_prompt: None,
                event_tx: Some(event_tx),
                runtime: StartTurnRuntimeSemantics::new(
                    None,
                    meerkat_core::types::HandlingMode::Queue,
                    skill_references,
                    flow_tool_overlay,
                    pre_turn_context_appends,
                    primitive.turn_metadata().cloned(),
                ),
            },
            match primitive {
                RunPrimitive::StagedInput(staged) => staged.boundary,
                _ => RunApplyBoundary::Immediate,
            },
            primitive.contributing_input_ids().to_vec(),
            active_turn,
            keep_alive,
        );
        let output = Self::await_service_apply_runtime_turn(session_id, output_rx).await?;
        Ok(output)
    }

    pub fn skill_identity_registry(&self) -> SourceIdentityRegistry {
        self.skill_identity_registry
            .read()
            .map(|state| state.registry.clone())
            .unwrap_or_default()
    }

    /// Build a source identity registry from runtime config.
    pub fn build_skill_identity_registry(
        config: &Config,
        context_root: Option<&std::path::Path>,
        user_root: Option<&std::path::Path>,
    ) -> Result<SourceIdentityRegistry, SkillError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (context_root, user_root);
            config.skills.build_source_identity_registry()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (context_root, user_root);
            config.skills.build_source_identity_registry()
        }
    }

    /// Create a new session with the given build configuration.
    ///
    /// Returns the session ID on success. The session is staged as "pending"
    /// and will be materialized inside the service on the first `start_turn`.
    pub async fn create_session(
        &self,
        mut build_config: AgentBuildConfig,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
    ) -> Result<SessionId, RpcError> {
        let effective_llm_identity = self.llm_identity_from_pending_build(&build_config).await?;
        build_config.self_hosted_server_id = effective_llm_identity.self_hosted_server_id.clone();
        if let Some(prompt) = deferred_prompt.as_ref() {
            self.validate_prompt_video_input(prompt, &effective_llm_identity)
                .await?;
        }

        // Pre-create a session to claim a stable SessionId.
        let session = Session::new();
        let session_id = session.id().clone();
        let admission = self
            .service
            .reserve_create_session_admission()
            .await
            .map_err(session_error_to_rpc)?;

        // Inject the pre-created session so the agent builder will reuse this ID.
        let bindings = self
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!(
                    "failed to prepare runtime bindings for session {session_id}: {e}"
                ),
                data: None,
            })?;

        let build_config = AgentBuildConfig {
            resume_session: Some(session),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..build_config
        };

        #[cfg(test)]
        let create_session_after_prepare_bindings_error = {
            self.create_session_after_prepare_bindings_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        };
        #[cfg(test)]
        if let Some(message) = create_session_after_prepare_bindings_error {
            *self
                .create_session_after_prepare_bindings_session_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session_id.clone());
            let _ = self.archive_runtime_cleanup().run(&session_id).await;
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message,
                data: None,
            });
        }

        #[cfg(feature = "mcp")]
        let build_config = match self
            .attach_mcp_adapter_for_pending_session(session_id.clone(), build_config)
            .await
        {
            Ok(build_config) => build_config,
            Err(err) => {
                let _ = self.archive_runtime_cleanup().run(&session_id).await;
                return Err(err);
            }
        };

        let now = now_unix_secs();
        if let Err(err) = self
            .staged_sessions
            .stage(
                session_id.clone(),
                StagedSlot {
                    effective_llm_identity,
                    phase: StagedPhase::Staged {
                        build_config: Box::new(build_config),
                    },
                    labels,
                    deferred_prompt,
                    created_at_secs: now,
                    updated_at_secs: now,
                    machine_archived_resume_authorized: false,
                },
            )
            .await
        {
            let _ = self.archive_runtime_cleanup().run(&session_id).await;
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("failed to stage session {session_id}: {err}"),
                data: None,
            });
        }
        if let Err(err) = self.insert_staged_capacity_admission(session_id.clone(), admission) {
            let _ = self.staged_sessions.abandon(&session_id).await;
            let _ = self.archive_runtime_cleanup().run(&session_id).await;
            return Err(err);
        }

        Ok(session_id)
    }

    /// Start a turn on the given session.
    ///
    /// Events are forwarded to `event_tx` during the turn. Returns the
    /// `RunResult` when the turn completes.
    ///
    /// `overrides` may contain per-turn overrides. For pending (deferred)
    /// sessions, all overrides are applied to the staged `AgentBuildConfig`.
    /// For materialized sessions, only `keep_alive` is allowed; all other
    /// overrides are rejected with an error.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub async fn start_turn(
        &self,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        #[allow(unused_mut)]
        let mut turn_prompt = prompt;

        // Check if this is a pending (not-yet-materialized) session.
        let pending_session = match self.staged_sessions.begin_promotion(session_id).await {
            Ok(slot) => slot,
            Err(meerkat::StagedLifecycleError::AlreadyPromoting(_)) => {
                return Err(RpcError {
                    code: error::SESSION_BUSY,
                    message: format!("session {session_id} is already being materialized"),
                    data: None,
                });
            }
            Err(e) => {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("staged session lifecycle error: {e}"),
                    data: None,
                });
            }
        };

        if let Some(slot) = pending_session {
            let staged_capacity_admission = self.take_staged_capacity_admission(session_id);
            let mut promotion_cleanup = PendingPromotionCleanup::new(
                Arc::clone(&self.staged_sessions),
                Arc::clone(&self.staged_capacity_admissions),
                session_id,
                &slot,
                staged_capacity_admission,
            );
            let PromotingSlot {
                build_config,
                labels,
                deferred_prompt,
                machine_archived_resume_authorized,
                ..
            } = slot;
            let mut build_config = *build_config;
            #[cfg(feature = "mcp")]
            if let Err(err) = self
                .apply_mcp_boundary_to_turn_prompt(session_id, &event_tx, &mut turn_prompt)
                .await
            {
                promotion_cleanup.restore_now().await;
                return Err(err);
            }
            // Prepend the deferred create-time prompt to the turn prompt.
            // Keep copies for rollback paths that re-stage the pending session.
            if let Some(deferred) = deferred_prompt {
                turn_prompt = merge_content_inputs(deferred, turn_prompt);
            }
            // Apply per-turn overrides to the pending build config.
            let mut resolved_override_identity = None;
            if let Some(ref ov) = overrides {
                if ov.provider.is_some() && ov.model.is_none() {
                    promotion_cleanup.restore_now().await;
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: "provider override requires model on a session turn".to_string(),
                        data: None,
                    });
                }
                let base_identity = match self.llm_identity_from_pending_build(&build_config).await
                {
                    Ok(identity) => identity,
                    Err(err) => {
                        promotion_cleanup.restore_now().await;
                        return Err(err);
                    }
                };
                let resolved_identity =
                    match self.resolve_target_llm_identity(&base_identity, ov).await {
                        Ok(identity) => identity,
                        Err(err) => {
                            promotion_cleanup.restore_now().await;
                            return Err(err);
                        }
                    };
                Self::apply_llm_identity_to_build_config(&mut build_config, &resolved_identity);
                resolved_override_identity = Some(resolved_identity);
                if let Some(max_tokens) = ov.max_tokens {
                    build_config.max_tokens = Some(max_tokens);
                }
                if let Some(ref system_prompt) = ov.system_prompt {
                    build_config.system_prompt = Some(system_prompt.clone());
                }
                if let Some(ref output_schema) = ov.output_schema {
                    match meerkat_core::OutputSchema::from_json_value(output_schema.clone()) {
                        Ok(os) => build_config.output_schema = Some(os),
                        Err(e) => {
                            // Restore pending state before returning error.
                            promotion_cleanup.restore_now().await;
                            return Err(RpcError {
                                code: error::INVALID_PARAMS,
                                message: format!("Invalid output_schema override: {e}"),
                                data: None,
                            });
                        }
                    }
                }
                if let Some(retries) = ov.structured_output_retries {
                    build_config.structured_output_retries = retries;
                }
                if let Some(keep_alive) = ov.keep_alive {
                    build_config.keep_alive = keep_alive;
                }
            }
            if let Err(err) = Self::validate_keep_alive_has_comms_name(&build_config) {
                promotion_cleanup.restore_now().await;
                return Err(err);
            }
            if let Some(identity) = resolved_override_identity {
                promotion_cleanup.update_effective_llm_identity(identity);
            }
            if overrides.is_some() {
                promotion_cleanup.update_build_config(&build_config);
            }
            // Inject default LLM client if the caller didn't provide one.
            if build_config.llm_client_override.is_none()
                && let Some(client) = self.default_llm_client()
            {
                build_config.llm_client_override = Some(client);
                promotion_cleanup.update_build_config(&build_config);
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = self.config_runtime() {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            let mut build = build_config.to_session_build_options();
            build.realm_id = build
                .realm_id
                .or_else(|| self.realm_id.as_ref().map(ToString::to_string));
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);
            let initial_turn_provider_hint = build_config
                .provider
                .or_else(|| meerkat_core::Provider::infer_from_model(&build_config.model))
                .map(|provider| provider.as_str());
            build.initial_turn_metadata =
                Some(Self::runtime_stamped_prompt_turn_metadata_from_overrides(
                    skill_references.clone(),
                    flow_tool_overlay.clone(),
                    additional_instructions.clone(),
                    overrides.as_ref(),
                    initial_turn_provider_hint,
                ));
            let turn_metadata = build.initial_turn_metadata.clone();
            let event_tx = self
                .pending_session_event_fanout_tx(session_id, event_tx)
                .await;

            let create_req = CreateSessionRequest {
                model: build_config.model.clone(),
                prompt: ContentInput::Text(String::new()),
                render_metadata: None,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: None,

                skill_references: skill_references.clone(),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: labels.clone(),
            };

            let result_rx = self.spawn_pending_create_and_start_turn_with_admission_guard(
                session_id.clone(),
                create_req,
                StartTurnRequest {
                    prompt: turn_prompt,
                    system_prompt: None,
                    event_tx: Some(event_tx),
                    runtime: StartTurnRuntimeSemantics::new(
                        None,
                        meerkat_core::types::HandlingMode::Queue,
                        skill_references,
                        flow_tool_overlay,
                        Vec::new(),
                        turn_metadata,
                    ),
                },
                promotion_cleanup,
                machine_archived_resume_authorized,
            );
            let result = Self::await_service_start_turn(session_id, result_rx).await;
            self.bridge_pending_session_event_streams(session_id).await;
            return result;
        }

        // Normal turn on an existing (materialized) session.
        // Reject overrides that cannot be applied mid-session.
        if let Some(ref ov) = overrides {
            let rejected = [
                ov.max_tokens.map(|_| "max_tokens"),
                ov.system_prompt.as_ref().map(|_| "system_prompt"),
                ov.output_schema.as_ref().map(|_| "output_schema"),
                ov.structured_output_retries
                    .map(|_| "structured_output_retries"),
            ];
            let rejected: Vec<&str> = rejected.into_iter().flatten().collect();
            if !rejected.is_empty() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "Cannot override {} on a materialized session; use deferred session/create",
                        rejected.join(", ")
                    ),
                    data: None,
                });
            }
        }

        let keep_alive = match overrides.as_ref().and_then(|ov| ov.keep_alive) {
            Some(keep_alive) => keep_alive,
            None => self
                .load_persisted_session(session_id)
                .await?
                .and_then(|session| session.session_metadata().map(|meta| meta.keep_alive))
                .unwrap_or(false),
        };
        let turn_metadata = Some(Self::runtime_stamped_prompt_turn_metadata_from_overrides(
            skill_references.clone(),
            flow_tool_overlay.clone(),
            additional_instructions.clone(),
            overrides.as_ref(),
            overrides.as_ref().and_then(|ov| ov.provider.as_deref()),
        ));

        if self.live_session_is_stale(session_id).await? {
            return Box::pin(self.try_recover_persisted_session(
                session_id,
                turn_prompt,
                event_tx,
                keep_alive,
                skill_references,
                flow_tool_overlay,
                additional_instructions,
                overrides.as_ref(),
            ))
            .await;
        }

        let active_turn = self.reserve_active_turn(session_id).await?;

        // Persist explicit keep_alive override so subsequent inheriting calls
        // (REST/MCP resume with None) observe the updated intent.
        // This is not fire-and-forget: if the update fails, the turn must not
        // proceed with divergent runtime vs persisted state.
        if overrides.as_ref().and_then(|ov| ov.keep_alive).is_some() {
            #[cfg(feature = "comms")]
            let comms_rt = self.service.comms_runtime(session_id).await;
            #[cfg(feature = "comms")]
            if keep_alive && comms_rt.is_none() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: "keep_alive requires a session created with comms_name".to_string(),
                    data: None,
                });
            }
            self.service
                .apply_runtime_session_keep_alive(session_id, keep_alive)
                .await
                .map_err(session_error_to_rpc)?;
            // W2-G: never reconfigure a mob-owned drain from the
            // keep_alive-override path. See `start_turn_via_runtime`.
            #[cfg(feature = "comms")]
            {
                let owner = self.runtime_adapter.peer_ingress_owner(session_id).await;
                if !owner.is_mob_owned() {
                    self.runtime_adapter
                        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
                        .await;
                }
            }
        }

        // Hot-swap LLM client if model/provider/provider_params changed.
        if let Some(ref ov) = overrides
            && (ov.model.is_some()
                || ov.provider.is_some()
                || ov.provider_params.is_some()
                || ov.clear_provider_params
                || ov.auth_binding.is_some()
                || ov.clear_auth_binding)
        {
            self.hot_swap_llm_client(session_id, ov).await?;
        }

        #[cfg(feature = "mcp")]
        self.apply_mcp_boundary_to_turn_prompt(session_id, &event_tx, &mut turn_prompt)
            .await?;

        let req = StartTurnRequest {
            prompt: turn_prompt.clone(),
            system_prompt: None,
            event_tx: Some(event_tx.clone()),
            runtime: StartTurnRuntimeSemantics::new(
                None,
                meerkat_core::types::HandlingMode::Queue,
                skill_references.clone(),
                flow_tool_overlay.clone(),
                Vec::new(),
                turn_metadata,
            ),
        };

        let result_rx = self.spawn_service_start_turn_with_admission_guard(
            session_id.clone(),
            req,
            active_turn.into_admission(),
        );
        let result = Self::await_service_start_turn(session_id, result_rx).await;
        match result {
            Ok(result) => Ok(result),
            Err(err) if err.code == error::SESSION_NOT_FOUND => {
                // Attempt persisted session recovery: the session may exist in the
                // store from a previous runtime lifetime.
                Box::pin(self.try_recover_persisted_session(
                    session_id,
                    turn_prompt,
                    event_tx,
                    keep_alive,
                    skill_references,
                    flow_tool_overlay,
                    additional_instructions,
                    overrides.as_ref(),
                ))
                .await
            }
            Err(err) => Err(err),
        }
    }

    /// Attempt to recover a persisted session that is no longer live.
    ///
    /// Mirrors the REST recovery path: load the session from the store, extract
    /// stored metadata, build an `AgentBuildConfig`, stage as pending, and
    /// fall through to the normal materialization path.
    ///
    /// Returns `SESSION_NOT_FOUND` if the session cannot be recovered.
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    async fn try_recover_persisted_session(
        &self,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        keep_alive: bool,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        let loaded_session = self.load_persisted_session(session_id).await?;

        let Some(session) = loaded_session else {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        };
        let recovery_overrides = self.recovery_overrides_from_turn(overrides, keep_alive)?;
        let admission = self.reserve_active_turn(session_id).await?;
        if self
            .archived_persisted_session_without_live(session_id)
            .await?
        {
            return Err(Self::session_not_found_rpc(session_id));
        }
        let recovered_create = self
            .recovered_create_request(session_id, session, recovery_overrides)
            .await?;
        let runtime_was_registered = recovered_create.runtime_was_registered;
        let create_request = recovered_create.request;
        let build = match create_request.build.clone() {
            Some(build) => build,
            None => {
                self.cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                    .await;
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!(
                        "recovered create request for session {session_id} is missing build options"
                    ),
                    data: None,
                });
            }
        };
        let model = create_request.model.clone();
        let mut build_config = AgentBuildConfig::new(model);
        build_config.apply_session_build_options(&build);
        build_config.system_prompt = create_request.system_prompt.clone();
        build_config.max_tokens = create_request.max_tokens;

        // Stage as pending and re-enter the materialization path.
        // Labels are managed by the session service — pass None so
        // CreateSessionRequest.labels is empty and the service preserves
        // whatever labels were stored with the original session.
        let labels: Option<BTreeMap<String, String>> = None;

        {
            let effective_llm_identity =
                match self.llm_identity_from_pending_build(&build_config).await {
                    Ok(identity) => identity,
                    Err(err) => {
                        self.cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                            .await;
                        return Err(err);
                    }
                };
            let now = now_unix_secs();
            if let Err(err) = self
                .staged_sessions
                .stage(
                    session_id.clone(),
                    StagedSlot {
                        effective_llm_identity,
                        phase: StagedPhase::Staged {
                            build_config: Box::new(build_config),
                        },
                        labels,
                        deferred_prompt: None,
                        created_at_secs: now,
                        updated_at_secs: now,
                        machine_archived_resume_authorized: false,
                    },
                )
                .await
            {
                self.cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                    .await;
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("failed to stage recovered session {session_id}: {err}"),
                    data: None,
                });
            }
        }
        if let Err(err) =
            self.insert_staged_capacity_admission(session_id.clone(), admission.into_admission())
        {
            let _ = self.staged_sessions.abandon(session_id).await;
            self.cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                .await;
            return Err(err);
        }

        // Recursively call start_turn which will now find the pending session.
        // Use Box::pin to avoid infinite recursion concerns in async.
        Box::pin(self.start_turn(
            session_id,
            prompt,
            event_tx,
            skill_references,
            flow_tool_overlay,
            additional_instructions,
            None,
        ))
        .await
    }

    async fn promoting_system_context_state(
        &self,
        session_id: &SessionId,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        self.staged_sessions
            .promoting_system_context_state(session_id)
            .await
    }

    async fn replay_promoted_system_context(
        &self,
        session_id: &SessionId,
        starting_state: &SessionSystemContextState,
        current_state: &SessionSystemContextState,
    ) -> Result<(), RpcError> {
        Self::replay_promoted_system_context_on_service(
            Arc::clone(&self.service),
            session_id,
            starting_state,
            current_state,
        )
        .await
    }

    async fn replay_promoted_system_context_on_service(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        session_id: &SessionId,
        starting_state: &SessionSystemContextState,
        current_state: &SessionSystemContextState,
    ) -> Result<(), RpcError> {
        for pending in &current_state.pending {
            if starting_state.pending.contains(pending) {
                continue;
            }
            service
                .append_system_context(
                    session_id,
                    AppendSystemContextRequest {
                        text: pending.text.clone(),
                        source: pending.source.clone(),
                        idempotency_key: pending.idempotency_key.clone(),
                    },
                )
                .await
                .map_err(system_context_error_to_rpc)?;
        }
        Ok(())
    }

    /// Interrupt a running turn on the given session.
    ///
    /// If the session is idle, this is a no-op.
    pub async fn interrupt(&self, session_id: &SessionId) -> Result<(), RpcError> {
        let staged_info = self.staged_sessions.info(session_id).await;
        let staged_is_promoting = match staged_info {
            Some(info) if info.is_promoting => true,
            Some(_) if !self.has_staged_capacity_admission(session_id) => true,
            Some(_) => return Ok(()),
            None => false,
        };

        if staged_is_promoting {
            return match self
                .service
                .interrupt_with_machine_authority(
                    session_id,
                    self.runtime_adapter.session_control_authority(),
                )
                .await
            {
                Ok(()) => Ok(()),
                Err(SessionError::NotRunning { .. } | SessionError::NotFound { .. }) => {
                    Err(Self::promoting_session_busy_rpc(session_id))
                }
                Err(err) => Err(session_error_to_rpc(err)),
            };
        }

        if self.authoritative_session_archived(session_id).await? {
            return Err(Self::archived_session_not_found_rpc(session_id));
        }

        match self
            .runtime_adapter
            .hard_cancel_current_run(session_id, "RPC session runtime interrupt")
            .await
        {
            Ok(()) => Ok(()),
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Running,
            }) if staged_is_promoting => Err(Self::promoting_session_busy_rpc(session_id)),
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle | RuntimeState::Attached,
            }) if staged_is_promoting => Err(Self::promoting_session_busy_rpc(session_id)),
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle | RuntimeState::Attached,
            }) => Ok(()),
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })
            | Err(RuntimeDriverError::Destroyed) => {
                if staged_is_promoting {
                    return Err(Self::promoting_session_busy_rpc(session_id));
                }
                match self.interrupt_noop_target(session_id).await? {
                    InterruptNoopTarget::Present => return Ok(()),
                    InterruptNoopTarget::Missing => {}
                }
                Err(RpcError {
                    code: error::SESSION_NOT_FOUND,
                    message: format!("Session not found: {session_id}"),
                    data: None,
                })
            }
            Err(RuntimeDriverError::NotReady { state }) => Err(RpcError {
                code: error::INVALID_REQUEST,
                message: format!("Session is not interruptible while runtime is {state}"),
                data: None,
            }),
            Err(e) => Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("Failed to interrupt session: {e}"),
                data: None,
            }),
        }
    }

    fn promoting_session_busy_rpc(session_id: &SessionId) -> RpcError {
        RpcError {
            code: error::SESSION_BUSY,
            message: format!(
                "session {session_id} is still being materialized and cannot be interrupted yet"
            ),
            data: None,
        }
    }

    fn archived_session_not_found_rpc(session_id: &SessionId) -> RpcError {
        RpcError {
            code: error::SESSION_NOT_FOUND,
            message: format!("Session not found: {session_id}"),
            data: None,
        }
    }

    pub(crate) async fn interrupt_noop_target(
        &self,
        session_id: &SessionId,
    ) -> Result<InterruptNoopTarget, RpcError> {
        if self.staged_sessions.contains(session_id).await {
            return Ok(InterruptNoopTarget::Present);
        }

        if let Some(session) = self.load_persisted_session(session_id).await? {
            if session_metadata_marks_mob_member(&session) {
                #[cfg(feature = "mob")]
                if let Some(mob_state) = self.mob_state() {
                    let owns_mob_session = mob_state.owns_live_bridge_session(session_id).await
                        || mob_state.owns_persisted_bridge_session(session_id).await;
                    return Ok(interrupt_noop_target_for_presence(owns_mob_session));
                }
                #[cfg(not(feature = "mob"))]
                return Ok(InterruptNoopTarget::Missing);
            }
            return Ok(interrupt_noop_target_for_presence(true));
        }

        #[cfg(feature = "mob")]
        if let Some(mob_state) = self.mob_state()
            && (mob_state.owns_live_bridge_session(session_id).await
                || mob_state.owns_persisted_bridge_session(session_id).await)
        {
            match mob_state.session_service().read(session_id).await {
                Ok(_) => {
                    return Ok(interrupt_noop_target_for_presence(true));
                }
                Err(SessionError::NotFound { .. }) => {}
                Err(err) => return Err(session_error_to_rpc(err)),
            }
        }

        match self.service.read(session_id).await {
            Ok(_) => {
                return Ok(interrupt_noop_target_for_presence(true));
            }
            Err(SessionError::NotFound { .. }) => {}
            Err(err) => return Err(session_error_to_rpc(err)),
        }

        Ok(interrupt_noop_target_for_presence(false))
    }

    /// Ask a running turn to break out at the next cooperative boundary.
    ///
    /// Unlike `interrupt`, this must not hard-cancel the current run. It is used
    /// by runtime peer ingress so queued work can proceed after a wait/yield
    /// boundary without aborting the in-flight turn.
    pub async fn interrupt_yielding(&self, session_id: &SessionId) -> Result<(), RpcError> {
        let staged_info = self.staged_sessions.info(session_id).await;
        let staged_is_materializing = match staged_info.as_ref() {
            Some(info) if info.is_promoting => true,
            Some(_) if !self.has_staged_capacity_admission(session_id) => true,
            Some(_) => false,
            None => false,
        };
        if staged_info.is_some() && !staged_is_materializing {
            return Ok(());
        }
        if staged_is_materializing {
            return Err(Self::promoting_session_busy_rpc(session_id));
        }

        if self.authoritative_session_archived(session_id).await? {
            return Err(Self::archived_session_not_found_rpc(session_id));
        }

        match self.runtime_adapter.cancel_after_boundary(session_id).await {
            Ok(()) => Ok(()),
            Err(RuntimeDriverError::NotReady { .. }) | Err(RuntimeDriverError::Destroyed)
                if staged_is_materializing =>
            {
                Err(Self::promoting_session_busy_rpc(session_id))
            }
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle | RuntimeState::Attached,
            }) => Ok(()),
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })
            | Err(RuntimeDriverError::Destroyed) => {
                match self.interrupt_noop_target(session_id).await? {
                    InterruptNoopTarget::Present => Ok(()),
                    InterruptNoopTarget::Missing => Err(RpcError {
                        code: error::SESSION_NOT_FOUND,
                        message: format!("Session not found: {session_id}"),
                        data: None,
                    }),
                }
            }
            Err(RuntimeDriverError::NotReady { state }) => Err(RpcError {
                code: error::INVALID_REQUEST,
                message: format!("Session is not interruptible while runtime is {state}"),
                data: None,
            }),
            Err(e) => Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("Failed to interrupt session at boundary: {e}"),
                data: None,
            }),
        }
    }

    /// Append runtime system context to a pending, live, or persisted session.
    pub async fn append_system_context(
        &self,
        session_id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, RpcError> {
        if let Some(result) = self
            .staged_sessions
            .append_system_context(
                session_id,
                &req,
                meerkat_core::time_compat::SystemTime::now(),
                now_unix_secs(),
            )
            .await
        {
            let status = result
                .map_err(|err| system_context_error_to_rpc(err.into_control_error(session_id)))?;
            return Ok(AppendSystemContextResult { status });
        }

        self.service
            .append_system_context(session_id, req)
            .await
            .map_err(system_context_error_to_rpc)
    }

    /// Get the current state of a session, or `None` if the session does not exist.
    pub async fn session_state(&self, session_id: &SessionId) -> Option<SessionInfo> {
        // Check pending sessions first.
        {
            if let Some(info) = self.staged_sessions.info(session_id).await {
                return Some(SessionInfo {
                    session_id: session_id.clone(),
                    state: if info.is_promoting {
                        SessionState::Running
                    } else {
                        SessionState::Idle
                    },
                    labels: info.labels,
                });
            }
        }

        // Use `list()` instead of `read()` to avoid blocking on a
        // `ReadSnapshot` command while a turn is in progress. `list()`
        // reads state from non-blocking watch receivers.
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                if summary.session_id == *session_id {
                    return Some(SessionInfo {
                        session_id: summary.session_id,
                        state: if summary.is_active {
                            SessionState::Running
                        } else {
                            SessionState::Idle
                        },
                        labels: summary.labels,
                    });
                }
            }
        }

        None
    }

    pub async fn pending_session_exists(&self, session_id: &SessionId) -> bool {
        self.staged_sessions.contains(session_id).await
    }

    async fn apply_pending_keep_alive_override(
        &self,
        session_id: &SessionId,
        keep_alive: bool,
    ) -> Result<bool, RpcError> {
        match self
            .staged_sessions
            .update_keep_alive(session_id, keep_alive, now_unix_secs())
            .await
        {
            Ok(applied) => Ok(applied),
            Err(meerkat::StagedLifecycleError::AlreadyPromoting(_)) => Err(RpcError {
                code: error::SESSION_BUSY,
                message: format!("session {session_id} is already being materialized"),
                data: None,
            }),
            Err(meerkat::StagedLifecycleError::KeepAliveRequiresCommsName) => Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "keep_alive requires a session created with comms_name".to_string(),
                data: None,
            }),
            Err(err) => Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("staged session lifecycle error: {err}"),
                data: None,
            }),
        }
    }

    fn validate_keep_alive_has_comms_name(build_config: &AgentBuildConfig) -> Result<(), RpcError> {
        if build_config.keep_alive && build_config.comms_name.is_none() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "keep_alive requires a session created with comms_name".to_string(),
                data: None,
            });
        }
        Ok(())
    }

    /// Read the authoritative session view from the owning session service.
    pub async fn read_session(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::service::SessionView, RpcError> {
        self.service
            .read(session_id)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Load the persisted session snapshot for authoritative post-turn inspection.
    pub async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, RpcError> {
        let Some(session) = self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(session_error_to_rpc)?
        else {
            return Ok(None);
        };
        if self
            .session_archived_by_authority(session_id, &session)
            .await?
        {
            return Ok(None);
        }
        Ok(Some(session))
    }

    async fn session_archived_by_authority(
        &self,
        session_id: &SessionId,
        session: &Session,
    ) -> Result<bool, RpcError> {
        self.service
            .session_archived_by_authority(session_id, session)
            .await
            .map_err(session_error_to_rpc)
    }

    pub(crate) async fn authoritative_session_archived(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RpcError> {
        let Some(session) = self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(session_error_to_rpc)?
        else {
            return Ok(false);
        };
        self.session_archived_by_authority(session_id, &session)
            .await
    }

    /// Archive (remove) a session.
    pub async fn archive_session(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Check pending sessions first.
        match self.staged_sessions.begin_archive(session_id).await {
            Ok(true) => {
                let mut staged_archive_guard =
                    StagedArchiveRollbackGuard::new(Arc::clone(&self.staged_sessions), session_id);
                #[cfg(test)]
                let staged_archive_after_service_hook = {
                    let guard = self
                        .staged_archive_after_service_hook
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.clone()
                };
                let staged_capacity_admission =
                    match self.take_staged_capacity_admission(session_id) {
                        Some(admission) => Some(admission),
                        None => {
                            self.staged_sessions.restore_archive(session_id).await;
                            staged_archive_guard.disarm();
                            return Err(RpcError {
                                code: error::SESSION_BUSY,
                                message: format!("session {session_id} has active work"),
                                data: None,
                            });
                        }
                    };

                await_guarded_staged_archive(
                    Arc::clone(&self.service),
                    Arc::clone(&self.staged_sessions),
                    Arc::clone(&self.staged_capacity_admissions),
                    staged_capacity_admission,
                    self.archive_runtime_cleanup(),
                    session_id.clone(),
                    staged_archive_guard,
                    #[cfg(test)]
                    staged_archive_after_service_hook,
                )
                .await
                .map_err(session_error_to_rpc)?;
                self.pending_session_event_streams
                    .lock()
                    .await
                    .remove(session_id);
                #[cfg(feature = "mcp")]
                if let Some(state) = self.mcp_sessions.write().await.remove(session_id) {
                    state.adapter.shutdown().await;
                }
                return Ok(());
            }
            Ok(false) => {}
            Err(meerkat::StagedLifecycleError::AlreadyPromoting(_)) => {
                return Err(RpcError {
                    code: error::SESSION_BUSY,
                    message: format!("session {session_id} is already being materialized"),
                    data: None,
                });
            }
            Err(err) => {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("staged session lifecycle error: {err}"),
                    data: None,
                });
            }
        }

        let result = await_session_archive_with_runtime_cleanup(
            Arc::clone(&self.service),
            self.archive_runtime_cleanup(),
            session_id.clone(),
        )
        .await
        .map_err(session_error_to_rpc);

        if result.is_ok() {
            self.runtime_adapter.unregister_session(session_id).await;

            #[cfg(feature = "mcp")]
            if let Some(state) = self.mcp_sessions.write().await.remove(session_id) {
                state.adapter.shutdown().await;
            }

            self.pending_session_event_streams
                .lock()
                .await
                .remove(session_id);

            #[cfg(feature = "comms")]
            self.runtime_adapter.abort_comms_drain(session_id).await;
        }

        result
    }

    /// List all active sessions, optionally filtered by query parameters.
    pub async fn list_sessions(&self, query: SessionQuery) -> Vec<SessionInfo> {
        let mut result = Vec::new();

        // Include pending sessions as Idle, filtered by labels if requested.
        for (session_id, info) in self.staged_sessions.list(query.labels.as_ref()).await {
            result.push(SessionInfo {
                session_id,
                state: if info.is_promoting {
                    SessionState::Running
                } else {
                    SessionState::Idle
                },
                labels: info.labels,
            });
        }

        // Include active sessions from the service. The service's `list()`
        // already handles label filtering on materialized sessions.
        if let Ok(summaries) = self.service.list(query).await {
            for summary in summaries {
                let state = if summary.is_active {
                    SessionState::Running
                } else {
                    SessionState::Idle
                };
                result.push(SessionInfo {
                    session_id: summary.session_id,
                    state,
                    labels: summary.labels,
                });
            }
        }

        result
    }

    /// List all active sessions as canonical wire summaries, including pending sessions.
    pub async fn list_sessions_rich(
        &self,
        query: SessionQuery,
    ) -> Vec<meerkat_contracts::WireSessionSummary> {
        let mut result = Vec::new();

        // Include pending sessions as synthetic entries.
        for (session_id, info) in self.staged_sessions.list(query.labels.as_ref()).await {
            result.push(meerkat_contracts::WireSessionSummary {
                session_id,
                session_ref: None,
                created_at: info.created_at_secs,
                updated_at: info.updated_at_secs,
                message_count: 0,
                total_tokens: 0,
                is_active: info.is_promoting,
                labels: info.labels,
            });
        }

        // Include materialized sessions from the service.
        if let Ok(summaries) = self.service.list(query).await {
            for summary in summaries {
                result.push(summary.into());
            }
        }

        result
    }

    /// Read a session as a canonical wire info object, checking pending and materialized.
    pub async fn read_session_rich(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_contracts::WireSessionInfo> {
        // Check pending sessions first.
        if let Some(info) = self.staged_sessions.info(session_id).await {
            let mut wire = meerkat_contracts::WireSessionInfo {
                session_id: session_id.clone(),
                session_ref: None,
                created_at: info.created_at_secs,
                updated_at: info.updated_at_secs,
                message_count: 0,
                is_active: info.is_promoting,
                model: info.effective_llm_identity.model.clone(),
                provider: info.effective_llm_identity.provider.as_str().to_string(),
                last_assistant_text: None,
                resolved_capabilities: self
                    .wire_resolved_capabilities_for_identity(&info.effective_llm_identity)
                    .await,
                labels: info.labels,
            };
            self.attach_resolved_capabilities_to_wire_info(&mut wire)
                .await;
            return Some(wire);
        }

        // Try list() first — non-blocking (uses watch receivers).
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                if summary.session_id == *session_id {
                    if !summary.is_active {
                        // Idle session: safe to call read() without blocking,
                        // which provides last_assistant_text.
                        if let Ok(view) = self.service.read(session_id).await {
                            let mut info = view.state.into();
                            self.attach_resolved_capabilities_to_wire_info(&mut info)
                                .await;
                            return Some(info);
                        }
                    }
                    // Active session: return summary without last_assistant_text
                    // to avoid blocking the caller during a running turn.
                    let wire: meerkat_contracts::WireSessionSummary = summary.into();
                    let llm_identity = self
                        .service
                        .live_session_llm_identity(session_id)
                        .await
                        .ok()?;
                    let mut info = meerkat_contracts::WireSessionInfo {
                        session_id: wire.session_id,
                        session_ref: None,
                        created_at: wire.created_at,
                        updated_at: wire.updated_at,
                        message_count: wire.message_count,
                        is_active: wire.is_active,
                        model: llm_identity.model,
                        provider: llm_identity.provider.as_str().to_string(),
                        last_assistant_text: None,
                        resolved_capabilities: None,
                        labels: wire.labels,
                    };
                    self.attach_resolved_capabilities_to_wire_info(&mut info)
                        .await;
                    return Some(info);
                }
            }
        }

        // Fallback: try read() for archived/persisted sessions not in the live list.
        if let Ok(view) = self.service.read(session_id).await {
            let mut info = view.state.into();
            self.attach_resolved_capabilities_to_wire_info(&mut info)
                .await;
            return Some(info);
        }

        None
    }

    /// Whether this runtime has a durable event replay projection installed.
    pub fn supports_event_replay(&self) -> bool {
        self.service.has_event_projection()
    }

    /// Read the latest durable event cursor for a session scope.
    pub async fn event_latest_cursor(
        &self,
        scope: meerkat_contracts::EventReplayScope,
    ) -> Result<Option<meerkat_contracts::EventReplayCursor>, RpcError> {
        let session_id = scope.session_id().clone();
        let latest = self
            .service
            .event_log_latest_seq(&session_id)
            .await
            .map_err(session_error_to_rpc)?;
        let Some(sequence) = latest else {
            return Ok(None);
        };
        if sequence == 0 && self.read_session_rich(&session_id).await.is_none() {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        }
        Ok(Some(meerkat_contracts::EventReplayCursor::new(
            scope, sequence,
        )))
    }

    /// Read durable events after a typed cursor.
    pub async fn event_list_since(
        &self,
        params: meerkat_contracts::EventsListSinceParams,
    ) -> Result<Option<meerkat_contracts::EventsListSinceResult>, RpcError> {
        let latest = match self.event_latest_cursor(params.scope.clone()).await? {
            Some(cursor) => cursor,
            None => return Ok(None),
        };
        let from_cursor = params
            .cursor
            .unwrap_or_else(|| meerkat_contracts::EventReplayCursor::new(params.scope.clone(), 0));
        let from_seq = from_cursor
            .validate_for_list_since(&params.scope, latest.sequence)
            .map_err(|err| RpcError {
                code: error::INVALID_PARAMS,
                message: match err {
                    meerkat_contracts::EventReplayCursorError::ScopeMismatch => {
                        "event replay cursor scope does not match requested scope".to_string()
                    }
                    meerkat_contracts::EventReplayCursorError::AheadOfLatest {
                        requested_sequence,
                        latest_sequence,
                    } => format!(
                        "event replay cursor sequence {requested_sequence} is ahead of latest sequence {latest_sequence}"
                    ),
                    meerkat_contracts::EventReplayCursorError::SequenceOverflow => {
                        "event replay cursor sequence overflow".to_string()
                    }
                },
                data: None,
            })?;
        let session_id = params.scope.session_id().clone();
        let stored = self
            .service
            .event_log_read_from(&session_id, from_seq)
            .await
            .map_err(session_error_to_rpc)?
            .unwrap_or_default();
        let limit = params.limit.unwrap_or(stored.len()).max(1);
        let has_more = stored.len() > limit;
        let events = stored
            .into_iter()
            .take(limit)
            .map(|stored| {
                meerkat_contracts::EventReplayEnvelope::session(
                    session_id.clone(),
                    stored.seq,
                    stored.timestamp,
                    stored.event,
                )
            })
            .collect();
        Ok(Some(meerkat_contracts::EventsListSinceResult {
            contract_version: meerkat_contracts::ContractVersion::CURRENT,
            scope: params.scope,
            from_cursor,
            latest_cursor: latest,
            events,
            has_more,
        }))
    }

    /// Read a point-in-time session snapshot and its paired replay cursor.
    pub async fn event_snapshot(
        &self,
        scope: meerkat_contracts::EventReplayScope,
    ) -> Result<Option<meerkat_contracts::EventsSnapshotResult>, RpcError> {
        let latest = match self.event_latest_cursor(scope.clone()).await? {
            Some(cursor) => cursor,
            None => return Ok(None),
        };
        let session = self
            .read_session_rich(scope.session_id())
            .await
            .ok_or_else(|| RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {}", scope.session_id()),
                data: None,
            })?;
        Ok(Some(meerkat_contracts::EventsSnapshotResult {
            contract_version: meerkat_contracts::ContractVersion::CURRENT,
            scope,
            cursor: latest,
            snapshot: meerkat_contracts::EventsSnapshotBody::Session { session },
        }))
    }

    /// Read a session transcript as canonical wire history, including pending sessions.
    pub async fn read_session_history_rich(
        &self,
        session_id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Option<meerkat_contracts::WireSessionHistory> {
        if self.staged_sessions.contains(session_id).await {
            return Some(meerkat_contracts::WireSessionHistory {
                session_id: session_id.clone(),
                session_ref: None,
                message_count: 0,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                messages: Vec::new(),
            });
        }

        self.service
            .read_history(session_id, query)
            .await
            .ok()
            .map(Into::into)
    }

    async fn reject_active_transcript_edit(&self, session_id: &SessionId) -> Result<(), RpcError> {
        let runtime_running = self
            .runtime_adapter
            .runtime_state(session_id)
            .await
            .is_ok_and(|state| matches!(state, RuntimeState::Running));
        let has_active_inputs = self
            .runtime_adapter
            .list_active_inputs(session_id)
            .await
            .is_ok_and(|inputs| !inputs.is_empty());
        if runtime_running || has_active_inputs {
            return Err(RpcError {
                code: error::SESSION_BUSY,
                message: format!(
                    "session {session_id} is active; transcript fork uses running_behavior=reject"
                ),
                data: None,
            });
        }
        Ok(())
    }

    /// Fork an idle materialized session at a transcript index.
    pub async fn fork_session_at(
        &self,
        session_id: &SessionId,
        req: SessionForkAtRequest,
    ) -> Result<SessionForkResult, RpcError> {
        if self.staged_sessions.contains(session_id).await {
            return Err(RpcError {
                code: error::SESSION_BUSY,
                message: format!(
                    "session {session_id} is not materialized; transcript fork is available only for idle materialized sessions"
                ),
                data: None,
            });
        }
        self.reject_active_transcript_edit(session_id).await?;

        self.service
            .fork_session_at(session_id, req)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Fork an idle materialized session and apply a typed transcript replacement.
    pub async fn fork_session_replace(
        &self,
        session_id: &SessionId,
        req: SessionForkReplaceRequest,
    ) -> Result<SessionForkResult, RpcError> {
        if self.staged_sessions.contains(session_id).await {
            return Err(RpcError {
                code: error::SESSION_BUSY,
                message: format!(
                    "session {session_id} is not materialized; transcript fork is available only for idle materialized sessions"
                ),
                data: None,
            });
        }
        self.reject_active_transcript_edit(session_id).await?;

        self.service
            .fork_session_replace(session_id, req)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Get the event injector for a session, if available.
    pub async fn event_injector(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.service.event_injector(session_id).await
    }

    async fn pending_session_event_stream_state(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<PendingSessionEventStreams> {
        self.pending_session_event_streams
            .lock()
            .await
            .get(session_id)
            .cloned()
    }

    async fn subscribe_pending_session_events(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_core::EventStream, meerkat_core::StreamError> {
        let sender = {
            let mut streams = self.pending_session_event_streams.lock().await;
            streams
                .entry(session_id.clone())
                .or_insert_with(|| {
                    let (tx, _rx) = broadcast::channel(PENDING_SESSION_EVENT_CHANNEL_CAPACITY);
                    PendingSessionEventStreams {
                        events: tx,
                        receiver_dropped: Arc::new(Notify::new()),
                    }
                })
                .clone()
        };
        let rx = sender.events.subscribe();
        let drop_guard = PendingSessionEventStreamDrop {
            receiver_dropped: Arc::clone(&sender.receiver_dropped),
        };
        Ok(Box::pin(futures::stream::unfold(
            (rx, drop_guard),
            |(mut rx, drop_guard)| async move {
                loop {
                    match rx.recv().await {
                        Ok(event) => return Some((event, (rx, drop_guard))),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        )))
    }

    async fn pending_session_event_fanout_tx(
        &self,
        session_id: &meerkat_core::SessionId,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
    ) -> mpsc::Sender<EventEnvelope<AgentEvent>> {
        let Some(pending_streams) = self.pending_session_event_stream_state(session_id).await
        else {
            return event_tx;
        };

        let (fanout_tx, mut fanout_rx) =
            mpsc::channel::<EventEnvelope<AgentEvent>>(PENDING_SESSION_EVENT_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            while let Some(event) = fanout_rx.recv().await {
                let _ = pending_streams.events.send(event.clone());
                let _ = event_tx.send(event).await;
            }
        });
        fanout_tx
    }

    async fn bridge_pending_session_event_streams(&self, session_id: &meerkat_core::SessionId) {
        let Some(pending_streams) = self.pending_session_event_stream_state(session_id).await
        else {
            return;
        };
        let Ok(mut live_stream) = self.service.subscribe_session_events(session_id).await else {
            return;
        };

        let session_id = session_id.clone();
        let pending_stream_registry = Arc::clone(&self.pending_session_event_streams);
        tokio::spawn(async move {
            loop {
                if pending_streams.events.receiver_count() == 0 {
                    break;
                }
                tokio::select! {
                    event = live_stream.next() => {
                        match event {
                            Some(event) => {
                                if pending_streams.events.receiver_count() == 0 {
                                    break;
                                }
                                let _ = pending_streams.events.send(event);
                            }
                            None => break,
                        }
                    }
                    () = pending_streams.receiver_dropped.notified() => {
                        if pending_streams.events.receiver_count() == 0 {
                            break;
                        }
                    }
                }
            }
            pending_stream_registry.lock().await.remove(&session_id);
        });
    }

    /// Subscribe to session-wide events regardless of triggering interaction.
    pub async fn subscribe_session_events(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_core::EventStream, meerkat_core::StreamError> {
        match self.service.subscribe_session_events(session_id).await {
            Ok(stream) => Ok(stream),
            Err(meerkat_core::StreamError::NotFound(_))
                if self.staged_sessions.contains(session_id).await =>
            {
                self.subscribe_pending_session_events(session_id).await
            }
            Err(err @ meerkat_core::StreamError::NotFound(_)) => self
                .service
                .subscribe_session_events(session_id)
                .await
                .map_err(|_| err),
            Err(err) => Err(err),
        }
    }

    /// Wait until a live session's authoritative summary timestamp advances.
    pub async fn wait_for_session_mutation_after(
        &self,
        session_id: &meerkat_core::SessionId,
        after: std::time::SystemTime,
    ) -> Result<std::time::SystemTime, meerkat_core::StreamError> {
        self.service
            .wait_for_session_mutation_after(session_id, after)
            .await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.service.comms_runtime(session_id).await
    }

    /// Shut down the runtime, closing all sessions.
    pub async fn shutdown(&self) {
        // Clear pending sessions.
        self.staged_sessions.clear().await;
        self.runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.staged_capacity_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.pending_session_event_streams.lock().await.clear();

        self.shutdown_schedule_host().await;

        // Shut down the service.
        self.service.shutdown().await;

        #[cfg(feature = "mcp")]
        {
            let mut map = self.mcp_sessions.write().await;
            let adapters = map
                .drain()
                .map(|(_, state)| state.adapter)
                .collect::<Vec<_>>();
            drop(map);
            for adapter in adapters {
                adapter.shutdown().await;
            }
        }

        // Abort all comms drain tasks.
        #[cfg(feature = "comms")]
        self.runtime_adapter.abort_comms_drains().await;
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_add(
        &self,
        session_id: &SessionId,
        server_config: McpServerConfig,
    ) -> Result<(), RpcError> {
        self.mcp_stage_add_with_persistence(session_id, server_config, false)
            .await
            .map(|_| ())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_add_with_persistence(
        &self,
        session_id: &SessionId,
        config: McpServerConfig,
        persisted: bool,
    ) -> Result<bool, RpcError> {
        self.ensure_session_exists(session_id).await?;
        if config.name.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }

        let adapter = self.mcp_adapter_for_session(session_id).await?;
        let rollback = if persisted {
            let (context_root, user_root) = self.skill_identity_roots();
            let authority =
                meerkat::surface::mcp_config_mutation_authority(context_root, user_root);
            match meerkat::surface::persist_mcp_add_if_requested(true, &authority, config.clone())
                .await
            {
                Ok(rollback) => rollback,
                Err(message) => {
                    return Err(RpcError {
                        code: error::INTERNAL_ERROR,
                        message,
                        data: None,
                    });
                }
            }
        } else {
            None
        };
        if let Err(e) = adapter.stage_add(config).await {
            let rollback_message =
                match meerkat::surface::rollback_mcp_persisted_mutation(rollback).await {
                    Ok(()) => String::new(),
                    Err(rollback_err) => format!("; persisted rollback failed: {rollback_err}"),
                };
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("{e}{rollback_message}"),
                data: None,
            });
        }
        Ok(rollback.is_some())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_remove(
        &self,
        session_id: &SessionId,
        server_name: String,
    ) -> Result<(), RpcError> {
        self.mcp_stage_remove_with_persistence(session_id, server_name, false)
            .await
            .map(|_| ())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_remove_with_persistence(
        &self,
        session_id: &SessionId,
        server_name: String,
        persisted: bool,
    ) -> Result<bool, RpcError> {
        self.ensure_session_exists(session_id).await?;
        if server_name.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }
        let adapter = self.mcp_adapter_for_session(session_id).await?;
        let rollback = if persisted {
            let (context_root, user_root) = self.skill_identity_roots();
            let authority =
                meerkat::surface::mcp_config_mutation_authority(context_root, user_root);
            match meerkat::surface::persist_mcp_remove_if_requested(true, &authority, &server_name)
                .await
            {
                Ok(rollback) => rollback,
                Err(message) => {
                    return Err(RpcError {
                        code: error::INTERNAL_ERROR,
                        message,
                        data: None,
                    });
                }
            }
        } else {
            None
        };
        if let Err(e) = adapter.stage_remove(server_name).await {
            let rollback_message =
                match meerkat::surface::rollback_mcp_persisted_mutation(rollback).await {
                    Ok(()) => String::new(),
                    Err(rollback_err) => format!("; persisted rollback failed: {rollback_err}"),
                };
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("{e}{rollback_message}"),
                data: None,
            });
        }
        Ok(rollback.is_some())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_reload(
        &self,
        session_id: &SessionId,
        server_name: Option<String>,
    ) -> Result<(), RpcError> {
        self.ensure_session_exists(session_id).await?;
        if let Some(name) = server_name.as_ref()
            && name.trim().is_empty()
        {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }
        let adapter = self.mcp_adapter_for_session(session_id).await?;
        match server_name {
            Some(name) => {
                // Validate server exists before staging to avoid deferred failures
                // at the next turn boundary.
                let active = adapter.active_server_names().await;
                if !active.iter().any(|n| n == &name) {
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: format!("MCP server '{name}' is not registered on this session"),
                        data: None,
                    });
                }
                adapter
                    .stage_reload(McpReloadTarget::ServerName(name))
                    .await
                    .map_err(|e| RpcError {
                        code: error::INTERNAL_ERROR,
                        message: e,
                        data: None,
                    })?;
            }
            None => {
                // Reload all active servers.
                let names = adapter.active_server_names().await;
                for name in names {
                    adapter
                        .stage_reload(McpReloadTarget::ServerName(name))
                        .await
                        .map_err(|e| RpcError {
                            code: error::INTERNAL_ERROR,
                            message: e,
                            data: None,
                        })?;
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn attach_mcp_adapter_for_pending_session(
        &self,
        session_id: SessionId,
        mut build_config: AgentBuildConfig,
    ) -> Result<AgentBuildConfig, RpcError> {
        let router = match &build_config.runtime_build_mode {
            meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => {
                McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface()))
            }
            meerkat_core::RuntimeBuildMode::StandaloneEphemeral => McpRouter::new(),
        };
        let adapter = Arc::new(McpRouterAdapter::new(router));
        let adapter_dispatcher: Arc<dyn AgentToolDispatcher> = adapter.clone();
        let combined = match build_config.external_tools.clone() {
            Some(existing) => Arc::new(
                ToolGateway::new(existing, Some(adapter_dispatcher)).map_err(|e| RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!("failed to compose external tools with MCP adapter: {e}"),
                    data: None,
                })?,
            ),
            None => adapter_dispatcher,
        };
        build_config.external_tools = Some(combined);

        let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel();
        let state = SessionMcpState {
            adapter,
            turn_counter: 0,
            lifecycle_tx,
            lifecycle_rx,
            drain_task_running: Arc::new(AtomicBool::new(false)),
        };
        self.mcp_sessions.write().await.insert(session_id, state);
        Ok(build_config)
    }

    #[cfg(feature = "mcp")]
    async fn ensure_session_exists(&self, session_id: &SessionId) -> Result<(), RpcError> {
        if self.staged_sessions.contains(session_id).await {
            return Ok(());
        }
        if self.session_state(session_id).await.is_none() {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn mcp_adapter_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRouterAdapter>, RpcError> {
        let map = self.mcp_sessions.read().await;
        map.get(session_id)
            .map(|state| state.adapter.clone())
            .ok_or_else(|| RpcError {
                code: error::INVALID_PARAMS,
                message: "session does not support live MCP operations".to_string(),
                data: None,
            })
    }

    #[cfg(feature = "mcp")]
    async fn apply_mcp_boundary(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        prompt: &mut String,
    ) -> Result<(), RpcError> {
        let (adapter, turn_number, drain_task_running, lifecycle_tx, mut queued_actions) = {
            let mut map = self.mcp_sessions.write().await;
            let state = match map.get_mut(session_id) {
                Some(state) => state,
                None => return Ok(()),
            };
            state.turn_counter = state.turn_counter.saturating_add(1);
            let mut queued = Vec::new();
            while let Ok(action) = state.lifecycle_rx.try_recv() {
                queued.push(action);
            }
            (
                state.adapter.clone(),
                state.turn_counter,
                state.drain_task_running.clone(),
                state.lifecycle_tx.clone(),
                queued,
            )
        };

        if !queued_actions.is_empty() {
            let drained = std::mem::take(&mut queued_actions);
            self.emit_mcp_lifecycle_actions(session_id, event_tx, prompt, turn_number, drained)
                .await;
        }

        let result = adapter.apply_staged().await.map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("failed to apply staged MCP operations: {e}"),
            data: None,
        })?;

        if result.delta.lifecycle_actions.iter().any(|action| {
            action.operation == ToolConfigChangeOperation::Remove
                && action.phase == McpLifecyclePhase::Draining
        }) {
            Self::spawn_mcp_drain_task_if_needed(adapter.clone(), drain_task_running, lifecycle_tx);
        }

        queued_actions.extend(result.delta.lifecycle_actions);
        if queued_actions.is_empty() {
            return Ok(());
        }

        self.emit_mcp_lifecycle_actions(session_id, event_tx, prompt, turn_number, queued_actions)
            .await;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn apply_mcp_boundary_to_turn_prompt(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        turn_prompt: &mut ContentInput,
    ) -> Result<(), RpcError> {
        let mut mcp_text = String::new();
        self.apply_mcp_boundary(session_id, event_tx, &mut mcp_text)
            .await?;
        if !mcp_text.is_empty() {
            let prompt = std::mem::replace(turn_prompt, ContentInput::Text(String::new()));
            let mut blocks = prompt.into_blocks();
            blocks.insert(
                0,
                meerkat_core::types::ContentBlock::Text { text: mcp_text },
            );
            *turn_prompt = ContentInput::Blocks(blocks);
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    fn spawn_mcp_drain_task_if_needed(
        adapter: Arc<McpRouterAdapter>,
        task_running: Arc<AtomicBool>,
        lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    ) {
        if task_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let delta = match adapter.progress_removals().await {
                    Ok(delta) => delta,
                    Err(err) => {
                        tracing::warn!("background MCP drain apply failed: {err}");
                        break;
                    }
                };

                for action in delta.lifecycle_actions {
                    let _ = lifecycle_tx.send(action);
                }

                match adapter.has_removing_servers().await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(err) => {
                        tracing::warn!("background MCP drain state check failed: {err}");
                        break;
                    }
                }
            }
            task_running.store(false, Ordering::Release);
        });
    }

    #[cfg(feature = "mcp")]
    async fn emit_mcp_lifecycle_actions(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        prompt: &mut String,
        turn_number: u32,
        actions: Vec<McpLifecycleAction>,
    ) {
        let source = meerkat_core::EventSourceIdentity::session(session_id.clone());
        meerkat::surface::emit_mcp_lifecycle_events(
            event_tx,
            &source,
            prompt,
            turn_number,
            actions,
        )
        .await;
    }
}

fn decode_wire_content_blocks(
    blocks: Option<Vec<meerkat_contracts::WireContentBlock>>,
) -> Result<Option<Vec<meerkat_core::types::ContentBlock>>, RpcError> {
    let Some(blocks) = blocks else {
        return Ok(None);
    };

    blocks
        .into_iter()
        .map(meerkat_core::types::ContentBlock::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
        .map_err(|message| RpcError {
            code: error::INVALID_PARAMS,
            message: message.to_string(),
            data: None,
        })
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn session_error_to_rpc(err: SessionError) -> RpcError {
    let code = match &err {
        SessionError::NotFound { .. } => error::SESSION_NOT_FOUND,
        SessionError::Busy { .. } => error::SESSION_BUSY,
        SessionError::NotRunning { .. } => error::INTERNAL_ERROR,
        SessionError::Agent(agent_err) => match agent_err {
            meerkat_core::AgentError::TokenBudgetExceeded { .. }
            | meerkat_core::AgentError::TimeBudgetExceeded { .. }
            | meerkat_core::AgentError::ToolCallBudgetExceeded { .. } => error::BUDGET_EXHAUSTED,
            meerkat_core::AgentError::ConfigError(_) => error::INVALID_PARAMS,
            meerkat_core::AgentError::HookDenied { .. } => error::HOOK_DENIED,
            meerkat_core::AgentError::Llm { .. } => error::PROVIDER_ERROR,
            meerkat_core::AgentError::SessionNotFound(_) => error::SESSION_NOT_FOUND,
            meerkat_core::AgentError::BuildError(_) => error::PROVIDER_ERROR,
            meerkat_core::AgentError::Cancelled => error::REQUEST_CANCELLED,
            meerkat_core::AgentError::InternalError(_) => error::INTERNAL_ERROR,
            _ => error::INTERNAL_ERROR,
        },
        _ => error::INTERNAL_ERROR,
    };
    let core_apply_failure_cause = match &err {
        SessionError::Agent(agent_err) => {
            use meerkat_core::lifecycle::core_executor::{
                CoreApplyFailureCause, CoreApplyFailureCauseKind,
            };
            let cause = CoreApplyFailureCause::from_agent_error(agent_err);
            match cause.kind {
                CoreApplyFailureCauseKind::HookDenied
                | CoreApplyFailureCauseKind::HookRuntimeFailure => Some(cause.kind.as_str()),
                _ => None,
            }
        }
        _ => None,
    };
    RpcError {
        code,
        message: err.to_string(),
        data: err.structured_data().or_else(|| {
            core_apply_failure_cause.map(|cause| {
                serde_json::json!({
                    "core_apply_failure_cause": cause
                })
            })
        }),
    }
}

fn rpc_error_to_session_error(err: RpcError, session_id: &SessionId) -> SessionError {
    match err.code {
        error::SESSION_NOT_FOUND => SessionError::NotFound {
            id: session_id.clone(),
        },
        error::SESSION_BUSY => SessionError::Busy {
            id: session_id.clone(),
        },
        _ => SessionError::Agent(meerkat_core::error::AgentError::InternalError(err.message)),
    }
}

fn completion_outcome_to_rpc_result(
    outcome: meerkat_runtime::completion::CompletionOutcome,
    session_id: &SessionId,
) -> Result<RunResult, RpcError> {
    use meerkat_runtime::completion::CompletionOutcome;

    match outcome {
        CompletionOutcome::Completed(result) => Ok(*result),
        CompletionOutcome::CompletedWithoutResult => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: "turn completed without result".to_string(),
            data: None,
        }),
        CompletionOutcome::CallbackPending { tool_name, args } => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("callback pending for tool '{tool_name}'"),
            data: Some(serde_json::json!({
                "session_id": session_id.to_string(),
                "resumable": true,
                "tool_name": tool_name,
                "args": args,
            })),
        }),
        CompletionOutcome::Cancelled => Err(RpcError {
            code: error::REQUEST_CANCELLED,
            message: "request cancelled".to_string(),
            data: None,
        }),
        CompletionOutcome::Abandoned(reason) => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("turn abandoned: {reason}"),
            data: None,
        }),
        CompletionOutcome::AbandonedWithError {
            reason,
            error: turn_error,
        } => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("turn abandoned: {reason}"),
            data: Some(serde_json::json!({
                "error": turn_error,
            })),
        }),
        CompletionOutcome::CompletedWithFinalizationFailure {
            result,
            error: turn_error,
        } => {
            let wire_result = meerkat_contracts::WireRunResult::from(*result);
            let structured_output = wire_result.structured_output.clone();
            Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: turn_error
                    .detail
                    .clone()
                    .unwrap_or_else(|| "turn finalization failed".to_string()),
                data: Some(serde_json::json!({
                    "error": turn_error,
                    "run_result": wire_result,
                    "structured_output": structured_output,
                })),
            })
        }
        CompletionOutcome::RuntimeTerminated(reason) => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("runtime terminated: {reason}"),
            data: None,
        }),
    }
}

fn system_context_error_to_rpc(err: SessionControlError) -> RpcError {
    match err {
        SessionControlError::Session(session_err) => session_error_to_rpc(session_err),
        SessionControlError::InvalidRequest { message } => RpcError {
            code: error::INVALID_PARAMS,
            message,
            data: None,
        },
        SessionControlError::Conflict { key, .. } => RpcError {
            code: error::INVALID_PARAMS,
            message: format!("system-context idempotency conflict for key '{key}'"),
            data: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // Phase 1a added ~6 DSL fields to MeerkatMachineState pushing several
    // test futures past the default 16384-byte stack budget. Tests only.
    clippy::large_futures
)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use futures::{StreamExt, stream};
    use meerkat::AgentBuildConfig;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::StopReason;
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceUuid,
    };
    use meerkat_core::skills_config::{
        SkillRepoTransport, SkillRepositoryConfig, SkillsConfig, SkillsIdentityConfig,
    };
    use meerkat_providers::auth_store::TokenStore;
    #[cfg(feature = "mcp")]
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};

    #[cfg(feature = "comms")]
    fn install_ephemeral_peer_request_response_authority(
        runtime: &Arc<meerkat::CommsRuntime>,
        session: &str,
    ) {
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test::initialize",
        )
        .expect("Initialize");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    session.to_string(),
                ),
            },
            "test::register_session",
        )
        .expect("RegisterSession");

        runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                    Arc::clone(&dsl),
                )),
                Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(dsl)),
            ),
        );
    }

    #[test]
    fn turn_metadata_from_overrides_does_not_stamp_execution_kind() {
        let metadata = SessionRuntime::turn_metadata_from_overrides(
            None,
            None,
            Some(vec!["runtime note".to_string()]),
            None,
            None,
        )
        .expect("additional instructions should produce metadata");

        assert_eq!(metadata.execution_kind, None);
    }

    #[test]
    fn runtime_prompt_metadata_helper_stamps_execution_kind() {
        let metadata = SessionRuntime::runtime_stamped_prompt_turn_metadata_from_overrides(
            None,
            None,
            Some(vec!["runtime note".to_string()]),
            None,
            None,
        );

        assert_eq!(
            metadata.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn realtime_open_config_context_comes_from_typed_system_context_state() {
        let mut state = SessionSystemContextState::default();
        state
            .stage_append(
                &AppendSystemContextRequest {
                    text: "Authoritative peer token is birch seventeen.".to_string(),
                    source: Some(
                        "peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
                            .to_string(),
                    ),
                    idempotency_key: Some("018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
                },
                meerkat_core::time_compat::SystemTime::UNIX_EPOCH,
            )
            .expect("append should stage");
        state.mark_pending_applied();

        let mut session = Session::new();
        session
            .set_system_context_state(state)
            .expect("state should serialize");

        let runtime_context = realtime_projection_runtime_system_context(&session);

        assert_eq!(runtime_context.len(), 1);
        assert_eq!(
            runtime_context[0].text,
            "Authoritative peer token is birch seventeen."
        );
        assert_eq!(
            runtime_context[0].source.as_deref(),
            Some("peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1")
        );
    }

    // -----------------------------------------------------------------------
    // Mock LLM client
    // -----------------------------------------------------------------------

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct CountingAgentLlmClient {
        inner: Arc<dyn meerkat_core::AgentLlmClient>,
        stream_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
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
            self.stream_calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.inner
                .stream_response(messages, tools, max_tokens, temperature, provider_params)
                .await
        }

        fn provider(&self) -> &'static str {
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
            constructions.fetch_add(1, AtomicOrdering::SeqCst);
            Arc::new(CountingAgentLlmClient {
                inner: client,
                stream_calls: Arc::clone(&stream_calls),
            })
        })
    }

    /// A mock LLM client that introduces a delay before responding.
    /// Used to test concurrent access (SESSION_BUSY).
    struct SlowMockLlmClient {
        delay_ms: u64,
    }

    impl SlowMockLlmClient {
        fn new(delay_ms: u64) -> Self {
            Self { delay_ms }
        }
    }

    #[async_trait]
    impl LlmClient for SlowMockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let delay_ms = self.delay_ms;
            Box::pin(async_stream::stream! {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Slow response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct BlockingMockLlmClient {
        calls: Arc<AtomicUsize>,
        release: Arc<Notify>,
    }

    struct FailingLifecycleRuntimeStore {
        inner: meerkat_runtime::InMemoryRuntimeStore,
        fail_lifecycle_once: AtomicBool,
    }

    impl FailingLifecycleRuntimeStore {
        fn new() -> Self {
            Self {
                inner: meerkat_runtime::InMemoryRuntimeStore::new(),
                fail_lifecycle_once: AtomicBool::new(false),
            }
        }

        fn fail_next_lifecycle_commit(&self) {
            self.fail_lifecycle_once
                .store(true, AtomicOrdering::Release);
        }
    }

    #[async_trait::async_trait]
    impl meerkat_runtime::RuntimeStore for FailingLifecycleRuntimeStore {
        async fn commit_session_snapshot(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
            session_delta: meerkat_runtime::store::SessionDelta,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
            session_delta: Option<meerkat_runtime::store::SessionDelta>,
            receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
            input_updates: Vec<meerkat_runtime::input_state::StoredInputState>,
            session_store_key: Option<SessionId>,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner
                .atomic_apply(
                    runtime_id,
                    session_delta,
                    receipt,
                    input_updates,
                    session_store_key,
                )
                .await
        }

        async fn load_input_states(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        ) -> Result<
            Vec<meerkat_runtime::input_state::StoredInputState>,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<
            Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, meerkat_runtime::RuntimeStoreError> {
            self.inner.load_session_snapshot(runtime_id).await
        }

        async fn persist_input_state(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
            state: &meerkat_runtime::input_state::StoredInputState,
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<
            Option<meerkat_runtime::input_state::StoredInputState>,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.inner.load_input_state(runtime_id, input_id).await
        }

        async fn load_runtime_state(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        ) -> Result<Option<RuntimeState>, meerkat_runtime::RuntimeStoreError> {
            self.inner.load_runtime_state(runtime_id).await
        }

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
            commit: meerkat_runtime::store::MachineLifecycleCommit,
            input_states: &[meerkat_runtime::input_state::StoredInputState],
        ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
            if self.fail_lifecycle_once.swap(false, AtomicOrdering::AcqRel) {
                return Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
                    "synthetic service-turn lifecycle commit failure".to_string(),
                ));
            }
            self.inner
                .commit_machine_lifecycle(runtime_id, commit, input_states)
                .await
        }
    }

    #[async_trait]
    impl LlmClient for BlockingMockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let calls = Arc::clone(&self.calls);
            let release = Arc::clone(&self.release);
            Box::pin(async_stream::stream! {
                calls.fetch_add(1, AtomicOrdering::SeqCst);
                release.notified().await;
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Blocked response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct BlockAfterFirstMockLlmClient {
        calls: Arc<AtomicUsize>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl LlmClient for BlockAfterFirstMockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            let release = Arc::clone(&self.release);
            Box::pin(async_stream::stream! {
                if call > 0 {
                    release.notified().await;
                }
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Slow response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct FailAfterFirstMockLlmClient {
        calls: AtomicUsize,
    }

    impl FailAfterFirstMockLlmClient {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl LlmClient for FailAfterFirstMockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if call == 0 {
                Box::pin(stream::iter(vec![
                    Ok(meerkat_client::LlmEvent::TextDelta {
                        delta: "first turn succeeds".to_string(),
                        meta: None,
                    }),
                    Ok(meerkat_client::LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ]))
            } else {
                Box::pin(stream::iter(vec![Err(LlmError::InvalidRequest {
                    message: "synthetic turn failure".to_string(),
                })]))
            }
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions"))
    }

    fn mock_build_config() -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(MockLlmClient)),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn test_auth_binding(realm: &str, binding: &str) -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse(realm).expect("valid realm fixture"),
            binding: meerkat_core::connection::BindingId::parse(binding)
                .expect("valid binding fixture"),
            profile: None,
        }
    }

    fn config_with_openai_managed_store_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_managed".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: "api_key".into(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_openai".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "openai_backend".into(),
                auth_profile: "openai_managed".into(),
                default_model: None,
                policy: Default::default(),
            },
        );
        section.default_binding = Some("default_openai".into());
        config.realm.insert("dev".into(), section);
        config
    }

    fn slow_build_config(delay_ms: u64) -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(SlowMockLlmClient::new(delay_ms))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn blocking_build_config() -> (AgentBuildConfig, Arc<AtomicUsize>, Arc<Notify>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let client = BlockingMockLlmClient {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
        };
        (
            AgentBuildConfig {
                llm_client_override: Some(Arc::new(client)),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            calls,
            release,
        )
    }

    fn block_after_first_build_config() -> (AgentBuildConfig, Arc<AtomicUsize>, Arc<Notify>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let client = BlockAfterFirstMockLlmClient {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
        };
        (
            AgentBuildConfig {
                llm_client_override: Some(Arc::new(client)),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            calls,
            release,
        )
    }

    fn fail_after_first_build_config() -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(FailAfterFirstMockLlmClient::new())),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    async fn wait_hook_reached(hook: &PendingPromotionPreTurnHook) {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if hook.reached_flag.load(AtomicOrdering::SeqCst) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "test hook was not reached before deadline"
            );
            tokio::select! {
                () = hook.reached.notified() => {}
                () = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {}
            }
        }
    }

    fn service_create_request(
        build_config: AgentBuildConfig,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: build_config.model.clone(),
            prompt: ContentInput::Text("service-created session".to_string()),
            render_metadata: None,
            system_prompt: build_config.system_prompt.clone(),
            max_tokens: build_config.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build_config.to_session_build_options()),
            labels: None,
        }
    }

    fn service_start_turn_request(prompt: &str) -> StartTurnRequest {
        StartTurnRequest {
            prompt: ContentInput::Text(prompt.to_string()),
            system_prompt: None,
            event_tx: None,
            runtime: StartTurnRuntimeSemantics::runtime_metadata(
                SessionRuntime::runtime_stamped_prompt_turn_metadata_from_overrides(
                    None, None, None, None, None,
                ),
            ),
        }
    }

    fn service_resume_pending_request() -> StartTurnRequest {
        StartTurnRequest {
            prompt: ContentInput::Text(String::new()),
            system_prompt: None,
            event_tx: None,
            runtime: StartTurnRuntimeSemantics::runtime_metadata(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    execution_kind: Some(
                        meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    ),
                    ..Default::default()
                },
            ),
        }
    }

    fn runtime_content_turn_primitive() -> RunPrimitive {
        RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: Vec::new(),
            context_appends: Vec::new(),
            contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
            turn_metadata: Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    execution_kind: Some(
                        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    ),
                    ..Default::default()
                },
            ),
        })
    }

    fn runtime_resume_pending_primitive() -> RunPrimitive {
        RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: Vec::new(),
            context_appends: Vec::new(),
            contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
            turn_metadata: Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    execution_kind: Some(
                        meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    ),
                    ..Default::default()
                },
            ),
        })
    }

    async fn wait_for_llm_calls(calls: &AtomicUsize, expected: usize, description: &str) {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if calls.load(AtomicOrdering::SeqCst) >= expected {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{description} did not enter the LLM stream before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_runtime_unregistered(
        runtime: &SessionRuntime,
        session_id: &SessionId,
        description: &str,
    ) {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if !runtime.runtime_adapter.contains_session(session_id).await {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{description} runtime registration was not cleaned up before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }

    async fn start_blocking_capacity_turn(
        runtime: &Arc<SessionRuntime>,
    ) -> (
        SessionId,
        tokio::task::JoinHandle<Result<RunResult, RpcError>>,
        Arc<Notify>,
    ) {
        let (build_config, calls, release) = blocking_build_config();
        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .expect("create blocking session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(runtime);
        let session_for_turn = session_id.clone();
        let turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "block active capacity".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if calls.load(AtomicOrdering::SeqCst) >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "blocking turn did not enter the LLM stream before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        (session_id, turn, release)
    }

    async fn start_runtime_prompt_without_registration_lock(
        runtime: &Arc<SessionRuntime>,
        session_id: &SessionId,
        prompt: &str,
    ) -> tokio::task::JoinHandle<meerkat_runtime::CompletionOutcome> {
        let runtime = Arc::clone(runtime);
        let session_id = session_id.clone();
        let prompt = prompt.to_string();
        tokio::spawn(async move {
            let input = meerkat_runtime::Input::Prompt(
                meerkat_runtime::PromptInput::from_content_input(ContentInput::Text(prompt), None),
            );
            let input_id = input.id().clone();
            let admission = runtime
                .reserve_active_turn(&session_id)
                .await
                .expect("reserve active runtime prompt");
            runtime
                .ensure_runtime_executor(&session_id)
                .await
                .expect("ensure runtime executor");
            let registration = runtime
                .register_runtime_pre_admission(session_id.clone(), input_id.clone(), admission)
                .expect("register active runtime prompt pre-admission");
            let (outcome, handle) = runtime
                .runtime_adapter
                .accept_input_with_completion(&session_id, input)
                .await
                .expect("accept active runtime prompt");
            assert!(
                matches!(outcome, meerkat_runtime::AcceptOutcome::Accepted { .. }),
                "active runtime prompt should be accepted: {outcome:?}"
            );
            let Some(handle) = handle else {
                return meerkat_runtime::CompletionOutcome::CompletedWithoutResult;
            };
            let cleanup = runtime
                .spawn_runtime_pre_admission_cleanup_with_outcome(session_id, input_id, handle);
            registration.disarm();
            cleanup
                .await
                .expect("active runtime prompt cleanup should report completion")
        })
    }

    #[tokio::test]
    async fn context_only_runtime_apply_preserves_run_checkpoint_boundary() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (initial_event_tx, _initial_event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize runtime session".into(),
                initial_event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        let input_id = meerkat_core::lifecycle::InputId::new();
        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::RunCheckpoint,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-rpc-checkpoint-boundary".to_string(),
                    content: CoreRenderable::Text {
                        text: "checkpoint-only runtime context".to_string(),
                    },
                }],
                contributing_input_ids: vec![input_id.clone()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let (event_tx, _event_rx) = mpsc::channel(100);

        let output = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("context-only apply should succeed");

        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(output.receipt.contributing_input_ids, vec![input_id]);
    }

    #[tokio::test]
    async fn context_only_runtime_executor_consumes_runtime_pre_admission() {
        use meerkat_core::lifecycle::core_executor::CoreExecutor;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (initial_event_tx, _initial_event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize runtime session".into(),
                initial_event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let input_id = meerkat_core::lifecycle::InputId::new();
        let admission = runtime
            .reserve_active_turn(&session_id)
            .await
            .expect("reserve runtime pre-admission");
        runtime
            .insert_runtime_pre_admission(session_id.clone(), input_id.clone(), admission)
            .expect("insert runtime pre-admission");

        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::RunCheckpoint,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-rpc-pre-admitted-context-only".to_string(),
                    content: CoreRenderable::Text {
                        text: "pre-admitted context-only runtime apply".to_string(),
                    },
                }],
                contributing_input_ids: vec![input_id.clone()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let mut executor =
            crate::session_executor::SessionRuntimeExecutor::new(Arc::clone(&runtime), session_id);
        let output = executor
            .apply(RunId::new(), primitive)
            .await
            .expect("context-only executor should consume pre-admission");

        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(output.receipt.contributing_input_ids, vec![input_id]);
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed context-only apply should release pre-admission");
    }

    #[tokio::test]
    async fn context_only_runtime_apply_recovers_persisted_live_missing_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (initial_event_tx, _initial_event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable context-only session".into(),
                initial_event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "test must remove live runtime bindings before context-only recovery"
        );

        let input_id = meerkat_core::lifecycle::InputId::new();
        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::RunCheckpoint,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-rpc-recovered-live-missing".to_string(),
                    content: CoreRenderable::Text {
                        text: "recovered live-missing runtime context".to_string(),
                    },
                }],
                contributing_input_ids: vec![input_id.clone()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let (event_tx, _event_rx) = mpsc::channel(100);

        let output = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("context-only apply should recover persisted session");

        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(output.receipt.contributing_input_ids, vec![input_id]);
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "context-only recovery should recreate runtime bindings"
        );

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");
        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains("ctx-rpc-recovered-live-missing")
                    && text.contains("recovered live-missing runtime context")
            }),
            "recovered live session should include context-only append: {projected:?}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_recovers_persisted_live_missing_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (initial_event_tx, _initial_event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable realtime-open session".into(),
                initial_event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "test must remove live runtime bindings before realtime-open recovery"
        );

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime open should recover persisted live session");

        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "realtime open recovery should recreate runtime bindings"
        );
        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected
                .iter()
                .any(|text| text.contains("persist recoverable realtime-open session")),
            "recovered realtime open should seed from durable transcript: {projected:?}"
        );
    }

    #[tokio::test]
    async fn context_only_runtime_apply_respects_active_admission_capacity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create candidate session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize candidate".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize candidate session");

        let (_blocker, blocking_turn, release) = start_blocking_capacity_turn(&runtime).await;
        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::Immediate,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-rpc-capacity-boundary".to_string(),
                    content: CoreRenderable::Text {
                        text: "capacity-bounded context-only runtime apply".to_string(),
                    },
                }],
                contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let (event_tx, _event_rx) = mpsc::channel(100);
        let blocked = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "context-only runtime apply should respect active admission capacity: {blocked:?}"
        );

        release.notify_waiters();
        blocking_turn
            .await
            .expect("blocking turn task")
            .expect("blocking turn should finish");
    }

    #[tokio::test]
    async fn failed_context_only_runtime_apply_restores_staged_admission() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(
                mock_build_config(),
                None,
                Some(ContentInput::Text("deferred prompt".to_string())),
            )
            .await
            .expect("create staged session");

        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::Immediate,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-staged-restore".to_string(),
                    content: CoreRenderable::Text {
                        text: "context-only staged restore".to_string(),
                    },
                }],
                contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "context-only apply against staged live-missing session should fail not-found: {rejected:?}"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "failed context-only apply must restore staged admission capacity: {blocked:?}"
        );

        runtime
            .archive_session(&session_id)
            .await
            .expect("archive staged session");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("archive should release restored staged admission");
    }

    #[tokio::test]
    async fn archived_live_context_only_runtime_apply_rejects_before_capacity_admission() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create candidate session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize candidate".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize candidate session");

        let mut archived = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted candidate")
            .expect("candidate should be persisted");
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived authoritative snapshot");

        let (_blocker, blocking_turn, release) = start_blocking_capacity_turn(&runtime).await;
        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::Immediate,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-archived-before-capacity".to_string(),
                    content: CoreRenderable::Text {
                        text: "archived context-only apply".to_string(),
                    },
                }],
                contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "archived live context-only apply should reject before capacity admission: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "archived context-only rejection should unregister stale runtime bindings"
        );

        release.notify_waiters();
        blocking_turn
            .await
            .expect("blocking turn task")
            .expect("blocking turn should finish");
    }

    #[tokio::test]
    async fn archived_live_external_runtime_inputs_reject_and_unregister_before_recreate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create candidate session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize candidate".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize candidate session");

        let mut archived = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted candidate")
            .expect("candidate should be persisted");
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived authoritative snapshot");

        let rejected = runtime
            .accept_external_event_via_runtime(
                &session_id,
                "external_archived_input".to_string(),
                serde_json::json!({"archived": true}),
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "archived external runtime input should reject as not found: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "archived external runtime input must unregister stale runtime state"
        );

        let peer_rejected = runtime
            .accept_peer_response_terminal_via_runtime(
                &session_id,
                meerkat_core::comms::PeerId::new(),
                None,
                meerkat_core::PeerCorrelationId::from_uuid(uuid::Uuid::new_v4()),
                meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
                serde_json::json!({"archived": true}),
            )
            .await;
        assert!(
            peer_rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "archived peer terminal runtime input should reject as not found: {peer_rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "archived peer terminal runtime input must not recreate runtime state"
        );
    }

    #[tokio::test]
    async fn create_failure_after_prepare_bindings_unregisters_runtime_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = make_runtime(temp_factory(&temp), 1);
        *runtime
            .create_session_after_prepare_bindings_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some("synthetic post-prepare create failure".to_string());

        let failed = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("synthetic post-prepare")),
            "create should fail at the test hook after prepare_bindings: {failed:?}"
        );
        let leaked_session_id = runtime
            .create_session_after_prepare_bindings_session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("test hook should capture prepared session id");
        assert!(
            !runtime
                .runtime_adapter
                .contains_session(&leaked_session_id)
                .await,
            "failed create must unregister prepared runtime bindings"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("failed create should release admission capacity");
    }

    #[tokio::test]
    async fn realtime_open_config_carries_pending_system_context_as_typed_runtime_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    text: "Authoritative peer token is birch seventeen.".to_string(),
                    source: Some(
                        "peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
                            .to_string(),
                    ),
                    idempotency_key: Some(
                        "peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
                            .to_string(),
                    ),
                },
            )
            .await
            .expect("append_system_context");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        assert_eq!(open_config.runtime_system_context.len(), 1);
        assert_eq!(
            open_config.runtime_system_context[0].text,
            "Authoritative peer token is birch seventeen."
        );
        assert_eq!(
            open_config.runtime_system_context[0].source.as_deref(),
            Some("peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1")
        );
        let seed_system_text = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !seed_system_text.contains("[Runtime System Context]"),
            "runtime system context must travel through typed config, not marker seed text: {seed_system_text}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_projects_build_state_additional_instructions_into_root_system_message()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-build-state".to_string());
        build.system_prompt = Some("You are the realtime operator.".to_string());
        build.additional_instructions = Some(vec![
            "Remember user-provided codewords verbatim.".to_string(),
            "Use the most recent authoritative terminal peer response.".to_string(),
        ]);

        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let root_system = open_config
            .seed_messages
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("expected root system projection");

        assert!(
            root_system.contains("You are the realtime operator."),
            "expected root system prompt to preserve canonical build-state system prompt: {root_system}"
        );
        assert!(
            root_system.contains("[Session Build Instructions]"),
            "expected root system prompt to render durable build-state instructions: {root_system}"
        );
        assert!(
            root_system.contains("Remember user-provided codewords verbatim.")
                && root_system
                    .contains("Use the most recent authoritative terminal peer response."),
            "expected root system prompt to include all durable additional instructions: {root_system}"
        );
    }

    #[tokio::test]
    async fn recovery_restores_build_state_additional_instructions_into_realtime_projection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.system_prompt = Some("You are the recovered realtime operator.".to_string());
        build.additional_instructions = Some(vec![
            "Remember user-provided codewords verbatim.".to_string(),
            "Prefer the latest authoritative peer response over stale memory.".to_string(),
        ]);

        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard_live_session");
        runtime
            .runtime_adapter()
            .unregister_session(&session_id)
            .await;

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .try_recover_persisted_session(
                &session_id,
                "Recover".into(),
                event_tx,
                false,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("try_recover_persisted_session");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let root_system = open_config
            .seed_messages
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("expected root system projection after recovery");

        assert!(
            root_system.contains("You are the recovered realtime operator."),
            "expected recovered realtime projection to preserve canonical system prompt: {root_system}"
        );
        assert!(
            root_system.contains("[Session Build Instructions]"),
            "expected recovered realtime projection to render durable build-state instructions: {root_system}"
        );
        assert!(
            root_system.contains("Remember user-provided codewords verbatim.")
                && root_system
                    .contains("Prefer the latest authoritative peer response over stale memory."),
            "expected recovered realtime projection to include all durable additional instructions: {root_system}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_includes_runtime_owned_terminal_peer_response_projection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut base_runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        base_runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(base_runtime);

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-peer-response".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn_via_runtime(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let (_outcome, handle) = runtime
            .runtime_adapter()
            .accept_input_with_completion(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                            display_identity: Some("Analyst".to_string()),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "request_subject": "alpha beta gamma",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let open_config = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let open_config = runtime
                    .realtime_session_open_config(
                        &session_id,
                        meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                    )
                    .await
                    .expect("realtime_session_open_config");
                let projected = open_config
                    .seed_messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                    ) && text.contains("birch seventeen")
                }) {
                    break open_config;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("terminal peer response should reach realtime projection");

        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains(
                    "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                ) && text.contains("birch seventeen")
            }),
            "expected realtime projection to include runtime-owned terminal peer response: {projected:?}"
        );
        assert!(
            open_config.runtime_system_context.iter().any(|append| {
                append.source.as_deref().is_some_and(|source| {
                    source.contains(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                    )
                }) && append.text.contains("birch seventeen")
            }),
            "expected realtime projection to carry terminal peer response as typed runtime context: {:?}",
            open_config.runtime_system_context
        );
    }

    #[tokio::test]
    async fn recovery_restores_runtime_owned_terminal_peer_response_from_authoritative_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut base_runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        base_runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(base_runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn_via_runtime(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        runtime
            .runtime_adapter()
            .accept_input(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                            display_identity: Some("Analyst".to_string()),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "request_subject": "alpha beta gamma",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response");

        let authoritative = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Some(session) = runtime
                    .load_persisted_session(&session_id)
                    .await
                    .expect("load authoritative session")
                else {
                    panic!("authoritative session missing");
                };
                let projected = session
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                    ) && text.contains("birch seventeen")
                }) {
                    break session;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("authoritative snapshot should include terminal peer response");

        let authoritative_projected = authoritative
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            authoritative_projected.iter().any(|text| {
                text.contains(
                    "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                ) && text.contains("birch seventeen")
            }),
            "expected authoritative snapshot to include runtime-owned terminal peer response: {authoritative_projected:?}"
        );

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard_live_session");

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .try_recover_persisted_session(
                &session_id,
                "Recover".into(),
                event_tx,
                false,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("try_recover_persisted_session");

        let open_config = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let open_config = runtime
                    .realtime_session_open_config(
                        &session_id,
                        meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                    )
                    .await
                    .expect("realtime_session_open_config");
                let projected = open_config
                    .seed_messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                    ) && text.contains("birch seventeen")
                }) {
                    break open_config;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("recovered realtime projection should include terminal peer response");

        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains(
                    "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                ) && text.contains("birch seventeen")
            }),
            "expected recovered realtime projection to include authoritative terminal peer response: {projected:?}"
        );
    }

    #[tokio::test]
    async fn accept_input_with_completion_persists_runtime_owned_terminal_peer_response() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn_via_runtime(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let (_outcome, handle) = runtime
            .runtime_adapter()
            .accept_input_with_completion(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                            display_identity: Some("Analyst".to_string()),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "request_subject": "alpha beta gamma",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response with completion");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let authoritative = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Some(session) = runtime
                    .load_persisted_session(&session_id)
                    .await
                    .expect("load authoritative session")
                else {
                    panic!("authoritative session missing");
                };
                let projected = session
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains(
                        "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                    ) && text.contains("birch seventeen")
                }) {
                    break session;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("authoritative snapshot should include terminal peer response");

        let projected = authoritative
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains(
                    "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                ) && text.contains("birch seventeen")
            }),
            "expected accept_input_with_completion to persist runtime-owned terminal peer response: {projected:?}"
        );
    }

    #[tokio::test]
    async fn accept_input_with_completion_emits_run_completed_for_runtime_owned_terminal_peer_response()
     {
        use futures::StreamExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("runtime-owned-terminal-peer-response-events".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn_via_runtime(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let mut events = runtime
            .service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");

        let (_outcome, handle) = runtime
            .runtime_adapter()
            .accept_input_with_completion(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                            display_identity: Some("Analyst".to_string()),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "request_subject": "alpha beta gamma",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response with completion");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let (result, usage) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let mut saw_context_started = false;
            while let Some(event) = events.next().await {
                match event.payload {
                    AgentEvent::RunStarted { prompt, .. } => {
                        let normalized = prompt.text_content().to_lowercase();
                        if normalized.contains(
                            "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                        ) {
                            assert!(
                                normalized.contains("birch seventeen"),
                                "run_started prompt should expose authoritative terminal peer payload: {normalized}"
                            );
                            saw_context_started = true;
                        }
                    }
                    AgentEvent::RunCompleted { result, usage, .. } if saw_context_started => {
                        return (result, usage);
                    }
                    _ => {}
                }
            }
            panic!("committed runtime context lifecycle events did not arrive");
        })
        .await
        .expect("runtime context lifecycle timeout");

        assert!(
            result.is_empty(),
            "context-only runtime apply should not synthesize assistant output: {result:?}"
        );
        assert_eq!(
            usage,
            meerkat_core::types::Usage::default(),
            "context-only runtime apply should not report model usage"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn keep_alive_comms_drain_emits_run_completed_for_terminal_peer_response() {
        use futures::StreamExt;
        use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
        use meerkat_core::comms::{CommsCommand, PeerName, PeerRoute, TrustedPeerDescriptor};
        use meerkat_core::interaction::InteractionId;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let operator_name = "operator-drain-test";
        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some(operator_name.to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn_via_runtime(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let operator_comms = runtime
            .comms_runtime(&session_id)
            .await
            .expect("session comms runtime");
        runtime
            .enable_comms_drain(&session_id, operator_comms.clone())
            .await;

        let sender = Arc::new(
            meerkat::CommsRuntime::inproc_only("analyst-drain-test").expect("sender comms runtime"),
        );
        install_ephemeral_peer_request_response_authority(&sender, "analyst-drain-test");
        let sender_public_key = sender.public_key();
        let sender_peer_id = sender_public_key.to_peer_id().to_string();
        let sender_addr = sender.advertised_address();
        let operator_peer_id = operator_comms.public_key().expect("operator peer id");
        let operator_addr = operator_comms
            .advertised_address()
            .expect("operator advertised address");
        let operator_pubkey =
            meerkat_comms::PubKey::from_pubkey_string(&operator_peer_id).expect("operator pubkey");

        CoreCommsRuntime::add_trusted_peer(
            &*sender,
            TrustedPeerDescriptor::unsigned_with_pubkey(
                operator_name,
                operator_pubkey.to_peer_id().to_string(),
                *operator_pubkey.as_bytes(),
                operator_addr,
            )
            .expect("operator trusted peer spec"),
        )
        .await
        .expect("sender trusts operator");
        CoreCommsRuntime::add_trusted_peer(
            operator_comms.as_ref(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "analyst-drain-test",
                sender_peer_id,
                *sender_public_key.as_bytes(),
                sender_addr,
            )
            .expect("sender trusted peer spec"),
        )
        .await
        .expect("operator trusts sender");

        let mut events = runtime
            .service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");

        let in_reply_to = InteractionId(uuid::Uuid::new_v4());
        sender
            .peer_interaction_handle()
            .expect("sender peer response authority")
            .request_received(meerkat_core::PeerCorrelationId::from_uuid(in_reply_to.0))
            .expect("seed inbound request before terminal peer response");

        CoreCommsRuntime::send(
            &*sender,
            CommsCommand::PeerResponse {
                to: PeerRoute::with_display_name(
                    operator_pubkey.to_peer_id(),
                    PeerName::new(operator_name.to_string()).expect("valid operator peer name"),
                ),
                in_reply_to,
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({
                    "request_intent": "checksum_token",
                    "request_subject": "alpha beta gamma",
                    "token": "birch seventeen",
                }),
                handling_mode: None,
            },
        )
        .await
        .expect("send terminal peer response");

        let (result, usage) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut saw_context_started = false;
            while let Some(event) = events.next().await {
                match event.payload {
                    AgentEvent::RunStarted { prompt, .. } => {
                        let normalized = prompt.text_content().to_lowercase();
                        if normalized.contains("peer_response_terminal:")
                            && normalized.contains("birch seventeen")
                        {
                            assert!(
                                normalized.contains("checksum_token"),
                                "run_started prompt should expose authoritative terminal peer payload: {normalized}"
                            );
                            saw_context_started = true;
                        }
                    }
                    AgentEvent::RunCompleted { result, usage, .. } if saw_context_started => {
                        return (result, usage);
                    }
                    _ => {}
                }
            }
            panic!("committed drain context lifecycle events did not arrive");
        })
        .await
        .expect("runtime context lifecycle timeout");

        assert!(
            result.is_empty(),
            "context-only drain apply should not synthesize assistant output"
        );
        assert_eq!(usage, meerkat_core::types::Usage::default());
    }

    #[tokio::test]
    async fn realtime_open_config_includes_runtime_context_applied_via_session_service() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-runtime-append".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: 018f6f79-7a82-7c4e-a552-a3b86f9630f1. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"request_subject\":\"alpha beta gamma\",\"token\":\"birch seventeen\"}. For checksum_token requests, the exact token answer is `birch seventeen`.".to_string(),
                    source: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
                    idempotency_key: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![],
            )
            .await
            .expect("apply_runtime_context_appends");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains(
                    "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                ) && text.contains("birch seventeen")
            }),
            "expected realtime projection to include runtime context applied via session service: {projected:?}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_seeds_from_durable_authority_when_live_export_fails_closed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut runtime = SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, Some(Arc::clone(&runtime_store)), blob_store),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-durable-authority".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "live uncommitted prompt".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let mut durable = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live export should carry recovery metadata before durable override");
        durable.push(Message::User(meerkat_core::types::UserMessage::text(
            "durable realtime seed".to_string(),
        )));
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        let live_export = runtime.service.export_live_session(&session_id).await;
        assert!(
            matches!(live_export, Err(SessionError::NotFound { .. })),
            "public live export should fail closed when durable authority must seed realtime open"
        );

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");
        let seed_user_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            seed_user_messages
                .iter()
                .any(|text| text.contains("durable realtime seed")),
            "realtime open config should seed from durable semantic authority: {seed_user_messages:?}"
        );
    }

    #[tokio::test]
    async fn runtime_turn_syncs_durable_authority_without_rebuilding_mob_owned_comms() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut runtime = SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, Some(Arc::clone(&runtime_store)), blob_store),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("mob-owned-stale-live-runtime-turn".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn_via_runtime(
                &session_id,
                "first runtime turn".into(),
                event_tx.clone(),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("initial runtime turn should materialize the live session");

        let comms_runtime = runtime
            .comms_runtime(&session_id)
            .await
            .expect("session comms runtime");
        let mob_id =
            meerkat_runtime::meerkat_machine::dsl::MobId::from("mob-owned-stale-live-sync");
        runtime
            .runtime_adapter()
            .maybe_spawn_mob_comms_drain(&session_id, comms_runtime, mob_id)
            .await;

        let mut durable = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live export should work before durable override");
        durable.push(Message::User(meerkat_core::types::UserMessage::text(
            "durable seed preserved across stale live sync".to_string(),
        )));
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        let result = runtime
            .start_turn_via_runtime(
                &session_id,
                "second runtime turn after durable authority wins".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("runtime turn must reuse live mechanics instead of rebuilding comms");
        assert!(
            result.text.contains("Hello from mock"),
            "unexpected runtime turn result: {:?}",
            result.text
        );

        let live = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live session should export after durable synchronization");
        assert!(
            live.messages().iter().any(|message| matches!(
                message,
                Message::User(user)
                    if user.text_content() == "durable seed preserved across stale live sync"
            )),
            "live session should carry durable semantic authority after runtime turn"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_seeds_runtime_context_from_durable_authority() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut runtime = SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, Some(Arc::clone(&runtime_store)), blob_store),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-durable-context".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let mut durable = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live export should be authoritative before durable context diverges");
        durable
            .set_system_context_state(SessionSystemContextState {
                applied: vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"request_subject\":\"alpha beta gamma\",\"token\":\"birch seventeen\"}.".to_string(),
                    source: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string()),
                    idempotency_key: Some("req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::UNIX_EPOCH,
                }],
                ..Default::default()
            })
            .expect("set durable system context state");
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        let live_export = runtime.service.export_live_session(&session_id).await;
        assert!(
            matches!(live_export, Err(SessionError::NotFound { .. })),
            "public live export should fail closed when durable runtime context diverges"
        );

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");
        assert!(
            open_config
                .runtime_system_context
                .iter()
                .any(|append| append.text.contains("birch seventeen")),
            "realtime open config should seed durable terminal peer context: {:?}",
            open_config.runtime_system_context
        );
    }

    #[tokio::test]
    async fn realtime_transcript_append_syncs_durable_context_without_dropping_live() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut runtime = SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, Some(Arc::clone(&runtime_store)), blob_store),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-transcript-context-sync".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let mut durable = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live export should be authoritative before durable context diverges");
        durable
            .set_system_context_state(SessionSystemContextState {
                applied: vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Result: {\"token\":\"birch seventeen\"}.".to_string(),
                    source: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string()),
                    idempotency_key: Some("req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::UNIX_EPOCH,
                }],
                ..Default::default()
            })
            .expect("set durable system context state");
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: serde_json::to_vec(&durable)
                        .expect("serialize durable session"),
                },
            )
            .await
            .expect("commit durable runtime snapshot");

        runtime
            .append_realtime_transcript_event(
                &session_id,
                meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: "item-user-1".to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    text: "realtime transcript after context".to_string(),
                },
            )
            .await
            .expect("realtime transcript append should sync durable context and keep live handle");

        assert!(
            runtime
                .service
                .has_live_session(&session_id)
                .await
                .expect("live-session status"),
            "durable context synchronization must not discard the realtime live handle"
        );
        let exported = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live export should be authoritative after transcript append persists");
        let projected_context = realtime_projection_runtime_system_context(&exported);
        assert!(
            projected_context
                .iter()
                .any(|append| append.text.contains("birch seventeen")),
            "durable runtime context should survive realtime transcript append: {projected_context:?}"
        );
        assert!(
            exported
                .messages()
                .iter()
                .any(|message| format!("{message:?}").contains("realtime transcript after context")),
            "realtime transcript append should persist against the synchronized live session: {exported:?}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_includes_runtime_backed_context_appends() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut runtime = SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, Some(Arc::clone(&runtime_store)), blob_store),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-runtime-backed-context".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let output = runtime
            .service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"request_subject\":\"alpha beta gamma\",\"token\":\"birch seventeen\"}.".to_string(),
                    source: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:req-123".to_string()),
                    idempotency_key: Some("req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::UNIX_EPOCH,
                }],
                vec![InputId::new()],
            )
            .await
            .expect("apply_runtime_context_appends");
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: output
                        .session_snapshot
                        .expect("runtime context output should carry a machine commit snapshot"),
                },
            )
            .await
            .expect("commit runtime context snapshot");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");
        assert!(
            open_config
                .runtime_system_context
                .iter()
                .any(|append| append.text.contains("birch seventeen")),
            "runtime-backed realtime open config should include durable context append: {:?}",
            open_config.runtime_system_context
        );
    }

    #[tokio::test]
    async fn realtime_open_config_preserves_committed_external_dialogue_for_reconstruction() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-dialogue-recap".to_string());
        build.additional_instructions = Some(vec![
            "Remember user-provided codewords verbatim.".to_string(),
            "Use authoritative terminal peer responses for token recall.".to_string(),
        ]);

        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .append_external_user_content(
                &session_id,
                meerkat_core::types::ContentInput::Text(
                    "Remember the codeword amber lantern.".to_string(),
                ),
            )
            .await
            .expect("append remember user turn");
        runtime
            .append_external_assistant_output(
                &session_id,
                vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Remembering amber lantern.".to_string(),
                    meta: None,
                }],
                meerkat_core::types::StopReason::EndTurn,
                meerkat_core::types::Usage::default(),
            )
            .await
            .expect("append remember assistant turn");
        runtime
            .append_external_user_content(
                &session_id,
                meerkat_core::types::ContentInput::Text("Ask analyst for the token.".to_string()),
            )
            .await
            .expect("append checksum request user turn");
        runtime
            .append_external_assistant_output(
                &session_id,
                vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Waiting for analyst token.".to_string(),
                    meta: None,
                }],
                meerkat_core::types::StopReason::EndTurn,
                meerkat_core::types::Usage::default(),
            )
            .await
            .expect("append checksum request assistant turn");
        runtime
            .service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: 018f6f79-7a82-7c4e-a552-a3b86f9630f1. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"request_subject\":\"alpha beta gamma\",\"token\":\"birch seventeen\"}. For checksum_token requests, the exact token answer is `birch seventeen`.".to_string(),
                    source: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
                    idempotency_key: Some("peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![],
            )
            .await
            .expect("apply runtime context");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let user_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let assistant_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::Assistant(assistant) => Some(assistant.content.clone()),
                Message::BlockAssistant(assistant) => {
                    Some(assistant.text_blocks().collect::<Vec<_>>().join("\n"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let system_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            user_messages
                .iter()
                .any(|text| text.contains("Remember the codeword amber lantern.")),
            "expected realtime projection to preserve remembered user turn: {user_messages:?}"
        );
        assert!(
            assistant_messages
                .iter()
                .any(|text| text.contains("Remembering amber lantern.")),
            "expected realtime projection to preserve remembered assistant turn: {assistant_messages:?}"
        );
        assert!(
            system_messages.iter().any(|text| {
                text.contains(
                    "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1",
                ) && text.contains("birch seventeen")
            }),
            "expected realtime projection to preserve authoritative token system context: {system_messages:?}"
        );
    }

    fn make_runtime(factory: AgentFactory, max_sessions: usize) -> SessionRuntime {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, None, blob_store),
            crate::router::NotificationSink::noop(),
        )
    }

    fn make_runtime_with_session_store(
        factory: AgentFactory,
        max_sessions: usize,
        store: Arc<dyn meerkat::SessionStore>,
    ) -> SessionRuntime {
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, None, blob_store),
            crate::router::NotificationSink::noop(),
        )
    }

    fn make_runtime_with_session_store_and_runtime_store(
        factory: AgentFactory,
        max_sessions: usize,
        store: Arc<dyn meerkat::SessionStore>,
    ) -> SessionRuntime {
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, Some(runtime_store), blob_store),
            crate::router::NotificationSink::noop(),
        )
    }

    fn make_runtime_with_runtime_store(
        factory: AgentFactory,
        max_sessions: usize,
    ) -> SessionRuntime {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, Some(runtime_store), blob_store),
            crate::router::NotificationSink::noop(),
        )
    }

    #[test]
    fn session_runtime_new_preserves_persistence_oauth_flow_authority() {
        let temp = tempfile::tempdir().unwrap();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let persistence = meerkat::PersistenceBundle::new(store, Some(runtime_store), blob_store);
        let adapter = persistence.runtime_adapter();
        let target = test_auth_binding("dev", "default_openai");
        let provider = meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt;
        let redirect_uri = "http://127.0.0.1:1455/callback";
        let state = adapter
            .oauth_flow_authority()
            .start(
                target.clone(),
                provider,
                redirect_uri.to_string(),
                "rpc-persistent-verifier".to_string(),
            )
            .expect("persistence authority admits OAuth flow before surface construction");

        let runtime = SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            persistence,
            crate::router::NotificationSink::noop(),
        );
        let flow = runtime
            .oauth_flow_authority()
            .consume(&state, &target, provider, redirect_uri)
            .expect("surface construction must preserve PersistenceBundle OAuth authority");

        assert_eq!(flow.pkce_verifier, "rpc-persistent-verifier");
    }

    struct ToggleFailSaveStore {
        inner: meerkat::MemoryStore,
        fail_save: AtomicBool,
        fail_after_saves: AtomicUsize,
    }

    impl ToggleFailSaveStore {
        fn new() -> Self {
            Self {
                inner: meerkat::MemoryStore::new(),
                fail_save: AtomicBool::new(false),
                fail_after_saves: AtomicUsize::new(usize::MAX),
            }
        }

        fn set_fail_save(&self, fail: bool) {
            self.fail_save.store(fail, AtomicOrdering::Release);
        }

        fn fail_after_successful_saves(&self, saves: usize) {
            self.fail_save.store(false, AtomicOrdering::Release);
            self.fail_after_saves.store(saves, AtomicOrdering::Release);
        }

        fn clear_failures(&self) {
            self.fail_save.store(false, AtomicOrdering::Release);
            self.fail_after_saves
                .store(usize::MAX, AtomicOrdering::Release);
        }
    }

    #[async_trait]
    impl meerkat::SessionStore for ToggleFailSaveStore {
        async fn save(&self, session: &Session) -> Result<(), meerkat_store::SessionStoreError> {
            if self.fail_save.load(AtomicOrdering::Acquire) {
                return Err(meerkat_store::SessionStoreError::Internal(
                    "forced save failure".to_string(),
                ));
            }
            loop {
                let remaining = self.fail_after_saves.load(AtomicOrdering::Acquire);
                if remaining == usize::MAX {
                    break;
                }
                if remaining == 0 {
                    return Err(meerkat_store::SessionStoreError::Internal(
                        "forced save failure".to_string(),
                    ));
                }
                if self
                    .fail_after_saves
                    .compare_exchange(
                        remaining,
                        remaining - 1,
                        AtomicOrdering::AcqRel,
                        AtomicOrdering::Acquire,
                    )
                    .is_ok()
                {
                    break;
                }
            }
            self.inner.save(session).await
        }

        async fn load(
            &self,
            id: &SessionId,
        ) -> Result<Option<Session>, meerkat_store::SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, meerkat_store::SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), meerkat_store::SessionStoreError> {
            self.inner.delete(id).await
        }
    }

    fn approval_request() -> meerkat_core::ApprovalRequest {
        meerkat_core::ApprovalRequest {
            requester: meerkat_core::ApprovalPrincipalId::new("human:alice").expect("principal"),
            owner: meerkat_core::ApprovalOwnerRef::Runtime,
            resource: meerkat_core::ApprovalResourceRef {
                kind: meerkat_core::ApprovalResourceKind::Runtime,
                id: "local".to_string(),
            },
            proposed_action: meerkat_core::ApprovalProposedAction {
                kind: meerkat_core::ApprovalActionKind::Other,
                summary: "manual gate".to_string(),
                body: None,
            },
            risk: meerkat_core::ApprovalRisk::Medium,
            request_body: None,
            allowed_decisions: std::collections::BTreeSet::from([
                meerkat_core::ApprovalDecision::Approve,
                meerkat_core::ApprovalDecision::Deny,
            ]),
            expires_at: None,
            metadata: meerkat_core::SurfaceMetadata::default(),
            request_provenance: None,
        }
    }

    #[tokio::test]
    async fn approval_service_reopens_from_realm_persistence_store_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_manifest, bundle) = meerkat::open_realm_persistence_in(
            temp.path(),
            "approval-realm",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await
        .expect("open persistence");
        let runtime = SessionRuntime::new(
            AgentFactory::new(temp.path().join("agent-sessions-a")),
            Config::default(),
            10,
            bundle,
            crate::router::NotificationSink::noop(),
        );
        assert!(runtime.approval_service().is_persistent());
        let record = runtime
            .approval_service()
            .request(approval_request())
            .expect("request approval");

        let (_manifest, reopened_bundle) = meerkat::open_realm_persistence_in(
            temp.path(),
            "approval-realm",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await
        .expect("reopen persistence");
        let reopened = SessionRuntime::new(
            AgentFactory::new(temp.path().join("agent-sessions-b")),
            Config::default(),
            10,
            reopened_bundle,
            crate::router::NotificationSink::noop(),
        );
        assert!(reopened.approval_service().is_persistent());
        let loaded = reopened
            .approval_service()
            .get(&record.approval_id)
            .expect("load approval after runtime reconstruction");
        assert_eq!(loaded.approval_id, record.approval_id);
        assert_eq!(loaded.status, meerkat_core::ApprovalStatus::Pending);
    }

    fn self_hosted_test_config(server: &str, inline_video: bool) -> Config {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            server.to_string(),
            meerkat_core::SelfHostedServerConfig {
                transport: meerkat_core::SelfHostedTransport::OpenAiCompatible,
                base_url: format!(
                    "http://127.0.0.1:{}",
                    if server == "local" { 11434 } else { 22434 }
                ),
                api_style: meerkat_core::SelfHostedApiStyle::ChatCompletions,
                bearer_token: None,
                bearer_token_env: None,
            },
        );
        config.self_hosted.models.insert(
            "gemma-4-e2b".to_string(),
            meerkat_core::SelfHostedModelConfig {
                server: server.to_string(),
                remote_model: "gemma4:e2b".to_string(),
                display_name: "Gemma 4 E2B".to_string(),
                family: "gemma-4".to_string(),
                tier: meerkat_models::ModelTier::Supported,
                context_window: Some(128_000),
                max_output_tokens: Some(8_192),
                vision: true,
                image_tool_results: true,
                inline_video,
                supports_temperature: true,
                supports_thinking: true,
                supports_reasoning: true,
                supports_web_search: false,
                call_timeout_secs: Some(600),
            },
        );
        config
    }

    fn inline_video_prompt() -> ContentInput {
        ContentInput::Blocks(vec![meerkat_core::ContentBlock::Video {
            media_type: "video/mp4".to_string(),
            duration_ms: 1000,
            data: meerkat_core::VideoData::Inline {
                data: "AAAA".to_string(),
            },
        }])
    }

    #[tokio::test]
    async fn provider_supports_inline_video_requires_provider_owned_profile() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let gemini_video_model_on_openai = SessionLlmIdentity {
            model: "gemini-3-flash-preview".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let gemini_video_model_on_gemini = SessionLlmIdentity {
            provider: meerkat_core::Provider::Gemini,
            ..gemini_video_model_on_openai.clone()
        };

        assert!(
            !runtime
                .provider_supports_inline_video(&gemini_video_model_on_openai)
                .await,
            "OpenAI identity must not inherit Gemini inline-video capability from model id alone"
        );
        assert!(
            runtime
                .provider_supports_inline_video(&gemini_video_model_on_gemini)
                .await,
            "owned Gemini inline-video capability should still resolve"
        );
    }

    #[tokio::test]
    async fn inline_video_validation_returns_typed_unsupported_capability_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let identity = SessionLlmIdentity {
            model: "gemini-3-flash-preview".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };

        let err = runtime
            .validate_prompt_video_input(&inline_video_prompt(), &identity)
            .await
            .expect_err("same model name under incompatible provider must fail closed");

        assert_eq!(err.code, error::INVALID_PARAMS);
        let data = err.data.expect("unsupported capability evidence data");
        assert_eq!(
            data["unsupported_capability"]["capability"],
            serde_json::json!("inline_video")
        );
        assert_eq!(
            data["unsupported_capability"]["reason"],
            serde_json::json!("provider_model_profile_missing")
        );
        assert_eq!(
            data["unsupported_capability"]["provider"],
            serde_json::json!("openai")
        );
        assert_eq!(
            data["unsupported_capability"]["model"],
            serde_json::json!("gemini-3-flash-preview")
        );
    }

    #[tokio::test]
    async fn reconfigure_capability_lookup_rejects_provider_model_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let host = runtime.llm_reconfigure_host();
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let request = SessionLlmReconfigureRequest {
            model: Some("gpt-5.4".to_string()),
            provider: Some("anthropic".to_string()),
            provider_params: None,
            clear_provider_params: false,
            auth_binding: None,
            clear_auth_binding: false,
        };

        let err = host
            .resolve_target_session_llm_identity(&request, &current)
            .await
            .expect_err("target model not owned by provider must fail closed");
        assert!(
            err.to_string().contains("anthropic") && err.to_string().contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {err}"
        );
    }

    #[tokio::test]
    async fn reconfigure_rejects_unknown_provider_model_identity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let host = runtime.llm_reconfigure_host();
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let request = SessionLlmReconfigureRequest {
            model: Some("unknown-model-xyz".to_string()),
            provider: Some("provider-shaped-cache-key".to_string()),
            provider_params: None,
            clear_provider_params: false,
            auth_binding: None,
            clear_auth_binding: false,
        };

        let err = host
            .resolve_target_session_llm_identity(&request, &current)
            .await
            .expect_err("unknown provider/model identity must fail closed");
        assert!(
            err.to_string().contains("unknown provider")
                && err.to_string().contains("provider-shaped-cache-key"),
            "error should reject the provider string before model fallback: {err}"
        );
    }

    #[tokio::test]
    async fn reconfigure_build_threads_auth_lease_for_managed_store_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let token_store = Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new());
        let factory = temp_factory(&temp).with_token_store(token_store.clone());
        let mut runtime = make_runtime(factory, 10);
        runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::new(meerkat_core::MemoryConfigStore::new(
                config_with_openai_managed_store_binding(),
            )),
            temp.path().join("config_state.json"),
        )));
        let auth_binding = test_auth_binding("dev", "default_openai");
        let tokens = meerkat_providers::auth_store::PersistedTokens::api_key("sk-hot-swap");
        token_store
            .save(
                &meerkat_providers::auth_store::TokenKey::from_auth_binding(&auth_binding),
                &tokens,
            )
            .await
            .unwrap();
        let auth_lease = runtime.auth_lease_handle();
        meerkat_core::publish_token_lifecycle_acquired(auth_lease.as_ref(), &auth_binding, &tokens)
            .expect("test credential lifecycle should be acquired");

        let host = runtime.llm_reconfigure_host();
        let identity = SessionLlmIdentity {
            model: "gpt-5.4".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(auth_binding),
        };

        host.build_adapter_for_llm_identity(&identity)
            .await
            .expect("hot-swap managed-store resolution should receive AuthMachine authority");
    }

    #[tokio::test]
    async fn reconfigure_build_adapter_applies_agent_llm_client_decorator() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let constructions = Arc::new(AtomicUsize::new(0));
        let stream_calls = Arc::new(AtomicUsize::new(0));
        runtime.set_agent_llm_client_decorator(Some(counting_agent_llm_client_decorator(
            Arc::clone(&constructions),
            Arc::clone(&stream_calls),
        )));

        let host = runtime.llm_reconfigure_host();
        let identity = SessionLlmIdentity {
            model: "gpt-5.4".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let adapter = host
            .build_adapter_for_llm_identity(&identity)
            .await
            .expect("hot-swap adapter should build through default client");
        adapter
            .stream_response(&[], &[], 64, None, None)
            .await
            .expect("decorated hot-swap adapter should remain usable");

        assert_eq!(constructions.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(stream_calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn pending_build_rejects_explicit_provider_that_contradicts_catalog_owner() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let mut build_config = AgentBuildConfig::new("gpt-5.4");
        build_config.provider = Some(meerkat_core::Provider::Anthropic);
        build_config.llm_client_override = Some(Arc::new(MockLlmClient));

        let err = runtime
            .llm_identity_from_pending_build(&build_config)
            .await
            .expect_err("pending explicit provider must match catalog owner");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("registered for provider 'openai'")
                && err.message.contains("not provider 'anthropic'")
                && err.message.contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn turn_model_override_without_provider_preserves_current_provider_for_owned_model() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("model-only override should resolve against the current provider");

        assert_eq!(
            resolved.provider,
            meerkat_core::Provider::Anthropic,
            "model-only turn overrides must not infer a new provider from model id alone"
        );
        assert_eq!(resolved.model, "claude-opus-4-6");
    }

    #[tokio::test]
    async fn turn_model_override_without_provider_rejects_other_provider_catalog_model() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        };

        let err = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect_err("model owned by another provider must fail closed");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("openai")
                && err.message.contains("anthropic")
                && err.message.contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn turn_explicit_provider_model_override_rejects_catalog_owner_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("anthropic".to_string()),
            ..Default::default()
        };

        let err = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect_err("explicit turn provider must match catalog owner");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("registered for provider 'openai'")
                && err.message.contains("not provider 'anthropic'")
                && err.message.contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn initial_model_only_build_preserves_catalog_provider_inference_compatibility() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let build_config = AgentBuildConfig {
            llm_client_override: Some(Arc::new(MockLlmClient)),
            ..AgentBuildConfig::new("gpt-5.4")
        };

        let identity = runtime
            .llm_identity_from_pending_build(&build_config)
            .await
            .expect("initial model-only build should infer catalog provider");

        assert_eq!(identity.provider, meerkat_core::Provider::OpenAI);
        assert_eq!(identity.model, "gpt-5.4");
    }

    #[cfg(feature = "mcp")]
    fn mcp_test_server_path() -> PathBuf {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let workspace_root = PathBuf::from(manifest_dir)
            .parent()
            .expect("workspace root")
            .to_path_buf();
        workspace_root
            .join("target")
            .join("debug")
            .join("mcp-test-server")
    }

    #[cfg(feature = "mcp")]
    fn maybe_mcp_server_config(server_name: &str) -> Option<McpServerConfig> {
        let path = mcp_test_server_path();
        if !path.exists() {
            eprintln!(
                "Skipping MCP runtime boundary test: mcp-test-server not built. Run `cargo build -p mcp-test-server` first."
            );
            return None;
        }
        Some(McpServerConfig::stdio(
            server_name,
            path.to_string_lossy().to_string(),
            Vec::new(),
            HashMap::new(),
        ))
    }

    #[tokio::test]
    async fn create_session_accepts_video_for_self_hosted_alias_from_runtime_config() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(self_hosted_test_config("local", true)),
        );
        runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        )));

        let session_id = runtime
            .create_session(
                AgentBuildConfig {
                    llm_client_override: Some(Arc::new(MockLlmClient)),
                    ..AgentBuildConfig::new("gemma-4-e2b")
                },
                None,
                Some(inline_video_prompt()),
            )
            .await
            .expect("self-hosted inline-video alias should validate against runtime registry");

        assert!(runtime.pending_session_exists(&session_id).await);
    }

    #[tokio::test]
    async fn create_session_pins_self_hosted_server_id_before_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(self_hosted_test_config("local", false)),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        ));
        runtime.set_config_runtime(config_runtime.clone());

        let session_id = runtime
            .create_session(
                AgentBuildConfig {
                    llm_client_override: Some(Arc::new(MockLlmClient)),
                    ..AgentBuildConfig::new("gemma-4-e2b")
                },
                None,
                None,
            )
            .await
            .expect("pending self-hosted session should stage");

        config_runtime
            .set(self_hosted_test_config("other", false), None)
            .await
            .expect("config patch");

        let staged = runtime
            .staged_sessions
            .effective_llm_identity(&session_id)
            .await
            .expect("pending session");
        assert_eq!(
            staged.self_hosted_server_id.as_deref(),
            Some("local"),
            "pending sessions must remain pinned to the server selected at create time"
        );
    }

    #[tokio::test]
    async fn provider_param_override_keeps_pinned_self_hosted_server_binding() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(self_hosted_test_config("local", false)),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        ));
        runtime.set_config_runtime(config_runtime.clone());

        config_runtime
            .set(self_hosted_test_config("other", false), None)
            .await
            .expect("config patch");

        let current = SessionLlmIdentity {
            model: "gemma-4-e2b".to_string(),
            provider: meerkat_core::Provider::SelfHosted,
            self_hosted_server_id: Some("local".to_string()),
            provider_params: None,
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(serde_json::json!({ "temperature": 0.2 })),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("provider-param override should resolve");

        assert_eq!(
            resolved.self_hosted_server_id.as_deref(),
            Some("local"),
            "provider-param overrides must preserve the pinned self-hosted server binding"
        );
    }

    #[tokio::test]
    async fn turn_override_auth_binding_propagates_to_resolved_identity() {
        // Dogma §10 (inherit/set) + Wave 3 row 15: an explicit
        // `auth_binding` override on a turn must flow through
        // `resolve_target_llm_identity` onto the resolved identity —
        // NOT be silently dropped to None. Guards against the earlier
        // bug where hot-swap inherited stale binding even when the
        // caller asked for a different realm.
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("tenant_a").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_default")
                    .expect("valid binding"),
                profile: None,
            }),
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("tenant_b").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_vip").expect("valid binding"),
                profile: None,
            }),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("auth_binding override should resolve");

        assert_eq!(
            resolved.auth_binding.as_ref().map(|c| c.realm.as_str()),
            Some("tenant_b"),
            "explicit auth_binding override must win over the session's current binding"
        );
        assert_eq!(
            resolved.auth_binding.as_ref().map(|c| c.binding.as_str()),
            Some("anthropic_vip"),
        );
    }

    #[tokio::test]
    async fn turn_without_auth_binding_override_preserves_current_binding() {
        // Dogma §10 inherit semantics: `None` on the override does
        // NOT mean "clear the binding" — it means "keep the current
        // one". The earlier bug returned None either way, breaking
        // multi-tenant sessions whose next turn happened to set
        // another override (model/provider_params) alongside no
        // explicit auth_binding.
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("tenant_a").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_default")
                    .expect("valid binding"),
                profile: None,
            }),
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(serde_json::json!({ "temperature": 0.1 })),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("resolve must succeed");

        assert_eq!(
            resolved.auth_binding.as_ref().map(|c| c.realm.as_str()),
            Some("tenant_a"),
            "absent auth_binding override must inherit the session's current binding, not drop it"
        );
    }

    #[tokio::test]
    async fn turn_clear_provider_params_removes_current_policy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: Some(serde_json::json!({ "temperature": 0.7 })),
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            clear_provider_params: true,
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("clear override should resolve");

        assert!(resolved.provider_params.is_none());
    }

    #[tokio::test]
    async fn turn_clear_auth_binding_removes_current_binding() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("tenant_a").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_default")
                    .expect("valid binding"),
                profile: None,
            }),
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            clear_auth_binding: true,
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("clear override should resolve");

        assert!(resolved.auth_binding.is_none());
    }

    #[tokio::test]
    async fn turn_rejects_set_and_clear_auth_binding_together() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("tenant_b").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_vip").expect("valid binding"),
                profile: None,
            }),
            clear_auth_binding: true,
            ..Default::default()
        };

        let err = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect_err("set+clear must be rejected");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(err.message.contains("clear_auth_binding"));
    }

    #[tokio::test]
    async fn turn_rejects_set_and_clear_provider_params_together() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(serde_json::json!({ "temperature": 0.2 })),
            clear_provider_params: true,
            ..Default::default()
        };

        let err = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect_err("set+clear must be rejected");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(err.message.contains("clear_provider_params"));
    }

    #[cfg(feature = "mcp")]
    fn collect_tool_config_events(
        event_rx: &mut mpsc::Receiver<EventEnvelope<AgentEvent>>,
    ) -> Vec<ToolConfigChangedPayload> {
        let mut out = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            if let AgentEvent::ToolConfigChanged { payload } = event.payload {
                out.push(payload);
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. Creating a session returns a SessionId and the state is Idle.
    #[tokio::test]
    async fn create_session_returns_id_and_idle_state() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let info = runtime.session_state(&session_id).await;
        assert_eq!(info.map(|i| i.state), Some(SessionState::Idle));
    }

    /// 2. Starting a turn returns a RunResult with expected text.
    #[tokio::test]
    async fn start_turn_returns_run_result() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(
            result.text.contains("Hello from mock"),
            "Expected mock response text, got: {}",
            result.text
        );
        assert_eq!(
            runtime
                .runtime_adapter()
                .runtime_state(&session_id)
                .await
                .expect("runtime state should be readable"),
            RuntimeState::Attached,
            "direct service start_turn must close its machine-owned run binding after durable return"
        );
        assert!(
            runtime
                .runtime_adapter()
                .list_active_inputs(&session_id)
                .await
                .expect("active inputs should be readable")
                .is_empty(),
            "direct service start_turn should not leave active runtime inputs"
        );
    }

    #[tokio::test]
    async fn direct_start_turn_lifecycle_commit_failure_fails_closed_before_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store = Arc::new(FailingLifecycleRuntimeStore::new());
        let runtime_store_dyn: Arc<dyn meerkat_runtime::RuntimeStore> = runtime_store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let runtime = Arc::new(SessionRuntime::new(
            temp_factory(&temp),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(store, Some(runtime_store_dyn), blob_store),
            crate::router::NotificationSink::noop(),
        ));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("first direct start_turn should commit");
        let durable_before = runtime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load durable session before failure")
            .expect("durable session should exist before failure");
        let durable_message_count = durable_before.messages().len();

        runtime_store.fail_next_lifecycle_commit();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let error = runtime
            .start_turn(
                &session_id,
                "second".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect_err("machine lifecycle commit failure must reject the direct turn");
        assert!(
            error
                .message
                .contains("synthetic service-turn lifecycle commit failure"),
            "unexpected error: {error:?}"
        );
        assert!(
            !runtime
                .service
                .has_live_session(&session_id)
                .await
                .expect("live status after failed commit"),
            "failed machine receipt must evict the uncommitted live turn"
        );
        let durable_after = runtime
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load durable session after failure")
            .expect("durable session should remain at previous snapshot");
        assert_eq!(
            durable_after.messages().len(),
            durable_message_count,
            "failed machine receipt must not persist the completed service turn snapshot"
        );
    }

    #[tokio::test]
    async fn append_system_context_survives_pending_session_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));
        let (build_config, calls, release) = blocking_build_config();

        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .unwrap();

        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = Arc::clone(&runtime);
        let sid_clone = session_id.clone();
        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".into(), event_tx, None, None, None, None)
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if calls.load(AtomicOrdering::SeqCst) > 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not start the service-side turn before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let append_req = AppendSystemContextRequest {
            text: "Coordinate with the orchestrator.".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-promotion".to_string()),
        };
        let append_result = runtime
            .append_system_context(&session_id, append_req.clone())
            .await
            .expect("append during promotion should succeed");
        assert_eq!(
            append_result.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        release.notify_waiters();
        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Blocked response"));

        let duplicate = runtime
            .append_system_context(&session_id, append_req)
            .await
            .expect("replayed append should be visible after materialization");
        assert_eq!(
            duplicate.status,
            meerkat_core::AppendSystemContextStatus::Duplicate
        );
    }

    #[tokio::test]
    async fn promotion_restore_preserves_system_context_appends() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let staged_sessions = Arc::new(StagedSessionRegistry::new());
        let mut build_config = mock_build_config();
        let session = Session::new();
        let session_id = session.id().clone();
        build_config.resume_session = Some(session);
        let effective_llm_identity = runtime
            .llm_identity_from_pending_build(&build_config)
            .await
            .expect("effective identity");
        let now = now_unix_secs();
        staged_sessions
            .stage(
                session_id.clone(),
                StagedSlot {
                    effective_llm_identity,
                    phase: StagedPhase::Staged {
                        build_config: Box::new(build_config),
                    },
                    labels: None,
                    deferred_prompt: None,
                    created_at_secs: now,
                    updated_at_secs: now,
                    machine_archived_resume_authorized: false,
                },
            )
            .await
            .expect("stage session");

        let slot = staged_sessions
            .begin_promotion(&session_id)
            .await
            .expect("begin promotion")
            .expect("promoting slot");
        let staged_capacity_admissions = Arc::new(StdMutex::new(HashMap::new()));
        let admission = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve staged capacity");
        let mut cleanup = PendingPromotionCleanup::new(
            Arc::clone(&staged_sessions),
            Arc::clone(&staged_capacity_admissions),
            &session_id,
            &slot,
            Some(admission),
        );
        let append = AppendSystemContextRequest {
            text: "Preserve this append across rollback.".to_string(),
            source: Some("test".to_string()),
            idempotency_key: Some("rollback-preserve".to_string()),
        };
        staged_sessions
            .append_system_context(
                &session_id,
                &append,
                meerkat_core::time_compat::SystemTime::now(),
                now_unix_secs(),
            )
            .await
            .expect("promoting slot should exist")
            .expect("append should stage");

        cleanup.restore_now().await;

        let restored = staged_sessions
            .begin_promotion(&session_id)
            .await
            .expect("begin restored promotion")
            .expect("restored slot should exist");
        let state = restored
            .build_config
            .resume_session
            .as_ref()
            .and_then(|session| session.system_context_state())
            .expect("restored system context state");
        assert!(
            state
                .pending
                .iter()
                .any(|pending| pending.text == append.text),
            "rollback must preserve appends made while promotion was in progress: {state:?}"
        );
    }

    /// 3. Verify state transitions: Idle -> Running -> Idle during a turn.
    #[tokio::test]
    async fn start_turn_transitions_idle_running_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        // Use a slow mock to give us time to observe Running state
        let session_id = runtime
            .create_session(slow_build_config(100), None, None)
            .await
            .unwrap();

        // Verify initial state
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle)
        );

        // Start the turn in a background task so we can observe state mid-run
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();

        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".into(), event_tx, None, None, None, None)
                .await
        });

        // Wait until the session transitions to Running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await.map(|i| i.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Wait for the turn to complete
        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        // After completion, state should be Idle again
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle),
            "Session should be Idle after turn completes"
        );
    }

    /// 4. Starting a second turn while one is running fails with SESSION_BUSY.
    #[tokio::test]
    async fn start_turn_on_busy_session_fails() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));
        let (build_config, calls, release) = blocking_build_config();

        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .unwrap();

        // Start first turn in background
        let (event_tx1, _rx1) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();
        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(
                    &sid_clone,
                    "First".into(),
                    event_tx1,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        // Wait until the first turn is definitely running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if calls.load(AtomicOrdering::SeqCst) > 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not start the service-side turn before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Try to start a second turn
        let (event_tx2, _rx2) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx2,
                None,
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_err(), "Second turn should fail");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            error::SESSION_BUSY,
            "Error code should be SESSION_BUSY, got: {}",
            err.code
        );

        release.notify_waiters();
        turn_handle
            .await
            .unwrap()
            .expect("first turn should finish after release");
    }

    /// 5. Agent events are forwarded through the per-turn event channel.
    #[tokio::test]
    async fn start_turn_emits_events() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let _result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Collect received events
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }

        // We should have received at least some events (RunStarted, TextDelta, etc.)
        assert!(
            !events.is_empty(),
            "Should have received at least one event"
        );

        // Check that we got a RunStarted event
        let has_run_started = events
            .iter()
            .any(|e| matches!(e.payload, AgentEvent::RunStarted { .. }));
        assert!(has_run_started, "Should have received a RunStarted event");
    }

    /// 6. Interrupting an idle session is a no-op (no error).
    #[tokio::test]
    async fn interrupt_on_idle_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Interrupt while idle should succeed without error
        let result = runtime.interrupt(&session_id).await;
        assert!(result.is_ok(), "Interrupt on idle should not fail");

        // State should still be idle
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle)
        );
    }

    #[tokio::test]
    async fn interrupt_on_service_owned_idle_session_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let build_config = mock_build_config();

        let created = runtime
            .service
            .create_session(CreateSessionRequest {
                model: build_config.model.clone(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build_config.to_session_build_options()),
                labels: None,
            })
            .await
            .expect("service-owned idle session should be created");

        assert!(
            !runtime
                .runtime_adapter
                .contains_session(&created.session_id)
                .await,
            "fixture should cover a service-owned session with no runtime adapter entry"
        );

        runtime
            .interrupt(&created.session_id)
            .await
            .expect("cold idle session interrupt should be a no-op");
    }

    #[test]
    fn interrupt_noop_target_for_presence_tracks_authoritative_presence_only() {
        assert_eq!(
            interrupt_noop_target_for_presence(true),
            InterruptNoopTarget::Present
        );
        assert_eq!(
            interrupt_noop_target_for_presence(false),
            InterruptNoopTarget::Missing
        );
    }

    #[tokio::test]
    async fn interrupt_on_cold_persisted_stopped_projection_is_noop_when_session_exists() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        let build_config = mock_build_config();

        let created = runtime
            .service
            .create_session(CreateSessionRequest {
                model: build_config.model.clone(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build_config.to_session_build_options()),
                labels: None,
            })
            .await
            .expect("service-owned idle session should be created");
        runtime
            .runtime_adapter
            .register_session(created.session_id.clone())
            .await;
        runtime
            .runtime_adapter
            .stop_runtime_executor(&created.session_id, "seed stopped projection")
            .await
            .expect("runtime state should persist");
        runtime
            .runtime_adapter
            .unregister_session(&created.session_id)
            .await;

        runtime
            .interrupt(&created.session_id)
            .await
            .expect("persisted stopped projection must not reject cold interrupt no-op");
    }

    /// 7. Archiving a session removes it from the runtime.
    #[tokio::test]
    async fn archive_session_removes_handle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Verify it exists
        assert!(runtime.session_state(&session_id).await.is_some());

        // Archive it
        runtime.archive_session(&session_id).await.unwrap();

        // Verify it's gone
        assert!(
            runtime.session_state(&session_id).await.is_none(),
            "Archived session should no longer exist"
        );

        // Archiving again should fail
        let result = runtime.archive_session(&session_id).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// set_mob_tools writes through to the builder, so sessions get mob tools.
    #[tokio::test]
    async fn set_mob_tools_delivers_tools_to_created_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(true)
            .mob(true);
        let mut runtime = make_runtime(factory, 10);

        // Create a MobMcpState and set it via set_mob_tools.
        let mob_svc = runtime.session_service();
        let mob_state = Arc::new(meerkat_mob_mcp::MobMcpState::new(mob_svc));
        runtime.set_mob_tools(Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
            mob_state,
        )));

        // Create a session — the agent should have mob tools.
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Read the session and check tool names. The session's tool list
        // should include mob tools (delegate, mob_create, etc.).
        let info = runtime.session_state(&session_id).await.unwrap();
        assert_eq!(info.state, SessionState::Idle);

        // Start a turn to materialize the agent, then check tool list
        // via a build that captures tool names.
        // (We can't directly inspect the agent's tools, but we can verify
        // the mob tools factory was injected by checking the builder slot.)
        let slot = runtime.builder_mob_tools_slot.read().unwrap();
        assert!(
            slot.is_some(),
            "builder_mob_tools_slot should be set after set_mob_tools"
        );
    }

    /// 8. Max sessions limit is enforced.
    #[tokio::test]
    async fn max_sessions_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 2);

        // Create two sessions (the max)
        let _s1 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let _s2 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Third should fail
        let result = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(result.is_err(), "Third session should fail");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("Max sessions"),
            "Error message should mention max sessions, got: {}",
            err.message
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_filters_archived_persisted_session_without_admission_leak() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();

        let mut archived = Session::new();
        let session_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived session");

        assert!(
            service
                .load_persisted_session(&session_id)
                .await
                .expect("load archived session through mob service")
                .is_none(),
            "mob service should hide archived persisted sessions"
        );

        let mut req = service_create_request(mock_build_config(), InitialTurnPolicy::Defer);
        req.build = Some(meerkat_core::service::SessionBuildOptions {
            resume_session: Some(archived),
            ..Default::default()
        });
        let rejected = service.create_session(req).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived resume through mob service should be rejected before admission: {rejected:?}"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("rejected archived mob resume should not leak admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_archived_start_turn_rejects_before_capacity_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let _active = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("fill active admission capacity");

        let mut archived = Session::new();
        let archived_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived store-only session");

        let rejected = service
            .start_turn(
                &archived_id,
                service_start_turn_request("archived mob bridge"),
            )
            .await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived mob start_turn should return not-found before capacity: {rejected:?}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_archived_runtime_apply_rejects_before_capacity_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let _active = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("fill active admission capacity");

        let mut archived = Session::new();
        let archived_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived store-only session");

        let rejected = service
            .apply_runtime_turn(
                &archived_id,
                RunId::new(),
                service_start_turn_request("archived mob bridge"),
                RunApplyBoundary::Immediate,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived mob runtime apply should return not-found before capacity: {rejected:?}"
        );
    }

    #[tokio::test]
    async fn archived_store_only_start_turn_rejects_before_capacity_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let _active = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("fill active admission capacity");

        let mut archived = Session::new();
        let archived_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived store-only session");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .start_turn(
                &archived_id,
                "archived store-only session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "archived store-only start_turn should return not-found before capacity: {rejected:?}"
        );
    }

    #[tokio::test]
    async fn archived_store_only_runtime_apply_rejects_before_capacity_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let _active = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("fill active admission capacity");

        let mut archived = Session::new();
        let archived_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived store-only session");

        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .apply_runtime_turn(
                &archived_id,
                RunId::new(),
                &primitive,
                ContentInput::Text("archived store-only session".to_string()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "archived store-only runtime apply should return not-found before capacity: {rejected:?}"
        );
    }

    #[tokio::test]
    async fn archived_store_only_runtime_routed_turn_rejects_before_registration() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = Arc::new(make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        ));

        let mut archived = Session::new();
        let archived_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived store-only session");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .start_turn_via_runtime(
                &archived_id,
                "archived runtime-routed session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "runtime-routed turn should return not-found before runtime registration: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&archived_id).await,
            "archived runtime-routed turn must not register runtime state"
        );
    }

    #[tokio::test]
    async fn invalid_external_event_rejects_before_runtime_registration() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = Arc::new(make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        meerkat::SessionStore::save(store.as_ref(), &session)
            .await
            .expect("save persisted session");

        let rejected = runtime
            .accept_external_event_via_runtime(
                &session_id,
                " ".to_string(),
                serde_json::json!({}),
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::INVALID_PARAMS),
            "invalid event type should reject before runtime registration: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "invalid external event must not register runtime state"
        );
    }

    #[tokio::test]
    async fn invalid_external_event_preserves_existing_runtime_registration() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "create_session should prepare runtime bindings"
        );

        let rejected = runtime
            .accept_external_event_via_runtime(
                &session_id,
                String::new(),
                serde_json::json!({}),
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::INVALID_PARAMS),
            "invalid event type should reject: {rejected:?}"
        );
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "invalid external event must not unregister existing runtime state"
        );
    }

    #[tokio::test]
    async fn external_event_accept_preserves_new_runtime_registration() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = Arc::new(make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        meerkat::SessionStore::save(store.as_ref(), &session)
            .await
            .expect("save persisted session");

        let accepted = runtime
            .accept_external_event_via_runtime(
                &session_id,
                "review.event".to_string(),
                serde_json::json!({"status": "queued"}),
                None,
            )
            .await
            .expect("external event should queue");
        assert!(
            matches!(accepted, meerkat_runtime::AcceptOutcome::Accepted { .. }),
            "external event should be accepted: {accepted:?}"
        );
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "external event should install runtime state"
        );

        runtime
            .unregister_new_runtime_registration_if_idle(
                &runtime.runtime_adapter,
                &session_id,
                false,
                false,
                true,
            )
            .await;
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "queued external event must protect the new runtime registration from stale cleanup"
        );
    }

    #[tokio::test]
    async fn runtime_routed_turn_capacity_full_rejects_before_input_accept() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before capacity check".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session and release active admission");

        let active_before = runtime
            .runtime_adapter
            .list_active_inputs(&session_id)
            .await
            .expect("list active inputs before rejected turn");
        assert!(
            active_before.is_empty(),
            "materialized idle session should have no active runtime inputs before rejection"
        );

        let _capacity_filler = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("fill active admission capacity");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .start_turn_via_runtime(
                &session_id,
                "must not enter runtime queue".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "capacity-full runtime-routed turn should reject before runtime input accept: {rejected:?}"
        );

        let active_after = runtime
            .runtime_adapter
            .list_active_inputs(&session_id)
            .await
            .expect("list active inputs after rejected turn");
        assert!(
            active_after.is_empty(),
            "capacity rejection must not enqueue runtime input: {active_after:?}"
        );
    }

    #[tokio::test]
    async fn runtime_submit_queued_same_session_input_extends_active_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let (build_config, calls, release) = blocking_build_config();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .expect("create staged blocking session");
        let first_turn = start_runtime_prompt_without_registration_lock(
            &runtime,
            &session_id,
            "first runtime turn blocks",
        )
        .await;
        wait_for_llm_calls(&calls, 1, "first runtime-routed turn").await;

        let queued_input = meerkat_runtime::peer_response_terminal_input(
            meerkat_core::comms::PeerId::new(),
            None,
            meerkat_core::PeerCorrelationId::from_uuid(uuid::Uuid::new_v4()),
            meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
            serde_json::json!({"token": "queued runtime submit should keep capacity"}),
        );
        let queued_runtime = Arc::clone(&runtime);
        let queued_session_id = session_id.clone();
        let queued_accept = tokio::spawn(async move {
            queued_runtime
                .accept_runtime_input_with_active_admission(
                    &queued_runtime.runtime_adapter,
                    &queued_session_id,
                    queued_input,
                )
                .await
        });
        release.notify_one();
        first_turn
            .await
            .expect("first runtime prompt task should not panic");
        let accepted = queued_accept
            .await
            .expect("queued runtime submit task should not panic")
            .expect("runtime submit should join running session admission");
        assert!(
            matches!(accepted, meerkat_runtime::AcceptOutcome::Accepted { .. }),
            "runtime submit should accept queued same-session input: {accepted:?}"
        );
        wait_for_llm_calls(&calls, 2, "queued runtime submit turn").await;
        let blocked = runtime.service.reserve_create_session_admission().await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "queued same-session runtime input must consume active capacity while running"
        );
        release.notify_one();
        tokio::time::timeout(tokio::time::Duration::from_secs(2), async {
            loop {
                match runtime.service.reserve_create_session_admission().await {
                    Ok(guard) => break guard,
                    Err(err) if err.to_string().contains("Max sessions") => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                    Err(err) => panic!("unexpected replacement admission error: {err:?}"),
                }
            }
        })
        .await
        .expect("queued runtime submit should eventually release active capacity");
    }

    #[tokio::test]
    async fn runtime_routed_turn_waits_for_registration_lock_before_active_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before lock ordering check".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session and release active admission");

        let registration_lock = runtime.runtime_registration_lock(&session_id);
        let registration_guard = registration_lock.mutex().lock().await;
        let turn_runtime = Arc::clone(&runtime);
        let turn_session_id = session_id.clone();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let mut turn_task = tokio::spawn(async move {
            turn_runtime
                .start_turn_via_runtime(
                    &turn_session_id,
                    "wait for registration lock before reserving".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        tokio::select! {
            result = &mut turn_task => {
                panic!("runtime-routed turn completed before registration lock release: {result:?}");
            }
            () = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => {}
        }

        let _capacity_filler = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("blocked runtime-routed turn must not reserve active capacity before lock");

        drop(registration_guard);
        drop(registration_lock);
        let rejected = tokio::time::timeout(tokio::time::Duration::from_secs(5), turn_task)
            .await
            .expect("runtime-routed turn should finish after lock release")
            .expect("runtime-routed turn task should not panic");
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "capacity filler should make the blocked runtime turn reject after lock release: {rejected:?}"
        );
    }

    #[tokio::test]
    async fn peer_response_terminal_capacity_full_rejects_before_input_accept() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before peer terminal capacity check".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session and release active admission");

        let active_before = runtime
            .runtime_adapter
            .list_active_inputs(&session_id)
            .await
            .expect("list active inputs before rejected peer terminal");
        assert!(
            active_before.is_empty(),
            "materialized idle session should have no active runtime inputs before rejection"
        );

        let _capacity_filler = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("fill active admission capacity");
        let rejected = runtime
            .accept_peer_response_terminal_via_runtime(
                &session_id,
                meerkat_core::comms::PeerId::new(),
                None,
                meerkat_core::PeerCorrelationId::from_uuid(uuid::Uuid::new_v4()),
                meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
                serde_json::json!({"token": "amber"}),
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "capacity-full peer terminal should reject before runtime input accept: {rejected:?}"
        );

        let active_after = runtime
            .runtime_adapter
            .list_active_inputs(&session_id)
            .await
            .expect("list active inputs after rejected peer terminal");
        assert!(
            active_after.is_empty(),
            "capacity rejection must not enqueue peer terminal input: {active_after:?}"
        );
    }

    #[tokio::test]
    async fn runtime_accept_validation_failure_unregisters_new_executor() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = Arc::new(make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        meerkat::SessionStore::save(store.as_ref(), &session)
            .await
            .expect("save store-only session");

        let input = meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
            header: meerkat_runtime::InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::Peer {
                    peer_id: meerkat_core::comms::PeerId::new().to_string(),
                    display_identity: Some("progress-peer".to_string()),
                    runtime_id: None,
                },
                durability: meerkat_runtime::InputDurability::Durable,
                visibility: meerkat_runtime::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(meerkat_runtime::PeerConvention::ResponseProgress {
                request_id: uuid::Uuid::new_v4().to_string(),
                phase: meerkat_runtime::ResponseProgressPhase::InProgress,
            }),
            body: "working".to_string(),
            payload: Some(serde_json::json!({"progress": "working"})),
            blocks: None,
            handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
        });

        let rejected = runtime
            .accept_runtime_input_with_active_admission(
                &runtime.runtime_adapter,
                &session_id,
                input,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::INVALID_PARAMS),
            "invalid runtime input should reject after executor installation: {rejected:?}"
        );
        wait_for_runtime_unregistered(&runtime, &session_id, "validation-failed runtime accept")
            .await;

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("accept validation failure should release active capacity");
    }

    #[tokio::test]
    async fn runtime_accept_terminal_no_handle_unregisters_new_executor() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = Arc::new(make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        ));

        let session = Session::new();
        let session_id = session.id().clone();
        meerkat::SessionStore::save(store.as_ref(), &session)
            .await
            .expect("save store-only session");

        let operation_id = meerkat_core::OperationId::new();
        let input = meerkat_runtime::Input::Operation(meerkat_runtime::OperationInput {
            header: meerkat_runtime::InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
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

        let outcome = runtime
            .accept_runtime_input_with_active_admission(
                &runtime.runtime_adapter,
                &session_id,
                input,
            )
            .await
            .expect("terminal operation input should be accepted without a completion handle");
        assert!(
            matches!(outcome, meerkat_runtime::AcceptOutcome::Accepted { .. }),
            "terminal operation input should be accepted: {outcome:?}"
        );
        wait_for_runtime_unregistered(&runtime, &session_id, "terminal no-handle runtime accept")
            .await;

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("terminal no-handle accept should leave active capacity available");
    }

    #[tokio::test]
    async fn new_runtime_cleanup_preserves_active_input() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let mut runtime = make_runtime(temp_factory(&temp), 2);
        runtime.set_default_llm_client(Some(Arc::new(BlockingMockLlmClient {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
        })));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "initial materialization".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("initial turn should complete");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure runtime executor");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let active_runtime = Arc::clone(&runtime);
        let active_session_id = session_id.clone();
        let active_turn = tokio::spawn(async move {
            active_runtime
                .start_turn_via_runtime(
                    &active_session_id,
                    "active input survives stale cleanup".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });
        wait_for_llm_calls(&calls, 1, "active runtime input").await;
        let active_inputs = runtime
            .runtime_adapter
            .list_active_inputs(&session_id)
            .await
            .expect("list active inputs");
        assert!(
            !active_inputs.is_empty(),
            "test requires an active runtime input before cleanup"
        );

        runtime
            .unregister_new_runtime_registration_if_idle(
                &runtime.runtime_adapter,
                &session_id,
                false,
                false,
                true,
            )
            .await;
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "new-runtime cleanup must not unregister a session with active runtime input"
        );

        release.notify_waiters();
        let result = tokio::time::timeout(tokio::time::Duration::from_secs(1), active_turn)
            .await
            .expect("active input should complete after release");
        result
            .expect("active turn task should not panic")
            .expect("active turn should not be terminated by cleanup");
    }

    #[tokio::test]
    async fn runtime_pre_admission_cleanup_releases_if_executor_never_takes_guard() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let session_id = SessionId::new();
        let input_id = InputId::new();
        let admission = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve active turn");
        runtime
            .insert_runtime_pre_admission(session_id.clone(), input_id.clone(), admission)
            .expect("insert pre-admission");

        let handle = meerkat_runtime::CompletionHandle::already_resolved(
            meerkat_runtime::CompletionOutcome::Abandoned(
                "executor never took pre-admission".to_string(),
            ),
        );
        runtime.spawn_runtime_pre_admission_cleanup(session_id, input_id, handle);

        let _replacement = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match runtime.service.reserve_create_session_admission().await {
                    Ok(guard) => break guard,
                    Err(err) if err.to_string().contains("Max sessions") => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(err) => panic!("unexpected reserve error: {err:?}"),
                }
            }
        })
        .await
        .expect("completion cleanup should release leaked pre-admission");
    }

    #[tokio::test]
    async fn runtime_pre_admission_cleanup_unregisters_after_boundary_commit_failure() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let session_id = SessionId::new();
        runtime
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare runtime bindings");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "test requires a runtime adapter entry before cleanup"
        );

        let input_id = InputId::new();
        let admission = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve active turn");
        runtime
            .insert_runtime_pre_admission(session_id.clone(), input_id.clone(), admission)
            .expect("insert pre-admission");

        let handle = meerkat_runtime::CompletionHandle::already_resolved(
            meerkat_runtime::CompletionOutcome::Abandoned(
                "apply failed: runtime boundary commit failed: injected failure".to_string(),
            ),
        );
        let outcome = runtime
            .spawn_runtime_pre_admission_cleanup_with_outcome(session_id.clone(), input_id, handle)
            .await
            .expect("cleanup task should report completion");
        assert!(
            matches!(outcome, meerkat_runtime::CompletionOutcome::Abandoned(_)),
            "test completion should be an apply failure: {outcome:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "boundary commit failure cleanup must unregister stale runtime state"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("failed runtime input cleanup should release active admission");
    }

    #[tokio::test]
    async fn runtime_pre_admission_registration_drop_releases_guard() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let session_id = SessionId::new();
        let input_id = InputId::new();
        let admission = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve active turn");
        let registration = runtime
            .register_runtime_pre_admission(session_id, input_id, admission)
            .expect("register pre-admission");

        drop(registration);

        let _replacement = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("dropping pre-admission registration should release active capacity");
    }

    #[tokio::test]
    async fn runtime_pre_admission_registration_drop_restores_staged_origin() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("staged session should reserve active capacity");
        let admission = runtime
            .reserve_active_turn(&session_id)
            .await
            .expect("runtime-routed staged turn should consume staged admission");
        let registration = runtime
            .register_runtime_pre_admission(session_id.clone(), InputId::new(), admission)
            .expect("register staged-origin runtime pre-admission");

        drop(registration);

        assert!(
            runtime.has_staged_capacity_admission(&session_id),
            "dropping staged-origin pre-admission registration must restore staged capacity"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "restored staged capacity must still occupy the original slot: {blocked:?}"
        );

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                ContentInput::Text("materialize after restored pre-admission".to_string()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("restored staged session should still materialize");
    }

    #[tokio::test]
    async fn stale_runtime_pre_admission_cleanup_does_not_remove_newer_input_guard() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let session_id = SessionId::new();
        let first_input_id = InputId::new();
        let first_admission = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve first active turn");
        runtime
            .insert_runtime_pre_admission(
                session_id.clone(),
                first_input_id.clone(),
                first_admission,
            )
            .expect("insert first pre-admission");

        let consumed = runtime
            .take_runtime_pre_admission(&session_id, std::slice::from_ref(&first_input_id))
            .expect("executor consumes first pre-admission");
        drop(consumed);

        let second_input_id = InputId::new();
        let second_admission = runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("reserve second active turn");
        runtime
            .insert_runtime_pre_admission(
                session_id.clone(),
                second_input_id.clone(),
                second_admission,
            )
            .expect("insert second pre-admission");

        let stale_handle = meerkat_runtime::CompletionHandle::already_resolved(
            meerkat_runtime::CompletionOutcome::Abandoned(
                "stale cleanup for first input".to_string(),
            ),
        );
        runtime.spawn_runtime_pre_admission_cleanup(
            session_id.clone(),
            first_input_id,
            stale_handle,
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let preserved =
            runtime.take_runtime_pre_admission(&session_id, std::slice::from_ref(&second_input_id));
        assert!(
            preserved.is_some(),
            "stale cleanup for a consumed input must not remove a newer input's guard"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn archived_store_only_rotation_registration_rejects_before_executor() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = Arc::new(make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        ));

        let mut archived = Session::new();
        let archived_id = archived.id().clone();
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived store-only session");

        let rejected = runtime
            .ensure_runtime_session_for_rotation(&archived_id)
            .await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "rotation registration should reject archived store-only session: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&archived_id).await,
            "archived rotation registration must not install runtime state"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_create_shares_rpc_staged_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let _rpc_staged = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("rpc staged session should reserve capacity");

        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await;
        assert!(
            direct
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "mob/direct service create must observe RPC staged capacity: {direct:?}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_staged_create_blocks_and_releases_rpc_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let service = runtime.session_service();

        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("mob/direct deferred create should reserve capacity");
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "rpc create should be blocked by mob/direct staged capacity: {blocked:?}"
        );

        service
            .archive(&direct.session_id)
            .await
            .expect("archiving direct staged session should release capacity");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("rpc create should succeed after direct staged archive");
    }

    #[cfg(feature = "mob")]
    struct DirectArchiveFailClearEventStore {
        inner: meerkat_mob::store::InMemoryMobEventStore,
        fail_clear: AtomicBool,
    }

    #[cfg(feature = "mob")]
    impl DirectArchiveFailClearEventStore {
        fn new() -> Self {
            Self {
                inner: meerkat_mob::store::InMemoryMobEventStore::new(),
                fail_clear: AtomicBool::new(true),
            }
        }

        fn allow_clear(&self) {
            self.fail_clear.store(false, AtomicOrdering::Relaxed);
        }
    }

    #[cfg(feature = "mob")]
    #[async_trait]
    impl meerkat_mob::store::MobEventStore for DirectArchiveFailClearEventStore {
        async fn append(
            &self,
            event: meerkat_mob::NewMobEvent,
        ) -> Result<meerkat_mob::MobEvent, meerkat_mob::store::MobStoreError> {
            self.inner.append(event).await
        }

        async fn append_terminal_event_if_absent(
            &self,
            event: meerkat_mob::NewMobEvent,
        ) -> Result<Option<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.append_terminal_event_if_absent(event).await
        }

        async fn append_batch(
            &self,
            events: Vec<meerkat_mob::NewMobEvent>,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.append_batch(events).await
        }

        async fn poll(
            &self,
            after_cursor: u64,
            limit: usize,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.poll(after_cursor, limit).await
        }

        async fn replay_all(
            &self,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.replay_all().await
        }

        async fn latest_cursor(&self) -> Result<u64, meerkat_mob::store::MobStoreError> {
            self.inner.latest_cursor().await
        }

        fn subscribe(
            &self,
        ) -> Result<meerkat_mob::store::MobEventReceiver, meerkat_mob::store::MobStoreError>
        {
            self.inner.subscribe()
        }

        async fn clear(&self) -> Result<(), meerkat_mob::store::MobStoreError> {
            if self.fail_clear.load(AtomicOrdering::Relaxed) {
                return Err(meerkat_mob::store::MobStoreError::Internal(
                    "forced direct mob session archive clear failure".to_string(),
                ));
            }
            self.inner.clear().await
        }
    }

    #[cfg(feature = "mob")]
    async fn insert_direct_archive_partial_destroy_mob(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
        owner_session_id: &str,
    ) -> (meerkat_mob::MobId, Arc<DirectArchiveFailClearEventStore>) {
        let mob_id = meerkat_mob::MobId::from("direct-session-archive-partial-destroy");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        definition.mark_owner_bridge_session_indexed(owner_session_id);
        let events = Arc::new(DirectArchiveFailClearEventStore::new());
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_session_service(mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create direct-archive-owned mob with failing event clear");
        mob_state.mob_insert_handle(mob_id.clone(), handle).await;
        (mob_id, events)
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_archive_retry_returns_success_after_retained_cleanup_completes() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        let service = runtime.session_service();
        let mob_state = Arc::new(meerkat_mob_mcp::MobMcpState::new(service.clone()));
        runtime.set_mob_state(mob_state.clone());
        let created = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create direct session service session");
        let session_id = created.session_id;
        let (mob_id, events) =
            insert_direct_archive_partial_destroy_mob(&mob_state, &session_id.to_string()).await;

        let first = service
            .archive(&session_id)
            .await
            .expect_err("first direct archive should surface incomplete mob cleanup");
        assert!(
            matches!(first, SessionError::FailedWithData { .. }),
            "direct archive should return typed incomplete cleanup data: {first:?}"
        );
        let retry = service
            .archive(&session_id)
            .await
            .expect_err("retry should still report retained cleanup while fault persists");
        assert!(
            matches!(retry, SessionError::FailedWithData { .. }),
            "direct archive retry should not collapse to stale NotFound: {retry:?}"
        );

        events.allow_clear();
        service
            .archive(&session_id)
            .await
            .expect("retry after retained mob cleanup completes should return success");
        assert!(
            mob_state.handle_for(&mob_id).await.is_err(),
            "successful direct archive retry must remove the retained mob cleanup anchor"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_cancelled_deferred_create_after_ack_discards_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        let service = Arc::new(
            RpcMobSessionService::new(
                Arc::clone(&runtime.service),
                Arc::clone(&runtime.staged_sessions),
                Arc::clone(&runtime.runtime_adapter),
                Arc::clone(&runtime.mob_state),
            )
            .with_direct_create_after_ack_hook(Arc::clone(&hook)),
        );
        let task_service = Arc::clone(&service);
        let create_task = tokio::spawn(async move {
            task_service
                .create_session(service_create_request(
                    mock_build_config(),
                    InitialTurnPolicy::Defer,
                ))
                .await
        });

        wait_hook_reached(&hook).await;
        create_task.abort();
        hook.release.notify_waiters();
        let _ = create_task.await;

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if runtime
                    .create_session(mock_build_config(), None, None)
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled direct deferred create should release admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_archive_rejects_rpc_staged_session_without_releasing_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("rpc staged session should reserve capacity");
        let service = runtime.session_service();

        let archived = service.archive(&session_id).await;
        assert!(
            matches!(archived, Err(SessionError::Busy { .. })),
            "direct mob service must not archive an RPC-staged session: {archived:?}"
        );
        let discarded = service.discard_live_session(&session_id).await;
        assert!(
            matches!(discarded, Err(SessionError::Busy { .. })),
            "direct mob service must not discard an RPC-staged session: {discarded:?}"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "failed direct archive/discard must not release RPC staged capacity: {blocked:?}"
        );
        runtime
            .archive_session(&session_id)
            .await
            .expect("RPC archive should still own RPC-staged cleanup");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("RPC archive should release staged capacity");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn rpc_archive_direct_deferred_failure_keeps_admission_until_retry() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ToggleFailSaveStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");
        store.set_fail_save(true);

        let archived = runtime.archive_session(&direct.session_id).await;
        assert!(
            archived
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("forced save failure")),
            "RPC archive of direct deferred session should surface durable failure: {archived:?}"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "failed archive should keep direct deferred admission reserved: {blocked:?}"
        );

        store.clear_failures();
        runtime
            .archive_session(&direct.session_id)
            .await
            .expect("archive should succeed after durable store recovers");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("successful retry archive should release admission");
    }

    #[tokio::test]
    async fn failed_non_staged_archive_preserves_runtime_cleanup_state_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ToggleFailSaveStore::new());
        let mut runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            2,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before failed archive".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session");
        let _stream = runtime
            .subscribe_pending_session_events(&session_id)
            .await
            .expect("seed runtime cleanup state");
        assert!(
            runtime
                .pending_session_event_streams
                .lock()
                .await
                .contains_key(&session_id),
            "test should seed runtime cleanup state before archive"
        );

        store.set_fail_save(true);
        let archived = runtime.archive_session(&session_id).await;
        assert!(
            archived
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("forced save failure")),
            "failed archive should surface durable error: {archived:?}"
        );
        assert!(
            runtime
                .pending_session_event_streams
                .lock()
                .await
                .contains_key(&session_id),
            "failed archive must preserve runtime cleanup state for retry"
        );

        store.clear_failures();
        runtime
            .archive_session(&session_id)
            .await
            .expect("retry archive should succeed after store recovers");
        assert!(
            !runtime
                .pending_session_event_streams
                .lock()
                .await
                .contains_key(&session_id),
            "successful archive should remove runtime cleanup state"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn rpc_discard_direct_deferred_session_releases_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "direct deferred session should consume RPC admission: {blocked:?}"
        );

        runtime
            .discard_live_session(&direct.session_id)
            .await
            .expect("runtime discard should remove direct deferred live session");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("discarding direct deferred live session should release RPC admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn direct_deferred_stale_live_cleanup_releases_rpc_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "direct deferred session should consume RPC admission: {blocked:?}"
        );

        let mut archived = runtime
            .service
            .export_live_session(&direct.session_id)
            .await
            .expect("export live direct deferred session");
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save stale archived durable snapshot");

        let failed = service
            .start_turn(
                &direct.session_id,
                service_start_turn_request("stale direct deferred turn"),
            )
            .await;
        assert!(
            matches!(failed, Err(SessionError::NotFound { .. })),
            "stale archived direct deferred turn should surface not found after cleanup: {failed:?}"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("stale live cleanup should release direct deferred RPC admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn direct_deferred_runtime_start_turn_stale_archive_releases_rpc_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");

        let mut archived = runtime
            .service
            .export_live_session(&direct.session_id)
            .await
            .expect("export live direct deferred session");
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save stale archived durable snapshot");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let failed = runtime
            .start_turn(
                &direct.session_id,
                "must not recover archived direct session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "runtime start_turn should reject stale archived direct session: {failed:?}"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("stale archived runtime start_turn should release direct deferred admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn direct_deferred_runtime_apply_stale_archive_releases_rpc_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");

        let mut archived = runtime
            .service
            .export_live_session(&direct.session_id)
            .await
            .expect("export live direct deferred session");
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save stale archived durable snapshot");

        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let failed = runtime
            .apply_runtime_turn(
                &direct.session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text("must not recover archived direct session".to_string()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "runtime apply should reject stale archived direct session: {failed:?}"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("stale archived runtime apply should release direct deferred admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn direct_deferred_has_live_stale_cleanup_releases_rpc_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "direct deferred session should consume RPC admission: {blocked:?}"
        );

        let mut archived = runtime
            .service
            .export_live_session(&direct.session_id)
            .await
            .expect("export live direct deferred session");
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save stale archived durable snapshot");

        let live = service
            .has_live_session(&direct.session_id)
            .await
            .expect("probe stale live session");
        assert!(
            !live,
            "stale archived direct deferred liveness probe should report no live session"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("stale live liveness probe should release direct deferred RPC admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn direct_deferred_timestamp_only_projection_is_not_stale() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        let service = runtime.session_service();
        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("direct deferred create should reserve capacity");

        let mut projected = runtime
            .service
            .export_live_session(&direct.session_id)
            .await
            .expect("export live direct deferred session");
        projected.set_metadata("projection_only", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &projected)
            .await
            .expect("save timestamp-only durable projection");

        assert!(
            !runtime
                .live_session_is_stale(&direct.session_id)
                .await
                .expect("query timestamp-only stale predicate"),
            "timestamp-only durable projection must not evict live runtime mechanics"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "timestamp-only projection must not release direct deferred admission: {blocked:?}"
        );
    }

    #[tokio::test]
    async fn cancelled_staged_archive_finishes_background_cleanup_after_service_archive() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        *runtime
            .staged_archive_after_service_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hook));

        let runtime_for_archive = Arc::clone(&runtime);
        let session_for_archive = session_id.clone();
        let archive_task = tokio::spawn(async move {
            runtime_for_archive
                .archive_session(&session_for_archive)
                .await
        });
        wait_hook_reached(&hook).await;
        archive_task.abort();
        let _ = archive_task.await;
        hook.release.notify_waiters();

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            match runtime
                .create_session(mock_build_config(), None, None)
                .await
            {
                Ok(_) => break,
                Err(err) if err.message.contains("Max sessions") => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "background staged archive cleanup did not release admission before deadline"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                Err(err) => panic!("unexpected create error after cancelled archive: {err:?}"),
            }
        }
        assert!(
            runtime.session_state(&session_id).await.is_none(),
            "background staged archive cleanup should remove the archived staged slot"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "background staged archive cleanup should unregister prepared runtime bindings"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_pre_run_apply_restores_staged_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let service = runtime.session_service();

        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("mob/direct deferred create should reserve capacity");
        let output = service
            .apply_runtime_turn(
                &direct.session_id,
                RunId::new(),
                service_resume_pending_request(),
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await
            .expect("resume-pending without a boundary should return a typed terminal");
        assert!(
            matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)),
            "direct service resume-pending should surface the typed no-pending terminal: {output:?}"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "pre-run direct apply should restore staged admission instead of releasing capacity: {blocked:?}"
        );

        service
            .archive(&direct.session_id)
            .await
            .expect("restored direct staged session should be archivable");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("archiving restored direct staged session should release capacity");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_pre_run_apply_error_restores_staged_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let service = runtime.session_service();

        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("mob/direct deferred create should reserve capacity");
        let mut request = service_start_turn_request("missing runtime stamp");
        request.runtime.turn_metadata = None;
        let result = service
            .apply_runtime_turn(
                &direct.session_id,
                RunId::new(),
                request,
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
            )
            .await;
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("runtime_execution_kind not set")),
            "direct service apply should reject missing runtime metadata before the run: {result:?}"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "pre-run direct apply error should restore staged admission instead of releasing capacity: {blocked:?}"
        );

        service
            .archive(&direct.session_id)
            .await
            .expect("restored direct staged session should remain archivable");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("archiving restored direct staged session should release capacity");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_runtime_executor_consumes_runtime_pre_admission_without_double_reserve() {
        use meerkat_core::lifecycle::core_executor::CoreExecutor;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before mob runtime executor apply".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session");

        let primitive = runtime_content_turn_primitive();
        let input_id = primitive
            .contributing_input_ids()
            .first()
            .cloned()
            .expect("runtime primitive should carry input id");
        let admission = runtime
            .reserve_active_turn(&session_id)
            .await
            .expect("reserve runtime pre-admission");
        runtime
            .insert_runtime_pre_admission(session_id.clone(), input_id, admission)
            .expect("insert runtime pre-admission");

        let mut executor = crate::session_executor::MobRpcRuntimeExecutor::new(
            runtime.session_service(),
            Some(Arc::clone(&runtime)),
            session_id.clone(),
            crate::router::NotificationSink::noop(),
        );
        executor
            .apply(RunId::new(), primitive)
            .await
            .expect("mob runtime executor should use existing pre-admission");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed mob runtime executor apply should release active admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_runtime_executor_context_only_recovers_without_pre_admission() {
        use meerkat_core::lifecycle::core_executor::CoreExecutor;

        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before mob context-only recovery".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "test must remove live runtime bindings before mob context-only recovery"
        );

        let input_id = meerkat_core::lifecycle::InputId::new();
        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: RunApplyBoundary::RunCheckpoint,
                appends: Vec::new(),
                context_appends: vec![ConversationContextAppend {
                    key: "ctx-mob-recovered-live-missing".to_string(),
                    content: CoreRenderable::Text {
                        text: "mob recovered live-missing runtime context".to_string(),
                    },
                }],
                contributing_input_ids: vec![input_id.clone()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                        ),
                        ..Default::default()
                    },
                ),
            });
        let mut executor = crate::session_executor::MobRpcRuntimeExecutor::new(
            runtime.session_service(),
            Some(Arc::clone(&runtime)),
            session_id.clone(),
            crate::router::NotificationSink::noop(),
        );
        let output = executor
            .apply(RunId::new(), primitive)
            .await
            .expect("mob runtime executor should recover via SessionRuntime");

        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(output.receipt.contributing_input_ids, vec![input_id]);
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "mob context-only recovery should recreate runtime bindings"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_runtime_executor_materialized_deferred_first_turn_consumes_staged_pre_admission() {
        use meerkat_core::lifecycle::core_executor::CoreExecutor;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let service = runtime.session_service();

        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("mob/direct deferred create should reserve capacity");
        let primitive = runtime_content_turn_primitive();
        let input_id = primitive
            .contributing_input_ids()
            .first()
            .cloned()
            .expect("runtime primitive should carry input id");
        let admission = runtime
            .reserve_active_turn(&direct.session_id)
            .await
            .expect("reserve staged-origin runtime pre-admission");
        runtime
            .insert_runtime_pre_admission(direct.session_id.clone(), input_id, admission)
            .expect("insert runtime pre-admission");

        let mut executor = crate::session_executor::MobRpcRuntimeExecutor::new(
            runtime.session_service(),
            Some(Arc::clone(&runtime)),
            direct.session_id.clone(),
            crate::router::NotificationSink::noop(),
        );
        executor
            .apply(RunId::new(), primitive)
            .await
            .expect("mob runtime executor should materialize the deferred first turn");

        assert!(
            !runtime.has_staged_capacity_admission(&direct.session_id),
            "successful first-turn materialization must not restore staged capacity"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed materialized first turn should release active admission");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_runtime_executor_no_pending_restores_promoted_staged_pre_admission() {
        use meerkat_core::lifecycle::core_executor::CoreExecutor;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let service = runtime.session_service();

        let direct = service
            .create_session(service_create_request(
                mock_build_config(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("mob/direct deferred create should reserve capacity");
        let primitive = runtime_resume_pending_primitive();
        let input_id = primitive
            .contributing_input_ids()
            .first()
            .cloned()
            .expect("runtime primitive should carry input id");
        let admission = runtime
            .reserve_active_turn(&direct.session_id)
            .await
            .expect("reserve promoted runtime pre-admission");
        runtime
            .insert_runtime_pre_admission(direct.session_id.clone(), input_id, admission)
            .expect("insert runtime pre-admission");

        let mut executor = crate::session_executor::MobRpcRuntimeExecutor::new(
            runtime.session_service(),
            Some(Arc::clone(&runtime)),
            direct.session_id.clone(),
            crate::router::NotificationSink::noop(),
        );
        let output = executor
            .apply(RunId::new(), primitive)
            .await
            .expect("mob runtime executor should return typed no-pending output");
        assert!(
            matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)),
            "resume-pending without a boundary should surface the typed no-pending terminal: {output:?}"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "mob executor no-pending apply should restore staged admission instead of releasing capacity: {blocked:?}"
        );

        service
            .archive(&direct.session_id)
            .await
            .expect("restored direct staged session should remain archivable");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("archiving restored direct staged session should release capacity");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_running_turn_blocks_rpc_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let (build_config, calls, release) = blocking_build_config();
        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .expect("create session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_initial = Arc::clone(&runtime);
        let session_for_initial = session_id.clone();
        let initial_turn = tokio::spawn(async move {
            runtime_for_initial
                .start_turn(
                    &session_for_initial,
                    "materialize".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if calls.load(AtomicOrdering::SeqCst) >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "initial turn did not enter the LLM stream before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
        release.notify_waiters();
        initial_turn
            .await
            .expect("initial turn task")
            .expect("materialize session");

        let service = runtime.session_service();
        let session_for_turn = session_id.clone();
        let direct_turn = tokio::spawn(async move {
            service
                .start_turn(
                    &session_for_turn,
                    service_start_turn_request("direct service turn"),
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if calls.load(AtomicOrdering::SeqCst) >= 2 {
                break;
            }
            if direct_turn.is_finished() {
                let result = direct_turn.await.expect("direct turn task");
                panic!("direct service turn finished before reaching LLM stream: {result:?}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "direct service turn did not enter the LLM stream before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "rpc create should be blocked by mob/direct running turn: {blocked:?}"
        );

        release.notify_waiters();
        direct_turn
            .await
            .expect("direct turn task")
            .expect("direct service turn should finish");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_cannot_release_rpc_pending_first_turn_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let (build_config, calls, release) = blocking_build_config();
        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .expect("create pending RPC session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let pending_first_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "materialize via RPC".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        wait_for_llm_calls(&calls, 1, "RPC pending first turn").await;

        let service = runtime.session_service();
        let direct_turn = service
            .start_turn(
                &session_id,
                service_start_turn_request("direct service race"),
            )
            .await;
        assert!(
            matches!(direct_turn, Err(SessionError::Busy { .. })),
            "mob/direct start_turn must see RPC pending first turn as running: {direct_turn:?}"
        );

        let archive = service.archive(&session_id).await;
        assert!(
            matches!(archive, Err(SessionError::Busy { .. })),
            "mob/direct archive must not release an active RPC pending first turn: {archive:?}"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "RPC create should remain blocked while first turn is running: {blocked:?}"
        );

        release.notify_waiters();
        pending_first_turn
            .await
            .expect("pending first turn task")
            .expect("pending first turn should finish");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("capacity should release after pending first turn stops");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_session_service_cancelled_eager_create_holds_admission_until_finish() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let service = runtime.session_service();
        let (build_config, calls, release) = blocking_build_config();
        let create = tokio::spawn({
            let service = Arc::clone(&service);
            async move {
                service
                    .create_session(service_create_request(
                        build_config,
                        InitialTurnPolicy::RunImmediately,
                    ))
                    .await
            }
        });

        wait_for_llm_calls(&calls, 1, "mob/direct eager create").await;
        create.abort();
        assert!(
            create
                .await
                .expect_err("outer create task should abort")
                .is_cancelled(),
            "outer create task should be cancelled"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "cancelled mob/direct create awaiter must not release running create capacity: {blocked:?}"
        );

        release.notify_waiters();
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(2);
        loop {
            match runtime
                .create_session(mock_build_config(), None, None)
                .await
            {
                Ok(_) => break,
                Err(err) if err.message.contains("Max sessions") => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "capacity did not release after service create finished"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                Err(err) => panic!("unexpected create_session error after cancellation: {err:?}"),
            }
        }
    }

    #[tokio::test]
    async fn completed_persisted_sessions_do_not_consume_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 2);

        for index in 0..4 {
            let session_id = runtime
                .create_session(mock_build_config(), None, None)
                .await
                .unwrap_or_else(|err| {
                    panic!("create_session {index} should ignore completed history: {err:?}")
                });
            let (event_tx, _event_rx) = mpsc::channel(100);
            runtime
                .start_turn(
                    &session_id,
                    format!("complete session {index}").into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .unwrap_or_else(|err| {
                    panic!("start_turn {index} should complete and release capacity: {err:?}")
                });
        }

        let listed = runtime.list_sessions(SessionQuery::default()).await;
        assert_eq!(
            listed.len(),
            4,
            "completed sessions should remain queryable as persisted history"
        );
    }

    #[tokio::test]
    async fn running_sessions_still_consume_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 2));

        let first = runtime
            .create_session(slow_build_config(250), None, None)
            .await
            .expect("create first session");
        let second = runtime
            .create_session(slow_build_config(250), None, None)
            .await
            .expect("create second session");

        let (event_tx1, _event_rx1) = mpsc::channel(100);
        let runtime_first = Arc::clone(&runtime);
        let first_for_task = first.clone();
        let first_turn = tokio::spawn(async move {
            runtime_first
                .start_turn(
                    &first_for_task,
                    "first".into(),
                    event_tx1,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let runtime_second = Arc::clone(&runtime);
        let second_for_task = second.clone();
        let second_turn = tokio::spawn(async move {
            runtime_second
                .start_turn(
                    &second_for_task,
                    "second".into(),
                    event_tx2,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            let first_running = runtime.session_state(&first).await.map(|info| info.state)
                == Some(SessionState::Running);
            let second_running = runtime.session_state(&second).await.map(|info| info.state)
                == Some(SessionState::Running);
            if first_running && second_running {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "sessions did not both enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let result = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(result.is_err(), "third session should fail while two run");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("Max sessions"),
            "Error message should mention max sessions, got: {}",
            err.message
        );

        first_turn.await.unwrap().expect("first turn should finish");
        second_turn
            .await
            .unwrap()
            .expect("second turn should finish");
    }

    #[tokio::test]
    async fn concurrent_create_session_admission_is_atomically_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 2));

        let results = futures::future::join_all((0..8).map(|_| {
            let runtime = Arc::clone(&runtime);
            async move {
                runtime
                    .create_session(mock_build_config(), None, None)
                    .await
            }
        }))
        .await;

        let successes = results.iter().filter(|result| result.is_ok()).count();
        let max_errors = results
            .iter()
            .filter(|result| {
                result
                    .as_ref()
                    .err()
                    .is_some_and(|err| err.message.contains("Max sessions"))
            })
            .count();
        assert_eq!(successes, 2, "only two creates should reserve capacity");
        assert_eq!(
            max_errors, 6,
            "remaining concurrent creates should be rejected by admission"
        );
    }

    #[tokio::test]
    async fn abandoned_staged_session_releases_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let staged = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked.is_err(),
            "capacity should be consumed while the session is staged"
        );

        runtime
            .archive_session(&staged)
            .await
            .expect("archive staged session");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("new session should be admitted after staged archive");
    }

    #[tokio::test]
    async fn archive_promoting_session_does_not_release_active_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(slow_build_config(250), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "first turn".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime
                .session_state(&session_id)
                .await
                .map(|info| info.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running promotion before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let archive = runtime.archive_session(&session_id).await;
        assert!(
            archive
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_BUSY),
            "archive during promotion should be rejected as busy: {archive:?}"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "promoting first turn should keep active capacity reserved: {blocked:?}"
        );

        running_turn
            .await
            .unwrap()
            .expect("promoting turn should finish");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("capacity should release after promotion completes");
    }

    #[tokio::test]
    async fn interrupt_reaches_promoting_pending_first_turn() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let (build_config, calls, _release) = blocking_build_config();
        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "interrupt me".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        wait_for_llm_calls(&calls, 1, "promoting pending first turn").await;
        assert!(
            runtime
                .staged_sessions
                .info(&session_id)
                .await
                .is_some_and(|info| info.is_promoting),
            "session should remain in promoting state while the service-side first turn runs"
        );

        runtime
            .interrupt(&session_id)
            .await
            .expect("interrupt should reach promoting live turn");
        let interrupted = tokio::time::timeout(tokio::time::Duration::from_secs(1), running_turn)
            .await
            .expect("promoting first turn should finish after interrupt")
            .expect("turn task should not panic")
            .expect_err("interrupted turn should return an RPC error");
        assert!(
            interrupted.message.contains("cancelled"),
            "interrupt should cancel the live first turn, got: {interrupted:?}"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("interrupted first turn should release admission");
    }

    #[tokio::test]
    async fn public_interrupt_during_pending_promotion_start_window_returns_busy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        *runtime
            .pending_promotion_pre_turn_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hook));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "interrupt in promotion window".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        while !hook.reached_flag.load(AtomicOrdering::SeqCst) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "pending promotion did not reach the pre-turn interrupt window"
            );
            hook.reached.notified().await;
        }

        let err = runtime
            .interrupt(&session_id)
            .await
            .expect_err("pre-running promotion should report busy instead of acknowledging cancel");
        assert_eq!(err.code, error::SESSION_BUSY);
        hook.release.notify_waiters();

        tokio::time::timeout(tokio::time::Duration::from_secs(1), running_turn)
            .await
            .expect("pending first turn should finish")
            .expect("turn task should not panic")
            .expect("busy pre-run interrupt must not cancel or corrupt the first turn");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed first turn should release active admission");
    }

    #[tokio::test]
    async fn turn_interrupt_handler_during_pending_promotion_start_window_returns_busy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        *runtime
            .pending_promotion_pre_turn_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hook));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "interrupt through RPC handler in promotion window".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        wait_hook_reached(&hook).await;
        let params = serde_json::value::to_raw_value(&serde_json::json!({
            "session_id": session_id.to_string(),
        }))
        .expect("serialize interrupt params");

        #[cfg(feature = "mob")]
        let mob_state = meerkat_mob_mcp::MobMcpState::new_in_memory();
        #[cfg(feature = "mob")]
        let response = crate::handlers::turn::handle_interrupt(
            Some(crate::protocol::RpcId::Num(294)),
            Some(params.as_ref()),
            runtime.as_ref(),
            mob_state.as_ref(),
        )
        .await;
        #[cfg(not(feature = "mob"))]
        let response = crate::handlers::turn::handle_interrupt(
            Some(crate::protocol::RpcId::Num(294)),
            Some(params.as_ref()),
            runtime.as_ref(),
        )
        .await;

        let err = response
            .error
            .expect("pre-running handler interrupt should return an error");
        assert_eq!(err.code, error::SESSION_BUSY);
        hook.release.notify_waiters();

        tokio::time::timeout(tokio::time::Duration::from_secs(1), running_turn)
            .await
            .expect("pending first turn should finish")
            .expect("turn task should not panic")
            .expect("busy handler interrupt must not cancel or corrupt the first turn");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed first turn should release active admission");
    }

    #[tokio::test]
    async fn turn_interrupt_handler_during_runtime_routed_pre_promotion_window_returns_busy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        *runtime
            .runtime_routed_pre_promotion_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hook));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn_via_runtime(
                    &session_for_turn,
                    "interrupt accepted runtime-routed first turn".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        wait_hook_reached(&hook).await;
        assert!(
            runtime
                .staged_sessions
                .info(&session_id)
                .await
                .is_some_and(|info| !info.is_promoting),
            "test must pause after runtime input accept but before staged promotion"
        );
        assert!(
            !runtime.has_staged_capacity_admission(&session_id),
            "runtime-routed accepted input must own staged capacity while waiting to promote"
        );

        let params = serde_json::value::to_raw_value(&serde_json::json!({
            "session_id": session_id.to_string(),
        }))
        .expect("serialize interrupt params");

        #[cfg(feature = "mob")]
        let mob_state = meerkat_mob_mcp::MobMcpState::new_in_memory();
        #[cfg(feature = "mob")]
        let response = crate::handlers::turn::handle_interrupt(
            Some(crate::protocol::RpcId::Num(295)),
            Some(params.as_ref()),
            runtime.as_ref(),
            mob_state.as_ref(),
        )
        .await;
        #[cfg(not(feature = "mob"))]
        let response = crate::handlers::turn::handle_interrupt(
            Some(crate::protocol::RpcId::Num(295)),
            Some(params.as_ref()),
            runtime.as_ref(),
        )
        .await;

        let err = response
            .error
            .expect("pre-promotion runtime-routed interrupt should return an error");
        assert_eq!(err.code, error::SESSION_BUSY);
        hook.release.notify_waiters();

        tokio::time::timeout(tokio::time::Duration::from_secs(1), running_turn)
            .await
            .expect("runtime-routed first turn should finish")
            .expect("turn task should not panic")
            .expect("busy pre-promotion interrupt must not cancel or corrupt the first turn");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed runtime-routed first turn should release active admission");
    }

    #[tokio::test]
    async fn yielding_interrupt_during_runtime_routed_pre_promotion_window_returns_busy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        *runtime
            .runtime_routed_pre_promotion_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hook));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn_via_runtime(
                    &session_for_turn,
                    "yielding interrupt runtime-routed first turn".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        wait_hook_reached(&hook).await;
        assert!(
            runtime
                .staged_sessions
                .info(&session_id)
                .await
                .is_some_and(|info| !info.is_promoting),
            "test must pause after runtime input accept but before staged promotion"
        );
        assert!(
            !runtime.has_staged_capacity_admission(&session_id),
            "runtime-routed accepted input must own staged capacity while waiting to promote"
        );

        let err = runtime
            .interrupt_yielding(&session_id)
            .await
            .expect_err("pre-promotion yielding interrupt should report busy");
        assert_eq!(err.code, error::SESSION_BUSY);
        hook.release.notify_waiters();

        tokio::time::timeout(tokio::time::Duration::from_secs(1), running_turn)
            .await
            .expect("runtime-routed first turn should finish")
            .expect("turn task should not panic")
            .expect("busy yielding interrupt must not cancel or corrupt the first turn");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed runtime-routed first turn should release active admission");
    }

    #[tokio::test]
    async fn public_interrupt_during_pending_promotion_apply_window_returns_busy() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let hook = Arc::new(PendingPromotionPreTurnHook::default());
        *runtime
            .pending_promotion_pre_turn_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&hook));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .apply_runtime_turn(
                    &session_for_turn,
                    RunId::new(),
                    &primitive,
                    ContentInput::Text("interrupt in apply window".to_string()),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        while !hook.reached_flag.load(AtomicOrdering::SeqCst) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "pending apply did not reach the pre-turn interrupt window"
            );
            hook.reached.notified().await;
        }

        let err = runtime.interrupt(&session_id).await.expect_err(
            "pre-running runtime apply should report busy instead of acknowledging cancel",
        );
        assert_eq!(err.code, error::SESSION_BUSY);
        hook.release.notify_waiters();

        tokio::time::timeout(tokio::time::Duration::from_secs(1), running_turn)
            .await
            .expect("pending apply should finish")
            .expect("apply task should not panic")
            .expect("busy pre-run interrupt must not cancel or corrupt runtime apply");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed runtime apply should release active admission");
    }

    #[tokio::test]
    async fn invalid_pending_turn_override_restores_staged_session_for_archive() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let err = runtime
            .start_turn(
                &session_id,
                "invalid override".into(),
                event_tx,
                None,
                None,
                None,
                Some(crate::handlers::turn::TurnOverrides {
                    provider: Some("openai".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .expect_err("provider-only override should fail before materialization");
        assert_eq!(err.code, error::INVALID_PARAMS);

        runtime
            .archive_session(&session_id)
            .await
            .expect("failed pending promotion should be restored and archivable");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("archive after failed promotion should release active admission");
    }

    #[tokio::test]
    async fn invalid_pending_start_keep_alive_override_does_not_persist() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let err = runtime
            .start_turn(
                &session_id,
                "invalid keep_alive".into(),
                event_tx,
                None,
                None,
                None,
                Some(crate::handlers::turn::TurnOverrides {
                    keep_alive: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .expect_err("keep_alive=true without comms_name should fail");
        assert_eq!(err.code, error::INVALID_PARAMS);

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "failed runtime keep_alive override should keep staged admission reserved: {blocked:?}"
        );

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "retry without override".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("failed keep_alive override must not poison the staged build config");
    }

    #[tokio::test]
    async fn invalid_pending_apply_keep_alive_override_does_not_persist() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let err = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                "invalid keep_alive".into(),
                event_tx,
                None,
                None,
                None,
                Some(crate::handlers::turn::TurnOverrides {
                    keep_alive: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .expect_err("keep_alive=true without comms_name should fail");
        assert_eq!(err.code, error::INVALID_PARAMS);

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "retry without override".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("failed runtime keep_alive override must not poison staged build config");
    }

    #[tokio::test]
    async fn pending_runtime_apply_pre_run_terminal_archives_and_releases_staged_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let primitive = runtime_resume_pending_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let output = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("no-pending boundary should be returned as a runtime terminal");
        assert!(
            matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)),
            "resume-pending without a boundary should return the typed no-pending terminal: {output:?}"
        );
        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load machine-archived no-pending snapshot")
            .is_none();
        assert!(
            stored,
            "no-pending runtime apply should hide the machine-archived snapshot behind runtime retirement"
        );
        let (event_tx, _event_rx) = mpsc::channel(100);
        let recovery = runtime
            .try_recover_persisted_session(
                &session_id,
                "should not recover restored staged no-pending snapshot".into(),
                event_tx,
                false,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            recovery
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "restored staged no-pending snapshot must not be recoverable before archive: {recovery:?}"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect(
                "pre-run no-pending terminal should release staged admission after machine archive",
            );
    }

    #[tokio::test]
    async fn pending_runtime_apply_pre_run_terminal_archived_session_cannot_materialize() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let primitive = runtime_resume_pending_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let output = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("no-pending boundary should be returned as a runtime terminal");
        assert!(
            matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)),
            "resume-pending without a boundary should return the typed no-pending terminal: {output:?}"
        );

        let (event_tx, _event_rx) = mpsc::channel(100);
        let retry = runtime
            .start_turn(
                &session_id,
                "retry after no-pending boundary".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect_err("machine-archived no-pending session must not materialize");
        assert!(
            retry.code == error::SESSION_NOT_FOUND,
            "machine-archived no-pending session should be gone, got {retry:?}"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("machine-archived no-pending session should release admission");
    }

    async fn drive_pending_start_resume_pending(
        runtime: &SessionRuntime,
        session_id: &SessionId,
    ) -> Result<RunResult, RpcError> {
        let slot = runtime
            .staged_sessions
            .begin_promotion(session_id)
            .await
            .map_err(|err| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("staged session lifecycle error: {err}"),
                data: None,
            })?
            .ok_or_else(|| RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            })?;
        let promotion_cleanup = PendingPromotionCleanup::new(
            Arc::clone(&runtime.staged_sessions),
            Arc::clone(&runtime.staged_capacity_admissions),
            session_id,
            &slot,
            runtime.take_staged_capacity_admission(session_id),
        );
        let PromotingSlot {
            build_config,
            labels,
            machine_archived_resume_authorized,
            ..
        } = slot;
        let build_config = *build_config;
        let create_req = CreateSessionRequest {
            model: build_config.model.clone(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: build_config.system_prompt.clone(),
            max_tokens: build_config.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build_config.to_session_build_options()),
            labels,
        };
        let result_rx = runtime.spawn_pending_create_and_start_turn_with_admission_guard(
            session_id.clone(),
            create_req,
            service_resume_pending_request(),
            promotion_cleanup,
            machine_archived_resume_authorized,
        );
        SessionRuntime::await_service_start_turn(session_id, result_rx).await
    }

    #[tokio::test]
    async fn pending_start_pre_run_failure_archives_snapshot_and_releases_admission() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime_with_runtime_store(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let no_pending = drive_pending_start_resume_pending(&runtime, &session_id)
            .await
            .expect_err("resume-pending start without a boundary should fail before run");
        assert!(
            no_pending.message.contains("no pending boundary"),
            "unexpected no-pending start error: {no_pending:?}"
        );
        let hidden = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load machine-archived no-pending start snapshot")
            .is_none();
        assert!(
            hidden,
            "pending start no-pending snapshot should be hidden by runtime retirement"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("machine-archived no-pending start should release staged admission");
    }

    #[tokio::test]
    async fn pending_start_restore_without_replenished_capacity_aborts_staged_restore() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let slot = runtime
            .staged_sessions
            .begin_promotion(&session_id)
            .await
            .expect("begin staged promotion")
            .expect("staged slot should exist");
        let mut promotion_cleanup = PendingPromotionCleanup::new(
            Arc::clone(&runtime.staged_sessions),
            Arc::clone(&runtime.staged_capacity_admissions),
            &session_id,
            &slot,
            runtime.take_staged_capacity_admission(&session_id),
        );
        let PromotingSlot {
            build_config,
            labels,
            ..
        } = slot;
        let build_config = *build_config;
        let create_req = CreateSessionRequest {
            model: build_config.model.clone(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: build_config.system_prompt.clone(),
            max_tokens: build_config.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build_config.to_session_build_options()),
            labels,
        };
        let admission = promotion_cleanup
            .take_staged_capacity_admission()
            .expect("promotion should own staged admission");
        runtime
            .service
            .create_session_with_reserved_admission(create_req, admission)
            .await
            .expect("materialize pending session");
        promotion_cleanup.mark_materialized();

        let result = runtime
            .service
            .start_turn(&session_id, service_resume_pending_request())
            .await;
        assert!(
            SessionRuntime::should_restore_pending_after_start_turn(
                &runtime.service,
                &session_id,
                &result,
            )
            .await,
            "test must reach the materialized pending restore path"
        );
        runtime
            .service
            .archive(&session_id)
            .await
            .expect("archive should release live capacity before replenishment");
        let _competitor = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("competing create should consume the released live capacity");
        let replenish = promotion_cleanup
            .replenish_staged_capacity_admission(&runtime.service)
            .await;
        assert!(
            replenish.is_err(),
            "competing create must make staged capacity replenishment fail"
        );

        promotion_cleanup.authorize_machine_archived_resume();
        promotion_cleanup.restore_now().await;
        promotion_cleanup.disarm();

        assert!(
            !runtime.staged_sessions.contains(&session_id).await,
            "restore must abort instead of re-staging without capacity"
        );
        assert!(
            !runtime.has_staged_capacity_admission(&session_id),
            "failed restore must not leave a capacity-less staged admission entry"
        );
    }

    #[tokio::test]
    async fn pending_promotion_drop_without_staged_capacity_aborts_restore() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let slot = runtime
            .staged_sessions
            .begin_promotion(&session_id)
            .await
            .expect("begin staged promotion")
            .expect("staged slot should exist");
        let mut promotion_cleanup = PendingPromotionCleanup::new(
            Arc::clone(&runtime.staged_sessions),
            Arc::clone(&runtime.staged_capacity_admissions),
            &session_id,
            &slot,
            runtime.take_staged_capacity_admission(&session_id),
        );
        let service_admission = promotion_cleanup
            .take_staged_capacity_admission()
            .expect("promotion should own staged admission");

        drop(promotion_cleanup);

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while runtime.staged_sessions.contains(&session_id).await {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("drop cleanup should abort restore instead of re-staging without capacity");
        assert!(
            !runtime.has_staged_capacity_admission(&session_id),
            "drop cleanup must not synthesize staged capacity without the original guard"
        );

        drop(service_admission);
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("aborted drop restore should release capacity after service admission drops");
    }

    #[tokio::test]
    async fn pending_start_archived_authoritative_row_abandons_staged_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(meerkat::MemoryStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let mut archived = Session::with_id(session_id.clone());
        archived.set_metadata("session_archived", serde_json::Value::Bool(true));
        meerkat::SessionStore::save(store.as_ref(), &archived)
            .await
            .expect("save archived authoritative snapshot");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .start_turn(
                &session_id,
                "materialize archived staged session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "archived authoritative row should terminally reject pending materialization: {rejected:?}"
        );
        assert!(
            !runtime.staged_sessions.contains(&session_id).await,
            "terminal archived create rejection must not restore the staged slot"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "terminal archived create rejection must unregister runtime bindings"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("terminal archived rejection should release admission capacity");
    }

    #[tokio::test]
    async fn recovered_create_archived_recheck_unregisters_prepared_bindings() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize recoverable session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live recoverable session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let recovery_overrides = runtime
            .recovery_overrides_from_turn(None, false)
            .expect("recovery overrides");
        let recovered_create = runtime
            .recovered_create_request(&session_id, stored, recovery_overrides)
            .await
            .expect("prepare recovered create request");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "recovered_create_request should prepare runtime bindings before create"
        );

        let admission = runtime
            .reserve_active_turn(&session_id)
            .await
            .expect("reserve recovery admission");
        runtime
            .service
            .archive_with_machine_protocol(
                &session_id,
                MachineSessionArchiveProtocol::from_machine(runtime.runtime_adapter.as_ref()),
            )
            .await
            .expect("archive should win before recovered create");
        let result_rx = runtime.spawn_recovered_create_and_apply_runtime_turn_with_admission_guard(
            session_id.clone(),
            recovered_create.request,
            recovered_create.runtime_was_registered,
            RunId::new(),
            service_start_turn_request("recover after archive"),
            RunApplyBoundary::Immediate,
            vec![meerkat_core::lifecycle::InputId::new()],
            admission.into_admission(),
            false,
        );
        let rejected =
            SessionRuntime::await_service_apply_runtime_turn(&session_id, result_rx).await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "recovered create should observe current archived row: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "archived recovered-create rejection must unregister prepared bindings"
        );
    }

    #[tokio::test]
    async fn recovered_create_request_failure_unregisters_new_runtime_registration() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize recoverable session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live recoverable session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "test starts without runtime registration"
        );

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let rejected = runtime
            .recovered_create_request(
                &session_id,
                stored,
                SurfaceSessionRecoveryOverrides {
                    provider: Some(meerkat_core::Provider::OpenAI),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::INVALID_PARAMS),
            "invalid recovery override should reject after prepare_bindings: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "failed recovery should unregister only the new runtime registration"
        );
    }

    #[tokio::test]
    async fn recovered_create_request_failure_preserves_existing_runtime_registration() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize recoverable session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live recoverable session");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "test requires a pre-existing runtime registration"
        );

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let rejected = runtime
            .recovered_create_request(
                &session_id,
                stored,
                SurfaceSessionRecoveryOverrides {
                    provider: Some(meerkat_core::Provider::OpenAI),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::INVALID_PARAMS),
            "invalid recovery override should reject after prepare_bindings: {rejected:?}"
        );
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "failed recovery must preserve a pre-existing runtime registration"
        );
    }

    #[tokio::test]
    async fn recovered_staging_failure_unregisters_prepared_runtime_state() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable session".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize recoverable session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live recoverable session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;

        let overrides = crate::handlers::turn::TurnOverrides {
            keep_alive: None,
            model: Some("gpt-5.4".to_string()),
            provider: Some("anthropic".to_string()),
            max_tokens: None,
            system_prompt: None,
            output_schema: None,
            structured_output_retries: None,
            provider_params: None,
            clear_provider_params: false,
            auth_binding: None,
            clear_auth_binding: false,
        };
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .try_recover_persisted_session(
                &session_id,
                ContentInput::Text("recover".to_string()),
                event_tx,
                false,
                None,
                None,
                None,
                Some(&overrides),
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("provider")),
            "invalid recovered staging override should fail validation: {rejected:?}"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "failed recovered staging should unregister prepared runtime state"
        );
    }

    #[tokio::test]
    async fn staged_archive_unregisters_prepared_runtime_bindings() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime_with_runtime_store(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "staged create should prepare runtime bindings"
        );

        runtime
            .archive_session(&session_id)
            .await
            .expect("archive staged session");
        assert_eq!(
            runtime
                .service
                .persisted_runtime_state(&session_id)
                .await
                .expect("read persisted staged archive runtime state"),
            Some(RuntimeState::Retired),
            "staged archive must retire prepared runtime state before cleanup"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "staged archive must unregister prepared runtime bindings"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("staged archive should release active admission capacity");
    }

    #[tokio::test]
    async fn materialized_archive_unregisters_runtime_bindings() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize before archive".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize session");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "materialized session should have runtime bindings"
        );

        runtime
            .archive_session(&session_id)
            .await
            .expect("archive materialized session");
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "materialized archive must unregister runtime bindings"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("materialized archive should release active admission capacity");
    }

    #[tokio::test]
    async fn pending_start_pre_run_archive_failure_restores_staged_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ToggleFailSaveStore::new());
        let runtime = make_runtime_with_session_store_and_runtime_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        store.fail_after_successful_saves(1);
        let failed = drive_pending_start_resume_pending(&runtime, &session_id).await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("forced save failure")),
            "archive failure should be surfaced for no-pending start cleanup: {failed:?}"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "failed no-pending start cleanup must restore staged admission: {blocked:?}"
        );

        store.clear_failures();
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "retry after failed no-pending cleanup".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("staged session should remain retryable after cleanup failure");
    }

    #[tokio::test]
    async fn pending_start_create_persist_failure_restores_staged_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ToggleFailSaveStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        store.set_fail_save(true);
        let (event_tx, _event_rx) = mpsc::channel(100);
        let failed = runtime
            .start_turn(
                &session_id,
                "first materialization should fail durable save".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("forced save failure")),
            "create persist failure should be surfaced for pending start: {failed:?}"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "restored staged session must keep admission reserved after create failure: {blocked:?}"
        );
        assert!(
            !runtime
                .service
                .has_live_session(&session_id)
                .await
                .expect("query live session after create failure"),
            "failed pending create should discard the unpersisted live deferred session"
        );

        store.clear_failures();
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "retry after create persist failure".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("staged session should retry after durable store recovers");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed retry should release active admission");
    }

    #[tokio::test]
    async fn pending_runtime_apply_pre_run_archive_failure_restores_staged_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ToggleFailSaveStore::new());
        let runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create staged session");
        store.fail_after_successful_saves(2);
        let primitive = runtime_resume_pending_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let failed = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("forced save failure")),
            "archive failure should be surfaced for no-pending apply cleanup: {failed:?}"
        );
        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "failed no-pending apply cleanup must restore staged admission: {blocked:?}"
        );

        store.clear_failures();
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "retry after failed apply cleanup".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("staged session should remain retryable after apply cleanup failure");
    }

    #[tokio::test]
    async fn failed_materialized_turn_releases_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);

        let session_id = runtime
            .create_session(fail_after_first_build_config(), None, None)
            .await
            .expect("create session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "first turn".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("first turn should succeed");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let failed = runtime
            .start_turn(
                &session_id,
                "second turn fails".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(failed.is_err(), "second turn should fail");

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("failed turn should release admission capacity");
    }

    #[tokio::test]
    async fn cancelled_materialized_turn_keeps_admission_until_service_turn_stops() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));
        let (build_config, calls, release) = block_after_first_build_config();

        let session_id = runtime
            .create_session(build_config, None, None)
            .await
            .expect("create session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("first turn should materialize session");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let session_for_turn = session_id.clone();
        let running_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &session_for_turn,
                    "slow materialized turn".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        wait_for_llm_calls(&calls, 2, "materialized blocking turn").await;

        running_turn.abort();
        assert!(
            running_turn.await.unwrap_err().is_cancelled(),
            "outer RPC future should be cancelled"
        );

        let blocked = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "cancelled caller must not release capacity while service turn is still running: {blocked:?}"
        );

        release.notify_waiters();
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(2);
        loop {
            match runtime
                .create_session(mock_build_config(), None, None)
                .await
            {
                Ok(_) => break,
                Err(err) if err.message.contains("Max sessions") => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "capacity did not release after service turn stopped"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                Err(err) => panic!("unexpected create_session error after cancellation: {err:?}"),
            }
        }
    }

    #[tokio::test]
    async fn materialized_turn_rejected_by_capacity_does_not_hot_swap_identity() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create materialized candidate");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "materialize candidate".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize candidate");

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load stored candidate")
            .expect("stored candidate exists");
        let original_meta = stored
            .session_metadata()
            .expect("stored candidate metadata before rejected turns");
        assert_eq!(original_meta.model, "claude-sonnet-4-5");
        assert_eq!(original_meta.provider, meerkat_core::Provider::Anthropic);

        let (_blocker, blocking_turn, release) = start_blocking_capacity_turn(&runtime).await;
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .start_turn(
                &session_id,
                "rejected start".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "capacity-full materialized start should be rejected before hot-swap: {rejected:?}"
        );
        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load stored candidate after rejected start")
            .expect("stored candidate exists after rejected start");
        let meta = stored
            .session_metadata()
            .expect("stored candidate metadata after rejected start");
        assert_eq!(meta.model, "claude-sonnet-4-5");
        assert_eq!(meta.provider, meerkat_core::Provider::Anthropic);
        release.notify_waiters();
        blocking_turn
            .await
            .expect("blocking turn task")
            .expect("blocking turn should finish");

        let (_blocker, blocking_turn, release) = start_blocking_capacity_turn(&runtime).await;
        let primitive = runtime_content_turn_primitive();
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text("rejected runtime apply".to_string()),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "capacity-full runtime apply should be rejected before hot-swap: {rejected:?}"
        );
        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load stored candidate after rejected apply")
            .expect("stored candidate exists after rejected apply");
        let meta = stored
            .session_metadata()
            .expect("stored candidate metadata after rejected apply");
        assert_eq!(meta.model, "claude-sonnet-4-5");
        assert_eq!(meta.provider, meerkat_core::Provider::Anthropic);
        release.notify_waiters();
        blocking_turn
            .await
            .expect("blocking turn task")
            .expect("blocking turn should finish");
    }

    #[tokio::test]
    async fn persisted_recovery_respects_active_admission_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let recoverable = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &recoverable,
                "persist me".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("complete recoverable session");
        runtime
            .service
            .discard_live_session(&recoverable)
            .await
            .expect("discard live recoverable session");
        runtime
            .runtime_adapter
            .unregister_session(&recoverable)
            .await;

        let blocker = runtime
            .create_session(slow_build_config(250), None, None)
            .await
            .expect("create blocker");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_for_turn = Arc::clone(&runtime);
        let blocker_for_task = blocker.clone();
        let blocking_turn = tokio::spawn(async move {
            runtime_for_turn
                .start_turn(
                    &blocker_for_task,
                    "block capacity".into(),
                    event_tx,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&blocker).await.map(|info| info.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "blocking session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let (event_tx, _event_rx) = mpsc::channel(100);
        let blocked_recovery = runtime
            .try_recover_persisted_session(
                &recoverable,
                "recover while full".into(),
                event_tx,
                false,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            blocked_recovery
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "recovery should be rejected while active capacity is full: {blocked_recovery:?}"
        );

        blocking_turn
            .await
            .unwrap()
            .expect("blocking turn should finish");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .try_recover_persisted_session(
                &recoverable,
                "recover after release".into(),
                event_tx,
                false,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("recovery should succeed after active capacity releases");
    }

    #[tokio::test]
    async fn runtime_apply_live_not_found_preserves_pre_admission_for_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist recoverable runtime turn".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("complete recoverable session");

        let admission = runtime
            .reserve_active_turn(&session_id)
            .await
            .expect("accepted runtime input should reserve active admission");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session before service apply");
        let result_rx = runtime.spawn_service_apply_runtime_turn_with_recoverable_admission_guard(
            session_id.clone(),
            RunId::new(),
            service_start_turn_request("live missing after pre-admission"),
            RunApplyBoundary::Immediate,
            vec![InputId::new()],
            admission.into_admission(),
        );
        let (result, recovered_admission) =
            SessionRuntime::await_service_apply_runtime_turn_with_recoverable_admission(
                &session_id,
                result_rx,
            )
            .await
            .expect("service apply task should report");
        assert!(
            matches!(result, Err(SessionError::NotFound { .. })),
            "discarded live service apply should return NotFound: {result:?}"
        );
        let recovered_admission =
            recovered_admission.expect("NotFound must preserve pre-admission for recovery");

        let blocked = runtime.service.reserve_create_session_admission().await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "preserved pre-admission should keep active capacity reserved"
        );
        drop(recovered_admission);

        runtime
            .service
            .reserve_create_session_admission()
            .await
            .expect("dropping recovered pre-admission should release active capacity");
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_capacity_full_rejects_before_mcp_boundary_apply() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 1);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create target session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "complete target".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("complete target session");

        {
            let mut mcp_sessions = runtime.mcp_sessions.write().await;
            let mcp_state = mcp_sessions
                .get_mut(&session_id)
                .expect("target session should have MCP adapter state");
            mcp_state.turn_counter = 0;
        }

        let _capacity_filler = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create capacity filler");

        let (event_tx, _event_rx) = mpsc::channel(100);
        let rejected = runtime
            .start_turn(
                &session_id,
                "must not apply MCP boundary".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("Max sessions")),
            "capacity-full start_turn should reject: {rejected:?}"
        );

        let mcp_sessions = runtime.mcp_sessions.read().await;
        let mcp_state = mcp_sessions
            .get(&session_id)
            .expect("target MCP state should remain registered");
        assert_eq!(
            mcp_state.turn_counter, 0,
            "capacity-rejected start_turn must not apply the MCP boundary"
        );
    }

    #[tokio::test]
    async fn recovered_runtime_apply_pre_run_terminal_discards_live_deferred_session() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create recoverable session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist me".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("complete recoverable session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live recoverable session");
        assert!(
            runtime.runtime_adapter.contains_session(&session_id).await,
            "discarding the live session should leave a stale runtime registration for recovery cleanup"
        );

        let primitive = runtime_resume_pending_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let output = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text(String::new()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("recovered no-pending boundary should be returned as a typed terminal");
        assert!(
            matches!(output.terminal, Some(CoreApplyTerminal::NoPendingBoundary)),
            "recovered resume-pending without a boundary should return no-pending terminal: {output:?}"
        );
        assert!(
            !runtime
                .service
                .has_live_session(&session_id)
                .await
                .expect("query live recovered session"),
            "recovered pre-run terminal must not leave an untracked live deferred session"
        );
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "recovered pre-run terminal must unregister stale runtime registration"
        );

        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("recovered pre-run terminal should release active admission");
    }

    #[tokio::test]
    async fn archived_runtime_apply_recovery_rejects_without_admission_leak() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 1);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create session to archive");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist before archive".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("complete session before archive");
        runtime
            .archive_session(&session_id)
            .await
            .expect("archive persisted session");

        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let recovered = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text("must not recover archived session".to_string()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            recovered
                .as_ref()
                .err()
                .is_some_and(|err| err.code == error::SESSION_NOT_FOUND),
            "runtime apply must reject archived persisted recovery: {recovered:?}"
        );
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("rejected archived recovery must not leak active admission");
    }

    #[tokio::test]
    async fn recovered_runtime_apply_create_persist_failure_discards_live_deferred_admission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(ToggleFailSaveStore::new());
        let mut runtime = make_runtime_with_session_store(
            temp_factory(&temp),
            1,
            Arc::clone(&store) as Arc<dyn meerkat::SessionStore>,
        );
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = SessionId::new();
        let mut persisted = Session::with_id(session_id.clone());
        persisted
            .set_session_metadata(meerkat_core::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
            })
            .expect("session metadata");
        let mut deferred = meerkat_core::SessionDeferredTurnState::default();
        deferred.mark_initial_turn_pending();
        persisted
            .set_deferred_turn_state(deferred)
            .expect("deferred turn state");
        meerkat::SessionStore::save(store.as_ref(), &persisted)
            .await
            .expect("seed persisted deferred session");

        store.set_fail_save(true);
        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let failed = runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text("first recovery should fail durable save".to_string()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.message.contains("forced save failure")),
            "recovered create persist failure should be surfaced: {failed:?}"
        );
        assert!(
            !runtime
                .service
                .has_live_session(&session_id)
                .await
                .expect("query live recovered session after create failure"),
            "failed recovered create should discard the unpersisted live deferred session"
        );

        store.clear_failures();
        let admitted = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("discarded recovered create failure should release admission");
        runtime
            .archive_session(&admitted)
            .await
            .expect("archive admitted control session before retrying recovery");

        let primitive = runtime_content_turn_primitive();
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .apply_runtime_turn(
                &session_id,
                RunId::new(),
                &primitive,
                ContentInput::Text("retry recovery after durable store recovers".to_string()),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("recovered session should retry after durable store recovers");
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("completed recovery retry should release active admission");
    }

    /// 9. session_state returns None for an unknown session ID.
    #[tokio::test]
    async fn session_state_none_for_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        assert_eq!(runtime.session_state(&unknown_id).await, None);
    }

    /// 10. Shutdown closes all sessions.
    #[tokio::test]
    async fn shutdown_closes_all_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let s1 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let s2 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Verify both exist
        assert!(runtime.session_state(&s1).await.is_some());
        assert!(runtime.session_state(&s2).await.is_some());

        // Shutdown
        runtime.shutdown().await;

        // Both should be gone from the sessions map
        let sessions = runtime.list_sessions(Default::default()).await;
        assert!(
            sessions.is_empty(),
            "All sessions should be removed after shutdown"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_applies_staged_mcp_ops_at_turn_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(
                &session_id,
                McpServerConfig::stdio("broken-server", "echo", Vec::new(), HashMap::new()),
            )
            .await
            .expect("staging should succeed");

        // Non-blocking: apply_staged spawns background task and succeeds
        // (the broken server fails asynchronously, not at staging time).
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let first = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            first.is_ok(),
            "non-blocking apply_staged should not fail synchronously"
        );
        let first_events = collect_tool_config_events(&mut event_rx);
        assert!(
            first_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Add
                    && payload.target == "broken-server"
                    && payload.status_text() == "pending"
            }),
            "should emit pending event for staged add"
        );

        let (event_tx, _event_rx) = mpsc::channel(32);
        let second = runtime
            .start_turn(
                &session_id,
                "hello again".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            second.is_ok(),
            "failed staged operation should not poison subsequent turns"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_applies_staged_mcp_remove_and_reload_at_turn_boundary() {
        let Some(server_config) = maybe_mcp_server_config("test-server") else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, server_config)
            .await
            .expect("stage add");

        // Non-blocking: add is now async — boundary emits "pending" not "applied".
        let (event_tx, mut event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("turn add should apply staged add");
        let add_events = collect_tool_config_events(&mut event_rx);
        assert!(add_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Add
                && payload.target == "test-server"
                && payload.status_text() == "pending"
        }));

        runtime
            .mcp_stage_remove(&session_id, "test-server".to_string())
            .await
            .expect("stage remove");
        let (event_tx, mut event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("turn remove should apply staged remove");
        let remove_events = collect_tool_config_events(&mut event_rx);
        assert!(remove_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "test-server"
                && matches!(payload.status_text().as_str(), "applied" | "draining")
        }));
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn async_mcp_removal_timeout_is_emitted_on_next_boundary() {
        let Some(server_config) = maybe_mcp_server_config("timeout-server") else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, server_config)
            .await
            .expect("stage add");

        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");

        // Wait for the background MCP connect to complete before setting
        // inflight calls — otherwise `drain_pending` on the next boundary
        // replaces the entry and resets active_calls to 0.
        adapter.wait_until_ready(Duration::from_secs(5)).await;

        adapter
            .set_removal_timeout_for_testing(Duration::from_millis(20))
            .await
            .expect("set timeout");
        adapter
            .set_inflight_calls_for_testing("timeout-server", 1)
            .await
            .expect("set inflight");

        runtime
            .mcp_stage_remove(&session_id, "timeout-server".to_string())
            .await
            .expect("stage remove");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove boundary");
        let first_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(first_turn_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "timeout-server"
                && payload.status_text() == "draining"
        }));

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !adapter
                    .has_removing_servers()
                    .await
                    .expect("check removing state")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("timed out waiting for forced removal to finalize");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn after timeout".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("follow-up boundary");
        let second_turn_events = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let events = collect_tool_config_events(&mut event_rx);
                if !events.is_empty() {
                    break events;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_default();
        assert!(
            second_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Remove
                    && payload.target == "timeout-server"
                    && matches!(payload.status_text().as_str(), "forced" | "applied")
            }),
            "expected forced removal event on a follow-up boundary, got: {second_turn_events:?}"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    #[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
    async fn staged_ops_remain_boundary_gated_while_background_drain_runs() {
        let Some(server1_config) = maybe_mcp_server_config("server-draining") else {
            return;
        };
        let Some(server2_config) = maybe_mcp_server_config("server-staged") else {
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, server1_config)
            .await
            .expect("stage add draining server");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add first server at boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");
        adapter
            .set_removal_timeout_for_testing(Duration::from_secs(3))
            .await
            .expect("set long timeout");
        adapter
            .set_inflight_calls_for_testing("server-draining", 1)
            .await
            .expect("keep server draining");

        runtime
            .mcp_stage_remove(&session_id, "server-draining".to_string())
            .await
            .expect("stage remove");
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn remove first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove starts draining");
        let first_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(first_turn_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "server-draining"
                && payload.status_text() == "draining"
        }));
        assert!(
            adapter.tools().is_empty(),
            "draining server should be hidden immediately"
        );

        runtime
            .mcp_stage_add(&session_id, server2_config)
            .await
            .expect("stage second add");

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            adapter.tools().is_empty(),
            "background drain must not apply newly staged add outside boundary"
        );

        // Turn 3: apply the staged add. Since MCP adds are non-blocking
        // (spawn_pending), this turn will emit PendingConnect ("pending"),
        // not "applied". The actual connection completes asynchronously.
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn apply staged add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("next boundary should apply staged add");

        let next_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(
            next_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Add
                    && payload.target == "server-staged"
                    && payload.status_text() == "pending"
            }),
            "expected Add+pending for server-staged at boundary, got: {next_turn_events:?}"
        );

        // Turn 4: after the background connection resolves, drain_pending
        // picks it up and the server becomes visible.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn drain pending add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("drain should resolve pending add");

        let drain_events = collect_tool_config_events(&mut event_rx);
        // The server becomes active via drain_pending → process_pending_result
        // → Activated lifecycle action. The event status may be "applied" (from
        // emit_mcp_lifecycle_events) or "activated" (from session service tool
        // scope machinery). Either confirms the server was successfully connected.
        let has_add = drain_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Add && payload.target == "server-staged"
        });
        assert!(
            has_add,
            "expected Add event for server-staged after drain, got: {drain_events:?}"
        );
        assert!(
            !adapter.tools().is_empty(),
            "second server should become visible after drain"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    #[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
    async fn queued_lifecycle_actions_survive_boundary_apply_failure() {
        let Some(server_config) = maybe_mcp_server_config("lossless-server") else {
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, server_config)
            .await
            .expect("stage add");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");
        adapter
            .set_removal_timeout_for_testing(Duration::from_millis(20))
            .await
            .expect("set timeout");
        adapter
            .set_inflight_calls_for_testing("lossless-server", 1)
            .await
            .expect("set inflight");

        runtime
            .mcp_stage_remove(&session_id, "lossless-server".to_string())
            .await
            .expect("stage remove");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove boundary");

        // Wait for the background drain task to process the forced removal.
        // Drain task polls every 100ms, timeout is 20ms, so after 500ms
        // the forced removal should be queued in lifecycle_rx.
        tokio::time::sleep(Duration::from_millis(500)).await;

        runtime
            .mcp_stage_add(
                &session_id,
                McpServerConfig::stdio("broken-server", "echo", Vec::new(), HashMap::new()),
            )
            .await
            .expect("stage broken add");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        // The broken add (echo) spawns a background connection that will fail
        // asynchronously. apply_staged() may or may not return an error since
        // MCP adds are non-blocking. The important assertion is that the
        // lossless-server's forced removal event was emitted via the lifecycle
        // channel despite the broken add also being staged.
        let _result = runtime
            .start_turn(
                &session_id,
                "turn with broken add and queued removal".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;

        let fail_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(
            fail_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Remove
                    && payload.target == "lossless-server"
                    && matches!(payload.status_text().as_str(), "forced" | "applied")
            }),
            "removal of lossless-server should survive alongside broken add, got: {fail_turn_events:?}"
        );
    }

    /// 11. Startup registry build rejects invalid lineage/remap config.
    #[test]
    fn build_skill_identity_registry_rejects_invalid_config() {
        let cfg = Config {
            skills: SkillsConfig {
                repositories: vec![
                    SkillRepositoryConfig {
                        name: "old".to_string(),
                        source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-old".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-a".to_string(),
                        source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-a".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-b".to_string(),
                        source_uuid: SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-b".to_string(),
                        },
                    },
                ],
                identity: SkillsIdentityConfig {
                    lineage: vec![SourceIdentityLineage {
                        event_id: "split-1".to_string(),
                        recorded_at_unix_secs: 1,
                        required_from_skills: vec![
                            SkillName::parse("email-extractor").expect("skill"),
                            SkillName::parse("pdf-processing").expect("skill"),
                        ],
                        event: SourceIdentityLineageEvent::Split {
                            from: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            into: vec![
                                SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                    .expect("uuid"),
                                SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                                    .expect("uuid"),
                            ],
                        },
                    }],
                    remaps: vec![SkillKeyRemap {
                        from: SkillKey {
                            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            skill_name: SkillName::parse("email-extractor").expect("skill"),
                        },
                        to: SkillKey {
                            source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                .expect("uuid"),
                            skill_name: SkillName::parse("mail-extractor").expect("skill"),
                        },
                        reason: None,
                    }],
                    aliases: vec![],
                },
                ..SkillsConfig::default()
            },
            ..Config::default()
        };

        let result = SessionRuntime::build_skill_identity_registry(&cfg, None, None);
        assert!(matches!(
            result,
            Err(meerkat_core::skills::SkillError::MissingSkillRemaps { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // Phase 1 tests: Deferred create, keep_alive override, per-turn overrides
    // -----------------------------------------------------------------------

    /// Deferred session/create returns a session in Idle state (no turn executed).
    #[tokio::test]
    async fn deferred_create_returns_idle_session() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Session exists and is idle (no turn started).
        let info = runtime.session_state(&session_id).await.unwrap();
        assert_eq!(info.state, SessionState::Idle);
    }

    /// Deferred create followed by turn/start works correctly.
    #[tokio::test]
    async fn deferred_create_then_turn_start_works() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let (event_tx, _event_rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(result.text.contains("Hello from mock"));
    }

    #[tokio::test]
    async fn pending_session_stream_bridge_removes_entry_when_listener_closes_after_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let mut stream = runtime
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe pending session events");

        assert!(
            runtime
                .pending_session_event_streams
                .lock()
                .await
                .contains_key(&session_id),
            "pending stream entry should exist before materialization"
        );

        let (event_tx, _event_rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(result.text.contains("Hello from mock"));

        let _first_event = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("pending stream event timeout")
            .expect("pending stream should receive first-turn event");

        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !runtime
                    .pending_session_event_streams
                    .lock()
                    .await
                    .contains_key(&session_id)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("pending stream entry should be removed after listener close");
    }

    /// turn/start with keep_alive override on a materialized session is not rejected.
    #[tokio::test]
    async fn turn_start_with_keep_alive_override_accepted_on_materialized() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session with a first turn.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "First".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Second turn with only keep_alive override (no model/provider etc.)
        // should not be rejected — keep_alive is allowed on materialized sessions.
        // Use keep_alive: false to avoid needing comms runtime.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            keep_alive: Some(false),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            result.is_ok(),
            "keep_alive override should succeed on materialized session"
        );
    }

    /// turn/start on a pending session with model override applies it.
    #[tokio::test]
    async fn turn_start_on_pending_session_with_model_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Start turn with model override on pending session.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            result.is_ok(),
            "model override should succeed on pending session"
        );
    }

    #[tokio::test]
    async fn pending_session_read_reports_effective_llm_identity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("pending session should be readable");
        assert_eq!(info.model, "claude-sonnet-4-5");
        assert_eq!(info.provider, "anthropic");
        let capabilities = info
            .resolved_capabilities
            .expect("pending sessions should expose resolved capabilities");
        assert!(capabilities.vision);
        assert!(capabilities.image_input);
        assert!(capabilities.image_tool_results);
        assert!(!capabilities.inline_video);
        assert!(!capabilities.realtime);
        assert!(capabilities.web_search);
        assert!(
            !capabilities.image_generation,
            "Anthropic sessions should not advertise auto image generation"
        );
    }

    #[tokio::test]
    async fn pending_realtime_session_read_reports_realtime_capability() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let mut build = AgentBuildConfig::new("gpt-realtime");
        build.provider = Some(meerkat_core::Provider::OpenAI);
        build.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_id = runtime.create_session(build, None, None).await.unwrap();

        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("pending realtime session should be readable");
        let capabilities = info
            .resolved_capabilities
            .expect("realtime session should expose resolved capabilities");
        assert_eq!(info.model, "gpt-realtime");
        assert_eq!(info.provider, "openai");
        assert!(capabilities.realtime);
        assert!(!capabilities.image_input);
        assert!(!capabilities.image_tool_results);
        assert!(!capabilities.web_search);
    }

    #[tokio::test]
    async fn turn_start_on_pending_session_rejects_provider_only_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let err = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .expect_err("provider-only override should be rejected on pending sessions too");
        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("provider override requires model"),
            "unexpected error message: {}",
            err.message
        );
    }

    /// turn/start on a materialized session allows model override (hot-swap).
    #[tokio::test]
    async fn turn_start_on_materialized_session_allows_model_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "First".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Second turn with model override should NOT be rejected with INVALID_PARAMS.
        // In CI (no API key), the client build may fail with INTERNAL_ERROR, which
        // is fine — the point is that the override is accepted, not rejected at the
        // parameter validation layer.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        if let Err(ref err) = result {
            assert_ne!(
                err.code,
                error::INVALID_PARAMS,
                "model override should not be rejected on materialized session: {err:?}"
            );
        }
    }

    /// StartTurnParams deserializes with all fields.
    #[test]
    fn turn_start_params_deserialize_with_all_fields() {
        use crate::handlers::turn::StartTurnParams;

        let json = serde_json::json!({
            "session_id": "test-id",
            "prompt": "hello",
            "keep_alive": true,
            "model": "claude-opus-4-6",
            "provider": "anthropic",
            "max_tokens": 4096,
            "system_prompt": "You are helpful",
            "output_schema": {"type": "object"},
            "structured_output_retries": 3,
            "provider_params": {"thinking": true},
            "clear_provider_params": false,
            "clear_auth_binding": true
        });
        let params: StartTurnParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_id, "test-id");
        assert_eq!(params.prompt, ContentInput::Text("hello".to_string()));
        assert_eq!(params.keep_alive, Some(true));
        assert_eq!(params.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(params.provider.as_deref(), Some("anthropic"));
        assert_eq!(params.max_tokens, Some(4096));
        assert_eq!(params.system_prompt.as_deref(), Some("You are helpful"));
        assert!(params.output_schema.is_some());
        assert_eq!(params.structured_output_retries, Some(3));
        assert!(params.provider_params.is_some());
        assert!(!params.clear_provider_params);
        assert!(params.clear_auth_binding);
    }

    #[test]
    fn runtime_turn_metadata_from_overrides_preserves_provider_params_set() {
        use crate::handlers::turn::TurnOverrides;
        use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;

        let provider_params = serde_json::json!({
            "temperature": 0.2,
            "thinking": { "budget_tokens": 10_000 }
        });
        let overrides = TurnOverrides {
            provider_params: Some(provider_params),
            ..Default::default()
        };

        let metadata = SessionRuntime::turn_metadata_from_overrides(
            None,
            None,
            None,
            Some(&overrides),
            Some("anthropic"),
        )
        .expect("provider params set should produce turn metadata");

        let Some(TurnMetadataOverride::Set(params)) = metadata.provider_params else {
            panic!("provider_params set override should be preserved");
        };
        assert_eq!(params.temperature, Some(0.2));
        assert!(
            params.provider_tag.is_some(),
            "provider-native keys must stay represented on the typed override"
        );
    }

    #[test]
    fn runtime_turn_metadata_round_trips_provider_native_params_as_legacy_json() {
        use crate::handlers::turn::TurnOverrides;

        let overrides = TurnOverrides {
            provider_params: Some(serde_json::json!({
                "thinking": { "budget_tokens": 10_000 },
                "effort": "xhigh",
                "web_search": null,
            })),
            ..Default::default()
        };
        let metadata = SessionRuntime::turn_metadata_from_overrides(
            None,
            None,
            None,
            Some(&overrides),
            Some("anthropic"),
        )
        .expect("provider params set should produce turn metadata");

        let round_tripped = SessionRuntime::turn_overrides_from_metadata(Some(&metadata))
            .expect("metadata should reconstruct turn overrides");
        let provider_params = round_tripped
            .provider_params
            .expect("provider params should survive metadata round trip");

        assert!(
            provider_params.get("provider_tag").is_none(),
            "runtime adapter must not feed typed provider_tag envelopes back as legacy params"
        );
        assert_eq!(
            provider_params["thinking"]["budget_tokens"],
            serde_json::json!(10_000)
        );
        assert_eq!(provider_params["effort"], serde_json::json!("xhigh"));
        assert!(
            provider_params
                .as_object()
                .is_some_and(|obj| obj.contains_key("web_search")),
            "explicit provider-native null must not be dropped"
        );
        assert!(provider_params["web_search"].is_null());
    }

    #[test]
    fn recovery_overrides_from_turn_preserve_clear_and_connection_intent() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let auth_binding = test_auth_binding("dev", "default");
        let overrides = TurnOverrides {
            clear_provider_params: true,
            auth_binding: Some(auth_binding.clone()),
            ..Default::default()
        };

        let recovered = runtime
            .recovery_overrides_from_turn(Some(&overrides), false)
            .expect("valid recovery overrides");

        assert!(recovered.clear_provider_params);
        assert_eq!(recovered.provider_params, None);
        assert_eq!(recovered.auth_binding, Some(auth_binding));
        assert!(!recovered.clear_auth_binding);

        let clear_connection = TurnOverrides {
            clear_auth_binding: true,
            ..Default::default()
        };
        let recovered = runtime
            .recovery_overrides_from_turn(Some(&clear_connection), false)
            .expect("valid clear connection recovery override");
        assert!(recovered.clear_auth_binding);
        assert_eq!(recovered.auth_binding, None);
    }

    // -----------------------------------------------------------------------
    // Phase 4: Persisted session recovery
    // -----------------------------------------------------------------------

    /// turn/start on a completely unknown session returns SESSION_NOT_FOUND.
    #[tokio::test]
    async fn turn_start_unknown_session_returns_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        let (event_tx, _rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &unknown_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// Pending session timestamps are stable across list calls.
    #[tokio::test]
    async fn pending_session_timestamps_are_stable() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let list1 = runtime.list_sessions_rich(Default::default()).await;
        let ts1 = list1.iter().find(|s| s.session_id == session_id).unwrap();
        let created1 = ts1.created_at;
        let updated1 = ts1.updated_at;
        assert!(created1 > 0, "created_at should be non-zero");
        assert_eq!(created1, updated1, "initial created_at == updated_at");

        // Wait a moment and list again — timestamps should be identical.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let list2 = runtime.list_sessions_rich(Default::default()).await;
        let ts2 = list2.iter().find(|s| s.session_id == session_id).unwrap();
        assert_eq!(ts2.created_at, created1, "created_at must not drift");
        assert_eq!(
            ts2.updated_at, updated1,
            "updated_at must not drift without mutation"
        );
    }

    /// append_system_context on a pending session bumps updated_at.
    #[tokio::test]
    async fn pending_session_updated_at_advances_on_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let before = runtime.read_session_rich(&session_id).await.unwrap();
        let created = before.created_at;

        // Wait so updated_at can differ.
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let req = AppendSystemContextRequest {
            text: "new context".to_string(),
            source: None,
            idempotency_key: Some("key-1".to_string()),
        };
        runtime
            .append_system_context(&session_id, req)
            .await
            .unwrap();

        let after = runtime.read_session_rich(&session_id).await.unwrap();
        assert_eq!(after.created_at, created, "created_at must not change");
        assert!(
            after.updated_at >= created,
            "updated_at ({}) should be >= created_at ({}) after mutation",
            after.updated_at,
            created,
        );
    }

    /// read_session_rich returns last_assistant_text for idle materialized sessions.
    #[tokio::test]
    async fn read_session_rich_returns_last_assistant_text_for_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session with a turn.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Session is now idle — read_session_rich should provide last_assistant_text.
        let info = runtime.read_session_rich(&session_id).await.unwrap();
        assert!(
            info.last_assistant_text.is_some(),
            "idle session should have last_assistant_text, got None"
        );
    }

    fn temp_factory_with_builtins(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions")).builtins(true)
    }

    /// After hot-swapping from an Anthropic model to an OpenAI model, the
    /// machine-owned capability filter should hide `view_image`.
    #[tokio::test]
    async fn hot_swap_to_openai_hides_view_image() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        // Create session with an Anthropic model (supports image_tool_results).
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize the session with the first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Hot-swap to an OpenAI model (does NOT support image_tool_results).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("hot-swapped session should be readable");
        let capabilities = info
            .resolved_capabilities
            .expect("hot-swapped session should expose resolved capabilities");
        assert_eq!(info.model, "gpt-5.4");
        assert_eq!(info.provider, "openai");
        assert!(capabilities.vision);
        assert!(capabilities.image_input);
        assert!(
            !capabilities.image_tool_results,
            "OpenAI hot-swap should update resolved image tool-result support"
        );
        assert!(capabilities.web_search);
        assert!(capabilities.image_generation);

        // Export the session and check that the capability-owned visibility
        // state blocks view_image without mutating the external overlay.
        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);
        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::capability_base_filter_for_image_tool_results(false),
            "hot-swap to OpenAI should deny view_image via the capability base filter"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap should not widen or replace the user-owned external filter"
        );
    }

    /// After hot-swapping from OpenAI back to Anthropic, the tool filter should
    /// be cleared (set to All).
    #[tokio::test]
    async fn hot_swap_to_anthropic_clears_view_image_deny() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Hot-swap to OpenAI (deny view_image).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Hot-swap back to Anthropic (should clear the deny).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-sonnet-4-5".to_string()),
            provider: Some("anthropic".to_string()),
            ..Default::default()
        };
        let (event_tx3, _event_rx3) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Third turn".into(),
                event_tx3,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Export session and verify the capability-owned deny is cleared.
        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);
        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap back to Anthropic should clear the capability-owned view_image deny"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap back to Anthropic should leave the external filter untouched"
        );
    }

    /// Hot-swapping preserves pre-existing session-local denies while adding
    /// capability-owned model gating on top.
    #[tokio::test]
    async fn hot_swap_preserves_preexisting_deny_filter() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Stage a pre-existing deny filter for "datetime".
        runtime
            .service
            .set_session_tool_filter(
                &session_id,
                meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            )
            .await
            .expect("staging deny filter for datetime should succeed");

        // Hot-swap to OpenAI — should add view_image to the deny set, not replace it.
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);
        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::capability_base_filter_for_image_tool_results(false),
            "hot-swap should deny view_image via the capability base filter"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            "hot-swap should preserve the existing user-owned deny filter"
        );
    }

    /// Hot-swapping back to a capable model clears only the capability-owned
    /// `view_image` denial, keeping user-owned denies.
    #[tokio::test]
    async fn hot_swap_back_preserves_other_denied_tools() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Simulate a prior OpenAI swap by combining a capability-owned
        // `view_image` deny with a user-owned external deny for `datetime`.
        let mut visibility_state = exported_tool_visibility_state(
            &runtime
                .service
                .export_live_session(&session_id)
                .await
                .unwrap(),
        );
        visibility_state.capability_base_filter =
            meerkat_core::capability_base_filter_for_image_tool_results(false);
        visibility_state.active_filter =
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect());
        visibility_state.staged_filter = visibility_state.active_filter.clone();
        visibility_state
            .filter_witnesses
            .insert("datetime".to_string(), builtin_tool_visibility_witness());
        runtime
            .service
            .set_session_tool_visibility_state(&session_id, Some(visibility_state))
            .await
            .expect("staging mixed capability and external denies should succeed");

        // Hot-swap to Anthropic — should clear only the capability-owned deny.
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);

        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap to Anthropic should clear the capability-owned view_image deny"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            "hot-swap to Anthropic should keep the user-owned deny filter"
        );
    }

    #[tokio::test]
    async fn materialized_provider_only_override_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let overrides = crate::handlers::turn::TurnOverrides {
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let err = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .expect_err("provider-only override should be rejected");
        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("provider override requires model"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn materialized_provider_params_hot_swap_persists_identity() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let provider_params = serde_json::json!({
            "thinking": { "budget_tokens": 10_000 }
        });
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(provider_params.clone()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let live = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live session after provider_params hot-swap");
        let live_meta = live
            .session_metadata()
            .expect("live session metadata after provider_params hot-swap");
        assert_eq!(live_meta.model, "claude-sonnet-4-5");
        assert_eq!(live_meta.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(live_meta.provider_params, Some(provider_params.clone()));

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session after provider_params hot-swap")
            .expect("stored session should exist");
        let stored_meta = stored
            .session_metadata()
            .expect("stored session metadata after provider_params hot-swap");
        assert_eq!(stored_meta.model, "claude-sonnet-4-5");
        assert_eq!(stored_meta.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(stored_meta.provider_params, Some(provider_params));
    }

    #[tokio::test]
    async fn materialized_hot_swap_updates_durable_identity_for_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let provider_params = serde_json::json!({
            "reasoning": { "effort": "medium" }
        });
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider: Some("openai".to_string()),
            provider_params: Some(provider_params.clone()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("read_session_rich after hot-swap");
        assert_eq!(info.model, "gpt-5.4");
        assert_eq!(info.provider, "openai");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session after hot-swap");

        let (event_tx, _rx) = mpsc::channel(100);
        let recovered = runtime
            .try_recover_persisted_session(
                &session_id,
                "recover".into(),
                event_tx,
                false,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            recovered.is_ok(),
            "expected persisted hot-swapped session to be recoverable: {:?}",
            recovered.err()
        );

        // After recovery the session has been promoted out of pending into a
        // live session.  Verify the recovered run completed with the preserved
        // hot-swapped identity by reading back the live session metadata.
        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("read recovered session after re-materialization");
        assert_eq!(info.model, "gpt-5.4", "recovered model must match hot-swap");
        assert_eq!(
            info.provider, "openai",
            "recovered provider must match hot-swap"
        );
    }

    #[tokio::test]
    async fn missing_live_recovery_applies_turn_clear_and_connection_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.provider_params = Some(serde_json::json!({
            "thinking": { "budget_tokens": 10_000 }
        }));
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("initial materialization");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session before recovery");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;

        let auth_binding = test_auth_binding("dev", "default");
        let overrides = crate::handlers::turn::TurnOverrides {
            clear_provider_params: true,
            auth_binding: Some(auth_binding.clone()),
            ..Default::default()
        };
        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "Recover with overrides".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .expect("recovery turn should apply overrides");

        let live = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live session after recovery");
        let live_meta = live
            .session_metadata()
            .expect("live metadata after recovery");
        assert_eq!(
            live_meta.provider_params, None,
            "clear_provider_params must survive missing-live recovery"
        );
        assert_eq!(
            live_meta.auth_binding,
            Some(auth_binding.clone()),
            "auth_binding set must survive missing-live recovery"
        );

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session after recovery")
            .expect("stored session should exist");
        let stored_meta = stored
            .session_metadata()
            .expect("stored metadata after recovery");
        assert_eq!(stored_meta.provider_params, None);
        assert_eq!(stored_meta.auth_binding, Some(auth_binding));
    }

    /// Regression: start_turn_via_runtime must reject build-only overrides
    /// (max_tokens, system_prompt, output_schema, structured_output_retries)
    /// that cannot be applied via RuntimeTurnMetadata. Before the fix these
    /// were silently dropped.
    #[tokio::test]
    async fn runtime_turn_rejects_build_only_overrides() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        for (field, overrides) in [
            (
                "max_tokens",
                TurnOverrides {
                    max_tokens: Some(1024),
                    ..Default::default()
                },
            ),
            (
                "system_prompt",
                TurnOverrides {
                    system_prompt: Some("override".into()),
                    ..Default::default()
                },
            ),
            (
                "output_schema",
                TurnOverrides {
                    output_schema: Some(serde_json::json!({"type": "object"})),
                    ..Default::default()
                },
            ),
            (
                "structured_output_retries",
                TurnOverrides {
                    structured_output_retries: Some(5),
                    ..Default::default()
                },
            ),
        ] {
            let (tx, _rx) = mpsc::channel(100);
            let result = runtime
                .start_turn_via_runtime(
                    &session_id,
                    "test".into(),
                    tx,
                    None,
                    None,
                    None,
                    Some(overrides),
                )
                .await;
            assert!(
                result.is_err(),
                "build-only override '{field}' must be rejected on runtime-routed turn"
            );
            let err = result.unwrap_err();
            assert_eq!(
                err.code,
                error::INVALID_PARAMS,
                "'{field}' rejection must use INVALID_PARAMS"
            );
            assert!(
                err.message.contains(field),
                "error message must mention '{field}': {}",
                err.message
            );
        }
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn runtime_turn_keep_alive_override_on_pending_requires_comms_name() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let (tx, _rx) = mpsc::channel(100);
        let err = runtime
            .start_turn_via_runtime(
                &session_id,
                "test".into(),
                tx,
                None,
                None,
                None,
                Some(TurnOverrides {
                    keep_alive: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .expect_err("keep_alive=true must require comms_name on pending sessions");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert_eq!(
            err.message,
            "keep_alive requires a session created with comms_name"
        );
        assert!(
            runtime.pending_session_exists(&session_id).await,
            "invalid staged keep_alive override must leave the session staged"
        );

        let (tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "after rejection".into(),
                tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("staged session should remain promotable after rejected override");
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn runtime_turn_keep_alive_override_on_persisted_only_unregisters_new_executor() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 1));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create session");
        let (tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "persist before invalid keep_alive override".into(),
                tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("materialize and persist session");
        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        runtime
            .runtime_adapter
            .unregister_session(&session_id)
            .await;
        assert!(
            !runtime.runtime_adapter.contains_session(&session_id).await,
            "test requires a persisted-only session with no runtime registration"
        );

        let (tx, _rx) = mpsc::channel(100);
        let err = runtime
            .start_turn_via_runtime(
                &session_id,
                "invalid keep_alive on persisted-only".into(),
                tx,
                None,
                None,
                None,
                Some(TurnOverrides {
                    keep_alive: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .expect_err("keep_alive=true must require comms_name on persisted-only sessions");

        assert_eq!(err.code, error::INVALID_PARAMS);
        assert_eq!(
            err.message,
            "keep_alive requires a session created with comms_name"
        );
        wait_for_runtime_unregistered(&runtime, &session_id, "persisted-only keep_alive rejection")
            .await;
        runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("invalid keep_alive override should release active capacity");
    }

    /// W2-G regression: once a mob has claimed peer-ingress ownership on a
    /// session, calling the session-runtime drain-reconfigure helpers with a
    /// different comms runtime must NOT swap the mob's comms runtime out
    /// from under it. This is the s71 silent-downgrade class referenced in
    /// issue #264: session-runtime code used to rewire a session's comms
    /// every turn, without checking whether a mob had already taken
    /// ownership.
    ///
    /// The W2-G fix enforces this structurally at two layers:
    ///   (a) DSL guard: `AttachSessionIngress` requires owner `== Unattached`,
    ///       so an attempted swap from `MobOwned` is rejected at the DSL
    ///       authority. This is the core structural guarantee.
    ///   (b) Shell short-circuit: `start_turn_via_runtime` (and the
    ///       materialization / recovery paths that also call
    ///       `update_peer_ingress_context`) consult
    ///       `peer_ingress_owner()` and skip the reconfigure branch when
    ///       the owner is `MobOwned`. This avoids spurious warnings and
    ///       the attendant rollback churn.
    ///
    /// This test exercises the runtime adapter directly (layer (a) + the
    /// shell short-circuit inside the adapter's
    /// `update_peer_ingress_context`). End-to-end `start_turn_via_runtime`
    /// coverage for mob-owned drains requires the mob harness to keep the
    /// session alive across the turn boundary, which is verified in the
    /// mob integration lane.
    #[tokio::test]
    async fn update_peer_ingress_context_preserves_mob_owned_identity() {
        use meerkat_core::agent::CommsRuntime as CommsRuntimeTrait;

        struct StubCommsRuntime;

        #[async_trait::async_trait]
        impl CommsRuntimeTrait for StubCommsRuntime {
            async fn drain_messages(&self) -> Vec<String> {
                Vec::new()
            }
            fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
                Arc::new(tokio::sync::Notify::new())
            }
            fn dismiss_received(&self) -> bool {
                true
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let adapter = runtime.runtime_adapter();

        // Mob claims peer-ingress ownership on behalf of this session.
        let mob_comms: Arc<dyn CommsRuntimeTrait> = Arc::new(StubCommsRuntime);
        let expected_id =
            meerkat_runtime::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&mob_comms);
        let mob_id = meerkat_runtime::meerkat_machine::dsl::MobId::from("mob-w2g-integration");
        adapter
            .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&mob_comms), mob_id.clone())
            .await;

        // Simulate the s71 regression path: session-runtime code tries to
        // reconfigure peer-ingress with a different comms runtime. Before
        // W2-G this would silently swap the mob-owned drain. With W2-G the
        // DSL's `AttachSessionIngress` guard rejects the transition.
        let session_comms: Arc<dyn CommsRuntimeTrait> = Arc::new(StubCommsRuntime);
        adapter
            .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&session_comms)))
            .await;

        // Assert the mob-owned drain's CommsRuntimeId is unchanged.
        let owner_after = adapter.peer_ingress_owner(&session_id).await;
        match owner_after {
            meerkat_runtime::PeerIngressOwner::MobOwned {
                comms_runtime_id,
                mob_id: actual_mob_id,
            } => {
                assert_eq!(
                    comms_runtime_id, expected_id,
                    "update_peer_ingress_context must not swap the mob-owned comms runtime id"
                );
                assert_eq!(actual_mob_id, mob_id);
            }
            other => {
                panic!("expected MobOwned to survive session-runtime swap attempt, got {other:?}")
            }
        }
    }

    #[test]
    fn completion_outcome_to_rpc_result_surfaces_callback_pending_payload() {
        let session_id = SessionId::new();
        let err = completion_outcome_to_rpc_result(
            meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: serde_json::json!({ "value": "browser" }),
            },
            &session_id,
        )
        .expect_err("callback pending should map to an RPC error");

        assert_eq!(err.code, error::INTERNAL_ERROR);
        assert_eq!(err.message, "callback pending for tool 'external_mock'");
        let data = err.data.expect("callback pending error data");
        assert_eq!(data["session_id"], session_id.to_string());
        assert_eq!(data["resumable"], true);
        assert_eq!(data["tool_name"], "external_mock");
        assert_eq!(data["args"], serde_json::json!({ "value": "browser" }));
    }

    #[test]
    fn completion_outcome_to_rpc_result_surfaces_cancelled() {
        let session_id = SessionId::new();
        let err = completion_outcome_to_rpc_result(
            meerkat_runtime::completion::CompletionOutcome::Cancelled,
            &session_id,
        )
        .expect_err("cancelled completion should map to an RPC error");

        assert_eq!(err.code, error::REQUEST_CANCELLED);
        assert_eq!(err.message, "request cancelled");
    }

    #[test]
    fn completion_outcome_to_rpc_result_surfaces_output_and_finalization_error() {
        let session_id = SessionId::new();
        let run_result = meerkat_core::RunResult {
            text: "{\"gate\":\"green\"}".to_string(),
            session_id: session_id.clone(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: Some(serde_json::json!({ "gate": "green" })),
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        };
        let err = completion_outcome_to_rpc_result(
            meerkat_runtime::completion::CompletionOutcome::CompletedWithFinalizationFailure {
                result: Box::new(run_result),
                error: meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                    "runtime loop commit failed: synthetic finalization failure",
                ),
            },
            &session_id,
        )
        .expect_err("finalization failure should map to an RPC error");

        assert_eq!(err.code, error::INTERNAL_ERROR);
        assert!(err.message.contains("synthetic finalization failure"));
        let data = err.data.expect("finalization failure error data");
        assert_eq!(data["error"]["kind"], "runtime_apply_failure");
        assert_eq!(data["error"]["terminal"], true);
        assert_eq!(
            data["run_result"]["structured_output"],
            serde_json::json!({ "gate": "green" })
        );
        assert_eq!(
            data["structured_output"],
            serde_json::json!({ "gate": "green" })
        );
    }

    #[test]
    fn session_error_to_rpc_surfaces_cancelled_as_request_cancelled() {
        let session_err = SessionError::Agent(meerkat_core::AgentError::Cancelled);
        let rpc_err = session_error_to_rpc(session_err);

        assert_eq!(rpc_err.code, error::REQUEST_CANCELLED);
    }

    // -- P2-6: Typed BuildError → PROVIDER_ERROR classification --

    #[test]
    fn test_build_error_missing_api_key_classifies_as_provider_error() {
        let session_err = SessionError::Agent(meerkat_core::AgentError::BuildError(
            "Missing API key for provider 'anthropic'".to_string(),
        ));
        let rpc_err = session_error_to_rpc(session_err);
        assert_eq!(
            rpc_err.code,
            error::PROVIDER_ERROR,
            "BuildError with missing API key must map to PROVIDER_ERROR, got code: {}",
            rpc_err.code,
        );
    }

    #[test]
    fn test_build_error_unknown_provider_classifies_as_provider_error() {
        let session_err = SessionError::Agent(meerkat_core::AgentError::BuildError(
            "Unknown provider for model 'llama-3'".to_string(),
        ));
        let rpc_err = session_error_to_rpc(session_err);
        assert_eq!(
            rpc_err.code,
            error::PROVIDER_ERROR,
            "BuildError with unknown provider must map to PROVIDER_ERROR, got code: {}",
            rpc_err.code,
        );
    }

    #[test]
    fn test_build_error_generic_classifies_as_provider_error() {
        // ALL BuildErrors should be PROVIDER_ERROR regardless of message content.
        // This is the point — we match on the variant, not the string.
        let session_err = SessionError::Agent(meerkat_core::AgentError::BuildError(
            "something completely unrelated to API keys or providers".to_string(),
        ));
        let rpc_err = session_error_to_rpc(session_err);
        assert_eq!(
            rpc_err.code,
            error::PROVIDER_ERROR,
            "ALL BuildErrors must map to PROVIDER_ERROR regardless of message content, got code: {}",
            rpc_err.code,
        );
    }
}

use super::*;
use meerkat_core::AgentExecutionSnapshot;
use meerkat_core::CommsCapabilityError;
use meerkat_core::ExternalToolSurfaceSnapshot;
use meerkat_core::PeerIngressRuntimeSnapshot;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::Session;
use meerkat_core::ToolScopeSnapshot;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::service::{AppendSystemContextRequest, StartTurnRequest};
use meerkat_core::service::{
    SessionControlError, SessionError, SessionServiceCommsExt, SessionServiceControlExt,
    SessionServiceHistoryExt,
};
use meerkat_core::{InputId, RunId};
use sha2::{Digest, Sha256};
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::sync::{Mutex, OnceLock, Weak};

fn build_runtime_receipt(
    run_id: RunId,
    boundary: RunApplyBoundary,
    contributing_input_ids: Vec<InputId>,
    session: &Session,
) -> Result<RunBoundaryReceiptDraft, SessionError> {
    let encoded_messages = serde_json::to_vec(session.messages()).map_err(|err| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "failed to serialize session for runtime receipt digest: {err}"
        )))
    })?;
    Ok(RunBoundaryReceiptDraft {
        run_id,
        boundary,
        contributing_input_ids,
        conversation_digest: Some(format!("{:x}", Sha256::digest(encoded_messages))),
        message_count: session.messages().len(),
    })
}

fn session_control_error_to_session_error(err: SessionControlError) -> SessionError {
    match err {
        SessionControlError::Session(err) => err,
        other => SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            other.to_string(),
        )),
    }
}

#[cfg(feature = "runtime-adapter")]
fn ephemeral_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
fn persistent_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(feature = "runtime-adapter")]
fn cached_runtime_adapter(
    cache: &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>,
    key: usize,
    init: impl FnOnce() -> Arc<meerkat_runtime::MeerkatMachine>,
) -> Arc<meerkat_runtime::MeerkatMachine> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|_, adapter| adapter.strong_count() > 0);
    if let Some(existing) = cache.get(&key).and_then(Weak::upgrade) {
        return existing;
    }
    let adapter = init();
    cache.insert(key, Arc::downgrade(&adapter));
    adapter
}

#[cfg(feature = "runtime-adapter")]
pub(crate) async fn retire_runtime_session_for_archive(
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    session_id: &SessionId,
) -> Result<(), SessionError> {
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
    if runtime_adapter
        .meerkat_machine_archive_snapshot(session_id)
        .await
        .is_some_and(|snapshot| {
            matches!(
                snapshot.control.phase,
                meerkat_runtime::RuntimeState::Retired | meerkat_runtime::RuntimeState::Stopped
            )
        })
    {
        return Ok(());
    }
    match meerkat_runtime::RuntimeControlPlane::retire(runtime_adapter, &runtime_id).await {
        Ok(_) => Ok(()),
        Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => {
            runtime_adapter
                .register_session(session_id.clone())
                .await
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "machine archive register before retire failed: {error}"
                    )))
                })?;
            if runtime_adapter
                .meerkat_machine_archive_snapshot(session_id)
                .await
                .is_some_and(|snapshot| {
                    matches!(
                        snapshot.control.phase,
                        meerkat_runtime::RuntimeState::Retired
                            | meerkat_runtime::RuntimeState::Stopped
                    )
                })
            {
                return Ok(());
            }
            meerkat_runtime::RuntimeControlPlane::retire(runtime_adapter, &runtime_id)
                .await
                .map(|_| ())
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "machine archive retire failed after registration: {error}"
                    )))
                })
        }
        Err(error) => Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "machine archive retire failed: {error}"
            )),
        )),
    }
}

// ---------------------------------------------------------------------------
// MobSessionService trait extension
// ---------------------------------------------------------------------------

/// Extension trait for session services used by the mob runtime.
///
/// Builds on `SessionServiceCommsExt` from core so mob orchestration can use
/// comms/injector access without per-crate bridge traits.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait MobSessionService:
    SessionServiceCommsExt + SessionServiceControlExt + SessionServiceHistoryExt
{
    /// Subscribe to session-wide events regardless of triggering interaction.
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        <Self as SessionService>::subscribe_session_events(self, session_id).await
    }

    /// Whether this service satisfies the persistent-session contract required
    /// by REQ-MOB-030.
    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        None
    }

    /// Apply a live hard cancel after `MeerkatMachine` accepts the cancel command.
    ///
    /// The machine has ALREADY admitted the interrupt when this runs — the
    /// implementation applies it to the LIVE agent (e.g. the ephemeral
    /// session task's interrupt slot) and returns. It must NOT call back
    /// into `MeerkatMachine::hard_cancel_current_run`: this method executes
    /// inside the machine's interrupt dispatch, so re-entering the machine
    /// forms a dispatch ring (deadlock or unbounded recursion).
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "interrupt for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    /// Apply a live cooperative boundary cancel after `MeerkatMachine` accepts the command.
    ///
    /// The machine has ALREADY admitted the cancel when this runs — the
    /// implementation applies it to the LIVE agent (e.g. the ephemeral
    /// session task's cancel-after-boundary channel) and returns. It must
    /// NOT call back into `MeerkatMachine::cancel_after_boundary`: this
    /// method executes inside the machine's boundary-cancel dispatch, so
    /// re-entering the machine forms a dispatch ring (the machine's
    /// `boundary_cancel_dispatch_pending` fact bounds it to one extra lap,
    /// but the re-entrant lap is still a contract violation).
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "cancel_after_boundary for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    async fn execution_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        Ok(None)
    }

    async fn tool_scope_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        Ok(None)
    }

    async fn external_tool_surface_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        Ok(None)
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        Ok(None)
    }

    /// Whether the mob archive authority owns this session's durable record.
    ///
    /// Disposal routes on this fact: owned sessions archive through the mob
    /// authority (where a mid-archive record loss stays a fail-closed
    /// split-state error), while sessions adopted from a host-owned store
    /// (`MemberLaunchMode::Resume` over a service the mob does not own, e.g.
    /// an embedder's console sessions) retire their runtime and release the
    /// binding without touching the authority — archiving a session the mob
    /// never owned is not this mob's to perform.
    ///
    /// Default `true`: a service without a durable read seam claims every
    /// session it is asked about, preserving the fail-closed archive path.
    /// Persistent services override this with a real store read.
    async fn session_known_to_archive_authority(
        &self,
        _session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(true)
    }

    /// Whether a listed session belongs to the given mob for reconciliation.
    ///
    /// Default: `false`. The wave-a demolition removed the comms-name matching
    /// probe; wave-c was intended to land a runtime-aware replacement but did
    /// not, so this remains a no-op default. Persistent services may override
    /// to implement real reconciliation; the ephemeral default treats no
    /// listed session as "belongs to mob".
    async fn session_belongs_to_mob(
        &self,
        _session_id: &SessionId,
        _mob_id: &crate::ids::MobId,
    ) -> bool {
        false
    }

    /// Load the persisted session snapshot when available.
    ///
    /// Default: `Ok(None)`. Matches the pre-demolition behavior where services
    /// without durable persistence returned no snapshot, letting callers treat
    /// the session as missing and fall through to recreate-from-roster paths.
    async fn load_persisted_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(None)
    }

    /// Archive a mob-owned session through the strongest lifecycle authority
    /// this service exposes. Runtime-backed persistent services override this
    /// to require a concrete `MeerkatMachine` archive protocol before writing
    /// archive projection state.
    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if self.runtime_adapter().is_some() {
            return Err(SessionError::Unsupported(format!(
                "archive for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
            )));
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        _session_id: &SessionId,
        _run_id: RunId,
        _req: StartTurnRequest,
        _boundary: RunApplyBoundary,
        _contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime-backed apply is unavailable for this session service".into(),
            ),
        ))
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
        for append in appends {
            self.append_system_context(
                session_id,
                AppendSystemContextRequest {
                    content: append.content,
                    source: append.source,
                    idempotency_key: append.idempotency_key,
                    source_kind: append.source_kind,
                    peer_response_terminal: None,
                },
            )
            .await
            .map_err(session_control_error_to_session_error)?;
        }

        Ok(CoreApplyOutput::without_terminal(
            RunBoundaryReceiptDraft {
                run_id,
                boundary,
                contributing_input_ids,
                conversation_digest: None,
                message_count: 0,
            },
            None,
        ))
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        _session_id: &SessionId,
        _appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime-backed context staging is unavailable for this session service".into(),
            ),
        ))
    }

    /// Stage runtime-owned context for an already-active turn's next LLM
    /// boundary. Unlike `apply_runtime_system_context_for_turn`, this must not
    /// route through the session task mailbox: the session task may be the
    /// active turn currently waiting for that boundary.
    async fn stage_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<Option<Vec<u8>>, SessionError> {
        let _ = (session_id, expected_run_id, appends);
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::NoPendingBoundary,
        ))
    }

    async fn discard_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        idempotency_keys: Vec<String>,
    ) -> Result<(), SessionError> {
        let _ = (session_id, expected_run_id, idempotency_keys);
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::NoPendingBoundary,
        ))
    }

    async fn active_turn_system_context_boundary_available(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<bool>, SessionError> {
        Ok(None)
    }

    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        _session_id: &SessionId,
        _session_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        Ok(())
    }

    async fn discard_live_session(&self, _session_id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    /// Cancel all active checkpointer gates.
    ///
    /// After this call in-flight saves complete but subsequent checkpoint
    /// calls on any session are no-ops. Call during `stop()` to prevent
    /// checkpoint writes from racing with external cleanup.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates cancelled by [`cancel_all_checkpointers`].
    ///
    /// Call during `resume()` to restore periodic persistence.
    async fn rearm_all_checkpointers(&self) {}
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl<B> MobSessionService for meerkat_session::EphemeralSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        let key = std::ptr::from_ref(self) as usize;
        Some(cached_runtime_adapter(
            ephemeral_runtime_adapter_cache(),
            key,
            || Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_core::service::SessionService::interrupt(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_core::service::SessionService::cancel_after_boundary(self, session_id).await
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Self as SessionService>::read(self, session_id).await?;
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            retire_runtime_session_for_archive(runtime_adapter.as_ref(), session_id).await?;
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::execution_snapshot(self, session_id).await
    }

    /// Export the LIVE session: for the ephemeral backend the live session is
    /// the canonical session, and callers that read session-owned facts
    /// through this seam (e.g. parent tool-access-policy resolution for
    /// spawn/fork `Inherit`) must see it. The trait default's `Ok(None)`
    /// would silently coalesce "session invisible to this read seam" into
    /// "session has no such fact" — on the policy path that is a fail-open
    /// containment escape (a restricted parent minting unrestricted
    /// children).
    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        match meerkat_session::EphemeralSessionService::<B>::export_session(self, session_id).await
        {
            Ok(session) => Ok(Some(session)),
            // A genuinely absent session is the documented `None` shape;
            // every other fault propagates so policy resolution fails closed.
            Err(SessionError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::tool_scope_snapshot(self, session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::external_tool_surface_snapshot(
            self, session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::EphemeralSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let run_result =
            meerkat_session::EphemeralSessionService::<B>::start_turn(self, session_id, req)
                .await?;
        let session =
            meerkat_session::EphemeralSessionService::<B>::export_session(self, session_id).await?;
        let receipt = build_runtime_receipt(run_id, boundary, contributing_input_ids, &session)?;
        let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;
        Ok(CoreApplyOutput::with_run_result(
            receipt,
            Some(session_snapshot),
            run_result,
        ))
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::apply_runtime_context_appends(
            self,
            session_id,
            run_id,
            appends,
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
        meerkat_session::EphemeralSessionService::<B>::apply_runtime_context_appends_with_boundary(
            self,
            session_id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        session_id: &SessionId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::apply_runtime_system_context(
            self, session_id, appends,
        )
        .await
    }

    async fn stage_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<Option<Vec<u8>>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::stage_runtime_system_context_for_active_turn(
            self,
            session_id,
            expected_run_id,
            appends,
        )
        .await?;
        Ok(None)
    }

    async fn discard_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        idempotency_keys: Vec<String>,
    ) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_runtime_system_context_for_active_turn(
            self,
            session_id,
            expected_run_id,
            idempotency_keys,
        )
        .await?;
        Ok(())
    }

    async fn active_turn_system_context_boundary_available(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<bool>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::active_turn_system_context_boundary_available(
            self, session_id,
        )
        .await
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let key = std::ptr::from_ref(self) as usize;
            let store = self.runtime_store();
            Some(cached_runtime_adapter(
                persistent_runtime_adapter_cache(),
                key,
                || {
                    Arc::new(meerkat_runtime::MeerkatMachine::persistent(
                        store,
                        self.blob_store(),
                    ))
                },
            ))
        }
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let Some(session) = self.load_authoritative_session(session_id).await? else {
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

    async fn session_known_to_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        // Authoritative store read, deliberately NOT the archived-filtered
        // `load_persisted_session`: an already-archived session is still
        // OWNED by this authority (the archive path handles idempotence);
        // only a session with no record at all is host-owned.
        Ok(self.load_authoritative_session(session_id).await?.is_some())
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            return meerkat_session::PersistentSessionService::<B>::archive_with_machine_protocol(
                self,
                session_id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(
                    runtime_adapter.as_ref(),
                ),
            )
            .await;
        }

        <Self as SessionService>::archive(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::interrupt_with_machine_authority(
            self, session_id, authority,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::cancel_after_boundary_with_machine_authority(
            self, session_id, authority,
        )
        .await
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::execution_snapshot(self, session_id).await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::tool_scope_snapshot(self, session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::external_tool_surface_snapshot(
            self, session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::PersistentSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::PersistentSessionService::<B>::apply_runtime_turn(
            self,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
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
        meerkat_session::PersistentSessionService::<B>::apply_runtime_context_appends(
            self,
            session_id,
            run_id,
            appends,
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
        meerkat_session::PersistentSessionService::<B>::apply_runtime_context_appends_with_boundary(
            self,
            session_id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        session_id: &SessionId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::apply_runtime_system_context_for_turn(
            self, session_id, appends,
        )
        .await
    }

    async fn stage_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<Option<Vec<u8>>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::stage_live_system_context_boundary_snapshot(
            self,
            session_id,
            expected_run_id,
            appends,
        )
        .await
    }

    async fn discard_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        idempotency_keys: Vec<String>,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_system_context_boundary_staging(
            self,
            session_id,
            expected_run_id,
            idempotency_keys,
        )
        .await?;
        Ok(())
    }

    async fn active_turn_system_context_boundary_available(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<bool>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::active_turn_system_context_boundary_available(
            self, session_id,
        )
        .await
    }

    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        session_id: &SessionId,
        session_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::checkpoint_committed_runtime_session_snapshot(
            self,
            session_id,
            session_snapshot,
        )
        .await
    }

    async fn cancel_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::cancel_all_checkpointers(self).await;
    }

    async fn rearm_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::rearm_all_checkpointers(self).await;
    }
}

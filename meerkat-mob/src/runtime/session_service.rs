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
    // No shell phase probes: the machine owns every retire verdict.
    // `Retire` admits Idle/Attached/Running/Stopped (Stopped retires
    // durably instead of the former silent early-return that stranded
    // stopped sessions un-retired), and `RetireAlreadyRetired` no-ops a
    // Retired machine. An unregistered session registers first — recovery
    // seeds the durable phase and the retire then lands as a machine
    // transition on it.
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
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
    /// Create while the caller already owns this session's stable runtime-turn
    /// finalization boundary. Persistent implementations override this to use
    /// their non-reentrant boundary-aware admission seam; simple/mock services
    /// may use ordinary creation because they have no nested boundary owner.
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError>;

    /// Create while B is held and publish the exact live actor incarnation as
    /// soon as registry insertion succeeds. Every service that exposes a
    /// runtime adapter must implement this with its actor-owning registry;
    /// registry-less services cannot participate in runtime-backed attachment.
    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service cannot publish exact actor identity during boundary-owned create"
                .into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support machine-authorized archived resume".into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support boundary-owned machine-authorized archived resume"
                .into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        _req: meerkat_core::service::CreateSessionRequest,
        _authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        _actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support exact-actor boundary-owned machine-authorized archived resume"
                .into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn promote_revivable_retired_session(
        &self,
        _session_id: &SessionId,
        _authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::PromotedArchivedResumeCommitLease, SessionError> {
        Err(SessionError::Unsupported(
            "session service does not support archived document revival".into(),
        ))
    }
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

    /// Mechanical presence of the live session actor, without exporting its
    /// document or reconciling durable authority. Health/revival uses this
    /// process-carrier witness so an actor busy inside a provider turn cannot
    /// block observation or be mistaken for dead state.
    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Self as SessionService>::has_live_session(self, session_id).await
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        None
    }

    /// Whether this service implements the runtime-owned turn application
    /// boundary used by [`MeerkatMachine`]. Merely exposing a runtime adapter
    /// is not sufficient: some hosts use that adapter only for lifecycle or
    /// comms authority and still execute turns through the direct
    /// [`SessionService::start_turn`] path.
    ///
    /// Per-turn LLM identity overrides are legal only when this capability is
    /// true, because the executor must apply the identity immediately before
    /// the exact admitted turn runs.
    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        false
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
        _expected_run_id: &RunId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "cancel_after_boundary for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
        )))
    }

    /// Apply a queued attachment-local cooperative cancel using explicit
    /// current-run semantics. Cloneable boundary handles must use the exact-run
    /// method above.
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "current-run cancel_after_boundary for runtime-backed mob session {session_id} must be implemented by the machine-owned session service"
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

    /// Load a retired session only for an explicit resume/revival operation.
    /// Ordinary reads remain archive-filtered. Implementations must return a
    /// value only when an intact authoritative snapshot and a Retired runtime
    /// lifecycle record coexist.
    async fn load_revivable_retired_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(None)
    }

    /// Load the persisted session METADATA view when available.
    ///
    /// Metadata-only sibling of [`Self::load_persisted_session`] (mobkit
    /// ask-24 clause 3): ownership routing, policy resolution, and presence
    /// probes that only need session-authority metadata facts must not force
    /// a full transcript materialization. Visibility parity with
    /// `load_persisted_session` is part of the contract: whenever that method
    /// returns `Ok(None)` (absent or archived), this one does too.
    ///
    /// Default: derives from `load_persisted_session`, so every backend
    /// exposes the seam with identical visibility. Persistent services
    /// override this with the metadata-only authoritative read. Corrupt
    /// durable metadata is a read FAULT (`Err`), never `Ok(None)`.
    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        let Some(session) = self.load_persisted_session(session_id).await? else {
            return Ok(None);
        };
        meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
            .map(Some)
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "session {session_id} durable metadata failed typed restore: {err}"
                )))
            })
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

    /// Archive while the caller owns the exact session turn-finalization
    /// boundary. Persistent implementations must use a non-reentrant service
    /// seam; the default is suitable only for services whose boundary is a
    /// no-op.
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError>;

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

    /// Prepare one exact already-active LLM boundary. Success means the actor
    /// is parked and owned by the returned non-clone commit/abort authority.
    async fn prepare_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        let _ = (session_id, expected_run_id, appends);
        Err(meerkat_core::CoreBoundaryStageError::unavailable(
            "session service does not support exact active-turn boundary preparation",
        ))
    }

    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        _session_id: &SessionId,
        _session_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        Ok(())
    }

    /// Acquire the stable session mutation boundary shared by runtime turns,
    /// direct turns, and non-turn durable writers. Ephemeral/custom services
    /// without such a boundary retain the no-op default.
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        _session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(()))
    }

    /// Checkpoint when the caller already owns the stable outer boundary.
    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        session_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        self.checkpoint_committed_runtime_session_snapshot(session_id, session_snapshot)
            .await
    }

    /// Remove the service-side live actor while the owning runtime entry is in
    /// its generated post-stop unregister window.
    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session(session_id).await
    }

    /// Cleanup when the caller already owns the stable outer boundary.
    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session_after_runtime_stop_terminalized(session_id)
            .await
    }

    async fn publish_interaction_terminals(
        &self,
        _session_id: &SessionId,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        Err(SessionError::Unsupported(
            "exact interaction terminal publication requires a persistent session service"
                .to_string(),
        ))
    }

    async fn discard_live_session(&self, _session_id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    /// Remove a live actor while the caller already owns its stable outer
    /// turn-finalization boundary.
    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError>;

    /// Compare-and-remove one exact actor while the caller already owns B.
    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        Err(SessionError::Unsupported(format!(
            "session service cannot discard exact live actor for {}",
            witness.session_id()
        )))
    }

    /// Await terminal drain of the current live incarnation's durable event
    /// projection after its producer has been quiesced and discarded.
    async fn await_event_projection_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        let _ = session_id;
        Ok(false)
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
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Self as meerkat_core::service::SessionService>::create_session(self, req).await
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        #[cfg(feature = "runtime-adapter")]
        let mut actor_materialization_permit = if let Some(bindings) =
            req.build
                .as_ref()
                .and_then(|build| match &build.runtime_build_mode {
                    meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => Some(bindings),
                    meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                }) {
            match meerkat_runtime::begin_session_runtime_actor_materialization(bindings) {
                Ok(permit) => Some(permit),
                Err(meerkat_runtime::RuntimeActorMaterializationError::RegistrationClosed) => {
                    return Err(SessionError::NotFound {
                        id: bindings.session_id().clone(),
                    });
                }
                Err(meerkat_runtime::RuntimeActorMaterializationError::InvalidAuthority(
                    reason,
                )) => {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(reason),
                    ));
                }
            }
        } else {
            None
        };

        let (result, actor_witness) =
            meerkat_session::EphemeralSessionService::<B>::create_session_with_admission_and_witness(
                self,
                req,
                None,
                Some(actor_witness_slot),
            )
            .await?;

        #[cfg(feature = "runtime-adapter")]
        if let Some(permit) = actor_materialization_permit.take()
            && let Err(error) = permit.commit()
        {
            let cleanup =
                meerkat_session::EphemeralSessionService::<B>::discard_live_session_actor(
                    self,
                    &actor_witness,
                )
                .await;
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(match cleanup {
                    Ok(_) => format!(
                        "runtime actor materialization commit failed for session {}: {error}",
                        result.session_id
                    ),
                    Err(cleanup_error) => format!(
                        "runtime actor materialization commit failed for session {}: {error}; exact actor cleanup also failed: {cleanup_error}",
                        result.session_id
                    ),
                }),
            ));
        }

        #[cfg(not(feature = "runtime-adapter"))]
        let _ = actor_witness;
        Ok(result)
    }

    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(
            meerkat_session::EphemeralSessionService::<B>::live_session_actor_registered(
                self, session_id,
            )
            .await,
        )
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
    fn supports_runtime_turn_apply(&self) -> bool {
        true
    }

    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(
            meerkat_session::EphemeralSessionService::<B>::acquire_runtime_turn_finalization_guard(
                self, session_id,
            )
            .await,
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
        expected_run_id: &RunId,
        _authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_core::service::SessionService::cancel_after_boundary_for_run(
            self,
            session_id,
            expected_run_id,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
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

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.archive_with_mob_lifecycle_authority(session_id).await
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

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session_actor(self, witness)
            .await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let run_result = meerkat_session::EphemeralSessionService::<B>::start_turn_under_runtime_turn_finalization_boundary(
            self,
            session_id,
            req,
        )
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

    async fn prepare_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        let prepared = meerkat_session::EphemeralSessionService::<B>::prepare_runtime_system_context_for_active_turn(
            self, session_id, expected_run_id, appends,
        )
        .await?;
        Ok(prepared.into_stage_output(None))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_admission_under_runtime_turn_boundary(req, admission)
            .await
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_admission_and_actor_witness_under_runtime_turn_boundary(
            req,
            admission,
            actor_witness_slot,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_machine_archived_resume_admission(
            req,
            admission,
            authorization,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_machine_archived_resume_admission_under_runtime_turn_boundary(
            req,
            admission,
            authorization,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let admission = self.reserve_create_session_admission().await?;
        self.create_session_with_reserved_machine_archived_resume_admission_and_actor_witness_under_runtime_turn_boundary(
            req,
            admission,
            authorization,
            actor_witness_slot,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn promote_revivable_retired_session(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::PromotedArchivedResumeCommitLease, SessionError> {
        self.revive_archived_session_with_prepared_materialization(session_id, authority)
            .await
    }
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(
            meerkat_session::PersistentSessionService::<B>::live_session_actor_registered(
                self, session_id,
            )
            .await,
        )
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

    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        true
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

    async fn load_revivable_retired_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        if self.persisted_runtime_state(session_id).await?
            != Some(meerkat_runtime::RuntimeState::Retired)
        {
            return Ok(None);
        }
        self.load_authoritative_session(session_id).await
    }

    /// Metadata-only authoritative read. Same visibility contract as
    /// [`Self::load_persisted_session`] — the archived filter runs through
    /// `session_archived_by_authority_with_terminal`, the terminal-fact
    /// sibling of the full-session overload, so archived sessions read as
    /// `None` on both seams.
    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        let Some(view) = self.load_authoritative_session_metadata(session_id).await? else {
            return Ok(None);
        };
        if self
            .session_archived_by_authority_with_terminal(
                session_id,
                view.lifecycle_terminal.as_ref(),
            )
            .await?
        {
            return Ok(None);
        }
        Ok(Some(view))
    }

    async fn session_known_to_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        // Direct durable-carrier ownership read, deliberately NOT the
        // visibility-arbitrated / archived-filtered `load_persisted_session`:
        // an already-archived row and a deferred pre-first-turn store row are
        // both still OWNED by this authority. Only a session with neither a
        // store projection nor a runtime snapshot is host-owned.
        meerkat_session::PersistentSessionService::<B>::session_known_to_archive_authority(
            self, session_id,
        )
        .await
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

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(runtime_adapter) = self.runtime_adapter() {
            return meerkat_session::PersistentSessionService::<B>::archive_with_machine_protocol_under_runtime_turn_boundary(
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
        expected_run_id: &RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::cancel_after_boundary_with_machine_authority(
            self, session_id, expected_run_id, authority,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::cancel_current_after_boundary_with_machine_authority(
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

    async fn await_event_projection_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        meerkat_session::PersistentSessionService::<B>::event_log_await_projection_drain(
            self, session_id,
        )
        .await
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

    async fn prepare_runtime_system_context_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &RunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        meerkat_session::PersistentSessionService::<B>::prepare_live_system_context_boundary(
            self,
            session_id,
            expected_run_id,
            appends,
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

    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(
            meerkat_session::PersistentSessionService::<B>::acquire_runtime_turn_finalization_guard(
                self,
                session_id,
            )
            .await,
        ))
    }

    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        session_snapshot: &[u8],
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::checkpoint_committed_runtime_session_snapshot_under_runtime_turn_boundary(
            self,
            session_id,
            session_snapshot,
        )
        .await
    }

    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_after_runtime_stop_terminalized(
            self,
            session_id,
        )
        .await
    }

    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_after_runtime_stop_terminalized_under_runtime_turn_boundary(
            self,
            session_id,
        )
            .await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_under_runtime_turn_boundary(
            self,
            session_id,
        )
        .await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session_actor_under_runtime_turn_boundary(
            self, witness,
        )
        .await
    }

    async fn publish_interaction_terminals(
        &self,
        session_id: &SessionId,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        meerkat_session::PersistentSessionService::<B>::publish_interaction_terminals_exact_batch(
            self, session_id, events,
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

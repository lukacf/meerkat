//! Staged-session promotion lifecycle.
//!
//! Populated by W1-B (`PendingPromotionCleanup` + `Mode` enum + Drop)
//! and W2-D (the staged-session promotion methods on the runtime).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreApplyTerminal, CoreExecutorTeardownReason,
};
use meerkat_core::lifecycle::run_primitive::ConversationContextAppend;
use meerkat_core::service::{CreateSessionRequest, SessionError, SessionService, StartTurnRequest};
use meerkat_core::types::RunResult;
use meerkat_core::types::SessionId;
use meerkat_core::{
    ContentInput, InputId, PendingSystemContextAppend, RunApplyBoundary, RunId, Session,
    SessionSystemContextState,
};
use meerkat_runtime::MeerkatMachine;

use meerkat_session::{
    MachineServiceTurnCommitProtocol, MachineSessionArchiveProtocol, PersistentSessionService,
};

use crate::session_runtime::admission::{
    ActiveCapacityGuard, StagedCapacityAdmissions, restore_staged_capacity_admission,
};
use crate::{
    AgentBuildConfig, FactoryAgentBuilder, GeneratedMachineArchivedResumeAdmission, PromotingSlot,
    StagedSessionRegistry,
};

/// Boxed-future shape used by the spawned-task callbacks the staged-promotion
/// helpers accept. Surfaces wrap their RPC-shaped closures into this type so
/// the moved free functions stay surface-agnostic.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Replay callback fired by [`spawn_pending_create_and_apply_runtime_turn_with_admission_guard`]
/// when finishing a successful promotion. Surfaces translate the
/// per-surface error into a stringly message for tracing.
pub type ReplayPromotedSystemContextFn = Arc<
    dyn Fn(
            SessionId,
            SessionSystemContextState,
            SessionSystemContextState,
        ) -> BoxFut<'static, Result<(), String>>
        + Send
        + Sync,
>;

/// Test-only pre-turn hook fired by the staged-promotion helpers right
/// before the underlying turn is dispatched. Surfaces wrap their RPC
/// hook slot into this callback shape.
pub type PreTurnHookFn = Arc<dyn Fn() -> BoxFut<'static, ()> + Send + Sync>;

/// Convert a slice of [`ConversationContextAppend`] entries into the
/// runtime-side [`PendingSystemContextAppend`] vector that staged
/// promotion replays into the session service after a turn commits.
#[must_use]
pub fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            content: append.content.clone(),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            // Durable keyed conversation context append — not a transient steer.
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            accepted_at,
            peer_response_terminal: None,
        })
        .collect()
}

/// Awaiter result shape for the staged-session apply-runtime-turn
/// helpers; surfaces translate `Err(StagedTaskJoinError)` onto their
/// own wire error.
#[derive(Debug, thiserror::Error)]
#[error("session service apply_runtime_turn task ended before reporting a result for {session_id}")]
pub struct StagedTaskJoinError {
    /// Session id whose worker task vanished before reporting.
    pub session_id: SessionId,
}

/// Type alias mirroring the receiver shape used by the staged-session
/// promotion paths.
pub type ServiceApplyRuntimeTurnResultReceiver =
    tokio::sync::oneshot::Receiver<Result<CoreApplyOutput, SessionError>>;

/// Type alias mirroring the receiver shape used by the staged-session
/// promotion paths when an admission needs to be returned alongside the
/// turn result.
pub type RecoverableServiceApplyRuntimeTurnResultReceiver = tokio::sync::oneshot::Receiver<(
    Result<CoreApplyOutput, SessionError>,
    Option<ActiveCapacityGuard>,
)>;

async fn unregister_session_compensation(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    primary_error: &impl std::fmt::Display,
) -> Result<(), SessionError> {
    runtime_adapter
        .unregister_session(session_id)
        .await
        .map_err(|cleanup_error| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "{primary_error}; additionally failed to unregister runtime session {session_id} during staged-session compensation: {cleanup_error}"
            )))
        })
}

/// Await the spawned `apply_runtime_turn` task and translate task-vanish
/// errors into a typed [`StagedTaskJoinError`]. Surfaces map the inner
/// `SessionError` onto their wire shape.
pub async fn await_service_apply_runtime_turn(
    session_id: &SessionId,
    result_rx: ServiceApplyRuntimeTurnResultReceiver,
) -> Result<Result<CoreApplyOutput, SessionError>, StagedTaskJoinError> {
    result_rx.await.map_err(|_| StagedTaskJoinError {
        session_id: session_id.clone(),
    })
}

/// Variant of [`await_service_apply_runtime_turn`] that surfaces a
/// recovered admission alongside the turn result.
pub async fn await_service_apply_runtime_turn_with_recoverable_admission(
    session_id: &SessionId,
    result_rx: RecoverableServiceApplyRuntimeTurnResultReceiver,
) -> Result<
    (
        Result<CoreApplyOutput, SessionError>,
        Option<ActiveCapacityGuard>,
    ),
    StagedTaskJoinError,
> {
    result_rx.await.map_err(|_| StagedTaskJoinError {
        session_id: session_id.clone(),
    })
}

/// Pure predicate: did `apply_runtime_turn` fail before the
/// runtime-driven turn boundary closed?
///
/// Either the dispatched [`SessionError`] is `Agent::NoPendingBoundary`
/// (the boundary was never opened), or the [`CoreApplyOutput`] reached
/// [`CoreApplyTerminal::NoPendingBoundary`] (the dispatcher commit
/// raced ahead of the boundary close). Both shapes mean the session
/// service surface saw the error before it had a chance to project the
/// terminal state, so the caller must restore the staged slot.
#[must_use]
pub fn is_pre_run_apply_runtime_turn_failure(
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

/// Pure predicate: did the underlying `create_session` call reject
/// because the session was archived?
#[must_use]
pub fn is_archived_create_rejection(error: &SessionError) -> bool {
    matches!(error, SessionError::NotFound { .. })
}

/// Probe the persistent session service for a session that is still
/// in `live_deferred_first_turn_pending` state. Used by the staged
/// promotion paths to decide whether to restore the staged slot after
/// a service-side error.
pub async fn pending_live_first_turn_is_still_deferred(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    session_id: &SessionId,
) -> Result<bool, SessionError> {
    service.live_deferred_first_turn_pending(session_id).await
}

/// Replay any promoted system-context state captured during staged
/// promotion back onto the session service after a successful turn.
///
/// `replay_promoted_system_context_on_service` is owned by SessionRuntime
/// (it returns the RPC `RpcError`), so this helper takes a `replay`
/// closure that the caller wires up with the right error mapping. The
/// replay only fires when there's a pending promoting state to reap.
///
/// Surfaces that don't need a custom replay closure can pass a no-op,
/// but the staged promotion paths in `meerkat-rpc` rely on the side
/// effect — preserving the contract is the caller's responsibility.
pub async fn finish_pending_promotion_after_service_turn<R, F, E>(
    staged_sessions: &StagedSessionRegistry,
    session_id: &SessionId,
    operation: &'static str,
    replay: R,
) where
    R: FnOnce(SessionSystemContextState, SessionSystemContextState) -> F,
    F: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    let Some((starting_system_context_state, current_system_context_state)) = staged_sessions
        .take_promoting_system_context_state(session_id)
        .await
    else {
        return;
    };
    if let Err(err) = replay(starting_system_context_state, current_system_context_state).await {
        tracing::warn!(
            session_id = %session_id,
            error = %err,
            operation,
            "failed to replay promoted system-context state after service turn"
        );
    }
}

/// Pure predicate: should the staged slot be restored after a runtime
/// `apply_runtime_turn` failure? True when the apply failed before the
/// runtime-driven boundary closed, or when the underlying live session
/// is still in deferred-first-turn-pending state.
pub async fn should_restore_pending_after_apply_runtime_turn(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    session_id: &SessionId,
    result: &Result<CoreApplyOutput, SessionError>,
) -> Result<bool, SessionError> {
    if is_pre_run_apply_runtime_turn_failure(result) {
        return Ok(true);
    }
    if result.is_err() {
        pending_live_first_turn_is_still_deferred(service, session_id).await
    } else {
        Ok(false)
    }
}

/// Pure predicate: should the staged slot be restored after a runtime
/// `start_turn` failure? Mirrors [`should_restore_pending_after_apply_runtime_turn`]
/// for the start-turn path. Used only by the test-only spawn helper but
/// kept un-gated so `meerkat-rpc` can call it from its own `cfg(test)`
/// builds even though the upstream `meerkat` crate is built without
/// the `cfg(test)` flag.
pub async fn should_restore_pending_after_start_turn(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    session_id: &SessionId,
    result: &Result<RunResult, SessionError>,
) -> Result<bool, SessionError> {
    if matches!(
        result,
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::NoPendingBoundary
        ))
    ) {
        return Ok(true);
    }
    if result.is_err() {
        pending_live_first_turn_is_still_deferred(service, session_id).await
    } else {
        Ok(false)
    }
}

/// Result-receiver returned by [`spawn_pending_create_and_apply_runtime_turn_with_admission_guard`]
/// and the recovered/recovery variants.
pub type ServiceApplyRuntimeTurnSpawnReceiver =
    tokio::sync::oneshot::Receiver<Result<CoreApplyOutput, StagedApplyRuntimeTurnError>>;

/// Result-receiver returned by [`spawn_pending_create_and_start_turn_with_admission_guard`].
pub type ServiceStartTurnSpawnReceiver =
    tokio::sync::oneshot::Receiver<Result<RunResult, SessionError>>;

/// Typed result from a promotion task running under `CoreExecutor::apply`.
/// Teardown requests are preserved separately from ordinary service failures
/// so the caller can hand the exact executor back before machine cleanup.
#[derive(Debug, thiserror::Error)]
pub enum StagedApplyRuntimeTurnError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("runtime teardown required ({reason:?}): {message}")]
    TeardownRequired {
        reason: CoreExecutorTeardownReason,
        message: String,
    },
}

impl StagedApplyRuntimeTurnError {
    pub fn teardown(reason: CoreExecutorTeardownReason, message: impl Into<String>) -> Self {
        Self::TeardownRequired {
            reason,
            message: message.into(),
        }
    }

    pub fn archived_teardown(message: impl Into<String>) -> Self {
        Self::teardown(CoreExecutorTeardownReason::ArchivedSession, message)
    }

    pub fn unavailable_teardown(message: impl Into<String>) -> Self {
        Self::teardown(CoreExecutorTeardownReason::SessionUnavailable, message)
    }
}

/// Preserve whether a recovered-create failure owns a newly prepared runtime.
/// A new registration requires post-handoff teardown; a pre-existing runtime
/// keeps the ordinary retryable service error. Archived authority always wins.
pub fn classify_recovered_create_failure(
    error: SessionError,
    runtime_was_registered: bool,
) -> StagedApplyRuntimeTurnError {
    if is_archived_create_rejection(&error) {
        return StagedApplyRuntimeTurnError::archived_teardown(error.to_string());
    }
    if !runtime_was_registered {
        return StagedApplyRuntimeTurnError::unavailable_teardown(error.to_string());
    }
    error.into()
}

/// Comms-side context callback fired after the staged session
/// materializes successfully but before the apply task waits for the
/// turn result. Surfaces wire their `update_peer_ingress_context`
/// invocation here; surfaces compiled without `comms` pass a no-op.
pub type CommsContextRefreshFn = Arc<dyn Fn(SessionId) -> BoxFut<'static, ()> + Send + Sync>;

/// Spawn the staged-session promotion task that materializes the live
/// session via `create_session` and immediately drives a runtime
/// `apply_runtime_turn`. The returned receiver preserves ordinary service
/// failures separately from typed post-handoff teardown requests; surfaces
/// translate both onto their runtime-owned error seam.
///
/// `replay_promoted_system_context` runs after a successful turn to
/// replay any system-context state captured during staged promotion.
/// `comms_context_refresh` re-syncs the comms ingress context after a
/// successful create. `pre_turn_hook` is a test-only hook fired between
/// create-success and the apply-runtime-turn dispatch.
#[allow(clippy::too_many_arguments)]
pub fn spawn_pending_create_and_apply_runtime_turn_with_admission_guard(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    runtime_adapter: Arc<MeerkatMachine>,
    replay_promoted_system_context: ReplayPromotedSystemContextFn,
    comms_context_refresh: CommsContextRefreshFn,
    pre_turn_hook: Option<PreTurnHookFn>,
    session_id: SessionId,
    create_req: CreateSessionRequest,
    run_id: RunId,
    req: StartTurnRequest,
    boundary: RunApplyBoundary,
    contributing_input_ids: Vec<InputId>,
    mut promotion_cleanup: PendingPromotionCleanup,
    generated_machine_archived_resume_admission: GeneratedMachineArchivedResumeAdmission,
) -> ServiceApplyRuntimeTurnSpawnReceiver {
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let create_result = match (
            promotion_cleanup.take_staged_capacity_admission(),
            generated_machine_archived_resume_admission.is_authorized_by_generated_authority(),
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
                if is_archived_create_rejection(&err) {
                    let teardown = StagedApplyRuntimeTurnError::archived_teardown(err.to_string());
                    promotion_cleanup.mark_materialized();
                    let _ = promotion_cleanup.finish_now().await;
                    promotion_cleanup.disarm();
                    let _ = result_tx.send(Err(teardown));
                    return;
                }
                match pending_live_first_turn_is_still_deferred(&service, &session_id).await {
                    Ok(true) => {
                        promotion_cleanup.mark_materialized();
                        finish_pending_promotion_after_service_turn(
                            staged_sessions.as_ref(),
                            &session_id,
                            "create_session",
                            |starting, current| {
                                (replay_promoted_system_context)(
                                    session_id.clone(),
                                    starting,
                                    current,
                                )
                            },
                        )
                        .await;
                    }
                    Ok(false) => {
                        let materialized_after_error = match service
                            .has_live_session(&session_id)
                            .await
                        {
                            Ok(materialized_after_error) => materialized_after_error,
                            Err(error) => {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to determine live-session materialization after create_session error"
                                );
                                let primary_error = SessionError::Agent(
                                    meerkat_core::error::AgentError::InternalError(format!(
                                        "failed to read live-session materialization authority after create_session error for {session_id}: {error}"
                                    )),
                                );
                                promotion_cleanup
                                    .fail_closed_after_unknown_materialization()
                                    .await;
                                promotion_cleanup.disarm();
                                let _ = result_tx.send(Err(
                                    StagedApplyRuntimeTurnError::unavailable_teardown(
                                        primary_error.to_string(),
                                    ),
                                ));
                                return;
                            }
                        };
                        if materialized_after_error {
                            promotion_cleanup.mark_materialized();
                            finish_pending_promotion_after_service_turn(
                                staged_sessions.as_ref(),
                                &session_id,
                                "create_session",
                                |starting, current| {
                                    (replay_promoted_system_context)(
                                        session_id.clone(),
                                        starting,
                                        current,
                                    )
                                },
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
                    Err(error) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %error,
                            create_error = %err,
                            "failed to determine deferred-first-turn materialization after create_session error"
                        );
                        let primary_error = SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "failed to read deferred-first-turn authority after create_session error for {session_id}: {error}"
                            )),
                        );
                        promotion_cleanup
                            .fail_closed_after_unknown_materialization()
                            .await;
                        promotion_cleanup.disarm();
                        let _ =
                            result_tx.send(Err(StagedApplyRuntimeTurnError::unavailable_teardown(
                                primary_error.to_string(),
                            )));
                        return;
                    }
                }
                promotion_cleanup.disarm();
                let _ = result_tx.send(Err(err.into()));
                return;
            }
        }

        if let Some(hook) = pre_turn_hook.as_ref() {
            (hook)().await;
        }

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

        (comms_context_refresh)(session_id.clone()).await;

        let result = turn_result_rx.await.unwrap_or_else(|_| {
            Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "session service apply_runtime_turn task ended before reporting a result for {session_id}"
                )),
            ))
        });
        let should_restore =
            match should_restore_pending_after_apply_runtime_turn(&service, &session_id, &result)
                .await
            {
                Ok(should_restore) => should_restore,
                Err(error) => {
                    promotion_cleanup
                        .fail_closed_after_unknown_materialization()
                        .await;
                    promotion_cleanup.disarm();
                    let _ = result_tx.send(Err(StagedApplyRuntimeTurnError::unavailable_teardown(
                        error.to_string(),
                    )));
                    return;
                }
            };
        if should_restore {
            let restore_result = promotion_cleanup
                .restore_after_materialized_failure(
                    &service,
                    MachineSessionArchiveProtocol::from_machine(runtime_adapter.as_ref()),
                )
                .await;
            promotion_cleanup.disarm();
            let _ = result_tx.send(match restore_result {
                Ok(()) => match result {
                    Ok(output) => Ok(output),
                    Err(error) => Err(StagedApplyRuntimeTurnError::unavailable_teardown(
                        error.to_string(),
                    )),
                },
                Err(error) => Err(StagedApplyRuntimeTurnError::unavailable_teardown(
                    error.to_string(),
                )),
            });
            return;
        }

        finish_pending_promotion_after_service_turn(
            staged_sessions.as_ref(),
            &session_id,
            "apply_runtime_turn",
            |starting, current| {
                (replay_promoted_system_context)(session_id.clone(), starting, current)
            },
        )
        .await;
        promotion_cleanup.disarm();
        let _ = result_tx.send(result.map_err(StagedApplyRuntimeTurnError::from));
    });
    result_rx
}

/// Spawn a recovered-session promotion task that materializes the live
/// session via `create_session_with_reserved_admission` and immediately
/// drives a runtime `apply_runtime_turn`. Required cleanup is reported as a
/// typed teardown request; this worker never unregisters the executor it is
/// currently serving.
#[allow(clippy::too_many_arguments)]
pub fn spawn_recovered_create_and_apply_runtime_turn_with_admission_guard(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    comms_context_refresh: CommsContextRefreshFn,
    session_id: SessionId,
    create_req: CreateSessionRequest,
    runtime_was_registered: bool,
    run_id: RunId,
    req: StartTurnRequest,
    boundary: RunApplyBoundary,
    contributing_input_ids: Vec<InputId>,
    admission: ActiveCapacityGuard,
) -> ServiceApplyRuntimeTurnSpawnReceiver {
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

                (comms_context_refresh)(session_id.clone()).await;

                let result = turn_result_rx.await.unwrap_or_else(|_| {
                    Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "session service apply_runtime_turn task ended before reporting a result for {session_id}"
                        )),
                    ))
                });
                let should_restore = match should_restore_pending_after_apply_runtime_turn(
                    &service,
                    &session_id,
                    &result,
                )
                .await
                {
                    Ok(should_restore) => should_restore,
                    Err(error) => {
                        let _ = result_tx.send(Err(error.into()));
                        return;
                    }
                };
                if should_restore {
                    return match result {
                        Ok(output) => {
                            let _ = result_tx.send(Ok(output));
                        }
                        Err(error) => {
                            let _ = result_tx.send(Err(
                                StagedApplyRuntimeTurnError::unavailable_teardown(
                                    error.to_string(),
                                ),
                            ));
                        }
                    };
                }
                result.map_err(StagedApplyRuntimeTurnError::from)
            }
            Err(err) => Err(classify_recovered_create_failure(
                err,
                runtime_was_registered,
            )),
        };
        let _ = result_tx.send(result);
    });
    result_rx
}

/// Test-only spawn helper: stages the session via `create_session` then
/// drives a single live `start_turn`. Mirrors the production
/// `spawn_pending_create_and_apply_runtime_turn_with_admission_guard`
/// shape but exits after `start_turn` instead of `apply_runtime_turn`.
/// Kept un-gated so `meerkat-rpc` can call it from its own `cfg(test)`
/// builds even when the upstream `meerkat` crate is not built in test
/// mode.
#[allow(clippy::too_many_arguments)]
pub fn spawn_pending_create_and_start_turn_with_admission_guard(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    runtime_adapter: Arc<MeerkatMachine>,
    replay_promoted_system_context: ReplayPromotedSystemContextFn,
    pre_turn_hook: Option<PreTurnHookFn>,
    session_id: SessionId,
    create_req: CreateSessionRequest,
    start_req: StartTurnRequest,
    mut promotion_cleanup: PendingPromotionCleanup,
    generated_machine_archived_resume_admission: GeneratedMachineArchivedResumeAdmission,
) -> ServiceStartTurnSpawnReceiver {
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let create_result = match (
            promotion_cleanup.take_staged_capacity_admission(),
            generated_machine_archived_resume_admission.is_authorized_by_generated_authority(),
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
                if is_archived_create_rejection(&err) {
                    let _ = service.discard_live_session(&session_id).await;
                    let err = unregister_session_compensation(
                        runtime_adapter.as_ref(),
                        &session_id,
                        &err,
                    )
                    .await
                    .err()
                    .unwrap_or(err);
                    promotion_cleanup.mark_materialized();
                    let _ = promotion_cleanup.finish_now().await;
                    promotion_cleanup.disarm();
                    let _ = result_tx.send(Err(err));
                    return;
                }
                match pending_live_first_turn_is_still_deferred(&service, &session_id).await {
                    Ok(true) => {
                        promotion_cleanup.mark_materialized();
                        finish_pending_promotion_after_service_turn(
                            staged_sessions.as_ref(),
                            &session_id,
                            "create_session",
                            |starting, current| {
                                (replay_promoted_system_context)(
                                    session_id.clone(),
                                    starting,
                                    current,
                                )
                            },
                        )
                        .await;
                    }
                    Ok(false) => {
                        let materialized_after_error = match service
                            .has_live_session(&session_id)
                            .await
                        {
                            Ok(materialized_after_error) => materialized_after_error,
                            Err(error) => {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to determine live-session materialization after create_session error"
                                );
                                let _ = service.discard_live_session(&session_id).await;
                                let primary_error = SessionError::Agent(
                                    meerkat_core::error::AgentError::InternalError(format!(
                                        "failed to read live-session materialization authority after create_session error for {session_id}: {error}"
                                    )),
                                );
                                let final_error = unregister_session_compensation(
                                    runtime_adapter.as_ref(),
                                    &session_id,
                                    &primary_error,
                                )
                                .await
                                .err()
                                .unwrap_or(primary_error);
                                promotion_cleanup
                                    .fail_closed_after_unknown_materialization()
                                    .await;
                                promotion_cleanup.disarm();
                                let _ = result_tx.send(Err(final_error));
                                return;
                            }
                        };
                        if materialized_after_error {
                            promotion_cleanup.mark_materialized();
                            finish_pending_promotion_after_service_turn(
                                staged_sessions.as_ref(),
                                &session_id,
                                "create_session",
                                |starting, current| {
                                    (replay_promoted_system_context)(
                                        session_id.clone(),
                                        starting,
                                        current,
                                    )
                                },
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
                    Err(error) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %error,
                            create_error = %err,
                            "failed to determine deferred-first-turn materialization after create_session error"
                        );
                        let _ = service.discard_live_session(&session_id).await;
                        let primary_error = SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "failed to read deferred-first-turn authority after create_session error for {session_id}: {error}"
                            )),
                        );
                        let final_error = unregister_session_compensation(
                            runtime_adapter.as_ref(),
                            &session_id,
                            &primary_error,
                        )
                        .await
                        .err()
                        .unwrap_or(primary_error);
                        promotion_cleanup
                            .fail_closed_after_unknown_materialization()
                            .await;
                        promotion_cleanup.disarm();
                        let _ = result_tx.send(Err(final_error));
                        return;
                    }
                }
                promotion_cleanup.disarm();
                let _ = result_tx.send(Err(err));
                return;
            }
        }

        if let Some(hook) = pre_turn_hook.as_ref() {
            (hook)().await;
        }

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
        let should_restore =
            match should_restore_pending_after_start_turn(&service, &session_id, &result).await {
                Ok(should_restore) => should_restore,
                Err(error) => {
                    let _ = service.discard_live_session(&session_id).await;
                    let error = unregister_session_compensation(
                        runtime_adapter.as_ref(),
                        &session_id,
                        &error,
                    )
                    .await
                    .err()
                    .unwrap_or(error);
                    promotion_cleanup
                        .fail_closed_after_unknown_materialization()
                        .await;
                    promotion_cleanup.disarm();
                    let _ = result_tx.send(Err(error));
                    return;
                }
            };
        if should_restore {
            let restore_result = promotion_cleanup
                .restore_after_materialized_failure(
                    &service,
                    MachineSessionArchiveProtocol::from_machine(runtime_adapter.as_ref()),
                )
                .await;
            let mut unregister_result = Ok(());
            if restore_result.is_ok() {
                let primary_error = result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "staged materialized turn requires restore".to_string());
                unregister_result = unregister_session_compensation(
                    runtime_adapter.as_ref(),
                    &session_id,
                    &primary_error,
                )
                .await;
                #[cfg(feature = "comms")]
                if let Err(error) = runtime_adapter.abort_comms_drain(&session_id).await {
                    // Secondary fault on the restore path: the primary
                    // restore outcome owns the result channel, so this drain
                    // fault is surfaced as a typed warning instead of
                    // overwriting the primary outcome.
                    tracing::warn!(
                        %session_id,
                        %error,
                        "failed to abort comms drain after materialized-failure restore"
                    );
                }
            }
            promotion_cleanup.disarm();
            let _ = result_tx.send(match (restore_result, unregister_result) {
                (Ok(()), Ok(())) => result,
                (Ok(()), Err(error)) | (Err(error), _) => Err(error),
            });
            return;
        }

        finish_pending_promotion_after_service_turn(
            staged_sessions.as_ref(),
            &session_id,
            "start_turn",
            |starting, current| {
                (replay_promoted_system_context)(session_id.clone(), starting, current)
            },
        )
        .await;
        promotion_cleanup.disarm();
        let _ = result_tx.send(result);
    });
    result_rx
}

/// Cleanup mode for [`PendingPromotionCleanup`].
///
/// `Restore` is the default for an in-flight staged session whose
/// promotion has not yet been finalized: Drop restores the slot to the
/// staged registry. `Finish` is set after the promotion materializes
/// successfully; Drop only reaps the promoting system-context state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingPromotionCleanupMode {
    /// Restore the staged slot back to the registry on Drop.
    Restore,
    /// Reap promoting metadata on Drop, but do not restore.
    Finish,
}

/// RAII bookkeeping around a staged session that is in the middle of
/// being promoted into a live session.
///
/// While `armed` and `mode == Restore`, Drop spawns a tokio task that
/// re-stages the slot in [`StagedSessionRegistry`] and returns the
/// reserved capacity to [`StagedCapacityAdmissions`]. After
/// `mark_materialized` flips `mode` to `Finish`, Drop only reaps the
/// promoting system-context state.
pub struct PendingPromotionCleanup {
    pub(crate) staged_sessions: Arc<StagedSessionRegistry>,
    pub(crate) staged_capacity_admissions: StagedCapacityAdmissions,
    pub(crate) session_id: SessionId,
    pub(crate) staged_capacity_admission: Option<ActiveCapacityGuard>,
    pub(crate) build_config: Option<AgentBuildConfig>,
    pub(crate) labels: Option<BTreeMap<String, String>>,
    pub(crate) deferred_prompt: Option<ContentInput>,
    pub(crate) deferred_injected_context: Vec<ContentInput>,
    pub(crate) created_at_secs: u64,
    pub(crate) updated_at_secs: u64,
    pub(crate) mode: PendingPromotionCleanupMode,
    pub(crate) armed: bool,
}

impl PendingPromotionCleanup {
    /// Build a fresh cleanup guard from a [`PromotingSlot`] snapshot.
    pub fn new(
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
            labels: slot.labels.clone(),
            deferred_prompt: slot.deferred_prompt.clone(),
            deferred_injected_context: slot.deferred_injected_context.clone(),
            created_at_secs: slot.created_at_secs,
            updated_at_secs: slot.updated_at_secs,
            mode: PendingPromotionCleanupMode::Restore,
            armed: true,
        }
    }

    /// Replace the staged build config that would be re-staged on
    /// rollback. No-op once disarmed.
    pub fn update_build_config(&mut self, build_config: &AgentBuildConfig) {
        if self.armed {
            self.build_config = Some(build_config.clone());
        }
    }

    /// Flip into `Finish` mode after the promotion materialized
    /// successfully. The Drop will no longer attempt to re-stage.
    pub fn mark_materialized(&mut self) {
        if self.armed {
            self.mode = PendingPromotionCleanupMode::Finish;
            self.staged_capacity_admission = None;
        }
    }

    /// Detach the staged-capacity admission for the caller to consume.
    pub fn take_staged_capacity_admission(&mut self) -> Option<ActiveCapacityGuard> {
        self.staged_capacity_admission.take()
    }

    /// Reserve a fresh staged-capacity admission if the guard does not
    /// already hold one.
    pub async fn replenish_staged_capacity_admission(
        &mut self,
        service: &PersistentSessionService<FactoryAgentBuilder>,
    ) -> Result<(), SessionError> {
        if self.staged_capacity_admission.is_none() {
            self.staged_capacity_admission =
                Some(service.reserve_create_session_admission().await?);
        }
        Ok(())
    }

    /// Reserve a runtime-turn admission for the materialized session.
    pub async fn recover_materialized_staged_capacity_admission(
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

    /// Abort restore when no admission is available; reaps promoting
    /// metadata and disarms.
    pub async fn abort_restore_without_capacity(&mut self) {
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

    /// Fail closed when materialization state cannot be read: remove the
    /// promoting staged slot instead of restoring it as if no live session
    /// exists.
    pub async fn fail_closed_after_unknown_materialization(&mut self) {
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

    /// Restore the staged session after a materialized-side failure
    /// (e.g. a pre-run apply failure on the live session).
    pub async fn restore_after_materialized_failure(
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
            self.restore_now().await;
            return Err(error);
        }
        self.finish_after_machine_archive().await;
        Ok(())
    }

    /// Reap promoting state and disarm after a successful machine
    /// archive.
    pub async fn finish_after_machine_archive(&mut self) {
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

    /// Mark, through generated staged-session authority, that the session may
    /// resume once from machine-archived state on its next promotion attempt.
    pub async fn authorize_machine_archived_resume(&mut self) -> Result<(), SessionError> {
        if !self.armed {
            return Ok(());
        }
        match self
            .staged_sessions
            .authorize_machine_archived_resume(&self.session_id)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "staged session {} disappeared during machine-archived resume authorization",
                    self.session_id
                )),
            )),
            Err(err) => Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(err.to_string()),
            )),
        }
    }

    /// Copy the current promoting system-context state into
    /// `build_config` so a re-stage preserves it.
    pub async fn preserve_promoting_system_context_state(
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

    /// Synchronously restore the staged session and return its
    /// admission to the staged-capacity ledger.
    pub async fn restore_now(&mut self) {
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
                self.labels.clone(),
                self.deferred_prompt.clone(),
                self.deferred_injected_context.clone(),
                self.created_at_secs,
                self.updated_at_secs,
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

    /// Reap promoting system-context state synchronously when in
    /// `Finish` mode; returns the (starting, current) pair if any.
    pub async fn finish_now(
        &mut self,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        if !self.armed || self.mode != PendingPromotionCleanupMode::Finish {
            return None;
        }
        self.staged_sessions
            .take_promoting_system_context_state(&self.session_id)
            .await
    }

    /// Suppress all Drop-time cleanup. Used by callers that have
    /// committed the promotion via a different path.
    pub fn disarm(&mut self) {
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
                let labels = self.labels.clone();
                let deferred_prompt = self.deferred_prompt.clone();
                let deferred_injected_context = self.deferred_injected_context.clone();
                let created_at_secs = self.created_at_secs;
                let updated_at_secs = self.updated_at_secs;
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
                            labels,
                            deferred_prompt,
                            deferred_injected_context,
                            created_at_secs,
                            updated_at_secs,
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

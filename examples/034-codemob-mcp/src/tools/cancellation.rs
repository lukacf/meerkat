use std::sync::Arc;

use meerkat::surface::{request_action, CancelActionInstallOutcome, RequestContext};
use meerkat::SessionService;
use meerkat_core::service::{CreateSessionRequest, StartTurnRequest};
use meerkat_core::{RunResult, SessionId};
use meerkat_session::{EphemeralSessionService, LiveSessionActorWitnessSlot, SessionAgentBuilder};
use tokio::sync::{oneshot, Notify};

use super::ToolCallError;

/// A wakeup from the shared request authority, not a second cancellation state.
pub async fn cancellation_signal(
    context: Option<&RequestContext>,
) -> Result<Arc<Notify>, ToolCallError> {
    let signal = Arc::new(Notify::new());
    if let Some(context) = context {
        let wake = signal.clone();
        if context
            .install_cancel_action_or_cancelled(request_action(move || {
                let wake = wake.clone();
                async move {
                    wake.notify_one();
                }
            }))
            .await
            == CancelActionInstallOutcome::AlreadyCancelled
        {
            return Err(ToolCallError::cancelled());
        }
    }
    Ok(signal)
}

pub async fn create_and_run<B: SessionAgentBuilder + 'static>(
    service: &Arc<EphemeralSessionService<B>>,
    create: CreateSessionRequest,
    turn: StartTurnRequest,
    context: Option<&RequestContext>,
) -> Result<RunResult, ToolCallError> {
    let signal = cancellation_signal(context).await?;
    let slot = LiveSessionActorWitnessSlot::default();
    if let Some(context) = context {
        let service = service.clone();
        let slot = slot.clone();
        context.set_unpublished_cleanup(request_action(move || {
            let service = service.clone();
            let slot = slot.clone();
            async move {
                if let Some(witness) = slot.witness() {
                    let _ = service.discard_live_session_actor(&witness).await;
                }
            }
        }));
    }

    let result = async {
        let (_, witness) = tokio::select! {
            biased;
            _ = signal.notified() => return Err(ToolCallError::cancelled()),
            result = service.create_session_with_admission_and_witness(create, None, Some(&slot)) =>
                result.map_err(|error| ToolCallError::internal(format!("Session error: {error}")))?,
        };
        run_turn(service, witness.session_id(), turn, &signal).await
    }
    .await;
    if result.is_err() {
        if let Some(witness) = slot.witness() {
            service
                .discard_live_session_actor(&witness)
                .await
                .map_err(|error| {
                    ToolCallError::internal(format!("Session cleanup failed: {error}"))
                })?;
        }
    }
    result
}

pub async fn run_turn<B: SessionAgentBuilder + 'static>(
    service: &EphemeralSessionService<B>,
    session_id: &SessionId,
    turn: StartTurnRequest,
    signal: &Notify,
) -> Result<RunResult, ToolCallError> {
    let (admitted_tx, mut admitted_rx) = oneshot::channel();
    let run = service.start_turn_with_admission_notification(session_id, turn, Some(admitted_tx));
    tokio::pin!(run);

    let result = tokio::select! {
        biased;
        _ = signal.notified() => {
            // The future may already have handed the turn to the actor during
            // an earlier poll. Never drop its finalization boundary after that.
            if admitted_rx.try_recv().is_ok() {
                let _ = service.interrupt(session_id).await;
                let _ = run.await;
            }
            return Err(ToolCallError::cancelled());
        }
        admission = &mut admitted_rx => {
            if admission.is_err() {
                run.await
            } else {
                tokio::select! {
                    biased;
                    _ = signal.notified() => {
                        let _ = service.interrupt(session_id).await;
                        let _ = run.await;
                        return Err(ToolCallError::cancelled());
                    }
                    result = &mut run => result,
                }
            }
        }
        result = &mut run => result,
    };
    result.map_err(|error| ToolCallError::internal(format!("Session error: {error}")))
}

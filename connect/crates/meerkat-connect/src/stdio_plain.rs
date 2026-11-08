//! Private presentation and retained close for the existing public-service owner.
//! The native review, consuming quiescent close and finalizer keep their meaning.
use super::*;
use crate::{
    session::{ClosedSetup, FinalizationClose},
    terminal_connector::{FinalizedAction, TerminalConnectors},
};

pub(super) async fn review(
    mut owner: TerminalConnectors,
    approvals: Arc<LocalApprovals>,
    bridge: PromptBridge,
    admission: Option<crate::terminal_connector::private_service::Admission>,
) -> Work {
    let outcome = loop {
        let request = tokio::select! { biased;
            _ = bridge.closed() => break Ok(None),
            request = owner.next() => match request { Some(value) => value, None => break Err(ended()) },
        };
        let result = if let Some(admission) = &admission {
            owner
                .present_private_services(request, &approvals, &bridge, admission.clone())
                .await
        } else {
            owner
                .present_private_plain(request, &approvals, async {
                    bridge.closed().await;
                    Ok(())
                })
                .await
        };
        match result {
            Ok(Some(handoff)) => break Ok(Some(handoff)),
            Ok(None) => {}
            Err(error) => break Err(error),
        }
    };
    Work::PlainReviewed(owner, outcome)
}

/// Keep the consuming native close and any entered administrative effect owned.
/// Only explicit Closed plus successful cleanup reaches the existing finalizer.
pub(super) async fn finalize(
    closed: ClosedSetup,
    mut owner: TerminalConnectors,
    mut cancelled: watch::Receiver<bool>,
) -> Result<Option<FinalizedAction>, ConnectError> {
    if let Err((closed, error)) = closed.finish_host_cleanup().await {
        return Err(closed.retain_error(error));
    }
    let reviewed = owner.take_reviewed_action().ok_or_else(ended)?;
    let interrupt = async move {
        loop {
            if *cancelled.borrow_and_update() {
                return Ok(());
            }
            if cancelled.changed().await.is_err() {
                return Ok(());
            }
        }
    };
    tokio::pin!(interrupt);
    reviewed
        .finalize_outcome_unless_cancelled(interrupt.as_mut())
        .await
}

impl Driver {
    pub(super) fn plain_reviewed(
        &mut self,
        mut owner: TerminalConnectors,
        result: Result<Option<crate::terminal_connector::plain::PlainHandoff>, ConnectError>,
    ) {
        self.workers -= 1;
        if self.retired && owner.retire().is_err() {
            self.failed = true;
        }
        self.plain_owner = Some(owner);
        match result {
            Ok(Some(handoff)) if !self.retired => {
                // This occurs before the tool waiter can return and before
                // native completion can allow another submitted input.
                self.plain_pending = true;
                handoff.complete();
                self.state_event();
            }
            Ok(_) => {}
            Err(error) => {
                self.close_failure = Some(error);
                self.failed = true;
                self.fence("failed");
                self.closing = true;
            }
        }
    }

    pub(super) fn plain_native_closed(&mut self, result: FinalizationClose) {
        self.workers -= 1;
        match result {
            FinalizationClose::Closed(closed) => {
                self.plain_closed = Some(closed);
                // Close only presentation/admission now. This internal fence
                // is not an external stop of the separately retained finalizer.
                self.fence("finalizing");
                self.event(json!({"kind":"service_finalization","state":"native_closed","grant_issued":false}));
            }
            FinalizationClose::Busy(session) => {
                self.session = Some(Arc::new(session));
                self.event(json!({"kind":"service_finalization","state":"close_busy","grant_issued":false}));
            }
            FinalizationClose::Unconfirmed {
                session,
                observation,
            } => {
                self.session = Some(Arc::new(session));
                self.plain_close_observations.push(observation);
                self.event(json!({"kind":"service_finalization","state":"close_unconfirmed","grant_issued":false}));
            }
        }
        self.state_event();
    }

    pub(super) fn plain_finalized(
        &mut self,
        result: Result<Option<FinalizedAction>, ConnectError>,
    ) {
        self.workers -= 1;
        match result {
            Ok(Some(outcome)) => {
                if FinalizedAction::finish_invocation(std::slice::from_ref(&outcome), Ok(())).is_err() {
                    self.failed = true;
                }
                if let Some(observation) = outcome.private_observation() {
                    self.event(json!({"kind":"service_finalization","state":"observed","observation":observation}));
                } else { self.failed = true; }
                self.plain_outcomes.push(outcome);
            },
            Ok(None) => self.event(json!({"kind":"service_finalization","state":"stopped_before_administration","grant_issued":false})),
            Err(error) => {
                self.failed = true;
                self.close_failure = Some(error);
                self.event(json!({"kind":"service_finalization","state":"cleanup_unconfirmed","grant_issued":false}));
            },
        }
    }

    /// Return true while this selected close/finalizer owns settlement. Never
    /// poll Arc counts or cancel accepted work to manufacture native quiescence.
    pub(super) fn settle_plain(&mut self) -> bool {
        if self.plain_pending
            && !self.plain_close_started
            && !self.retired
            && self.active.is_none()
            && self.histories == 0
            && !self.connector_busy
        {
            // Sent and History are the actual workers that retain Session Arcs.
            // The presenter/resource workers hold their own owners, not Session.
            let Some(session) = self.session.take() else {
                return false;
            };
            let session = match Arc::try_unwrap(session) {
                Ok(value) => value,
                Err(retained) => {
                    self.session = Some(retained);
                    self.failed = true;
                    self.fence("failed");
                    self.closing = true;
                    return false;
                }
            };
            self.plain_close_started = true;
            self.workers += 1;
            self.tasks.push(
                async move { Work::PlainNativeClosed(session.try_close_for_finalization().await) }
                    .boxed(),
            );
            return true;
        }
        if self.plain_closed.is_some()
            && self.workers == 0
            && self.histories == 0
            && self.active.is_none()
        {
            let closed = self.plain_closed.take().expect("retained native Closed");
            let Some(owner) = self.plain_owner.take() else {
                self.failed = true;
                self.close_failure = Some(closed.retain_error(ended()));
                self.private_close_started = true;
                return false;
            };
            // This task performs the same host cleanup as the terminal before
            // granting. The generic retirement path must not close it first.
            self.private_close_started = true;
            self.workers += 1;
            if self.failed {
                self.plain_cancel.send_replace(true);
            }
            let cancelled = self.plain_cancel.subscribe();
            if let Some(captured) = self.probe_capture.take() {
                // The native Closed owner still retains its real physical
                // cleanup. Idle transport aliases must not keep the old auth
                // facade/router custody alive across fresh administration.
                self.connector.take();
                self.lazy.take();
                let Some(selection) = self.selected_services() else {
                    self.failed = true;
                    self.close_failure = Some(closed.retain_error(ended()));
                    self.workers -= 1;
                    return false;
                };
                self.probe_commissioning = true;
                let bridge = PromptBridge {
                    prompts: self.bridge.prompts.clone(),
                    retired: self.plain_cancel.subscribe(),
                    probe_lifetime: Some((
                        selection.host().lifetime.clone(),
                        selection.host().binding.expires_at_ms,
                    )),
                    commissioning: true,
                };
                let admission = crate::terminal_connector::private_service::Admission::new(
                    selection.host().binding.expires_at_ms,
                    self.plain_cancel.subscribe(),
                )
                .with_host(Some(selection.host()));
                self.state_event();
                self.tasks.push(
                    async move {
                        Work::ProbeFinalized(
                            super::probe::finalize(
                                captured, closed, owner, cancelled, selection, bridge, admission,
                            )
                            .await,
                        )
                    }
                    .boxed(),
                );
            } else {
                self.tasks.push(
                    async move { Work::PlainFinalized(finalize(closed, owner, cancelled).await) }
                        .boxed(),
                );
            }
            return true;
        }
        self.plain_closed.is_some()
            || self.plain_close_started && self.session.is_none() && self.workers != 0
    }
}

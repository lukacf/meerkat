//! Console delivery for the same installed account request and native owners.
//! This module supplies no HTTP authentication or principal conversion.

use super::*;
use crate::{
    approval::ApprovalPresenter,
    provider_setup::SignInDisposition,
    stdio::{PromptAnswer, PromptBridge},
};
use futures_util::future::BoxFuture;
use std::{sync::atomic::{AtomicBool, Ordering}, future::Future, pin::Pin, time::Duration};

mod callback;
pub(crate) use callback::{CallbackDelivery, CallbackResult, answer_retained, answer_retained_authorized,
    cancel_retained_sign_in_authorized};

/// The installed HTTP owner supplies a closure over its retained verified
/// human, login and request. Each call must resolve fresh native access and
/// exact source disclosure. It is transport eligibility, never a native grant.
/// The J1 binding is intentionally not constructed from a principal string.
pub(crate) type Refresh<'a> = dyn Fn() -> BoxFuture<'static, Result<(), ConnectError>> + Send + Sync + 'a;
/// Captured once from the exact accepted answer, never a mutable latest viewer.
/// The synchronous check re-resolves that request's native Human and access at
/// the existing parent, child and source publication entry checks.
pub(crate) type DecisionCheck = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) type CaptureDecision<'a> = dyn Fn() -> Result<DecisionCheck, ConnectError> + Send + Sync + 'a;

impl InstalledAccountRequests {
    /// Drive an actual account request without terminal input. The caller must
    /// retain and poll this future through retirement, including native cleanup.
    /// `bridge` and `callbacks` belong to this request's same protected viewer.
    /// A future J1-qualified route calls here after selecting that exact owner;
    /// RemoteControllerLogin scope/expiry alone cannot authorize this entry.
    ///
    /// `redirect` is the configured Console callback URL, not a browser Host
    /// header. Loopback via the selected SSH/IAP tunnel remains supported.
    pub(crate) async fn present_console<F: Future<Output = ()>>(
        &mut self,
        request: InstalledAccountRequest,
        presenter: &mut Option<ApprovalPresenter>,
        bridge: PromptBridge,
        callbacks: mpsc::Sender<CallbackDelivery>,
        redirect: String,
        refresh: &Refresh<'_>,
        decision: &CaptureDecision<'_>,
        closed: F,
    ) -> Result<bool, ConnectError> {
        let owner_closed = AtomicBool::new(false);
        let owner_close = async {
            closed.await;
            owner_closed.store(true, Ordering::SeqCst);
        };
        let result = async {
            // This task comes from the actual borrowed native invocation and
            // its exact parent A. A browser label cannot replace its login.
            let login = request.task.as_ref().and_then(|task| task.console_login()).cloned()
                .ok_or_else(|| refused("The account request has no retained Console login"))?;
            let reservation = login.reserve().map_err(|_| refused("The Console login refused account entry"))?;
            let checked_refresh = || -> BoxFuture<'static, Result<(), ConnectError>> {
                let login = login.clone();
                let current = login.check().map_err(|_| refused("The Console login is unavailable"));
                let refreshed = current.is_ok().then(|| refresh());
                Box::pin(async move {
                    current?;
                    refreshed.expect("a current login selected refresh").await?;
                    login.check().map_err(|_| refused("The Console login is unavailable"))?;
                    Ok(())
                })
            };
            let closed = async {
                tokio::pin!(owner_close);
                tokio::select! { biased;
                    _ = owner_close.as_mut() => {},
                    _ = login.closed() => {},
                }
            };
            let result = self
                .present_console_inner(
                    request, presenter, bridge, callbacks, redirect, &checked_refresh, decision, &login, closed,
                    &owner_closed,
                )
                .await;
            if result.is_ok() { reservation.settled(); }
            result
        }.await;
        if owner_closed.load(Ordering::SeqCst) || presenter.is_none() {
            let retired = self.review.retire();
            drop(presenter.take());
            return match result {
                Ok(_) => retired.map(|()| false),
                Err(error) => present::join_cleanup(Err(error), retired).map(|()| false),
            };
        }
        if result.is_err() {
            if let Err(cleanup) = self.review.control.retire().map_err(refused) {
                return Err(ConnectError::Cleanup {
                    primary: Box::new(result.unwrap_err()),
                    cleanup: Box::new(cleanup),
                });
            }
        }
        // Login loss, caller loss and request failure never close the host.
        result.map(|_| true)
    }

    async fn present_console_inner<F: Future<Output = ()>>(
        &mut self,
        mut request: InstalledAccountRequest,
        presenter: &mut Option<ApprovalPresenter>,
        bridge: PromptBridge,
        callbacks: mpsc::Sender<CallbackDelivery>,
        redirect: String,
        refresh: &Refresh<'_>,
        decision: &CaptureDecision<'_>,
        console_login: &toolkit_mobkit::private_host::ConsoleLoginHandle,
        closed: F,
        owner_closed: &AtomicBool,
    ) -> Result<bool, ConnectError> {
        if request.response.is_closed() {
            return Ok(true);
        }
        let approvals = self.review.approvals.clone();
        let (login, software) = self
            .review
            .selected_login(&request.job_id, request.selection.operations.clone())?;
        login.check_target(&request.target).map_err(refused)?;
        if software != request.software {
            return Err(refused("The selected installation changed"));
        }
        tokio::pin!(closed);
        let mut entry = Some(request.entry);
        let mut custody = None;
        if !checkpoint(
            &request.current,
            &mut request.response,
            &bridge,
            refresh,
            closed.as_mut(),
        )
        .await?
        {
            self.review.control.retire().map_err(refused)?;
            return Ok(request.response.is_closed());
        }
        let (observed, stopped) = join_on_close(
            self.review.observe_account(&login, &request.target),
            &mut request.response,
            &bridge,
            closed.as_mut(),
        )
        .await;
        if stopped {
            self.review.control.retire().map_err(refused)?;
            return Ok(request.response.is_closed());
        }
        if !checkpoint(
            &request.current,
            &mut request.response,
            &bridge,
            refresh,
            closed.as_mut(),
        )
        .await?
        {
            self.review.control.retire().map_err(refused)?;
            return Ok(request.response.is_closed());
        }
        let mut login_completed = false;
        if observed.expect("an unstopped observation was polled")?.phase
            != meerkat::ConnectorAuthPhase::Authorized
        {
            approvals.check_session(&self.review.session)?;
            login.check_current().map_err(refused)?;
            let (started, stopped) = join_on_close(
                async {
                    // Entry and the first account effect follow the same
                    // winning cancellation check, without an intervening wait.
                    custody = Some(
                        entry
                            .take()
                            .expect("one retained request entry")
                            .enter()
                            .map_err(|_| refused("The current account request cannot enter"))?,
                    );
                    self.review.start_account(
                        &request.job_id,
                        request.target.clone(),
                        request.selection.operations.clone(),
                        redirect,
                    ).await
                },
                &mut request.response,
                &bridge,
                closed.as_mut(),
            )
            .await;
            if stopped {
                self.review.control.retire().map_err(refused)?;
                return Ok(request.response.is_closed());
            }
            let started = started.expect("an unstopped start was polled")?;
            if !checkpoint(
                &request.current,
                &mut request.response,
                &bridge,
                refresh,
                closed.as_mut(),
            )
            .await?
            {
                self.review.control.retire().map_err(refused)?;
                return Ok(request.response.is_closed());
            }
            // Only native Control owns the attempt/target/redirect and handle.
            // This one-shot transports its callback; it cannot select an owner.
            let (delivery, receive) = CallbackDelivery::channel(&started.state);
            if callbacks.try_send(delivery).is_err() {
                self.review.control.retire().map_err(refused)?;
                return Err(refused("The protected callback receiver is unavailable"));
            }
            let mut notice = bridge
                .provider_notice(json!({"kind":"provider_browser",
                "authorization_url":started.authorize_url}))
                .await?;
            let (callback, stopped) = tokio::select! { biased;
                _ = stop(&mut request.response, &bridge, closed.as_mut()) => (None, true),
                _ = notice.cancelled() => (None, false),
                _ = tokio::time::sleep(Duration::from_secs(120)) => (None, false),
                callback = receive => (callback.ok(), false),
            };
            let Some(callback) = callback else {
                // Retire only this native attempt on ordinary dismissal/timeout.
                // The same helper can request another account later.
                let retired = self.review.control.retire().map_err(refused);
                let finished = notice.finish(SignInDisposition::Refused).await;
                present::join_cleanup(retired, finished)?;
                if !stopped && !bridge.retired() && !request.response.is_closed() {
                    let _ = request.response.send(AccountReply::new(
                        AccountState::Unavailable,
                        false,
                        false,
                    ));
                }
                return Ok(true);
            };
            let current = checkpoint(
                &request.current,
                &mut request.response,
                &bridge,
                refresh,
                closed.as_mut(),
            ).await;
            if !matches!(current, Ok(true)) {
                let retired = self.review.control.retire().map_err(refused);
                if current.is_ok() {
                    let _ = callback.response.send(CallbackResult::Refused);
                } else { drop(callback.response); }
                let finished = notice.finish(SignInDisposition::Refused).await;
                present::join_cleanup(current.map(|_| ()),
                    present::join_cleanup(retired, finished))?;
                return Ok(request.response.is_closed());
            }
            let Some(code) = callback.code else {
                // Provider denial never answers the separate service review.
                let retired = self.review.control.retire().map_err(refused);
                let _ = callback.response.send(CallbackResult::Refused);
                let finished = notice.finish(SignInDisposition::Refused).await;
                present::join_cleanup(retired, finished)?;
                let _ = request.response.send(AccountReply::new(
                    AccountState::Unavailable,
                    false,
                    false,
                ));
                return Ok(true);
            };
            let completion = complete_joined(
                &mut self.review,
                callback.state,
                code,
                started.host_attempt_handle,
                sign_in_closed(&mut notice, async {
                    stop(&mut request.response, &bridge, closed.as_mut()).await;
                }),
            ).await;
            let (completed, stopped) = match completion {
                Ok(value) => value,
                Err(error) => {
                    // Even an entry refusal must retire the exact started
                    // attempt and join its informational prompt before return.
                    let retired = self.review.control.retire().map_err(refused);
                    drop(callback.response);
                    let finished = notice.finish(SignInDisposition::Refused).await;
                    return present::join_cleanup(Err(error),
                        present::join_cleanup(retired, finished)).map(|()| true);
                }
            };
            let Some(completed) = completed else {
                // The callback was consumed, but native completion never ran.
                // Explicit Cancel is local to this request, not owner closure.
                let retired = self.review.control.retire().map_err(refused);
                let current = if notice.cancel_requested() {
                    checkpoint(&request.current, &mut request.response, &bridge,
                        refresh, closed.as_mut()).await
                } else { Ok(false) };
                let reply = matches!(current, Ok(true));
                if reply {
                    let _ = callback.response.send(if retired.is_ok() {
                        CallbackResult::Refused
                    } else { CallbackResult::CleanupUnconfirmed });
                } else { drop(callback.response); }
                let finished = notice.finish(SignInDisposition::Refused).await;
                present::join_cleanup(current.map(|_| ()),
                    present::join_cleanup(retired, finished))?;
                if reply {
                    let _ = request.response.send(AccountReply::new(
                        AccountState::Unavailable, false, false));
                }
                return Ok(true);
            };
            login_completed = completed.result.is_ok();
            // This is the joined native result, not an instruction or grant.
            let disposition = if completed.cleanup_unconfirmed {
                CallbackResult::CleanupUnconfirmed
            } else if login_completed {
                CallbackResult::Completed
            } else {
                CallbackResult::Refused
            };
            // No callback response or private review is released under stale
            // transport access. Known native completion remains in Control.
            let current = if stopped && !notice.cancel_requested() {
                Ok(false)
            } else {
                checkpoint(
                    &request.current,
                    &mut request.response,
                    &bridge,
                    refresh,
                    closed.as_mut(),
                )
                .await
            };
            if !matches!(current, Ok(true)) {
                let retired = self.review.control.retire().map_err(refused);
                drop(callback.response);
                let finished = notice.finish(SignInDisposition::Refused).await;
                present::join_cleanup(current.map(|_| ()), present::join_cleanup(retired, finished))?;
                return Ok(request.response.is_closed());
            }
            let _ = callback.response.send(disposition);
            let cancel_requested = notice
                .finish_with_cancellation(if login_completed {
                    SignInDisposition::Completed
                } else {
                    SignInDisposition::Refused
                })
                .await?;
            if completed.cleanup_unconfirmed {
                return Err(refused("Native account cleanup is unconfirmed"));
            }
            if cancel_requested || !login_completed {
                // Retirement leaves a completed account intact. A successful
                // login is known, but this request has no access review/grant.
                self.review.control.retire().map_err(refused)?;
                let _ = request.response.send(AccountReply::new(
                    AccountState::Unavailable,
                    login_completed,
                    false,
                ));
                return Ok(true);
            }
        }
        if !checkpoint(
            &request.current,
            &mut request.response,
            &bridge,
            refresh,
            closed.as_mut(),
        )
        .await?
        {
            self.review.control.retire().map_err(refused)?;
            return Ok(request.response.is_closed());
        }
        if let Some(entry) = entry.take() {
            custody = Some(
                entry
                    .enter()
                    .map_err(|_| refused("The current account request cannot enter"))?,
            );
        }
        let lifetime = request.selection.expires_in_ms;
        let task_tool_policy = request.selection.tool_policy.clone();
        let (cancel_review, cancellation) = ReviewCancellation::channel();
        let reviewed = drive_review_cancellable(
            self.review.review_account_with_task_refreshed(
                &request.job_id,
                request.target,
                request.selection.operations.clone(),
                InstalledAccessProposal {
                    requester: ApprovalPrincipalId::new("connect-installed-setup-agent")
                        .map_err(|error| refused(error.to_string()))?,
                    summary: "Review access for the verified installed account".into(),
                    selection: request.selection,
                    decision_expires_at: SystemTime::now() + Duration::from_secs(120),
                },
                request.task.take(),
                Some(&request.current),
                lifetime,
                task_tool_policy,
                Some(refresh),
                Some(decision),
                Some(cancellation),
            ),
            &approvals,
            presenter,
            &bridge,
            refresh,
            &mut request.response,
            closed.as_mut(),
            Some(console_login),
            Some(cancel_review),
            Some(owner_closed),
        )
        .await?;
        let Some(reviewed) = reviewed else {
            return Ok(request.response.is_closed());
        };
        if !checkpoint(
            &request.current,
            &mut request.response,
            &bridge,
            refresh,
            closed.as_mut(),
        )
        .await?
        {
            self.review.control.retire().map_err(refused)?;
            return Ok(request.response.is_closed());
        }
        let reply = match reviewed {
            InstalledAccountReview::Approved(reviewed) => {
                let retained = reviewed.fresh_instruction_retained();
                self.reviewed.push((request.original, reviewed));
                let mut reply = AccountReply::new(AccountState::Reviewed, login_completed, true);
                reply.grant_issued = retained;
                reply.fresh_instruction_retained = retained;
                reply
            }
            InstalledAccountReview::Denied(_) => {
                AccountReply::new(AccountState::Denied, login_completed, true)
            }
        };
        let _ = request.response.send(reply);
        drop(custody);
        Ok(true)
    }
}

#[derive(Clone, Copy)]
enum Stop { Request, Presentation }
impl Stop {
    fn owner_closed(self, owner_closed: Option<&AtomicBool>) -> bool {
        matches!(self, Self::Presentation) && owner_closed.is_none_or(|closed| closed.load(Ordering::SeqCst))
    }
}

async fn stop<F: Future<Output = ()>, R>(
    response: &mut oneshot::Sender<R>,
    bridge: &PromptBridge,
    mut closed: Pin<&mut F>,
) -> Stop {
    tokio::select! { biased;
        _ = closed.as_mut() => Stop::Presentation,
        _ = bridge.closed() => Stop::Request,
        _ = response.closed() => Stop::Request,
    }
}

pub(super) async fn checkpoint<F: Future<Output = ()>, R>(
    current: &CurrentRequest,
    response: &mut oneshot::Sender<R>,
    bridge: &PromptBridge,
    refresh: &Refresh<'_>,
    mut closed: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    tokio::select! { biased;
        _ = stop(response, bridge, closed.as_mut()) => Ok(false),
        result = async {
            refresh().await?;
            let current = current.check().await;
            refresh().await?;
            Ok(current)
        } => result,
    }
}

pub(crate) async fn join_on_close<T, F: Future<Output = T>, C: Future<Output = ()>>(
    future: F,
    response: &mut oneshot::Sender<AccountReply>,
    bridge: &PromptBridge,
    mut closed: Pin<&mut C>,
) -> (Option<T>, bool) {
    let closing = stop(response, bridge, closed.as_mut());
    tokio::pin!(future, closing);
    let mut polled = false;
    let mut stopped = false;
    futures_util::future::poll_fn(|cx| {
        if !stopped && closing.as_mut().poll(cx).is_ready() {
            stopped = true;
            if !polled {
                // A future that never ran owns no entered account operation.
                return std::task::Poll::Ready((None, true));
            }
        }
        // Once polled, the actual future may hold native custody. Retain and
        // join it on close; this bit is poll bookkeeping, not authority.
        polled = true;
        future.as_mut().poll(cx).map(|result| (Some(result), stopped))
    }).await
}

// Keep the private account reply channel inside its owning module while the
// external native fixture exercises the real close-before-first-poll path.
#[cfg(test)]
pub(crate) async fn fixture_join_on_close<T, F: Future<Output = T>, C: Future<Output = ()>>(
    future: F,
    bridge: &PromptBridge,
    closed: Pin<&mut C>,
) -> (Option<T>, bool) {
    let (mut response, _receive) = oneshot::channel();
    join_on_close(future, &mut response, bridge, closed).await
}

// Explicit cancellation uses the same retained provider notice both before
// callback delivery and while native completion is entered. It never closes
// the Console login, the host, a shared account or the original business task.
async fn sign_in_closed<C: Future<Output = ()>>(
    notice: &mut crate::stdio::ProviderNotice, closed: C,
) {
    tokio::select! { biased; _ = closed => {}, _ = notice.cancelled() => {} }
}

// Kept separate to exercise the actual Control cancellation/join in the
// existing connector fixture. It never drops an entered completion future.
pub(crate) async fn complete_joined<C: Future<Output = ()>>(
    review: &mut InstalledServiceReview,
    state: String,
    code: String,
    handle: Option<String>,
    closed: C,
) -> Result<(Option<ProtectedAccountCompletion>, bool), ConnectError> {
    let (_live, mut retired) = tokio::sync::watch::channel(false);
    let (cancel, receive) = oneshot::channel();
    let mut cancel = Some(cancel);
    let completing = review.complete_account(state, code, handle, &mut retired, receive);
    tokio::pin!(completing, closed);
    let mut polled = false;
    let mut stopped = false;
    futures_util::future::poll_fn(|cx| {
        if !stopped && closed.as_mut().poll(cx).is_ready() {
            stopped = true;
            if !polled {
                return std::task::Poll::Ready(Ok((None, true)));
            }
            let _ = cancel.take().expect("one cancellation sender").send(());
        }
        polled = true;
        completing.as_mut().poll(cx).map(|result| result.map(|result| (Some(result), stopped)))
    }).await
}

#[cfg(test)]
async fn drive_review<T, F, C>(
    review: F,
    approvals: &LocalApprovals,
    presenter: &mut Option<ApprovalPresenter>,
    bridge: &PromptBridge,
    refresh: &Refresh<'_>,
    response: &mut oneshot::Sender<AccountReply>,
    closed: Pin<&mut C>,
    decision_login: Option<&toolkit_mobkit::private_host::ConsoleLoginHandle>,
) -> Result<Option<T>, ConnectError>
where
    F: Future<Output = Result<T, ConnectError>>,
    C: Future<Output = ()>,
{
    drive_review_cancellable(review, approvals, presenter, bridge, refresh,
        response, closed, decision_login, None, None).await
}

pub(super) async fn drive_review_cancellable<T, F, C, R>(
    review: F,
    approvals: &LocalApprovals,
    presenter: &mut Option<ApprovalPresenter>,
    bridge: &PromptBridge,
    refresh: &Refresh<'_>,
    response: &mut oneshot::Sender<R>,
    mut closed: Pin<&mut C>,
    decision_login: Option<&toolkit_mobkit::private_host::ConsoleLoginHandle>,
    cancel_review: Option<ReviewControl>,
    owner_closed: Option<&AtomicBool>,
) -> Result<Option<T>, ConnectError>
where
    F: Future<Output = Result<T, ConnectError>>,
    C: Future<Output = ()>,
{
    tokio::pin!(review);
    let mut presented = false;
    loop {
        tokio::select! { biased;
            stopped = stop(response, bridge, closed.as_mut()) => {
                if let Some(cancel) = &cancel_review { cancel.retire(); }
                let retired = if stopped.owner_closed(owner_closed) {
                    let retired = approvals.retire_checked(); drop(presenter.take()); retired
                } else { Ok(()) };
                present::join_cleanup(retired, joined_review(review.await))?;
                return Ok(None);
            }
            result = review.as_mut() => return result.map(Some),
            prompt = async {
                match &cancel_review {
                    Some(control) => {
                        let id = control.approval().await?;
                        presenter.as_mut().expect("same retained presenter").next_for_review(&id).await
                    }
                    None => Ok(presenter.as_mut().expect("same retained presenter").next().await),
                }
            }, if !presented => {
                let prompt = match prompt {
                    Ok(prompt) => prompt,
                    Err(error) => {
                        if let Some(cancel) = &cancel_review { cancel.retire(); }
                        present::join_cleanup(Err(error), joined_review(review.await))?;
                        return Ok(None);
                    }
                };
                presented = true;
                let Some(mut prompt) = prompt else {
                    if let Some(cancel) = &cancel_review { cancel.retire(); }
                    let retired = approvals.retire_checked(); drop(presenter.take());
                    present::join_cleanup(retired, joined_review(review.await))?;
                    return Ok(None);
                };
                let shown = async {
                    refresh().await?;
                    let record = prompt.record();
                    let allowed = record.allowed_decisions.iter().filter_map(|decision| match decision {
                        ApprovalDecision::Approve => Some("approve"),
                        ApprovalDecision::Deny => Some("deny"),
                        _ => None,
                    }).collect::<Vec<_>>();
                    let projection = json!({"kind":"approval","approval_id":record.approval_id,
                        "summary":record.proposed_action.summary,"body":record.proposed_action.body,
                        "allowed_decisions":allowed,"expires_at":record.expires_at});
                    let answer = tokio::select! { biased;
                        _ = prompt.closed() => return Err(refused("The protected review expired")),
                        answer = bridge.ask(projection) => answer?,
                    };
                    refresh().await?;
                    let reservation = decision_login.map(|login| login.reserve()).transpose()
                        .map_err(|_| refused("The Console login refused approval entry"))?;
                    let result = match answer {
                        PromptAnswer::Approval { decision } => prompt.decide(match decision {
                            crate::stdio::wire::Decision::Approve => ApprovalDecision::Approve,
                            crate::stdio::wire::Decision::Deny => ApprovalDecision::Deny,
                        }),
                        _ => Err(refused("The protected account answer is invalid")),
                    };
                    if result.is_ok() {
                        if let Some(reservation) = reservation { reservation.settled(); }
                    }
                    result
                };
                let result = tokio::select! { biased;
                    stopped = stop(response, bridge, closed.as_mut()) => (None, Some(stopped)),
                    // The same review owns expiry and retirement while its
                    // prompt is displayed; presentation must keep polling it.
                    result = review.as_mut() => return result.map(Some),
                    result = shown => (Some(result), None),
                };
                if !matches!(result.0, Some(Ok(()))) {
                    if let Some(cancel) = &cancel_review { cancel.retire(); }
                    drop(prompt);
                    let retired = if result.1.is_some_and(|stop| stop.owner_closed(owner_closed)) {
                        let retired = approvals.retire_checked(); drop(presenter.take()); retired
                    } else { Ok(()) };
                    let joined = present::join_cleanup(retired, joined_review(review.await));
                    present::join_cleanup(result.0.unwrap_or(Ok(())), joined)?;
                    return Ok(None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

//! Private terminal transport for the same installed account owner.

use super::*;
use crate::{
    approval::{ApprovalPresenter, ApprovalPromptOutcome, present_approval_outcome},
    terminal::{TerminalInput, review_text},
};
use futures_util::FutureExt;
use meerkat_auth_core::auth_oauth::{
    LoopbackWaitEnd, OAuthError, bind_loopback_callback_with_redirect,
};
use std::{
    future::Future,
    io::{self, Write},
    pin::Pin,
    time::Duration,
};

// Callback resource bound, not access lifetime or native authority extension.
const CALLBACK_WINDOW: Duration = Duration::from_secs(120);

impl InstalledAccountRequests {
    /// This future stays owned until every entered native operation and callback
    /// cleanup has returned. The admitting tool only waits for historical facts.
    pub(crate) async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
        &mut self,
        request: InstalledAccountRequest,
        presenter: &mut Option<ApprovalPresenter>,
        input: &mut TerminalInput,
        output: &mut W,
        interrupt: F,
    ) -> Result<bool, ConnectError> {
        let result = self.present_inner(request, presenter, input, output, interrupt).await;
        let retired = if matches!(result, Ok(false))
            || self.review.approvals.check_session(&self.review.session).is_err()
        {
            let retired = self.review.retire();
            drop(presenter.take());
            retired
        } else if result.is_err() {
            self.review.control.retire().map_err(refused)
        } else { Ok(()) };
        match result {
            Ok(more) => retired.map(|()| more),
            Err(error) => join_cleanup(Err(error), retired).map(|()| true),
        }
    }

    async fn present_inner<F: Future<Output = io::Result<()>> + Send, W: Write>(
        &mut self,
        mut request: InstalledAccountRequest,
        presenter: &mut Option<ApprovalPresenter>,
        input: &mut TerminalInput,
        output: &mut W,
        interrupt: F,
    ) -> Result<bool, ConnectError> {
        if request.response.is_closed() {
            return Ok(true);
        }
        if request.task.as_ref().and_then(|task| task.console_login()).is_some() {
            // The selected Console human must use its same protected presenter.
            // A local terminal does not inherit that human's live login.
            return join_cleanup(
                Err(refused("This account request requires its retained Console presenter")),
                self.review.control.retire().map_err(refused),
            ).map(|()| false);
        }
        let approvals = self.review.approvals.clone();
        let (login, software) = self
            .review
            .selected_login(&request.job_id, request.selection.operations.clone())?;
        login.check_target(&request.target).map_err(refused)?;
        if software != request.software {
            return Err(refused("The selected installation changed"));
        }
        tokio::pin!(interrupt);
        if let Some(signal) = interrupt.as_mut().now_or_never() {
            return retire_stop(&mut self.review, Stop::Owner(signal));
        }
        let mut entry = Some(request.entry);
        if !checkpoint_or_retire(
            &mut self.review,
            &request.current,
            &mut request.response,
            input,
            interrupt.as_mut(),
        )
        .await?
        {
            return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
        }
        // This private observation uses the retained native account owner and
        // reveals no status to the tool. Mutation entry remains unspent until
        // the native ticket checks this exact request immediately before entry.
        let (status, stopped) = wait_joined(
            self.review.observe_account(&login, &request.target),
            input,
            &mut request.response,
            interrupt.as_mut(),
        )
        .await;
        if let Some(signal) = stopped {
            return retire_stop(&mut self.review, signal);
        }
        if !checkpoint_or_retire(
            &mut self.review,
            &request.current,
            &mut request.response,
            input,
            interrupt.as_mut(),
        )
        .await?
        {
            return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
        }
        let status = status?;
        let mut login_completed = false;
        let mut custody = None;
        if status.phase != meerkat::ConnectorAuthPhase::Authorized {
            // The native reviewed request already owns this exact sign-in
            // operation. Do not add a second generic approval for the same act.
            if request.response.is_closed() {
                return Ok(true);
            }
            approvals.check_session(&self.review.session)?;
            login.check_current().map_err(refused)?;
            // Enter once before opening the callback and starting authentication.
            // The native ticket retains this exact call's complete authority.
            custody = Some(
                entry
                    .take()
                    .expect("one native request entry")
                    .enter()
                    .map_err(|_| refused("The current account request cannot enter"))?,
            );
            let (binding, stopped) = wait_joined(
                bind_loopback_callback_with_redirect(
                    meerkat_auth_core::connector_login::CONNECTOR_OAUTH_CALLBACK_PATH,
                    "127.0.0.1",
                    &[0],
                ),
                input,
                &mut request.response,
                interrupt.as_mut(),
            )
            .await;
            let binding = match binding {
                Ok(binding) => binding,
                Err(error) => {
                    let retired = match stopped {
                        Some(stop) => retire_stop(&mut self.review, stop).map(|_| ()),
                        None => self.review.control.retire().map_err(refused),
                    };
                    return join_cleanup(
                        Err(ConnectError::provider_setup("Bind account callback", error)), retired,
                    )
                    .map(|()| false);
                }
            };
            if let Some(signal) = stopped {
                let cleanup = binding
                    .cancel()
                    .await
                    .map_err(|e| ConnectError::provider_setup("Close account callback", e));
                let more = retire_stop(&mut self.review, signal);
                return join_stop_cleanup(more, cleanup);
            }
            let current = checkpoint(
                &mut self.review,
                &request.current,
                &mut request.response,
                input,
                interrupt.as_mut(),
            )
            .await;
            if !matches!(current, Ok(true)) {
                let primary = join_cleanup(current.map(|_| ()), self.review.control.retire().map_err(refused));
                let cleanup = binding
                    .cancel()
                    .await
                    .map_err(|e| ConnectError::provider_setup("Close account callback", e));
                join_cleanup(primary, cleanup)?;
                return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
            }
            let redirect = binding.redirect_url.clone();
            let (started, stopped) = wait_joined(
                self.review.start_account(
                    &request.job_id,
                    request.target.clone(),
                    request.selection.operations.clone(),
                    redirect,
                ),
                input,
                &mut request.response,
                interrupt.as_mut(),
            )
            .await;
            if let Some(signal) = stopped {
                let more = retire_stop(&mut self.review, signal);
                let cleanup = binding
                    .cancel()
                    .await
                    .map_err(|e| ConnectError::provider_setup("Close account callback", e));
                let cleanup = join_cleanup(started.map(|_| ()), cleanup);
                return join_stop_cleanup(more, cleanup);
            }
            let started = match started {
                Ok(started) => started,
                Err(error) => {
                    let cleanup = binding
                        .cancel()
                        .await
                        .map_err(|e| ConnectError::provider_setup("Close account callback", e));
                    return join_cleanup(join_cleanup(Err(error), self.review.control.retire().map_err(refused)), cleanup)
                        .map(|()| false);
                }
            };
            let handle = started.host_attempt_handle.clone();
            let callback = binding.expect_state(started.state);
            // Finish formatting before entering any awaited cleanup branch.
            let displayed = writeln!(output, "Open this private sign-in URL in your browser:\n{}\nWaiting for account sign-in. Ctrl-C or terminal EOF waits for callback cleanup.", review_text(&started.authorize_url))
                .and_then(|()| output.flush());
            if let Err(error) = displayed {
                let primary = join_cleanup(Err(ConnectError::native("Display account sign-in", error)), self.review.retire());
                let cleanup = callback.cancel().await.map_err(|e| ConnectError::provider_setup("Close account callback", e));
                return join_cleanup(primary, cleanup).map(|()| false);
            }
            let mut retirement = None;
            let callback = callback
                .wait_or_cancel(tokio::time::Instant::now() + CALLBACK_WINDOW, async {
                    let signal =
                        account_stop(input, &mut request.response, interrupt.as_mut()).await;
                    retirement = Some(retire_stop(&mut self.review, signal));
                })
                .await;
            check_input_cleanup(&mut self.review, input)?;
            let callback = match callback {
                LoopbackWaitEnd::Completed { outcome, .. } => outcome,
                LoopbackWaitEnd::Cancelled { .. } => {
                    return retirement.unwrap_or_else(|| {
                        self.review.control.retire().map_err(refused).map(|()| true)
                    });
                }
                end => {
                    let failure = match end {
                        LoopbackWaitEnd::Refused { error, .. } => {
                            ConnectError::provider_setup("Await account callback", error)
                        }
                        LoopbackWaitEnd::TimedOut { .. } => ConnectError::provider_setup(
                            "Await account callback",
                            OAuthError::Timeout,
                        ),
                        LoopbackWaitEnd::RetirementFailed => {
                            refused("Native callback cleanup is unconfirmed")
                        }
                        _ => refused("Native callback returned an unsupported outcome"),
                    };
                    return join_cleanup(
                        Err(failure),
                        retirement.map(|result| result.map(|_| ()))
                            .unwrap_or_else(|| self.review.control.retire().map_err(refused)),
                    )
                    .map(|()| false);
                }
            };
            // Browser completion does not renew setup authority. Ask the same
            // still-borrowed dispatcher immediately before native completion.
            if !checkpoint_or_retire(
                &mut self.review,
                &request.current,
                &mut request.response,
                input,
                interrupt.as_mut(),
            )
            .await?
            {
                return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
            }
            let (_live, mut retired) = tokio::sync::watch::channel(false);
            let (cancel, receive) = oneshot::channel();
            let (completed, stopped) = {
                let completing = self.review.complete_account(
                    callback.state,
                    callback.code,
                    handle,
                    &mut retired,
                    receive,
                );
                tokio::pin!(completing);
                tokio::select! { biased;
                    signal = account_stop(input, &mut request.response, interrupt.as_mut()) => {
                        let _ = cancel.send(()); (completing.await, Some(signal))
                    },
                    result = completing.as_mut() => (result, None),
                }
            };
            check_input_cleanup(&mut self.review, input)?;
            let more = stopped.map(|stop| retire_stop(&mut self.review, stop));
            let completed = match completed {
                Ok(completed) => completed,
                Err(error) => return join_cleanup(Err(error),
                    more.map(|result| result.map(|_| ())).unwrap_or(Ok(()))).map(|()| false),
            };
            // Preserve actual native completion despite a later cancellation.
            login_completed = completed.result.is_ok();
            if completed.cleanup_unconfirmed {
                return join_cleanup(Err(refused("Native account cleanup is unconfirmed")),
                    more.map(|result| result.map(|_| ())).unwrap_or(Ok(()))).map(|()| false);
            }
            if let Some(more) = more { return more; }
            if !login_completed {
                // Clear only this terminal attempt through its existing owner,
                // so a later integration can use the same helper session.
                self.review.control.retire().map_err(refused)?;
                let _ = request.response.send(AccountReply::new(
                    AccountState::Unavailable,
                    false,
                    false,
                ));
                return Ok(true);
            }
        }
        if !checkpoint_or_retire(
            &mut self.review,
            &request.current,
            &mut request.response,
            input,
            interrupt.as_mut(),
        )
        .await?
        {
            return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
        }
        if let Some(entry) = entry.take() {
            custody = Some(entry.enter().map_err(|_| refused("The current account request cannot enter"))?);
        }
        let task_expires_in_ms = request.selection.expires_in_ms;
        let task_tool_policy = request.selection.tool_policy.clone();
        let (cancel_review, cancellation) = ReviewCancellation::channel();
        let reviewing = self.review.review_account_with_task(
            &request.job_id,
            request.target,
            request.selection.operations.clone(),
            InstalledAccessProposal {
                requester: requester()?,
                summary: "Review access for the verified installed account".into(),
                selection: request.selection,
                decision_expires_at: SystemTime::now() + Duration::from_secs(120),
            },
            request.task.take(),
            Some(&request.current),
            task_expires_in_ms,
            task_tool_policy,
            Some(cancellation),
        );
        let reviewed = drive_review_cancellable(
            reviewing,
            &approvals,
            presenter,
            input,
            output,
            &mut request.response,
            interrupt.as_mut(),
            Some(cancel_review),
        )
        .await?;
        let Some(reviewed) = reviewed else {
            return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
        };
        if !checkpoint_or_retire(
            &mut self.review,
            &request.current,
            &mut request.response,
            input,
            interrupt.as_mut(),
        )
        .await?
        {
            return Ok(self.review.approvals.check_session(&self.review.session).is_ok());
        }
        let reply = match reviewed {
            InstalledAccountReview::Approved(reviewed) => {
                let retained = reviewed.fresh_instruction_retained();
                self.reviewed.push((request.original, reviewed));
                let mut reply = AccountReply::new(AccountState::Reviewed, login_completed, true);
                reply.fresh_instruction_retained = retained;
                reply.grant_issued = retained;
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

// The existing ApprovalService owns these records. This requester is a private
// presentation label, never the application's native grantee or authority.
fn requester() -> Result<ApprovalPrincipalId, ConnectError> {
    ApprovalPrincipalId::new("connect-installed-setup-agent")
        .map_err(|e| ConnectError::native("Name installed account requester", e))
}

fn check_input_cleanup(
    review: &mut InstalledServiceReview,
    input: &TerminalInput,
) -> Result<(), ConnectError> {
    if let Err(error) = input.check_cleanup() {
        return join_cleanup(
            Err(ConnectError::native("Restore account input", error)),
            review.retire(),
        );
    }
    Ok(())
}

enum Stop { Caller, Owner(io::Result<()>) }

fn retire_stop(review: &mut InstalledServiceReview, stop: Stop) -> Result<bool, ConnectError> {
    match stop {
        Stop::Caller => review.control.retire().map_err(refused).map(|()| true),
        Stop::Owner(signal) => join_cleanup(
            signal.map_err(|error| ConnectError::native("Listen for account cancellation", error)),
            review.retire(),
        ).map(|()| false),
    }
}

fn join_stop_cleanup(more: Result<bool, ConnectError>, cleanup: Result<(), ConnectError>)
    -> Result<bool, ConnectError>
{
    match more {
        Ok(more) => cleanup.map(|()| more),
        Err(error) => join_cleanup(Err(error), cleanup).map(|()| false),
    }
}

async fn checkpoint_or_retire<I: Future<Output = io::Result<()>> + Send>(
    review: &mut InstalledServiceReview,
    current: &CurrentRequest,
    caller: &mut oneshot::Sender<AccountReply>,
    input: &mut TerminalInput,
    interrupt: Pin<&mut I>,
) -> Result<bool, ConnectError> {
    checkpoint(review, current, caller, input, interrupt).await
}

async fn checkpoint<I: Future<Output = io::Result<()>> + Send>(
    review: &mut InstalledServiceReview,
    current: &CurrentRequest,
    caller: &mut oneshot::Sender<AccountReply>,
    input: &mut TerminalInput,
    mut interrupt: Pin<&mut I>,
) -> Result<bool, ConnectError> {
    let result = tokio::select! { biased;
        signal = account_stop(input, caller, interrupt.as_mut()) => {
            retire_stop(review, signal)?;
            Ok(false)
        }
        current = current.check() => {
            if !current { review.control.retire().map_err(refused)?; }
            Ok(current)
        },
    };
    check_input_cleanup(review, input)?;
    result
}

async fn account_stop<I: Future<Output = io::Result<()>> + Send>(
    input: &mut TerminalInput,
    caller: &mut oneshot::Sender<AccountReply>,
    mut interrupt: Pin<&mut I>,
) -> Stop {
    tokio::select! { biased;
        signal = interrupt.as_mut() => Stop::Owner(signal),
        _ = caller.closed() => Stop::Caller,
        ended = input.wait_for_end() => Stop::Owner(ended),
    }
}

// Start this exact future once, then join it if closure wins. This helper does
// not claim that a future has entered a native effect merely because it polled.
// The existing native future/guard remains responsible for its effect cleanup.
async fn wait_joined<F, I>(
    future: F,
    input: &mut TerminalInput,
    caller: &mut oneshot::Sender<AccountReply>,
    mut interrupt: Pin<&mut I>,
) -> (F::Output, Option<Stop>)
where
    F: Future,
    I: Future<Output = io::Result<()>> + Send,
{
    tokio::pin!(future);
    let (result, stopped) = match future.as_mut().now_or_never() {
        Some(result) => (result, None),
        None => tokio::select! { biased;
            signal = account_stop(input, caller, interrupt.as_mut()) => (future.await, Some(signal)),
            result = future.as_mut() => (result, None),
        },
    };
    let cleanup = input.check_cleanup();
    let stopped = match (stopped, cleanup) {
        (_, Err(error)) => Some(Stop::Owner(Err(error))),
        (Some(signal), Ok(())) => Some(signal),
        (None, Ok(())) => None,
    };
    (result, stopped)
}

/// Join this same future on every stop. Only actual presentation-owner closure
/// drops the sole receiver; request-local refusal preserves sibling reviews.
/// A returned decision may already be followed by an entered status read.
async fn drive_review_cancellable<T, F, I, W>(
    future: F,
    approvals: &LocalApprovals,
    presenter: &mut Option<ApprovalPresenter>,
    input: &mut TerminalInput,
    output: &mut W,
    caller: &mut oneshot::Sender<AccountReply>,
    mut interrupt: Pin<&mut I>,
    cancel_review: Option<ReviewControl>,
) -> Result<Option<T>, ConnectError>
where
    F: Future<Output = Result<T, ConnectError>>,
    I: Future<Output = io::Result<()>> + Send,
    W: Write,
{
    tokio::pin!(future);
    let mut presented = false;
    loop {
        tokio::select! { biased;
            signal = account_stop(input, caller, interrupt.as_mut()) => {
                if let Some(cancel) = &cancel_review { cancel.retire(); }
                let retired = match signal {
                    Stop::Caller => Ok(()),
                    Stop::Owner(signal) => {
                        let retired = approvals.retire_checked(); drop(presenter.take());
                        join_cleanup(signal.map_err(|error| ConnectError::native(
                            "Listen for account review cancellation", error)), retired)
                    }
                };
                join_cleanup(retired, joined_review(future.await))?;
                return Ok(None);
            }
            result = future.as_mut() => {
                input.check_cleanup().map_err(|e| ConnectError::native("Restore account review input", e))?;
                return result.map(Some);
            },
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
                        join_cleanup(Err(error), joined_review(future.await))?;
                        return Ok(None);
                    }
                };
                presented = true;
                let Some(prompt) = prompt else {
                    if let Some(cancel) = &cancel_review { cancel.retire(); }
                    let retired = approvals.retire_checked(); drop(presenter.take());
                    join_cleanup(retired, joined_review(future.await))?;
                    return Ok(None);
                };
                // Bytes typed before these account facts were displayed cannot
                // approve them. Keep the existing successful account attempt;
                // only the same input owner's type-ahead is discarded.
                let fresh = input.fresh_prompt();
                if !matches!(fresh, Ok(true)) {
                    drop(prompt);
                    if let Some(cancel) = &cancel_review { cancel.retire(); }
                    let retired = approvals.retire_checked(); drop(presenter.take());
                    join_cleanup(retired, joined_review(future.await))?;
                    fresh.map_err(|e| ConnectError::native("Prepare fresh account review input", e))?;
                    return Ok(None);
                }
                let owner_stopped = std::sync::atomic::AtomicBool::new(false);
                let (shown, completed) = {
                    let shown = present_approval_outcome(prompt, input, output, async {
                        tokio::select! { biased;
                            signal = interrupt.as_mut() => {
                                owner_stopped.store(true, std::sync::atomic::Ordering::SeqCst); signal
                            },
                            _ = caller.closed() => Ok(()),
                        }
                    });
                    tokio::pin!(shown);
                    tokio::select! { biased;
                        shown = shown.as_mut() => (Some(shown), None),
                        result = future.as_mut() => (None, Some(result)),
                    }
                };
                if let Some(result) = completed {
                    input.check_cleanup().map_err(|e| ConnectError::native("Restore account review input", e))?;
                    return result.map(Some);
                }
                let shown = shown.expect("presentation completed without the review");
                if !matches!(shown, Ok(ApprovalPromptOutcome::Continue)) {
                    if let Some(cancel) = &cancel_review { cancel.retire(); }
                    let owner_closed = owner_stopped.load(std::sync::atomic::Ordering::SeqCst)
                        || shown.is_err()
                        || (matches!(shown, Ok(ApprovalPromptOutcome::Stopped)) && !caller.is_closed());
                    let retired = if owner_closed {
                        let retired = approvals.retire_checked(); drop(presenter.take()); retired
                    } else { Ok(()) };
                    let shown = match shown {
                        Ok(ApprovalPromptOutcome::Refused(error)) | Err(error) => Err(error),
                        _ => Ok(()),
                    };
                    let joined = join_cleanup(retired, joined_review(future.await));
                    join_cleanup(shown, joined)?;
                    return Ok(None);
                }
            }
        }
    }
}

pub(super) fn join_cleanup(
    first: Result<(), ConnectError>,
    second: Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    match (first, second) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{poll_fn, ready},
        task::Poll,
    };

    struct NoticedOutput {
        bytes: Vec<u8>,
        shown: std::sync::Arc<tokio::sync::Notify>,
    }
    impl Write for NoticedOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.shown.notify_one();
            Ok(())
        }
    }
    fn expiring_account_request(session: &SessionId, lifetime: Duration) -> ApprovalRequest {
        ApprovalRequest {
            requester: requester().unwrap(),
            owner: ApprovalOwnerRef::Session { session_id: session.clone() },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("test-account"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "Exact private account review".into(),
                body: Some(json!({"verified":"account"})),
            },
            risk: ApprovalRisk::Medium,
            request_body: Some(json!({"verified":"account"})),
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
            expires_at: Some((SystemTime::now() + lifetime).into()),
            metadata: Default::default(),
            request_provenance: None,
        }
    }

    #[tokio::test]
    async fn terminal_account_review_expires_while_shown_and_reuses_input_and_presenter() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory = tempfile::tempdir().unwrap();
            let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let mut presenter = Some(presenter);
            let (lines, receive_lines) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive_lines);
            let (mut caller, _reply) = oneshot::channel();
            let stop = std::future::pending::<io::Result<()>>();
            tokio::pin!(stop);
            let (control, cancellation) = ReviewCancellation::channel();
            let review = approvals.review_access_cancellable(
                expiring_account_request(&session, Duration::from_millis(500)),
                cancellation.cancelled(), |id| cancellation.approval_created(id));
            let mut output = Vec::new();
            let result = drive_review_cancellable(review, &approvals, &mut presenter,
                &mut input, &mut output, &mut caller, stop.as_mut(), Some(control)).await;
            assert!(matches!(result, Err(ConnectError::InvalidInput(ref message))
                if message == "The access review expired without a returned decision"));
            assert_eq!(String::from_utf8(output).unwrap().matches("Type 'approve ").count(), 1);
            assert!(presenter.is_some());
            approvals.check_session(&session).unwrap();
            input.check_cleanup().unwrap();
            lines.send(Ok("input after expiry".into())).await.unwrap();
            assert_eq!(input.recv().await.unwrap().unwrap(), "input after expiry");

            let (control, cancellation) = ReviewCancellation::channel();
            let (created, receive_created) = oneshot::channel();
            let review = approvals.review_access_cancellable(
                expiring_account_request(&session, Duration::from_secs(120)),
                cancellation.cancelled(), |id| {
                    cancellation.approval_created(id);
                    created.send(id.clone()).unwrap();
                });
            let shown = std::sync::Arc::new(tokio::sync::Notify::new());
            let mut output = NoticedOutput { bytes: Vec::new(), shown: shown.clone() };
            let driver = drive_review_cancellable(review, &approvals, &mut presenter,
                &mut input, &mut output, &mut caller, stop.as_mut(), Some(control));
            let respond = async {
                shown.notified().await;
                let id = receive_created.await.unwrap();
                lines.send(Ok(format!("deny {id}"))).await.unwrap();
            };
            let (result, ()) = tokio::join!(driver, respond);
            assert_eq!(result.unwrap().unwrap().status, meerkat_core::ApprovalStatus::Denied);
            assert_eq!(String::from_utf8(output.bytes).unwrap().matches("Type 'approve ").count(), 1);
            assert!(presenter.is_some());
            approvals.check_session(&session).unwrap();
            input.check_cleanup().unwrap();
        }).await.expect("displayed terminal account review did not settle at its actual approval deadline");
    }

    #[tokio::test]
    async fn terminal_account_review_cancellation_while_shown_joins_and_releases_input() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory = tempfile::tempdir().unwrap();
            let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let mut presenter = Some(presenter);
            let (lines, receive_lines) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive_lines);
            let (mut caller, _reply) = oneshot::channel();
            let (stop, receive_stop) = oneshot::channel();
            let interrupt = async { receive_stop.await.unwrap() };
            tokio::pin!(interrupt);
            let (control, cancellation) = ReviewCancellation::channel();
            let review = approvals.review_access_cancellable(
                expiring_account_request(&session, Duration::from_secs(120)),
                cancellation.cancelled(), |id| cancellation.approval_created(id));
            let shown = std::sync::Arc::new(tokio::sync::Notify::new());
            let mut output = NoticedOutput { bytes: Vec::new(), shown: shown.clone() };
            let driver = drive_review_cancellable(review, &approvals, &mut presenter,
                &mut input, &mut output, &mut caller, interrupt.as_mut(), Some(control));
            let cancel = async { shown.notified().await; stop.send(Ok(())).unwrap(); };
            let (result, ()) = tokio::join!(driver, cancel);
            assert!(result.unwrap().is_none());
            assert!(presenter.is_none());
            assert!(approvals.check_session(&session).is_err());
            input.check_cleanup().unwrap();
            lines.send(Ok("input after cancellation".into())).await.unwrap();
            assert_eq!(input.recv().await.unwrap().unwrap(), "input after cancellation");
        }).await.expect("displayed terminal cancellation did not join its review and input");
    }

    #[tokio::test]
    async fn cancelled_private_review_drops_queued_prompt_then_joins_same_native_review() {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let request = ApprovalRequest {
            requester: requester().unwrap(),
            owner: ApprovalOwnerRef::Session {
                session_id: session.clone(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("test-account"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "Exact private account review".into(),
                body: Some(json!({"verified":"account"})),
            },
            risk: ApprovalRisk::Medium,
            request_body: Some(json!({"verified":"account"})),
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: Some((SystemTime::now() + Duration::from_secs(120)).into()),
            metadata: Default::default(),
            request_provenance: None,
        };
        let (cancel_review, cancellation) = ReviewCancellation::channel();
        let review = approvals.review_access_cancellable(request, cancellation.cancelled(),
            |id| cancellation.approval_created(id));
        tokio::pin!(review);
        assert!(
            poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        let mut presenter = Some(presenter);
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        let mut output = Vec::new();
        let (mut caller, _reply) = oneshot::channel();
        let stop = ready(Ok(()));
        tokio::pin!(stop);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            drive_review_cancellable(
                review,
                &approvals,
                &mut presenter,
                &mut input,
                &mut output,
                &mut caller,
                stop.as_mut(),
                Some(cancel_review),
            ),
        )
        .await;
        assert!(
            result
                .expect("queued prompt prevented joined cancellation")
                .unwrap()
                .is_none()
        );
        assert!(presenter.is_none());
        assert!(approvals.check_session(&session).is_err());
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn terminal_eof_joins_the_same_pending_account_wait_before_returning() {
        // Exercise the actual production wait helper. The held future is only
        // a transport/cleanup oracle, not a fabricated native account outcome.
        let (lines, receive_lines) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive_lines);
        let (mut caller, _reply) = oneshot::channel();
        let stop = std::future::pending::<io::Result<()>>();
        tokio::pin!(stop);
        let (finish, receive_finish) = oneshot::channel();
        let entered = std::cell::Cell::new(false);
        let joined = std::cell::Cell::new(false);
        let operation = async {
            entered.set(true);
            let value = receive_finish.await.unwrap();
            joined.set(true);
            value
        };
        let waiting = wait_joined(operation, &mut input, &mut caller, stop.as_mut());
        tokio::pin!(waiting);
        assert!(
            poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        assert!(entered.get());
        drop(lines); // EOF through the same input owner, after the wait entered.
        assert!(
            poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        assert!(
            !joined.get(),
            "EOF dropped or replaced the pending operation"
        );
        finish.send(41).unwrap();
        let (value, stopped) = waiting.await;
        assert_eq!(value, 41);
        assert!(joined.get());
        assert!(
            matches!(stopped, Some(Stop::Owner(Ok(())))),
            "EOF was not the stopping event"
        );
    }
}

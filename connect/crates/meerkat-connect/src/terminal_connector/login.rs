//! One retained terminal ceremony for installation and existing-account login.
use super::*;
use crate::connector_auth::{CompletedLogin, LoginSelection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stop { Cancelled, Exit, CallerGone }

pub(super) enum End {
    Completed(Arc<CompletedLogin>),
    Refused(&'static str),
    Stopped { reason: Stop, completion: Option<Result<Arc<CompletedLogin>, &'static str>> },
}

impl End {
    pub(super) fn stop_message(&self) -> &'static str {
        match self {
            Self::Stopped { completion: Some(Ok(_)), .. } => "Sign-in completed before cancellation settled. The account remains connected. No installation or grant was performed by this ceremony.",
            Self::Stopped { completion: Some(Err(_)), .. } => "Sign-in completion is unconfirmed. Cancellation does not prove credentials were unchanged. Do not repeat the attempt automatically.",
            _ => "This sign-in attempt was cancelled. No login completion was entered. Existing accounts and grants were not disconnected or revoked.",
        }
    }
    pub(super) fn stop_reply(&self) -> Option<Value> {
        let Self::Stopped { reason, completion } = self else { return None };
        Some(json!({"state":match reason { Stop::Cancelled => "cancelled", Stop::Exit => "stopped", Stop::CallerGone => "caller_gone" },
            "phase":"sign_in", "login_completed":match completion {
                None => Some(false), Some(Ok(_)) => Some(true), Some(Err(_)) => None,
            }, "completion_reason":completion.as_ref().and_then(|result| result.as_ref().err()).copied(),
            "grant_issued":false,
            "instruction":"This sign-in attempt has settled. A completed login remains connected; cancelling an attempt does not disconnect an account or revoke access. An unavailable completion is not proof that credentials were unchanged. No installation or grant was performed by this ceremony."}))
    }
}

async fn wait_stop<F: Future<Output = io::Result<()>>>(
    input: &mut TerminalInput, caller: &mut oneshot::Sender<Value>,
    mut interrupt: Pin<&mut F>, output: &mut impl Write,
) -> io::Result<Stop> {
    loop {
        tokio::select! { biased;
            signal = interrupt.as_mut() => { signal?; return Ok(Stop::Exit); },
            _ = caller.closed() => return Ok(Stop::CallerGone),
            line = input.recv() => match line {
                None => return Ok(Stop::Exit),
                Some(Err(error)) => return Err(error),
                Some(Ok(line)) if line == "/cancel" => return Ok(Stop::Cancelled),
                Some(Ok(line)) if line == "/quit" => return Ok(Stop::Exit),
                Some(Ok(_)) => {
                    // Never echo a value accidentally pasted at an auth step.
                    writeln!(output, "Sign-in is in progress. Enter /cancel to return to the conversation, or /quit to exit.")?;
                    output.flush()?;
                },
            },
        }
    }
}

fn stopped(reason: io::Result<Stop>, completion: Option<Result<Arc<CompletedLogin>, &'static str>>)
    -> Result<End, ConnectError> {
    Ok(End::Stopped { reason:reason.map_err(|e| ConnectError::native("Listen for sign-in cancellation", e))?, completion })
}

/// The caller owns this future until start, completion and callback cleanup
/// have settled. No stop drops an entered native effect or changes its target.
pub(super) async fn run<F: Future<Output = io::Result<()>> + Send, W: Write>(
    control: &mut Control, target: meerkat::ConnectorOAuthTarget, selection: LoginSelection,
    caller: &mut oneshot::Sender<Value>, input: &mut TerminalInput, output: &mut W,
    interrupt: Pin<&mut F>,
) -> Result<End, ConnectError> {
    run_with_window(control, target, selection, caller, input, output, interrupt, WINDOW).await
}

// Production always uses WINDOW. The component control exercises native expiry
// through this same ceremony without extending its deadline or sleeping.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_with_window<F: Future<Output = io::Result<()>> + Send, W: Write>(
    control: &mut Control, target: meerkat::ConnectorOAuthTarget, selection: LoginSelection,
    caller: &mut oneshot::Sender<Value>, input: &mut TerminalInput, output: &mut W,
    mut interrupt: Pin<&mut F>, window: Duration,
) -> Result<End, ConnectError> {
    let binding = bind_loopback_callback_with_redirect(
        meerkat_auth_core::connector_login::CONNECTOR_OAUTH_CALLBACK_PATH, "127.0.0.1", &[0],
    ).await.map_err(|e| ConnectError::provider_setup("Bind connector callback", e))?;
    let redirect = binding.redirect_url.clone();
    let before_start = {
        let stopping = wait_stop(input, caller, interrupt.as_mut(), output);
        tokio::pin!(stopping);
        stopping.as_mut().now_or_never()
    };
    if let Some(signal) = before_start {
        binding.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e))?;
        input.check_cleanup().map_err(|e| ConnectError::native("Restore sign-in input", e))?;
        return stopped(signal, None);
    }
    if let Err(error) = input.check_cleanup() {
        let cleanup = binding.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e));
        return join_results(Err(ConnectError::native("Restore sign-in input", error)), cleanup)
            .map(|()| End::Refused("terminal_unavailable"));
    }
    let (started, stop) = {
        let stopping = wait_stop(input, caller, interrupt.as_mut(), output);
        tokio::pin!(stopping);
        let starting = control.start_selected(target, redirect, selection);
        tokio::pin!(starting);
        match starting.as_mut().now_or_never() {
            Some(result) => (result, None),
            None => tokio::select! { biased;
                signal = stopping.as_mut() => (starting.await, Some(signal)),
                result = starting.as_mut() => (result, None),
            },
        }
    };
    if let Some(signal) = stop {
        let retired = control.retire().map_err(unavailable);
        let cleanup = binding.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e));
        join_results(retired, cleanup)?;
        input.check_cleanup().map_err(|e| ConnectError::native("Restore sign-in input", e))?;
        return stopped(signal, None);
    }
    if let Err(error) = input.check_cleanup() {
        let retired = control.retire().map_err(unavailable);
        let cleanup = binding.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e));
        return join_results(Err(ConnectError::native("Restore sign-in input", error)), join_results(retired, cleanup))
            .map(|()| End::Refused("terminal_unavailable"));
    }
    if let Err(code) = started {
        binding.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e))?;
        return Ok(End::Refused(code));
    }
    let Some(started) = control.started() else {
        binding.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e))?;
        return Err(unavailable("Native sign-in did not retain its ceremony"));
    };
    let state = started.state.clone();
    let handle = started.host_attempt_handle.clone();
    let callback = binding.expect_state(state);
    if let Err(error) = writeln!(output,
        "Open this private sign-in URL in your browser:\n{}\nWaiting for native sign-in. /cancel returns to the conversation; /quit or Ctrl-C exits after cleanup.",
        review_text(&started.authorize_url)).and_then(|()| output.flush()) {
        let retired = control.retire().map_err(unavailable);
        let cleanup = callback.cancel().await.map_err(|e| ConnectError::provider_setup("Close connector callback", e));
        join_results(retired, cleanup)?;
        return Err(ConnectError::native("Display connector sign-in", error));
    }
    let deadline = tokio::time::Instant::now() + window;
    let mut stop = None;
    let mut retirement = None;
    // The native callback accepts a Send cancellation future. Keep terminal
    // I/O on this caller's task and relay only its stop signal to that owner.
    // Retire the exact attempt before signalling, then join the same drain.
    let (cancel, cancelled) = oneshot::channel();
    let callback = {
        let waiting = callback.wait_or_cancel(deadline, async { let _ = cancelled.await; });
        tokio::pin!(waiting);
        tokio::select! { biased;
            signal = wait_stop(input, caller, interrupt.as_mut(), output) => {
                retirement = Some(control.retire().map_err(unavailable));
                stop = Some(signal);
                let _ = cancel.send(());
                waiting.await
            },
            result = waiting.as_mut() => result,
        }
    };
    let callback = match callback {
        LoopbackWaitEnd::Completed { outcome, .. } => outcome,
        LoopbackWaitEnd::Cancelled { .. } => {
            retirement.unwrap_or_else(|| control.retire().map_err(unavailable))?;
            input.check_cleanup().map_err(|e| ConnectError::native("Restore sign-in input", e))?;
            return stopped(stop.unwrap_or(Ok(Stop::Cancelled)), None);
        },
        LoopbackWaitEnd::Refused { .. } | LoopbackWaitEnd::TimedOut { .. } => {
            // Both outcomes include native callback drain receipts. Retire the
            // exact attempt and restore terminal input before local feedback.
            retirement.unwrap_or_else(|| control.retire().map_err(unavailable))?;
            input.check_cleanup().map_err(|e| ConnectError::native("Restore sign-in input", e))?;
            if let Some(signal) = stop { return stopped(signal, None); }
            return Ok(End::Refused("callback_unavailable"));
        },
        end => {
            let failure = match end {
                LoopbackWaitEnd::RetirementFailed => ConnectError::provider_setup("Close connector callback", CallbackWaitFailure::RetirementFailed),
                _ => ConnectError::provider_setup("Close connector callback", CallbackWaitFailure::UnsupportedOutcome),
            };
            let retired = retirement.unwrap_or_else(|| control.retire().map_err(unavailable));
            return join_results(Err(failure), retired).map(|()| End::Refused("callback_unavailable"));
        },
    };
    if let Some(signal) = stop {
        retirement.unwrap_or_else(|| control.retire().map_err(unavailable))?;
        input.check_cleanup().map_err(|e| ConnectError::native("Restore sign-in input", e))?;
        return stopped(signal, None);
    }
    if let Err(error) = input.check_cleanup() {
        let retired = control.retire().map_err(unavailable);
        return join_results(Err(ConnectError::native("Restore sign-in input", error)), retired)
            .map(|()| End::Refused("terminal_unavailable"));
    }
    let (_live, mut retired) = watch::channel(false);
    let (cancel, receive) = oneshot::channel();
    let (completed, stop) = {
        let completing = control.complete(callback.state, callback.code, handle, &mut retired, receive);
        tokio::pin!(completing);
        let stopping = wait_stop(input, caller, interrupt.as_mut(), output);
        tokio::pin!(stopping);
        tokio::select! { biased;
            signal = stopping.as_mut() => { let _ = cancel.send(()); (completing.await, Some(signal)) },
            result = completing.as_mut() => (result, None),
        }
    };
    let (reply, _cancellation, cleanup_failed) = completed;
    let completed = settle_terminal_completion(control, reply, cleanup_failed)?;
    input.check_cleanup().map_err(|e| ConnectError::native("Restore sign-in input", e))?;
    if let Some(signal) = stop { return stopped(signal, Some(completed)); }
    match completed { Ok(completed) => Ok(End::Completed(completed)), Err(code) => Ok(End::Refused(code)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_cancel_returns_to_conversation_and_does_not_echo_input() {
        let (send, receive) = mpsc::channel(4);
        send.send(Ok("accidentally-pasted-private-value".into())).await.unwrap();
        send.send(Ok("/cancel".into())).await.unwrap();
        let mut input = TerminalInput::scripted(receive);
        let (mut caller, _response) = oneshot::channel();
        let signal = std::future::pending(); tokio::pin!(signal);
        let mut output = Vec::new();
        assert_eq!(wait_stop(&mut input, &mut caller, signal.as_mut(), &mut output).await.unwrap(), Stop::Cancelled);
        assert!(!String::from_utf8(output).unwrap().contains("private-value"));
        send.send(Ok("a different integration".into())).await.unwrap();
        assert_eq!(input.recv().await.unwrap().unwrap(), "a different integration");
    }

    #[test]
    fn cancellation_does_not_translate_unknown_native_completion_into_no_login() {
        let before = End::Stopped { reason:Stop::Cancelled, completion:None }.stop_reply().unwrap();
        assert_eq!(before["login_completed"], false);
        let unknown = End::Stopped { reason:Stop::Cancelled, completion:Some(Err("connector_unavailable")) }.stop_reply().unwrap();
        assert!(unknown["login_completed"].is_null());
        assert_eq!(unknown["completion_reason"], "connector_unavailable");
        assert_eq!(unknown["grant_issued"], false);
    }
}

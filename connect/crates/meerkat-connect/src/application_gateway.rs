//! Local controller and setup review in the existing gateway process. The inherited
//! application channel may request a review; only the explicitly owned terminal
//! supplies the decision. Neither channel JSON nor the terminal descriptor is a
//! native principal. The existing OS facade and same grant owner issue access.

pub(crate) mod setup_presenter;
mod local_recovery;
pub mod console_setup;
pub use console_setup::serve_console_setup;
pub use setup_presenter::{LocalSetupApprovals, LocalSetupReviews};
use setup_presenter::{Enrollment, SetupEvent, SetupPresenters};

use crate::{
    ConnectError,
    application_operator::{review_local_controller_parent, review_local_setup_request},
    approval::{ApprovalPresenter, ApprovalPrompt, ApprovalPromptOutcome, LocalApprovals, present_approval, present_approval_outcome},
    terminal::TerminalInput,
};
use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::{OwnedFd, RawFd},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::Arc,
};
use toolkit_mobkit::{
    host_gateway::ToolkitGatewayHost,
    private_host::{LocalControllerReviews, PrivateHostClosed},
};

/// The parent transfers this terminal exclusively, before Tokio or child
/// creation. RPC/private descriptors cannot double as operator input. There is
/// no /dev/tty lookup and no background reader or input queue.
pub struct InheritedOperatorTerminal {
    input: TerminalInput,
    output: File,
}

impl InheritedOperatorTerminal {
    /// Enroll setup decisions in this terminal's existing operator loop.
    /// Pass the returned receiver to `serve_local_operator_with_setup`; retain
    /// the creator only in trusted construction of the selected setup helper.
    /// This creates no approval store, session, task or second input reader.
    pub fn setup_reviews(&self, artifact_parent: PathBuf) -> (LocalSetupApprovals, LocalSetupReviews) {
        setup_presenter::channel(artifact_parent)
    }

    pub fn adopt(fd: RawFd, private_read: RawFd, private_write: RawFd) -> io::Result<Self> {
        if fd < 3 || fd == private_read || fd == private_write {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "operator terminal must be distinct from RPC and private descriptors"));
        }
        // This source descriptor belongs to the child even if validation fails.
        // The private/RPC descriptors were excluded before taking ownership.
        struct Original(RawFd);
        impl Drop for Original {
            fn drop(&mut self) { let _ = nix::unistd::close(self.0); }
        }
        let original = Original(fd);
        let path = format!("/dev/fd/{fd}");
        let before = std::fs::metadata(&path)?;
        if !before.file_type().is_char_device() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "operator input is not a terminal"));
        }
        // Reopening an inherited PTY must not make it this process's controlling
        // terminal. A session-leading gateway otherwise receives SIGHUP when
        // its operator closes the master, before owned cleanup can settle.
        let file = OpenOptions::new().read(true).write(true)
            .custom_flags(nix::libc::O_NOCTTY).open(path)?;
        let after = file.metadata()?;
        if before.dev() != after.dev() || before.ino() != after.ino()
            || before.rdev() != after.rdev()
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "operator terminal changed during adoption"));
        }
        nix::sys::termios::tcgetattr(&file).map_err(io::Error::from)?;
        let flags = nix::fcntl::fcntl(&file, nix::fcntl::FcntlArg::F_GETFD)
            .map_err(io::Error::from)?;
        if !nix::fcntl::FdFlag::from_bits_truncate(flags)
            .contains(nix::fcntl::FdFlag::FD_CLOEXEC)
        {
            return Err(io::Error::other("operator terminal is not close-on-exec"));
        }
        let input: OwnedFd = file.try_clone()?.into();
        drop(original);
        Ok(Self { input: TerminalInput::from_owned_fd(input), output: file })
    }
}

// Close the sole receiver before joining the review. A queued prompt owns its
// one-shot sender even after LocalApprovals retirement, so retaining the receiver
// here would defer cancellation until the human deadline. A review already past
// the decision still owns its native work and is always awaited to settlement.
async fn retire_presenter_and_join<T>(
    approvals: &LocalApprovals,
    presenter: &mut Option<ApprovalPresenter>,
    review: impl std::future::Future<Output = Result<T, ConnectError>>,
) -> Result<T, ConnectError> {
    let retired = approvals.retire_checked();
    drop(presenter.take());
    let result = review.await;
    retired?;
    result
}

enum ReviewPresentation<T> {
    Reviewed(Result<T, ConnectError>),
    Presented(Result<bool, ConnectError>),
}

// Presentation retains the sole input owner while the same review drives its
// deadline. A ready interruption inside presentation keeps its precedence.
async fn present_approval_with_review<T>(
    prompt: ApprovalPrompt,
    input: &mut TerminalInput,
    output: &mut impl io::Write,
    interrupted: impl std::future::Future<Output = io::Result<()>>,
    review: std::pin::Pin<&mut impl std::future::Future<Output = Result<T, ConnectError>>>,
) -> ReviewPresentation<T> {
    let result = {
        let shown = present_approval(prompt, input, output, interrupted);
        tokio::pin!(shown);
        tokio::select! { biased;
            shown = shown.as_mut() => ReviewPresentation::Presented(shown),
            result = review => ReviewPresentation::Reviewed(result),
        }
    };
    if matches!(&result, ReviewPresentation::Reviewed(_)) {
        if let Err(error) = input.check_cleanup() {
            return ReviewPresentation::Reviewed(Err(ConnectError::native("restore operator terminal", error)));
        }
    }
    result
}

/// Run beside the existing Meta gateway. The private server retains every
/// admitted request in its JoinSet. Parent loss retires undecided prompts, while
/// a returned human decision and already-started native persistence are joined.
/// Only an exact result is returned for the application's existing compiler;
/// this does not publish, reissue, restart or admit the original automatically.
pub async fn serve_local_operator(
    host: Arc<ToolkitGatewayHost>,
    requests: LocalControllerReviews,
    terminal: InheritedOperatorTerminal,
    artifact_parent: PathBuf,
    closed: PrivateHostClosed,
) -> Result<(), ConnectError> {
    serve_operator(host, requests, terminal, artifact_parent, closed, None).await
}

/// Present the selected helper's exact software and account review prompts on
/// the same terminal between complete controller review requests. A separate
/// setup prompt never pauses an entered controller future. Dropping enrolled presenters
/// retires unanswered eligibility. Entered native dispatch remains under the
/// runtime shutdown owner and its checked report. Account requests enter the
/// existing account owner only when this loop accepts them. This loop retains
/// and awaits its start/callback/completion through cancellation before close.
/// Enrollment transfers owners and requests, never already-entered futures.
pub async fn serve_local_operator_with_setup(
    host: Arc<ToolkitGatewayHost>,
    requests: LocalControllerReviews,
    terminal: InheritedOperatorTerminal,
    artifact_parent: PathBuf,
    closed: PrivateHostClosed,
    setup: LocalSetupReviews,
) -> Result<(), ConnectError> {
    serve_operator(host, requests, terminal, artifact_parent, closed, Some(setup)).await
}

async fn serve_operator(
    host: Arc<ToolkitGatewayHost>,
    mut requests: LocalControllerReviews,
    mut terminal: InheritedOperatorTerminal,
    artifact_parent: PathBuf,
    closed: PrivateHostClosed,
    setup: Option<LocalSetupReviews>,
) -> Result<(), ConnectError> {
    let mut setup = setup.map(SetupPresenters::new);
    let recovery_selected = host.local_operator_recovery_selected();
    if recovery_selected {
        use std::io::Write as _;
        writeln!(terminal.output, "Installed-host operator recovery is selected. Type /recover while this terminal is idle to inspect unresolved approvals.")
            .and_then(|()| terminal.output.flush())
            .map_err(|error| ConnectError::native("display local recovery entry", error))?;
    }
    loop {
        let request = tokio::select! {
            biased;
            _ = closed.wait_closed() => return Ok(()),
            prompt = async { setup.as_mut().expect("guarded setup presenter").next().await }, if setup.is_some() => {
                let Some((enrollment, prompt)) = prompt else { drop(setup.take()); continue; };
                let more = present_setup_prompt(setup.as_mut().expect("retained setup presenter"),
                    enrollment, prompt, &mut terminal, &closed).await?;
                terminal.input.check_cleanup()
                    .map_err(|e| ConnectError::native("restore operator terminal", e))?;
                if !more { return Ok(()); }
                continue;
            }
            request = requests.next() => match request { Some(request) => request, None => return Ok(()) },
            command = terminal.input.recv(), if recovery_selected => {
                let Some(command) = command else { return Ok(()); };
                let command = command.map_err(|error| ConnectError::native("read operator command", error))?;
                if command.trim() == "/recover" {
                    if !local_recovery::present(&host, &mut terminal, &artifact_parent, &closed).await? { return Ok(()); }
                } else {
                    use std::io::Write as _;
                    writeln!(terminal.output, "Use /recover to inspect unresolved installed-host approvals. Other input is handled by its active review prompt.")
                        .and_then(|()| terminal.output.flush())
                        .map_err(|error| ConnectError::native("display operator command help", error))?;
                }
                continue;
            }
        };
        // This native call is retained to completion even if the parent closes.
        // It returns before input admission and before any human wait starts.
        let prepared = match host.prepare_local_controller_setup(request.submission).await {
            Ok(prepared) => prepared,
            Err(_) => {
                let _ = request.response.send(Err(
                    meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable));
                continue;
            }
        };
        let mut ended = false;
        let result = async {
            terminal.input.require_terminal()
                .map_err(|e| ConnectError::native("check retained operator terminal", e))?;
            // Reject a foreign or stale owner before reserving a prompt/store.
            host.local_operator_setup_for(&prepared)
                .map_err(|e| ConnectError::native("join local operator source owner", e))?;
            let (approvals, presenter) = LocalApprovals::create(&artifact_parent)?;
            let mut presenter = Some(presenter);
            approvals.bind_session(prepared.session_id())?;
            let review = async {
                match request.purpose {
                    toolkit_mobkit::private_host::ParentReviewKind::Controller => review_local_controller_parent(&host, prepared, &approvals).await,
                    toolkit_mobkit::private_host::ParentReviewKind::SetupRequest => review_local_setup_request(&host, prepared, &approvals).await,
                }
            };
            tokio::pin!(review);
            let result = loop {
                tokio::select! {
                    biased;
                    result = &mut review => break result,
                    _ = closed.wait_closed() => {
                        drop(setup.take());
                        break retire_presenter_and_join(&approvals, &mut presenter, review).await;
                    }
                    signal = tokio::signal::ctrl_c() => {
                        ended = true;
                        drop(setup.take());
                        let result = retire_presenter_and_join(&approvals, &mut presenter, review).await;
                        signal.map_err(|e| ConnectError::native("listen for operator cancellation", e))?;
                        break result;
                    }
                    prompt = presenter.as_mut().expect("presenter is retained until shutdown").next() => {
                        let Some(prompt) = prompt else {
                            ended = true;
                            drop(setup.take());
                            break retire_presenter_and_join(&approvals, &mut presenter, review).await;
                        };
                        let interrupt = async {
                            tokio::select! {
                                biased;
                                _ = closed.wait_closed() => Ok(()),
                                signal = tokio::signal::ctrl_c() => signal,
                            }
                        };
                        match present_approval_with_review(prompt, &mut terminal.input, &mut terminal.output,
                            interrupt, review.as_mut()).await {
                            ReviewPresentation::Reviewed(result) => break result,
                            ReviewPresentation::Presented(Ok(true)) => {}
                            ReviewPresentation::Presented(Ok(false)) => {
                                ended = true;
                                drop(setup.take());
                                break retire_presenter_and_join(&approvals, &mut presenter, review).await;
                            }
                            ReviewPresentation::Presented(Err(error)) => {
                                ended = true;
                                drop(setup.take());
                                // Preserve retirement failure, but always join the review first.
                                let retired = approvals.retire_checked();
                                drop(presenter.take());
                                let _ = review.await;
                                retired?;
                                break Err(error);
                            }
                        }
                    }
                }
            };
            approvals.retire_checked()?;
            terminal.input.check_cleanup()
                .map_err(|e| ConnectError::native("restore operator terminal", e))?;
            result
        }.await;
        let wire = result.map_err(|_| {
            meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable
        });
        // A lost response leaves the native decision and issuance record with
        // their existing owners. It does not cause another attempt or grant.
        let _ = request.response.send(wire);
        terminal.input.check_cleanup()
            .map_err(|e| ConnectError::native("restore operator terminal", e))?;
        if ended { return Ok(()); }
    }
}

// The setup branch uses the same input and native ApprovalPrompt renderer.
// Account provider work is async and entered cleanup is joined here.
async fn present_setup_prompt(
    setup: &mut SetupPresenters,
    mut enrollment: Enrollment,
    event: SetupEvent,
    terminal: &mut InheritedOperatorTerminal,
    closed: &PrivateHostClosed,
) -> Result<bool, ConnectError> {
    let interrupt = async {
        tokio::select! { biased;
            _ = closed.wait_closed() => Ok(()),
            signal = tokio::signal::ctrl_c() => signal,
        }
    };
    if let SetupEvent::Credentials(request) = event {
        let result = async {
            let panel = enrollment.console().ok_or_else(|| ConnectError::InvalidInput(
                "This credential task has no configured Console presenter".into()))?;
            panel.present_credentials(enrollment.accounts.as_mut().ok_or_else(|| ConnectError::InvalidInput(
                "The selected service owner is unavailable".into()))?, request,
                &mut enrollment.presenter, closed).await
        }.await;
        return finish_account_presentation(setup, enrollment, result);
    }
    if let SetupEvent::Public(request) = event {
        let result = async {
            let panel = enrollment.console().ok_or_else(|| ConnectError::InvalidInput(
                "This public service task has no configured Console presenter".into()))?;
            panel.present_public(enrollment.accounts.as_mut().ok_or_else(|| ConnectError::InvalidInput(
                "The selected service owner is unavailable".into()))?, request,
                &mut enrollment.presenter, closed).await
        }.await;
        return finish_account_presentation(setup, enrollment, result);
    }
    if let SetupEvent::Account(request) = event {
        let result = async {
            if request.console_task().is_some() {
                let panel = enrollment.console().ok_or_else(|| ConnectError::InvalidInput(
                    "This Console task has no configured Console presenter".into()))?;
                panel.present(enrollment.accounts.as_mut().ok_or_else(|| ConnectError::InvalidInput(
                    "The selected account owner is unavailable".into()))?, request,
                    &mut enrollment.presenter, closed).await
            } else {
                // Retain start/callback/completion until native cleanup settles.
                enrollment.accounts.as_mut().ok_or_else(|| ConnectError::InvalidInput(
                    "The selected account owner is unavailable".into()))?
                    .present(request, &mut enrollment.presenter, &mut terminal.input,
                        &mut terminal.output, interrupt).await
            }
        }.await;
        return finish_account_presentation(setup, enrollment, result);
    }
    let SetupEvent::Approval(prompt) = event else { unreachable!("service requests handled above") };
    let result = if prompt.console_task().is_some() {
        match enrollment.console() {
            Some(panel) => panel.present_software(prompt, closed).await,
            None => Ok(ApprovalPromptOutcome::Refused(ConnectError::InvalidInput(
                "This Console task has no configured Console presenter".into()))),
        }
    } else {
        present_approval_outcome(prompt, &mut terminal.input, &mut terminal.output, interrupt).await
    };
    finish_software_presentation(setup, enrollment, result)
}

pub(super) fn finish_software_presentation(
    setup: &mut SetupPresenters,
    enrollment: Enrollment,
    result: Result<ApprovalPromptOutcome, ConnectError>,
) -> Result<bool, ConnectError> {
    match result {
        Ok(ApprovalPromptOutcome::Continue) => { setup.resume(enrollment); Ok(true) }
        Ok(ApprovalPromptOutcome::Refused(error)) => {
            // Only the presentation source can identify a request-local result.
            // A decision may already be durable. Native dispatch/settlement
            // remains owned by its existing runtime and checked-install guard.
            let message = match error {
                ConnectError::Cleanup { .. } => "Setup request unavailable; cleanup failed.",
                _ => "Setup request unavailable.",
            };
            eprintln!("{message}");
            setup.resume(enrollment);
            Ok(true)
        }
        Ok(ApprovalPromptOutcome::Stopped) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn finish_account_presentation(
    setup: &mut SetupPresenters,
    enrollment: Enrollment,
    result: Result<bool, ConnectError>,
) -> Result<bool, ConnectError> {
    match result {
        Err(error) if enrollment.presenter.is_some() => {
            // The exact request has already joined and its response channel
            // yields native unavailable feedback. Keep other eligible work.
            crate::installed_service_review::account_request::report_failure(&error);
            setup.resume(enrollment);
            Ok(true)
        }
        Ok(true) => { setup.resume(enrollment); Ok(true) }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};

    fn access_request(session: meerkat_core::SessionId) -> meerkat_core::ApprovalRequest {
        use meerkat_core::*;
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("test-requester").unwrap(),
            owner: ApprovalOwnerRef::Session { session_id: session },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("application-controller"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "Allow the selected model".into(),
                body: Some("Exact retained controller proposal".into()),
            },
            risk: ApprovalRisk::High,
            request_body: None,
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
            expires_at: Some((std::time::SystemTime::now() + std::time::Duration::from_secs(120)).into()),
            metadata: Default::default(),
            request_provenance: None,
        }
    }

    #[tokio::test]
    async fn terminal_controller_review_expires_while_shown_and_releases_input() {
        use std::{future::{Future, poll_fn}, task::Poll, time::{Duration, SystemTime}};
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory = tempfile::tempdir().unwrap();
            let (approvals, mut presenter) = LocalApprovals::create(directory.path()).unwrap();
            let session = meerkat_core::SessionId::new();
            approvals.bind_session(&session).unwrap();
            let mut request = access_request(session.clone());
            request.expires_at = Some((SystemTime::now() + Duration::from_millis(500)).into());
            let review = approvals.review_access(request);
            tokio::pin!(review);
            assert!(poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx))).await.is_pending());
            let prompt = presenter.next().await.unwrap();
            let (lines, receive_lines) = tokio::sync::mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive_lines);
            let mut output = Vec::new();
            let result = present_approval_with_review(prompt, &mut input, &mut output,
                std::future::pending(), review.as_mut()).await;
            assert!(matches!(result, ReviewPresentation::Reviewed(Err(ConnectError::InvalidInput(ref message)))
                if message == "The access review expired without a returned decision"));
            assert_eq!(String::from_utf8(output).unwrap().matches("Type 'approve ").count(), 1);
            input.check_cleanup().unwrap();
            lines.send(Ok("input after expiry".into())).await.unwrap();
            assert_eq!(input.recv().await.unwrap().unwrap(), "input after expiry");
            approvals.check_session(&session).unwrap();

            let later = approvals.review_access(access_request(session));
            let respond = async {
                let mut prompt = presenter.next().await.unwrap();
                prompt.decide(meerkat_core::ApprovalDecision::Deny).unwrap();
            };
            let (record, ()) = tokio::join!(later, respond);
            assert_eq!(record.unwrap().status, meerkat_core::ApprovalStatus::Denied);
        }).await.expect("displayed terminal controller review did not settle at its actual approval deadline");
    }

    #[tokio::test]
    async fn terminal_controller_review_ready_stop_wins_then_joins_the_same_review() {
        use std::{future::{Future, poll_fn}, task::Poll, time::{Duration, SystemTime}};
        tokio::time::timeout(Duration::from_secs(2), async {
            let directory = tempfile::tempdir().unwrap();
            let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
            let mut presenter = Some(presenter);
            let session = meerkat_core::SessionId::new();
            approvals.bind_session(&session).unwrap();
            let mut request = access_request(session.clone());
            request.expires_at = Some((SystemTime::now() + Duration::from_millis(50)).into());
            let review = approvals.review_access(request);
            tokio::pin!(review);
            assert!(poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx))).await.is_pending());
            let prompt = presenter.as_mut().unwrap().next().await.unwrap();
            let (lines, receive_lines) = tokio::sync::mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive_lines);
            let mut output = Vec::new();
            tokio::time::sleep(Duration::from_millis(60)).await;
            let result = present_approval_with_review(prompt, &mut input, &mut output,
                std::future::ready(Ok(())), review.as_mut()).await;
            assert!(matches!(result, ReviewPresentation::Presented(Ok(false))));
            assert!(retire_presenter_and_join(&approvals, &mut presenter, review).await.is_err());
            assert!(presenter.is_none());
            assert!(approvals.check_session(&session).is_err());
            input.check_cleanup().unwrap();
            lines.send(Ok("input after stop".into())).await.unwrap();
            assert_eq!(input.recv().await.unwrap().unwrap(), "input after stop");
        }).await.expect("terminal stop did not preserve precedence and joined input cleanup");
    }

    #[tokio::test]
    async fn shutdown_drops_queued_native_prompt_before_joining_review() {
        use std::{future::{Future, poll_fn}, task::Poll};
        let directory = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
        let mut presenter = Some(presenter);
        let session = meerkat_core::SessionId::new();
        approvals.bind_session(&session).unwrap();
        let review = approvals.review_access(access_request(session));
        tokio::pin!(review);
        // One poll creates the real native record and queues its prompt, then
        // waits on the one-shot. No presenter.next has consumed the queue.
        assert!(poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx))).await.is_pending());
        // Both parent-close and interrupt production branches use this join.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2),
            retire_presenter_and_join(&approvals, &mut presenter, review)).await;
        assert!(result.expect("shutdown waited for the 120-second decision deadline").is_err());
        assert!(presenter.is_none());
    }

    #[tokio::test]
    async fn shutdown_keeps_work_after_returned_native_decision_joined() {
        use std::{future::{Future, poll_fn}, task::Poll};
        let directory = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
        let mut presenter = Some(presenter);
        let session = meerkat_core::SessionId::new();
        approvals.bind_session(&session).unwrap();
        let review = approvals.review_access(access_request(session));
        tokio::pin!(review);
        assert!(poll_fn(|cx| Poll::Ready(review.as_mut().poll(cx))).await.is_pending());
        let mut prompt = tokio::time::timeout(std::time::Duration::from_secs(2),
            presenter.as_mut().unwrap().next()).await.unwrap().unwrap();
        prompt.decide(meerkat_core::ApprovalDecision::Approve).unwrap();
        let decision = tokio::time::timeout(std::time::Duration::from_secs(2), review).await.unwrap().unwrap();
        assert_eq!(decision.status, meerkat_core::ApprovalStatus::Approved);
        // An inert post-decision continuation controls settlement. This proves
        // join ownership, not a native grant or physical effect.
        let (release, wait) = tokio::sync::oneshot::channel();
        let work = async { wait.await.unwrap(); Ok::<_, ConnectError>(decision) };
        let joined = retire_presenter_and_join(&approvals, &mut presenter, work);
        tokio::pin!(joined);
        let pending = poll_fn(|cx| Poll::Ready(joined.as_mut().poll(cx))).await.is_pending();
        let released = release.send(()).is_ok();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), joined).await;
        assert!(pending);
        assert!(released);
        assert_eq!(result.unwrap().unwrap().status, meerkat_core::ApprovalStatus::Approved);
    }

    #[test]
    fn operator_fd_cannot_consume_rpc_or_private_channel() {
        let first = tempfile::tempfile().unwrap();
        let second = tempfile::tempfile().unwrap();
        for fd in [0, 1, 2, first.as_raw_fd(), second.as_raw_fd()] {
            assert!(InheritedOperatorTerminal::adopt(fd, first.as_raw_fd(), second.as_raw_fd()).is_err());
        }
        // Collision refusal must not close another channel's descriptor.
        assert!(first.metadata().is_ok());
        assert!(second.metadata().is_ok());
    }

    #[test]
    fn nonterminal_is_refused_before_prompting() {
        let file = tempfile::tempfile().unwrap();
        let fd = file.into_raw_fd();
        assert!(InheritedOperatorTerminal::adopt(fd, fd + 1, fd + 2).is_err());
    }

    #[test]
    fn owned_terminal_is_retained_close_on_exec_without_a_second_input_owner() {
        let pty = nix::pty::openpty(None, None).unwrap();
        assert_eq!(nix::sys::termios::tcgetsid(&pty.slave), Err(nix::errno::Errno::ENOTTY));
        let fd = pty.slave.into_raw_fd();
        let terminal = InheritedOperatorTerminal::adopt(fd, fd + 1, fd + 2).unwrap();
        terminal.input.require_terminal().unwrap();
        assert_eq!(nix::sys::termios::tcgetsid(&terminal.output), Err(nix::errno::Errno::ENOTTY));
        let flags = nix::fcntl::fcntl(&terminal.output, nix::fcntl::FcntlArg::F_GETFD).unwrap();
        assert!(nix::fcntl::FdFlag::from_bits_truncate(flags).contains(nix::fcntl::FdFlag::FD_CLOEXEC));
        drop(terminal);
        drop(pty.master);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn adopting_terminal_does_not_acquire_a_controlling_terminal() {
        const CHILD: &str = "MEERKAT_CONNECT_TERMINAL_SESSION_TEST";
        if std::env::var_os(CHILD).as_deref() != Some(std::ffi::OsStr::new("child")) {
            // Run the real adoption path in a fresh session, without changing
            // the test runner's terminal or suppressing the SIGHUP disposition.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "application_gateway::tests::adopting_terminal_does_not_acquire_a_controlling_terminal",
                    "--exact", "--test-threads=1", "--nocapture",
                ])
                .env(CHILD, "child")
                .output()
                .unwrap();
            assert!(output.status.success(), "terminal child {:?}: {}{}",
                output.status, String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr));
            return;
        }
        let session = nix::unistd::setsid().unwrap();
        assert_eq!(session, nix::unistd::getpid());
        assert_eq!(nix::unistd::getsid(None).unwrap(), session);
        owned_terminal_is_retained_close_on_exec_without_a_second_input_owner();
    }
}

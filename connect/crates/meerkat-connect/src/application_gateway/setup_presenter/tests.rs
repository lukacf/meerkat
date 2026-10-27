use super::*;
use meerkat_core::*;
use std::{future::{Future, poll_fn}, task::Poll, time::{Duration, SystemTime}};

fn request(session: SessionId) -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("selected-application").unwrap(),
        owner: ApprovalOwnerRef::Session { session_id: session },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-account-access"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review the exact selected account access".into(),
            body: Some(serde_json::json!({"selection": {"account": "test-account", "operations": ["read"]}})),
        },
        risk: ApprovalRisk::High,
        request_body: Some(serde_json::json!({"application_selection": "exact-private-value"})),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
        expires_at: Some((SystemTime::now() + Duration::from_secs(120)).into()),
        metadata: Default::default(),
        request_provenance: None,
    }
}

async fn pending<F: Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    assert!(poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await.is_pending());
}

#[tokio::test]
async fn enrollment_preserves_the_exact_native_request_and_both_decisions() {
    for decision in [ApprovalDecision::Approve, ApprovalDecision::Deny] {
        let directory = tempfile::tempdir().unwrap();
        let (factory, reviews) = channel(directory.path().to_owned());
        let owner = factory.create().unwrap();
        let session = SessionId::new();
        owner.bind_session(&session).unwrap();
        let expected = request(session);
        let work = owner.review_access(expected.clone());
        tokio::pin!(work);
        pending(work.as_mut()).await;
        let mut presenters = SetupPresenters::new(reviews);
        let (enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap() else { panic!("unexpected account request"); };
        assert_eq!(prompt.record().owner, expected.owner);
        assert_eq!(prompt.record().proposed_action, expected.proposed_action);
        assert_eq!(prompt.record().request_body, expected.request_body);
        prompt.decide(decision).unwrap();
        let record = work.await.unwrap();
        assert_eq!(record.status, match decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Deny => ApprovalStatus::Denied,
            _ => unreachable!(),
        });
        presenters.resume(enrollment);
        assert!(owner.check_session(match &record.owner {
            ApprovalOwnerRef::Session { session_id } => session_id,
            _ => unreachable!(),
        }).is_ok());
    }
}

#[tokio::test]
async fn idle_retirement_wakes_and_releases_the_existing_presenter() {
    let directory = tempfile::tempdir().unwrap();
    let (factory, reviews) = channel(directory.path().to_owned());
    let owner = factory.create().unwrap();
    let mut presenters = SetupPresenters::new(reviews);
    let next = presenters.next();
    tokio::pin!(next);
    pending(next.as_mut()).await;
    owner.retire();
    drop(factory);
    assert!(tokio::time::timeout(Duration::from_secs(2), next).await
        .expect("retired idle presenter did not wake").is_none());
}

#[tokio::test]
async fn same_session_replacement_gets_a_fresh_owner_without_retiring_the_old_one() {
    let directory = tempfile::tempdir().unwrap();
    let (factory, reviews) = channel(directory.path().to_owned());
    let first = factory.create().unwrap();
    let session = SessionId::new();
    first.bind_session(&session).unwrap();
    let mut presenters = SetupPresenters::new(reviews);
    {
        let next = presenters.next();
        tokio::pin!(next);
        pending(next.as_mut()).await;
    }
    let second = factory.create().unwrap();
    second.bind_session(&session).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_ne!(first.store_path(), second.store_path());
    assert!(first.check_session(&session).is_ok());
    first.retire();
    assert!(second.check_session(&session).is_ok());
    let work = second.review_access(request(session));
    tokio::pin!(work);
    pending(work.as_mut()).await;
    let (_enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap() else { panic!("unexpected account request"); };
    prompt.decide(ApprovalDecision::Approve).unwrap();
    assert_eq!(work.await.unwrap().status, ApprovalStatus::Approved);
}

#[tokio::test]
async fn close_retires_waiting_and_queued_owners_without_a_synthetic_denial() {
    let directory = tempfile::tempdir().unwrap();
    let (factory, reviews) = channel(directory.path().to_owned());
    let first = factory.create().unwrap();
    let first_session = SessionId::new();
    first.bind_session(&first_session).unwrap();
    let work = first.review_access(request(first_session.clone()));
    tokio::pin!(work);
    pending(work.as_mut()).await;
    let mut presenters = SetupPresenters::new(reviews);
    let (enrollment, SetupEvent::Approval(prompt)) = presenters.next().await.unwrap() else { panic!("unexpected account request"); };
    let approval_id = prompt.record().approval_id.clone();
    presenters.resume(enrollment);
    let second = factory.create().unwrap();
    let second_session = SessionId::new();
    second.bind_session(&second_session).unwrap();
    drop(presenters);
    drop(prompt);
    assert!(work.await.is_err());
    assert!(first.check_session(&first_session).is_err());
    assert!(second.check_session(&second_session).is_err());
    assert!(factory.create().is_err());
    let store = meerkat_store::FileApprovalStore::open(first.store_path()).unwrap();
    let service = ApprovalService::with_store(Arc::new(store)).unwrap();
    assert_eq!(service.get(&approval_id).unwrap().status, ApprovalStatus::Pending);
}

#[tokio::test]
async fn presentation_close_does_not_replace_a_returned_decision_or_own_its_effect_future() {
    let directory = tempfile::tempdir().unwrap();
    let (factory, reviews) = channel(directory.path().to_owned());
    let owner = factory.create().unwrap();
    let session = SessionId::new();
    owner.bind_session(&session).unwrap();
    let work = owner.review_access(request(session));
    tokio::pin!(work);
    pending(work.as_mut()).await;
    let mut presenters = SetupPresenters::new(reviews);
    let (enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap() else { panic!("unexpected account request"); };
    prompt.decide(ApprovalDecision::Approve).unwrap();
    let record = work.await.unwrap();
    presenters.resume(enrollment);
    let (release, wait) = tokio::sync::oneshot::channel();
    // Controlled post-decision work proves only custody separation. It is not
    // a native dispatch or account-completion oracle.
    let entered = async { wait.await.unwrap(); record };
    tokio::pin!(entered);
    pending(entered.as_mut()).await;
    drop(presenters);
    pending(entered.as_mut()).await;
    release.send(()).unwrap();
    assert_eq!(entered.await.status, ApprovalStatus::Approved);
}

#[tokio::test]
async fn full_or_closed_enrollment_refuses_without_consuming_an_existing_owner() {
    let directory = tempfile::tempdir().unwrap();
    let (factory, reviews) = channel(directory.path().to_owned());
    let first = factory.create().unwrap();
    let session = SessionId::new();
    first.bind_session(&session).unwrap();
    assert!(factory.create().is_err());
    assert!(first.check_session(&session).is_ok());
    drop(reviews);
    assert!(factory.create().is_err());
    assert!(first.check_session(&session).is_err());
}

#[tokio::test]
async fn account_enrollment_uses_the_same_native_review_and_refuses_missing_dispatch_authority() {
    use meerkat_auth_core::auth_store::{EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence};
    let directory = tempfile::tempdir().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let facade = meerkat::HostAuthService::new(ProviderAuthPersistence::new(
        Arc::new(EphemeralTokenStore::new()), Arc::new(InMemoryCoordinator::new())),
        machine.provider_auth_runtime_authority());
    let realm = RealmLocator { state_root: directory.path().join("realm"), realm: RealmId::parse("work").unwrap() };
    let session = SessionId::new();
    let (factory, mut reviews) = channel(directory.path().to_owned());
    let mut retained = None;
    let dispatcher = factory.create_installed(|approvals| {
        approvals.bind_session(&session)?;
        retained = Some(approvals.clone());
        crate::installed_service_review::compose_installed_service_review(
            Arc::new(meerkat::McpRouterAdapter::new(meerkat::McpRouter::new())), facade,
            realm.clone(), session.clone(), approvals)
    }).unwrap();
    // Empty native router and actual enrollment only. No server, auth command,
    // input or native tool authorization is executed by this control.
    let enrollment = reviews.0.recv().await.unwrap();
    assert!(enrollment.accounts.is_some());
    let approvals = retained.unwrap();
    assert!(approvals.check_session(&session).is_ok());
    let names = dispatcher.tools();
    assert!(names.iter().any(|tool| tool.name == meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST));
    assert!(names.iter().any(|tool| tool.name == meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST));
    assert!(!names.iter().any(|tool| tool.name == toolkit_mobkit::FIND_AND_INSTALL));
    // Observe the actual registered leaf through its native composite, not a
    // helper predicate. Required-review admission depends on this declaration.
    use meerkat_core::approval::review::ReviewEntrySupport;
    assert_eq!(dispatcher.review_entry_support(meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST),
        ReviewEntrySupport::ConsumesAtEntry);
    assert_eq!(dispatcher.review_entry_support(meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST),
        ReviewEntrySupport::ConsumesAtEntry);
    assert_eq!(dispatcher.review_entry_support("unregistered-account-request"),
        ReviewEntrySupport::Unsupported);
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView { id: "request", name: meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST, args: &args };
    assert!(dispatcher.dispatch_with_context(call, &ToolDispatchContext::default()).await.is_err());
    let public = ToolCallView { id: "public-request", name: meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST, args: &args };
    assert!(dispatcher.dispatch_with_context(public, &ToolDispatchContext::default()).await.is_err());
    drop(enrollment);
    assert!(approvals.check_session(&session).is_err());
}


fn checked_installation_helper() -> crate::setup::PreparedSetupHelper {
    crate::setup::PreparedSetupConnection::from_preparer_output(
        br#"{"mcpServers":{"toolkit-setup":{"command":"/usr/bin/python3","args":["-I","-B","/retained/adapter.py"]}}}"#,
    ).unwrap().for_mobkit_helper(
        MobMemberBinding { mob_id: "application".into(), role: "setup".into(), member: "helper".into() },
        crate::setup::SetupHelperTools::CheckedInstallation,
    ).unwrap()
}

#[tokio::test]
async fn native_terminal_composer_enrolls_fresh_accounts_for_each_same_session_construction() {
    use meerkat::McpAdapterComposer;
    use meerkat_auth_core::auth_store::{EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence};
    let directory = tempfile::tempdir().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let facade = meerkat::HostAuthService::new(ProviderAuthPersistence::new(
        Arc::new(EphemeralTokenStore::new()), Arc::new(InMemoryCoordinator::new())),
        machine.provider_auth_runtime_authority());
    let realm = RealmLocator { state_root: directory.path().join("realm"), realm: RealmId::parse("work").unwrap() };
    let helper = checked_installation_helper();
    let selected = helper.member().clone();
    let (presenters, mut reviews) = channel(directory.path().to_owned());
    let composer = crate::installed_service_review::composer::terminal_composer(
        selected.clone(), facade, realm, presenters).unwrap();
    let session = SessionId::new();
    let adapter = Arc::new(meerkat::McpRouterAdapter::new(meerkat::McpRouter::new()));
    let first = composer.compose(&adapter, &session, Some(&selected)).unwrap();
    let first_owner = reviews.0.recv().await.unwrap();
    assert!(first_owner.accounts.is_some());
    let second = composer.compose(&adapter, &session, Some(&selected)).unwrap();
    let second_owner = reviews.0.recv().await.unwrap();
    assert!(second_owner.accounts.is_some());
    assert!(!Arc::ptr_eq(&first, &second));
    for dispatcher in [&first, &second] {
        assert!(dispatcher.tools().iter().any(|tool| tool.name == meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST));
        assert!(!dispatcher.tools().iter().any(|tool| tool.name == toolkit_mobkit::FIND_AND_INSTALL));
    }
    // Each callback went through bind_session, which refuses owner reuse. The
    // first enrollment remains retained while the same-session second succeeds.
    drop(first_owner);
    assert!(second_owner.presenter.is_some());
    drop(second_owner);
    drop(reviews);
    assert!(composer.compose(&adapter, &session, Some(&selected)).is_err());
    // This invokes the actual composer and private channel on an empty router,
    // not a gateway init, native actor rebuild or admitted account request.
}

#[tokio::test]
async fn native_terminal_composer_keeps_other_members_and_refuses_selected_alias_substitution() {
    use meerkat::McpAdapterComposer;
    use meerkat_auth_core::auth_store::{EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence};
    let directory = tempfile::tempdir().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let facade = meerkat::HostAuthService::new(ProviderAuthPersistence::new(
        Arc::new(EphemeralTokenStore::new()), Arc::new(InMemoryCoordinator::new())),
        machine.provider_auth_runtime_authority());
    let realm = RealmLocator { state_root: directory.path().join("realm"), realm: RealmId::parse("work").unwrap() };
    let helper = checked_installation_helper();
    let selected = helper.member().clone();
    let (presenters, mut reviews) = channel(directory.path().to_owned());
    let composer = crate::installed_service_review::composer::terminal_composer(
        selected.clone(), facade, realm, presenters).unwrap();
    let adapter = Arc::new(meerkat::McpRouterAdapter::new(meerkat::McpRouter::new()));
    let raw: Arc<dyn AgentToolDispatcher> = adapter.clone();
    let session = SessionId::new();
    let other = MobMemberBinding { member: "researcher".into(), ..selected.clone() };
    for member in [None, Some(&other)] {
        let actual = composer.compose(&adapter, &session, member).unwrap();
        assert!(Arc::ptr_eq(&actual, &raw));
    }
    let substituted = MobMemberBinding { role: "other".into(), ..selected.clone() };
    assert!(composer.compose(&adapter, &session, Some(&substituted)).is_err());
    assert!(matches!(reviews.0.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
    assert!(composer.compose(&adapter, &session, Some(&selected)).is_ok());
    assert!(reviews.0.recv().await.unwrap().accounts.is_some());
}

#[tokio::test]
async fn account_failure_resumes_the_same_enrollment_and_later_review() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let directory = tempfile::tempdir().unwrap();
        let (factory, reviews) = channel(directory.path().to_owned());
        let owner = factory.create().unwrap();
        let session = SessionId::new();
        owner.bind_session(&session).unwrap();
        let first = owner.review_access(request(session.clone()));
        tokio::pin!(first);
        pending(first.as_mut()).await;
        let mut presenters = SetupPresenters::new(reviews);
        let (enrollment, SetupEvent::Approval(prompt)) = presenters.next().await.unwrap()
            else { panic!("approval expected") };
        drop(prompt);
        assert!(first.await.is_err());
        // Exercise the actual operator error boundary. This supplied error is
        // not a claim that a native account status request was executed.
        let failure = ConnectError::provider_setup("Observe selected connector account",
            std::io::Error::other("private-provider-canary"));
        assert!(super::super::finish_account_presentation(&mut presenters, enrollment, Err(failure)).unwrap());
        owner.check_session(&session).unwrap();
        let later = owner.review_access(request(session.clone()));
        let answer = async {
            let (enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap()
                else { panic!("later approval expected") };
            prompt.decide(ApprovalDecision::Approve).unwrap();
            presenters.resume(enrollment);
        };
        let (result, ()) = tokio::join!(later, answer);
        assert_eq!(result.unwrap().status, ApprovalStatus::Approved);
        owner.check_session(&session).unwrap();
    }).await.expect("one account failure retired the existing operator enrollment");
}


// Reuse the real private-pipe lifetime fixture from
// service_install_tests::console_lifetime. Component opens refuse; this supplies
// only an actual connection's close observer, not native operation authority.
pub(crate) async fn presentation_pipe() -> (
    toolkit_mobkit::private_host::PrivateHostConnection,
    tokio::fs::File,
    tokio::io::BufReader<tokio::fs::File>,
) {
    use meerkat_toolkit::host_lifetime::control::*;
    use std::{fs::File, os::fd::IntoRawFd};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use toolkit_mobkit::private_host::{InheritedHostPipes, PrivateHostConnection};

    struct PipeOwner(HostCloseHandle);
    impl PipeOwner {
        fn observation(&self) -> HostCloseObservation {
            HostCloseObservation { handle: self.0.clone(), state: HostCloseState::Settled, failure: None }
        }
    }
    #[async_trait::async_trait]
    impl PrivateHostLifetimeControl for PipeOwner {
        fn validate_current_selection(&self) -> Result<(), HostLifetimeError> { Ok(()) }
        fn open_component(&self, _: &toolkit_package::Id) -> Result<HostComponentObservation, HostLifetimeError> { Err(HostLifetimeError::Denied) }
        fn begin_close_component(&self, _: &toolkit_package::Id, _: HostCloseReason) -> Result<HostCloseObservation, HostLifetimeError> { Err(HostLifetimeError::Denied) }
        fn begin_close_activation(&self, _: HostCloseReason) -> Result<HostCloseObservation, HostLifetimeError> { Ok(self.observation()) }
        fn observe_close(&self, handle: &HostCloseHandle) -> Result<HostCloseObservation, HostLifetimeError> {
            if handle != &self.0 { return Err(HostLifetimeError::WrongOwner); } Ok(self.observation())
        }
        async fn join_close(&self, handle: &HostCloseHandle) -> Result<HostCloseObservation, HostLifetimeError> { self.observe_close(handle) }
        fn host_lost(&self) -> Result<HostCloseObservation, HostLifetimeError> { Ok(self.observation()) }
    }
    let (child_read, parent_write) = nix::unistd::pipe().unwrap();
    let (parent_read, child_write) = nix::unistd::pipe().unwrap();
    for fd in [&child_read, &parent_write, &parent_read, &child_write] {
        nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC)).unwrap();
    }
    let pipes = InheritedHostPipes::adopt(child_read.into_raw_fd(), child_write.into_raw_fd()).unwrap();
    let mut commands = tokio::fs::File::from_std(File::from(parent_write));
    let mut responses = BufReader::new(tokio::fs::File::from_std(File::from(parent_read)));
    let artifact = serde_json::json!({"path":"/fixture/transport-only.json","sha256":"0".repeat(64)});
    let mut bootstrap = serde_json::to_vec(&serde_json::json!({"version":1,"id":1,
        "command":{"kind":"bootstrap", "selection":{"deployment_root":"/fixture",
            "activation":artifact, "policy_manifest":artifact, "local_policy":artifact}}})).unwrap();
    bootstrap.push(b'\n');
    commands.write_all(&bootstrap).await.unwrap();
    commands.flush().await.unwrap();
    let (_, connection) = PrivateHostConnection::bootstrap(pipes, |_| Ok(PipeOwner(serde_json::from_value(serde_json::json!({
        "incarnation":"00000000-0000-4000-8000-000000000001",
        "close_id":"00000000-0000-4000-8000-000000000002", "scope":{"kind":"activation"}
    })).unwrap()))).await.unwrap();
    let mut line = String::new();
    assert!(responses.read_line(&mut line).await.unwrap() > 0);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&line).unwrap()["result"]["kind"], "bootstrap_accepted");
    (connection, commands, responses)
}

#[tokio::test]
async fn software_native_decision_refusal_keeps_the_enrollment_and_later_review() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (connection, commands, responses) = presentation_pipe().await;
        let closed = connection.closed_observer();
        assert!(!closed.is_closed_for_presenter());
        let directory = tempfile::tempdir().unwrap();
        let (factory, reviews) = channel(directory.path().to_owned());
        let owner = factory.create().unwrap();
        let session = SessionId::new();
        owner.bind_session(&session).unwrap();
        let first = owner.review_access(request(session.clone()));
        tokio::pin!(first);
        pending(first.as_mut()).await;
        let mut presenters = SetupPresenters::new(reviews);
        let (enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap()
            else { panic!("unexpected account request"); };
        let first_id = prompt.record().approval_id.clone();
        // Retain a real native decision, then present its now-obsolete prompt.
        // The second decision must refuse without rewriting the first record.
        prompt.decide(ApprovalDecision::Approve).unwrap();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        send.send(Ok(format!("deny {first_id}"))).await.unwrap();
        let mut terminal = super::super::InheritedOperatorTerminal {
            input: crate::terminal::TerminalInput::scripted(receive),
            output: tempfile::tempfile().unwrap(),
        };
        // Exercise the original caller, including its enrollment disposition.
        // The former public renderer route returned Err and dropped this owner.
        assert!(super::super::present_setup_prompt(&mut presenters, enrollment,
            SetupEvent::Approval(prompt), &mut terminal, &closed).await.unwrap());
        assert_eq!(first.await.unwrap().status, ApprovalStatus::Approved);
        owner.check_session(&session).unwrap();
        // This is the actual same ApprovalService and enrollment, not a policy
        // check or an invented native setup_account_request admission.
        let later = owner.review_access(request(session));
        tokio::pin!(later);
        pending(later.as_mut()).await;
        let (_enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap()
            else { panic!("unexpected account request"); };
        assert_ne!(prompt.record().approval_id, first_id);
        prompt.decide(ApprovalDecision::Approve).unwrap();
        assert_eq!(later.await.unwrap().status, ApprovalStatus::Approved);
        let service = ApprovalService::with_store(Arc::new(
            meerkat_store::FileApprovalStore::open(owner.store_path()).unwrap())).unwrap();
        assert_eq!(service.get(&first_id).unwrap().status, ApprovalStatus::Approved);
        assert!(!closed.is_closed_for_presenter());
        connection.finish().await.unwrap();
        closed.wait_closed().await;
        drop((commands, responses));
    }).await.expect("software request-local refusal stranded the same enrollment");
}

#[tokio::test]
async fn software_request_unavailable_keeps_a_later_real_review() {
    use crate::approval::ApprovalPromptOutcome;
    tokio::time::timeout(Duration::from_secs(2), async {
        let directory = tempfile::tempdir().unwrap();
        let (factory, reviews) = channel(directory.path().to_owned());
        let owner = factory.create().unwrap();
        let session = SessionId::new();
        owner.bind_session(&session).unwrap();
        let first = owner.review_access(request(session.clone()));
        tokio::pin!(first);
        pending(first.as_mut()).await;
        let mut presenters = SetupPresenters::new(reviews);
        let (enrollment, SetupEvent::Approval(prompt)) = presenters.next().await.unwrap()
            else { panic!("unexpected account request"); };
        let id = prompt.record().approval_id.clone();
        drop(prompt);
        assert!(first.await.is_err());
        // The boundary receives a typed request-local transport failure. This
        // does not claim that a live Console viewer or native task was built.
        let local = Ok(ApprovalPromptOutcome::Refused(ConnectError::InvalidInput(
            "Controlled request-local transport loss".into())));
        assert!(super::super::finish_software_presentation(&mut presenters, enrollment, local).unwrap());
        let later = owner.review_access(request(session.clone()));
        tokio::pin!(later);
        pending(later.as_mut()).await;
        let (_enrollment, SetupEvent::Approval(mut prompt)) = presenters.next().await.unwrap()
            else { panic!("unexpected account request"); };
        prompt.decide(ApprovalDecision::Approve).unwrap();
        assert_eq!(later.await.unwrap().status, ApprovalStatus::Approved);
        owner.check_session(&session).unwrap();
        let service = ApprovalService::with_store(Arc::new(
            meerkat_store::FileApprovalStore::open(owner.store_path()).unwrap())).unwrap();
        assert_eq!(service.get(&id).unwrap().status, ApprovalStatus::Pending);
    }).await.expect("software unavailable response stranded later work");
}

#[tokio::test]
async fn software_owner_stop_and_typed_cleanup_failure_do_not_resume_enrollment() {
    use crate::approval::ApprovalPromptOutcome;
    for cleanup_failed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (factory, reviews) = channel(directory.path().to_owned());
        let owner = factory.create().unwrap();
        let session = SessionId::new();
        owner.bind_session(&session).unwrap();
        let work = owner.review_access(request(session.clone()));
        tokio::pin!(work);
        pending(work.as_mut()).await;
        let mut presenters = SetupPresenters::new(reviews);
        let (enrollment, SetupEvent::Approval(prompt)) = presenters.next().await.unwrap()
            else { panic!("unexpected account request"); };
        drop(prompt);
        let outcome = if cleanup_failed {
            Err(ConnectError::Cleanup {
                primary: Box::new(ConnectError::InvalidInput("Controlled terminal failure".into())),
                cleanup: Box::new(ConnectError::InvalidInput("Controlled cleanup failure".into())),
            })
        } else { Ok(ApprovalPromptOutcome::Stopped) };
        let result = super::super::finish_software_presentation(&mut presenters, enrollment, outcome);
        if cleanup_failed { assert!(matches!(result, Err(ConnectError::Cleanup { .. }))); }
        else { assert!(!result.unwrap()); }
        assert!(owner.check_session(&session).is_err());
        assert!(work.await.is_err());
    }
}

#[path = "tests/request_tool_native.rs"]
mod request_tool_native;

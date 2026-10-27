//! Actual A/B source, grant, SQLite and native Apply preparation. Only the
//! accepted attachment reply/observer is scripted: this isolates Toolkit
//! custody/routing and does not claim a physical member MCP connection.
use super::*;
use super::super::admission_tests::{acquisition, approve_b_operations, reviewed_config_operations};
use futures::FutureExt;
use meerkat_core::{mcp_attachment::{McpAttachmentAttempt, McpAttachmentAttemptObserver,
    McpAttachmentOutcome, McpAttachmentRefusal}, turn_execution_authority::TurnTerminalOutcome};
use std::{panic::AssertUnwindSafe, sync::atomic::{AtomicUsize, Ordering}};

const WAIT: Duration = Duration::from_secs(10);
struct Observer {
    value: Mutex<McpAttachmentReadiness>,
    operations: Mutex<Vec<String>>,
    held: Option<(tokio::sync::Notify, tokio::sync::Notify)>,
}
#[async_trait::async_trait]
impl McpAttachmentAttemptObserver for Observer {
    async fn observe(&self, raw: &str) -> McpAttachmentReadiness {
        self.operations.lock().unwrap().push(raw.to_owned());
        self.value.lock().unwrap().clone()
    }
    async fn wait(&self, raw: &str, _: Duration) -> McpAttachmentReadiness {
        if let Some((entered, release)) = &self.held {
            entered.notify_one();
            release.notified().await;
        }
        self.observe(raw).await
    }
}
fn receipt(observer: Arc<Observer>) -> LiveMcpAttachmentReceipt {
    LiveMcpAttachmentReceipt { outcome: McpAttachmentOutcome { persisted: false, staged: true },
        attempt: McpAttachmentAttempt::new("mail", observer) }
}
fn observer(value: McpAttachmentReadiness) -> Arc<Observer> {
    Arc::new(Observer { value: Mutex::new(value), operations: Mutex::new(vec![]), held: None })
}
async fn approved(fixture: &ContinuationFixture) -> (PrivateOriginalRef, StoredRecord,
    Arc<McpAttachmentControlRequest>)
{
    approved_operations(fixture, &["read"]).await
}
async fn approved_operations(fixture: &ContinuationFixture, names: &[&str])
    -> (PrivateOriginalRef, StoredRecord, Arc<McpAttachmentControlRequest>)
{
    let a = fixture.submit().await;
    fixture.finish(&a).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let run = dispatch.retained_work().unwrap().run_id().clone();
    tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session, &run)).await.unwrap();
    assert_eq!(fixture.machine.run_turn_terminal(&fixture.session, &run).await,
        Some(TurnTerminalOutcome::Completed));
    let snapshot = acquisition(fixture, &dispatch).await;
    let operations = names.iter().map(|name| Id::new(*name).unwrap()).collect();
    let b = approve_b_operations(fixture, snapshot, operations).await;
    let stored = fixture.store.get(&fixture.source.key(&b).unwrap()).await.unwrap().unwrap();
    // Production reserves B before attachment preparation captures publication.
    // Keep that same order when substituting only the native reply/observer.
    fixture.source.reserve_service_continuation(&stored).await.unwrap();
    let control = fixture.source.prepare_service_mcp_control(&fixture.store, &b,
        reviewed_config_operations(&fixture.scope.realm,
            &names.iter().map(|name| Id::new(*name).unwrap()).collect())).await.unwrap();
    let registration = fixture.machine.current_session_registration_witness(&fixture.session).await.unwrap();
    let parent = fixture.binding().await.unwrap().unwrap();
    let authority = fixture.machine.prepare_mcp_attachment(&registration,
        &parent.candidate().target, control.clone()).await.unwrap();
    assert_eq!(authority.authorize_apply(), Ok(()), "actual native Apply before testing its reply custody");
    (b, stored, control)
}
fn keep(
    source: &PrivateOriginals, b: &PrivateOriginalRef, stored: &StoredRecord,
    control: Arc<McpAttachmentControlRequest>, future: NativeCall,
) -> Arc<HeldServiceAttachment> {
    keep_with_login(source, b, stored, control, future, None)
}
fn keep_with_login(
    source: &PrivateOriginals, b: &PrivateOriginalRef, stored: &StoredRecord,
    control: Arc<McpAttachmentControlRequest>, future: NativeCall,
    login: Option<ConsoleLoginHandle>,
) -> Arc<HeldServiceAttachment> {
    let task = source.decode_record(stored, b).unwrap().approved_task.unwrap();
    let operations = source.current_service_mcp_control(&control,
        &control.effect_binding(McpAttachmentEffect::Apply), source.owner.clock().now().unwrap().unix_ms, false)
        .unwrap().operations;
    let held = Arc::new(HeldServiceAttachment::new(b.clone(), task.attachments()[0].clone(), control, operations, future, login));
    source.attachment_parent(&task.attachments()[0]).unwrap().service_attachments.lock().unwrap().push(held.clone());
    held
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_recovery_retains_exact_pending_or_refused_attempt_without_binding_b() {
    for readiness in [McpAttachmentReadiness::Pending, McpAttachmentReadiness::Superseded,
        McpAttachmentReadiness::NotCurrent, McpAttachmentReadiness::Retired,
        McpAttachmentReadiness::ToolNotRouted { connection_generation: 7 }]
    {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (b, stored, control) = approved(&fixture).await;
            let requests = fixture.model_requests();
            let calls = Arc::new(AtomicUsize::new(0));
            let entered = calls.clone();
            let observed = observer(readiness);
            let answer = receipt(observed.clone());
            let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
                entered.fetch_add(1, Ordering::SeqCst); Ok(answer)
            }));
            for _ in 0..2 {
                // The actual production entry, with the exact installed store.
                fixture.source.step_service_input_with_attachment(&fixture.store, &stored).await.unwrap();
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1, "accepted work never reattaches");
            assert_eq!(*observed.operations.lock().unwrap(), vec!["read", "read"]);
            assert!(Arc::ptr_eq(&held, &fixture.source.held_service_attachment(&b, &held.task).unwrap().unwrap()));
            let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
            assert_eq!(saved, stored, "pending/refused attachment cannot bind B or publish admission");
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), requests);
            fixture.source.join_service_attachments().await;
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_attachment_reply_survives_waiter_loss_and_cleanup_never_starts_unpolled_work() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let (send, receive) = tokio::sync::oneshot::channel();
        let started = calls.clone();
        let signal = entered.clone();
        let held = keep(&fixture.source, &b, &stored, control.clone(), Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst); signal.notify_one(); receive.await.unwrap()
        }));
        let mut waiter = Box::pin(held.receipt(|| fixture.source.attachment_current(&held)));
        tokio::select! {
            result = &mut waiter => panic!("native reply is held: {result:?}"),
            _ = entered.notified() => (),
        }
        drop(waiter); // The same future/oneshot remains in A's keeper.
        send.send(Ok(receipt(observer(McpAttachmentReadiness::Pending)))).unwrap();
        held.join_if_polled().await;
        assert!(held.receipt(|| panic!("an entered call cannot start again")).await.unwrap().is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        fixture.source.remove_service_attachment(&held);

        let started = calls.clone();
        let unpolled = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst);
            Err(LiveMcpAttachError::Refused(McpAttachmentRefusal::Denied))
        }));
        fixture.source.join_service_attachments().await;
        assert!(matches!(unpolled.receipt(|| Ok(())).await, Err(PrivateOriginalError::Denied)));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "cleanup did not first-poll the second call");
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_recovery_uses_ready_reply_once_and_never_reattaches_admitted_b() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let started = calls.clone();
        let observed = observer(McpAttachmentReadiness::Usable { connection_generation: 7 });
        let answer = receipt(observed.clone());
        let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst); Ok(answer)
        }));
        fixture.source.step_service_input_with_attachment(&fixture.store, &stored).await.unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_record(&saved, &b).unwrap();
        assert!(record.final_association.is_some() && record.service_admission.is_some());
        assert_eq!(fixture.inputs().await, Ok(2));
        assert_eq!(*observed.operations.lock().unwrap(), vec!["read", "read"]);
        assert!(fixture.source.held_service_attachment(&b, &held.task).unwrap().is_none());
        fixture.source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(2));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_readiness_cannot_publish_b_after_canonical_task_cancellation() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let observed = Arc::new(Observer { value: Mutex::new(McpAttachmentReadiness::Usable { connection_generation: 7 }),
            operations: Mutex::new(vec![]), held: Some((tokio::sync::Notify::new(), tokio::sync::Notify::new())) });
        let answer = receipt(observed.clone());
        let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move { Ok(answer) }));
        let mut work = Box::pin(fixture.source.step_service_input_with_attachment(&fixture.store, &stored));
        tokio::select! {
            result = &mut work => panic!("readiness should be held: {result:?}"),
            _ = observed.held.as_ref().unwrap().0.notified() => (),
        }
        fixture.source.record_task_group_cancelled(&b).await.unwrap();
        observed.held.as_ref().unwrap().1.notify_one();
        assert!(matches!(tokio::time::timeout(WAIT, work).await.unwrap(),
            Err(PrivateOriginalError::Denied | PrivateOriginalError::Unavailable)));
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_record(&saved, &b).unwrap();
        assert!(record.operation_cancelled);
        assert!(record.final_association.is_none() && record.service_admission.is_none());
        assert!(record.bound_service_input.is_none(), "stale readiness cannot retain success context");
        assert_eq!(fixture.inputs().await, Ok(1));
        fixture.source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
        assert!(fixture.source.held_service_attachment(&b, &held.task).unwrap().is_none());
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_busy_releases_unentered_attachment_custody_for_later_preparation() {
    for answer in [LiveMcpAttachError::Busy, LiveMcpAttachError::Unsupported,
        LiveMcpAttachError::Refused(McpAttachmentRefusal::Unavailable)] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (b, stored, control) = approved(&fixture).await;
            let busy = answer == LiveMcpAttachError::Busy;
            let calls = Arc::new(AtomicUsize::new(0));
            let started = calls.clone();
            let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
                started.fetch_add(1, Ordering::SeqCst); Err(answer)
            }));
            let outcome = fixture.source.ensure_service_attachment_member(&fixture.store, &b, &held.task).await;
            if busy { assert!(matches!(outcome, Ok(false))); }
            else {
                assert!(outcome.is_err());
                assert!(fixture.source.ensure_service_attachment_member(&fixture.store, &b, &held.task).await.is_err());
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.source.held_service_attachment(&b, &held.task).unwrap().is_none(), busy);
            assert_eq!(fixture.store.get(&stored.key).await.unwrap(), Some(stored));
            assert_eq!(fixture.inputs().await, Ok(1));
            fixture.source.join_service_attachments().await;
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

struct PerOperationObserver { calls: Mutex<Vec<String>> }
#[async_trait::async_trait]
impl McpAttachmentAttemptObserver for PerOperationObserver {
    async fn observe(&self, raw: &str) -> McpAttachmentReadiness {
        self.calls.lock().unwrap().push(raw.into());
        match raw {
            "read" => McpAttachmentReadiness::Usable { connection_generation: 7 },
            "search" => McpAttachmentReadiness::Usable { connection_generation: 8 },
            _ => panic!("only the actual reviewed raw set may be observed"),
        }
    }
    async fn wait(&self, raw: &str, _: Duration) -> McpAttachmentReadiness { self.observe(raw).await }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_reviewed_raw_operation_set_must_belong_to_one_attempt_generation() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved_operations(&fixture, &["read", "search"]).await;
        let observed = Arc::new(PerOperationObserver { calls: Mutex::new(vec![]) });
        let answer = LiveMcpAttachmentReceipt {
            outcome: McpAttachmentOutcome { persisted: false, staged: true },
            attempt: McpAttachmentAttempt::new("mail", observed.clone()),
        };
        keep(&fixture.source, &b, &stored, control, Box::pin(async move { Ok(answer) }));
        fixture.source.step_service_input_with_attachment(&fixture.store, &stored).await.unwrap();
        assert_eq!(*observed.calls.lock().unwrap(), vec!["read", "search"]);
        assert_eq!(fixture.store.get(&stored.key).await.unwrap(), Some(stored));
        assert_eq!(fixture.inputs().await, Ok(1));
        fixture.source.join_service_attachments().await;
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observation_timeout_keeps_same_accepted_attempt_and_cannot_reattach() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let started = calls.clone();
        let observed = Arc::new(Observer {
            value: Mutex::new(McpAttachmentReadiness::Usable { connection_generation: 7 }),
            operations: Mutex::new(vec![]),
            held: Some((tokio::sync::Notify::new(), tokio::sync::Notify::new())),
        });
        let answer = receipt(observed.clone());
        let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst); Ok(answer)
        }));
        for _ in 0..2 {
            tokio::time::timeout(WAIT, fixture.source.step_service_input_with_attachment(
                &fixture.store, &stored)).await.unwrap().unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(observed.operations.lock().unwrap().is_empty(), "read never reached its scripted usable answer");
        assert!(Arc::ptr_eq(&held, &fixture.source.held_service_attachment(&b, &held.task).unwrap().unwrap()));
        assert_eq!(fixture.store.get(&stored.key).await.unwrap(), Some(stored));
        assert_eq!(fixture.inputs().await, Ok(1));
        fixture.source.join_service_attachments().await;
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}


// These cases exercise the actual keeper and real mechanical login owner.
// They do not authenticate a Human or prove the production B-to-A login join.
fn login_owner() -> (tokio::sync::watch::Sender<bool>, crate::private_host::ConsoleLogins, ConsoleLoginHandle) {
    let (connection, owner) = crate::private_host::ConsoleLogins::fixture_owner();
    let expiry = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000;
    let login = owner.fixture_enroll(2, crate::private_host::ConsoleLoginSelection {
        issuer: "attachment-owner-fixture".into(), subject: "fixture-subject".into(),
        login_scope_id: "fixture-login".into(), expires_at_ms: expiry,
    }).unwrap();
    (connection, owner, login)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_attachment_login_never_polls_native_and_does_not_use_another_login() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let (_connection, logins, login) = login_owner();
        let (_other_connection, other_logins, other) = login_owner();
        assert!(!login.same_owner(&other));
        let retired = logins.fixture_retire(2).unwrap().await.unwrap();
        assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = calls.clone();
        let held = keep_with_login(&fixture.source, &b, &stored, control, Box::pin(async move {
            entered.fetch_add(1, Ordering::SeqCst); Err(LiveMcpAttachError::Busy)
        }), Some(login));
        assert!(matches!(held.receipt(|| Ok(())).await, Err(PrivateOriginalError::Denied)));
        held.join_if_polled().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(held.receipt(|| Ok(())).await, Err(PrivateOriginalError::Denied)));
        other.reserve().unwrap().settled();
        assert!(!other_logins.fixture_retire(2).unwrap().await.unwrap().uncertain);
        assert_eq!(fixture.store.get(&stored.key).await.unwrap(), Some(stored));
        assert_eq!(fixture.inputs().await, Ok(1));
        fixture.source.remove_service_attachment(&held);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_login_reservation_survives_waiter_loss_until_exact_native_reply() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let (_connection, logins, login) = login_owner();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let (send, receive) = tokio::sync::oneshot::channel();
        let count = calls.clone(); let signal = entered.clone();
        let held = keep_with_login(&fixture.source, &b, &stored, control, Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst); signal.notify_one(); receive.await.unwrap()
        }), Some(login));
        let mut waiter = Box::pin(held.receipt(|| Ok(())));
        tokio::select! {
            result = &mut waiter => panic!("the native reply must still be held: {result:?}"),
            _ = entered.notified() => (),
        }
        let mut retirement = logins.fixture_retire(2).unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut retirement).await.is_err());
        drop(waiter);
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut retirement).await.is_err(),
            "dropping the waiter must not release the keeper's reservation");
        send.send(Ok(receipt(observer(McpAttachmentReadiness::Pending)))).unwrap();
        held.join_if_polled().await;
        let result = retirement.await.unwrap();
        assert!(result.fenced && result.reservations_drained && !result.uncertain);
        assert!(held.receipt(|| panic!("settled reply must not start again")).await.unwrap().is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.store.get(&stored.key).await.unwrap(), Some(stored));
        assert_eq!(fixture.inputs().await, Ok(1));
        fixture.source.remove_service_attachment(&held);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_busy_is_definite_but_failed_stage_keeps_login_retirement_uncertain() {
    for answer in [LiveMcpAttachError::Busy, LiveMcpAttachError::Stage("reply unavailable".into())] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (b, stored, control) = approved(&fixture).await;
            let (_connection, logins, login) = login_owner();
            let busy = answer == LiveMcpAttachError::Busy;
            let calls = Arc::new(AtomicUsize::new(0)); let count = calls.clone();
            let held = keep_with_login(&fixture.source, &b, &stored, control, Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst); Err(answer)
            }), Some(login));
            let returned = held.receipt(|| Ok(())).await.unwrap();
            assert_eq!(matches!(returned, Err(LiveMcpAttachError::Busy)), busy);
            let retired = logins.fixture_retire(2).unwrap().await.unwrap();
            assert!(retired.fenced && retired.reservations_drained);
            assert_eq!(retired.uncertain, !busy);
            assert!(held.receipt(|| panic!("native result must stay retained")).await.unwrap().is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.store.get(&stored.key).await.unwrap(), Some(stored));
            assert_eq!(fixture.inputs().await, Ok(1));
            fixture.source.remove_service_attachment(&held);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[path = "attachment_native_tests.rs"]
mod native;


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attachment_failure_fences_b_and_retains_exact_cause_without_reattachment() {
    use meerkat_toolkit::setup::ApprovedServiceAttachmentFailure as Failure;
    for (readiness, expected) in [
        (McpAttachmentReadiness::Failed { detail: "untrusted startup detail must not be persisted".into() }, Failure::Failed),
        (McpAttachmentReadiness::Rejected { detail: "untrusted refusal detail must not be persisted".into() }, Failure::Rejected),
    ] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (b, stored, control) = approved(&fixture).await;
            let record = fixture.source.decode_stored(&stored).unwrap();
            let selected = fixture.source.service_task_snapshot(&record).await.unwrap();
            let observed = observer(readiness);
            let answer = receipt(observed.clone());
            let calls = Arc::new(AtomicUsize::new(0));
            let started = calls.clone();
            let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
                started.fetch_add(1, Ordering::SeqCst); Ok(answer)
            }));
            fixture.source.step_service_input_with_attachment(&fixture.store, &stored).await.unwrap();
            let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
            let record = fixture.source.decode_stored(&saved).unwrap();
            assert!(record.operation_cancelled && record.service_admission.is_none() && record.final_association.is_none());
            let terminal = fixture.source.service_task_snapshot(&record).await.unwrap();
            assert_eq!(terminal.state().lifecycle_phase, toolkit_catalog::AcquisitionPhase::Unavailable);
            assert!(terminal.service_attachment_failure() == Some(expected));
            assert!(terminal.approved_service() == selected.approved_service());
            assert_eq!(terminal.request().authority_digest().unwrap(), selected.request().authority_digest().unwrap());
            let obligation = fixture.source.service_setup_store().unwrap().committed_continuation(
                &Id::new(terminal.key().id.clone()).unwrap()).await.unwrap().unwrap();
            assert!(fixture.source.failed_service_allows_original_result(&obligation).await.unwrap());
            let encoded = String::from_utf8(toolkit_package::canonical_json(&obligation).unwrap()).unwrap();
            assert!(!encoded.contains("untrusted"));
            assert!(fixture.source.held_service_attachment(&b, &held.task).unwrap().is_none());
            fixture.source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(*observed.operations.lock().unwrap(), vec!["read"]);
            assert_eq!(fixture.inputs().await, Ok(1), "failure settlement itself never submits A or B");
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attachment_failure_after_cancellation_cannot_stamp_failure_provenance() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let observed = Arc::new(Observer { value: Mutex::new(McpAttachmentReadiness::Failed { detail: "late failure".into() }),
            operations: Mutex::new(vec![]), held: Some((tokio::sync::Notify::new(), tokio::sync::Notify::new())) });
        let answer = receipt(observed.clone());
        keep(&fixture.source, &b, &stored, control, Box::pin(async move { Ok(answer) }));
        let mut work = Box::pin(fixture.source.step_service_input_with_attachment(&fixture.store, &stored));
        tokio::select! {
            result = &mut work => panic!("readiness should be held: {result:?}"),
            _ = observed.held.as_ref().unwrap().0.notified() => (),
        }
        fixture.source.record_task_group_cancelled(&b).await.unwrap();
        observed.held.as_ref().unwrap().1.notify_one();
        assert!(tokio::time::timeout(WAIT, work).await.unwrap().is_err());
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_stored(&saved).unwrap();
        assert!(record.operation_cancelled && record.final_association.is_none() && record.service_admission.is_none());
        fixture
            .source
            .reject_failed_service_attachment_with_observation(
                &b,
                meerkat_toolkit::setup::ApprovedServiceAttachmentFailure::Failed,
                Some(
                    serde_json::from_value(serde_json::json!({
                        "phase": "initialize", "class": {"kind": "unknown"},
                    }))
                    .unwrap(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.store.get(&stored.key).await.unwrap().unwrap(),
            saved
        );
        let task = fixture.source.service_task_snapshot(&record).await.unwrap();
        assert!(task.service_attachment_failure().is_none());
        assert!(task.service_attachment_observation().is_none());
        fixture.source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
        let task = fixture.source.service_task_snapshot(&record).await.unwrap();
        assert!(task.service_attachment_failure().is_none());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attachment_failure_cannot_use_an_unsettled_cancellation_as_nonentry() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let answer = receipt(observer(McpAttachmentReadiness::Failed { detail: "startup failed".into() }));
        keep(&fixture.source, &b, &stored, control, Box::pin(async move { Ok(answer) }));
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_cancel BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.operation_cancelled') = 1 \
             BEGIN SELECT RAISE(FAIL, 'cancellation not durable'); END;", stored.key.id)).unwrap();
        assert!(fixture.source.step_service_input_with_attachment(&fixture.store, &stored).await.is_err());
        assert!(
            fixture
                .source
                .reject_failed_service_attachment_with_observation(
                    &b,
                    meerkat_toolkit::setup::ApprovedServiceAttachmentFailure::Failed,
                    Some(
                        serde_json::from_value(serde_json::json!({
                            "phase": "initialize", "class": {"kind": "protocol_error"},
                        }))
                        .unwrap()
                    )
                )
                .await
                .is_err()
        );
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_stored(&saved).unwrap();
        assert!(!record.operation_cancelled && record.final_association.is_none() && record.service_admission.is_none());
        assert!(record.service_attachment_observation.is_none());
        let task = fixture.source.service_task_snapshot(&record).await.unwrap();
        assert!(task.service_attachment_failure().is_none());
        assert!(fixture.source.service_setup_store().unwrap().committed_continuation(
            &Id::new(task.key().id.clone()).unwrap()).await.unwrap().is_none());
        assert_eq!(fixture.inputs().await, Ok(1));
        sql.execute_batch("DROP TRIGGER fail_cancel;").unwrap();
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attachment_failure_replays_after_cancel_commit_before_result_commit() {
    attachment_failure_replay(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_attachment_observation_replays_from_exact_cancellation_cas() {
    attachment_failure_replay(Some(serde_json::from_value(serde_json::json!({
        "phase": "tool_discovery", "class": {"kind": "closed", "exit": {"kind": "signal", "value": 15}},
    })).unwrap())).await;
}

async fn attachment_failure_replay(
    observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
) {
    use meerkat_toolkit::setup::ApprovedServiceAttachmentFailure;
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let before = fixture.source.decode_stored(&stored).unwrap();
        assert!(before.service_attachment_failure.is_none());
        assert!(stored.data.body.get("service_attachment_failure").is_none(), "legacy source bytes omit None");
        assert_eq!(toolkit_package::canonical_json(&before).unwrap(),
            toolkit_package::canonical_json(&stored.data.body).unwrap());
        let task = fixture.source.service_task_snapshot(&before).await.unwrap();
        let answer = receipt(observer(McpAttachmentReadiness::Failed { detail: "fixture terminal cause".into() }));
        let calls = Arc::new(AtomicUsize::new(0));
        let started = calls.clone();
        let held = keep(&fixture.source, &b, &stored, control, Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst); Ok(answer)
        }));
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_setup_result BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.service_attachment_failure') IS NOT NULL \
             BEGIN SELECT RAISE(FAIL, 'setup result write unavailable'); END;", task.key().id)).unwrap();
        let result = if observation.is_some() {
            // This scripted observer supplies no metadata. The explicit
            // synthetic value exercises partial-commit replay in the writer.
            let reply = held.receipt(|| Ok(())).await.unwrap().unwrap();
            let ServiceAttachmentReadiness::Failed { failure, observation: None } = fixture.source
                .service_attachment_ready(&held, &reply).await.unwrap()
                else { panic!("scripted native readiness must fail"); };
            fixture.source.attachment_current_after_read(&held).await.unwrap();
            fixture.source.reject_failed_service_attachment_with_observation(&b, failure, observation).await
        } else {
            fixture.source.step_service_input_with_attachment(&fixture.store, &stored).await
        };
        assert!(result.is_err());
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let cancelled = fixture.source.decode_stored(&saved).unwrap();
        assert_eq!(cancelled.service_attachment_observation, observation);
        assert!(cancelled.operation_cancelled && cancelled.service_attachment_failure == Some(ApprovedServiceAttachmentFailure::Failed));
        assert!(cancelled.final_association.is_none() && cancelled.service_admission.is_none());
        assert!(fixture.source.service_task_snapshot(&cancelled).await.unwrap().service_attachment_failure().is_none());
        // Lose process-local attachment custody too. Only durable source facts
        // plus its exact native cancellation evidence can repair settlement.
        fixture.source.retire_service_attachment_member(&b, &held.task).await;
        sql.execute_batch("DROP TRIGGER fail_setup_result;").unwrap();
        fixture.source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
        let terminal = fixture.source.service_task_snapshot(&cancelled).await.unwrap();
        assert!(terminal.service_attachment_failure() == Some(ApprovedServiceAttachmentFailure::Failed));
        assert_eq!(terminal.service_attachment_observation(), observation);
        assert_eq!(terminal.state().lifecycle_phase, toolkit_catalog::AcquisitionPhase::Unavailable);
        let obligation = fixture.source.service_setup_store().unwrap().committed_continuation(
            &Id::new(task.key().id.clone()).unwrap()).await.unwrap().unwrap();
        assert!(fixture.source.failed_service_allows_original_result(&obligation).await.unwrap());
        assert_eq!(obligation.service_attachment_observation(), observation);
        let digest = obligation.digest().unwrap();
        // Repeated settlement cannot enrich or replace the first committed
        // observation, including a legacy None, after cancellation/retirement.
        fixture.source.reject_failed_service_attachment_with_observation(&b,
            ApprovedServiceAttachmentFailure::Rejected,
            Some(serde_json::from_value(serde_json::json!({
                "phase": "launch", "class": {"kind": "spawn_failed"},
            })).unwrap())).await.unwrap();
        assert_eq!(fixture.source.service_setup_store().unwrap().committed_continuation(
            obligation.operation()).await.unwrap().unwrap().digest().unwrap(), digest);
        fixture.source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
        assert_eq!(fixture.store.get(&stored.key).await.unwrap().unwrap(), saved,
            "generic cancellation replay keeps the atomically retained cause and evidence");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

// These controls substitute only the optional native observation. The physical
// Core/Meta control is separate in attachment_native_tests.rs.
struct MetadataObserver {
    readiness: McpAttachmentReadiness,
    metadata: Option<McpStdioSetupFailure>,
    held: Option<(tokio::sync::Notify, tokio::sync::Notify)>,
    reads: AtomicUsize,
    tails: AtomicUsize,
}
#[async_trait::async_trait]
impl McpAttachmentAttemptObserver for MetadataObserver {
    async fn observe(&self, _: &str) -> McpAttachmentReadiness {
        self.readiness.clone()
    }
    async fn wait(&self, _: &str, _: Duration) -> McpAttachmentReadiness {
        self.readiness.clone()
    }
    async fn setup_failure(&self) -> Option<McpStdioSetupFailure> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some((entered, release)) = &self.held {
            entered.notify_one();
            release.notified().await;
        }
        self.metadata.clone()
    }
    async fn stderr_tail(&self) -> Option<meerkat_core::mcp_attachment::McpStdioStderrTail> {
        self.tails.fetch_add(1, Ordering::SeqCst);
        panic!("Connect must never request sensitive stderr");
    }
}
fn metadata_observer(held: bool, rejected: bool, present: bool) -> Arc<MetadataObserver> {
    Arc::new(MetadataObserver {
        readiness: if rejected {
            McpAttachmentReadiness::Rejected {
                detail: "untrusted-readiness-sentinel".into(),
            }
        } else {
            McpAttachmentReadiness::Failed {
                detail: "untrusted-readiness-sentinel".into(),
            }
        },
        metadata: present.then(|| {
            McpStdioSetupFailure::new(
                "untrusted-server-sentinel",
                913,
                McpStdioSetupPhase::Initialize,
                McpStdioSetupFailureClass::Closed {
                    exit: Some(McpStdioExit::Code(3)),
                },
                true,
            )
        }),
        held: held.then(|| (tokio::sync::Notify::new(), tokio::sync::Notify::new())),
        reads: AtomicUsize::new(0),
        tails: AtomicUsize::new(0),
    })
}
fn metadata_receipt(observer: Arc<MetadataObserver>) -> LiveMcpAttachmentReceipt {
    LiveMcpAttachmentReceipt {
        outcome: McpAttachmentOutcome {
            persisted: false,
            staged: true,
        },
        attempt: McpAttachmentAttempt::new("mail", observer),
    }
}

#[test]
fn setup_failure_projection_keeps_only_known_typed_fields() {
    use meerkat_toolkit::setup::{
        ApprovedServiceAttachmentClass as Class, ApprovedServiceAttachmentExit as Exit,
        ApprovedServiceAttachmentPhase as Phase,
    };
    let classes = [
        (
            McpStdioSetupFailureClass::SpawnFailed,
            Class::SpawnFailed {},
        ),
        (
            McpStdioSetupFailureClass::Closed {
                exit: Some(McpStdioExit::Code(3)),
            },
            Class::Closed {
                exit: Some(Exit::Code(3)),
            },
        ),
        (
            McpStdioSetupFailureClass::Closed {
                exit: Some(McpStdioExit::Signal(15)),
            },
            Class::Closed {
                exit: Some(Exit::Signal(15)),
            },
        ),
        (
            McpStdioSetupFailureClass::Closed { exit: None },
            Class::Closed { exit: None },
        ),
        (
            McpStdioSetupFailureClass::Rejected { code: -32601 },
            Class::Rejected {},
        ),
        (
            McpStdioSetupFailureClass::ProtocolError,
            Class::ProtocolError {},
        ),
        (McpStdioSetupFailureClass::TimedOut, Class::TimedOut {}),
        (McpStdioSetupFailureClass::Unknown, Class::Unknown {}),
    ];
    for (native_phase, phase) in [
        (McpStdioSetupPhase::Launch, Phase::Launch),
        (McpStdioSetupPhase::Initialize, Phase::Initialize),
        (McpStdioSetupPhase::ToolDiscovery, Phase::ToolDiscovery),
    ] {
        for (native_class, class) in classes {
            let projected = project_setup_failure(McpStdioSetupFailure::new(
                "untrusted-server-sentinel",
                913,
                native_phase,
                native_class,
                true,
            ))
            .unwrap();
            assert_eq!(projected.phase, phase);
            assert_eq!(projected.class, class);
            let encoded = serde_json::to_value(projected).unwrap();
            assert_eq!(encoded.as_object().unwrap().len(), 2);
            assert!(!encoded.to_string().contains("sentinel"));
            assert!(encoded.get("attempt").is_none());
            assert!(encoded.get("stderr_tail_retained").is_none());
            assert!(encoded["class"].get("code").is_none());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_metadata_is_committed_with_the_exact_terminal_failure() {
    use meerkat_toolkit::setup::ApprovedServiceAttachmentFailure;
    for (rejected, present) in [(false, true), (true, true), (false, false)] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (b, stored, control) = approved(&fixture).await;
            let observed = metadata_observer(false, rejected, present);
            let expected = observed.metadata.clone().and_then(project_setup_failure);
            let answer = metadata_receipt(observed.clone());
            let held = keep(
                &fixture.source,
                &b,
                &stored,
                control,
                Box::pin(async move { Ok(answer) }),
            );
            fixture
                .source
                .step_service_input_with_attachment(&fixture.store, &stored)
                .await
                .unwrap();
            let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
            let record = fixture.source.decode_stored(&saved).unwrap();
            assert_eq!(record.service_attachment_observation, expected);
            assert!(record.operation_cancelled && record.service_admission.is_none());
            assert!(record.final_association.is_none());
            assert!(
                record.service_attachment_failure
                    == Some(if rejected {
                        ApprovedServiceAttachmentFailure::Rejected
                    } else {
                        ApprovedServiceAttachmentFailure::Failed
                    })
            );
            let task = fixture.source.service_task_snapshot(&record).await.unwrap();
            assert_eq!(task.service_attachment_observation(), expected);
            let obligation = fixture
                .source
                .service_setup_store()
                .unwrap()
                .committed_continuation(&Id::new(task.key().id.clone()).unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(obligation.service_attachment_observation(), expected);
            let encoded = toolkit_package::canonical_json(&obligation).unwrap();
            assert!(!String::from_utf8(encoded).unwrap().contains("sentinel"));
            assert!(
                fixture
                    .source
                    .held_service_attachment(&b, &held.task)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(observed.reads.load(Ordering::SeqCst), 1);
            assert_eq!(observed.tails.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.inputs().await, Ok(1));
        }))
        .catch_unwind()
        .await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_metadata_timeout_preserves_the_timely_terminal_failure() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let observed = metadata_observer(true, false, true);
        let answer = metadata_receipt(observed.clone());
        keep(
            &fixture.source,
            &b,
            &stored,
            control,
            Box::pin(async move { Ok(answer) }),
        );
        tokio::time::timeout(
            Duration::from_secs(2),
            fixture
                .source
                .step_service_input_with_attachment(&fixture.store, &stored),
        )
        .await
        .expect("optional metadata is bounded by the readiness deadline")
        .unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_stored(&saved).unwrap();
        assert!(record.operation_cancelled && record.service_attachment_failure.is_some());
        assert!(record.service_attachment_observation.is_none());
        assert!(record.final_association.is_none() && record.service_admission.is_none());
        let task = fixture.source.service_task_snapshot(&record).await.unwrap();
        assert!(task.service_attachment_failure().is_some());
        assert!(task.service_attachment_observation().is_none());
        assert_eq!(observed.reads.load(Ordering::SeqCst), 1);
        assert_eq!(observed.tails.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.inputs().await, Ok(1));
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_metadata_read_cannot_stamp_failure_provenance() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (b, stored, control) = approved(&fixture).await;
        let observed = metadata_observer(true, false, true);
        let answer = metadata_receipt(observed.clone());
        keep(
            &fixture.source,
            &b,
            &stored,
            control,
            Box::pin(async move { Ok(answer) }),
        );
        let mut work = Box::pin(
            fixture
                .source
                .step_service_input_with_attachment(&fixture.store, &stored),
        );
        tokio::select! {
            result = &mut work => panic!("metadata should be held: {result:?}"),
            _ = observed.held.as_ref().unwrap().0.notified() => (),
        }
        fixture
            .source
            .record_task_group_cancelled(&b)
            .await
            .unwrap();
        observed.held.as_ref().unwrap().1.notify_one();
        assert!(tokio::time::timeout(WAIT, work).await.unwrap().is_err());
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = fixture.source.decode_stored(&saved).unwrap();
        assert!(record.operation_cancelled && record.service_attachment_failure.is_none());
        assert!(record.service_attachment_observation.is_none());
        let task = fixture.source.service_task_snapshot(&record).await.unwrap();
        assert!(task.service_attachment_failure().is_none());
        assert!(task.service_attachment_observation().is_none());
        assert_eq!(observed.reads.load(Ordering::SeqCst), 1);
        assert_eq!(observed.tails.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.inputs().await, Ok(1));
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

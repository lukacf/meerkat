//! Real Find-fenced SQLite attempt mechanics, independent of the public SDK
//! selection fixture. This does not construct recipient custody or a G input.
use super::*;
use crate::installed_host::acquisition::recovery::{AcquisitionRecovery, RecoveryControl};
use crate::private_originals::OwnedTransferCommit;
use meerkat_workgraph::{
    CreateWorkItemRequest, SqliteWorkGraphStore, WorkAdmissionKey, WorkAdmissionOutcome,
    WorkGraphService, WorkNamespace, WorkStatus,
};

struct CommitProbe {
    source: Arc<PrivateOriginals>,
    service: WorkGraphService,
    control: Arc<RecoveryControl>,
    attempts: Mutex<Vec<Arc<OwnedTransferCommit>>>,
    dropped_join: tokio::sync::Notify,
    committed: tokio::sync::Notify,
    resume: tokio::sync::Semaphore,
    finished: tokio::sync::Notify,
    count: AtomicUsize,
}
impl CommitProbe {
    fn request(call: ToolCallView<'_>, scope: &ToolDispatchContext) -> CreateWorkItemRequest {
        CreateWorkItemRequest {
            title: "Actual Find owned commit control".into(),
            status: Some(WorkStatus::Open),
            description: Some(
                String::from_utf8(
                    canonical_json(&serde_json::json!({
                        "purpose":"owned-attempt-control", "call":call.id,
                        "request":call.parse_args::<serde_json::Value>().unwrap(),
                        "scope":scope.work_authorization().unwrap().execution_scope(),
                    }))
                    .unwrap(),
                )
                .unwrap(),
            ),
            ..Default::default()
        }
    }
    fn enter(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        label: &str,
    ) -> Arc<OwnedTransferCommit> {
        let deadline = self.source.owner.clock().now().unwrap().unix_ms + 60_000;
        self.control
            .enter_transfer(|| {
                let mut attempts = self.attempts.lock().unwrap();
                let held = Arc::new(
                    OwnedTransferCommit::start(
                        call,
                        context,
                        self.source.owner.clone(),
                        self.service.clone(),
                        WorkAdmissionKey::new(label).unwrap(),
                        Self::request(call, context),
                        deadline,
                    )
                    .unwrap(),
                );
                attempts.push(held.clone());
                held
            })
            .expect("the actual recovery stop owner admitted this attempt")
    }
}
#[async_trait]
impl FindDispatchProbe for CommitProbe {
    async fn inspect(&self, call: ToolCallView<'_>, context: &ToolDispatchContext) {
        assert_eq!(self.count.fetch_add(1, Ordering::SeqCst), 0);
        assert_eq!(call.name, crate::FIND_AND_INSTALL);
        let original = self.source.acquisition_original(context).await.unwrap();
        assert_eq!(
            original
                .original
                .association()
                .candidate()
                .original_work
                .work,
            original.reference.original
        );
        // The external lock is held before this actual Find enters. The task
        // cannot finish, and dropping only the join future must retain its owner.
        let first = self.enter(call, context, "owned-transfer-first");
        assert!(
            tokio::time::timeout(Duration::from_millis(1), first.join())
                .await
                .is_err()
        );
        assert!(!first.finished());
        assert!(Arc::ptr_eq(&self.attempts.lock().unwrap()[0], &first));
        self.dropped_join.notify_one();
        let admission = first.join().await.unwrap();
        assert_eq!(first.admission(), &admission);
        assert_eq!(
            first.join().await.unwrap(),
            admission,
            "joined result is retained without a second write"
        );
        assert!(matches!(
            self.service
                .observe_admission(admission.key.clone(), Self::request(call, context))
                .await
                .unwrap(),
            Some(WorkAdmissionOutcome::Replayed(_))
        ));
        self.committed.notify_one();
        self.resume.acquire().await.unwrap().forget();
        // The controller call remains borrowed, but the actual A source was
        // durably cancelled. Native current operation entry must now refuse.
        let denied = self.enter(call, context, "owned-transfer-after-cancel");
        assert!(matches!(
            denied.join().await,
            Err(PrivateOriginalError::Denied)
        ));
        assert!(
            self.service
                .observe_admission(
                    WorkAdmissionKey::new("owned-transfer-after-cancel").unwrap(),
                    Self::request(call, context)
                )
                .await
                .unwrap()
                .is_none()
        );
        self.finished.notify_one();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_find_commit_retains_dropped_join_and_refuses_cancelled_source() {
    let script = Script::paused_for_helper_entry();
    let (fixture, _history) = FirstFixture::<meerkat::PersistentSessionService<FactoryAgentBuilder>>::
        for_approved_acquisition(script.clone()).await;
    let lifecycle = AcquisitionRecovery::default();
    let path = fixture.source_path.with_extension("owned-transfer.sqlite");
    let service = WorkGraphService::with_scope(
        Arc::new(SqliteWorkGraphStore::open(&path).unwrap()),
        fixture.scope.realm.clone(),
        WorkNamespace::new("owned-commit-control").unwrap(),
    );
    let probe = Arc::new(CommitProbe {
        source: fixture.source.clone(),
        service,
        control: lifecycle.transfer_control(),
        attempts: Mutex::new(Vec::new()),
        dropped_join: tokio::sync::Notify::new(),
        committed: tokio::sync::Notify::new(),
        resume: tokio::sync::Semaphore::new(0),
        finished: tokio::sync::Notify::new(),
        count: AtomicUsize::new(0),
    });
    assert!(
        script
            .dispatch
            .probe
            .lock()
            .unwrap()
            .replace(probe.clone())
            .is_none()
    );
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        locked_tx.send(()).unwrap();
        let _ = release_rx.recv();
        db.execute_batch("ROLLBACK").unwrap();
    });
    locked_rx.recv().unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, async {
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let submitted = fixture.submit().await;
        assert!(matches!(
            &submitted,
            Ok(PrivateOriginalSubmission::Admitted { .. })
        ));
        script.first_response_waiting().await;
        script.release_first_response();
        probe.dropped_join.notified().await;
        release_tx.send(()).unwrap();
        probe.committed.notified().await;
        assert!(matches!(
            fixture
                .source
                .cancel_original_operation(&fixture.submission.original)
                .await
                .unwrap(),
            PrivateOriginalCancellation::OperationsCancelled
        ));
        probe.resume.add_permits(1);
        probe.finished.notified().await;
        // Cancellation may end the original turn with refusal. It may not
        // cause the entered transfer to replay or create a post-cancel record.
        let _ = fixture.finish(&submitted).await;
        assert_eq!(probe.count.load(Ordering::SeqCst), 1);
    }))
    .catch_unwind()
    .await;
    // These releases are harmless on success and ensure cleanup owns every
    // started native/store task on an assertion failure or caller timeout.
    let _ = release_tx.send(());
    script.release_first_response();
    probe.resume.add_permits(1);
    lifecycle.stop();
    assert!(
        lifecycle
            .transfer_control()
            .enter_transfer(|| panic!("no attempt after stop"))
            .is_none()
    );
    lifecycle.join().await.unwrap(); // No recovery task was started in this control.
    holder.join().unwrap();
    let retained = probe.attempts.lock().unwrap().clone();
    for attempt in retained {
        let _ = attempt.join().await;
    }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("actual Find control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[path = "fenced_retry_tests.rs"]
mod fenced_retry_tests;

//! Actual Core R3 entry and SQLite I/O under the existing local probe policy.
//! This covers the deferred native Used observation across caller loss. It
//! does not claim installed Find, helper-graph or Console authentication proof.
use super::*;
use meerkat_core::approval::review::{
    OperationReviewObserver, ReviewAttemptStatus, ReviewObservation,
};
use toolkit_store::{
    AdmissionRequest, RecordChangeGuard, RecordChangePublication, RecordData, SqliteStore,
    StoreKey, StoreResult, ToolkitStore, Transition,
};

#[path = "io_custody_tests/entered_operation.rs"]
mod entered_operation;

struct PublicationPause {
    entered: tokio::sync::Notify,
    release: (Mutex<bool>, std::sync::Condvar),
    active: AtomicBool,
}
struct PublicationGuard<'a>(&'a PublicationPause);
impl RecordChangeGuard for PublicationGuard<'_> {}
impl Drop for PublicationGuard<'_> {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::SeqCst);
    }
}
impl RecordChangePublication for PublicationPause {
    fn begin_record_change(&self) -> StoreResult<Box<dyn RecordChangeGuard + '_>> {
        assert!(!self.active.swap(true, Ordering::SeqCst));
        self.entered.notify_one();
        let (lock, ready) = &self.release;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = ready.wait(released).unwrap();
        }
        Ok(Box::new(PublicationGuard(self)))
    }
}
struct Release(Arc<PublicationPause>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.release.0.lock().unwrap() = true;
        self.0.release.1.notify_all();
    }
}
struct Observations {
    publication: Arc<PublicationPause>,
    rows: Mutex<Vec<(ReviewObservation, bool)>>,
    used: tokio::sync::Notify,
}
impl OperationReviewObserver for Observations {
    fn observe(&self, observation: ReviewObservation) {
        let used = observation.status == ReviewAttemptStatus::Used;
        self.rows
            .lock()
            .unwrap()
            .push((observation, self.publication.active.load(Ordering::SeqCst)));
        if used {
            self.used.notify_one();
        }
    }
}
struct SqliteProbe {
    store: Arc<SqliteStore>,
    change: Transition,
}
#[async_trait::async_trait]
impl AgentToolDispatcher for SqliteProbe {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Probe::default().tools()
    }
    fn review_entry_support(&self, _: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::access_denied(call.name))
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let (_, custody) = context.enter_reviewed_effect_held(call, Some(plan))?;
        toolkit_store::retain_io_custody(
            Arc::new(custody),
            self.store.compare_and_swap(self.change.clone()),
        )
        .await
        .map_err(|_| ToolError::access_denied(call.name))?;
        Ok(meerkat_core::ToolResult::new(
            call.id.to_owned(),
            "SQLite probe committed".into(),
            false,
        )
        .into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumed_native_review_is_not_used_while_cancelled_sqlite_write_remains_live() {
    let temp = tempfile::tempdir().unwrap();
    // The real issuer requires a canonical private parent. macOS TMPDIR may
    // name /var while the same directory's canonical identity is /private/var.
    let root = temp.path().canonicalize().unwrap();
    let owner = ApplicationConsentOwner::create(&root, identity()).unwrap();
    let publication = Arc::new(PublicationPause {
        entered: tokio::sync::Notify::new(),
        release: (Mutex::new(false), std::sync::Condvar::new()),
        active: AtomicBool::new(false),
    });
    let release = Release(publication.clone());
    let store = Arc::new(
        SqliteStore::open_published(temp.path().join("probe.sqlite"), publication.clone()).unwrap(),
    );
    let prior = store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "probe".into(),
                owner: "probe".into(),
                id: "record".into(),
            },
            admission_key: "probe-record".into(),
            data: RecordData {
                schema: "test.review-custody".into(),
                version: 1,
                body: serde_json::json!({"value":1}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap()
        .record()
        .clone();
    let probe = Arc::new(SqliteProbe {
        store: store.clone(),
        change: Transition {
            key: prior.key.clone(),
            expected_revision: prior.revision,
            expected_digest: Some(prior.content_digest.clone()),
            claim: None,
            data: RecordData {
                body: serde_json::json!({"value":2}),
                ..prior.data.clone()
            },
            events: vec![],
            effects: vec![],
        },
    });
    let observed = Arc::new(Observations {
        publication: publication.clone(),
        rows: Mutex::new(vec![]),
        used: tokio::sync::Notify::new(),
    });
    let review = Arc::new(
        BoundOperationReview::new(
            Arc::new(NoModelReview),
            owner.approvals(),
            Duration::from_secs(30),
        )
        .with_human_consent(Arc::new(LocalHumanRoute), Duration::from_secs(3600))
        .with_observer(observed.clone()),
    );
    let session = meerkat_core::SessionId::new();
    let claim =
        grant_session_hosting(&hosting(temp.path()), &HostingOwner::mint(), &session).unwrap();
    let context = context(&owner, &claim).with_operation_review(Some(review));
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id: "sqlite-held-entry",
        name: "consent_probe",
        args: &args,
    };
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_secs(30),
        )])
        .unwrap(),
    );
    let plan =
        meerkat_core::resolve_tool_execution_plan_fenced(&probe, call, &context, &resolution)
            .unwrap();
    let approval = match meerkat_core::dispatch_tool_execution_plan_fenced(
        &probe, call, &context, &plan,
    )
    .await
    {
        Err(ToolError::ReviewUnsatisfied {
            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
            approval_id: Some(id),
        }) => id,
        result => panic!("expected real unconsumed R3 refusal: {result:?}"),
    };
    assert!(!publication.active.load(Ordering::SeqCst));
    approve(&owner, &approval);
    let attempt = approval.clone();
    let entered = tokio::spawn(async move {
        meerkat_core::dispatch_retained_consent_attempt(&probe, &context, &attempt).await
    });
    tokio::time::timeout(Duration::from_secs(10), publication.entered.notified())
        .await
        .unwrap();
    assert!(
        owner
            .approvals()
            .get(&approval)
            .unwrap()
            .consent
            .unwrap()
            .consumed_at
            .is_some()
    );
    entered.abort();
    assert!(entered.await.unwrap_err().is_cancelled());
    assert!(publication.active.load(Ordering::SeqCst));
    assert!(
        !observed
            .rows
            .lock()
            .unwrap()
            .iter()
            .any(|(row, _)| row.status == ReviewAttemptStatus::Used),
        "native dispatch ended, but its actual SQLite write still owns custody"
    );
    drop(release);
    tokio::time::timeout(Duration::from_secs(10), observed.used.notified())
        .await
        .unwrap();
    {
        let rows = observed.rows.lock().unwrap();
        let used = rows
            .iter()
            .filter(|(row, _)| row.status == ReviewAttemptStatus::Used)
            .collect::<Vec<_>>();
        assert_eq!(used.len(), 1);
        assert_eq!(used[0].0.tool_call_id.as_ref(), "sqlite-held-entry");
        assert!(
            !used[0].1,
            "Used must run after the actual publication guard released"
        );
    }
    let actual = store.get(&prior.key).await.unwrap().unwrap();
    assert_eq!(actual.revision, prior.revision + 1);
    assert_eq!(actual.data.body, serde_json::json!({"value":2}));
}

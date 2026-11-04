//! Real R3 consumption and entered-witness custody in the actual SQLite worker.
//! The selected policy grants only the local probe; this is not Find authority
//! or a substitute for native graph/child-step authorization tests.
use super::*;
use meerkat_core::EnteredDispatchOperation;

struct EnteredSqliteProbe {
    store: Arc<SqliteStore>,
    change: Transition,
    retained: Arc<Mutex<Option<EnteredDispatchOperation>>>,
}
#[async_trait::async_trait]
impl AgentToolDispatcher for EnteredSqliteProbe {
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
        let (refreshed, custody) = context.enter_reviewed_effect_held(call, Some(plan))?;
        let context = refreshed.as_ref().unwrap_or(context);
        let entered = context.governed_operation(call)?.into_entered(custody)?;
        assert!(
            self.retained
                .lock()
                .unwrap()
                .replace(entered.clone())
                .is_none(),
            "one native entry, not a second spend for the SQLite worker"
        );
        toolkit_store::retain_io_custody(
            Arc::new(entered),
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

async fn exercise(cancel: bool) {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let owner = ApplicationConsentOwner::create(&root, identity()).unwrap();
    let publication = Arc::new(PublicationPause {
        entered: tokio::sync::Notify::new(),
        release: (Mutex::new(false), std::sync::Condvar::new()),
        active: AtomicBool::new(false),
    });
    let mut release = Some(Release(publication.clone()));
    let store = Arc::new(
        SqliteStore::open_published(root.join("entered.sqlite"), publication.clone()).unwrap(),
    );
    let prior = store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "probe".into(),
                owner: "probe".into(),
                id: "entered-record".into(),
            },
            admission_key: "entered-probe-record".into(),
            data: RecordData {
                schema: "test.entered-custody".into(),
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
    let retained = Arc::new(Mutex::new(None));
    let probe = Arc::new(EnteredSqliteProbe {
        store: store.clone(),
        retained: retained.clone(),
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
    let claim = grant_session_hosting(&hosting(&root), &HostingOwner::mint(), &session).unwrap();
    let context = context(&owner, &claim).with_operation_review(Some(review));
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id: "sqlite-entered-witness",
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
        result => panic!("real R3 pre-entry refusal required: {result:?}"),
    };
    assert!(retained.lock().unwrap().is_none());
    assert!(
        owner
            .approvals()
            .get(&approval)
            .unwrap()
            .consent
            .unwrap()
            .consumed_at
            .is_none()
    );
    approve(&owner, &approval);
    assert!(
        owner
            .approvals()
            .get(&approval)
            .unwrap()
            .consent
            .unwrap()
            .consumed_at
            .is_none()
    );
    let attempt = approval.clone();
    let mut dispatch = tokio::spawn(async move {
        meerkat_core::dispatch_retained_consent_attempt(&probe, &context, &attempt).await
    });
    let mut dispatch_joined = false;
    let body = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(10), publication.entered.notified())
            .await
            .unwrap();
        let consumed = owner.approvals().get(&approval).unwrap();
        assert!(consumed.consent.as_ref().unwrap().consumed_at.is_some());
        assert!(retained.lock().unwrap().is_some());
        assert!(publication.active.load(Ordering::SeqCst));
        assert!(
            !observed
                .rows
                .lock()
                .unwrap()
                .iter()
                .any(|(row, _)| row.status == ReviewAttemptStatus::Used)
        );
        if cancel {
            // End the actual native dispatch, not merely one witness clone.
            dispatch.abort();
            let terminal = (&mut dispatch).await;
            dispatch_joined = true;
            assert!(terminal.unwrap_err().is_cancelled());
            assert!(publication.active.load(Ordering::SeqCst));
            assert!(
                !observed
                    .rows
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(row, _)| row.status == ReviewAttemptStatus::Used)
            );
        }
        drop(release.take());
        if !cancel {
            let terminal = (&mut dispatch).await;
            dispatch_joined = true;
            terminal.unwrap().unwrap();
        }
        let actual = tokio::time::timeout(Duration::from_secs(10), store.get(&prior.key))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(actual.revision, prior.revision + 1);
        assert_eq!(actual.data.body, serde_json::json!({"value":2}));
        assert!(!publication.active.load(Ordering::SeqCst));
        assert_eq!(
            owner.approvals().get(&approval).unwrap(),
            consumed,
            "witness clones do not spend or rewrite the original consent"
        );
        assert!(
            !observed
                .rows
                .lock()
                .unwrap()
                .iter()
                .any(|(row, _)| row.status == ReviewAttemptStatus::Used),
            "the explicit final witness still retains the single native entry"
        );
        let last = retained.lock().unwrap().take().unwrap();
        drop(last);
        tokio::time::timeout(Duration::from_secs(10), observed.used.notified())
            .await
            .unwrap();
        let rows = observed.rows.lock().unwrap();
        let used = rows
            .iter()
            .filter(|(row, _)| row.status == ReviewAttemptStatus::Used)
            .collect::<Vec<_>>();
        assert_eq!(used.len(), 1);
        assert_eq!(used[0].0.tool_call_id.as_ref(), "sqlite-entered-witness");
        assert!(!used[0].1, "actual publication ended before native Used");
    })
    .catch_unwind()
    .await;
    // Failure cleanup releases the actual blocking owner and joins the real
    // dispatch before allowing the temporary database/issuer to be dropped.
    drop(release.take());
    if !dispatch_joined {
        dispatch.abort();
        let _ = (&mut dispatch).await;
    }
    let joined_io = tokio::time::timeout(Duration::from_secs(10), store.get(&prior.key)).await;
    let last = retained
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let entry_state = owner.approvals().get(&approval);
    let entered = last.is_some()
        || entry_state.as_ref().is_ok_and(|record| {
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_some())
        });
    drop(last);
    let native_released = if entered
        && !observed
            .rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|(row, _)| row.status == ReviewAttemptStatus::Used)
    {
        tokio::time::timeout(Duration::from_secs(10), observed.used.notified())
            .await
            .is_ok()
    } else {
        true
    };
    if let Err(panic) = body {
        eprintln!(
            "entered SQLite cleanup completed: {}; native custody released: {}",
            matches!(joined_io, Ok(Ok(_))),
            native_released
        );
        std::panic::resume_unwind(panic);
    }
    assert!(
        matches!(joined_io, Ok(Ok(Some(_)))),
        "actual SQLite owner must be joined"
    );
    assert!(
        entry_state.is_ok(),
        "actual native consent must remain readable after cleanup"
    );
    assert!(
        native_released,
        "actual entered native custody must be released"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entered_witness_keeps_real_r3_sqlite_custody_after_dispatch_cancellation() {
    exercise(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entered_witness_keeps_real_r3_sqlite_custody_after_dispatch_return() {
    exercise(false).await;
}

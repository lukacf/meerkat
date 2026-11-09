//! Selection/read-cost controls use actual validated terminal acquisitions and
//! retained source choices. The read proxy only counts or refuses canonical
//! acquisition gets; it does not supply terminality or native permission.
use super::*;
use toolkit_store::*;

struct CanonicalReads {
    inner: Arc<dyn ToolkitStore>,
    acquisition: StoreKey,
    reads: AtomicUsize,
    unavailable: AtomicBool,
}
impl CanonicalReads {
    fn new(inner: Arc<dyn ToolkitStore>, acquisition: StoreKey) -> Arc<Self> {
        Arc::new(Self {
            inner,
            acquisition,
            reads: 0.into(),
            unavailable: false.into(),
        })
    }
}
#[allow(unused_variables)]
#[async_trait::async_trait]
impl ToolkitStore for CanonicalReads {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, _: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        panic!("terminal selection must not admit records")
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if key == &self.acquisition {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(StoreError::Worker);
            }
        }
        self.inner.get(key).await
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        panic!("terminal selection must not update records")
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn read_events(
        &self,
        key: &StoreKey,
        after: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        panic!("terminal selection must not mutate effects or claims")
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        panic!("terminal selection must not mutate effects or claims")
    }
}

async fn ordinary_request(f: &FirstFixture, c: &Case) -> SetupSnapshot {
    let task = NativeOriginalTask {
        origin_session_id: f.session.clone(),
        retained_work: c.a_dispatch.retained_work().unwrap().clone(),
        member: None,
    };
    let store = f.source.workflow_store().unwrap();
    let template = store
        .service_task_snapshot(&id(&c.membership.acquisition().id), &task)
        .await
        .unwrap();
    let evidence = template.request().evidence();
    let constraints = evidence.constraints();
    let claims = evidence.association().candidate();
    let context = SetupUseContext {
        tenant: &constraints.tenant,
        deployment: &constraints.deployment,
        realm: &claims.ingress_namespace.realm,
        target: &claims.target,
        policy_revision: &constraints.policy_revision,
        now_ms: u64::try_from(f.store.now_ms().await.unwrap()).unwrap(),
        authorization: c.a_dispatch.work_authorization().unwrap(),
    };
    let mut host = template.request().host().clone();
    host.correlation = id("ordinary-terminal-call");
    host.work_context = Some(child_work(c, "ordinary-terminal").await);
    let request = validate_request(
        template.request().request().clone(),
        host,
        evidence,
        &context,
        &Inline,
    )
    .unwrap();
    // This acquisition has no workflow membership and an explicitly absent
    // host planning selection. Its native Original is nevertheless real.
    let snapshot = store
        .admit(&request, evidence, &context, &Inline)
        .await
        .unwrap()
        .snapshot;
    f.source
        .retain_workflow_selection(&c.a, &snapshot.request().host().correlation, None, None)
        .unwrap();
    snapshot
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_nonshared_candidates_skip_canonical_reads_with_or_without_retained_source() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let deadline = u64::try_from(f.store.now_ms().await.unwrap()).unwrap() + 60_000;
        let c = prepare_with_waiter_options(&f, Some((deadline, true))).await;
        let ordinary = ordinary_request(&f, &c).await;
        let operation = id(&ordinary.key().id);
        let terminal = ready_operation(&f, &c, &operation).await;
        let delivery = Delivery::new(true);
        assert!(matches!(f.source.workflow_store().unwrap().step_continuation(
            &operation, delivery.as_ref(), &options()).await.unwrap(), ContinuationStep::Acknowledged(_)));
        let before = f.store.get(terminal.key()).await.unwrap();
        let effects = f.store.list_effects(terminal.key(), None, 16).await.unwrap();
        let reads = CanonicalReads::new(f.store.clone(), terminal.key().clone());
        reads.unavailable.store(true, Ordering::SeqCst);
        for retained in [true, false] {
            if !retained {
                // Lose only this adapter's exact retained handle. The actual
                // ordinary source owner and its durable/native custody remain.
                let mut retained = f.source.retained.lock().unwrap();
                let before = retained.len();
                retained.retain(|held| held.original.association() != c.a.original.association());
                assert_eq!(retained.len() + 1, before, "remove exactly the ordinary source handle");
                assert!(!retained.iter().any(|held|
                    held.original.association() == c.a.original.association()));
            }
            for _ in 0..3 {
                let page = f.source.workflow_store().unwrap().continuation_recovery_page(None, 32).await.unwrap();
                assert!(!page.operations.contains(&operation), "result was already acknowledged");
                let candidate = page.terminal_operations.iter().find(|candidate| candidate.operation() == &operation).unwrap();
                let outcome = f.source.terminal_workflow_with_read_probe(candidate, reads.as_ref()).await;
                assert!(matches!(outcome, Ok(WorkflowTerminalRecovery::NotSelected)),
                    "ordinary missing handles must not engage unavailable backoff");
            }
        }
        assert_eq!(reads.reads.load(Ordering::SeqCst), 0,
            "neither committed_continuation nor service_task_snapshot reads a nonselected acquisition");
        assert_eq!(f.store.get(terminal.key()).await.unwrap(), before);
        assert_eq!(f.store.list_effects(terminal.key(), None, 16).await.unwrap(), effects);
        assert_eq!(delivery.dispatches.load(Ordering::SeqCst), 1);
    })).catch_unwind().await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded nonshared selection"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_acknowledged_candidate_reopens_after_unavailable_read() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let deadline = u64::try_from(f.store.now_ms().await.unwrap()).unwrap() + 60_000;
        let c = prepare_with_waiter_options(&f, Some((deadline, true))).await;
        f.source
            .retain_workflow_selection(&c.a, &id("waiter-a-call"), Some(&c.planning), Some(&c.work))
            .unwrap();
        let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
        f.finish(&Ok::<_, String>(admitted)).await.unwrap();
        let terminal = ready(&f, &c).await;
        let operation = id(&terminal.key().id);
        let delivery = Delivery::new(true);
        assert!(matches!(
            f.source
                .workflow_store()
                .unwrap()
                .step_continuation(&operation, delivery.as_ref(), &options())
                .await
                .unwrap(),
            ContinuationStep::Acknowledged(_)
        ));
        let before = f.store.get(terminal.key()).await.unwrap();
        let effects = f
            .store
            .list_effects(terminal.key(), None, 16)
            .await
            .unwrap();
        let reads = CanonicalReads::new(f.store.clone(), terminal.key().clone());
        reads.unavailable.store(true, Ordering::SeqCst);
        let page = f
            .source
            .workflow_store()
            .unwrap()
            .continuation_recovery_page(None, 32)
            .await
            .unwrap();
        assert!(!page.operations.contains(&operation));
        let candidate = page
            .terminal_operations
            .iter()
            .find(|candidate| candidate.operation() == &operation)
            .unwrap();
        assert!(
            matches!(
                f.source
                    .terminal_workflow_with_read_probe(candidate, reads.as_ref())
                    .await,
                Err(PrivateOriginalError::Unavailable)
            ),
            "selected unavailable work must remain delayed"
        );
        assert_eq!(reads.reads.load(Ordering::SeqCst), 1);
        assert!(
            !f.source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(c.membership.workflow())
                .await
                .unwrap()
                .departed(&c.membership)
                .unwrap()
        );
        reads.unavailable.store(false, Ordering::SeqCst);
        let next = f
            .source
            .workflow_store()
            .unwrap()
            .continuation_recovery_page(None, 32)
            .await
            .unwrap();
        assert!(!next.operations.contains(&operation));
        let candidate = next
            .terminal_operations
            .iter()
            .find(|candidate| candidate.operation() == &operation)
            .unwrap();
        assert!(matches!(
            f.source
                .terminal_workflow_with_read_probe(candidate, reads.as_ref())
                .await,
            Ok(WorkflowTerminalRecovery::Departed)
        ));
        assert_eq!(
            reads.reads.load(Ordering::SeqCst),
            3,
            "retry performs both canonical terminal and exact native-task reads"
        );
        assert!(
            f.source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(c.membership.workflow())
                .await
                .unwrap()
                .departed(&c.membership)
                .unwrap()
        );
        assert_eq!(f.store.get(terminal.key()).await.unwrap(), before);
        assert_eq!(
            f.store
                .list_effects(terminal.key(), None, 16)
                .await
                .unwrap(),
            effects
        );
        assert_eq!(delivery.dispatches.load(Ordering::SeqCst), 1);
    }))
    .catch_unwind()
    .await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded selected recovery"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

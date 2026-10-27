//! Actual native B admission and source-owner read controls. The member
//! visibility callback below is an explicit component seam, not native Console
//! authentication. The delivered HTTP handler must supply that final proof.
use super::*;
use crate::private_originals::governed_submit::tests::DeliveryMode;
use std::sync::atomic::{AtomicBool, Ordering};
use toolkit_store::*;

struct ReadProbe {
    inner: Arc<dyn ToolkitStore>,
    hold: AtomicBool,
    unavailable: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    expiry_clock: Option<i64>,
    held_get: Option<StoreKey>,
}
impl ReadProbe {
    fn new(inner: Arc<dyn ToolkitStore>, hold: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            hold: AtomicBool::new(hold),
            unavailable: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            expiry_clock: None,
            held_get: None,
        })
    }
    fn after_source_get(inner: Arc<dyn ToolkitStore>, key: StoreKey) -> Arc<Self> {
        Arc::new(Self { inner, hold: AtomicBool::new(true), unavailable: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
            expiry_clock: None, held_get: Some(key) })
    }
    // Only the expiry writer uses this explicit clock seam. Ordinary status
    // probes below still panic on every admission or update.
    fn expiry_writer(inner: Arc<dyn ToolkitStore>, now: i64) -> Self {
        Self { inner, hold: AtomicBool::new(false), unavailable: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
            expiry_clock: Some(now), held_get: None }
    }
}
#[allow(unused_variables)]
#[async_trait::async_trait]
impl ToolkitStore for ReadProbe {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        match self.expiry_clock { Some(now) => Ok(now), None => self.inner.now_ms().await }
    }
    async fn admit(&self, _request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        panic!("status read must not admit records")
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Worker);
        }
        let real = self.inner.get(key).await;
        if self.held_get.as_ref() == Some(key) && self.hold.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        real
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Worker);
        }
        let real = self.inner.list_records(scope, after, limit).await;
        if self.held_get.is_none() && self.hold.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        real
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        if self.expiry_clock.is_some() { return self.inner.compare_and_swap(transition).await; }
        panic!("status read must not update records")
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        panic!("status read must not mutate effects or claims")
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        panic!("status read must not mutate effects or claims")
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        panic!("status read must not mutate effects or claims")
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        panic!("status read must not mutate effects or claims")
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
        panic!("status read must not mutate effects or claims")
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        panic!("status read must not mutate effects or claims")
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        panic!("status read must not mutate effects or claims")
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        panic!("status read must not mutate effects or claims")
    }
}

async fn prepared(fixture: &ContinuationFixture) -> PrivateOriginalRef {
    let a = fixture.submit().await;
    fixture.finish(&a).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    tokio::time::timeout(
        WAIT,
        fixture
            .machine
            .wait_run_settled(&fixture.session, dispatch.retained_work().unwrap().run_id()),
    )
    .await
    .unwrap();
    let snapshot = acquisition(fixture, &dispatch).await;
    approve_b(fixture, snapshot).await
}

struct CancellationDetail(ContextReference);
impl ContextReferenceOwner for CancellationDetail {
    fn verify_access(&self, value: &ContextReference, evidence: &SetupRequestEvidence,
        context: &SetupUseContext<'_>) -> Result<(), RequestError> {
        evidence.check_context(context)?;
        if value == &self.0 { Ok(()) } else { Err(RequestError::ContextUnavailable) }
    }
}

async fn cancel_setup(fixture: &ContinuationFixture, snapshot: &SetupSnapshot) {
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let evidence = snapshot.request().evidence();
    let constraints = snapshot.request().constraints();
    let context = SetupUseContext {
        tenant: &constraints.tenant, deployment: &constraints.deployment,
        realm: &evidence.association().candidate().ingress_namespace.realm,
        target: &evidence.association().candidate().target,
        policy_revision: &constraints.policy_revision,
        now_ms: u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap(),
        authorization: dispatch.work_authorization().unwrap(),
    };
    let detail = ContextReference { owner: id("service-admission-test"), id: id("status-cancel"),
        revision: revision("1"), content_digest: toolkit_package::ContentDigest::of_bytes(b"cancel") };
    let references = CancellationDetail(detail.clone());
    fixture.source.service_setup_store().unwrap().cancel(&id(&snapshot.key().id), detail,
        meerkat_toolkit::setup::SetupAccess { evidence, context: &context, references: &references })
        .await.unwrap();
}

struct ExpiryOwner;
#[async_trait::async_trait]
impl meerkat_toolkit::setup::SetupDeadlineOwner for ExpiryOwner {
    async fn authorize_expiry(&self, _: &SetupSnapshot) -> Result<(), meerkat_toolkit::setup::SetupError> {
        Ok(())
    }
}

async fn next_acquisition(fixture: &ContinuationFixture, previous: &SetupSnapshot) -> SetupSnapshot {
    named_acquisition(fixture, previous, "status-next-setup").await
}

async fn named_acquisition(fixture: &ContinuationFixture, previous: &SetupSnapshot, correlation: &str) -> SetupSnapshot {
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let evidence = previous.request().evidence();
    let constraints = previous.request().constraints();
    let context = SetupUseContext {
        tenant: &constraints.tenant, deployment: &constraints.deployment,
        realm: &evidence.association().candidate().ingress_namespace.realm,
        target: &evidence.association().candidate().target,
        policy_revision: &constraints.policy_revision,
        now_ms: u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap(),
        authorization: dispatch.work_authorization().unwrap(),
    };
    let mut host = previous.request().host().clone();
    host.correlation = id(correlation);
    let request = validate_request(previous.request().request().clone(), host, evidence, &context, &NoReferences).unwrap();
    fixture.source.service_setup_store().unwrap().admit(&request, evidence, &context, &NoReferences)
        .await.unwrap().snapshot
}

#[derive(Clone, Copy)]
enum HistoricalEnd { SourceCancelled, SetupCancelled, SetupExpired }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_terminal_source_or_setup_does_not_hide_a_later_same_owner_instruction() {
    use meerkat_toolkit::host_lifetime::control::PrivateHostLifetimeControl;
    for ending in [HistoricalEnd::SourceCancelled, HistoricalEnd::SetupCancelled, HistoricalEnd::SetupExpired] {
        let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let old = prepared(&fixture).await;
            let source = &fixture.source;
            let old_row = fixture.store.get(&source.key(&old).unwrap()).await.unwrap().unwrap();
            let old_record = source.decode_record(&old_row, &old).unwrap();
            let old_task = source.service_task_snapshot(&old_record).await.unwrap();
            match ending {
                HistoricalEnd::SourceCancelled => source.record_task_group_cancelled(&old).await.unwrap(),
                HistoricalEnd::SetupCancelled => cancel_setup(&fixture, &old_task).await,
                HistoricalEnd::SetupExpired => {
                    // The generated expiry transition and SQLite commit are real;
                    // only this maintenance writer's clock is a component seam.
                    let clock = ReadProbe::expiry_writer(fixture.store.clone(),
                        i64::try_from(old_task.request().host().limits.deadline_ms).unwrap());
                    let setup = SetupStore::new(&clock, SetupScope::new(id(&fixture.scope.realm), id(&fixture.scope.owner))).unwrap();
                    let expired = setup.expire(&id(&old_task.key().id), &ExpiryOwner).await.unwrap();
                    assert_eq!(expired.state().lifecycle_phase, toolkit_catalog::AcquisitionPhase::Expired);
                }
            }
            // Another actual protected approval under this same source, human,
            // member, native parent and installed owner follows the old result.
            let next = next_acquisition(&fixture, &old_task).await;
            assert_ne!(next.key(), old_task.key());
            let live = approve_b_call(&fixture, next, [id("read")].into(), None, "status-later-approval").await;
            let live_row = fixture.store.get(&source.key(&live).unwrap()).await.unwrap().unwrap();
            let live_record = source.decode_record(&live_row, &live).unwrap();
            assert!(live_record.source == old_record.source);
            assert!(live_record.approved_task.as_ref().unwrap().parent == old_record.approved_task.as_ref().unwrap().parent);
            let link = live_record.approved_task.as_ref().unwrap();
            let live_id = digest(&canonical_json(&(link.acquisition.clone(), &link.original_task, &live)).unwrap());
            let human = source.route(&live.route).unwrap().requester.clone();
            let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
            let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
            let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
            let requests = fixture.model_requests();
            let probe = ReadProbe::new(fixture.store.clone(), false);
            for _ in 0..3 {
                let projection = source.service_task_status_with_read_probe(probe.as_ref(), &human, &visible).await.unwrap();
                assert_eq!(projection["tasks"].as_array().unwrap().len(), 1);
                assert_eq!(projection["tasks"][0]["id"], live_id);
                assert_eq!(projection["tasks"][0]["phase"]["text"], "Setup handoff status has not been reported");
                assert_eq!(fixture.inputs().await, Ok(1), "status does not submit either approved instruction");
            }
            assert_eq!(fixture.model_requests(), requests);
            assert_eq!(fixture.store.list_records(&fixture.scope, None, 256).await.unwrap(), before);
            assert!(matches!(source.service_task_status_projection(&human, &|_| Ok(false)).await,
                Err(PrivateOriginalError::Denied)));
            probe.unavailable.store(true, Ordering::SeqCst);
            assert!(matches!(source.service_task_status_with_read_probe(probe.as_ref(), &human, &visible).await,
                Err(PrivateOriginalError::Unavailable)));
            probe.unavailable.store(false, Ordering::SeqCst);

            if matches!(ending, HistoricalEnd::SetupCancelled) {
                // A failed canonical write leaves the existing source owner
                // CancellationPending even though this acquisition is terminal.
                let row = Arc::new(fixture.store.get(&source.key(&old).unwrap()).await.unwrap().unwrap());
                let record = source.decode_record(&row, &old).unwrap();
                let cancelled = source.owner.record_linked_operations_cancelled(
                    || async move { Ok(vec![crate::original_ingress::OriginalCancellationSource {
                        realm: record.source.realm,
                        original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                            authority: record.source.source_authority, work: record.event.original,
                        }, association: record.final_association, record: row,
                    }]) },
                    |_| async { Err(OriginalWriterError::Unavailable) },
                ).await;
                assert!(matches!(cancelled, Err(OriginalWriterError::Unavailable)));
                assert!(matches!(source.service_task_status_projection(&human, &visible).await,
                    Err(PrivateOriginalError::Unavailable)));
                let pending = fixture.store.get(&source.key(&old).unwrap()).await.unwrap().unwrap();
                let original = source.decode_record(&pending, &old).unwrap();
                let mut altered = original.clone();
                altered.approved_task.as_mut().unwrap().header_mut().before_ms -= 1;
                let changed = fixture.store.compare_and_swap(Transition {
                    key: pending.key, expected_revision: pending.revision,
                    expected_digest: Some(pending.content_digest), claim: None,
                    data: encode(altered).unwrap(), events: vec![], effects: vec![],
                }).await.unwrap();
                assert!(matches!(source.service_task_status_projection(&human, &visible).await,
                    Err(PrivateOriginalError::Conflict)), "unbound cancellation retains its exact source origin");
                fixture.store.compare_and_swap(Transition {
                    key: changed.key, expected_revision: changed.revision,
                    expected_digest: Some(changed.content_digest), claim: None,
                    data: encode(original).unwrap(), events: vec![], effects: vec![],
                }).await.unwrap();
                assert!(matches!(source.service_task_status_projection(&human, &visible).await,
                    Err(PrivateOriginalError::Unavailable)), "restoring origin does not settle pending custody");
                source.record_task_group_cancelled(&old).await.unwrap();
                assert_eq!(source.service_task_status_projection(&human, &visible).await.unwrap()["tasks"].as_array().unwrap().len(), 1);
            }

            if matches!(ending, HistoricalEnd::SourceCancelled) {
                // A durable parent flag while its retained owner is still Open
                // is not a settled terminal observation. The ordinary group
                // writer below must join that publication before omission.
                let parent = fixture.store.get(&source.key(&link.parent).unwrap()).await.unwrap().unwrap();
                source.persist_source_cancellation(Arc::new(parent)).await.unwrap();
                assert!(matches!(source.service_task_status_projection(&human, &visible).await,
                    Err(PrivateOriginalError::Conflict)));
            }

            // Settled cancellation of the exact parent also withholds its two
            // children without turning the status endpoint into permanent 503.
            source.record_task_group_cancelled(&link.parent).await.unwrap();
            let ended = source.service_task_status_projection(&human, &visible).await.unwrap();
            assert_eq!(ended["tasks"].as_array().unwrap().len(), 0);
            // Terminal omission does not turn malformed historical correlation
            // into an accepted empty list.
            let stored = fixture.store.get(&source.key(&old).unwrap()).await.unwrap().unwrap();
            let mut malformed = source.decode_record(&stored, &old).unwrap();
            malformed.approved_task.as_mut().unwrap().header_mut().original_task.origin_session_id = meerkat_core::SessionId::new();
            fixture.store.compare_and_swap(Transition { key: stored.key, expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest), claim: None, data: encode(malformed).unwrap(),
                events: vec![], effects: vec![] }).await.unwrap();
            assert!(matches!(source.service_task_status_projection(&human, &visible).await,
                Err(PrivateOriginalError::Conflict)));
            fixture.lifetime().host_lost().unwrap();
            assert!(matches!(source.service_task_status_projection(&human, &visible).await,
                Err(PrivateOriginalError::Denied)));
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), requests);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_terminal_acquisition_snapshot_is_omitted_but_later_cancellation_refuses() {
    for (expire, observed_active) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let old = prepared(&fixture).await;
            let source = &fixture.source;
            let old_row = fixture.store.get(&source.key(&old).unwrap()).await.unwrap().unwrap();
            let old_record = source.decode_record(&old_row, &old).unwrap();
            let task = source.service_task_snapshot(&old_record).await.unwrap();
            let next = next_acquisition(&fixture, &task).await;
            let live = approve_b_call(&fixture, next, [id("read")].into(), None, "status-race-sibling").await;
            let human = source.route(&live.route).unwrap().requester.clone();
            let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
            let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
            let before = source.service_task_status_projection(&human, &visible).await.unwrap();
            assert_eq!(before["tasks"].as_array().unwrap().len(), 2);
            let requests = fixture.model_requests();
            // The first source get occurs inside status_records_current,
            // after this exact acquisition was already observed active. The
            // initial list pause is earlier than its first canonical snapshot.
            let paused = if observed_active {
                ReadProbe::after_source_get(fixture.store.clone(), source.key(&old).unwrap())
            } else { ReadProbe::new(fixture.store.clone(), true) };
            let read = source.service_task_status_with_read_probe(paused.as_ref(), &human, &visible);
            tokio::pin!(read);
            tokio::select! { _ = paused.entered.notified() => {}, result = &mut read => panic!("read did not hold: {result:?}") }
            if expire {
                let clock = ReadProbe::expiry_writer(fixture.store.clone(),
                    i64::try_from(task.request().host().limits.deadline_ms).unwrap());
                SetupStore::new(&clock, SetupScope::new(id(&fixture.scope.realm), id(&fixture.scope.owner))).unwrap()
                    .expire(&id(&task.key().id), &ExpiryOwner).await.unwrap();
            } else { cancel_setup(&fixture, &task).await; }
            paused.release.add_permits(1);
            let during = read.await;
            let current = source.service_task_status_projection(&human, &visible).await.unwrap();
            let live_row = fixture.store.get(&source.key(&live).unwrap()).await.unwrap().unwrap();
            let record = source.decode_record(&live_row, &live).unwrap();
            let link = record.approved_task.as_ref().unwrap();
            let live_id = digest(&canonical_json(&(link.acquisition.clone(), &link.original_task, &live)).unwrap());
            if observed_active {
                assert!(matches!(during, Err(PrivateOriginalError::Denied)),
                    "an acquisition already observed active keeps its final cancellation check");
            } else {
                let during = during.unwrap();
                assert_eq!(during["tasks"].as_array().unwrap().len(), 1);
                assert_eq!(during["tasks"][0]["id"], live_id,
                    "a first terminal snapshot can omit only that task after exact rereads");
            }
            assert_eq!(current["tasks"].as_array().unwrap().len(), 1);
            assert_eq!(current["tasks"][0]["id"], live_id);
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), requests);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_publication_is_readable_but_terminal_bound_association_mismatch_refuses() {
    use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::LoseSecondAcceptedAnswer).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = prepared(&fixture).await;
        let source = &fixture.source;
        let human = source.route(&b.route).unwrap().requester.clone();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
        let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
        let stored = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        // Actual native admission with a lost source answer retains Bound/Open
        // custody while the acquisition is still Preparing, not Ready.
        assert!(matches!(source.step_service_input(&stored).await, Err(PrivateOriginalError::Unavailable)));
        assert_eq!(fixture.inputs().await, Ok(2));
        let held = source.retained.lock().unwrap().iter().find(|held| held.reference == b).unwrap().clone();
        assert_eq!(held.unresolved.lock().unwrap().len(), 1);
        let before = source.service_task_status_projection(&human, &visible).await.unwrap();
        assert_eq!(before["tasks"][0]["phase"]["text"], "Waiting for the setup handoff outcome");
        let paused = ReadProbe::new(fixture.store.clone(), true);
        let read = source.service_task_status_with_read_probe(paused.as_ref(), &human, &visible);
        tokio::pin!(read);
        tokio::select! { _ = paused.entered.notified() => {}, result = &mut read => panic!("read did not hold: {result:?}") }
        let publication = source.owner.publication();
        let (_, old_stamp) = publication.observe(|| ()).unwrap();
        // Explicit publication seam: invalidate a global observation without
        // changing this source, acquisition, owner disposition or viewer.
        drop(publication.begin_owner_change().unwrap());
        assert!(old_stamp.check_current().is_err());
        paused.release.add_permits(1);
        assert_eq!(read.await.unwrap(), before);

        let stored = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&stored, &b).unwrap();
        let task = source.service_task_snapshot(&record).await.unwrap();
        assert_eq!(task.state().lifecycle_phase, toolkit_catalog::AcquisitionPhase::Preparing);
        cancel_setup(&fixture, &task).await;
        assert_eq!(source.service_task_status_projection(&human, &visible).await.unwrap()["tasks"].as_array().unwrap().len(), 0);
        let mut altered = record.clone();
        let retained = altered.final_association.as_ref().unwrap();
        assert!(retained == held.original.association());
        let mut candidate = retained.candidate().clone();
        candidate.target.context_generation += 1;
        altered.final_association = Some(InputAuthorityAssociation::new(candidate).unwrap());
        let changed = fixture.store.compare_and_swap(Transition {
            key: stored.key, expected_revision: stored.revision,
            expected_digest: Some(stored.content_digest), claim: None,
            data: encode(altered.clone()).unwrap(), events: vec![], effects: vec![],
        }).await.unwrap();
        // A changed association alone disagrees with the actually retained
        // Input and must still be refused by the canonical source decoder.
        assert!(matches!(source.decode_record(&changed, &b), Err(PrivateOriginalError::Conflict)));
        assert!(matches!(source.service_task_status_projection(&human, &visible).await,
            Err(PrivateOriginalError::Conflict)));
        // Keep the tampered row internally consistent so the next refusal must
        // reach the retained native owner instead of stopping at Input equality.
        let meerkat_runtime::Input::Prompt(mut input) = altered.bound_service_input
            .as_ref().unwrap().decode().unwrap() else { panic!("expected retained prompt"); };
        assert!(input.header.authority_association.as_ref() == Some(held.original.association()));
        input.header.authority_association = altered.final_association.clone();
        altered.bound_service_input = Some(serde_json::from_value(
            serde_json::to_value(meerkat_runtime::Input::Prompt(input)).unwrap(),
        ).unwrap());
        let changed = fixture.store.compare_and_swap(Transition {
            key: changed.key, expected_revision: changed.revision,
            expected_digest: Some(changed.content_digest), claim: None,
            data: encode(altered).unwrap(), events: vec![], effects: vec![],
        }).await.unwrap();
        // This is a well-formed association for the same qualified event. Its
        // disagreement is with the existing retained owner, not JSON shape or
        // the A acquisition/contributor join checked before terminal omission.
        assert!(source.decode_record(&changed, &b).is_ok());
        assert!(matches!(source.service_task_status_projection(&human, &visible).await,
            Err(PrivateOriginalError::Conflict)));
        fixture.store.compare_and_swap(Transition {
            key: changed.key, expected_revision: changed.revision,
            expected_digest: Some(changed.content_digest), claim: None,
            data: encode(record).unwrap(), events: vec![], effects: vec![],
        }).await.unwrap();
        assert_eq!(source.service_task_status_projection(&human, &visible).await.unwrap()["tasks"].as_array().unwrap().len(), 0);
        assert_eq!(held.unresolved.lock().unwrap().len(), 1, "status does not settle unknown source custody");
        assert_eq!(fixture.inputs().await, Ok(2));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_admitted_b_pending_and_permanent_publication_are_read_without_replay() {
    for (mode, expected) in [
        (
            DeliveryMode::LoseSecondAcceptedAnswer,
            "Waiting for the setup handoff outcome",
        ),
        (
            DeliveryMode::PoisonPublicationAfterSecondAcceptance,
            "Setup handoff needs attention",
        ),
        (DeliveryMode::Normal, "Setup handoff confirmed"),
    ] {
        let fixture = ContinuationFixture::for_source_status(mode).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let b = prepared(&fixture).await;
            let source = &fixture.source;
            let human = source.route(&b.route).unwrap().requester.clone();
            let member = fixture
                .lifetime()
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap()
                .members[0]
                .member
                .clone();
            let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
            let probe = ReadProbe::new(fixture.store.clone(), false);
            for _ in 0..3 {
                let before = source
                    .service_task_status_with_read_probe(probe.as_ref(), &human, &visible)
                    .await
                    .unwrap();
                assert_eq!(
                    before["tasks"][0]["phase"]["text"],
                    "Setup handoff status has not been reported"
                );
                assert_eq!(fixture.inputs().await, Ok(1));
            }
            let key = source.key(&b).unwrap();
            let stored = fixture.store.get(&key).await.unwrap().unwrap();
            let admitted = source.step_service_input(&stored).await;
            if matches!(mode, DeliveryMode::Normal) {
                assert!(admitted.is_ok());
            } else {
                assert!(matches!(admitted, Err(PrivateOriginalError::Unavailable)));
            }
            assert_eq!(
                fixture.inputs().await,
                Ok(2),
                "actual native B admission precedes either source outcome"
            );
            let exact_before = fixture
                .store
                .list_records(&fixture.scope, None, 256)
                .await
                .unwrap();
            let held = source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.reference == b)
                .unwrap()
                .clone();
            let pending_count = if matches!(mode, DeliveryMode::Normal) {
                0
            } else {
                1
            };
            assert_eq!(held.unresolved.lock().unwrap().len(), pending_count);
            let mut last = None;
            for _ in 0..3 {
                let projection = source
                    .service_task_status_with_read_probe(probe.as_ref(), &human, &visible)
                    .await
                    .unwrap();
                assert_eq!(projection["tasks"].as_array().unwrap().len(), 1);
                assert_eq!(projection["tasks"][0]["phase"]["text"], expected);
                assert_eq!(projection["tasks"][0]["controls"], serde_json::json!([]));
                assert_eq!(fixture.inputs().await, Ok(2));
                assert_eq!(
                    held.unresolved.lock().unwrap().len(),
                    pending_count,
                    "reads never consume source custody"
                );
                last = Some(projection);
            }
            assert_eq!(
                fixture
                    .store
                    .list_records(&fixture.scope, None, 256)
                    .await
                    .unwrap(),
                exact_before
            );
            probe.unavailable.store(true, Ordering::SeqCst);
            for _ in 0..3 {
                assert!(
                    matches!(
                        source
                            .service_task_status_with_read_probe(probe.as_ref(), &human, &visible)
                            .await,
                        Err(PrivateOriginalError::Unavailable)
                    ),
                    "transient store failure is not a permanent publication result or empty list"
                );
                assert_eq!(fixture.inputs().await, Ok(2));
            }
            probe.unavailable.store(false, Ordering::SeqCst);
            let current = source
                .service_task_status_with_read_probe(probe.as_ref(), &human, &visible)
                .await
                .unwrap();
            assert_eq!(current, last.unwrap());
            if matches!(mode, DeliveryMode::PoisonPublicationAfterSecondAcceptance) {
                if let Some(path) = std::env::var_os("TOOLKIT_SOURCE_STATUS_NATIVE_FIXTURE") {
                    std::fs::write(path, canonical_json(&current).unwrap()).unwrap();
                }
            }
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
async fn actual_source_cancellation_during_held_read_withholds_the_instruction() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = prepared(&fixture).await;
        let source = &fixture.source;
        let human = source.route(&b.route).unwrap().requester.clone();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
        let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
        let foreign = PrincipalRef::in_domain(meerkat_core::PrincipalKind::Human, "another-subject",
            meerkat_core::auth::principal::TrustDomainId::new("oidc-issuer:https://other.example").unwrap()).unwrap();
        assert!(matches!(source.service_task_status_projection(&foreign, &visible).await,
            Err(PrivateOriginalError::Denied)), "member visibility alone does not disclose another human's task");
        assert!(matches!(source.service_task_status_projection(&human, &|_| Ok(false)).await,
            Err(PrivateOriginalError::Denied)), "source identity without member access also refuses");
        // This mutable callback is only a component access check. The actual
        // HTTP route must obtain fresh native access from the viewer resolver.
        let allowed = AtomicBool::new(true);
        let component_access = |candidate: &meerkat_core::MobMemberBinding|
            Ok(candidate == &member && allowed.load(Ordering::SeqCst));
        let revoked = ReadProbe::new(fixture.store.clone(), true);
        let read = source.service_task_status_with_read_probe(revoked.as_ref(), &human, &component_access);
        tokio::pin!(read);
        tokio::select! { _ = revoked.entered.notified() => {}, result = &mut read => panic!("read did not hold: {result:?}") }
        allowed.store(false, Ordering::SeqCst);
        revoked.release.add_permits(1);
        assert!(matches!(read.await, Err(PrivateOriginalError::Denied)));
        // The page decoder uses the same live component visibility checks
        // after canonical awaits, even though its source slice is preselected.
        let page = source.source_status_page(None).await.unwrap();
        allowed.store(true, Ordering::SeqCst);
        let page_probe = ReadProbe::after_source_get(fixture.store.clone(), source.key(&b).unwrap());
        let page_read = source.service_task_status_page_with_read_probe(page_probe.as_ref(), &human, &component_access, &page.rows);
        tokio::pin!(page_read);
        tokio::select! { _ = page_probe.entered.notified() => {}, result = &mut page_read => panic!("page read did not hold: {result:?}") }
        allowed.store(false, Ordering::SeqCst); page_probe.release.add_permits(1);
        assert!(matches!(page_read.await, Err(PrivateOriginalError::Denied)));
        // Positive: same actual source and same member check across a held read.
        let unchanged = ReadProbe::new(fixture.store.clone(), true);
        let read = source.service_task_status_with_read_probe(unchanged.as_ref(), &human, &visible);
        tokio::pin!(read);
        tokio::select! { _ = unchanged.entered.notified() => {}, result = &mut read => panic!("read did not hold: {result:?}") }
        unchanged.release.add_permits(1);
        let positive = read.await.unwrap();
        assert_eq!(positive["tasks"].as_array().unwrap().len(), 1);
        // Negative: cancel A through the existing canonical group writer after
        // the real SQLite page read, before the reader sees its answer.
        let paused = ReadProbe::new(fixture.store.clone(), true);
        let read = source.service_task_status_with_read_probe(paused.as_ref(), &human, &visible);
        tokio::pin!(read);
        tokio::select! { _ = paused.entered.notified() => {}, result = &mut read => panic!("read did not hold: {result:?}") }
        let stored = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let parent = source.decode_record(&stored, &b).unwrap().approved_task.unwrap().parent.clone();
        source.record_task_group_cancelled(&parent).await.unwrap();
        paused.release.add_permits(1);
        assert!(matches!(read.await, Err(PrivateOriginalError::Denied)));
        assert_eq!(fixture.inputs().await, Ok(1), "status reads do not admit B");
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_service_reservation_during_status_read_preserves_the_current_instruction() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = prepared(&fixture).await;
        let source = &fixture.source;
        let human = source.route(&b.route).unwrap().requester.clone();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap()
            .members[0].member.clone();
        let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
        let stored = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&stored, &b).unwrap();
        let before_task = source.service_task_snapshot(&record).await.unwrap();
        assert!(before_task.approved_service().is_none());
        let before = source.service_task_status_projection(&human, &visible).await.unwrap();
        let requests = fixture.model_requests();

        // Hold the actual read after it observed this task and fetched its
        // source row. The production reservation advances only the canonical
        // acquisition; it does not admit B or rewrite the approved request.
        let paused = ReadProbe::after_source_get(fixture.store.clone(), stored.key.clone());
        let read = source.service_task_status_with_read_probe(paused.as_ref(), &human, &visible);
        tokio::pin!(read);
        tokio::time::timeout(WAIT, async {
            tokio::select! {
                _ = paused.entered.notified() => {},
                result = &mut read => panic!("status read did not hold: {result:?}"),
            }
        }).await.expect("bounded held status read");
        let reserved = source.reserve_service_continuation(&stored).await;
        paused.release.add_permits(1);
        let during = tokio::time::timeout(WAIT, &mut read).await;
        reserved.unwrap();
        let after_task = source.service_task_snapshot(&record).await.unwrap();
        assert_ne!(after_task.condition(), before_task.condition());
        assert!(after_task.approved_service() == Some(&source.service_selection(&stored, &record).unwrap()));
        assert!(!after_task.approved_service_was_admitted());
        assert_eq!(after_task.state().lifecycle_phase, toolkit_catalog::AcquisitionPhase::Preparing);
        assert_eq!(after_task.request().input_digest(), before_task.request().input_digest());
        assert_eq!(during.expect("bounded released status read").unwrap(), before);
        // An already captured pre-reservation snapshot remains truthful. A
        // fresh read projects only the exact canonical reservation; identity,
        // approved instruction and all controls remain unchanged.
        let mut expected = before.clone();
        expected["tasks"][0]["phase"]["text"] = serde_json::json!("Access approved; setup handoff pending");
        expected["tasks"][0]["nextAction"] = serde_json::json!(
            "Setup retained the approved instruction. Connection readiness and delivery to your agent are not yet confirmed.");
        assert_eq!(source.service_task_status_projection(&human, &visible).await.unwrap(), expected);
        assert_eq!(fixture.store.get(&stored.key).await.unwrap().unwrap(), stored);
        assert_eq!(fixture.inputs().await, Ok(1), "status and reservation do not admit B");
        assert_eq!(fixture.model_requests(), requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_source_publication_during_status_read_preserves_the_current_instruction() {
    for (mode, expected) in [
        (DeliveryMode::Normal, "Setup handoff confirmed"),
        (DeliveryMode::LoseSecondAcceptedAnswer, "Waiting for the setup handoff outcome"),
    ] {
        let fixture = ContinuationFixture::for_source_status(mode).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let b = prepared(&fixture).await;
            let source = &fixture.source;
            let human = source.route(&b.route).unwrap().requester.clone();
            let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap()
                .members[0].member.clone();
            let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
            let stored = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
            let before = source.decode_record(&stored, &b).unwrap();
            assert!(before.final_association.is_none());
            assert!(before.service_admission.is_none());
            let prior = source.service_task_status_projection(&human, &visible).await.unwrap();

            // The real page contains the unbound source. Admit B through the
            // existing owner while that awaited page read is held, then let
            // the reader reopen and validate the actual updated source.
            let paused = ReadProbe::new(fixture.store.clone(), true);
            let read = source.service_task_status_with_read_probe(paused.as_ref(), &human, &visible);
            tokio::pin!(read);
            tokio::time::timeout(WAIT, async {
                tokio::select! {
                    _ = paused.entered.notified() => {},
                    result = &mut read => panic!("status read did not hold: {result:?}"),
                }
            }).await.expect("bounded held status read");
            let admitted = source.step_service_input(&stored).await;
            paused.release.add_permits(1);
            let during = tokio::time::timeout(WAIT, &mut read).await;
            if matches!(mode, DeliveryMode::Normal) { admitted.unwrap(); }
            else { assert!(matches!(admitted, Err(PrivateOriginalError::Unavailable))); }
            let after = fixture.store.get(&stored.key).await.unwrap().unwrap();
            assert_ne!(after, stored);
            let current = source.decode_record(&after, &b).unwrap();
            assert!(current.final_association.is_some());
            assert_eq!(current.service_admission.is_some(), matches!(mode, DeliveryMode::Normal));
            let projection = during.expect("bounded released status read").unwrap();
            assert_eq!(projection["tasks"].as_array().unwrap().len(), 1);
            assert_eq!(projection["tasks"][0]["id"], prior["tasks"][0]["id"]);
            assert_eq!(projection["tasks"][0]["reason"], prior["tasks"][0]["reason"]);
            assert_eq!(projection["tasks"][0]["phase"]["text"], expected);
            assert_eq!(source.service_task_status_projection(&human, &visible).await.unwrap(), projection);
            assert_eq!(fixture.store.get(&stored.key).await.unwrap().unwrap(), after,
                "status observation does not write the source");
            assert_eq!(fixture.inputs().await, Ok(2), "status observation does not replay B");
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatched_task_correlation_is_not_a_disclosure_capability() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = prepared(&fixture).await;
        let source = &fixture.source;
        let human = source.route(&b.route).unwrap().requester.clone();
        let member = fixture
            .lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .members[0]
            .member
            .clone();
        let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
        let key = source.key(&b).unwrap();
        let stored = fixture.store.get(&key).await.unwrap().unwrap();
        let mut record = source.decode_record(&stored, &b).unwrap();
        // Corrupt only the retained task correlation in the actual test store.
        // It cannot confer the other session's disclosure or input authority.
        record
            .approved_task
            .as_mut()
            .unwrap()
            .header_mut()
            .original_task
            .origin_session_id = meerkat_core::SessionId::new();
        fixture
            .store
            .compare_and_swap(Transition {
                key: stored.key,
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest),
                claim: None,
                data: encode(record).unwrap(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let probe = ReadProbe::new(fixture.store.clone(), false);
        assert!(matches!(
            source
                .service_task_status_with_read_probe(probe.as_ref(), &human, &visible)
                .await,
            Err(PrivateOriginalError::Conflict)
        ));
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
async fn application_access_reads_retained_sources_and_revokes_only_the_exact_issued_leaf() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let first = prepared(&fixture).await;
        let source = &fixture.source;
        let stored = fixture.store.get(&source.key(&first).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&stored, &first).unwrap();
        let task = source.service_task_snapshot(&record).await.unwrap();
        let second = approve_b_call(&fixture, next_acquisition(&fixture, &task).await,
            [id("read")].into(), None, "access-second-approval").await;
        let human = source.route(&first.route).unwrap().requester.clone();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
        let probe = ReadProbe::new(fixture.store.clone(), false);
        let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        let requests = fixture.model_requests();
        let read = source.read_application_access_from(probe.as_ref(), &human, &[member.clone()]).await.unwrap();
        assert_eq!(read.rows.len(), 2);
        assert!(read.check_visible(|candidate| candidate == &member).is_ok());
        assert!(matches!(read.check_visible(|_| false), Err(PrivateOriginalError::Denied)),
            "the final disclosure gate used by the binding rechecks visibility after store awaits");
        assert!(read.rows.iter().all(|row| row.projection["state"] == "grant_issued"));
        assert_ne!(read.rows[0].id, read.rows[1].id);
        assert!(source.read_application_access_from(probe.as_ref(), &human, &[]).await.unwrap().rows.is_empty());
        let other = meerkat_core::PrincipalRef::in_domain(meerkat_core::PrincipalKind::Human, "other-person",
            meerkat_core::TrustDomainId::new("other-console").unwrap()).unwrap();
        assert!(source.read_application_access_from(probe.as_ref(), &other, &[member.clone()]).await.unwrap().rows.is_empty());
        let (_pipe, logins) = crate::private_host::ConsoleLogins::fixture_owner();
        let login = logins.fixture_enroll(2, crate::private_host::ConsoleLoginSelection {
            issuer:"https://console.example".into(), subject:"actual-component-selection".into(), login_scope_id:"access".into(),
            expires_at_ms:u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap() + 60_000,
        }).unwrap();
        // The native source/issuer/leaf and installed phase are real. This
        // mechanical login fixture is not a synthetic native Console Human.
        source.recheck_application_access(&read, &read.rows[0]).await.unwrap();
        let lifetime = fixture.lifetime();
        let blocked = logins.fixture_enroll(3, crate::private_host::ConsoleLoginSelection {
            issuer:"https://console.example".into(), subject:"actual-component-selection".into(), login_scope_id:"blocked-access".into(),
            expires_at_ms:u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap() + 60_000,
        }).unwrap();
        let reservation = blocked.reserve().unwrap();
        let held_mutation = fixture.machine.try_controller_grant_mutation().unwrap();
        let refused_before_entry = lifetime.with_current_setup(|| blocked.with_current_revoke(||
            source.revoke_application_access_now(&read, &read.rows[0]))
            .map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied)).unwrap();
        drop(held_mutation);
        assert!(matches!(refused_before_entry, Err(PrivateOriginalError::Unavailable)),
            "mutation-custody contention has no actual native revoke result");
        let terminal = crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(reservation,
            refused_before_entry.map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable));
        let retired = logins.fixture_retire(3).unwrap().await.unwrap();
        assert!(terminal.is_err()); assert!(retired.reservations_drained); assert!(!retired.uncertain);
        source.recheck_application_access(&read, &read.rows[0]).await.unwrap();
        let reservation = login.reserve().unwrap();
        let revoked = lifetime.with_current_setup(|| login.with_current_revoke(||
            source.revoke_application_access_now(&read, &read.rows[0]))
            .map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied)).unwrap();
        let succeeded = matches!(&revoked, Ok(Ok(())));
        let terminal = crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(reservation,
            revoked.map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable));
        let retired = logins.fixture_retire(2).unwrap().await.unwrap();
        assert!(succeeded);
        assert_eq!(terminal.unwrap()["state"], "revoked");
        assert!(retired.reservations_drained); assert!(!retired.uncertain);
        assert!(source.recheck_application_access(&read, &read.rows[1]).await.is_err(), "old offers are never relabelled after mutation");
        let current = source.read_application_access_from(probe.as_ref(), &human, &[member.clone()]).await.unwrap();
        assert_eq!(current.rows.iter().find(|row| row.id == read.rows[0].id).unwrap().projection["state"], "grant_unavailable");
        assert_eq!(current.rows.iter().find(|row| row.id == read.rows[1].id).unwrap().projection["state"], "grant_issued");
        assert!(current.rows.iter().all(|row| row.projection["state"] != "revoked"), "only the actual action reports successful revocation");
        assert_eq!(fixture.store.list_records(&fixture.scope, None, 256).await.unwrap(), before);
        assert_eq!(fixture.inputs().await, Ok(1)); assert_eq!(fixture.model_requests(), requests);
        // Terminal setup history is attribution evidence, not task liveness.
        cancel_setup(&fixture, &task).await;
        let history = source.read_application_access_from(probe.as_ref(), &human, &[member]).await.unwrap();
        assert_eq!(history.rows.len(), 2);
        assert_ne!(first.original, second.original);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

// Schedule only inside the real native custody callback. This wrapper cannot
// permit a reference that its actual machine custody refuses.
struct BeforeGrantMutation<'a, C, F> {
    inner: &'a mut C,
    before: F,
    requested: Vec<meerkat_authorization_contracts::grant::GrantLineageRef>,
    entered: usize,
}
impl<C, F> meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for BeforeGrantMutation<'_, C, F>
where
    C: meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    F: FnMut(),
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self, reference: &meerkat_authorization_contracts::grant::GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal> {
        let Self { inner, before, requested, entered } = self;
        requested.push(reference.clone());
        inner.with_unreferenced_controller_grant(reference, || {
            *entered += 1;
            before();
            mutate()
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_access_policy_swap_refuses_revoke_then_fresh_review_revokes_only_selected_leaf() {
    use meerkat_authorization::grants::ExpectedRevokeRefusal;
    use meerkat_authorization::publication::PublicationError;
    use meerkat_authorization_contracts::work_association::OriginalWorkRef;
    use meerkat_mobkit::{AccessControlConfig, AccessController, AccessGroup, AccessRule};
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let first = prepared(&fixture).await;
        let source = &fixture.source;
        let stored = fixture.store.get(&source.key(&first).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&stored, &first).unwrap();
        let task = source.service_task_snapshot(&record).await.unwrap();
        let second = approve_b_call(&fixture, next_acquisition(&fixture, &task).await,
            [id("read")].into(), None, "access-policy-swap-second").await;
        let route = source.route(&first.route).unwrap();
        let human = route.requester.clone();
        let subject = human.id.as_str().to_owned();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
        let identity = source.bound_identity_runtime().unwrap()
            .owned_identity_for_member_alias(route.grant_target.as_str()).await.unwrap();
        let mut lineages = Vec::new();
        for original in [&first, &second] {
            let stored = fixture.store.get(&source.key(original).unwrap()).await.unwrap().unwrap();
            let record = source.decode_record(&stored, original).unwrap();
            lineages.push(source.service_grant_lineage(&record.approved_task.as_ref().unwrap().singleton_grant().unwrap(),
                &OriginalWorkRef { authority: record.source.source_authority.clone(),
                    work: original.original.clone() }).await.unwrap());
        }
        assert_ne!(lineages[0].last(), lineages[1].last());
        let live = |lineage: &Vec<meerkat_authorization_contracts::grant::GrantLineageRef>| {
            source.owner.grants().resolve_lineage(lineage, &route.logical_executor,
                route.represented_subject.as_ref()).is_ok_and(|resolved| resolved.check_current().is_ok())
        };
        let live_before = lineages.iter().map(&live).collect::<Vec<_>>();

        // This private controller is joined before exposing any of its views
        // or writers. The actual route's subject is not the configured admin.
        // It is a component subject, not a fabricated native Console carrier.
        let original_group = AccessGroup { members: vec![subject.clone()], ..Default::default() };
        let controller = AccessController::new(AccessControlConfig {
            enabled: true, admins: vec![format!("other-admin:{subject}")],
            groups: [("selected-requester".into(), original_group.clone())].into(),
            rules: vec![AccessRule { id: "selected-member-view".into(),
                groups: vec!["selected-requester".into()], actions: vec!["agent.view".into()],
                agents: vec![identity.as_str().into()], ..Default::default() }],
            ..Default::default()
        }).unwrap();
        let publication = source.authorization_publication();
        controller.join_authorization_publication(publication.clone()).unwrap();
        let (_pipe, logins) = crate::private_host::ConsoleLogins::fixture_owner();
        let login = logins.fixture_enroll(4, crate::private_host::ConsoleLoginSelection {
            issuer: "https://console.example".into(), subject: subject.clone(), login_scope_id: "policy-swap".into(),
            expires_at_ms: u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap() + 60_000,
        }).unwrap();
        let (allowed, allowed_stamp) = publication.observe(|| controller.view_for_subject(Some(&subject))).unwrap();
        let read = source.read_application_access(&human, &[member.clone()]).await.unwrap();
        assert_eq!(read.rows.len(), 2);
        assert!(allowed.enforced() && allowed.can_view_agent(identity.as_str()));
        assert_eq!(allowed.subject(), Some(subject.as_str()));
        allowed_stamp.check_current().unwrap();
        read.check_visible(|candidate| candidate == &member && allowed.can_view_agent(identity.as_str())).unwrap();
        source.recheck_application_access(&read, &read.rows[0]).await.unwrap();
        let selected_id = read.rows[0].id.clone();
        let sibling_id = read.rows[1].id.clone();
        let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        let requests = fixture.model_requests();
        let lifetime = fixture.lifetime();
        let mut update = None;
        let mut old_stamp_after_swap = None;
        let mut denied = None;
        let mut denied_current = None;

        let reservation = login.reserve().unwrap();
        let mut actual_custody = fixture.machine.try_controller_grant_mutation().unwrap();
        let mut scheduled = BeforeGrantMutation { inner: &mut actual_custody, requested: vec![], entered: 0,
            before: || {
                // set_group returns after the real visible swap and all of its
                // publication/state/mutation guards have been released. Do not
                // add another Toolkit precheck or prevent the native callback.
                update = Some(controller.set_group("selected-requester", AccessGroup::default()));
                old_stamp_after_swap = Some(read.publication.check_current());
                denied = Some(publication.observe(|| controller.view_for_subject(Some(&subject))));
                denied_current = denied.as_ref().and_then(|observed| observed.as_ref().ok())
                    .map(|(_, stamp)| stamp.check_current());
            } };
        let raced = lifetime.with_current_setup(|| login.with_current_revoke(||
            source.revoke_application_access_with_test_custody(&read, &read.rows[0], &mut scheduled))
            .map_err(|_| HostLifetimeError::Denied));
        let requested = scheduled.requested.clone();
        let callback_entries = scheduled.entered;
        drop(scheduled);
        drop(actual_custody);
        let native_result = raced.as_ref().ok().and_then(|result| result.as_ref().ok()).copied();
        let terminal = crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(reservation,
            raced.and_then(|result| result.map_err(|_| HostLifetimeError::Unavailable)));

        // Resolve BOTH exact native lineages before restoration or any second
        // action. Policy denial and unchanged Toolkit records cannot prove this.
        let live_after_race = lineages.iter().map(&live).collect::<Vec<_>>();
        let after_race = source.read_application_access(&human, &[member.clone()]).await;
        let restored = controller.set_group("selected-requester", original_group);
        let denied_after_restore = denied.as_ref().and_then(|observed| observed.as_ref().ok())
            .map(|(_, stamp)| stamp.check_current());
        let restored_view = publication.observe(|| controller.view_for_subject(Some(&subject)));
        let fresh = source.read_application_access(&human, &[member.clone()]).await;
        let mut fresh_native = None;
        let mut fresh_terminal = None;
        if let (Ok(fresh), Ok((view, stamp))) = (&fresh, &restored_view) {
            if let Some(row) = fresh.rows.iter().find(|row| row.id == selected_id) {
                let visible = restored.is_ok() && stamp.check_current().is_ok() && view.enforced()
                    && fresh.check_visible(|candidate| candidate == &member && view.can_view_agent(identity.as_str())).is_ok();
                if visible && source.recheck_application_access(fresh, row).await.is_ok() {
                    let reservation = login.reserve().unwrap();
                    let fresh_revoke = lifetime.with_current_setup(|| login.with_current_revoke(||
                        source.revoke_application_access_now(fresh, row)).map_err(|_| HostLifetimeError::Denied));
                    fresh_native = fresh_revoke.as_ref().ok().and_then(|result| result.as_ref().ok()).copied();
                    fresh_terminal = Some(crate::host_gateway::ConsoleSetupBinding::finish_access_revoke(reservation,
                        fresh_revoke.and_then(|result| result.map_err(|_| HostLifetimeError::Unavailable))));
                }
            }
        }
        let live_after_fresh = lineages.iter().map(&live).collect::<Vec<_>>();
        let final_read = source.read_application_access(&human, &[member.clone()]).await;
        let retired = logins.fixture_retire(4).unwrap().await;
        let after = fixture.store.list_records(&fixture.scope, None, 256).await;
        let inputs = fixture.inputs().await;
        let final_requests = fixture.model_requests();
        cancel_setup(&fixture, &task).await;

        // All adverse observations and fixture actions above complete before
        // these assertions. The outer unwind guard always joins fixture cleanup.
        assert_eq!(live_before, vec![true, true]);
        assert_eq!(callback_entries, 1);
        assert_eq!(requested.len(), 1);
        let selected = lineages.iter().position(|lineage| lineage.last() == requested.first()).unwrap();
        assert!(matches!(update, Some(Ok(1))), "{update:?}");
        assert!(matches!(old_stamp_after_swap, Some(Err(PublicationError::Changed))));
        let (denied_view, _) = denied.unwrap().unwrap();
        assert!(denied_view.enforced() && !denied_view.can_view_agent(identity.as_str()));
        assert!(matches!(denied_current, Some(Ok(()))));
        // The observation succeeded inside the callback after the update, with
        // no writer held. Later restoration correctly makes this stamp stale.
        assert!(matches!(denied_after_restore, Some(Err(PublicationError::Changed))));
        assert_eq!(native_result, Some(Err(ExpectedRevokeRefusal::NotCurrent)));
        assert_eq!(terminal.unwrap()["state"], "not_current");
        assert_eq!(live_after_race, vec![true, true]);
        assert!(after_race.unwrap().rows.iter().all(|row| row.projection["state"] == "grant_issued"));
        assert!(matches!(restored, Ok(2)), "{restored:?}");
        assert_eq!(fresh_native, Some(Ok(())));
        assert_eq!(fresh_terminal.unwrap().unwrap()["state"], "revoked");
        assert_eq!(live_after_fresh[selected], false);
        assert_eq!(live_after_fresh[1 - selected], true);
        let final_read = final_read.unwrap();
        assert_eq!(final_read.rows.iter().find(|row| row.id == selected_id).unwrap().projection["state"], "grant_unavailable");
        assert_eq!(final_read.rows.iter().find(|row| row.id == sibling_id).unwrap().projection["state"], "grant_issued");
        let retired = retired.unwrap();
        assert!(retired.reservations_drained && !retired.uncertain);
        assert_eq!(after.unwrap(), before);
        assert_eq!(inputs, Ok(1));
        assert_eq!(final_requests, requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_access_binds_historical_requester_and_retains_unknown_issuance_without_replay() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = prepared(&fixture).await;
        let source = &fixture.source;
        let human = source.route(&b.route).unwrap().requester.clone();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
        let source_row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&source_row, &b).unwrap();
        let access_key = crate::private_originals::service_grants::tests::fixture_access_key(source,
            &record.approved_task.as_ref().unwrap().singleton_grant().unwrap()).unwrap();
        let access = fixture.store.get(&access_key).await.unwrap().unwrap();
        let mut uncertain = access.data.clone();
        // Model only the existing durable pending observation after a lost
        // issuance acknowledgement. The read may not derive its missing leaf.
        uncertain.body["issuance"] = serde_json::json!({"state":"pending_native_issuance"});
        fixture.store.compare_and_swap(Transition { key:access.key, expected_revision:access.revision,
            expected_digest:Some(access.content_digest), claim:None, data:uncertain, events:vec![], effects:vec![] }).await.unwrap();
        let probe = ReadProbe::new(fixture.store.clone(), false);
        let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        let read = source.read_application_access_from(probe.as_ref(), &human, &[member.clone()]).await.unwrap();
        assert_eq!(read.rows.len(), 1);
        assert_eq!(read.rows[0].projection["state"], "pending_reconciliation");
        assert_eq!(read.rows[0].projection["can_revoke"], false);
        assert!(source.recheck_application_access(&read, &read.rows[0]).await.is_err());
        assert_eq!(fixture.store.list_records(&fixture.scope, None, 256).await.unwrap(), before);
        let mut historical = record;
        historical.source.requester = meerkat_core::PrincipalRef::in_domain(meerkat_core::PrincipalKind::Human,
            "prior-requester", meerkat_core::TrustDomainId::new("prior-console").unwrap()).unwrap();
        historical.source.represented_subject = None;
        fixture.store.compare_and_swap(Transition { key:source_row.key, expected_revision:source_row.revision,
            expected_digest:Some(source_row.content_digest), claim:None, data:encode(historical).unwrap(),
            events:vec![], effects:vec![] }).await.unwrap();
        assert!(source.read_application_access_from(probe.as_ref(), &human, &[member]).await.unwrap().rows.is_empty(),
            "today's same route ID never attributes an older requester's row to this person");
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_access_refuses_a_source_changed_during_its_read() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = prepared(&fixture).await;
        let source = &fixture.source;
        let human = source.route(&b.route).unwrap().requester.clone();
        let members = [fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone()];
        let key = source.key(&b).unwrap();
        let probe = ReadProbe::new(fixture.store.clone(), true);
        let read = source.read_application_access_from(probe.as_ref(), &human, &members);
        tokio::pin!(read);
        tokio::select! { _ = probe.entered.notified() => {}, _ = &mut read => panic!("canonical read did not hold") }
        let row = fixture.store.get(&key).await.unwrap().unwrap();
        let mut changed = source.decode_record(&row, &b).unwrap();
        changed.approved_task.as_mut().unwrap().header_mut().original_task.origin_session_id = meerkat_core::SessionId::new();
        fixture.store.compare_and_swap(Transition { key, expected_revision:row.revision,
            expected_digest:Some(row.content_digest), claim:None, data:encode(changed).unwrap(), events:vec![], effects:vec![] }).await.unwrap();
        probe.release.add_permits(1);
        assert!(read.await.is_err());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_source_pages_cross_sparse_history_and_sixty_four_real_approvals_without_replay() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::Normal).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let first = prepared(&fixture).await;
        let source = &fixture.source;
        let first_row = fixture.store.get(&source.key(&first).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&first_row, &first).unwrap();
        let task = source.service_task_snapshot(&record).await.unwrap();
        let mut approved = vec![first.clone()];
        for index in 1..65 {
            // Real request validation, canonical acquisition, protected decision
            // and native issuance. Distinct host correlations avoid deduplication.
            let next = named_acquisition(&fixture, &task, &format!("page-acquisition-{index}")).await;
            approved.push(approve_b_call(&fixture, next, [id("read")].into(), None,
                &format!("page-approval-{index}")).await);
        }
        let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        assert!(before.len() < 256, "the existing approval fixture's exact lookup remains in range");
        let expected = before.iter().filter(|row| row.data.schema == SOURCE_SCHEMA)
            .map(|row| row.key.id.clone()).collect::<Vec<_>>();
        assert!(expected.len() > 64);
        assert!(approved.iter().enumerate().all(|(index, current)|
            approved[..index].iter().all(|prior| prior.original != current.original)));
        // Non-source rows test only the canonical scan bound, not authentication
        // or a fabricated task. Add them after all real fixture approvals.
        for index in 0..1024 {
            let key = format!("!page-padding-{index:04}");
            assert!(key.as_str() < before[0].key.id.as_str());
            fixture.store.admit(AdmissionRequest {
                key:StoreKey { realm:fixture.scope.realm.clone(), owner:fixture.scope.owner.clone(), id:key.clone() },
                admission_key:key, data:RecordData { schema:"fixture.unrelated-history".into(), version:1,
                    body:serde_json::json!({"index":index}) }, events:vec![], effects:vec![],
            }).await.unwrap();
        }
        let human = source.route(&first.route).unwrap().requester.clone();
        let member = fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone();
        let visible = |candidate: &meerkat_core::MobMemberBinding| Ok(candidate == &member);
        let probe = ReadProbe::new(fixture.store.clone(), false);
        let requests = fixture.model_requests();
        assert!(matches!(source.service_task_status_with_read_probe(probe.as_ref(), &human, &visible).await,
            Err(PrivateOriginalError::Unavailable)), "the old whole-read contract remains bounded");
        let first_page = source.source_status_page_from(probe.as_ref(), None).await.unwrap();
        assert!(first_page.rows.is_empty());
        assert_eq!(first_page.next_after.as_deref(), Some("!page-padding-1023"));
        let empty_tasks = source.service_task_status_page_with_read_probe(probe.as_ref(), &human, &visible, &first_page.rows).await.unwrap();
        assert!(empty_tasks["tasks"].as_array().unwrap().is_empty());
        assert!(source.read_application_access_page_with_read_probe(probe.as_ref(), &human, &[member.clone()], &first_page.rows).await.unwrap().rows.is_empty());
        // The sparse page's continuation refers to more raw history, not a
        // claimed matching task or access count. Refresh repeats its lower bound.
        let refresh = source.source_status_page_from(probe.as_ref(), None).await.unwrap();
        assert_eq!(refresh.rows, first_page.rows); assert_eq!(refresh.next_after, first_page.next_after);
        let mut after = first_page.next_after;
        let mut seen = Vec::new(); let mut tasks = std::collections::BTreeSet::new();
        let mut access = std::collections::BTreeSet::new(); let mut saved_first_rows = None; let mut pages = 0;
        while let Some(start) = after.take() {
            let page = source.source_status_page_from(probe.as_ref(), Some(&start)).await.unwrap();
            assert!(page.rows.len() <= 64); assert!(page.rows.iter().all(|row| row.key.id > start));
            if pages == 0 {
                assert_eq!(page.rows.len(), 64);
                assert_eq!(page.next_after.as_ref(), page.rows.last().map(|row| &row.key.id),
                    "a fetched tail or lookahead cannot be consumed by the page cursor");
            }
            let projected = source.service_task_status_page_with_read_probe(probe.as_ref(), &human, &visible, &page.rows).await.unwrap();
            for task in projected["tasks"].as_array().unwrap() { assert!(tasks.insert(task["id"].as_str().unwrap().to_owned())); }
            let selected = source.read_application_access_page_with_read_probe(probe.as_ref(), &human, &[member.clone()], &page.rows).await.unwrap();
            selected.check_visible(|candidate| candidate == &member).unwrap();
            for row in &selected.rows { assert!(access.insert(row.id.clone())); assert_eq!(row.projection["state"], "grant_issued"); }
            assert!(source.read_application_access_page_with_read_probe(probe.as_ref(), &human, &[], &page.rows).await.unwrap().rows.is_empty());
            assert!(matches!(source.service_task_status_page_with_read_probe(probe.as_ref(), &human, &|_| Ok(false), &page.rows).await,
                Err(PrivateOriginalError::Denied)));
            if page.rows.iter().any(|row| row.key == first_row.key) { saved_first_rows = Some(page.rows.clone()); }
            seen.extend(page.rows.iter().map(|row| row.key.id.clone()));
            after = page.next_after; pages += 1; assert!(pages < 8, "bounded fixture traversal makes progress");
        }
        assert_eq!(seen, expected); assert_eq!(tasks.len(), 65); assert_eq!(access.len(), 65);
        assert!(pages > 1); assert_eq!(fixture.inputs().await, Ok(1)); assert_eq!(fixture.model_requests(), requests);
        // The probe panics on any mutation. Also compare all real source/task/
        // access bytes; sparse padding has no authority and is not decoded.
        for row in &before { assert_eq!(fixture.store.get(&row.key).await.unwrap().as_ref(), Some(row)); }
        probe.unavailable.store(true, Ordering::SeqCst);
        assert!(matches!(source.source_status_page_from(probe.as_ref(), None).await, Err(PrivateOriginalError::Unavailable)));
        probe.unavailable.store(false, Ordering::SeqCst);
        // Page decoding still rechecks canonical cancellation after its captured
        // slice. This is an actual owner transition, not a synthetic page edit.
        let rows = saved_first_rows.unwrap();
        source.record_task_group_cancelled(&first).await.unwrap();
        assert!(matches!(source.service_task_status_page_with_read_probe(probe.as_ref(), &human, &visible, &rows).await,
            Err(PrivateOriginalError::Denied)));
        assert_eq!(fixture.inputs().await, Ok(1)); assert_eq!(fixture.model_requests(), requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

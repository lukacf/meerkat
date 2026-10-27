//! Actual common review owner and native child-effect close controls.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use toolkit_store::*;

pub(super) struct StoreProbe {
    inner: Arc<dyn ToolkitStore>,
    pub(super) target_fault: AtomicUsize,
    writes: AtomicUsize,
    pause_access_admission: AtomicBool,
    pause_parent_admission: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl StoreProbe {
    pub(super) fn new(inner: Arc<dyn ToolkitStore>, pause: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            target_fault: AtomicUsize::new(0),            writes: AtomicUsize::new(0),
            pause_access_admission: AtomicBool::new(pause),
            pause_parent_admission: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }
}
#[async_trait::async_trait]
impl ToolkitStore for StoreProbe {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        let pause = request.data.schema == "toolkit.authenticated-service-access-decision"
            && self.pause_access_admission.swap(false, Ordering::SeqCst)
            || request.data.schema == "toolkit.operator-application-parent"
            && self.pause_parent_admission.swap(false, Ordering::SeqCst);
        self.writes.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.admit(request).await;
        if pause {
            // The real SQLite pending decision has committed, but its caller
            // has not returned to the native issuance boundary.
            self.entered.notify_one();
            self.release.notified().await;
        }
        result
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
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
        self.writes.fetch_add(1, Ordering::SeqCst);
        let fault = self.target_fault.load(Ordering::SeqCst);
        let state = &transition.data.body["target_issuance"][0]["state"];
        let selected = transition.data.schema == "toolkit.operator-application-parent"
            && (fault == 1 && state == "pending" || fault == 2 && state == "issued");
        let result = self.inner.compare_and_swap(transition).await;
        if selected && self.target_fault.compare_exchange(fault, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            result?;
            return Err(StoreError::OwnerUnavailable);
        }
        result
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        self.inner.claim_record(key, revision, worker, ttl).await
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
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
        self.inner.claim_effect(key, revision, worker, ttl).await
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl).await
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}

#[tokio::test]
async fn common_parent_review_refuses_other_source_or_lifetime_before_receiver_writes() {
    let (_creator_directory, creator_selection) = super::super::super::tests::fixture();
    let creator = OperatorSetup::open(creator_selection.clone(), startup()).unwrap();
    let (_receiver_directory, receiver_selection) = super::super::super::tests::fixture();
    let mut receiver = OperatorSetup::open(receiver_selection.clone(), startup()).unwrap();
    assert_eq!(creator.native.realm, receiver.native.realm);
    assert_eq!(
        creator.native.grant_administrator,
        receiver.native.grant_administrator
    );
    let receiver_path = state_directory(
        &receiver_selection,
        &receiver.native,
        NativeOwnerStartup::Restore,
    )
    .unwrap();
    let creator_path = state_directory(
        &creator_selection,
        &creator.native,
        NativeOwnerStartup::Restore,
    )
    .unwrap();
    let creator_before = fs::read(creator_path.join("grants.jsonl")).unwrap();
    let before = fs::read(receiver_path.join("grants.jsonl")).unwrap();
    let store = StoreProbe::new(receiver.originals.clone(), false);
    receiver.originals = store.clone();
    let proposal = creator
        .prepare_parent_review(Id::new("console").unwrap(), bounds(), request())
        .unwrap();
    let approval = reviewed(&proposal, ApprovalDecision::Approve);
    assert!(matches!(
        receiver
            .apply_parent_decision(proposal.clone(), approval.clone())
            .await,
        Err(HostLifetimeError::WrongOwner)
    ));
    // The same source with a different valid lifetime must also refuse. This
    // uses the existing facade constructor and the actual open receiver phase.
    let other_lifetime = OperatorSetup::from_local_gateway(
        receiver.lifetime.clone(),
        store.clone(),
        creator.source.clone(),
    )
    .unwrap();
    assert!(matches!(
        other_lifetime
            .apply_parent_decision(proposal, approval)
            .await,
        Err(HostLifetimeError::WrongOwner)
    ));
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    let after = fs::read(receiver_path.join("grants.jsonl")).unwrap();
    assert_eq!(
        after.split(|byte| *byte == b'\n').count(),
        before.split(|byte| *byte == b'\n').count()
    );
    assert_eq!(after, before);
    assert_eq!(
        fs::read(creator_path.join("grants.jsonl")).unwrap(),
        creator_before
    );
    drop(other_lifetime);
    creator.finish().await.unwrap();
    receiver.finish().await.unwrap();
}

#[tokio::test]
async fn service_parent_review_from_closed_creator_refuses_before_receiver_writes() {
    let (_creator_directory, creator_selection) = super::super::super::tests::fixture();
    let mut creator = OperatorSetup::open(creator_selection.clone(), startup()).unwrap();
    let (_receiver_directory, receiver_selection) = super::super::super::tests::fixture();
    let mut receiver = OperatorSetup::open(receiver_selection.clone(), startup()).unwrap();
    let (decision, selection, request) = service_proposal(&creator);
    let review = creator
        .prepare_service_mcp_parent_review(decision.clone(), selection.clone(), request.clone())
        .unwrap();
    let after_drop = creator
        .prepare_service_mcp_parent_review(decision, selection, request)
        .unwrap();
    let approval = service_approval(&review, ApprovalDecision::Approve);
    let creator_path = state_directory(
        &creator_selection,
        &creator.native,
        NativeOwnerStartup::Restore,
    )
    .unwrap();
    let creator_before = fs::read(creator_path.join("grants.jsonl")).unwrap();
    let creator_store = StoreProbe::new(creator.originals.clone(), false);
    creator.originals = creator_store.clone();
    let closed_view = OperatorSetup::from_local_gateway(
        creator.lifetime.clone(),
        creator_store.clone(),
        creator.source.clone(),
    )
    .unwrap();
    let creator_source = Arc::downgrade(&creator.source);
    let creator_lifetime = Arc::downgrade(&creator.lifetime);
    creator.finish().await.unwrap();
    // Keep the actual source and lifetime alive. Refusal must follow the closed
    // phase, rather than merely a failed weak upgrade.
    assert!(creator_source.upgrade().is_some());
    assert!(creator_lifetime.upgrade().is_some());
    assert!(matches!(
        closed_view
            .apply_service_mcp_parent_decision(review, approval.clone())
            .await,
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(creator_store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(creator_path.join("grants.jsonl")).unwrap(),
        creator_before
    );
    drop(closed_view);
    assert!(creator_source.upgrade().is_none());
    assert!(creator_lifetime.upgrade().is_none());
    let path = state_directory(
        &receiver_selection,
        &receiver.native,
        NativeOwnerStartup::Restore,
    )
    .unwrap();
    let before = fs::read(path.join("grants.jsonl")).unwrap();
    let store = StoreProbe::new(receiver.originals.clone(), false);
    receiver.originals = store.clone();
    assert!(matches!(
        receiver
            .apply_service_mcp_parent_decision(after_drop, approval)
            .await,
        Err(HostLifetimeError::Unavailable)
    ));
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    let after = fs::read(path.join("grants.jsonl")).unwrap();
    assert_eq!(
        after.split(|byte| *byte == b'\n').count(),
        before.split(|byte| *byte == b'\n').count()
    );
    assert_eq!(after, before);
    receiver.finish().await.unwrap();
}

#[tokio::test]
async fn close_during_committed_access_store_wait_prevents_native_child_write() {
    use crate::private_originals::PrivateServiceGrantState;
    let (_directory, selection) = super::super::super::tests::fixture();
    let mut setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
    let path = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
    let (decision, mcp, request) = service_proposal(&setup);
    let review = setup
        .prepare_service_mcp_parent_review(decision, mcp, request)
        .unwrap();
    let approval = service_approval(&review, ApprovalDecision::Approve);
    let ServiceMcpParentOutcome::Approved(approved) = setup
        .apply_service_mcp_parent_decision(review, approval)
        .await
        .unwrap()
    else {
        panic!("expected native reviewed parent")
    };
    let before = fs::read(path.join("grants.jsonl")).unwrap();
    let store = StoreProbe::new(setup.originals.clone(), true);
    setup.originals = store.clone();
    let adapter = service_adapter(&setup).await;
    let lifetime = setup.lifetime.clone();
    let scope = StoreScope {
        realm: setup.native.realm.to_string(),
        owner: setup.lifetime.selected_policy().deployment.to_string(),
    };
    let pending = adapter.apply_mcp_service_access_decision(approved);
    tokio::pin!(pending);
    tokio::select! {
        _ = store.entered.notified() => {},
        _ = &mut pending => panic!("issuance must still be held in the real store await"),
    }
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(path.join("grants.jsonl")).unwrap(), before);
    // Close settles while there is no entered native mutation. The retained
    // future and source stay owned, so this cannot pass merely by dropping them.
    setup.finish().await.unwrap();
    assert!(lifetime.with_current_setup(|| Ok(())).is_err());
    store.release.notify_one();
    let outcome = pending.await.unwrap();
    assert_eq!(outcome.state, PrivateServiceGrantState::GrantUnavailable);
    assert_eq!(store.writes.load(Ordering::SeqCst), 2); // Pending row, then honest refusal.
    let after = fs::read(path.join("grants.jsonl")).unwrap();
    assert_eq!(
        after.split(|byte| *byte == b'\n').count(),
        before.split(|byte| *byte == b'\n').count()
    );
    assert_eq!(after, before);
    let rows = store.inner.list_records(&scope, None, 10).await.unwrap();
    let access: Vec<_> = rows
        .iter()
        .filter(|row| row.data.schema == "toolkit.authenticated-service-access-decision")
        .collect();
    assert_eq!(access.len(), 1);
    assert_eq!(access[0].data.body["issuance"]["state"], "native_refused");
}

// These use actual SQLite admission and native grant storage. The changed
// predicate models loss of the already-captured action eligibility; it is not
// a fabricated authenticated Human or an HTTP/authentication fixture.
#[tokio::test]
async fn captured_answer_loss_during_parent_admission_prevents_native_root_write() {
    let (_directory, selection) = super::super::super::tests::fixture();
    let mut setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
    let path = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
    let (decision, mcp, request) = service_proposal(&setup);
    let review = setup.prepare_service_mcp_parent_review(decision, mcp, request).unwrap();
    let approval = service_approval(&review, ApprovalDecision::Approve);
    let before = fs::read(path.join("grants.jsonl")).unwrap();
    let store = StoreProbe::new(setup.originals.clone(), false);
    store.pause_parent_admission.store(true, Ordering::SeqCst);
    setup.originals = store.clone();
    let eligible = Arc::new(AtomicBool::new(true));
    let current = eligible.clone();
    let pending = setup.apply_service_mcp_parent_decision_current(review, approval,
        Arc::new(move || current.load(Ordering::SeqCst)));
    tokio::pin!(pending);
    tokio::select! {
        _ = store.entered.notified() => {},
        _ = &mut pending => panic!("hold the actual committed parent admission"),
    }
    eligible.store(false, Ordering::SeqCst);
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(path.join("grants.jsonl")).unwrap(), before);
    store.release.notify_one();
    assert!(matches!(pending.await.unwrap(), ServiceMcpParentOutcome::Unavailable));
    assert_eq!(fs::read(path.join("grants.jsonl")).unwrap(), before);
    assert_eq!(store.writes.load(Ordering::SeqCst), 2);
    // The host remains open: only the exact answer predicate refused entry.
    setup.lifetime.with_current_setup(|| Ok(())).unwrap();
}

#[tokio::test]
async fn captured_answer_loss_during_child_admission_prevents_native_child_write() {
    use crate::private_originals::PrivateServiceGrantState;
    let (_directory, selection) = super::super::super::tests::fixture();
    let mut setup = OperatorSetup::open(selection.clone(), startup()).unwrap();
    let path = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore).unwrap();
    let (decision, mcp, request) = service_proposal(&setup);
    let review = setup.prepare_service_mcp_parent_review(decision, mcp, request).unwrap();
    let approval = service_approval(&review, ApprovalDecision::Approve);
    let ServiceMcpParentOutcome::Approved(approved) = setup.apply_service_mcp_parent_decision(review, approval).await.unwrap()
        else { panic!("actual native reviewed parent") };
    let before = fs::read(path.join("grants.jsonl")).unwrap();
    let store = StoreProbe::new(setup.originals.clone(), true);
    setup.originals = store.clone();
    let adapter = service_adapter(&setup).await;
    let eligible = Arc::new(AtomicBool::new(true));
    let current = eligible.clone();
    let pending = adapter.apply_mcp_service_access_decision_current(approved,
        Arc::new(move || current.load(Ordering::SeqCst)));
    tokio::pin!(pending);
    tokio::select! {
        _ = store.entered.notified() => {},
        _ = &mut pending => panic!("hold the actual committed child admission"),
    }
    eligible.store(false, Ordering::SeqCst);
    store.release.notify_one();
    assert_eq!(pending.await.unwrap().state, PrivateServiceGrantState::GrantUnavailable);
    assert_eq!(fs::read(path.join("grants.jsonl")).unwrap(), before);
    assert_eq!(store.writes.load(Ordering::SeqCst), 2);
    setup.lifetime.with_current_setup(|| Ok(())).unwrap();
}

//! Actual shared application review operation over native preparation and SQLite.
//! Human response and store faults are mechanical; no protected HTTP UI is run.
use super::tests::{prepared, request};
use super::*;
use crate::installed_host::operator_setup::{OperatorParentOutcome, OperatorSetup};
use crate::private_originals::ContinuationFixture;
use futures::FutureExt;
use meerkat_core::{
    ApprovalDecision, ApprovalPrincipalId, ApprovalRecord, ApprovalRequest, ApprovalService,
};
use std::{
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use toolkit_store::*;

const DECISION_SCHEMA: &str = "toolkit.operator-application-parent";
struct RecoveryStore {
    inner: Arc<dyn ToolkitStore>,
    mode: AtomicUsize,
    fail_get: Mutex<Option<StoreKey>>,
    fail_list: AtomicBool,
    list_calls: AtomicUsize,
    missing_get: Mutex<Option<StoreKey>>,
}
impl RecoveryStore {
    fn new(inner: Arc<dyn ToolkitStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            mode: AtomicUsize::new(0),
            fail_get: Mutex::new(None),
            fail_list: AtomicBool::new(false),
            list_calls: AtomicUsize::new(0),
            missing_get: Mutex::new(None),
        })
    }
}
#[async_trait::async_trait]
impl ToolkitStore for RecoveryStore {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        let decision = request.data.schema == DECISION_SCHEMA;
        let result = self.inner.admit(request).await;
        if decision
            && self
                .mode
                .compare_exchange(4, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            // Pending decision committed; caller lost before native issuance.
            return Err(StoreError::OwnerUnavailable);
        }
        result
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if self.fail_get.lock().unwrap().as_ref() == Some(key) {
            self.fail_get.lock().unwrap().take();
            return Err(StoreError::OwnerUnavailable);
        }
        if self.missing_get.lock().unwrap().as_ref() == Some(key) {
            self.missing_get.lock().unwrap().take();
            return Ok(None);
        }
        self.inner.get(key).await
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_list.load(Ordering::SeqCst) {
            return Err(StoreError::OwnerUnavailable);
        }
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let mode = self.mode.load(Ordering::SeqCst);
        let selected_source = transition.data.schema == SOURCE_SCHEMA
            && !transition.data.body["setup_request"].is_null();
        let issued_decision = transition.data.schema == DECISION_SCHEMA
            && transition.data.body["issued"]["state"] == "issued";
        if (selected_source && matches!(mode, 1 | 2)) || (issued_decision && mode == 3) {
            self.mode.store(0, Ordering::SeqCst);
            let key = transition.key.clone();
            if mode == 2 {
                self.inner.compare_and_swap(transition).await?;
            }
            if selected_source {
                *self.fail_get.lock().unwrap() = Some(key);
            }
            return Err(StoreError::OwnerUnavailable);
        }
        self.inner.compare_and_swap(transition).await
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

async fn fault_fixture() -> (ContinuationFixture, Arc<RecoveryStore>) {
    let selected = Arc::new(Mutex::new(None));
    let capture = selected.clone();
    let fixture = ContinuationFixture::for_service_input_with_source(Box::new(move |store| {
        let fault = RecoveryStore::new(store);
        *capture.lock().unwrap() = Some(fault.clone());
        fault
    })).await;
    let store = selected.lock().unwrap().take();
    match store {
        Some(store) => (fixture, store),
        None => {
            let cleanup = fixture.shutdown().await;
            assert!(cleanup.is_ok(), "{cleanup:?}");
            panic!("source store was not selected during actual fixture construction");
        }
    }
}

fn decide(request: ApprovalRequest, choice: ApprovalDecision) -> ApprovalRecord {
    let approvals = ApprovalService::new();
    let pending = approvals.request(request).unwrap();
    approvals
        .decide(
            &pending.approval_id,
            choice,
            ApprovalPrincipalId::new("retained-review-fixture").unwrap(),
            None,
            None,
        )
        .unwrap()
}
async fn owner(source: &Arc<PrivateOriginals>, fixture: &ContinuationFixture) -> OperatorSetup {
    source
        .local_operator_setup(fixture.lifetime().clone())
        .unwrap()
}
async fn rows(store: &RecoveryStore, scope: &StoreScope) -> Vec<StoredRecord> {
    store
        .inner
        .list_records(scope, None, 256)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.data.schema == DECISION_SCHEMA)
        .collect()
}
async fn retry(
    source: &Arc<PrivateOriginals>,
    submission: &PrivateOriginalSubmit,
    owner: &OperatorSetup,
) -> Result<OperatorParentOutcome, HostLifetimeError> {
    let prepared = source
        .prepare_local_controller_setup(submission.clone())
        .await
        .unwrap();
    let request = request(&prepared);
    owner
        .review_original_setup_request(prepared, request, |_| async {
            panic!("retry must recover the actual prior decision without a new review")
        })
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_review_recovers_precommit_and_committed_source_error_with_failed_readback() {
    for mode in [1, 2] {
        let (fixture, store) = fault_fixture().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let source = fixture.source.clone();
            let (_, submission) = prepared(&fixture, &format!("public-source-fault-{mode}")).await;
            let prepared = source
                .prepare_local_controller_setup(submission.clone())
                .await
                .unwrap();
            let request = request(&prepared);
            let owner = owner(&source, &fixture).await;
            let prompts = AtomicUsize::new(0);
            store.mode.store(mode, Ordering::SeqCst);
            let first = owner
                .review_original_setup_request(prepared, request, |request| async {
                    prompts.fetch_add(1, Ordering::SeqCst);
                    Ok(decide(request, ApprovalDecision::Approve))
                })
                .await
                .unwrap();
            assert!(matches!(
                first,
                OperatorParentOutcome::PendingReconciliation
            ));
            let before = rows(&store, &fixture.scope).await;
            assert_eq!(before.len(), 1);
            let lineage: Vec<GrantLineageRef> =
                serde_json::from_value(before[0].data.body["issued"]["lineage"].clone()).unwrap();
            assert_eq!(lineage.len(), 1);
            let selected = match retry(&source, &submission, &owner).await.unwrap() {
                OperatorParentOutcome::SetupRequestSelected { selection } => selection,
                _ => panic!("shared review owner did not recover the exact selection"),
            };
            assert!(selected.parent == lineage);
            assert!(selected.original == submission.original);
            assert_eq!(selected.session_id, fixture.session);
            assert_eq!(rows(&store, &fixture.scope).await, before);
            assert_eq!(prompts.load(Ordering::SeqCst), 1);
            assert!(matches!(
                retry(&source, &submission, &owner).await.unwrap(),
                OperatorParentOutcome::SetupRequestSelected { .. }
            ));
            assert_eq!(rows(&store, &fixture.scope).await, before);
            assert_eq!(fixture.inputs().await.unwrap(), 0);
            assert_eq!(fixture.model_requests(), 0);
        }))
        .catch_unwind()
        .await;
        drop(store);
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_review_recovers_native_parent_after_issued_record_cas_failure() {
    let (fixture, store) = fault_fixture().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let source = fixture.source.clone();
        let (_, submission) = prepared(&fixture, "public-parent-cas").await;
        let prepared = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let request = request(&prepared);
        let owner = owner(&source, &fixture).await;
        store.mode.store(3, Ordering::SeqCst);
        assert!(matches!(
            owner
                .review_original_setup_request(prepared, request, |request| async {
                    Ok(decide(request, ApprovalDecision::Approve))
                })
                .await
                .unwrap(),
            OperatorParentOutcome::PendingReconciliation
        ));
        let before = rows(&store, &fixture.scope).await;
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].data.body["issued"]["state"], "pending");
        let id = EvidenceId::new(before[0].key.id.clone()).unwrap();
        let actual = source
            .owner
            .grants()
            .issued_reference(&id)
            .unwrap()
            .unwrap();
        let selected = match retry(&source, &submission, &owner).await.unwrap() {
            OperatorParentOutcome::SetupRequestSelected { selection } => selection,
            _ => panic!("actual pending parent was not recovered"),
        };
        assert!(selected.parent == vec![actual]);
        let after = rows(&store, &fixture.scope).await;
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].key, before[0].key);
        assert_eq!(
            after[0].data.body["approval"],
            before[0].data.body["approval"]
        );
        assert_eq!(after[0].data.body["issued"]["state"], "issued");
        assert_eq!(fixture.inputs().await.unwrap(), 0);
    }))
    .catch_unwind()
    .await;
    drop(store);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_review_never_reissues_unknown_pending_or_prompts_after_failed_exact_read() {
    let (fixture, store) = fault_fixture().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let source = fixture.source.clone();
        let (_, submission) = prepared(&fixture, "public-parent-unknown").await;
        let prepared = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let request = request(&prepared);
        let owner = owner(&source, &fixture).await;
        store.mode.store(4, Ordering::SeqCst);
        assert!(
            owner
                .review_original_setup_request(prepared, request, |request| async {
                    Ok(decide(request, ApprovalDecision::Approve))
                })
                .await
                .is_err()
        );
        let before = rows(&store, &fixture.scope).await;
        assert_eq!(before.len(), 1);
        let id = EvidenceId::new(before[0].key.id.clone()).unwrap();
        assert!(
            source
                .owner
                .grants()
                .issued_reference(&id)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            retry(&source, &submission, &owner).await.unwrap(),
            OperatorParentOutcome::PendingReconciliation
        ));
        assert!(
            source
                .owner
                .grants()
                .issued_reference(&id)
                .unwrap()
                .is_none()
        );
        *store.fail_get.lock().unwrap() = Some(before[0].key.clone());
        assert!(retry(&source, &submission, &owner).await.is_err());
        assert!(matches!(
            retry(&source, &submission, &owner).await.unwrap(),
            OperatorParentOutcome::PendingReconciliation
        ));
        assert_eq!(rows(&store, &fixture.scope).await, before);
    }))
    .catch_unwind()
    .await;
    drop(store);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_review_preserves_denial_then_explicit_review_and_refuses_changed_context() {
    let (fixture, store) = fault_fixture().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let source = fixture.source.clone();
        let (_, submission) = prepared(&fixture, "public-denial-retry").await;
        let owner = owner(&source, &fixture).await;
        let prepared = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let request = request(&prepared);
        assert!(matches!(
            owner
                .review_original_setup_request(prepared, request, |request| async {
                    Ok(decide(request, ApprovalDecision::Deny))
                })
                .await
                .unwrap(),
            OperatorParentOutcome::Denied
        ));
        let prepared = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let request = super::tests::request(&prepared);
        assert!(matches!(
            owner
                .review_original_setup_request(prepared, request, |request| async {
                    Ok(decide(request, ApprovalDecision::Approve))
                })
                .await
                .unwrap(),
            OperatorParentOutcome::SetupRequestSelected { .. }
        ));
        let before = rows(&store, &fixture.scope).await;
        assert_eq!(before.len(), 2);
        let prepared = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let mut request = super::tests::request(&prepared);
        request.metadata.app_context =
            Some(serde_json::json!({"protected_login_scope":"different-login"}));
        assert!(matches!(
            owner
                .review_original_setup_request(prepared, request, |_| async {
                    panic!("changed review context must not replace an approved original")
                })
                .await,
            Err(HostLifetimeError::Denied)
        ));
        assert_eq!(rows(&store, &fixture.scope).await, before);
    }))
    .catch_unwind()
    .await;
    drop(store);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_review_refuses_changed_session_revocation_and_cancellation_without_reprompt() {
    let (fixture, store) = fault_fixture().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let source = fixture.source.clone();
        let (_, submission) = prepared(&fixture, "public-current-parent").await;
        let captured = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let owner = owner(&source, &fixture).await;
        let req = request(&captured);
        let selected = match owner
            .review_original_setup_request(captured, req, |request| async {
                Ok(decide(request, ApprovalDecision::Approve))
            })
            .await
            .unwrap()
        {
            OperatorParentOutcome::SetupRequestSelected { selection } => selection,
            _ => panic!("actual review did not select the source"),
        };
        let before = rows(&store, &fixture.scope).await;
        let captured = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        // This mismatch control changes only the already-public test fixture
        // capture. It does not claim a live native session replacement schedule.
        let other = LocalControllerSetup::owner_test_fixture(
            &source,
            submission.original.clone(),
            meerkat_core::SessionId::new(),
            captured.controller().clone(),
        );
        let req = request(&other);
        assert!(matches!(
            owner
                .review_original_setup_request(other, req, |_| async {
                    panic!("a different session cannot silently obtain a new parent")
                })
                .await,
            Err(HostLifetimeError::Denied)
        ));
        {
            let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
            source
                .owner
                .grants()
                .revoke(
                    &selected.parent[0].root_authority,
                    &selected.parent[0],
                    &mut custody,
                )
                .unwrap();
        }
        assert!(matches!(
            retry(&source, &submission, &owner).await.unwrap(),
            OperatorParentOutcome::Unavailable
        ));
        source
            .cancel_original_operation(&submission.original)
            .await
            .unwrap();
        assert!(matches!(
            retry(&source, &submission, &owner).await,
            Err(HostLifetimeError::Denied)
        ));
        assert_eq!(rows(&store, &fixture.scope).await, before);
        assert_eq!(fixture.inputs().await.unwrap(), 0);
    }))
    .catch_unwind()
    .await;
    drop(store);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_review_uses_exact_decision_with_unrelated_rows_and_failed_listing() {
    let (fixture, store) = fault_fixture().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let source = fixture.source.clone();
        let (_, submission) = prepared(&fixture, "public-direct-decision").await;
        let captured = source
            .prepare_local_controller_setup(submission.clone())
            .await
            .unwrap();
        let req = request(&captured);
        let owner = owner(&source, &fixture).await;
        // One native fixture, unrelated canonical rows beyond the old page
        // size. Poisoning all listing and observing zero calls discriminates
        // direct ownership from any realm enumeration, regardless of its cap.
        for n in 0..65 {
            let key = StoreKey {
                realm: fixture.scope.realm.clone(),
                owner: fixture.scope.owner.clone(),
                id: format!("000-unrelated-setup-{n:03}"),
            };
            store
                .inner
                .admit(AdmissionRequest {
                    key: key.clone(),
                    admission_key: key.id,
                    data: RecordData {
                        schema: "fixture-unrelated-setup".into(),
                        version: 1,
                        body: serde_json::json!({"n": n}),
                    },
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
        }
        store.list_calls.store(0, Ordering::SeqCst);
        store.fail_list.store(true, Ordering::SeqCst);
        store.mode.store(1, Ordering::SeqCst);
        let prompts = AtomicUsize::new(0);
        assert!(matches!(
            owner
                .review_original_setup_request(captured, req, |request| async {
                    prompts.fetch_add(1, Ordering::SeqCst);
                    Ok(decide(request, ApprovalDecision::Approve))
                })
                .await
                .unwrap(),
            OperatorParentOutcome::PendingReconciliation
        ));
        let before = rows(&store, &fixture.scope).await;
        assert_eq!(before.len(), 1);
        let lineage: Vec<GrantLineageRef> =
            serde_json::from_value(before[0].data.body["issued"]["lineage"].clone()).unwrap();
        let selected = match retry(&source, &submission, &owner).await.unwrap() {
            OperatorParentOutcome::SetupRequestSelected { selection } => selection,
            _ => panic!("failed listing must not block exact decision recovery"),
        };
        assert!(selected.parent == lineage);
        assert_eq!(prompts.load(Ordering::SeqCst), 1);
        assert_eq!(store.list_calls.load(Ordering::SeqCst), 0);
        assert_eq!(rows(&store, &fixture.scope).await, before);

        // An already selected source cannot authorize a new review when its
        // exact decision is absent, including an unsupported prior key format.
        *store.missing_get.lock().unwrap() = Some(before[0].key.clone());
        assert!(matches!(
            retry(&source, &submission, &owner).await,
            Err(HostLifetimeError::Unavailable)
        ));
        assert!(matches!(
            retry(&source, &submission, &owner).await.unwrap(),
            OperatorParentOutcome::SetupRequestSelected { .. }
        ));

        // A record at the correct key still must name the exact reviewed
        // original. A well-formed but mismatched record is not absence.
        let mut data = before[0].data.clone();
        data.body["setup_request"]["original"]["event_sha256"] =
            serde_json::Value::String("0".repeat(64));
        let mismatched = store
            .inner
            .compare_and_swap(Transition {
                key: before[0].key.clone(),
                expected_revision: before[0].revision,
                expected_digest: Some(before[0].content_digest.clone()),
                claim: None,
                data,
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        assert!(matches!(
            retry(&source, &submission, &owner).await,
            Err(HostLifetimeError::Denied)
        ));
        store
            .inner
            .compare_and_swap(Transition {
                key: mismatched.key,
                expected_revision: mismatched.revision,
                expected_digest: Some(mismatched.content_digest),
                claim: None,
                data: before[0].data.clone(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        assert!(matches!(
            retry(&source, &submission, &owner).await.unwrap(),
            OperatorParentOutcome::SetupRequestSelected { .. }
        ));
        assert_eq!(store.list_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.inputs().await.unwrap(), 0);
        assert_eq!(fixture.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    drop(store);
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

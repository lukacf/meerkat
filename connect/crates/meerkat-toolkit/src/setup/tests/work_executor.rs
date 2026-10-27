//! Real Toolkit and WorkGraph SQLite handoffs. Policy/reference owners are fixtures.
use super::*;
use crate::work_admission::*;
use meerkat_workgraph::{
    CreateWorkItemRequest, MemoryWorkGraphStore, SqliteWorkGraphStore, WorkGraphService,
    WorkNamespace,
};
use std::sync::{Arc, atomic::AtomicUsize};
use toolkit_package::canonical_json;

struct Prepare;
impl SetupWorkPreparer for Prepare {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        let mut input_reference = reference("canonical-work-request");
        input_reference.content_digest =
            ContentDigest::of_bytes(&canonical_json(request.request()).unwrap());
        Ok(SetupWorkPreparation {
            delegation: None,
            scope: HostWorkGraphScope::new(id("tenant-a"), WorkNamespace::new("setup")?)?,
            input_reference,
            work_request: CreateWorkItemRequest {
                title: "Run the configured setup agents".into(),
                ..Default::default()
            },
        })
    }
}

struct Policy {
    dispatch: Arc<std::sync::RwLock<bool>>,
    observe: std::sync::RwLock<bool>,
    dispatch_calls: AtomicUsize,
    observation_calls: AtomicUsize,
}
impl Policy {
    fn new() -> Self {
        Self {
            dispatch: Arc::new(std::sync::RwLock::new(true)),
            observe: true.into(),
            dispatch_calls: 0.into(),
            observation_calls: 0.into(),
        }
    }
}
#[async_trait]
impl SetupWorkAuthorizer for Policy {
    async fn admission_fence(
        &self,
        request: &SetupWorkAdmission,
        exact: &meerkat_workgraph::WorkAdmission,
    ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError> {
        self.dispatch_calls.fetch_add(1, Ordering::SeqCst);
        if *self.dispatch.read().unwrap() {
            Ok(Arc::new(
                crate::work_admission::test_fence::TestAdmissionFence {
                    allowed: self.dispatch.clone(),
                    scope: request.scope().clone(),
                    admission: exact.clone(),
                },
            ))
        } else {
            Err(SetupWorkAdmissionError::Denied)
        }
    }
}
#[async_trait]
impl SetupWorkObservationAuthorizer for Policy {
    async fn authorize_observation(
        &self,
        _: &SetupWorkAdmission,
    ) -> Result<(), SetupWorkAdmissionError> {
        self.observation_calls.fetch_add(1, Ordering::SeqCst);
        if *self.observe.read().unwrap() {
            Ok(())
        } else {
            Err(SetupWorkAdmissionError::Denied)
        }
    }
    fn with_current_disclosure(
        &self,
        request: &SetupWorkAdmission,
        admission: &meerkat_workgraph::WorkAdmission,
        disclose: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError> {
        let current = self.observe.read().unwrap();
        if !*current || admission.key != *request.admission_key() {
            return Err(SetupWorkAdmissionError::Denied);
        }
        disclose();
        Ok(())
    }
}
fn options(worker: &str) -> WorkAdmissionStepOptions {
    WorkAdmissionStepOptions {
        worker: id(worker),
        lease_ms: 2000,
        owner_timeout_ms: 100,
        settlement_budget_ms: 1000,
    }
}
fn service(f: &Fixture) -> WorkGraphService {
    WorkGraphService::with_scope(
        Arc::new(SqliteWorkGraphStore::open(f.directory.path().join("work.sqlite")).unwrap()),
        "tenant-a",
        WorkNamespace::new("setup").unwrap(),
    )
}
fn backend(service: &WorkGraphService) -> SetupWorkBackend {
    // These fixtures opened the real SQLite store immediately above. The
    // production caller obtains this declaration from its native provider open.
    SetupWorkBackend::new(
        service.clone(),
        "fixture-sqlite".into(),
        id("fixture-workgraph"),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap()
}
fn host<'a>(service: &WorkGraphService, policy: &'a Policy) -> SetupWorkHost<'a> {
    SetupWorkHost {
        backend: backend(service),
        dispatch: policy,
        observation: policy,
    }
}
async fn prepare(f: &Fixture) -> (SetupSnapshot, Id, PreparedSetupWork) {
    let snapshot = f.admit().await;
    let operation = id(&snapshot.key().id);
    let prepared = f
        .bridge()
        .prepare_work_protocol_fixture(&operation, f.access(&f.context(200)), &Prepare)
        .await
        .unwrap();
    (snapshot, operation, prepared)
}
async fn effect(f: &Fixture, snapshot: &SetupSnapshot) -> StoredEffect {
    f.store
        .get_effect(&EffectKey {
            record: snapshot.key().clone(),
            id: "work-admission".into(),
        })
        .await
        .unwrap()
        .unwrap()
}
async fn ambiguous_claim(f: &Fixture, snapshot: &SetupSnapshot) -> StoredEffect {
    let original = effect(f, snapshot).await;
    let EffectClaimOutcome::Claimed(claimed) = f
        .store
        .claim_effect(&original.key, snapshot.state().revision, "crashed", 1000)
        .await
        .unwrap()
    else {
        panic!()
    };
    f.store
        .acknowledge_effect(
            claimed.lease.as_ref().unwrap(),
            EffectOutcome::Uncertain {
                evidence: RecordData {
                    schema: "test.lost_reply".into(),
                    version: 1,
                    body: serde_json::json!({}),
                },
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn work_executor_admits_once_and_reopens_without_claiming_setup_ready() {
    let f = Fixture::new();
    let (snapshot, operation, _) = prepare(&f).await;
    let service = service(&f);
    let policy = Policy::new();
    let actual = f
        .bridge()
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            Some(f.access(&f.context(200))),
            &options("first"),
        )
        .await
        .unwrap();
    let WorkAdmissionStep::Admitted(actual) = actual else {
        panic!()
    };
    assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 2);
    assert_eq!(effect(&f, &snapshot).await.status, EffectStatus::Succeeded);
    assert_eq!(effect(&f, &snapshot).await.attempt, 1);
    assert_eq!(
        f.store.get(snapshot.key()).await.unwrap().unwrap().revision,
        snapshot.state().revision
    );
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    *policy.dispatch.write().unwrap() = false;
    f.references.0.store(false, Ordering::SeqCst);
    let again = f
        .for_store(&reopened)
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            None,
            &options("reopened"),
        )
        .await
        .unwrap();
    let WorkAdmissionStep::Admitted(again) = again else {
        panic!()
    };
    assert_eq!(actual, again);
    assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 2);
    *policy.observe.write().unwrap() = false;
    assert!(
        f.bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&service, &policy),
                None,
                &options("denied")
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn work_executor_lost_reply_settles_after_cancel_without_context_or_dispatch() {
    let f = Fixture::new();
    let (snapshot, operation, prepared) = prepare(&f).await;
    ambiguous_claim(&f, &snapshot).await;
    let service = service(&f);
    let policy = Policy::new();
    let admitted = admit_setup_work_protocol_fixture(&service, prepared.admission(), &policy)
        .await
        .unwrap()
        .into_item()
        .id;
    f.bridge()
        .resolve(
            &snapshot,
            Resolution::Cancelled(reference("cancel-work")),
            None,
            f.access(&f.context(200)),
        )
        .await
        .unwrap();
    *policy.dispatch.write().unwrap() = false;
    f.references.0.store(false, Ordering::SeqCst);
    let result = f
        .bridge()
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            None,
            &options("recover"),
        )
        .await
        .unwrap();
    let WorkAdmissionStep::Admitted(actual) = result else {
        panic!()
    };
    assert_eq!(actual.work_item_id(), &admitted);
    assert_eq!(effect(&f, &snapshot).await.attempt, 1);
    assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn work_executor_preserves_uncertainty_then_recovers_late_commit_after_budget() {
    let f = Fixture::new();
    let (snapshot, operation, prepared) = prepare(&f).await;
    let service = service(&f);
    let policy = Policy::new();
    for attempt in 1..=5 {
        let claimed = ambiguous_claim(&f, &snapshot).await;
        assert_eq!(claimed.attempt, attempt);
        assert!(matches!(
            f.bridge()
                .step_work_admission_protocol_fixture(
                    &operation,
                    &host(&service, &policy),
                    None,
                    &options("reconcile")
                )
                .await
                .unwrap(),
            WorkAdmissionStep::RetrySafe
        ));
    }
    assert!(matches!(
        f.bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&service, &policy),
                Some(f.access(&f.context(200))),
                &options("no-budget")
            )
            .await
            .unwrap(),
        WorkAdmissionStep::DispatchBudgetExhausted
    ));
    assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 0);
    let admitted = admit_setup_work_protocol_fixture(&service, prepared.admission(), &policy)
        .await
        .unwrap()
        .into_item()
        .id;
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    *policy.dispatch.write().unwrap() = false;
    let WorkAdmissionStep::Admitted(actual) = f
        .for_store(&reopened)
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            None,
            &options("late"),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(actual.work_item_id(), &admitted);
    assert_eq!(effect(&f, &snapshot).await.attempt, 5);
    let events = f.store.read_events(snapshot.key(), 0, 100).await.unwrap();
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(encoded.contains("RetrySafe"));
    assert!(!encoded.contains("NotApplied"));
}

#[tokio::test]
async fn work_executor_refuses_ephemeral_scope_mismatch_and_unclaimed_owner_work() {
    let f = Fixture::new();
    let (snapshot, operation, prepared) = prepare(&f).await;
    let policy = Policy::new();
    let memory = WorkGraphService::with_scope(
        Arc::new(MemoryWorkGraphStore::new()),
        "tenant-a",
        WorkNamespace::new("setup").unwrap(),
    );
    assert!(matches!(
        SetupWorkBackend::new(
            memory,
            "fixture-memory".into(),
            id("fixture-workgraph"),
            &[meerkat_core::DurabilityDeclaration::durable(
                "workgraph",
                meerkat_core::DurabilityResolution::DeclaredEphemeral,
            )],
        ),
        Err(SetupError::InvalidRecord)
    ));
    assert_eq!(policy.observation_calls.load(Ordering::SeqCst), 0);
    let service = service(&f);
    let wrong = WorkGraphService::with_scope(
        service.store().clone(),
        "tenant-other",
        WorkNamespace::new("setup").unwrap(),
    );
    assert!(matches!(
        f.bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&wrong, &policy),
                Some(f.access(&f.context(200))),
                &options("wrong")
            )
            .await,
        Err(SetupError::WorkAdmission(
            SetupWorkAdmissionError::ScopeMismatch
        ))
    ));
    admit_setup_work_protocol_fixture(&service, prepared.admission(), &policy)
        .await
        .unwrap();
    assert!(matches!(
        f.bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&service, &policy),
                None,
                &options("unclaimed")
            )
            .await,
        Err(SetupError::InvalidRecord)
    ));
    assert_eq!(effect(&f, &snapshot).await.attempt, 0);
}

#[tokio::test]
async fn work_executor_fresh_expired_cancelled_and_revoked_requests_do_not_dispatch() {
    for case in ["expired", "cancelled", "context", "policy", "no-access"] {
        let f = Fixture::new();
        let (snapshot, operation, _) = prepare(&f).await;
        let service = service(&f);
        let policy = Policy::new();
        if case == "cancelled" {
            f.bridge()
                .resolve(
                    &snapshot,
                    Resolution::Cancelled(reference("cancel")),
                    None,
                    f.access(&f.context(200)),
                )
                .await
                .unwrap();
        }
        if case == "context" {
            f.references.0.store(false, Ordering::SeqCst);
        }
        if case == "policy" {
            *policy.dispatch.write().unwrap() = false;
        }
        let context = f.context(if case == "expired" { 950 } else { 200 });
        let result = f
            .bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&service, &policy),
                if case == "no-access" {
                    None
                } else {
                    Some(f.access(&context))
                },
                &options("stopped"),
            )
            .await;
        assert!(
            !matches!(result, Ok(WorkAdmissionStep::Admitted(_))),
            "{case}"
        );
        assert_eq!(effect(&f, &snapshot).await.attempt, 0, "{case}");
        if case == "policy" {
            assert!(matches!(
                result,
                Err(SetupError::WorkAdmission(SetupWorkAdmissionError::Denied))
            ));
            assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 0, "{case}");
        }
    }
}

#[tokio::test]
async fn work_executor_does_not_trust_saved_receipt_or_replacement_empty_database() {
    let f = Fixture::new();
    let (snapshot, operation, _) = prepare(&f).await;
    let service = service(&f);
    let policy = Policy::new();
    f.bridge()
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            Some(f.access(&f.context(200))),
            &options("first"),
        )
        .await
        .unwrap();
    let wrong = WorkGraphService::with_scope(
        Arc::new(
            SqliteWorkGraphStore::open(f.directory.path().join("replacement.sqlite")).unwrap(),
        ),
        "tenant-a",
        WorkNamespace::new("setup").unwrap(),
    );
    assert!(matches!(
        f.bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&wrong, &policy),
                None,
                &options("missing")
            )
            .await,
        Err(SetupError::WorkAdmission(
            SetupWorkAdmissionError::MissingReceipt
        ))
    ));
    let connection = rusqlite::Connection::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let mut saved = effect(&f, &snapshot).await;
    let Some(EffectOutcome::Succeeded { receipt }) = saved.outcome.as_mut() else {
        panic!()
    };
    receipt.body["work_item_id"] = "fabricated".into();
    connection
        .execute(
            "UPDATE toolkit_effects SET data=?1 WHERE id='work-admission'",
            [serde_json::to_string(&saved).unwrap()],
        )
        .unwrap();
    assert!(
        f.bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &host(&service, &policy),
                None,
                &options("forged")
            )
            .await
            .is_err()
    );
    assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_executor_competing_workers_converge_through_real_owner() {
    let f = Fixture::new();
    let (snapshot, operation, prepared) = prepare(&f).await;
    let service = service(&f);
    let policy = Policy::new();
    let bridge = f.bridge();
    let context = f.context(200);
    let host = host(&service, &policy);
    let a = options("a");
    let b = options("b");
    let (a, b) = concurrently(
        bridge.step_work_admission_protocol_fixture(
            &operation,
            &host,
            Some(f.access(&context)),
            &a,
        ),
        bridge.step_work_admission_protocol_fixture(
            &operation,
            &host,
            Some(f.access(&context)),
            &b,
        ),
    )
    .await;
    assert!(a.is_ok() || b.is_ok());
    let WorkAdmissionStep::Admitted(actual) = bridge
        .step_work_admission_protocol_fixture(&operation, &host, None, &options("settled"))
        .await
        .unwrap()
    else {
        panic!()
    };
    let owner = service
        .observe_admission(
            prepared.admission().admission_key().clone(),
            prepared.admission().work_request().clone(),
        )
        .await
        .unwrap()
        .unwrap();
    let meerkat_workgraph::WorkAdmissionOutcome::Replayed(owner) = owner else {
        panic!("work admission observation did not replay the retained item")
    };
    assert_eq!(actual.work_item_id(), &owner.id);
    assert_eq!(effect(&f, &snapshot).await.attempt, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_executor_timed_out_sqlite_call_can_commit_after_local_claim_settles_uncertain() {
    let f = Fixture::new();
    let (snapshot, operation, prepared) = prepare(&f).await;
    let service = service(&f);
    let policy = Policy::new();
    let blocker = rusqlite::Connection::open(f.directory.path().join("work.sqlite")).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut bounds = options("timeout");
    bounds.owner_timeout_ms = 40;
    let result = f
        .bridge()
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            Some(f.access(&f.context(200))),
            &bounds,
        )
        .await;
    assert!(matches!(
        result,
        Err(SetupError::WorkAdmission(SetupWorkAdmissionError::Timeout))
    ));
    assert_eq!(effect(&f, &snapshot).await.status, EffectStatus::Uncertain);
    blocker.execute_batch("COMMIT").unwrap();
    let mut found = false;
    for _ in 0..100 {
        if observe_setup_work(&service, prepared.admission(), &policy)
            .await
            .unwrap()
            .is_some()
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        found,
        "the already dispatched blocking SQLite task should finish"
    );
    *policy.dispatch.write().unwrap() = false;
    let result = f
        .bridge()
        .step_work_admission_protocol_fixture(
            &operation,
            &host(&service, &policy),
            None,
            &options("late"),
        )
        .await
        .unwrap();
    assert!(matches!(result, WorkAdmissionStep::Admitted(_)));
    assert_eq!(effect(&f, &snapshot).await.attempt, 1);
}

struct PausedObservation {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl PausedObservation {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        }
    }
}
#[async_trait]
impl SetupWorkObservationAuthorizer for PausedObservation {
    async fn authorize_observation(
        &self,
        _: &SetupWorkAdmission,
    ) -> Result<(), SetupWorkAdmissionError> {
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(())
    }
    fn with_current_disclosure(
        &self,
        request: &SetupWorkAdmission,
        admission: &meerkat_workgraph::WorkAdmission,
        disclose: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError> {
        // This fixture has no revocation state; it controls only pre-read order.
        if admission.key != *request.admission_key() {
            return Err(SetupWorkAdmissionError::Denied);
        }
        disclose();
        Ok(())
    }
}
fn race_options(worker: &str) -> WorkAdmissionStepOptions {
    WorkAdmissionStepOptions {
        worker: id(worker),
        lease_ms: 5000,
        owner_timeout_ms: 1500,
        settlement_budget_ms: 1000,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_executor_refreshes_attempt_zero_after_peer_admission() {
    for peer_state in ["succeeded", "active", "uncertain", "forged-receipt"] {
        let f = Fixture::new();
        let (snapshot, operation, prepared) = prepare(&f).await;
        let service = service(&f);
        let policy = Policy::new();
        let pause = PausedObservation::new();
        let stale_host = SetupWorkHost {
            backend: backend(&service),
            dispatch: &policy,
            observation: &pause,
        };
        let bridge = f.bridge();
        let bounds = race_options("stale-reader");
        let stale =
            bridge.step_work_admission_protocol_fixture(&operation, &stale_host, None, &bounds);
        let peer = async {
            pause.entered.notified().await;
            assert_eq!(effect(&f, &snapshot).await.attempt, 0);
            if matches!(peer_state, "succeeded" | "forged-receipt") {
                assert!(matches!(
                    bridge
                        .step_work_admission_protocol_fixture(
                            &operation,
                            &host(&service, &policy),
                            Some(f.access(&f.context(200))),
                            &race_options("peer"),
                        )
                        .await
                        .unwrap(),
                    WorkAdmissionStep::Admitted(_)
                ));
                if peer_state == "forged-receipt" {
                    let mut saved = effect(&f, &snapshot).await;
                    let Some(EffectOutcome::Succeeded { receipt }) = saved.outcome.as_mut() else {
                        panic!()
                    };
                    receipt.body["realm"] = "foreign-tenant".into();
                    rusqlite::Connection::open(f.directory.path().join("toolkit.sqlite"))
                        .unwrap()
                        .execute(
                            "UPDATE toolkit_effects SET data=?1 WHERE id='work-admission'",
                            [serde_json::to_string(&saved).unwrap()],
                        )
                        .unwrap();
                }
            } else {
                let original = effect(&f, &snapshot).await;
                let EffectClaimOutcome::Claimed(claimed) = f
                    .store
                    .claim_effect(&original.key, snapshot.state().revision, "peer", 5000)
                    .await
                    .unwrap()
                else {
                    panic!()
                };
                admit_setup_work_protocol_fixture(&service, prepared.admission(), &policy)
                    .await
                    .unwrap();
                if peer_state == "uncertain" {
                    f.store
                        .acknowledge_effect(
                            claimed.lease.as_ref().unwrap(),
                            EffectOutcome::Uncertain {
                                evidence: RecordData {
                                    schema: "test.lost_reply".into(),
                                    version: 1,
                                    body: serde_json::json!({}),
                                },
                            },
                        )
                        .await
                        .unwrap();
                }
            }
            let saved = effect(&f, &snapshot).await;
            pause.resume.notify_one();
            saved
        };
        let (result, before) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            concurrently(stale, peer).await
        })
        .await
        .expect("deterministic peer interleaving did not finish");
        match peer_state {
            "active" => assert!(matches!(
                result,
                Err(SetupError::Store(StoreError::Claimed))
            )),
            "forged-receipt" => assert!(matches!(result, Err(SetupError::InvalidRecord))),
            _ => {
                let WorkAdmissionStep::Admitted(actual) = result.unwrap() else {
                    panic!()
                };
                let owner = observe_setup_work(&service, prepared.admission(), &policy)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(actual, owner);
                assert_eq!(effect(&f, &snapshot).await.status, EffectStatus::Succeeded);
            }
        }
        assert_eq!(effect(&f, &snapshot).await.attempt, 1, "{peer_state}");
        if peer_state != "uncertain" {
            assert_eq!(effect(&f, &snapshot).await, before, "{peer_state}");
        }
    }
}

// Forward to the actual SQLite store, pausing only the second effect read. This
// makes the owner-read/effect-read and effect-read/claim races deterministic.
struct PausedEffectStore<'a> {
    inner: &'a SqliteStore,
    reads: AtomicUsize,
    capture_before_pause: bool,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl<'a> PausedEffectStore<'a> {
    fn new(inner: &'a SqliteStore, capture_before_pause: bool) -> Self {
        Self {
            inner,
            reads: 0.into(),
            capture_before_pause,
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        }
    }
}
#[async_trait]
impl ToolkitStore for PausedEffectStore<'_> {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
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
    async fn recover_record_claim(&self, lease: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(lease).await
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
        if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
            let prior = if self.capture_before_pause {
                self.inner.get_effect(key).await?
            } else {
                None
            };
            self.entered.notify_one();
            self.resume.notified().await;
            if self.capture_before_pause {
                return Ok(prior);
            }
        }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_executor_absence_racing_peer_success_stays_pending_then_reobserves() {
    for capture_before_pause in [false, true] {
        let f = Fixture::new();
        let (snapshot, operation, prepared) = prepare(&f).await;
        let service = service(&f);
        let policy = Policy::new();
        let paused = PausedEffectStore::new(&f.store, capture_before_pause);
        let bridge = SetupStore::new(
            &paused,
            SetupScope::new(f.tenant.clone(), f.deployment.clone()),
        )
        .unwrap();
        let context = f.context(200);
        let current_host = host(&service, &policy);
        let bounds = race_options("absence-reader");
        let stale = bridge.step_work_admission_protocol_fixture(
            &operation,
            &current_host,
            Some(f.access(&context)),
            &bounds,
        );
        let peer = async {
            paused.entered.notified().await;
            assert!(
                observe_setup_work(&service, prepared.admission(), &policy)
                    .await
                    .unwrap()
                    .is_none()
            );
            let WorkAdmissionStep::Admitted(actual) = f
                .bridge()
                .step_work_admission_protocol_fixture(
                    &operation,
                    &current_host,
                    Some(f.access(&context)),
                    &race_options("peer"),
                )
                .await
                .unwrap()
            else {
                panic!()
            };
            let before = effect(&f, &snapshot).await;
            let events = f.store.read_events(snapshot.key(), 0, 100).await.unwrap();
            paused.resume.notify_one();
            (actual, before, events)
        };
        let (result, (actual, before, events)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                concurrently(stale, peer).await
            })
            .await
            .expect("deterministic peer interleaving did not finish");
        assert!(matches!(result.unwrap(), WorkAdmissionStep::Pending));
        assert_eq!(effect(&f, &snapshot).await, before);
        assert_eq!(
            f.store.read_events(snapshot.key(), 0, 100).await.unwrap(),
            events
        );
        let WorkAdmissionStep::Admitted(recovered) = f
            .bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &current_host,
                None,
                &options("reobserve"),
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(actual, recovered);
        assert_eq!(effect(&f, &snapshot).await.attempt, 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn work_executor_peer_settlement_after_refreshed_observation_is_retryable() {
    let f = Fixture::new();
    let (snapshot, operation, prepared) = prepare(&f).await;
    let service = service(&f);
    let policy = Policy::new();
    ambiguous_claim(&f, &snapshot).await;
    admit_setup_work_protocol_fixture(&service, prepared.admission(), &policy)
        .await
        .unwrap();
    let paused = PausedEffectStore::new(&f.store, true);
    let bridge = SetupStore::new(
        &paused,
        SetupScope::new(f.tenant.clone(), f.deployment.clone()),
    )
    .unwrap();
    let current_host = host(&service, &policy);
    let bounds = race_options("old-uncertain");
    let stale =
        bridge.step_work_admission_protocol_fixture(&operation, &current_host, None, &bounds);
    let peer = async {
        paused.entered.notified().await;
        let WorkAdmissionStep::Admitted(actual) = f
            .bridge()
            .step_work_admission_protocol_fixture(
                &operation,
                &current_host,
                None,
                &race_options("settle-peer"),
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let before = effect(&f, &snapshot).await;
        paused.resume.notify_one();
        (actual, before)
    };
    let (result, (actual, before)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            concurrently(stale, peer).await
        })
        .await
        .expect("deterministic peer interleaving did not finish");
    assert!(matches!(result.unwrap(), WorkAdmissionStep::Pending));
    assert_eq!(effect(&f, &snapshot).await, before);
    let WorkAdmissionStep::Admitted(recovered) = f
        .bridge()
        .step_work_admission_protocol_fixture(&operation, &current_host, None, &options("retry"))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(actual, recovered);
    assert_eq!(effect(&f, &snapshot).await.attempt, 1);
}

#[tokio::test]
async fn production_native_work_entrypoints_refuse_before_any_owner_call() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let operation = id(&snapshot.key().id);
    let service = service(&f);
    let policy = Policy::new();
    let host = SetupWorkHost {
        backend: backend(&service),
        dispatch: &policy,
        observation: &policy,
    };
    let before = f.store.get(snapshot.key()).await.unwrap().unwrap();
    let effects = f
        .store
        .list_effects(snapshot.key(), None, 100)
        .await
        .unwrap();
    struct Never;
    impl SetupWorkPreparer for Never {
        fn prepare(
            &self,
            _: &ValidatedSetupRequest,
        ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
            panic!("native refusal must precede preparation and content exposure")
        }
    }
    impl ContextReferenceOwner for Never {
        fn verify_access(
            &self,
            _: &ContextReference,
            _: &SetupRequestEvidence,
            _: &SetupUseContext<'_>,
        ) -> Result<(), RequestError> {
            panic!("native refusal must precede reopening context references")
        }
    }
    let context = f.context(200);
    let access = || SetupAccess {
        evidence: &f.authority,
        context: &context,
        references: &Never,
    };
    assert!(matches!(
        f.bridge().prepare_work(&operation, access(), &Never).await,
        Err(SetupError::NativeExecutionUnavailable)
    ));
    assert!(matches!(
        f.bridge()
            .step_work_admission(&operation, &host, Some(access()), &options("guard"))
            .await,
        Err(SetupError::NativeExecutionUnavailable)
    ));
    assert!(matches!(
        f.bridge()
            .step_work_admission(&operation, &host, None, &options("guard"))
            .await,
        Err(SetupError::NativeExecutionUnavailable)
    ));
    assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 0);
    assert_eq!(policy.observation_calls.load(Ordering::SeqCst), 0);
    let after = f.store.get(snapshot.key()).await.unwrap().unwrap();
    assert_eq!(before.content_digest, after.content_digest);
    assert_eq!(before.revision, after.revision);
    assert_eq!(
        serde_json::to_value(effects).unwrap(),
        serde_json::to_value(
            f.store
                .list_effects(snapshot.key(), None, 100)
                .await
                .unwrap()
        )
        .unwrap()
    );
}

/// Poll both borrowed operations without macro expansion hiding either body from
/// source governance. Completed futures are never polled again.
async fn concurrently<A, B>(first: A, second: B) -> (A::Output, B::Output)
where
    A: std::future::Future,
    B: std::future::Future,
{
    let mut first = Box::pin(first);
    let mut second = Box::pin(second);
    let mut first_result = None;
    let mut second_result = None;
    std::future::poll_fn(|context| {
        if first_result.is_none()
            && let std::task::Poll::Ready(value) = first.as_mut().poll(context)
        {
            first_result = Some(value);
        }
        if second_result.is_none()
            && let std::task::Poll::Ready(value) = second.as_mut().poll(context)
        {
            second_result = Some(value);
        }
        if first_result.is_some() && second_result.is_some() {
            std::task::Poll::Ready((first_result.take().unwrap(), second_result.take().unwrap()))
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

struct DeniedInputOwner(AtomicUsize);
impl ContextReferenceOwner for DeniedInputOwner {
    fn verify_access(
        &self,
        _: &ContextReference,
        _: &SetupRequestEvidence,
        _: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(RequestError::ContextUnavailable)
    }
}
struct PermittedInputOwner;
impl ContextReferenceOwner for PermittedInputOwner {
    fn verify_access(
        &self,
        _: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        Ok(())
    }
}
struct StepInputPrepare<'a>(&'a str);
impl SetupWorkPreparer for StepInputPrepare<'_> {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        let mut value = Prepare.prepare(request)?;
        if self.0 != "external" {
            value.input_reference = canonical_setup_input_reference(request).unwrap();
            match self.0 {
                "canonical" => {}
                "owner" => value.input_reference.owner = id("different-owner"),
                "id" => value.input_reference.id = id("different-input"),
                "revision" => value.input_reference.revision = rev("different-revision"),
                _ => panic!("unknown input control"),
            }
        }
        Ok(value)
    }
}
fn inline_input_fixture() -> Fixture {
    let mut f = Fixture::new();
    let mut request = f.request.request().clone();
    request.user_context.clear();
    f.request = validate_request(
        request, f.request.host().clone(), &f.authority, &f.context(200), &f.references,
    ).unwrap();
    f
}

#[tokio::test]
async fn legacy_step_accepts_canonical_inline_input_with_denied_external_owner() {
    let f = inline_input_fixture();
    let snapshot = f.admit().await;
    let operation = id(&snapshot.key().id);
    let denied = DeniedInputOwner(AtomicUsize::new(0));
    let context = f.context(200);
    let access = || SetupAccess { evidence: &f.authority, context: &context, references: &denied };
    let prepared = f.bridge().prepare_work_protocol_fixture(&operation, access(), &StepInputPrepare("canonical"))
        .await.unwrap();
    let service = service(&f);
    let policy = Policy::new();
    let host = host(&service, &policy);
    let WorkAdmissionStep::Admitted(done) = f.bridge()
        .step_work_admission_protocol_fixture(&operation, &host, Some(access()), &options("inline"))
        .await.unwrap()
    else { panic!("canonical input was not admitted") };
    let WorkAdmissionStep::Admitted(replay) = f.bridge()
        .step_work_admission_protocol_fixture(&operation, &host, Some(access()), &options("inline-replay"))
        .await.unwrap()
    else { panic!("canonical admission did not replay") };
    assert_eq!(done, replay);
    assert_eq!(done.correlation_digest(), prepared.admission().correlation_digest());
    assert_eq!(denied.0.load(Ordering::SeqCst), 0);
    assert_eq!(effect(&f, &snapshot).await.attempt, 1);
    assert_eq!(effect(&f, &snapshot).await.status, EffectStatus::Succeeded);
}

#[tokio::test]
async fn legacy_step_preserves_changed_descriptor_and_external_owner_refusal() {
    for kind in ["owner", "id", "revision", "external", "user-context"] {
        let f = if kind == "user-context" { Fixture::new() } else { inline_input_fixture() };
        let snapshot = f.admit().await;
        let operation = id(&snapshot.key().id);
        let context = f.context(200);
        let allow = SetupAccess { evidence: &f.authority, context: &context, references: &PermittedInputOwner };
        let prepared = f.bridge().prepare_work_protocol_fixture(&operation, allow,
            &StepInputPrepare(if kind == "user-context" { "canonical" } else { kind })).await.unwrap();
        let service = service(&f);
        let policy = Policy::new();
        let host = host(&service, &policy);
        let denied = DeniedInputOwner(AtomicUsize::new(0));
        let access = SetupAccess { evidence: &f.authority, context: &context, references: &denied };
        let before = effect(&f, &snapshot).await;
        assert!(matches!(f.bridge().step_work_admission_protocol_fixture(
            &operation, &host, Some(access), &options("refused")).await,
            Err(SetupError::Request(RequestError::ContextUnavailable))), "{kind}");
        assert_eq!(denied.0.load(Ordering::SeqCst), 1, "{kind}");
        assert_eq!(policy.dispatch_calls.load(Ordering::SeqCst), 0, "{kind}");
        assert_eq!(effect(&f, &snapshot).await, before);
        assert!(service.observe_admission(prepared.admission().admission_key().clone(),
            prepared.admission().work_request().clone()).await.unwrap().is_none());
    }
}

//! Prepared lifecycle controls over the real installed pair and native runtime.
//! No member, model, input admission or acquisition Ready is constructed.
//! The barrier retains an entered canonical census read, not a synthetic task.
use super::*;
use std::sync::{OnceLock, atomic::{AtomicBool, AtomicUsize, Ordering}};
use toolkit_store::*;

struct PausedSourceStore {
    inner: Arc<dyn ToolkitStore>,
    scope: StoreScope,
    armed: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    started: AtomicUsize,
    reads: AtomicUsize,
    completed: AtomicUsize,
    cancelled: AtomicUsize,
}
impl PausedSourceStore {
    fn new(inner: Arc<dyn ToolkitStore>, scope: StoreScope) -> Self {
        Self { inner, scope, armed: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
            started: AtomicUsize::new(0), reads: AtomicUsize::new(0), completed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0) }
    }
    fn arm(&self) {
        assert!(!self.armed.swap(true, Ordering::SeqCst));
    }
}
struct EnteredRead<'a> { store: &'a PausedSourceStore, complete: bool }
impl Drop for EnteredRead<'_> {
    fn drop(&mut self) {
        if !self.complete { self.store.cancelled.fetch_add(1, Ordering::SeqCst); }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for PausedSourceStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        self.inner.get(key).await
    }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32)
        -> StoreResult<Vec<StoredRecord>>
    {
        if scope == &self.scope { self.reads.fetch_add(1, Ordering::SeqCst); }
        if scope == &self.scope && self.armed.swap(false, Ordering::SeqCst) {
            assert!(after.is_none());
            assert_eq!(limit, 32, "observe the first bounded canonical census read");
            self.started.fetch_add(1, Ordering::SeqCst);
            let mut entered = EnteredRead { store: self, complete: false };
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            let result = self.inner.list_records(scope, after, limit).await;
            self.completed.fetch_add(1, Ordering::SeqCst);
            entered.complete = true;
            result
        } else { self.inner.list_records(scope, after, limit).await }
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(&self, key: &StoreKey, revision: u64, worker: &str, ttl: u64)
        -> StoreResult<RecordLease>
    { self.inner.claim_record(key, revision, worker, ttl).await }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
    }
    async fn read_events(&self, key: &StoreKey, after: u64, limit: u32) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(&self, key: &StoreKey, after: Option<&str>, limit: u32)
        -> StoreResult<Vec<StoredEffect>>
    { self.inner.list_effects(key, after, limit).await }
    async fn claim_effect(&self, key: &EffectKey, revision: u64, worker: &str, ttl: u64)
        -> StoreResult<EffectClaimOutcome>
    { self.inner.claim_effect(key, revision, worker, ttl).await }
    async fn acknowledge_effect(&self, lease: &EffectLease, outcome: EffectOutcome)
        -> StoreResult<StoredEffect>
    { self.inner.acknowledge_effect(lease, outcome).await }
    async fn claim_reconciliation(&self, key: &EffectKey, worker: &str, ttl: u64)
        -> StoreResult<StoredEffect>
    { self.inner.claim_reconciliation(key, worker, ttl).await }
    async fn reconcile_effect(&self, lease: &EffectLease, resolution: Reconciliation)
        -> StoreResult<StoredEffect>
    { self.inner.reconcile_effect(lease, resolution).await }
}

#[derive(Clone, Copy)]
enum CloseCase { BeforeAdoption, FailedAdoption, CancelledJoin }

async fn prepared_close_control(case: CloseCase) {
    use meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy;
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let local: CompiledLocalPolicy = serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let selected = local.native_host.as_ref().unwrap();
    let directory = state_directory(&selection, selected, NativeOwnerStartup::Restore).unwrap();
    let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
    let scope = StoreScope { realm: selected.realm.to_string(), owner: local.deployment.to_string() };
    let selected_store = Arc::new(OnceLock::<Arc<PausedSourceStore>>::new());
    let captured_store = selected_store.clone();
    let captured_scope = scope.clone();
    let policy = PreparedInstalledToolPolicy::from_canonical(
        &read_bundle_artifact(&selection, &selected.application_tool_policy).unwrap()).unwrap();
    let registry = policy.registry();
    let prepared = PreparedInstallationOwner::open(selection.deployment_root.clone(), PreparedLocalPolicy {
        deployment: local.deployment.clone(), components: local.components.clone(),
        channels: local.channels.clone(), native_host: selected.preparation(),
    }, operator_setup::OperatorOwnerStartup::Restore).unwrap()
        .complete_with_source_store(policy, Box::new(move |inner| {
            let store = Arc::new(PausedSourceStore::new(inner, captured_scope));
            assert!(captured_store.set(store.clone()).is_ok());
            store
        })).await.unwrap();
    let originals = prepared.originals();
    let store = selected_store.get().unwrap().clone();
    let policy_rows = store.list_records(&scope, None, 32).await.unwrap();
    assert_eq!(policy_rows.len(), 1);
    assert_eq!(policy_rows[0].data.schema, "toolkit.application-service-tool-policy");
    let registration = Arc::new(prepared_registration::PreparedMemberRegistration::new(
        Arc::new(UnenteredPreparationRegistration), registry,
        Arc::new(|_, _| panic!("empty prepared control must not select a destination")),
    ));
    let participant = prepared.gateway_runtime(registration,
        acquisition::InstalledAcquisitionTools::new(Duration::from_secs(10)).unwrap(),
        [toolkit_package::Id::new("hook-fixture-viewer").unwrap()].into()).unwrap();
    // HostJobSource requires the same real runtime store/jobs as this service.
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::SqliteSessionStore::open(directory.join("prepared-native-sessions.sqlite3")).unwrap()),
        runtime_store.clone(), Arc::new(meerkat::MemoryBlobStore::new()),
        prepared.take_configuration().unwrap(),
    ).unwrap();
    let jobs = persistence.job_store();
    let (service, machine) = meerkat::surface::build_runtime_backed_service(
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default()),
        1, persistence,
    ).unwrap();
    let service = Arc::new(service);
    let spec = MobBootstrapSpec::new(definition(), MobStorage::in_memory(), service.clone())
        .with_session_runtime_adapter(machine).unwrap().with_declared_ephemeral_mob_storage()
        .with_runtime_delivery(meerkat_mobkit::mob_handle_runtime::MobRuntimeDelivery::new(runtime_store, jobs));
    let spec = participant.prepare_bootstrap(spec).unwrap();
    let mut prepared = Some(prepared);
    let mut native = UnifiedRuntime::bootstrap(spec, MetaConfig {
        modules: Vec::new(), discovery: DiscoverySpec { namespace: "factory".into(), modules: Vec::new() },
        pre_spawn: Vec::new(),
    }, Duration::from_secs(10)).await.unwrap();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(15), async {
        native.install_identity_first_context(context(&native, true));
        store.arm();
        participant.bind_identity_runtime(&mut native).unwrap();
        store.entered.notified().await;
        assert_eq!(store.started.load(Ordering::SeqCst), 1);
        assert_eq!(store.completed.load(Ordering::SeqCst), 0);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        assert!(originals.bound_identity_runtime().is_ok());
        assert!(service.list(SessionQuery::default()).await.unwrap().is_empty());

        if matches!(case, CloseCase::FailedAdoption) {
            // Consume the actual preparation on an invalid selected deployment.
            // Its retained participant must still own and join the entered read.
            let foreign = tempfile::tempdir().unwrap();
            let mut wrong = selection.clone();
            wrong.deployment_root = foreign.path().to_path_buf();
            assert!(matches!(prepared.take().unwrap().adopt(wrong),
                Err(HostLifetimeError::InvalidSelection)));
        }
        participant.begin_custody_close();
        let mut first_join = Box::pin(participant.join_custody_close());
        assert!(matches!(futures::poll!(first_join.as_mut()), std::task::Poll::Pending),
            "prepared close must retain the actual entered read");
        if matches!(case, CloseCase::CancelledJoin) {
            drop(first_join);
            assert_eq!(store.completed.load(Ordering::SeqCst), 0);
            assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
            let mut same_join = Box::pin(participant.join_custody_close());
            assert!(matches!(futures::poll!(same_join.as_mut()), std::task::Poll::Pending),
                "cancelling the observer must preserve the same running task");
            store.release.add_permits(1);
            same_join.await.unwrap();
        } else {
            store.release.add_permits(1);
            first_join.await.unwrap();
        }
        assert_eq!(store.completed.load(Ordering::SeqCst), 1);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        participant.join_custody_close().await.unwrap();
        assert!(originals.start_custody_recovery(None).is_err(), "closed source cannot restart recovery");
        assert_eq!(store.list_records(&scope, None, 32).await.unwrap(), policy_rows,
            "prepared close preserves the exact selected service policy row");
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), grant_bytes);
    })).catch_unwind().await;

    // Always release the controlled read and join both real owners, including
    // after a body timeout, panic or cancellation of the first join observer.
    participant.begin_custody_close();
    store.release.add_permits(1);
    let report = native.shutdown().await;
    let joined = participant.join_custody_close().await;
    assert!(joined.is_ok());
    assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
    assert!(report.cleanup_completed());
    assert!(match report.mob_terminal_shutdown {
        MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
        MobTerminalShutdownOutcome::AlreadyShutDown => true,
        _ => false,
    });
    drop(prepared);
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded prepared recovery close control"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test]
async fn prepared_close_before_adoption_joins_entered_canonical_read() {
    prepared_close_control(CloseCase::BeforeAdoption).await;
}
#[tokio::test]
async fn failed_prepared_adoption_keeps_entered_recovery_owned_until_join() {
    prepared_close_control(CloseCase::FailedAdoption).await;
}
#[tokio::test]
async fn cancelled_prepared_join_observer_preserves_exact_rejoin() {
    prepared_close_control(CloseCase::CancelledJoin).await;
}

#[path = "cold_restore_tests.rs"]
mod cold_restore_tests;

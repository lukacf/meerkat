use crate::{
    request::{ContextReference, TaskContinuation},
    work_admission::{
        HostWorkGraphScope, SetupWorkAdmission, SetupWorkAdmissionError, SetupWorkAuthorizer,
        SetupWorkCorrelation, SetupWorkObservationAuthorizer, WorkAdmissionResult,
        admit_setup_work_protocol_fixture, observe_setup_work,
    },
};
use async_trait::async_trait;
use meerkat_workgraph::{
    CreateWorkItemRequest, ExternalWorkRef, MemoryWorkGraphStore, SqliteWorkGraphStore,
    UpdateWorkItemRequest, WorkGraphError, WorkGraphEventFilter,
    WorkGraphEventKind, WorkGraphService, WorkGraphStore, WorkItemFilter, WorkNamespace,
    WorkPriority,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use toolkit_package::{ContentDigest, Id, Revision};

fn id(value: &str) -> Id {
    Id::new(value).expect("valid fixture identity")
}

fn scope() -> HostWorkGraphScope {
    HostWorkGraphScope::new(id("tenant-a"), WorkNamespace::new("setup-a").unwrap()).unwrap()
}

fn correlation() -> SetupWorkCorrelation {
    SetupWorkCorrelation {
        operation: id("operation-a"),
        effect: id("effect-a"),
        input_digest: ContentDigest::of_bytes(b"full original validated input"),
        input_reference: ContextReference {
            owner: id("request-owner"),
            id: id("request-a"),
            revision: Revision::new("revision-1").unwrap(),
            content_digest: ContentDigest::of_bytes(b"original user request and supplied context"),
        },
        continuation: TaskContinuation { native_original: None,
            owner: id("original-task-owner"),
            task: id("original-task"),
            revision: Revision::new("original-task-revision").unwrap(),
            delivery_key: id("original-task-delivery"),
            audience: [id("person-a")].into(),
        },
    }
}

fn work_request() -> CreateWorkItemRequest {
    CreateWorkItemRequest {
        title: "Investigate the original setup request".into(),
        description: Some(
            "The trusted bridge supplied this text exactly.\n  Preserve spacing.".into(),
        ),
        priority: WorkPriority::High,
        labels: BTreeSet::from(["host-issued-cycle-a".into()]),
        external_refs: vec![ExternalWorkRef {
            kind: "host-immutable-context".into(),
            id: "request-a@revision-1".into(),
            url: None,
        }],
        ..Default::default()
    }
}

fn admission() -> SetupWorkAdmission {
    SetupWorkAdmission::from_host_protocol_fixture(scope(), correlation(), work_request()).unwrap()
}

fn service(store: Arc<dyn WorkGraphStore>) -> WorkGraphService {
    WorkGraphService::with_scope(store, "tenant-a", WorkNamespace::new("setup-a").unwrap())
}

/// Test host which grants only the exact preapproved immutable request and
/// explicitly models revocation. This is not a production default authorizer.
struct TestHost {
    approved_correlation: ContentDigest,
    approved_payload: serde_json::Value,
    allowed: Arc<std::sync::RwLock<bool>>,
    checks: AtomicUsize,
}

impl TestHost {
    fn approves(request: &SetupWorkAdmission) -> Self {
        Self {
            approved_correlation: request.correlation_digest().clone(),
            approved_payload: serde_json::to_value(request.work_request()).unwrap(),
            allowed: Arc::new(std::sync::RwLock::new(true)),
            checks: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl SetupWorkAuthorizer for TestHost {
    async fn admission_fence(
        &self,
        request: &SetupWorkAdmission,
        exact: &meerkat_workgraph::WorkAdmission,
    ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if !*self.allowed.read().unwrap()
            || request.correlation_digest() != &self.approved_correlation
            || serde_json::to_value(request.work_request()).unwrap() != self.approved_payload
        {
            return Err(SetupWorkAdmissionError::Denied);
        }
        Ok(Arc::new(super::test_fence::TestAdmissionFence {
            allowed: self.allowed.clone(),
            scope: request.scope().clone(),
            admission: exact.clone(),
        }))
    }
}

async fn assert_single_creation(store: &dyn WorkGraphStore) {
    let items = store
        .list_items(WorkItemFilter {
            realm_id: Some("tenant-a".into()),
            namespace: Some(WorkNamespace::new("setup-a").unwrap()),
            include_terminal: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    let events = store
        .list_events(WorkGraphEventFilter::default())
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == WorkGraphEventKind::Created)
            .count(),
        1
    );
}

async fn concurrent_same_key(store: Arc<dyn WorkGraphStore>) {
    let service = Arc::new(service(store.clone()));
    let request = Arc::new(admission());
    let host = Arc::new(TestHost::approves(&request));
    let barrier = Arc::new(tokio::sync::Barrier::new(24));
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let service = service.clone();
        let request = request.clone();
        let host = host.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            admit_setup_work_protocol_fixture(&service, &request, host.as_ref())
                .await
                .unwrap()
        }));
    }
    let mut identities = BTreeSet::new();
    let mut created = 0;
    for task in tasks {
        let receipt = task.await.unwrap();
        created += usize::from(matches!(receipt, WorkAdmissionResult::Created(_)));
        identities.insert(receipt.item().id.clone());
        assert_eq!(receipt.item().realm_id, "tenant-a");
        assert_eq!(receipt.item().namespace.as_str(), "setup-a");
    }
    assert_eq!(created, 1);
    assert_eq!(identities.len(), 1);
    assert_eq!(host.checks.load(Ordering::SeqCst), 24);
    assert_single_creation(store.as_ref()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_concurrent_calls_admit_one_work_item() {
    concurrent_same_key(Arc::new(MemoryWorkGraphStore::new())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sqlite_concurrent_calls_admit_one_work_item() {
    let directory = tempfile::tempdir().unwrap();
    concurrent_same_key(Arc::new(
        SqliteWorkGraphStore::open(directory.path().join("workgraph.sqlite3")).unwrap(),
    ))
    .await;
}

#[tokio::test]
async fn lost_reply_replays_after_sqlite_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workgraph.sqlite3");
    let request = admission();
    let host = TestHost::approves(&request);
    let original = {
        let service = service(Arc::new(SqliteWorkGraphStore::open(&path).unwrap()));
        admit_setup_work_protocol_fixture(&service, &request, &host)
            .await
            .unwrap()
            .into_item()
    };
    // The setup store received no acknowledgment. Recovery rebuilds the exact
    // request from its persisted effect rather than choosing another identity.
    let reconstructed = admission();
    assert_eq!(reconstructed.admission_key(), request.admission_key());
    let reopened = Arc::new(SqliteWorkGraphStore::open(&path).unwrap());
    let recovered =
        admit_setup_work_protocol_fixture(&service(reopened.clone()), &reconstructed, &host)
            .await
            .unwrap();
    assert!(matches!(recovered, WorkAdmissionResult::Existing(_)));
    assert_eq!(recovered.item(), &original);
    assert_eq!(host.checks.load(Ordering::SeqCst), 2);
    assert_single_creation(reopened.as_ref()).await;
}

#[tokio::test]
async fn replay_returns_current_owner_revision_without_new_admission() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = service(store.clone());
    let request = admission();
    let host = TestHost::approves(&request);
    let original = admit_setup_work_protocol_fixture(&service, &request, &host)
        .await
        .unwrap()
        .into_item();
    let changed = service
        .update(UpdateWorkItemRequest {
            id: original.id.clone(),
            realm_id: Some("tenant-a".into()),
            namespace: Some(WorkNamespace::new("setup-a").unwrap()),
            expected_revision: original.revision,
            title: Some("Investigation progressed under the WorkGraph owner".into()),
            description: None,
            priority: None,
            completion_policy: None,
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
        })
        .await
        .unwrap();
    let replay = admit_setup_work_protocol_fixture(&service, &request, &host)
        .await
        .unwrap();
    assert!(matches!(replay, WorkAdmissionResult::Existing(_)));
    assert_eq!(replay.item(), &changed);
    assert!(replay.item().revision > original.revision);
    assert_single_creation(store.as_ref()).await;
}

async fn changed_work_payload_conflicts(store: Arc<dyn WorkGraphStore>) {
    let service = service(store.clone());
    let original = admission();
    admit_setup_work_protocol_fixture(&service, &original, &TestHost::approves(&original))
        .await
        .unwrap();
    for changed in [
        CreateWorkItemRequest {
            title: "Changed title".into(),
            ..work_request()
        },
        CreateWorkItemRequest {
            description: Some("Changed description".into()),
            ..work_request()
        },
        CreateWorkItemRequest {
            priority: WorkPriority::Low,
            ..work_request()
        },
        CreateWorkItemRequest {
            labels: BTreeSet::from(["different-cycle".into()]),
            ..work_request()
        },
        CreateWorkItemRequest {
            external_refs: Vec::new(),
            ..work_request()
        },
    ] {
        let changed =
            SetupWorkAdmission::from_host_protocol_fixture(scope(), correlation(), changed)
                .unwrap();
        assert_eq!(changed.admission_key(), original.admission_key());
        let error =
            admit_setup_work_protocol_fixture(&service, &changed, &TestHost::approves(&changed))
                .await
                .unwrap_err();
        assert!(matches!(
            error,
            SetupWorkAdmissionError::WorkGraph(WorkGraphError::Conflict(_))
        ));
    }
    assert_single_creation(store.as_ref()).await;
}

#[tokio::test]
async fn memory_changed_work_payload_conflicts() {
    changed_work_payload_conflicts(Arc::new(MemoryWorkGraphStore::new())).await;
}

#[tokio::test]
async fn sqlite_changed_work_payload_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    changed_work_payload_conflicts(Arc::new(
        SqliteWorkGraphStore::open(directory.path().join("workgraph.sqlite3")).unwrap(),
    ))
    .await;
}

#[tokio::test]
async fn changed_immutable_context_conflicts_under_original_effect_identity() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = service(store.clone());
    let original = admission();
    admit_setup_work_protocol_fixture(&service, &original, &TestHost::approves(&original))
        .await
        .unwrap();
    let mut changed_input = correlation();
    changed_input.input_digest = ContentDigest::of_bytes(b"changed original input");
    let mut changed_context = correlation();
    changed_context.input_reference.revision = Revision::new("revision-2").unwrap();
    let mut changed_content = correlation();
    changed_content.input_reference.content_digest = ContentDigest::of_bytes(b"changed context");
    let mut changed_continuation = correlation();
    changed_continuation.continuation.delivery_key = id("different-continuation");
    for correlation in [
        changed_input,
        changed_context,
        changed_content,
        changed_continuation,
    ] {
        let changed =
            SetupWorkAdmission::from_host_protocol_fixture(scope(), correlation, work_request())
                .unwrap();
        assert_eq!(changed.admission_key(), original.admission_key());
        assert_ne!(changed.correlation_digest(), original.correlation_digest());
        assert!(matches!(
            admit_setup_work_protocol_fixture(&service, &changed, &TestHost::approves(&changed))
                .await
                .unwrap_err(),
            SetupWorkAdmissionError::WorkGraph(WorkGraphError::Conflict(_))
        ));
    }
    assert_single_creation(store.as_ref()).await;
}

#[tokio::test]
async fn revoked_host_permission_denies_even_an_exact_replay() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = service(store.clone());
    let request = admission();
    let host = TestHost::approves(&request);
    admit_setup_work_protocol_fixture(&service, &request, &host)
        .await
        .unwrap();
    *host.allowed.write().unwrap() = false;
    assert!(matches!(
        admit_setup_work_protocol_fixture(&service, &request, &host)
            .await
            .unwrap_err(),
        SetupWorkAdmissionError::Denied
    ));
    assert_eq!(host.checks.load(Ordering::SeqCst), 2);
    assert_single_creation(store.as_ref()).await;
}

#[tokio::test]
async fn denied_host_permission_commits_no_work() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = service(store.clone());
    let request = admission();
    let host = TestHost::approves(&request);
    *host.allowed.write().unwrap() = false;
    assert!(matches!(
        admit_setup_work_protocol_fixture(&service, &request, &host)
            .await
            .unwrap_err(),
        SetupWorkAdmissionError::Denied
    ));
    assert!(
        store
            .list_items(WorkItemFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn service_scope_mismatch_is_rejected_before_host_callback() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let request = admission();
    let host = TestHost::approves(&request);
    for service in [
        WorkGraphService::with_scope(
            store.clone(),
            "tenant-b",
            WorkNamespace::new("setup-a").unwrap(),
        ),
        WorkGraphService::with_scope(
            store.clone(),
            "tenant-a",
            WorkNamespace::new("setup-b").unwrap(),
        ),
    ] {
        assert!(matches!(
            admit_setup_work_protocol_fixture(&service, &request, &host)
                .await
                .unwrap_err(),
            SetupWorkAdmissionError::ScopeMismatch
        ));
    }
    assert_eq!(host.checks.load(Ordering::SeqCst), 0);
    assert!(
        store
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[test]
fn scope_mismatch_cannot_be_imported_from_work_request() {
    for request in [
        CreateWorkItemRequest {
            realm_id: Some("tenant-b".into()),
            ..work_request()
        },
        CreateWorkItemRequest {
            namespace: Some(WorkNamespace::new("setup-b").unwrap()),
            ..work_request()
        },
    ] {
        assert!(matches!(
            SetupWorkAdmission::from_host_protocol_fixture(scope(), correlation(), request)
                .unwrap_err(),
            SetupWorkAdmissionError::ScopeMismatch
        ));
    }
}

#[test]
fn exact_host_scope_is_resolved_once_and_payload_is_preserved() {
    let original = work_request();
    let frozen = admission();
    let mut expected = serde_json::to_value(&original).unwrap();
    expected["realm_id"] = serde_json::json!("tenant-a");
    expected["namespace"] = serde_json::json!("setup-a");
    let mut actual = serde_json::to_value(frozen.work_request()).unwrap();
    let references = actual["external_refs"].as_array_mut().unwrap();
    let correlation_ref = references.pop().unwrap();
    assert_eq!(expected, actual);
    assert_eq!(
        correlation_ref["id"],
        serde_json::json!(frozen.correlation_digest())
    );
    assert_eq!(frozen.correlation(), &correlation());
    let mut explicit = original;
    explicit.realm_id = Some("tenant-a".into());
    explicit.namespace = Some(WorkNamespace::new("setup-a").unwrap());
    let explicit =
        SetupWorkAdmission::from_host_protocol_fixture(scope(), correlation(), explicit).unwrap();
    assert_eq!(explicit.admission_key(), frozen.admission_key());
    assert_eq!(
        serde_json::to_value(explicit.work_request()).unwrap(),
        serde_json::to_value(frozen.work_request()).unwrap()
    );
}

#[test]
fn supplied_reserved_correlation_reference_is_rejected() {
    let mut request = work_request();
    request.external_refs.push(ExternalWorkRef {
        kind: "meerkat_toolkit.setup_work_correlation.v1".into(),
        id: "caller-authored-digest".into(),
        url: None,
    });
    assert!(matches!(
        SetupWorkAdmission::from_host_protocol_fixture(scope(), correlation(), request)
            .unwrap_err(),
        SetupWorkAdmissionError::ReservedCorrelation
    ));
}

#[test]
fn independent_operation_effect_and_host_scope_have_distinct_keys() {
    let original = admission();
    let mut other_operation = correlation();
    other_operation.operation = id("operation-b");
    let mut other_effect = correlation();
    other_effect.effect = id("effect-b");
    let other_scope =
        HostWorkGraphScope::new(id("tenant-b"), WorkNamespace::new("setup-a").unwrap()).unwrap();
    for request in [
        SetupWorkAdmission::from_host_protocol_fixture(scope(), other_operation, work_request())
            .unwrap(),
        SetupWorkAdmission::from_host_protocol_fixture(scope(), other_effect, work_request())
            .unwrap(),
        SetupWorkAdmission::from_host_protocol_fixture(other_scope, correlation(), work_request())
            .unwrap(),
    ] {
        assert_ne!(request.admission_key(), original.admission_key());
        assert_ne!(request.correlation_digest(), original.correlation_digest());
    }
}

#[test]
fn malformed_deserialized_namespace_does_not_bypass_host_scope_validation() {
    // The upstream transparent WorkNamespace decoder does not run its smart
    // constructor. This boundary rechecks it before accepting a host scope.
    for value in ["", " leading", "trailing ", "control\nvalue"] {
        let namespace: WorkNamespace = serde_json::from_value(serde_json::json!(value)).unwrap();
        assert!(HostWorkGraphScope::new(id("tenant-a"), namespace).is_err());
    }
}

#[test]
fn debug_output_does_not_disclose_original_request_or_context() {
    let request = admission();
    let debug = format!("{request:?} {:?}", request.correlation());
    assert!(!debug.contains("Preserve spacing"));
    assert!(!debug.contains("request-owner"));
    assert!(!debug.contains("original-task-delivery"));
}

struct ObservationHost {
    exact: TestHost,
}
#[async_trait]
impl SetupWorkObservationAuthorizer for ObservationHost {
    async fn authorize_observation(
        &self,
        request: &SetupWorkAdmission,
    ) -> Result<(), SetupWorkAdmissionError> {
        self.exact.checks.fetch_add(1, Ordering::SeqCst);
        if !*self.exact.allowed.read().unwrap()
            || request.correlation_digest() != &self.exact.approved_correlation
            || serde_json::to_value(request.work_request()).unwrap() != self.exact.approved_payload
        {
            Err(SetupWorkAdmissionError::Denied)
        } else {
            Ok(())
        }
    }

    fn with_current_disclosure(
        &self,
        request: &SetupWorkAdmission,
        admission: &meerkat_workgraph::WorkAdmission,
        disclose: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError> {
        let current = self.exact.allowed.read().unwrap();
        if !*current
            || request.correlation_digest() != &self.exact.approved_correlation
            || serde_json::to_value(request.work_request()).unwrap() != self.exact.approved_payload
            || admission.key != *request.admission_key()
        {
            return Err(SetupWorkAdmissionError::Denied);
        }
        disclose();
        Ok(())
    }
}

#[tokio::test]
async fn exact_observation_can_recover_after_dispatch_revoke_without_creating_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workgraph.sqlite3");
    let store = Arc::new(SqliteWorkGraphStore::open(&path).unwrap());
    let work = service(store.clone());
    let request = admission();
    let dispatch = TestHost::approves(&request);
    let observe = ObservationHost {
        exact: TestHost::approves(&request),
    };
    assert!(
        observe_setup_work(&work, &request, &observe)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .list_items(WorkItemFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
    let created = admit_setup_work_protocol_fixture(&work, &request, &dispatch)
        .await
        .unwrap()
        .into_item();
    *dispatch.allowed.write().unwrap() = false;
    let reopened = service(Arc::new(SqliteWorkGraphStore::open(&path).unwrap()));
    let observed = observe_setup_work(&reopened, &request, &observe)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.work_item_id(), &created.id);
    assert_eq!(observed.scope(), request.scope());
    assert_eq!(observed.admission_key(), request.admission_key());
    assert_eq!(observed.correlation_digest(), request.correlation_digest());
    assert!(matches!(
        admit_setup_work_protocol_fixture(&reopened, &request, &dispatch).await,
        Err(SetupWorkAdmissionError::Denied)
    ));
    *observe.exact.allowed.write().unwrap() = false;
    assert!(matches!(
        observe_setup_work(&reopened, &request, &observe).await,
        Err(SetupWorkAdmissionError::Denied)
    ));
    assert_single_creation(store.as_ref()).await;
}

#[tokio::test]
async fn metadata_observation_excludes_worker_content_and_is_stable_across_work_updates() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workgraph.sqlite3");
    let store = Arc::new(SqliteWorkGraphStore::open(&path).unwrap());
    let work = service(store.clone());
    let request = admission();
    let dispatch = TestHost::approves(&request);
    let observer = ObservationHost {
        exact: TestHost::approves(&request),
    };
    let created = admit_setup_work_protocol_fixture(&work, &request, &dispatch)
        .await
        .unwrap()
        .into_item();
    let before = observe_setup_work(&work, &request, &observer)
        .await
        .unwrap()
        .unwrap();
    let changed = work
        .update(UpdateWorkItemRequest {
            id: created.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: created.revision,
            title: Some("Restricted worker title".into()),
            description: Some("Restricted later worker result".into()),
            priority: None,
            completion_policy: None,
            labels: None,
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: vec![ExternalWorkRef {
                kind: "restricted-worker-reference".into(),
                id: "restricted-content".into(),
                url: Some("https://example.invalid/restricted".into()),
            }],
        })
        .await
        .unwrap();
    assert!(changed.revision > created.revision);
    *dispatch.allowed.write().unwrap() = false;
    let events = store
        .list_events(WorkGraphEventFilter::default())
        .await
        .unwrap();
    let reopened = service(Arc::new(SqliteWorkGraphStore::open(&path).unwrap()));
    let observed = observe_setup_work(&reopened, &request, &observer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed, before);
    let encoded = serde_json::to_value(&observed).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "scope": request.scope(),
            "work_item_id": created.id,
            "admission_key": request.admission_key(),
            "correlation_digest": request.correlation_digest(),
        })
    );
    assert!(!format!("{observed:?}").contains("Restricted"));
    assert_eq!(
        store
            .list_events(WorkGraphEventFilter::default())
            .await
            .unwrap(),
        events
    );
    assert!(matches!(
        admit_setup_work_protocol_fixture(&reopened, &request, &dispatch).await,
        Err(SetupWorkAdmissionError::Denied)
    ));
    *observer.exact.allowed.write().unwrap() = false;
    assert!(matches!(
        observe_setup_work(&reopened, &request, &observer).await,
        Err(SetupWorkAdmissionError::Denied)
    ));
}

#[tokio::test]
async fn observation_refuses_changed_original_payload_and_wrong_scope() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let work = service(store.clone());
    let request = admission();
    let dispatch = TestHost::approves(&request);
    admit_setup_work_protocol_fixture(&work, &request, &dispatch)
        .await
        .unwrap();
    let changed = SetupWorkAdmission::from_host_protocol_fixture(
        scope(),
        correlation(),
        CreateWorkItemRequest {
            title: "Changed after original admission".into(),
            ..work_request()
        },
    )
    .unwrap();
    let observer = ObservationHost {
        exact: TestHost::approves(&changed),
    };
    assert!(matches!(
        observe_setup_work(&work, &changed, &observer).await,
        Err(SetupWorkAdmissionError::WorkGraph(
            WorkGraphError::Conflict(_)
        ))
    ));
    let wrong = WorkGraphService::with_scope(
        store.clone(),
        "other-tenant",
        WorkNamespace::new("setup-a").unwrap(),
    );
    let observer = ObservationHost {
        exact: TestHost::approves(&request),
    };
    assert!(matches!(
        observe_setup_work(&wrong, &request, &observer).await,
        Err(SetupWorkAdmissionError::ScopeMismatch)
    ));
    assert_eq!(observer.exact.checks.load(Ordering::SeqCst), 0);
    assert_single_creation(store.as_ref()).await;
}

#[tokio::test]
async fn production_work_adapter_cannot_create_an_unscoped_admission() {
    assert!(matches!(
        SetupWorkAdmission::from_host(scope(), correlation(), work_request()),
        Err(SetupWorkAdmissionError::NativeExecutionUnavailable)
    ));
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = service(store.clone());
    struct Never;
    #[async_trait]
    impl SetupWorkAuthorizer for Never {
        async fn admission_fence(
            &self,
            _: &SetupWorkAdmission,
            _: &meerkat_workgraph::WorkAdmission,
        ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError>
        {
            panic!("production refusal must precede owner calls")
        }
    }
    let fixture = admission();
    assert!(matches!(
        crate::work_admission::admit_setup_work(&service, &fixture, &Never).await,
        Err(SetupWorkAdmissionError::NativeExecutionUnavailable)
    ));
    assert!(
        store
            .list_items(WorkItemFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn prepared_workgraph_fence_rechecks_revocation_after_async_preparation() {
    struct RevokeAfterPreparation {
        current: Arc<std::sync::RwLock<bool>>,
    }
    #[async_trait]
    impl SetupWorkAuthorizer for RevokeAfterPreparation {
        async fn admission_fence(
            &self,
            request: &SetupWorkAdmission,
            exact: &meerkat_workgraph::WorkAdmission,
        ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError>
        {
            let fence = super::test_fence::TestAdmissionFence {
                allowed: self.current.clone(),
                scope: request.scope().clone(),
                admission: exact.clone(),
            };
            // Represents current cancellation/revocation between preparation
            // and the later blocking owner task; cached allow must not survive.
            *self.current.write().unwrap() = false;
            Ok(Arc::new(fence))
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conditional-work.sqlite3");
    let store = Arc::new(SqliteWorkGraphStore::open(&path).unwrap());
    let target = service(store.clone());
    let host = RevokeAfterPreparation {
        current: Arc::new(std::sync::RwLock::new(true)),
    };
    assert!(matches!(
        admit_setup_work_protocol_fixture(&target, &admission(), &host).await,
        Err(SetupWorkAdmissionError::WorkGraph(
            WorkGraphError::Conflict(_)
        ))
    ));
    assert!(
        store
            .list_items(WorkItemFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
    let reopened = service(Arc::new(SqliteWorkGraphStore::open(&path).unwrap()));
    assert!(
        reopened
            .observe_admission(
                admission().admission_key().clone(),
                admission().work_request().clone()
            )
            .await
            .unwrap()
            .is_none()
    );
}


/// Pause after preliminary screening. Another task changes the independent
/// observer's actual fixture authority before the private read/disclosure.
struct PausedScreening {
    exact: ObservationHost,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
    disclosures: AtomicUsize,
}
#[async_trait]
impl SetupWorkObservationAuthorizer for PausedScreening {
    async fn authorize_observation(
        &self,
        request: &SetupWorkAdmission,
    ) -> Result<(), SetupWorkAdmissionError> {
        self.exact.authorize_observation(request).await?;
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
        self.disclosures.fetch_add(1, Ordering::SeqCst);
        self.exact.with_current_disclosure(request, admission, disclose)
    }
}

#[tokio::test]
async fn revoked_observation_withholds_identity_absence_conflict_and_store_error() {
    for case in ["identity", "absence", "conflict", "store-error"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workgraph.sqlite3");
        let store = Arc::new(SqliteWorkGraphStore::open(&path).unwrap());
        let work = service(store);
        let original = admission();
        if case != "absence" {
            admit_setup_work_protocol_fixture(
                &work,
                &original,
                &TestHost::approves(&original),
            )
            .await
            .unwrap();
        }
        let request = if case == "conflict" {
            SetupWorkAdmission::from_host_protocol_fixture(
                scope(),
                correlation(),
                CreateWorkItemRequest {
                    title: "different request under the original key".into(),
                    ..work_request()
                },
            )
            .unwrap()
        } else {
            original
        };
        let observer = PausedScreening {
            exact: ObservationHost { exact: TestHost::approves(&request) },
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
            disclosures: AtomicUsize::new(0),
        };
        let revoke = async {
            observer.entered.notified().await;
            *observer.exact.exact.allowed.write().unwrap() = false;
            if case == "store-error" {
                // The native read must refuse this missing backing store. Its
                // error still cannot bypass current observation authorization.
                std::fs::remove_file(&path).unwrap();
            }
            observer.resume.notify_one();
        };
        let (result, ()) = tokio::join!(observe_setup_work(&work, &request, &observer), revoke);
        assert!(matches!(result, Err(SetupWorkAdmissionError::Denied)), "{case}");
        assert_eq!(observer.disclosures.load(Ordering::SeqCst), 1, "{case}");
    }
}

struct SkippedDisclosure;
#[async_trait]
impl SetupWorkObservationAuthorizer for SkippedDisclosure {
    async fn authorize_observation(
        &self,
        _: &SetupWorkAdmission,
    ) -> Result<(), SetupWorkAdmissionError> {
        Ok(())
    }
    fn with_current_disclosure(
        &self,
        _: &SetupWorkAdmission,
        _: &meerkat_workgraph::WorkAdmission,
        _: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError> {
        // Deliberately broken test owner. Default success is not disclosure.
        Ok(())
    }
}

#[tokio::test]
async fn observation_owner_cannot_skip_the_guarded_disclosure_callback() {
    let work = service(Arc::new(MemoryWorkGraphStore::new()));
    let request = admission();
    assert!(matches!(
        observe_setup_work(&work, &request, &SkippedDisclosure).await,
        Err(SetupWorkAdmissionError::OwnerTaskFailed)
    ));
}


mod held_sqlite_observation {
    use super::*;
    use chrono::{DateTime, Utc};
    use meerkat_workgraph::{
        AttentionListRequest, WorkAdmissionKey, WorkAttentionBinding, WorkEdge, WorkGraphEvent,
        WorkGraphStoreKind, WorkItem, WorkItemAdmissionState, WorkItemId,
    };
    use std::{sync::Mutex, time::Duration};
    use tokio::sync::oneshot;

    const WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, PartialEq, Eq)]
    enum ReadClass {
        Present,
        Absent,
        Error,
    }

    // This test adapter holds the real SQLite result inside the actual native
    // store-read method. It does not manufacture admission results or errors.
    struct HeldSqliteRead {
        inner: Arc<SqliteWorkGraphStore>,
        entered: Mutex<Option<oneshot::Sender<ReadClass>>>,
        release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl WorkGraphStore for HeldSqliteRead {
        fn kind(&self) -> WorkGraphStoreKind {
            WorkGraphStoreKind::Custom
        }
        async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
            self.inner.get_store_time_utc().await
        }
        async fn insert_item(
            &self,
            item: WorkItem,
            event: WorkGraphEvent,
        ) -> Result<WorkItem, WorkGraphError> {
            self.inner.insert_item(item, event).await
        }
        async fn update_item_cas(
            &self,
            item: WorkItem,
            previous: u64,
            event: WorkGraphEvent,
        ) -> Result<WorkItem, WorkGraphError> {
            self.inner.update_item_cas(item, previous, event).await
        }
        async fn update_item_and_attention_cas(
            &self,
            item: WorkItem,
            previous: u64,
            event: WorkGraphEvent,
            attention: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
        ) -> Result<WorkItem, WorkGraphError> {
            self.inner
                .update_item_and_attention_cas(item, previous, event, attention)
                .await
        }
        async fn get_item(
            &self,
            realm: &str,
            namespace: &WorkNamespace,
            id: &WorkItemId,
        ) -> Result<Option<WorkItem>, WorkGraphError> {
            self.inner.get_item(realm, namespace, id).await
        }
        async fn list_items(
            &self,
            filter: WorkItemFilter,
        ) -> Result<Vec<WorkItem>, WorkGraphError> {
            self.inner.list_items(filter).await
        }
        async fn list_attention_matching_bounded(
            &self,
            filter: AttentionListRequest,
            observed_at: DateTime<Utc>,
            limit: usize,
        ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
            self.inner
                .list_attention_matching_bounded(filter, observed_at, limit)
                .await
        }
        async fn insert_edge(
            &self,
            edge: WorkEdge,
            event: WorkGraphEvent,
        ) -> Result<WorkEdge, WorkGraphError> {
            self.inner.insert_edge(edge, event).await
        }
        async fn list_edges(
            &self,
            realm: &str,
            namespace: &WorkNamespace,
        ) -> Result<Vec<WorkEdge>, WorkGraphError> {
            self.inner.list_edges(realm, namespace).await
        }
        async fn list_events(
            &self,
            filter: WorkGraphEventFilter,
        ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
            self.inner.list_events(filter).await
        }
        async fn observe_item_admission(
            &self,
            realm: &str,
            namespace: &WorkNamespace,
            key: &WorkAdmissionKey,
        ) -> Result<Option<(WorkItem, WorkItemAdmissionState)>, WorkGraphError> {
            let release = self.release.lock().await.take().ok_or_else(|| {
                WorkGraphError::Store("held-read fixture was called more than once".into())
            })?;
            let result = self
                .inner
                .observe_item_admission(realm, namespace, key)
                .await;
            let class = match &result {
                Ok(Some(_)) => ReadClass::Present,
                Ok(None) => ReadClass::Absent,
                Err(_) => ReadClass::Error,
            };
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(class);
            }
            // A dropped test-side sender also releases this wait. The timeout
            // prevents a failed controller from leaving the blocking owner
            // task waiting on a test barrier indefinitely.
            match tokio::time::timeout(WAIT, release).await {
                Ok(Ok(())) => result,
                _ => Err(WorkGraphError::Store(
                    "held-read fixture was not released".into(),
                )),
            }
        }
    }

    #[tokio::test]
    async fn revocation_during_sqlite_read_withholds_every_stored_outcome() {
        for case in ["identity", "absence", "conflict", "store-error"] {
            for revoke in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("workgraph.sqlite3");
                let store = Arc::new(SqliteWorkGraphStore::open(&path).unwrap());
                let original = admission();
                let created = if case == "absence" {
                    None
                } else {
                    Some(
                        admit_setup_work_protocol_fixture(
                            &service(store.clone()),
                            &original,
                            &TestHost::approves(&original),
                        )
                        .await
                        .unwrap()
                        .into_item()
                        .id,
                    )
                };
                let request = if case == "conflict" {
                    SetupWorkAdmission::from_host_protocol_fixture(
                        scope(),
                        correlation(),
                        CreateWorkItemRequest {
                            title: "different request under the original key".into(),
                            ..work_request()
                        },
                    )
                    .unwrap()
                } else {
                    original
                };
                if case == "store-error" {
                    // The actual SQLite observation refuses a missing backing
                    // file. The adapter holds that real error, not a substitute.
                    std::fs::remove_file(&path).unwrap();
                }
                let observer = Arc::new(ObservationHost {
                    exact: TestHost::approves(&request),
                });
                let (entered_tx, entered_rx) = oneshot::channel();
                let (release_tx, release_rx) = oneshot::channel();
                let work = service(Arc::new(HeldSqliteRead {
                    inner: store,
                    entered: Mutex::new(Some(entered_tx)),
                    release: tokio::sync::Mutex::new(Some(release_rx)),
                }));
                let task_observer = observer.clone();
                let mut task = tokio::spawn(async move {
                    observe_setup_work(&work, &request, task_observer.as_ref()).await
                });
                let entered = tokio::time::timeout(WAIT, entered_rx).await;
                let screening_checks = observer.exact.checks.load(Ordering::SeqCst);
                let revoked = if matches!(&entered, Ok(Ok(_))) && revoke {
                    // A faulty pre-read guard must fail this control rather
                    // than deadlock the test runtime while it holds the read.
                    match observer.exact.allowed.try_write() {
                        Ok(mut allowed) => {
                            *allowed = false;
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    false
                };
                // Release and join before asserting, including a failed entry
                // wait. No assertion can strand the read at our barrier.
                let released = release_tx.send(());
                let joined = tokio::time::timeout(WAIT, &mut task).await;
                let result = match joined {
                    Ok(result) => result,
                    Err(_) => {
                        task.abort();
                        let _ = tokio::time::timeout(WAIT, &mut task).await;
                        panic!("observation did not join after release: {case}, revoke={revoke}");
                    }
                };
                let class = entered
                    .expect("actual store read must reach the barrier")
                    .expect("actual store read must report its result class");
                assert!(released.is_ok(), "{case}, revoke={revoke}");
                assert_eq!(
                    revoked, revoke,
                    "{case}: revocation must occur during the held read"
                );
                assert_eq!(screening_checks, 1, "{case}, revoke={revoke}");
                assert_eq!(
                    class,
                    match case {
                        "absence" => ReadClass::Absent,
                        "store-error" => ReadClass::Error,
                        _ => ReadClass::Present,
                    },
                    "{case}, revoke={revoke}",
                );
                let result = result.expect("observation owner task must not panic");
                if revoke {
                    assert!(
                        matches!(result, Err(SetupWorkAdmissionError::Denied)),
                        "{case}: stale identity, None or error must not be disclosed",
                    );
                } else {
                    match case {
                        "identity" => assert_eq!(
                            result.unwrap().unwrap().work_item_id(),
                            created.as_ref().unwrap(),
                        ),
                        "absence" => assert!(result.unwrap().is_none()),
                        "conflict" => match result {
                            Err(SetupWorkAdmissionError::WorkGraph(WorkGraphError::Conflict(
                                detail,
                            ))) => {
                                assert!(detail.contains(created.as_ref().unwrap().as_str()));
                            }
                            _ => {
                                panic!("allowed observation must expose the actual native conflict")
                            }
                        },
                        "store-error" => assert!(matches!(
                            result,
                            Err(SetupWorkAdmissionError::WorkGraph(
                                WorkGraphError::BackingStoreUnavailable { .. }
                            )),
                        )),
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}

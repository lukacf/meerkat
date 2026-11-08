//! Actual Find-owned durable transfer, separate from native G input delivery.
use super::*;
use meerkat_workgraph::{
    UpdateWorkItemRequest, WorkAdmission, WorkGraphService, WorkItem, WorkItemAdmissionState,
};
use toolkit_mobkit::private_originals::{
    InstalledWorkflowContextTransfer, PrivateOriginalError, PrivateOriginalRef,
    WorkflowContextObservation,
};

pub(super) type Transfer = (
    PrivateOriginalRef,
    Arc<InstalledWorkflowContextTransfer>,
    WorkGraphService,
);

async fn item(planning: &PlanningCase, transfer: &Transfer) -> (WorkItem, WorkItemAdmissionState) {
    let results = planning.script.results.lock().unwrap().clone();
    let call = if transfer.0.route.as_str() == "original-a" {
        "a-planning-find"
    } else {
        "b-planning-find"
    };
    let result = &results[call];
    assert_eq!(result["worker"]["context_transfer"]["state"], "committed");
    let expected: WorkAdmission =
        serde_json::from_value(result["worker"]["context_transfer"]["admission"].clone()).unwrap();
    // Evidence only, read through the actual host's private recipient service.
    let (item, actual) = transfer
        .2
        .store()
        .observe_item_admission(
            transfer.2.default_realm_id(),
            transfer.2.default_namespace(),
            &expected.key,
        )
        .await
        .unwrap()
        .unwrap();
    let expected_state: WorkItemAdmissionState = serde_json::from_value(json!({
        "lifecycle_phase": "admitted",
        "admission_key": expected.key,
        "request_digest": expected.request_digest,
    }))
    .unwrap();
    assert_eq!(actual, expected_state);
    let envelope: Value = serde_json::from_str(item.description.as_deref().unwrap()).unwrap();
    assert_eq!(envelope["purpose"], "toolkit-workflow-inline-context-v1");
    assert_eq!(envelope["recipient"]["original"], json!(transfer.0));
    assert_eq!(
        envelope["recipient"]["planning_source"],
        json!(planning.lead.original)
    );
    let a = planning
        .source
        .binding_evidence(&transfer.0)
        .await
        .unwrap()
        .unwrap();
    let g = planning
        .source
        .binding_evidence(&planning.lead.original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.candidate().target.audience, g.candidate().target.audience);
    assert_eq!(
        envelope["recipient"]["native_audience"],
        json!(g.candidate().target.audience)
    );
    assert_eq!(envelope["recipient"]["output_audiences"], json!(["human"]));
    let parsed: meerkat_toolkit::request::FindAndInstallRequest =
        serde_json::from_value(planning.script.calls.lock().unwrap()[call].clone()).unwrap();
    assert_eq!(envelope["request"], json!(parsed));
    (item, actual)
}

async fn committed(transfer: &Transfer) {
    assert!(matches!(
        transfer.1.observe().await.unwrap(),
        WorkflowContextObservation::Committed(_)
    ));
}

/// The actual host publishes this in-process runtime's one-use bootstrap
/// completion. That starts the selected acquisition recovery owner whose
/// control commits each Find-owned transfer; construction alone selects none.
pub(super) fn complete_bootstrap(case: &Case) {
    case.host
        .as_ref()
        .unwrap()
        .publish_runtime_bootstrap_completed(case.native.as_ref().unwrap())
        .unwrap();
}

/// Actual public host construction and two actual Find-owned commits. The
/// caller retains Case and must join its actual SDK/native owners on every exit,
/// with `Case::cleanup_completed_runtime` once the runtime completed.
pub(super) async fn prepare(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
) -> (
    PlanningCase,
    Vec<Transfer>,
    PrivateOriginalRef,
    PrivateOriginalRef,
) {
    let planning = construction::construct(case, pipes).await;
    complete_bootstrap(case);
    let launch = case.launch.as_ref().unwrap().clone();
    planning
        .script
        .allow_context_delivery
        .store(true, std::sync::atomic::Ordering::Release);
    launch.enable_context_transfers(planning.source.clone());
    let (a, b) = planning.run_open(case).await;
    let transfers = launch.context_transfers();
    assert_eq!(transfers.len(), 2);
    for transfer in &transfers {
        committed(transfer).await;
    }
    (planning, transfers, a, b)
}

async fn exercise(
    case: &mut Case,
    pipes: toolkit_mobkit::private_host::InheritedHostPipes,
    mutate: bool,
) {
    let (planning, transfers, a, b) = prepare(case, pipes).await;
    let ta = transfers.iter().find(|row| row.0 == a).unwrap();
    let tb = transfers.iter().find(|row| row.0 == b).unwrap();
    let initial_a = item(&planning, ta).await;
    let initial_b = item(&planning, tb).await;
    committed(ta).await;
    committed(tb).await;
    committed(ta).await;
    assert_eq!(item(&planning, ta).await, initial_a);
    assert_eq!(item(&planning, tb).await, initial_b);
    assert_eq!(input_count(case, "planning-lead").await, 3);
    let g_before = history(case, "planning-lead").await;
    if mutate {
        // A mutable canonical item must be rejoined to its immutable admission.
        let update: UpdateWorkItemRequest = serde_json::from_value(json!({
            "id": initial_a.0.id, "realm_id": initial_a.0.realm_id, "namespace":initial_a.0.namespace,
            "expected_revision": initial_a.0.revision, "description":"{\"purpose\":\"foreign\"}"
        })).unwrap();
        ta.2.update(update).await.unwrap();
        assert!(matches!(
            ta.1.observe().await,
            Err(PrivateOriginalError::Conflict)
        ));
        committed(tb).await;
        assert_eq!(item(&planning, tb).await, initial_b);
    } else {
        let cancelled = case
            .client
            .lock()
            .await
            .call(json!({"kind":"cancel_original_operation", "original":a}))
            .await
            .unwrap();
        assert_eq!(cancelled["kind"], "original_operation_cancelled");
        let (workflow, effects, _) = workflow_evidence(case);
        let key_a = planning.source.source_record_key(&a).await.unwrap();
        let key_b = planning.source.source_record_key(&b).await.unwrap();
        let waiters = workflow["waiters"].as_object().unwrap();
        assert_eq!(
            waiters
                .values()
                .find(|row| row["identity"]["source"] == json!(key_a))
                .unwrap()["departed"],
            true
        );
        assert_eq!(
            waiters
                .values()
                .find(|row| row["identity"]["source"] == json!(key_b))
                .unwrap()["departed"],
            false
        );
        // Completed A disclosure survives A cancellation while B keeps G open.
        committed(ta).await;
        committed(tb).await;
        for effect in effects
            .iter()
            .filter(|row| row.intent.kind == "setup_workflow_delivery")
        {
            assert_eq!(effect.status, toolkit_store::EffectStatus::Succeeded);
            // A refused attempt is reconciled NotApplied and retried with the
            // same input (see await_automatic_context_deliveries).
            assert!(effect.attempt >= 1);
        }
        assert_eq!(item(&planning, ta).await, initial_a);
        assert_eq!(item(&planning, tb).await, initial_b);
        let cancelled = case
            .client
            .lock()
            .await
            .call(json!({"kind":"cancel_original_operation", "original":b}))
            .await
            .unwrap();
        assert_eq!(cancelled["kind"], "original_operation_cancelled");
        assert!(matches!(
            ta.1.observe().await,
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            tb.1.observe().await,
            Err(PrivateOriginalError::Denied)
        ));
    }
    assert_eq!(input_count(case, "planning-lead").await, 3);
    assert_eq!(history(case, "planning-lead").await, g_before);
    planning.cancel_parent(case).await;
    assert!(matches!(
        tb.1.observe().await,
        Err(PrivateOriginalError::Denied)
    ));
}

fn run(mutate: bool) {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let checked = std::panic::AssertUnwindSafe(exercise(&mut case, pipes, mutate))
            .catch_unwind()
            .await;
        let cleanup = case.cleanup_completed_runtime().await;
        assert!(
            cleanup.is_empty(),
            "actual SDK and native owners must be joined: {cleanup:?}"
        );
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    });
}

#[test]
fn sdk_completed_context_survives_a_departure_and_refuses_closed_recipient() {
    run(false);
}
#[test]
fn sdk_completed_context_validates_mutable_payload_without_replaying_find() {
    run(true);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
async fn deadline(deadline: u64) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while now_ms() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
struct SourceWriteHold {
    release: Option<std::sync::mpsc::Sender<()>>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl SourceWriteHold {
    fn enter(case: &Case) -> Self {
        let realm = meerkat_core::mob_realm_id(MOB).unwrap().to_string();
        let path = case
            .directory
            .path()
            .join("shared/data/toolkit-native")
            .join(digest(realm.as_bytes()))
            .join("originals.sqlite3")
            .canonicalize()
            .unwrap();
        let (ready, wait) = std::sync::mpsc::channel();
        let (release, closed) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            let db = rusqlite::Connection::open(path).unwrap();
            db.busy_timeout(BOUND).unwrap();
            db.execute_batch("BEGIN IMMEDIATE").unwrap();
            ready.send(()).unwrap();
            let _ = closed.recv();
            db.execute_batch("ROLLBACK").unwrap();
        });
        wait.recv().unwrap();
        Self {
            release: Some(release),
            task: Some(task),
        }
    }
    fn join(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
impl Drop for SourceWriteHold {
    fn drop(&mut self) {
        self.join();
    }
}

#[test]
fn sdk_completed_context_outlives_donor_deadline_but_not_last_live_interest() {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let mut write_hold = None;
        let checked = std::panic::AssertUnwindSafe(async {
            let limits = toolkit_mobkit::installed_host::acquisition::InstalledSetupLimits {
                setup_timeout_ms: 20_000,
                ..Default::default()
            };
            let planning = construction::construct_with_limits(&mut case, pipes, limits).await;
            complete_bootstrap(&case);
            let launch = case.launch.as_ref().unwrap().clone();
            planning
                .script
                .allow_context_delivery
                .store(true, std::sync::atomic::Ordering::Release);
            launch.enable_context_transfers(planning.source.clone());
            let (a, b) = planning
                .run_open_with_stagger(&mut case, Some(Duration::from_secs(4)))
                .await;
            let transfers = launch.context_transfers();
            let ta = transfers.iter().find(|row| row.0 == a).unwrap();
            let tb = transfers.iter().find(|row| row.0 == b).unwrap();
            let first = item(&planning, ta).await;
            let second = item(&planning, tb).await;
            let a_envelope: Value =
                serde_json::from_str(first.0.description.as_deref().unwrap()).unwrap();
            let b_envelope: Value =
                serde_json::from_str(second.0.description.as_deref().unwrap()).unwrap();
            let a_deadline = a_envelope["deadline_ms"].as_u64().unwrap();
            let b_deadline = b_envelope["deadline_ms"].as_u64().unwrap();
            assert!(b_deadline >= a_deadline + 3_000 && b_deadline < planning.terms.deadline_ms);
            let cancelled = case
                .client
                .lock()
                .await
                .call(json!({"kind":"cancel_original_operation","original":a}))
                .await
                .unwrap();
            assert_eq!(cancelled["kind"], "original_operation_cancelled");
            // The external write barrier is an ordering control only. Native
            // source reads and recipient checks still use their actual owners.
            // It prevents canonical expiry/departure from making this test pass.
            write_hold = Some(SourceWriteHold::enter(&case));
            deadline(a_deadline).await;
            assert!(now_ms() < b_deadline);
            committed(ta).await;
            committed(tb).await;
            assert_eq!(item(&planning, ta).await, first);
            deadline(b_deadline).await;
            let (workflow, _, _) = workflow_evidence(&case);
            let key_b = planning.source.source_record_key(&b).await.unwrap();
            let waiters = workflow["waiters"].as_object().unwrap();
            assert_eq!(
                waiters
                    .values()
                    .find(|row| row["identity"]["source"] == json!(key_b))
                    .unwrap()["departed"],
                false
            );
            assert!(matches!(
                ta.1.observe().await,
                Err(PrivateOriginalError::Denied)
            ));
            assert!(matches!(
                tb.1.observe().await,
                Err(PrivateOriginalError::Denied)
            ));
            assert_eq!(input_count(&case, "planning-lead").await, 3);
            assert_eq!(item(&planning, tb).await, second);
        })
        .catch_unwind()
        .await;
        if let Some(mut hold) = write_hold.take() {
            hold.join();
        }
        let cleanup = case.cleanup_completed_runtime().await;
        assert!(
            cleanup.is_empty(),
            "actual SDK/native and SQLite owners must join: {cleanup:?}"
        );
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    });
}

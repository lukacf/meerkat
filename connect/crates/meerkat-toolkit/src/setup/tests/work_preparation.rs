//! Persisted outbound preparation against actual Toolkit and WorkGraph SQLite.
//! Host configuration, authority and reference custodians remain fixtures.
use super::*;
use crate::work_admission::{
    SetupWorkAdmission, SetupWorkAdmissionError, SetupWorkAuthorizer,
    admit_setup_work_protocol_fixture,
};
use meerkat_workgraph::{
    CreateWorkItemRequest, SqliteWorkGraphStore, WorkGraphService, WorkNamespace,
};
use std::sync::{Arc, atomic::AtomicUsize};
use toolkit_package::canonical_json;

struct Preparer {
    title: String,
    calls: AtomicUsize,
    barrier: Option<Arc<std::sync::Barrier>>,
    wrong_input: bool,
}
impl Preparer {
    fn new(title: &str) -> Self {
        Self {
            title: title.into(),
            calls: 0.into(),
            barrier: None,
            wrong_input: false,
        }
    }
}
impl SetupWorkPreparer for Preparer {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(barrier) = &self.barrier {
            barrier.wait();
        }
        let mut input = reference("canonical-original-request");
        input.content_digest = ContentDigest::of_bytes(&canonical_json(request.request()).unwrap());
        if self.wrong_input {
            input.content_digest = ContentDigest::of_bytes(b"truncated request");
        }
        Ok(SetupWorkPreparation {
            delegation: None,
            scope: crate::work_admission::HostWorkGraphScope::new(
                id("tenant-a"),
                WorkNamespace::new("setup").unwrap(),
            )?,
            input_reference: input,
            work_request: CreateWorkItemRequest {
                title: self.title.clone(),
                description: Some("Pinned setup profile.\n  Preserve this whitespace.".into()),
                labels: ["exact-host-cycle".into()].into(),
                ..Default::default()
            },
        })
    }
}
fn operation(snapshot: &SetupSnapshot) -> Id {
    id(&snapshot.key().id)
}

#[tokio::test]
async fn prepared_request_survives_reopen_and_changed_configuration() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let preparer = Preparer::new("Original environment configuration");
    let current = f.context(200);
    let saved = f
        .bridge()
        .prepare_work_protocol_fixture(&operation(&snapshot), f.access(&current), &preparer)
        .await
        .unwrap();
    assert_eq!(preparer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        saved.admission().correlation().input_digest,
        *f.request.input_digest()
    );
    assert_eq!(
        saved.admission().correlation().continuation,
        f.request.host().continuation
    );
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let changed = Preparer::new("Changed deployment after crash");
    let recovered = f
        .for_store(&reopened)
        .prepare_work_protocol_fixture(&operation(&snapshot), f.access(&current), &changed)
        .await
        .unwrap();
    assert_eq!(saved.digest(), recovered.digest());
    assert_eq!(
        saved.admission().admission_key(),
        recovered.admission().admission_key()
    );
    assert_eq!(
        canonical_json(saved.admission().work_request()).unwrap(),
        canonical_json(recovered.admission().work_request()).unwrap()
    );
    assert_eq!(changed.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.store.get(snapshot.key()).await.unwrap().unwrap().revision,
        snapshot.state().revision
    );
    assert_eq!(
        f.store
            .list_effects(snapshot.key(), None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.store
            .read_events(saved.key(), 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        f.store
            .list_effects(saved.key(), None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.store
            .list_effects(snapshot.key(), None, 10)
            .await
            .unwrap()[0]
            .status,
        EffectStatus::Pending
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_preparations_freeze_one_exact_payload_and_conflict_on_change() {
    let f = Arc::new(Fixture::new());
    let snapshot = f.admit().await;
    let op = operation(&snapshot);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut tasks = vec![];
    for title in ["Configuration A", "Configuration B"] {
        let f = f.clone();
        let op = op.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let mut preparer = Preparer::new(title);
            preparer.barrier = Some(barrier);
            let current = f.context(200);
            f.bridge()
                .prepare_work_protocol_fixture(&op, f.access(&current), &preparer)
                .await
        }));
    }
    let a = tasks.remove(0).await.unwrap();
    let b = tasks.remove(0).await.unwrap();
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (saved, error) = if let Ok(saved) = a {
        (saved, b.err().unwrap())
    } else {
        (b.unwrap(), a.err().unwrap())
    };
    assert!(matches!(
        error,
        SetupError::Store(StoreError::AdmissionConflict)
    ));
    let never = Preparer::new("Must not overwrite winner");
    let recovered = f
        .bridge()
        .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &never)
        .await
        .unwrap();
    assert_eq!(saved.digest(), recovered.digest());
    assert_eq!(never.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn preparation_rejects_revoked_context_expired_missing_work_and_wrong_input() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let op = operation(&snapshot);
    let preparer = Preparer::new("Configured setup team");
    assert!(
        f.bridge()
            .prepare_work_protocol_fixture(&op, f.access(&f.context(900)), &preparer)
            .await
            .is_err()
    );
    assert_eq!(preparer.calls.load(Ordering::SeqCst), 0);
    f.references.0.store(false, Ordering::SeqCst);
    assert!(
        f.bridge()
            .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &preparer)
            .await
            .is_err()
    );
    assert_eq!(preparer.calls.load(Ordering::SeqCst), 0);
    f.references.0.store(true, Ordering::SeqCst);
    let mut wrong = Preparer::new("Wrong reference");
    wrong.wrong_input = true;
    assert!(matches!(
        f.bridge()
            .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &wrong)
            .await,
        Err(SetupError::InvalidRecord)
    ));
    let good = f
        .bridge()
        .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &preparer)
        .await
        .unwrap();
    // Read-only recovery is useful after deadline; it cannot authorize dispatch.
    let recovered = f
        .bridge()
        .prepare_work_protocol_fixture(&op, f.access(&f.context(950)), &preparer)
        .await
        .unwrap();
    assert_eq!(recovered.digest(), good.digest());
    assert!(
        f.request
            .revalidate(&f.authority, &f.context(950), &f.references)
            .is_err()
    );
    f.references.0.store(false, Ordering::SeqCst);
    assert!(
        f.bridge()
            .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &preparer)
            .await
            .is_err()
    );
    assert_eq!(preparer.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_before_preparation_cannot_create_frozen_work() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let op = operation(&snapshot);
    f.bridge()
        .resolve(
            &snapshot,
            Resolution::Cancelled(reference("cancelled")),
            None,
            f.access(&f.context(200)),
        )
        .await
        .unwrap();
    let preparer = Preparer::new("Must never prepare");
    assert!(matches!(
        f.bridge()
            .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &preparer)
            .await,
        Err(SetupError::Rejected)
    ));
    assert_eq!(preparer.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn edited_preparation_and_changed_generated_effect_are_rejected() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let op = operation(&snapshot);
    let preparer = Preparer::new("Original");
    let saved = f
        .bridge()
        .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &preparer)
        .await
        .unwrap();
    let record = f.store.get(saved.key()).await.unwrap().unwrap();
    let mut changed = record.data.clone();
    changed.body["request"]["title"] = "Altered work".into();
    f.store
        .compare_and_swap(Transition {
            key: record.key,
            expected_revision: record.revision,
            expected_digest: Some(record.content_digest),
            claim: None,
            data: changed,
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    assert!(matches!(
        f.bridge()
            .prepare_work_protocol_fixture(&op, f.access(&f.context(200)), &preparer)
            .await,
        Err(SetupError::InvalidRecord)
    ));
    let fresh = Fixture::new();
    let snapshot = fresh.admit().await;
    let op = operation(&snapshot);
    let conn = rusqlite::Connection::open(fresh.directory.path().join("toolkit.sqlite")).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT data FROM toolkit_effects WHERE id='work-admission'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut effect: StoredEffect = serde_json::from_str(&raw).unwrap();
    effect.intent.request.body["request_digest"] = "wrong original".into();
    effect.request_digest = effect.intent.digest().unwrap();
    conn.execute(
        "UPDATE toolkit_effects SET data=?1 WHERE id='work-admission'",
        [serde_json::to_string(&effect).unwrap()],
    )
    .unwrap();
    let never = Preparer::new("Must not prepare from a forged effect");
    assert!(matches!(
        fresh
            .bridge()
            .prepare_work_protocol_fixture(&op, fresh.access(&fresh.context(200)), &never)
            .await,
        Err(SetupError::InvalidRecord)
    ));
    assert_eq!(never.calls.load(Ordering::SeqCst), 0);
}

struct ExactDispatch(ContentDigest);
#[async_trait]
impl SetupWorkAuthorizer for ExactDispatch {
    async fn admission_fence(
        &self,
        request: &SetupWorkAdmission,
        exact: &meerkat_workgraph::WorkAdmission,
    ) -> Result<Arc<dyn meerkat_workgraph::WorkGraphAdmissionFence>, SetupWorkAdmissionError> {
        if ContentDigest::of_bytes(&canonical_json(request.work_request()).unwrap()) == self.0 {
            Ok(Arc::new(
                crate::work_admission::test_fence::TestAdmissionFence {
                    allowed: Arc::new(std::sync::RwLock::new(true)),
                    scope: request.scope().clone(),
                    admission: exact.clone(),
                },
            ))
        } else {
            Err(SetupWorkAdmissionError::Denied)
        }
    }
}

#[tokio::test]
async fn frozen_outbound_payload_recovers_one_actual_workgraph_admission() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let op = operation(&snapshot);
    let saved = f
        .bridge()
        .prepare_work_protocol_fixture(
            &op,
            f.access(&f.context(200)),
            &Preparer::new("Configured setup work"),
        )
        .await
        .unwrap();
    let authority = ExactDispatch(ContentDigest::of_bytes(
        &canonical_json(saved.admission().work_request()).unwrap(),
    ));
    let work_path = f.directory.path().join("workgraph.sqlite");
    let make_service = || {
        WorkGraphService::with_scope(
            Arc::new(SqliteWorkGraphStore::open(&work_path).unwrap()),
            "tenant-a",
            WorkNamespace::new("setup").unwrap(),
        )
    };
    let first = admit_setup_work_protocol_fixture(&make_service(), saved.admission(), &authority)
        .await
        .unwrap()
        .into_item();
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let recovered = f
        .for_store(&reopened)
        .prepare_work_protocol_fixture(
            &op,
            f.access(&f.context(200)),
            &Preparer::new("Changed configuration"),
        )
        .await
        .unwrap();
    let replay =
        admit_setup_work_protocol_fixture(&make_service(), recovered.admission(), &authority)
            .await
            .unwrap();
    assert!(matches!(
        replay,
        crate::work_admission::WorkAdmissionResult::Existing(_)
    ));
    assert_eq!(replay.item().id, first.id);
    assert_eq!(
        f.store.get(snapshot.key()).await.unwrap().unwrap().revision,
        snapshot.state().revision
    );
    assert_eq!(
        f.store
            .list_effects(snapshot.key(), None, 10)
            .await
            .unwrap()[0]
            .status,
        EffectStatus::Pending
    );
}

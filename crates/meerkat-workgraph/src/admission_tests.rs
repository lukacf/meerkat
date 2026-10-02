//! Exact keyed work item admission (`WorkGraphService::create_idempotent`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use crate::machines::work_item_admission::{WorkAdmissionReplayKind, WorkItemAdmissionPhase};
use crate::types::{ClaimWorkItemRequest, WorkItemFilter, WorkOwner, WorkOwnerKey, WorkOwnerKind};
use crate::{
    CloseWorkItemRequest, CreateWorkItemRequest, ExternalWorkRef, MemoryWorkGraphStore,
    WorkAdmissionKey, WorkAdmissionOutcome, WorkGraphError, WorkGraphEventFilter, WorkGraphMachine,
    WorkGraphService, WorkGraphStore, WorkItem, WorkNamespace, WorkStatus,
};

fn request(title: &str) -> CreateWorkItemRequest {
    CreateWorkItemRequest {
        title: title.to_string(),
        ..Default::default()
    }
}

fn key(value: &str) -> WorkAdmissionKey {
    WorkAdmissionKey::new(value).expect("valid admission key")
}

fn created(outcome: WorkAdmissionOutcome) -> WorkItem {
    match outcome {
        WorkAdmissionOutcome::Created(item) => item,
        other => panic!("expected Created, got {other:?}"),
    }
}

fn replayed(outcome: WorkAdmissionOutcome) -> WorkItem {
    match outcome {
        WorkAdmissionOutcome::Replayed(item) => item,
        other => panic!("expected Replayed, got {other:?}"),
    }
}

async fn item_count(service: &WorkGraphService) -> usize {
    service
        .list(WorkItemFilter {
            include_terminal: true,
            ..Default::default()
        })
        .await
        .expect("list items")
        .len()
}

async fn event_count(store: &dyn WorkGraphStore) -> usize {
    store
        .list_events(WorkGraphEventFilter {
            all_namespaces: true,
            ..Default::default()
        })
        .await
        .expect("list events")
        .len()
}

#[test]
fn admission_key_must_be_canonical() {
    for bad in ["", " lead", "trail ", "line\nbreak", &"k".repeat(513)] {
        assert!(
            matches!(
                WorkAdmissionKey::new(bad),
                Err(WorkGraphError::InvalidInput(_))
            ),
            "accepted non-canonical key {bad:?}"
        );
    }
    assert_eq!(key("toolkit-setup:abc").as_str(), "toolkit-setup:abc");
    assert!(WorkAdmissionKey::new("k".repeat(512)).is_ok());
    let decoded: Result<WorkAdmissionKey, _> = serde_json::from_str("\" padded \"");
    assert!(
        decoded.is_err(),
        "deserialization must apply the same validation"
    );
}

#[tokio::test]
async fn new_key_creates_once_without_touching_the_lifecycle_state() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = WorkGraphService::new(store.clone());

    let item = created(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("admit"),
    );

    assert_eq!(item.status, WorkStatus::Open);
    let unkeyed = service
        .create(request("setup"))
        .await
        .expect("plain create");
    assert_eq!(
        item.machine_state, unkeyed.machine_state,
        "admission identity is not lifecycle state"
    );
    assert_eq!(item_count(&service).await, 2);
    assert_eq!(event_count(store.as_ref()).await, 2);
}

#[test]
fn created_route_binds_admitted_or_unkeyed_identity() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let (_, _, keyed) = WorkGraphMachine::create_item_with_admission(
        request("setup"),
        "default".into(),
        WorkNamespace::default(),
        chrono::Utc::now(),
        Some((&key("setup-1"), digest.as_str())),
    )
    .expect("keyed create");
    assert_eq!(keyed.lifecycle_phase, WorkItemAdmissionPhase::Admitted);
    assert_eq!(
        keyed.admission_key.as_ref().map(|k| k.0.as_str()),
        Some("setup-1")
    );
    assert_eq!(
        keyed.request_digest.as_ref().map(|d| d.0.as_str()),
        Some(digest.as_str())
    );

    let (_, _, unkeyed) = WorkGraphMachine::create_item_with_admission(
        request("setup"),
        "default".into(),
        WorkNamespace::default(),
        chrono::Utc::now(),
        None,
    )
    .expect("unkeyed create");
    assert_eq!(unkeyed.lifecycle_phase, WorkItemAdmissionPhase::Unkeyed);
    assert!(unkeyed.admission_key.is_none() && unkeyed.request_digest.is_none());
}

#[tokio::test]
async fn same_key_same_request_replays_without_writing() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = WorkGraphService::new(store.clone());
    let first = created(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("admit"),
    );

    let again = replayed(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("replay"),
    );

    assert_eq!(again, first);
    assert_eq!(item_count(&service).await, 1);
    assert_eq!(
        event_count(store.as_ref()).await,
        1,
        "a replay appends nothing"
    );
}

#[tokio::test]
async fn same_key_different_request_is_typed_conflict() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = WorkGraphService::new(store.clone());
    let first = created(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("admit"),
    );

    let mut changed_refs = request("setup");
    changed_refs.external_refs.push(ExternalWorkRef {
        kind: "toolkit".into(),
        id: "provenance".into(),
        url: None,
    });
    for changed in [request("different title"), changed_refs] {
        match service
            .create_idempotent(key("setup-1"), changed)
            .await
            .expect("classify")
        {
            WorkAdmissionOutcome::Conflict {
                admission_key,
                existing_item_id,
            } => {
                assert_eq!(admission_key.as_str(), "setup-1");
                assert_eq!(existing_item_id, first.id);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }
    let stored = service
        .get(None, None, first.id.clone())
        .await
        .expect("get item");
    assert_eq!(
        stored, first,
        "a conflict must not mutate the admitted item"
    );
    assert_eq!(item_count(&service).await, 1);
    assert_eq!(event_count(store.as_ref()).await, 1);
}

#[tokio::test]
async fn replay_returns_current_state_of_an_advanced_or_terminal_item() {
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let first = created(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("admit"),
    );
    let claimed = service
        .claim(ClaimWorkItemRequest {
            id: first.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: first.revision,
            owner: WorkOwner {
                key: WorkOwnerKey::new(WorkOwnerKind::Label, "worker").expect("owner"),
                display_name: None,
            },
            lease_seconds: None,
            lease_expires_at: None,
        })
        .await
        .expect("claim");
    let closed = service
        .close(CloseWorkItemRequest {
            id: claimed.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: claimed.revision,
            status: WorkStatus::Completed,
        })
        .await
        .expect("close");

    let again = replayed(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("replay terminal"),
    );
    assert_eq!(again.id, first.id);
    assert_eq!(again.status, WorkStatus::Completed);
    assert_eq!(again.revision, closed.revision);
}

#[tokio::test]
async fn default_and_explicit_scope_digest_equal_and_namespaces_are_independent() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let service = WorkGraphService::new(store.clone());
    let first = created(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("admit"),
    );
    let mut explicit = request("setup");
    explicit.realm_id = Some("default".into());
    explicit.namespace = Some(WorkNamespace::default());
    assert_eq!(
        replayed(
            service
                .create_idempotent(key("setup-1"), explicit)
                .await
                .expect("explicit replay")
        )
        .id,
        first.id
    );

    // The same key in another namespace (granted to another service over the
    // same store) is an independent admission.
    let other_service = WorkGraphService::with_scope(
        store,
        "default",
        WorkNamespace::new("other").expect("namespace"),
    );
    let other = created(
        other_service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("separate namespace admits"),
    );
    assert_ne!(other.id, first.id);
}

#[tokio::test]
async fn unkeyed_items_and_keyed_items_coexist() {
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let plain = service
        .create(request("setup"))
        .await
        .expect("plain create");
    let keyed = created(
        service
            .create_idempotent(key("setup-1"), request("setup"))
            .await
            .expect("keyed create"),
    );
    assert_ne!(plain.id, keyed.id);
    // An unkeyed item is never an admission replay or conflict.
    let (_, _, unkeyed) = WorkGraphMachine::create_item_with_admission(
        request("setup"),
        "default".into(),
        WorkNamespace::default(),
        chrono::Utc::now(),
        None,
    )
    .expect("unkeyed create");
    assert_eq!(
        WorkGraphMachine::classify_admission_replay(
            &plain.id,
            &unkeyed,
            &key("setup-1"),
            "sha256:digest",
        )
        .expect("classify"),
        WorkAdmissionReplayKind::KeyMismatch
    );
    // An admission that was never bound (Absent) is a key mismatch too: the
    // classify input is total in every phase, never a guard rejection.
    assert_eq!(
        WorkGraphMachine::classify_admission_replay(
            &plain.id,
            crate::machines::work_item_admission::WorkItemAdmissionMachineAuthority::new().state(),
            &key("setup-1"),
            "sha256:digest",
        )
        .expect("classify absent"),
        WorkAdmissionReplayKind::KeyMismatch
    );
}

#[tokio::test]
async fn concurrent_same_key_memory_admission_creates_exactly_once() {
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let service = service.clone();
        tasks.spawn(async move {
            service
                .create_idempotent(key("setup-1"), request("setup"))
                .await
        });
    }
    let mut created_ids = Vec::new();
    let mut replay_ids = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined.expect("join").expect("admit") {
            WorkAdmissionOutcome::Created(item) => created_ids.push(item.id),
            WorkAdmissionOutcome::Replayed(item) => replay_ids.push(item.id),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(created_ids.len(), 1);
    assert_eq!(replay_ids.len(), 15);
    assert!(replay_ids.iter().all(|id| id == &created_ids[0]));
    assert_eq!(item_count(&service).await, 1);
}

#[cfg(not(target_arch = "wasm32"))]
mod sqlite {
    use super::*;
    use crate::SqliteWorkGraphStore;

    fn open_service(path: &std::path::Path) -> (Arc<SqliteWorkGraphStore>, WorkGraphService) {
        let store = Arc::new(SqliteWorkGraphStore::open(path).expect("open sqlite store"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        (store, service)
    }

    #[tokio::test]
    async fn replay_and_conflict_survive_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let first = {
            let (_store, service) = open_service(&path);
            created(
                service
                    .create_idempotent(key("setup-1"), request("setup"))
                    .await
                    .expect("admit"),
            )
        };

        // A fresh process view of the same file.
        let (store, service) = open_service(&path);
        let again = replayed(
            service
                .create_idempotent(key("setup-1"), request("setup"))
                .await
                .expect("replay after restart"),
        );
        assert_eq!(again, first);
        match service
            .create_idempotent(key("setup-1"), request("changed"))
            .await
            .expect("classify after restart")
        {
            WorkAdmissionOutcome::Conflict {
                existing_item_id, ..
            } => assert_eq!(existing_item_id, first.id),
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(item_count(&service).await, 1);
        assert_eq!(event_count(store.as_ref()).await, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_key_admission_across_connections_creates_exactly_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        // Initialize the file once, then race independent store instances
        // (independent SQLite connections) on the same key.
        drop(open_service(&path));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let path = path.clone();
            tasks.spawn(async move {
                let (_store, service) = open_service(&path);
                service
                    .create_idempotent(key("setup-1"), request("setup"))
                    .await
            });
        }
        let mut created_ids = Vec::new();
        let mut replay_ids = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined.expect("join").expect("admit") {
                WorkAdmissionOutcome::Created(item) => created_ids.push(item.id),
                WorkAdmissionOutcome::Replayed(item) => replay_ids.push(item.id),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(created_ids.len(), 1, "exactly one admission creates");
        assert_eq!(replay_ids.len(), 7);
        assert!(replay_ids.iter().all(|id| id == &created_ids[0]));
        let (_store, service) = open_service(&path);
        assert_eq!(item_count(&service).await, 1);
    }

    fn row_count(path: &std::path::Path, table: &str) -> i64 {
        let conn =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("inspect");
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
    }

    /// A crash between writing the item and writing its admission identity
    /// (simulated by aborting either insert inside the store transaction)
    /// leaves neither: no item without its identity, no identity without its
    /// item, no event. After the fault clears, the same admission succeeds.
    #[tokio::test]
    async fn crash_between_item_and_identity_writes_leaves_neither() {
        for (table, trigger) in [
            ("workgraph_item_admissions", "abort_identity_insert"),
            ("workgraph_items", "abort_item_insert"),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("workgraph.sqlite3");
            drop(open_service(&path));
            rusqlite::Connection::open(&path)
                .expect("fault injector")
                .execute_batch(&format!(
                    "CREATE TRIGGER {trigger} BEFORE INSERT ON {table}
                     BEGIN SELECT RAISE(ABORT, 'injected crash'); END;"
                ))
                .expect("install fault");

            let (_store, service) = open_service(&path);
            let failed = service
                .create_idempotent(key("setup-1"), request("setup"))
                .await;
            assert!(
                failed.is_err(),
                "the injected fault on {table} must fail the admission"
            );
            assert_eq!(
                row_count(&path, "workgraph_items"),
                0,
                "no item without identity"
            );
            assert_eq!(
                row_count(&path, "workgraph_item_admissions"),
                0,
                "no identity without item"
            );
            assert_eq!(row_count(&path, "workgraph_events"), 0, "no Created event");

            rusqlite::Connection::open(&path)
                .expect("fault remover")
                .execute_batch(&format!("DROP TRIGGER {trigger};"))
                .expect("remove fault");
            created(
                service
                    .create_idempotent(key("setup-1"), request("setup"))
                    .await
                    .expect("admission after the fault clears"),
            );
            assert_eq!(row_count(&path, "workgraph_items"), 1);
            assert_eq!(row_count(&path, "workgraph_item_admissions"), 1);
        }
    }

    #[tokio::test]
    async fn released_v3_file_migrates_to_the_admission_index_on_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        {
            let mut conn = rusqlite::Connection::open(&path).expect("seed");
            let tx = conn.transaction().expect("transaction");
            crate::store::build_released_0_8_16_workgraph_schema_for_tests(&tx)
                .expect("released v3 schema");
            tx.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);",
            )
            .expect("ledger");
            tx.execute(
                "INSERT INTO meerkat_schema VALUES (?1, 3)",
                [crate::WORKGRAPH_DOMAIN.name],
            )
            .expect("version");
            tx.commit().expect("commit");
        }

        let (_store, service) = open_service(&path);
        let item = created(
            service
                .create_idempotent(key("setup-1"), request("setup"))
                .await
                .expect("admit on migrated file"),
        );
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("inspect");
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, crate::WORKGRAPH_DOMAIN.name).expect("ledger"),
            Some(crate::WORKGRAPH_DOMAIN.supported_version())
        );
        let indexed: String = conn
            .query_row(
                "SELECT item_id FROM workgraph_item_admissions WHERE admission_key = 'setup-1'",
                [],
                |row| row.get(0),
            )
            .expect("admission row");
        assert_eq!(indexed, item.id.as_str());
    }
}

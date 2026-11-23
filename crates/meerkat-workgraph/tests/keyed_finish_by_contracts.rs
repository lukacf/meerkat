#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_workgraph::{
    CreateWorkItemRequest, MemoryWorkGraphStore, WorkAdmissionKey, WorkAdmissionOutcome,
    WorkGraphEvent, WorkGraphEventKind, WorkGraphService, WorkGraphStore, WorkItem, WorkItemFilter,
};

const FINISH: &str = "2030-01-02T03:04:05Z";
const DIFFERENT_FINISH: &str = "2030-01-03T03:04:05Z";

#[derive(Debug, PartialEq)]
struct Snapshot {
    items: Vec<WorkItem>,
    events: Vec<WorkGraphEvent>,
}

async fn snapshot(store: &dyn WorkGraphStore) -> Snapshot {
    let mut items = store
        .list_items(WorkItemFilter {
            include_terminal: true,
            ..Default::default()
        })
        .await
        .expect("all stored items");
    items.sort_by_key(|item| {
        (
            item.realm_id.clone(),
            item.namespace.clone(),
            item.id.clone(),
        )
    });
    Snapshot {
        items,
        events: store
            .list_events(Default::default())
            .await
            .expect("all events"),
    }
}

fn request(finish: Option<&str>) -> CreateWorkItemRequest {
    let mut value = serde_json::json!({"title": "A keyed finish-by target"});
    if let Some(finish) = finish {
        value["finish_by"] = serde_json::json!(finish);
    }
    serde_json::from_value(value).expect("public create request")
}

type Admitted = (WorkAdmissionKey, CreateWorkItemRequest, WorkItem);

async fn seed(store: Arc<dyn WorkGraphStore>) -> Vec<Admitted> {
    let service = WorkGraphService::new(store.clone());
    let mut admitted = Vec::new();
    for (key, finish) in [("with-finish", Some(FINISH)), ("without-finish", None)] {
        let key = WorkAdmissionKey::new(key).expect("canonical key");
        let request = request(finish);
        let result = service
            .create_idempotent(key.clone(), request.clone())
            .await
            .expect("admit");
        let WorkAdmissionOutcome::Created(item) = result else {
            panic!("a fresh key must create exactly one item: {result:?}");
        };
        assert_eq!(item.finish_by, request.finish_by);
        let events = store
            .list_events(Default::default())
            .await
            .expect("created events");
        let event = events
            .iter()
            .find(|event| event.item_id.as_ref() == Some(&item.id))
            .expect("this item's Created event");
        assert_eq!(event.kind, WorkGraphEventKind::Created);
        let event_item: WorkItem =
            serde_json::from_value(event.payload["item"].clone()).expect("stored event item");
        assert_eq!(
            event_item, item,
            "admission and event carry the same finish-by data"
        );
        admitted.push((key, request, item));
    }
    assert_eq!(snapshot(store.as_ref()).await.events.len(), 2);
    admitted
}

async fn check_replay_and_conflicts(store: Arc<dyn WorkGraphStore>, admitted: &[Admitted]) {
    let service = WorkGraphService::new(store.clone());
    for (key, original, item) in admitted {
        let before = snapshot(store.as_ref()).await;
        let replay = service
            .create_idempotent(key.clone(), original.clone())
            .await
            .expect("replay");
        assert_eq!(replay, WorkAdmissionOutcome::Replayed(item.clone()));
        assert_eq!(
            snapshot(store.as_ref()).await,
            before,
            "equal request writes nothing"
        );

        // The request differs ONLY in finish_by. For an existing value these
        // are change and removal; for an absent value this is an addition.
        let alternatives = if original.finish_by.is_some() {
            vec![request(Some(DIFFERENT_FINISH)), request(None)]
        } else {
            vec![request(Some(FINISH))]
        };
        for different in alternatives {
            let outcome = service
                .create_idempotent(key.clone(), different)
                .await
                .expect("typed conflict");
            assert_eq!(
                outcome,
                WorkAdmissionOutcome::Conflict {
                    admission_key: key.clone(),
                    existing_item_id: item.id.clone(),
                }
            );
            assert_eq!(
                snapshot(store.as_ref()).await,
                before,
                "a changed, added or removed finish-by target changes neither item nor event log"
            );
        }
        assert_eq!(
            service
                .create_idempotent(key.clone(), original.clone())
                .await
                .expect("original still owns key"),
            WorkAdmissionOutcome::Replayed(item.clone())
        );
        assert_eq!(snapshot(store.as_ref()).await, before);
    }
}

#[tokio::test]
async fn memory_keyed_finish_by_replays_equal_and_refuses_changed_added_removed_without_writes() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let admitted = seed(store.clone()).await;
    check_replay_and_conflicts(store, &admitted).await;
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn sqlite_keyed_finish_by_identity_survives_rebuild_and_reopen() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("work.sqlite");
    let store = Arc::new(meerkat_workgraph::SqliteWorkGraphStore::open(&path).expect("open"));
    let admitted = seed(store.clone()).await;
    check_replay_and_conflicts(store.clone(), &admitted).await;
    let expected = snapshot(store.as_ref()).await;
    store
        .rebuild_projection_from_events()
        .expect("rebuild from actual admission events");
    assert_eq!(snapshot(store.as_ref()).await, expected);
    check_replay_and_conflicts(store.clone(), &admitted).await;
    drop(store);
    let reopened = Arc::new(meerkat_workgraph::SqliteWorkGraphStore::open(&path).expect("reopen"));
    assert_eq!(snapshot(reopened.as_ref()).await, expected);
    check_replay_and_conflicts(reopened, &admitted).await;
}

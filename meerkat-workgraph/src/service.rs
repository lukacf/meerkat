use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::json;

use crate::WorkGraphError;
use crate::machine::WorkGraphMachine;
use crate::store::{WorkGraphEventFilter, WorkGraphStore};
use crate::types::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest,
    LinkWorkItemsRequest, ReadyWorkFilter, ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkEdge,
    WorkEdgeKind, WorkGraphEvent, WorkGraphEventKind, WorkGraphSnapshot, WorkGraphSnapshotFilter,
    WorkItem, WorkItemFilter, WorkItemId, WorkNamespace,
};

#[derive(Clone)]
pub struct WorkGraphService {
    store: Arc<dyn WorkGraphStore>,
    default_realm_id: Arc<str>,
    default_namespace: WorkNamespace,
}

impl WorkGraphService {
    pub fn new(store: Arc<dyn WorkGraphStore>) -> Self {
        Self::with_scope(store, "default", WorkNamespace::default())
    }

    pub fn with_scope(
        store: Arc<dyn WorkGraphStore>,
        default_realm_id: impl Into<String>,
        default_namespace: WorkNamespace,
    ) -> Self {
        Self {
            store,
            default_realm_id: Arc::<str>::from(default_realm_id.into()),
            default_namespace,
        }
    }

    pub fn store(&self) -> &Arc<dyn WorkGraphStore> {
        &self.store
    }

    pub fn default_realm_id(&self) -> &str {
        &self.default_realm_id
    }

    pub fn default_namespace(&self) -> &WorkNamespace {
        &self.default_namespace
    }

    pub async fn create(&self, request: CreateWorkItemRequest) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let (realm_id, namespace) = self.scope(request.realm_id.clone(), request.namespace.clone());
        let (item, event) = WorkGraphMachine::create_item(request, realm_id, namespace, now)?;
        self.store.insert_item(item, event).await
    }

    pub async fn get(
        &self,
        realm_id: Option<String>,
        namespace: Option<WorkNamespace>,
        id: WorkItemId,
    ) -> Result<WorkItem, WorkGraphError> {
        let (realm_id, namespace) = self.scope(realm_id, namespace);
        self.store
            .get_item(&realm_id, &namespace, &id)
            .await?
            .ok_or_else(|| WorkGraphError::not_found(realm_id, namespace, id))
    }

    pub async fn list(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        self.store
            .list_items(self.normalize_item_filter(filter))
            .await
    }

    pub async fn ready(&self, filter: ReadyWorkFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let (realm_id, namespace) = self.scope(filter.realm_id.clone(), filter.namespace.clone());
        let all_items = self
            .store
            .list_items(WorkItemFilter {
                realm_id: Some(realm_id.clone()),
                namespace: Some(namespace.clone()),
                include_terminal: true,
                ..WorkItemFilter::default()
            })
            .await?;
        let labels = filter.labels.clone();
        let mut ready = WorkGraphMachine::ready_items(
            all_items
                .into_iter()
                .filter(|item| labels.iter().all(|label| item.labels.contains(label)))
                .collect(),
            now,
        );
        if let Some(limit) = filter.limit {
            ready.truncate(limit);
        }
        Ok(ready)
    }

    pub async fn snapshot(
        &self,
        filter: WorkGraphSnapshotFilter,
    ) -> Result<WorkGraphSnapshot, WorkGraphError> {
        let captured_at = self.store.get_store_time_utc().await?;
        let filter = self.normalize_snapshot_filter(filter);
        let realm_id = filter
            .realm_id
            .clone()
            .unwrap_or_else(|| self.default_realm_id.to_string());
        let items = self
            .store
            .list_items(WorkItemFilter {
                realm_id: Some(realm_id.clone()),
                namespace: filter.namespace.clone(),
                all_namespaces: filter.all_namespaces,
                statuses: filter.statuses.clone(),
                labels: filter.labels.clone(),
                include_terminal: filter.include_terminal,
                limit: filter.limit,
            })
            .await?;

        let namespaces = self.snapshot_namespaces(&realm_id, &filter, &items).await?;
        let mut edges = Vec::new();
        for namespace in &namespaces {
            edges.extend(self.store.list_edges(&realm_id, namespace).await?);
        }

        let ready_item_ids = self
            .ready_item_ids_in_namespaces(&realm_id, &namespaces, &filter.labels, captured_at)
            .await?;
        let event_high_water_mark = self
            .store
            .list_events(WorkGraphEventFilter {
                realm_id: Some(realm_id.clone()),
                namespace: if filter.all_namespaces {
                    None
                } else {
                    filter.namespace.clone()
                },
                all_namespaces: filter.all_namespaces,
                after_seq: None,
                limit: None,
            })
            .await?
            .into_iter()
            .filter_map(|event| event.seq)
            .max();

        Ok(WorkGraphSnapshot {
            realm_id,
            namespace: if filter.all_namespaces {
                None
            } else {
                filter.namespace
            },
            all_namespaces: filter.all_namespaces,
            captured_at,
            event_high_water_mark,
            items,
            edges,
            ready_item_ids,
        })
    }

    pub async fn claim(&self, request: ClaimWorkItemRequest) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let (realm_id, namespace) = self.scope(request.realm_id.clone(), request.namespace.clone());
        let item = self
            .store
            .get_item(&realm_id, &namespace, &request.id)
            .await?
            .ok_or_else(|| {
                WorkGraphError::not_found(realm_id.clone(), namespace.clone(), request.id.clone())
            })?;
        let expected_previous_revision = item.revision;
        let (item, event) = WorkGraphMachine::claim_ready_item(item, request, now)?;
        self.store
            .update_item_cas(item, expected_previous_revision, event)
            .await
    }

    pub async fn release(
        &self,
        request: ReleaseWorkItemRequest,
    ) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let item = self
            .get(
                request.realm_id.clone(),
                request.namespace.clone(),
                request.id.clone(),
            )
            .await?;
        let expected_previous_revision = item.revision;
        let (item, event) = WorkGraphMachine::release_item(item, request, now)?;
        self.store
            .update_item_cas(item, expected_previous_revision, event)
            .await
    }

    pub async fn update(&self, request: UpdateWorkItemRequest) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let item = self
            .get(
                request.realm_id.clone(),
                request.namespace.clone(),
                request.id.clone(),
            )
            .await?;
        let expected_previous_revision = item.revision;
        let (item, event) = WorkGraphMachine::update_item(item, request, now)?;
        self.store
            .update_item_cas(item, expected_previous_revision, event)
            .await
    }

    pub async fn block(
        &self,
        realm_id: Option<String>,
        namespace: Option<WorkNamespace>,
        id: WorkItemId,
        expected_revision: u64,
    ) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let item = self.get(realm_id, namespace, id).await?;
        let expected_previous_revision = item.revision;
        let (item, event) = WorkGraphMachine::block_item(item, expected_revision, now)?;
        self.store
            .update_item_cas(item, expected_previous_revision, event)
            .await
    }

    pub async fn close(&self, request: CloseWorkItemRequest) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let item = self
            .get(
                request.realm_id.clone(),
                request.namespace.clone(),
                request.id.clone(),
            )
            .await?;
        let expected_previous_revision = item.revision;
        let (item, event) = WorkGraphMachine::close_item(item, request, now)?;
        let closed = self
            .store
            .update_item_cas(item, expected_previous_revision, event)
            .await?;
        self.refresh_dependents_after_blocker_change(&closed, now)
            .await?;
        Ok(closed)
    }

    pub async fn link(&self, request: LinkWorkItemsRequest) -> Result<WorkEdge, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let (realm_id, namespace) = self.scope(request.realm_id.clone(), request.namespace.clone());
        let edge = WorkEdge {
            realm_id,
            namespace,
            kind: request.kind,
            from_id: request.from_id,
            to_id: request.to_id,
            created_at: now,
        };
        let existing_edges = self
            .store
            .list_edges(&edge.realm_id, &edge.namespace)
            .await?;
        let existing_items = self
            .store
            .list_items(WorkItemFilter {
                realm_id: Some(edge.realm_id.clone()),
                namespace: Some(edge.namespace.clone()),
                include_terminal: true,
                ..WorkItemFilter::default()
            })
            .await?;
        WorkGraphMachine::validate_link(&edge, &existing_items, &existing_edges)?;
        let event = WorkGraphEvent::graph(
            edge.realm_id.clone(),
            edge.namespace.clone(),
            WorkGraphEventKind::Linked,
            now,
            json!({ "edge": edge }),
        );
        let inserted = self.store.insert_edge(edge, event).await?;
        if inserted.kind == WorkEdgeKind::Blocks {
            self.refresh_item_eligibility(
                &inserted.realm_id,
                &inserted.namespace,
                &inserted.to_id,
                now,
            )
            .await?;
        }
        Ok(inserted)
    }

    pub async fn add_evidence(
        &self,
        request: AddEvidenceRequest,
    ) -> Result<WorkItem, WorkGraphError> {
        let now = self.store.get_store_time_utc().await?;
        let item = self
            .get(
                request.realm_id.clone(),
                request.namespace.clone(),
                request.id.clone(),
            )
            .await?;
        let expected_previous_revision = item.revision;
        let (item, event) = WorkGraphMachine::add_evidence(item, request, now)?;
        self.store
            .update_item_cas(item, expected_previous_revision, event)
            .await
    }

    pub async fn events(
        &self,
        mut filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        if filter.realm_id.is_none() {
            filter.realm_id = Some(self.default_realm_id.to_string());
        }
        if !filter.all_namespaces && filter.namespace.is_none() {
            filter.namespace = Some(self.default_namespace.clone());
        }
        self.store.list_events(filter).await
    }

    fn scope(
        &self,
        realm_id: Option<String>,
        namespace: Option<WorkNamespace>,
    ) -> (String, WorkNamespace) {
        (
            realm_id.unwrap_or_else(|| self.default_realm_id.to_string()),
            namespace.unwrap_or_else(|| self.default_namespace.clone()),
        )
    }

    fn normalize_item_filter(&self, mut filter: WorkItemFilter) -> WorkItemFilter {
        if filter.realm_id.is_none() {
            filter.realm_id = Some(self.default_realm_id.to_string());
        }
        if !filter.all_namespaces && filter.namespace.is_none() {
            filter.namespace = Some(self.default_namespace.clone());
        }
        filter
    }

    fn normalize_snapshot_filter(
        &self,
        mut filter: WorkGraphSnapshotFilter,
    ) -> WorkGraphSnapshotFilter {
        if filter.realm_id.is_none() {
            filter.realm_id = Some(self.default_realm_id.to_string());
        }
        if !filter.all_namespaces && filter.namespace.is_none() {
            filter.namespace = Some(self.default_namespace.clone());
        }
        filter
    }

    async fn snapshot_namespaces(
        &self,
        realm_id: &str,
        filter: &WorkGraphSnapshotFilter,
        items: &[WorkItem],
    ) -> Result<BTreeSet<WorkNamespace>, WorkGraphError> {
        if !filter.all_namespaces {
            return Ok(BTreeSet::from_iter([filter
                .namespace
                .clone()
                .unwrap_or_else(|| self.default_namespace.clone())]));
        }

        let mut namespaces = items
            .iter()
            .map(|item| item.namespace.clone())
            .collect::<BTreeSet<_>>();
        if namespaces.is_empty() {
            namespaces.extend(
                self.store
                    .list_events(WorkGraphEventFilter {
                        realm_id: Some(realm_id.to_string()),
                        namespace: None,
                        all_namespaces: true,
                        after_seq: None,
                        limit: None,
                    })
                    .await?
                    .into_iter()
                    .map(|event| event.namespace),
            );
        }
        Ok(namespaces)
    }

    async fn ready_item_ids_in_namespaces(
        &self,
        realm_id: &str,
        namespaces: &BTreeSet<WorkNamespace>,
        labels: &[String],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<WorkItemId>, WorkGraphError> {
        let mut ready_ids = Vec::new();
        for namespace in namespaces {
            let all_items = self
                .store
                .list_items(WorkItemFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                })
                .await?;
            let ready_items = WorkGraphMachine::ready_items(
                all_items
                    .into_iter()
                    .filter(|item| labels.iter().all(|label| item.labels.contains(label)))
                    .collect(),
                now,
            );
            ready_ids.extend(ready_items.into_iter().map(|item| item.id));
        }
        Ok(ready_ids)
    }

    async fn refresh_dependents_after_blocker_change(
        &self,
        blocker: &WorkItem,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), WorkGraphError> {
        let edges = self
            .store
            .list_edges(&blocker.realm_id, &blocker.namespace)
            .await?;
        for edge in edges
            .iter()
            .filter(|edge| edge.kind == WorkEdgeKind::Blocks && edge.from_id == blocker.id)
        {
            self.refresh_item_eligibility(&blocker.realm_id, &blocker.namespace, &edge.to_id, now)
                .await?;
        }
        Ok(())
    }

    async fn refresh_item_eligibility(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), WorkGraphError> {
        let Some(item) = self.store.get_item(realm_id, namespace, id).await? else {
            return Ok(());
        };
        let all_items = self
            .store
            .list_items(WorkItemFilter {
                realm_id: Some(realm_id.to_string()),
                namespace: Some(namespace.clone()),
                include_terminal: true,
                ..WorkItemFilter::default()
            })
            .await?
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect::<BTreeMap<_, _>>();
        let edges = self.store.list_edges(realm_id, namespace).await?;
        let unresolved_blockers = unresolved_blocker_count(&item, &all_items, &edges);
        if let Some((item, event)) =
            WorkGraphMachine::refresh_eligibility(item, unresolved_blockers, now)?
        {
            let expected_previous_revision = item.revision;
            self.store
                .update_item_cas(item, expected_previous_revision, event)
                .await?;
        }
        Ok(())
    }
}

fn unresolved_blocker_count(
    item: &WorkItem,
    all_items: &BTreeMap<WorkItemId, WorkItem>,
    edges: &[WorkEdge],
) -> u64 {
    edges
        .iter()
        .filter(|edge| edge.kind == WorkEdgeKind::Blocks && edge.to_id == item.id)
        .filter(|edge| {
            all_items
                .get(&edge.from_id)
                .is_none_or(|blocker| !blocker.status.is_terminal_success())
        })
        .count()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use crate::store::WorkGraphEventFilter;
    use crate::types::{
        ClaimWorkItemRequest, LinkWorkItemsRequest, WorkEdgeKind, WorkOwner, WorkOwnerKey,
    };
    use crate::{
        CreateWorkItemRequest, MemoryWorkGraphStore, UpdateWorkItemRequest, WorkGraphService,
        WorkNamespace,
    };

    fn create_req(title: &str) -> CreateWorkItemRequest {
        CreateWorkItemRequest {
            realm_id: None,
            namespace: None,
            title: title.to_string(),
            description: None,
            priority: Default::default(),
            labels: BTreeSet::new(),
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
            evidence_refs: Vec::new(),
            status: None,
        }
    }

    #[tokio::test]
    async fn blocked_dependencies_are_not_ready_until_completed() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let blocker = service
            .create(create_req("blocker"))
            .await
            .expect("blocker");
        let blocked = service
            .create(create_req("blocked"))
            .await
            .expect("blocked");
        service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: blocker.id.clone(),
                to_id: blocked.id.clone(),
            })
            .await
            .expect("link");

        let ready = service.ready(Default::default()).await.expect("ready");
        assert!(ready.iter().any(|item| item.id == blocker.id));
        assert!(!ready.iter().any(|item| item.id == blocked.id));
        service
            .close(crate::CloseWorkItemRequest {
                id: blocker.id,
                realm_id: None,
                namespace: None,
                expected_revision: blocker.revision,
                status: crate::WorkStatus::Completed,
            })
            .await
            .expect("close blocker");
        let ready = service.ready(Default::default()).await.expect("ready");
        assert!(ready.iter().any(|item| item.id == blocked.id));
    }

    #[tokio::test]
    async fn blocked_dependency_stays_unready_after_item_update() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let blocker = service
            .create(create_req("blocker"))
            .await
            .expect("blocker");
        let blocked = service
            .create(create_req("blocked"))
            .await
            .expect("blocked");
        service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: blocker.id,
                to_id: blocked.id.clone(),
            })
            .await
            .expect("link");
        let blocked = service
            .get(None, None, blocked.id.clone())
            .await
            .expect("blocked after link");

        service
            .update(UpdateWorkItemRequest {
                id: blocked.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: blocked.revision,
                title: Some("blocked, updated".to_string()),
                description: None,
                priority: None,
                labels: None,
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
            })
            .await
            .expect("update blocked item");

        let ready = service.ready(Default::default()).await.expect("ready");
        assert!(!ready.iter().any(|item| item.id == blocked.id));
    }

    #[tokio::test]
    async fn concurrent_claim_attempts_have_one_winner() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let item = service.create(create_req("claim")).await.expect("create");
        let request = ClaimWorkItemRequest {
            id: item.id,
            realm_id: None,
            namespace: None,
            expected_revision: item.revision,
            owner: WorkOwner::new(WorkOwnerKey::label("worker").expect("owner key")),
            lease_seconds: Some(60),
            lease_expires_at: None,
        };
        let first = service.claim(request.clone()).await;
        let second = service.claim(request).await;
        assert!(first.is_ok() ^ second.is_ok());
    }

    #[tokio::test]
    async fn dependency_cycles_are_rejected() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let first = service.create(create_req("first")).await.expect("first");
        let second = service.create(create_req("second")).await.expect("second");
        service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: first.id.clone(),
                to_id: second.id.clone(),
            })
            .await
            .expect("first edge");
        let error = service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: second.id,
                to_id: first.id,
            })
            .await
            .expect_err("cycle should fail");
        assert!(matches!(error, crate::WorkGraphError::InvalidTransition(_)));
    }

    #[tokio::test]
    async fn topology_rejects_self_duplicate_and_missing_endpoint_edges() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let first = service.create(create_req("first")).await.expect("first");
        let second = service.create(create_req("second")).await.expect("second");

        let self_edge = service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: first.id.clone(),
                to_id: first.id.clone(),
            })
            .await
            .expect_err("self edge should fail");
        assert!(matches!(
            self_edge,
            crate::WorkGraphError::InvalidTransition(_)
        ));

        let missing_endpoint = service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: first.id.clone(),
                to_id: crate::WorkItemId::generated(),
            })
            .await
            .expect_err("missing endpoint should fail");
        assert!(matches!(
            missing_endpoint,
            crate::WorkGraphError::InvalidTransition(_)
        ));

        service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: first.id.clone(),
                to_id: second.id.clone(),
            })
            .await
            .expect("first edge");

        let duplicate = service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: first.id,
                to_id: second.id,
            })
            .await
            .expect_err("duplicate edge should fail");
        assert!(matches!(
            duplicate,
            crate::WorkGraphError::InvalidTransition(_)
        ));
    }

    #[tokio::test]
    async fn snapshot_includes_items_edges_ready_ids_and_event_high_water_mark() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let blocker = service
            .create(create_req("blocker"))
            .await
            .expect("blocker");
        let blocked = service
            .create(create_req("blocked"))
            .await
            .expect("blocked");
        service
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Blocks,
                from_id: blocker.id.clone(),
                to_id: blocked.id.clone(),
            })
            .await
            .expect("link");

        let snapshot = service
            .snapshot(crate::WorkGraphSnapshotFilter::default())
            .await
            .expect("snapshot");
        assert_eq!(snapshot.realm_id, "realm");
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.edges.len(), 1);
        assert!(snapshot.ready_item_ids.iter().any(|id| id == &blocker.id));
        assert!(!snapshot.ready_item_ids.iter().any(|id| id == &blocked.id));
        assert!(snapshot.event_high_water_mark.is_some());
    }

    #[tokio::test]
    async fn events_can_span_all_namespaces_when_requested() {
        let store = Arc::new(MemoryWorkGraphStore::new());
        let default_service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let other_service = WorkGraphService::with_scope(
            store,
            "realm",
            WorkNamespace::new("other").expect("namespace"),
        );

        default_service
            .create(create_req("default item"))
            .await
            .expect("default item");
        other_service
            .create(create_req("other item"))
            .await
            .expect("other item");

        let default_events = default_service
            .events(WorkGraphEventFilter::default())
            .await
            .expect("default events");
        assert_eq!(default_events.len(), 1);

        let all_events = default_service
            .events(WorkGraphEventFilter {
                all_namespaces: true,
                ..WorkGraphEventFilter::default()
            })
            .await
            .expect("all events");
        assert_eq!(all_events.len(), 2);
    }
}

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(not(target_arch = "wasm32"))]
use rusqlite::{
    Connection, Error, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::WorkGraphError;
use crate::types::{
    AttentionListRequest, AttentionPruneRequest, WorkAttentionBinding, WorkAttentionBindingId,
    WorkAttentionStatus, WorkEdge, WorkGraphEvent, WorkGraphEventKind, WorkItem, WorkItemFilter,
    WorkItemId, WorkNamespace,
};
use crate::{WorkAttentionMachine, WorkGraphMachine};

#[cfg(not(target_arch = "wasm32"))]
const SQLITE_BUSY_TIMEOUT_MS: u64 = 5000;

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkGraphStoreKind {
    Disabled,
    Memory,
    Sqlite,
    Custom,
}

impl WorkGraphStoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Memory => "memory",
            Self::Sqlite => "sqlite",
            Self::Custom => "custom",
        }
    }
}

impl std::fmt::Display for WorkGraphStoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkGraphEventFilter {
    pub realm_id: Option<String>,
    pub namespace: Option<WorkNamespace>,
    #[serde(default)]
    pub all_namespaces: bool,
    pub after_seq: Option<i64>,
    pub limit: Option<usize>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait WorkGraphStore: Send + Sync {
    fn kind(&self) -> WorkGraphStoreKind;

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError>;

    async fn insert_item(
        &self,
        item: WorkItem,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError>;

    async fn update_item_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError>;

    async fn update_item_and_attention_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        item_event: WorkGraphEvent,
        attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
    ) -> Result<WorkItem, WorkGraphError>;

    async fn get_item(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError>;

    async fn list_items(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError>;

    async fn insert_goal(
        &self,
        _item: WorkItem,
        _item_event: WorkGraphEvent,
        _attention: WorkAttentionBinding,
        _attention_event: WorkGraphEvent,
    ) -> Result<(WorkItem, WorkAttentionBinding), WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn update_attention_cas(
        &self,
        _attention: WorkAttentionBinding,
        _expected_previous_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn reassign_attention_cas(
        &self,
        _previous: WorkAttentionBinding,
        _expected_previous_revision: u64,
        _previous_event: WorkGraphEvent,
        _replacement: WorkAttentionBinding,
        _replacement_event: WorkGraphEvent,
    ) -> Result<(WorkAttentionBinding, WorkAttentionBinding), WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn get_attention(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _binding_id: &WorkAttentionBindingId,
    ) -> Result<Option<WorkAttentionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_attention(
        &self,
        _filter: AttentionListRequest,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    /// Return at most `limit` attention rows. Backends should push this bound
    /// into iteration/query ownership; the default is compatibility-only for
    /// custom stores.
    async fn list_attention_bounded(
        &self,
        filter: AttentionListRequest,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        let mut bindings = self.list_attention(filter).await?;
        bindings.truncate(limit);
        Ok(bindings)
    }

    /// Delete TERMINAL (superseded/stopped) attention binding rows in scope.
    /// The event stream keeps the audit history; binding rows otherwise grow
    /// monotonically with reassignment churn. Returns the pruned row count.
    async fn prune_terminal_attention(
        &self,
        _filter: AttentionPruneRequest,
    ) -> Result<u64, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_edge(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError>;

    async fn insert_edge_validated(
        &self,
        _edge: WorkEdge,
        _event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_edges(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError>;

    /// Return at most `limit` edges in one namespace.
    async fn list_edges_bounded(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        limit: usize,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        let mut edges = self.list_edges(realm_id, namespace).await?;
        edges.truncate(limit);
        Ok(edges)
    }

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError>;

    /// Highest sequence matching a scope without retaining the event history.
    async fn latest_event_seq(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Option<i64>, WorkGraphError> {
        Ok(self
            .list_events(filter)
            .await?
            .into_iter()
            .filter_map(|event| event.seq)
            .max())
    }
}

#[derive(Default)]
pub struct DisabledWorkGraphStore;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WorkGraphStore for DisabledWorkGraphStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Disabled
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_item(
        &self,
        _item: WorkItem,
        _event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn update_item_cas(
        &self,
        _item: WorkItem,
        _expected_previous_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn update_item_and_attention_cas(
        &self,
        _item: WorkItem,
        _expected_previous_revision: u64,
        _item_event: WorkGraphEvent,
        _attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
    ) -> Result<WorkItem, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn get_item(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_items(&self, _filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_goal(
        &self,
        _item: WorkItem,
        _item_event: WorkGraphEvent,
        _attention: WorkAttentionBinding,
        _attention_event: WorkGraphEvent,
    ) -> Result<(WorkItem, WorkAttentionBinding), WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn update_attention_cas(
        &self,
        _attention: WorkAttentionBinding,
        _expected_previous_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn get_attention(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _binding_id: &WorkAttentionBindingId,
    ) -> Result<Option<WorkAttentionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_attention(
        &self,
        _filter: AttentionListRequest,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_edge(
        &self,
        _edge: WorkEdge,
        _event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn insert_edge_validated(
        &self,
        _edge: WorkEdge,
        _event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_edges(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_events(
        &self,
        _filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }
}

fn unsupported(kind: WorkGraphStoreKind) -> WorkGraphError {
    WorkGraphError::UnsupportedBackend(kind.to_string())
}

#[derive(Default)]
pub struct MemoryWorkGraphStore {
    inner: Arc<RwLock<MemoryWorkGraphState>>,
}

#[derive(Default)]
struct MemoryWorkGraphState {
    items: BTreeMap<(String, WorkNamespace, WorkItemId), WorkItem>,
    attention: BTreeMap<(String, WorkNamespace, WorkAttentionBindingId), WorkAttentionBinding>,
    edges: Vec<WorkEdge>,
    events: Vec<WorkGraphEvent>,
    next_event_seq: i64,
}

impl MemoryWorkGraphStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WorkGraphStore for MemoryWorkGraphStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Memory
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        Ok(Utc::now())
    }

    async fn insert_item(
        &self,
        item: WorkItem,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        let mut guard = self.inner.write().await;
        let key = item_key(&item.realm_id, &item.namespace, &item.id);
        if guard.items.contains_key(&key) {
            return Err(WorkGraphError::Conflict(format!(
                "work item {} already exists",
                item.id
            )));
        }
        guard.items.insert(key, item.clone());
        guard.append_event(event);
        Ok(item)
    }

    async fn update_item_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        let mut guard = self.inner.write().await;
        let key = item_key(&item.realm_id, &item.namespace, &item.id);
        let Some(current) = guard.items.get(&key) else {
            return Err(WorkGraphError::not_found(
                item.realm_id.clone(),
                item.namespace.clone(),
                item.id.clone(),
            ));
        };
        if current.revision != expected_previous_revision {
            return Err(WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected: expected_previous_revision,
                actual: current.revision,
            });
        }
        guard.items.insert(key, item.clone());
        guard.append_event(event);
        Ok(item)
    }

    async fn get_item(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard.items.get(&item_key(realm_id, namespace, id)).cloned())
    }

    async fn list_items(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        let guard = self.inner.read().await;
        let compare = |left: &WorkItem, right: &WorkItem| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        };
        if let Some(limit) = filter.limit {
            let mut items = Vec::with_capacity(limit.min(1024));
            for item in guard
                .items
                .values()
                .filter(|item| item_matches_filter(item, &filter))
            {
                let index = items
                    .binary_search_by(|existing| compare(existing, item))
                    .unwrap_or_else(|index| index);
                if index < limit {
                    items.insert(index, item.clone());
                    if items.len() > limit {
                        items.pop();
                    }
                }
            }
            return Ok(items);
        }
        let mut items = guard
            .items
            .values()
            .filter(|item| item_matches_filter(item, &filter))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(compare);
        Ok(items)
    }

    async fn insert_goal(
        &self,
        item: WorkItem,
        item_event: WorkGraphEvent,
        attention: WorkAttentionBinding,
        attention_event: WorkGraphEvent,
    ) -> Result<(WorkItem, WorkAttentionBinding), WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        let mut guard = self.inner.write().await;
        let item_key = item_key(&item.realm_id, &item.namespace, &item.id);
        if guard.items.contains_key(&item_key) {
            return Err(WorkGraphError::Conflict(format!(
                "work item {} already exists",
                item.id
            )));
        }
        let attention_key = attention_key(
            &attention.work_ref.realm_id,
            &attention.work_ref.namespace,
            &attention.binding_id,
        );
        if guard.attention.contains_key(&attention_key) {
            return Err(WorkGraphError::Conflict(format!(
                "work attention binding {} already exists",
                attention.binding_id
            )));
        }
        if let Some(occupant) = active_target_occupant_in(guard.attention.values(), &attention) {
            return Err(active_target_conflict(&attention, &occupant));
        }
        guard.items.insert(item_key, item.clone());
        guard.attention.insert(attention_key, attention.clone());
        guard.append_event(item_event);
        guard.append_event(attention_event);
        Ok((item, attention))
    }

    async fn update_attention_cas(
        &self,
        attention: WorkAttentionBinding,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        let mut guard = self.inner.write().await;
        let key = attention_key(
            &attention.work_ref.realm_id,
            &attention.work_ref.namespace,
            &attention.binding_id,
        );
        let Some(current) = guard.attention.get(&key) else {
            return Err(WorkGraphError::not_found(
                attention.work_ref.realm_id.clone(),
                attention.work_ref.namespace.clone(),
                attention.work_ref.item_id.clone(),
            ));
        };
        if current.machine_state.revision != expected_previous_revision {
            return Err(WorkGraphError::StaleRevision {
                id: attention.work_ref.item_id.clone(),
                expected: expected_previous_revision,
                actual: current.machine_state.revision,
            });
        }
        if let Some(occupant) = active_target_occupant_in(guard.attention.values(), &attention) {
            return Err(active_target_conflict(&attention, &occupant));
        }
        guard.attention.insert(key, attention.clone());
        guard.append_event(event);
        Ok(attention)
    }

    async fn reassign_attention_cas(
        &self,
        previous: WorkAttentionBinding,
        expected_previous_revision: u64,
        previous_event: WorkGraphEvent,
        replacement: WorkAttentionBinding,
        replacement_event: WorkGraphEvent,
    ) -> Result<(WorkAttentionBinding, WorkAttentionBinding), WorkGraphError> {
        let mut guard = self.inner.write().await;
        let previous_key = attention_key(
            &previous.work_ref.realm_id,
            &previous.work_ref.namespace,
            &previous.binding_id,
        );
        let Some(current) = guard.attention.get(&previous_key) else {
            return Err(WorkGraphError::attention_not_found(
                previous.work_ref.realm_id.clone(),
                previous.work_ref.namespace.clone(),
                previous.binding_id.clone(),
            ));
        };
        if current.machine_state.revision != expected_previous_revision {
            return Err(WorkGraphError::StaleRevision {
                id: previous.work_ref.item_id.clone(),
                expected: expected_previous_revision,
                actual: current.machine_state.revision,
            });
        }
        let replacement_key = attention_key(
            &replacement.work_ref.realm_id,
            &replacement.work_ref.namespace,
            &replacement.binding_id,
        );
        if guard.attention.contains_key(&replacement_key) {
            return Err(WorkGraphError::Conflict(format!(
                "work attention binding {} already exists",
                replacement.binding_id
            )));
        }
        // Occupancy over the post-reassign state: `previous` is being
        // superseded in this same mutation, so it is excluded from the probe.
        if let Some(occupant) = active_target_occupant_in(
            guard
                .attention
                .values()
                .filter(|binding| binding.binding_id != previous.binding_id),
            &replacement,
        ) {
            return Err(active_target_conflict(&replacement, &occupant));
        }
        guard.attention.insert(previous_key, previous.clone());
        guard.attention.insert(replacement_key, replacement.clone());
        guard.append_event(previous_event);
        guard.append_event(replacement_event);
        Ok((previous, replacement))
    }

    async fn update_item_and_attention_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        item_event: WorkGraphEvent,
        attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        let mut guard = self.inner.write().await;
        let key = item_key(&item.realm_id, &item.namespace, &item.id);
        let Some(current) = guard.items.get(&key) else {
            return Err(WorkGraphError::not_found(
                item.realm_id.clone(),
                item.namespace.clone(),
                item.id.clone(),
            ));
        };
        if current.revision != expected_previous_revision {
            return Err(WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected: expected_previous_revision,
                actual: current.revision,
            });
        }
        for (attention, expected_revision, _) in &attention_updates {
            let key = attention_key(
                &attention.work_ref.realm_id,
                &attention.work_ref.namespace,
                &attention.binding_id,
            );
            let Some(current) = guard.attention.get(&key) else {
                return Err(WorkGraphError::not_found(
                    attention.work_ref.realm_id.clone(),
                    attention.work_ref.namespace.clone(),
                    attention.work_ref.item_id.clone(),
                ));
            };
            if current.machine_state.revision != *expected_revision {
                return Err(WorkGraphError::StaleRevision {
                    id: attention.work_ref.item_id.clone(),
                    expected: *expected_revision,
                    actual: current.machine_state.revision,
                });
            }
        }
        // Occupancy over the post-update state: exclude every binding this
        // batch rewrites, then judge each Active-status update against the
        // survivors plus its already-applied batch predecessors.
        let batch_ids: Vec<WorkAttentionBindingId> = attention_updates
            .iter()
            .map(|(attention, _, _)| attention.binding_id.clone())
            .collect();
        for (index, (attention, _, _)) in attention_updates.iter().enumerate() {
            let occupant = active_target_occupant_in(
                guard
                    .attention
                    .values()
                    .filter(|binding| !batch_ids.contains(&binding.binding_id))
                    .chain(
                        attention_updates[..index]
                            .iter()
                            .map(|(applied, _, _)| applied),
                    ),
                attention,
            );
            if let Some(occupant) = occupant {
                return Err(active_target_conflict(attention, &occupant));
            }
        }
        guard.items.insert(key, item.clone());
        guard.append_event(item_event);
        for (attention, _, event) in attention_updates {
            let key = attention_key(
                &attention.work_ref.realm_id,
                &attention.work_ref.namespace,
                &attention.binding_id,
            );
            guard.attention.insert(key, attention);
            guard.append_event(event);
        }
        Ok(item)
    }

    async fn get_attention(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        binding_id: &WorkAttentionBindingId,
    ) -> Result<Option<WorkAttentionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .attention
            .get(&attention_key(realm_id, namespace, binding_id))
            .cloned())
    }

    async fn list_attention(
        &self,
        filter: AttentionListRequest,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        let mut bindings = guard
            .attention
            .values()
            .filter(|binding| attention_matches_filter(binding, &filter))
            .cloned()
            .collect::<Vec<_>>();
        bindings.sort_by(|left, right| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.binding_id.cmp(&right.binding_id))
        });
        Ok(bindings)
    }

    async fn list_attention_bounded(
        &self,
        filter: AttentionListRequest,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        let compare = |left: &WorkAttentionBinding, right: &WorkAttentionBinding| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.binding_id.cmp(&right.binding_id))
        };
        let mut bindings = Vec::with_capacity(limit.min(1024));
        for binding in guard
            .attention
            .values()
            .filter(|binding| attention_matches_filter(binding, &filter))
        {
            let index = bindings
                .binary_search_by(|existing| compare(existing, binding))
                .unwrap_or_else(|index| index);
            if index < limit {
                bindings.insert(index, binding.clone());
                if bindings.len() > limit {
                    bindings.pop();
                }
            }
        }
        Ok(bindings)
    }

    async fn prune_terminal_attention(
        &self,
        filter: AttentionPruneRequest,
    ) -> Result<u64, WorkGraphError> {
        let mut guard = self.inner.write().await;
        let before = guard.attention.len();
        guard.attention.retain(|_, binding| {
            let in_scope = filter
                .realm_id
                .as_ref()
                .is_none_or(|realm_id| &binding.work_ref.realm_id == realm_id)
                && filter
                    .namespace
                    .as_ref()
                    .is_none_or(|namespace| &binding.work_ref.namespace == namespace)
                && filter
                    .updated_before
                    .is_none_or(|updated_before| binding.updated_at < updated_before);
            !(in_scope && binding.status.is_terminal())
        });
        Ok((before - guard.attention.len()) as u64)
    }

    async fn insert_edge(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        let mut guard = self.inner.write().await;
        if guard.edges.iter().any(|existing| existing == &edge) {
            return Err(duplicate_edge_error(&edge));
        }
        guard.edges.push(edge.clone());
        guard.append_event(event);
        Ok(edge)
    }

    async fn insert_edge_validated(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        let mut guard = self.inner.write().await;
        if guard.edges.iter().any(|existing| existing == &edge) {
            return Err(duplicate_edge_error(&edge));
        }
        let existing_edges = guard
            .edges
            .iter()
            .filter(|existing| {
                existing.realm_id == edge.realm_id && existing.namespace == edge.namespace
            })
            .cloned()
            .collect::<Vec<_>>();
        let existing_items = guard
            .items
            .values()
            .filter(|item| item.realm_id == edge.realm_id && item.namespace == edge.namespace)
            .cloned()
            .collect::<Vec<_>>();
        WorkGraphMachine::validate_link(&edge, &existing_items, &existing_edges)?;
        guard.edges.push(edge.clone());
        guard.append_event(event);
        Ok(edge)
    }

    async fn list_edges(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && edge.namespace == *namespace)
            .cloned()
            .collect())
    }

    async fn list_edges_bounded(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        limit: usize,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && edge.namespace == *namespace)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        let guard = self.inner.read().await;
        let events = guard
            .events
            .iter()
            .filter(|event| event_matches_filter(event, &filter))
            .take(filter.limit.unwrap_or(usize::MAX))
            .cloned()
            .collect::<Vec<_>>();
        Ok(events)
    }

    async fn latest_event_seq(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Option<i64>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .events
            .iter()
            .filter(|event| event_matches_filter(event, &filter))
            .filter_map(|event| event.seq)
            .max())
    }
}

impl MemoryWorkGraphState {
    fn append_event(&mut self, mut event: WorkGraphEvent) {
        self.next_event_seq += 1;
        event.seq = Some(self.next_event_seq);
        self.events.push(event);
    }
}

fn item_key(
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkItemId,
) -> (String, WorkNamespace, WorkItemId) {
    (realm_id.to_string(), namespace.clone(), id.clone())
}

fn attention_key(
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkAttentionBindingId,
) -> (String, WorkNamespace, WorkAttentionBindingId) {
    (realm_id.to_string(), namespace.clone(), id.clone())
}

fn item_matches_filter(item: &WorkItem, filter: &WorkItemFilter) -> bool {
    if let Some(realm_id) = &filter.realm_id
        && &item.realm_id != realm_id
    {
        return false;
    }
    if !filter.all_namespaces
        && let Some(namespace) = &filter.namespace
        && &item.namespace != namespace
    {
        return false;
    }
    if !filter.statuses.is_empty() && !filter.statuses.contains(&item.status) {
        return false;
    }
    // The terminality verdict (which lifecycle phases are terminal) is a machine
    // fact owned by WorkGraphLifecycleMachine, not this filter. We drive the
    // machine's ClassifyTerminality over the item's recovered state and mirror the
    // verdict, failing closed: an item the machine cannot classify is treated as
    // terminal so it is never surfaced as live work when terminals are excluded.
    if !filter.include_terminal && WorkGraphMachine::classify_terminality(item).unwrap_or(true) {
        return false;
    }
    filter
        .labels
        .iter()
        .all(|label| item.labels.contains(label))
}

fn attention_matches_filter(binding: &WorkAttentionBinding, filter: &AttentionListRequest) -> bool {
    if let Some(realm_id) = &filter.realm_id
        && &binding.work_ref.realm_id != realm_id
    {
        return false;
    }
    if let Some(namespace) = &filter.namespace
        && &binding.work_ref.namespace != namespace
    {
        return false;
    }
    if let Some(target) = &filter.target
        && &binding.target != target
    {
        return false;
    }
    if let Some(status) = &filter.status
        && !attention_status_matches_filter(&binding.status, status)
    {
        return false;
    }
    true
}

fn attention_status_matches_filter(
    actual: &crate::types::WorkAttentionStatus,
    filter: &crate::types::WorkAttentionStatus,
) -> bool {
    use crate::types::WorkAttentionStatus;

    match (actual, filter) {
        (WorkAttentionStatus::Active, WorkAttentionStatus::Active)
        | (WorkAttentionStatus::Superseded, WorkAttentionStatus::Superseded)
        | (WorkAttentionStatus::Stopped, WorkAttentionStatus::Stopped) => true,
        (WorkAttentionStatus::Paused { .. }, WorkAttentionStatus::Paused { until: None }) => true,
        (
            WorkAttentionStatus::Paused {
                until: Some(actual_until),
            },
            WorkAttentionStatus::Paused {
                until: Some(filter_until),
            },
        ) => actual_until == filter_until,
        _ => false,
    }
}

fn event_matches_filter(event: &WorkGraphEvent, filter: &WorkGraphEventFilter) -> bool {
    if let Some(after_seq) = filter.after_seq
        && event.seq.unwrap_or_default() <= after_seq
    {
        return false;
    }
    if let Some(realm_id) = &filter.realm_id
        && &event.realm_id != realm_id
    {
        return false;
    }
    if !filter.all_namespaces
        && let Some(namespace) = &filter.namespace
        && &event.namespace != namespace
    {
        return false;
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
pub struct SqliteWorkGraphStore {
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl SqliteWorkGraphStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, WorkGraphError> {
        let store = Self { path: path.into() };
        store.with_connection(migrate_sqlite_attention_query_columns)?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rebuild_projection_from_events(&self) -> Result<(), WorkGraphError> {
        self.with_connection(|conn| {
            // Rebuild is a whole-projection writer: it must acquire the write
            // lock before deleting projected rows so concurrent writers either
            // wait on busy_timeout or proceed after the rebuild commits.
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            tx.execute("DELETE FROM workgraph_items", [])
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            tx.execute("DELETE FROM workgraph_edges", [])
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            tx.execute("DELETE FROM workgraph_attention", [])
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;

            let events = {
                let mut stmt = tx
                    .prepare("SELECT event_json FROM workgraph_events ORDER BY seq ASC")
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| row_json::<WorkGraphEvent>(row, 0))
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let mut events = Vec::new();
                for row in rows {
                    events.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
                }
                events
            };

            for event in events {
                replay_event_tx(&tx, &event)?;
            }
            normalize_attention_for_terminal_items_tx(&tx)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))
        })
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, WorkGraphError>,
    ) -> Result<T, WorkGraphError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        }
        let mut conn =
            Connection::open(&self.path).map_err(|err| WorkGraphError::Store(err.to_string()))?;
        conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        init_sqlite_schema(&conn)?;
        f(&mut conn)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl WorkGraphStore for SqliteWorkGraphStore {
    fn kind(&self) -> WorkGraphStoreKind {
        WorkGraphStoreKind::Sqlite
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
        Ok(Utc::now())
    }

    async fn insert_item(
        &self,
        item: WorkItem,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            insert_item_tx(&tx, &item)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(item)
        })
    }

    async fn update_item_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let changed = update_item_tx(&tx, &item, expected_previous_revision)?;
            if changed == 0 {
                let actual = current_revision_tx(&tx, &item.realm_id, &item.namespace, &item.id)?;
                return match actual {
                    Some(actual) => Err(WorkGraphError::StaleRevision {
                        id: item.id,
                        expected: expected_previous_revision,
                        actual,
                    }),
                    None => Err(WorkGraphError::not_found(
                        item.realm_id,
                        item.namespace,
                        item.id,
                    )),
                };
            }
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(item)
        })
    }

    async fn get_item(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        id: &WorkItemId,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| select_item(conn, realm_id, namespace, id))
    }

    async fn list_items(&self, filter: WorkItemFilter) -> Result<Vec<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_items(conn, &filter))
    }

    async fn insert_goal(
        &self,
        item: WorkItem,
        item_event: WorkGraphEvent,
        attention: WorkAttentionBinding,
        attention_event: WorkGraphEvent,
    ) -> Result<(WorkItem, WorkAttentionBinding), WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            if let Some(occupant) = active_target_occupant_tx(&tx, &attention)? {
                return Err(active_target_conflict(&attention, &occupant));
            }
            insert_item_tx(&tx, &item)?;
            insert_attention_tx(&tx, &attention)?;
            insert_event_tx(&tx, &item_event)?;
            insert_event_tx(&tx, &attention_event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok((item, attention))
        })
    }

    async fn update_attention_cas(
        &self,
        attention: WorkAttentionBinding,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let changed = update_attention_tx(&tx, &attention, expected_previous_revision)?;
            if changed == 0 {
                let actual = current_attention_revision_tx(
                    &tx,
                    &attention.work_ref.realm_id,
                    &attention.work_ref.namespace,
                    &attention.binding_id,
                )?;
                return match actual {
                    Some(actual) => Err(WorkGraphError::StaleRevision {
                        id: attention.work_ref.item_id,
                        expected: expected_previous_revision,
                        actual,
                    }),
                    None => Err(WorkGraphError::not_found(
                        attention.work_ref.realm_id,
                        attention.work_ref.namespace,
                        attention.work_ref.item_id,
                    )),
                };
            }
            // Occupancy after the row rewrite (the probe excludes the
            // candidate itself); a conflict drops the transaction, rolling
            // the rewrite back.
            if let Some(occupant) = active_target_occupant_tx(&tx, &attention)? {
                return Err(active_target_conflict(&attention, &occupant));
            }
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(attention)
        })
    }

    async fn reassign_attention_cas(
        &self,
        previous: WorkAttentionBinding,
        expected_previous_revision: u64,
        previous_event: WorkGraphEvent,
        replacement: WorkAttentionBinding,
        replacement_event: WorkGraphEvent,
    ) -> Result<(WorkAttentionBinding, WorkAttentionBinding), WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let changed = update_attention_tx(&tx, &previous, expected_previous_revision)?;
            if changed == 0 {
                let actual = current_attention_revision_tx(
                    &tx,
                    &previous.work_ref.realm_id,
                    &previous.work_ref.namespace,
                    &previous.binding_id,
                )?;
                return match actual {
                    Some(actual) => Err(WorkGraphError::StaleRevision {
                        id: previous.work_ref.item_id,
                        expected: expected_previous_revision,
                        actual,
                    }),
                    None => Err(WorkGraphError::attention_not_found(
                        previous.work_ref.realm_id,
                        previous.work_ref.namespace,
                        previous.binding_id,
                    )),
                };
            }
            // Occupancy over the post-reassign state: `previous` was just
            // rewritten to Superseded inside this transaction, so the probe
            // no longer sees it as active.
            if let Some(occupant) = active_target_occupant_tx(&tx, &replacement)? {
                return Err(active_target_conflict(&replacement, &occupant));
            }
            insert_attention_tx(&tx, &replacement)?;
            insert_event_tx(&tx, &previous_event)?;
            insert_event_tx(&tx, &replacement_event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok((previous, replacement))
        })
    }

    async fn update_item_and_attention_cas(
        &self,
        item: WorkItem,
        expected_previous_revision: u64,
        item_event: WorkGraphEvent,
        attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let changed = update_item_tx(&tx, &item, expected_previous_revision)?;
            if changed == 0 {
                let actual = current_revision_tx(&tx, &item.realm_id, &item.namespace, &item.id)?;
                return match actual {
                    Some(actual) => Err(WorkGraphError::StaleRevision {
                        id: item.id,
                        expected: expected_previous_revision,
                        actual,
                    }),
                    None => Err(WorkGraphError::not_found(
                        item.realm_id,
                        item.namespace,
                        item.id,
                    )),
                };
            }
            insert_event_tx(&tx, &item_event)?;
            for (attention, expected_revision, event) in &attention_updates {
                let changed = update_attention_tx(&tx, attention, *expected_revision)?;
                if changed == 0 {
                    let actual = current_attention_revision_tx(
                        &tx,
                        &attention.work_ref.realm_id,
                        &attention.work_ref.namespace,
                        &attention.binding_id,
                    )?;
                    return match actual {
                        Some(actual) => Err(WorkGraphError::StaleRevision {
                            id: attention.work_ref.item_id.clone(),
                            expected: *expected_revision,
                            actual,
                        }),
                        None => Err(WorkGraphError::not_found(
                            attention.work_ref.realm_id.clone(),
                            attention.work_ref.namespace.clone(),
                            attention.work_ref.item_id.clone(),
                        )),
                    };
                }
                // Occupancy after the row rewrite (the probe excludes the
                // candidate itself); a conflict drops the transaction.
                if let Some(occupant) = active_target_occupant_tx(&tx, attention)? {
                    return Err(active_target_conflict(attention, &occupant));
                }
                insert_event_tx(&tx, event)?;
            }
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(item)
        })
    }

    async fn get_attention(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        binding_id: &WorkAttentionBindingId,
    ) -> Result<Option<WorkAttentionBinding>, WorkGraphError> {
        self.with_connection(|conn| select_attention(conn, realm_id, namespace, binding_id))
    }

    async fn list_attention(
        &self,
        filter: AttentionListRequest,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_attention(conn, &filter, None))
    }

    async fn list_attention_bounded(
        &self,
        filter: AttentionListRequest,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_attention(conn, &filter, Some(limit)))
    }

    async fn prune_terminal_attention(
        &self,
        filter: AttentionPruneRequest,
    ) -> Result<u64, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            // Candidate scan is NULL-tolerant (rows written by older binaries
            // carry NULL status); each candidate is decoded and judged in
            // Rust before deletion, so only provably terminal rows go.
            let candidates: Vec<(String, String, String)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT realm_id, namespace, binding_id, attention_json
                           FROM workgraph_attention
                          WHERE status IN ('superseded', 'stopped') OR status IS NULL",
                    )
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row_json::<WorkAttentionBinding>(row, 3)?,
                        ))
                    })
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let mut candidates = Vec::new();
                for row in rows {
                    let (realm_id, namespace, binding_id, binding) =
                        row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
                    let in_scope = filter
                        .realm_id
                        .as_ref()
                        .is_none_or(|realm| &binding.work_ref.realm_id == realm)
                        && filter
                            .namespace
                            .as_ref()
                            .is_none_or(|ns| &binding.work_ref.namespace == ns)
                        && filter
                            .updated_before
                            .is_none_or(|updated_before| binding.updated_at < updated_before);
                    if in_scope && binding.status.is_terminal() {
                        candidates.push((realm_id, namespace, binding_id));
                    }
                }
                candidates
            };
            let mut pruned = 0u64;
            for (realm_id, namespace, binding_id) in candidates {
                pruned += tx
                    .execute(
                        "DELETE FROM workgraph_attention
                          WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3",
                        params![realm_id, namespace, binding_id],
                    )
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?
                    as u64;
            }
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(pruned)
        })
    }

    async fn insert_edge(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            insert_edge_tx(&tx, &edge)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(edge)
        })
    }

    async fn insert_edge_validated(
        &self,
        edge: WorkEdge,
        event: WorkGraphEvent,
    ) -> Result<WorkEdge, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let existing_edges = list_sqlite_edges(&tx, &edge.realm_id, &edge.namespace, None)?;
            let existing_items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(edge.realm_id.clone()),
                    namespace: Some(edge.namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            WorkGraphMachine::validate_link(&edge, &existing_items, &existing_edges)?;
            insert_edge_tx(&tx, &edge)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(edge)
        })
    }

    async fn list_edges(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_edges(conn, realm_id, namespace, None))
    }

    async fn list_edges_bounded(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        limit: usize,
    ) -> Result<Vec<WorkEdge>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_edges(conn, realm_id, namespace, Some(limit)))
    }

    async fn list_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_events(conn, &filter))
    }

    async fn latest_event_seq(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Option<i64>, WorkGraphError> {
        self.with_connection(|conn| latest_sqlite_event_seq(conn, &filter))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn init_sqlite_schema(conn: &Connection) -> Result<(), WorkGraphError> {
    conn.execute_batch(
        r"
        CREATE TABLE IF NOT EXISTS workgraph_items (
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            item_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            updated_at_utc TEXT NOT NULL,
            item_json TEXT NOT NULL,
            PRIMARY KEY (realm_id, namespace, item_id)
        );
        CREATE INDEX IF NOT EXISTS idx_workgraph_items_realm_namespace_updated
            ON workgraph_items (realm_id, namespace, updated_at_utc);

        CREATE TABLE IF NOT EXISTS workgraph_attention (
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            binding_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            updated_at_utc TEXT NOT NULL,
            attention_json TEXT NOT NULL,
            PRIMARY KEY (realm_id, namespace, binding_id)
        );
        CREATE INDEX IF NOT EXISTS idx_workgraph_attention_realm_namespace_updated
            ON workgraph_attention (realm_id, namespace, updated_at_utc);

        CREATE TABLE IF NOT EXISTS workgraph_edges (
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            edge_kind TEXT NOT NULL,
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            edge_json TEXT NOT NULL,
            PRIMARY KEY (realm_id, namespace, edge_kind, from_id, to_id)
        );

        CREATE TABLE IF NOT EXISTS workgraph_events (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            item_id TEXT,
            event_kind TEXT NOT NULL,
            at_utc TEXT NOT NULL,
            event_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_workgraph_events_realm_namespace_seq
            ON workgraph_events (realm_id, namespace, seq);
        ",
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_item_tx(tx: &Transaction<'_>, item: &WorkItem) -> Result<(), WorkGraphError> {
    let json = serde_json::to_string(item).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_items (realm_id, namespace, item_id, revision, updated_at_utc, item_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            item.realm_id,
            item.namespace.as_str(),
            item.id.as_str(),
            item.revision,
            item.updated_at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|err| map_sqlite_insert_item_error(err, item))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn update_item_tx(
    tx: &Transaction<'_>,
    item: &WorkItem,
    expected_previous_revision: u64,
) -> Result<usize, WorkGraphError> {
    let json = serde_json::to_string(item).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "UPDATE workgraph_items
            SET revision = ?4, updated_at_utc = ?5, item_json = ?6
          WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3 AND revision = ?7",
        params![
            item.realm_id,
            item.namespace.as_str(),
            item.id.as_str(),
            item.revision,
            item.updated_at.to_rfc3339(),
            json,
            expected_previous_revision,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn upsert_item_tx(tx: &Transaction<'_>, item: &WorkItem) -> Result<(), WorkGraphError> {
    let json = serde_json::to_string(item).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_items
            (realm_id, namespace, item_id, revision, updated_at_utc, item_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(realm_id, namespace, item_id) DO UPDATE SET
            revision = excluded.revision,
            updated_at_utc = excluded.updated_at_utc,
            item_json = excluded.item_json",
        params![
            item.realm_id,
            item.namespace.as_str(),
            item.id.as_str(),
            item.revision,
            item.updated_at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn map_sqlite_insert_item_error(err: Error, item: &WorkItem) -> WorkGraphError {
    if sqlite_constraint_violation(&err) {
        return WorkGraphError::Conflict(format!("work item {} already exists", item.id));
    }
    WorkGraphError::Store(err.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn map_sqlite_insert_attention_error(
    err: Error,
    attention: &WorkAttentionBinding,
) -> WorkGraphError {
    if sqlite_constraint_violation(&err) {
        return WorkGraphError::Conflict(format!(
            "work attention binding {} already exists",
            attention.binding_id
        ));
    }
    WorkGraphError::Store(err.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn sqlite_constraint_violation(err: &Error) -> bool {
    matches!(
        err,
        Error::SqliteFailure(sqlite_error, _)
            if sqlite_error.code == ErrorCode::ConstraintViolation
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn current_revision_tx(
    tx: &Transaction<'_>,
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkItemId,
) -> Result<Option<u64>, WorkGraphError> {
    tx.query_row(
        "SELECT revision FROM workgraph_items WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
        params![realm_id, namespace.as_str(), id.as_str()],
        |row| row.get::<_, u64>(0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_attention_tx(
    tx: &Transaction<'_>,
    attention: &WorkAttentionBinding,
) -> Result<(), WorkGraphError> {
    let json =
        serde_json::to_string(attention).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_attention
            (realm_id, namespace, binding_id, revision, updated_at_utc, attention_json,
             status, target_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            attention.work_ref.realm_id,
            attention.work_ref.namespace.as_str(),
            attention.binding_id.as_str(),
            attention.machine_state.revision,
            attention.updated_at.to_rfc3339(),
            json,
            attention.status.status_key(),
            attention.target.target_key(),
        ],
    )
    .map_err(|err| map_sqlite_insert_attention_error(err, attention))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn update_attention_tx(
    tx: &Transaction<'_>,
    attention: &WorkAttentionBinding,
    expected_previous_revision: u64,
) -> Result<usize, WorkGraphError> {
    let json =
        serde_json::to_string(attention).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "UPDATE workgraph_attention
            SET revision = ?4, updated_at_utc = ?5, attention_json = ?6,
                status = ?8, target_key = ?9
          WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3 AND revision = ?7",
        params![
            attention.work_ref.realm_id,
            attention.work_ref.namespace.as_str(),
            attention.binding_id.as_str(),
            attention.machine_state.revision,
            attention.updated_at.to_rfc3339(),
            json,
            expected_previous_revision,
            attention.status.status_key(),
            attention.target.target_key(),
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

/// One-time, idempotent migration adding the indexed `status` / `target_key`
/// query columns to `workgraph_attention` (SQL filter pushdown + the
/// active-binding-per-target occupancy guard) and backfilling existing rows.
/// Rows written by OLDER binaries after this migration carry NULL columns:
/// every reader of these columns is NULL-tolerant and falls back to decoding
/// `attention_json`, so mixed-version shared stores stay correct.
#[cfg(not(target_arch = "wasm32"))]
fn migrate_sqlite_attention_query_columns(conn: &mut Connection) -> Result<(), WorkGraphError> {
    for alter in [
        "ALTER TABLE workgraph_attention ADD COLUMN status TEXT",
        "ALTER TABLE workgraph_attention ADD COLUMN target_key TEXT",
    ] {
        if let Err(err) = conn.execute(alter, []) {
            let message = err.to_string();
            if !message.contains("duplicate column name") {
                return Err(WorkGraphError::Store(message));
            }
        }
    }
    let backfill: Vec<(String, String, String, WorkAttentionBinding)> = {
        let mut stmt = conn
            .prepare(
                "SELECT realm_id, namespace, binding_id, attention_json
                   FROM workgraph_attention
                  WHERE status IS NULL OR target_key IS NULL",
            )
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row_json::<WorkAttentionBinding>(row, 3)?,
                ))
            })
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        let mut backfill = Vec::new();
        for row in rows {
            backfill.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
        }
        backfill
    };
    for (realm_id, namespace, binding_id, binding) in backfill {
        conn.execute(
            "UPDATE workgraph_attention
                SET status = ?4, target_key = ?5
              WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3",
            params![
                realm_id,
                namespace,
                binding_id,
                binding.status.status_key(),
                binding.target.target_key(),
            ],
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_workgraph_attention_scope_status
             ON workgraph_attention (realm_id, namespace, status, target_key)",
        [],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

/// Occupancy probe for the active-binding-per-target invariant, run INSIDE
/// the same immediate write transaction as the mutation it guards so the
/// check is race-free next to the data. NULL-column rows (written by older
/// binaries) are decoded from JSON before judging, so mixed-version stores
/// cannot dodge the guard.
#[cfg(not(target_arch = "wasm32"))]
fn active_target_occupant_tx(
    tx: &Transaction<'_>,
    candidate: &WorkAttentionBinding,
) -> Result<Option<WorkAttentionBindingId>, WorkGraphError> {
    if !matches!(candidate.status, WorkAttentionStatus::Active) {
        return Ok(None);
    }
    let target_key = candidate.target.target_key();
    let mut stmt = tx
        .prepare(
            "SELECT binding_id, attention_json FROM workgraph_attention
              WHERE realm_id = ?1 AND namespace = ?2 AND binding_id != ?3
                AND (status = 'active' OR status IS NULL)
                AND (target_key = ?4 OR target_key IS NULL)",
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map(
            params![
                candidate.work_ref.realm_id,
                candidate.work_ref.namespace.as_str(),
                candidate.binding_id.as_str(),
                target_key,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row_json::<WorkAttentionBinding>(row, 1)?,
                ))
            },
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    for row in rows {
        let (_, binding) = row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
        if matches!(binding.status, WorkAttentionStatus::Active)
            && binding.target.target_key() == target_key
        {
            return Ok(Some(binding.binding_id));
        }
    }
    Ok(None)
}

/// Typed conflict naming the occupant, so hosts get the invariant they were
/// building by hand (mobkit admission guards demote to defense-in-depth).
fn active_target_conflict(
    candidate: &WorkAttentionBinding,
    occupant: &WorkAttentionBindingId,
) -> WorkGraphError {
    WorkGraphError::Conflict(format!(
        "active attention binding {occupant} already targets {} in {}/{}",
        candidate.target.target_key(),
        candidate.work_ref.realm_id,
        candidate.work_ref.namespace.as_str(),
    ))
}

/// Memory-store twin of [`active_target_occupant_tx`], run under the store's
/// write lock.
fn active_target_occupant_in<'a>(
    bindings: impl Iterator<Item = &'a WorkAttentionBinding>,
    candidate: &WorkAttentionBinding,
) -> Option<WorkAttentionBindingId> {
    if !matches!(candidate.status, WorkAttentionStatus::Active) {
        return None;
    }
    let target_key = candidate.target.target_key();
    bindings
        .filter(|binding| {
            binding.binding_id != candidate.binding_id
                && binding.work_ref.realm_id == candidate.work_ref.realm_id
                && binding.work_ref.namespace == candidate.work_ref.namespace
                && matches!(binding.status, WorkAttentionStatus::Active)
                && binding.target.target_key() == target_key
        })
        .map(|binding| binding.binding_id.clone())
        .next()
}

#[cfg(not(target_arch = "wasm32"))]
fn upsert_attention_tx(
    tx: &Transaction<'_>,
    attention: &WorkAttentionBinding,
) -> Result<(), WorkGraphError> {
    let json =
        serde_json::to_string(attention).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_attention
            (realm_id, namespace, binding_id, revision, updated_at_utc, attention_json,
             status, target_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(realm_id, namespace, binding_id) DO UPDATE SET
            revision = excluded.revision,
            updated_at_utc = excluded.updated_at_utc,
            attention_json = excluded.attention_json,
            status = excluded.status,
            target_key = excluded.target_key",
        params![
            attention.work_ref.realm_id,
            attention.work_ref.namespace.as_str(),
            attention.binding_id.as_str(),
            attention.machine_state.revision,
            attention.updated_at.to_rfc3339(),
            json,
            attention.status.status_key(),
            attention.target.target_key(),
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn current_attention_revision_tx(
    tx: &Transaction<'_>,
    realm_id: &str,
    namespace: &WorkNamespace,
    binding_id: &WorkAttentionBindingId,
) -> Result<Option<u64>, WorkGraphError> {
    tx.query_row(
        "SELECT revision FROM workgraph_attention
         WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3",
        params![realm_id, namespace.as_str(), binding_id.as_str()],
        |row| row.get::<_, u64>(0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_edge_tx(tx: &Transaction<'_>, edge: &WorkEdge) -> Result<(), WorkGraphError> {
    let json = serde_json::to_string(edge).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_edges
            (realm_id, namespace, edge_kind, from_id, to_id, edge_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            edge.realm_id,
            edge.namespace.as_str(),
            format!("{:?}", edge.kind),
            edge.from_id.as_str(),
            edge.to_id.as_str(),
            json,
        ],
    )
    .map_err(|err| map_sqlite_insert_edge_error(err, edge))?;
    Ok(())
}

fn duplicate_edge_error(edge: &WorkEdge) -> WorkGraphError {
    WorkGraphError::Conflict(format!(
        "work edge {:?} {} -> {} already exists",
        edge.kind, edge.from_id, edge.to_id
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn map_sqlite_insert_edge_error(err: rusqlite::Error, edge: &WorkEdge) -> WorkGraphError {
    match err {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation =>
        {
            duplicate_edge_error(edge)
        }
        err => WorkGraphError::Store(err.to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn insert_event_tx(tx: &Transaction<'_>, event: &WorkGraphEvent) -> Result<(), WorkGraphError> {
    let json =
        serde_json::to_string(event).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    tx.execute(
        "INSERT INTO workgraph_events
            (realm_id, namespace, item_id, event_kind, at_utc, event_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.realm_id,
            event.namespace.as_str(),
            event.item_id.as_ref().map(WorkItemId::as_str),
            format!("{:?}", event.kind),
            event.at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn select_item(
    conn: &Connection,
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkItemId,
) -> Result<Option<WorkItem>, WorkGraphError> {
    conn.query_row(
        "SELECT item_json FROM workgraph_items WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
        params![realm_id, namespace.as_str(), id.as_str()],
        |row| row_json(row, 0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_items(
    conn: &Connection,
    filter: &WorkItemFilter,
) -> Result<Vec<WorkItem>, WorkGraphError> {
    let mut stmt = conn
        .prepare("SELECT item_json FROM workgraph_items ORDER BY updated_at_utc ASC, item_id ASC")
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| row_json::<WorkItem>(row, 0))
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut items = Vec::new();
    for row in rows {
        let item = row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
        if item_matches_filter(&item, filter) {
            items.push(item);
            if filter.limit.is_some_and(|limit| items.len() >= limit) {
                break;
            }
        }
    }
    Ok(items)
}

#[cfg(not(target_arch = "wasm32"))]
fn select_attention(
    conn: &Connection,
    realm_id: &str,
    namespace: &WorkNamespace,
    binding_id: &WorkAttentionBindingId,
) -> Result<Option<WorkAttentionBinding>, WorkGraphError> {
    conn.query_row(
        "SELECT attention_json FROM workgraph_attention
         WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3",
        params![realm_id, namespace.as_str(), binding_id.as_str()],
        |row| row_json(row, 0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_attention(
    conn: &Connection,
    filter: &AttentionListRequest,
    limit: Option<usize>,
) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
    if limit == Some(0) {
        return Ok(Vec::new());
    }
    // SQL filter pushdown over the indexed query columns. Every predicate is
    // NULL-tolerant: rows written by older binaries carry NULL status /
    // target_key and must still reach the Rust-side filter, which remains the
    // final authority over every returned row.
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(realm_id) = &filter.realm_id {
        params.push(Box::new(realm_id.clone()));
        clauses.push(format!("realm_id = ?{}", params.len()));
    }
    if let Some(namespace) = &filter.namespace {
        params.push(Box::new(namespace.as_str().to_string()));
        clauses.push(format!("namespace = ?{}", params.len()));
    }
    if let Some(status) = &filter.status {
        params.push(Box::new(status.status_key().to_string()));
        clauses.push(format!("(status = ?{} OR status IS NULL)", params.len()));
    }
    if let Some(target) = &filter.target {
        params.push(Box::new(target.target_key()));
        clauses.push(format!(
            "(target_key = ?{} OR target_key IS NULL)",
            params.len()
        ));
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT attention_json FROM workgraph_attention{where_clause}
         ORDER BY updated_at_utc ASC, binding_id ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row_json::<WorkAttentionBinding>(row, 0)
        })
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut bindings = Vec::new();
    for row in rows {
        let binding = row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
        if attention_matches_filter(&binding, filter) {
            bindings.push(binding);
            if limit.is_some_and(|limit| bindings.len() >= limit) {
                break;
            }
        }
    }
    Ok(bindings)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_edges(
    conn: &Connection,
    realm_id: &str,
    namespace: &WorkNamespace,
    limit: Option<usize>,
) -> Result<Vec<WorkEdge>, WorkGraphError> {
    if limit == Some(0) {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT edge_json FROM workgraph_edges
             WHERE realm_id = ?1 AND namespace = ?2
             ORDER BY edge_kind ASC, from_id ASC, to_id ASC",
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map(params![realm_id, namespace.as_str()], |row| {
            row_json::<WorkEdge>(row, 0)
        })
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
        if limit.is_some_and(|limit| edges.len() >= limit) {
            break;
        }
    }
    Ok(edges)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_events(
    conn: &Connection,
    filter: &WorkGraphEventFilter,
) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
    let mut stmt = conn
        .prepare("SELECT seq, event_json FROM workgraph_events ORDER BY seq ASC")
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            let seq = row.get::<_, i64>(0)?;
            let mut event = row_json::<WorkGraphEvent>(row, 1)?;
            event.seq = Some(seq);
            Ok(event)
        })
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut events = Vec::new();
    for row in rows {
        let event = row.map_err(|err| WorkGraphError::Store(err.to_string()))?;
        if event_matches_filter(&event, filter) {
            events.push(event);
            if filter.limit.is_some_and(|limit| events.len() >= limit) {
                break;
            }
        }
    }
    Ok(events)
}

#[cfg(not(target_arch = "wasm32"))]
fn latest_sqlite_event_seq(
    conn: &Connection,
    filter: &WorkGraphEventFilter,
) -> Result<Option<i64>, WorkGraphError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(realm_id) = &filter.realm_id {
        params.push(Box::new(realm_id.clone()));
        clauses.push(format!("realm_id = ?{}", params.len()));
    }
    if !filter.all_namespaces
        && let Some(namespace) = &filter.namespace
    {
        params.push(Box::new(namespace.as_str().to_string()));
        clauses.push(format!("namespace = ?{}", params.len()));
    }
    if let Some(after_seq) = filter.after_seq {
        params.push(Box::new(after_seq));
        clauses.push(format!("seq > ?{}", params.len()));
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    conn.query_row(
        &format!("SELECT MAX(seq) FROM workgraph_events{where_clause}"),
        rusqlite::params_from_iter(params.iter()),
        |row| row.get::<_, Option<i64>>(0),
    )
    .map_err(|error| WorkGraphError::Store(error.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn replay_event_tx(tx: &Transaction<'_>, event: &WorkGraphEvent) -> Result<(), WorkGraphError> {
    match event.kind {
        WorkGraphEventKind::Linked => {
            let edge = payload_field::<WorkEdge>(event, "edge")?;
            insert_edge_tx(tx, &edge)
        }
        WorkGraphEventKind::AttentionCreated | WorkGraphEventKind::AttentionUpdated => {
            let attention = payload_field::<WorkAttentionBinding>(event, "attention")?;
            upsert_attention_tx(tx, &attention)
        }
        WorkGraphEventKind::Created
        | WorkGraphEventKind::Updated
        | WorkGraphEventKind::Claimed
        | WorkGraphEventKind::Released
        | WorkGraphEventKind::Blocked
        | WorkGraphEventKind::Closed
        | WorkGraphEventKind::EvidenceAdded => {
            let item = payload_field::<WorkItem>(event, "item")?;
            upsert_item_tx(tx, &item)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn normalize_attention_for_terminal_items_tx(tx: &Transaction<'_>) -> Result<(), WorkGraphError> {
    let bindings = {
        let mut stmt = tx
            .prepare("SELECT attention_json FROM workgraph_attention")
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        let rows = stmt
            .query_map([], |row| row_json::<WorkAttentionBinding>(row, 0))
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        let mut bindings = Vec::new();
        for row in rows {
            bindings.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
        }
        bindings
    };

    for binding in bindings {
        if matches!(
            binding.status,
            WorkAttentionStatus::Stopped | WorkAttentionStatus::Superseded
        ) {
            continue;
        }
        let item = tx
            .query_row(
                "SELECT item_json FROM workgraph_items
                 WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
                params![
                    binding.work_ref.realm_id,
                    binding.work_ref.namespace.as_str(),
                    binding.work_ref.item_id.as_str(),
                ],
                |row| row_json::<WorkItem>(row, 0),
            )
            .optional()
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        let Some(item) = item else {
            continue;
        };
        // Terminality is a WorkGraph machine fact: the shell mirrors the
        // canonical classify verdict rather than re-deciding `is_terminal()`.
        if WorkGraphMachine::classify_terminality(&item)? {
            let expected_revision = binding.machine_state.revision;
            let stopped = WorkAttentionMachine::stop(binding, expected_revision, item.updated_at)?;
            upsert_attention_tx(tx, &stopped)?;
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn payload_field<T: serde::de::DeserializeOwned>(
    event: &WorkGraphEvent,
    field: &str,
) -> Result<T, WorkGraphError> {
    let value = event.payload.get(field).ok_or_else(|| {
        WorkGraphError::Store(format!(
            "workgraph event {:?} missing payload field `{field}`",
            event.kind
        ))
    })?;
    serde_json::from_value(value.clone()).map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn row_json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let json = row.get::<_, String>(index)?;
    serde_json::from_str(&json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(err))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::Utc;
    use serde_json::json;

    use crate::types::WorkEdge;
    use crate::{
        AttentionDelegatedAuthority, AttentionProjectionPolicy, CreateWorkItemRequest,
        GoalAttentionTarget, GoalCreateRequest, GoalRequestCloseRequest, GoalTerminalStatus,
        LinkWorkItemsRequest, MemoryWorkGraphStore, WorkAttentionMode, WorkAttentionStatus,
        WorkCompletionPolicy, WorkEdgeKind, WorkGraphError, WorkGraphEvent, WorkGraphEventFilter,
        WorkGraphEventKind, WorkGraphService, WorkGraphStore, WorkItemFilter, WorkItemId,
        WorkNamespace,
    };

    fn test_edge() -> WorkEdge {
        WorkEdge {
            realm_id: "realm".to_string(),
            namespace: WorkNamespace::default(),
            kind: WorkEdgeKind::Blocks,
            from_id: WorkItemId::generated(),
            to_id: WorkItemId::generated(),
            created_at: Utc::now(),
        }
    }

    fn link_event(edge: &WorkEdge) -> WorkGraphEvent {
        WorkGraphEvent::graph(
            edge.realm_id.clone(),
            edge.namespace.clone(),
            WorkGraphEventKind::Linked,
            edge.created_at,
            json!({ "edge": edge }),
        )
    }

    #[tokio::test]
    async fn memory_store_namespace_filters_do_not_leak() {
        let store = std::sync::Arc::new(MemoryWorkGraphStore::new());
        let default_service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let other_service = WorkGraphService::with_scope(
            store.clone(),
            "realm",
            WorkNamespace::new("other").expect("namespace"),
        );
        default_service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "default".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create default");
        other_service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "other".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create other");

        let items = store
            .list_items(WorkItemFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(WorkNamespace::default()),
                ..WorkItemFilter::default()
            })
            .await
            .expect("list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "default");
    }

    #[tokio::test]
    async fn memory_store_duplicate_edge_does_not_append_event() {
        let store = MemoryWorkGraphStore::new();
        let edge = test_edge();
        store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect("insert edge");

        let error = store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect_err("duplicate edge should fail");
        assert!(matches!(error, WorkGraphError::Conflict(_)));

        let events = store
            .list_events(WorkGraphEventFilter {
                realm_id: Some(edge.realm_id),
                namespace: Some(edge.namespace),
                all_namespaces: false,
                after_seq: None,
                limit: None,
            })
            .await
            .expect("events");
        assert_eq!(events.len(), 1);
    }

    /// Pins the SQLite UNIQUE-violation mapping for duplicate item inserts:
    /// a second insert of an existing item id must surface as the typed
    /// `Conflict`, not a generic `Store` error.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_store_duplicate_item_insert_maps_to_conflict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let item = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "unique item".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create");

        let event = WorkGraphEvent::graph(
            item.realm_id.clone(),
            item.namespace.clone(),
            WorkGraphEventKind::Created,
            item.created_at,
            json!({ "item_id": item.id }),
        );
        let error = store
            .insert_item(item, event)
            .await
            .expect_err("duplicate item insert must fail");
        assert!(
            matches!(error, WorkGraphError::Conflict(_)),
            "duplicate item insert must map to Conflict, got: {error:?}"
        );
    }

    /// Pins the SQLite UNIQUE-violation mapping for duplicate attention
    /// binding inserts (via the compound goal insert): the typed `Conflict`,
    /// not a generic `Store` error.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_store_duplicate_attention_insert_maps_to_conflict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let goal = service
            .create_goal(GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "unique goal".to_string(),
                description: None,
                target: GoalAttentionTarget::Session {
                    session_id: meerkat_core::SessionId::new(),
                },
                mode: WorkAttentionMode::Coordinate,
                completion_policy: WorkCompletionPolicy::SelfAttest,
                delegated_authority: AttentionDelegatedAuthority::AddEvidence,
                projection_policy: AttentionProjectionPolicy::default(),
            })
            .await
            .expect("create goal");

        let mut fresh_item = goal.item.clone();
        fresh_item.id = WorkItemId::generated();
        let item_event = WorkGraphEvent::graph(
            fresh_item.realm_id.clone(),
            fresh_item.namespace.clone(),
            WorkGraphEventKind::Created,
            fresh_item.created_at,
            json!({ "item_id": fresh_item.id }),
        );
        let attention_event = WorkGraphEvent::graph(
            goal.attention.work_ref.realm_id.clone(),
            goal.attention.work_ref.namespace.clone(),
            WorkGraphEventKind::AttentionCreated,
            goal.attention.updated_at,
            json!({ "binding_id": goal.attention.binding_id }),
        );
        let error = store
            .insert_goal(fresh_item, item_event, goal.attention, attention_event)
            .await
            .expect_err("duplicate attention insert must fail");
        assert!(
            matches!(error, WorkGraphError::Conflict(_)),
            "duplicate attention insert must map to Conflict, got: {error:?}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_persistence_survives_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service = WorkGraphService::with_scope(store, "realm", WorkNamespace::default());
        let item = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "persist me".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create");

        let reopened = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service = WorkGraphService::with_scope(reopened, "realm", WorkNamespace::default());
        let fetched = service.get(None, None, item.id.clone()).await.expect("get");
        assert_eq!(fetched.title, "persist me");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_item_without_machine_state_fails_closed_on_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let item = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "legacy item".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create");

        store
            .with_connection(|conn| {
                let json: String = conn
                    .query_row(
                        "SELECT item_json FROM workgraph_items
                         WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
                        rusqlite::params![
                            &item.realm_id,
                            item.namespace.as_str(),
                            item.id.as_str()
                        ],
                        |row| row.get(0),
                    )
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                let mut value = serde_json::from_str::<serde_json::Value>(&json)
                    .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                value
                    .as_object_mut()
                    .expect("item json object")
                    .remove("machine_state");
                conn.execute(
                    "UPDATE workgraph_items
                        SET item_json = ?4
                      WHERE realm_id = ?1 AND namespace = ?2 AND item_id = ?3",
                    rusqlite::params![
                        &item.realm_id,
                        item.namespace.as_str(),
                        item.id.as_str(),
                        serde_json::to_string(&value)
                            .map_err(|err| WorkGraphError::Store(err.to_string()))?
                    ],
                )
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
                Ok(())
            })
            .expect("strip machine state");

        // machine_state is the sole machine-owned lifecycle/revision authority.
        // A persisted item missing it can no longer be backfilled from projected
        // fields (that fabrication path was deleted); reading it must FAIL CLOSED
        // with a typed error rather than reconstructing machine truth.
        let reopened = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service = WorkGraphService::with_scope(reopened, "realm", WorkNamespace::default());
        let err = service
            .get(None, None, item.id)
            .await
            .expect_err("reading an item with no machine_state must fail closed");
        assert!(
            matches!(err, WorkGraphError::Store(_)),
            "expected a typed Store deserialization error, got: {err:?}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_event_replay_rebuilds_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let blocker = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "blocker".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create blocker");
        let blocked = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "blocked".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("create blocked");
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

        store
            .with_connection(|conn| {
                conn.execute("DELETE FROM workgraph_items", [])
                    .map_err(|err| crate::WorkGraphError::Store(err.to_string()))?;
                conn.execute("DELETE FROM workgraph_edges", [])
                    .map_err(|err| crate::WorkGraphError::Store(err.to_string()))?;
                Ok(())
            })
            .expect("clear projection");

        let empty_items = store
            .list_items(WorkItemFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(WorkNamespace::default()),
                ..WorkItemFilter::default()
            })
            .await
            .expect("empty list");
        assert!(empty_items.is_empty());

        store
            .rebuild_projection_from_events()
            .expect("rebuild projection");

        let rebuilt_items = store
            .list_items(WorkItemFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(WorkNamespace::default()),
                ..WorkItemFilter::default()
            })
            .await
            .expect("rebuilt list");
        assert_eq!(rebuilt_items.len(), 2);
        let rebuilt_edges = store
            .list_edges("realm", &WorkNamespace::default())
            .await
            .expect("rebuilt edges");
        assert_eq!(rebuilt_edges.len(), 1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_event_replay_stops_attention_for_terminal_goal_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let session_id = meerkat_core::SessionId::parse("019e63c2-0000-7000-8000-000000000045")
            .expect("session id");
        let goal = service
            .create_goal(GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "terminal goal".to_string(),
                description: None,
                target: GoalAttentionTarget::Session { session_id },
                mode: WorkAttentionMode::Pursue,
                completion_policy: WorkCompletionPolicy::SelfAttest,
                delegated_authority: AttentionDelegatedAuthority::CloseIfPolicyAllows,
                projection_policy: AttentionProjectionPolicy::default(),
            })
            .await
            .expect("create goal");
        service
            .goal_request_close(GoalRequestCloseRequest {
                binding_id: goal.attention.binding_id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: goal.item.revision,
                status: GoalTerminalStatus::Completed,
            })
            .await
            .expect("close goal");

        store
            .with_connection(|conn| {
                conn.execute("DELETE FROM workgraph_items", [])
                    .map_err(|err| crate::WorkGraphError::Store(err.to_string()))?;
                conn.execute("DELETE FROM workgraph_attention", [])
                    .map_err(|err| crate::WorkGraphError::Store(err.to_string()))?;
                Ok(())
            })
            .expect("clear projection");

        store
            .rebuild_projection_from_events()
            .expect("rebuild projection");

        let binding = store
            .get_attention(
                "realm",
                &WorkNamespace::default(),
                &goal.attention.binding_id,
            )
            .await
            .expect("read binding")
            .expect("rebuilt binding");
        assert_eq!(binding.status, WorkAttentionStatus::Stopped);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_store_duplicate_edge_does_not_append_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let store = crate::SqliteWorkGraphStore::open(&path).expect("open");
        let edge = test_edge();
        store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect("insert edge");

        let error = store
            .insert_edge(edge.clone(), link_event(&edge))
            .await
            .expect_err("duplicate edge should fail");
        assert!(matches!(error, WorkGraphError::Conflict(_)));

        let events = store
            .list_events(WorkGraphEventFilter {
                realm_id: Some(edge.realm_id),
                namespace: Some(edge.namespace),
                all_namespaces: false,
                after_seq: None,
                limit: None,
            })
            .await
            .expect("events");
        assert_eq!(events.len(), 1);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod legacy_schema_tests {
    use super::*;
    use crate::{
        AttentionDelegatedAuthority, AttentionProjectionPolicy, GoalAttentionTarget,
        GoalCreateRequest, WorkAttentionMode, WorkCompletionPolicy, WorkGraphService,
    };
    use meerkat_core::SessionId;

    /// Ask 24/25 migration pin: a store created by an OLDER binary (no
    /// status/target_key columns) is backfilled on open, and both the SQL
    /// filter pushdown and the occupancy guard see its legacy rows.
    #[tokio::test]
    async fn legacy_attention_rows_are_backfilled_and_guarded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let session_id = SessionId::new();

        // Simulate the old binary: old-schema table + one active binding row
        // written without the query columns.
        {
            let conn = Connection::open(&path).expect("open raw");
            conn.execute_batch(
                r"
                CREATE TABLE workgraph_attention (
                    realm_id TEXT NOT NULL,
                    namespace TEXT NOT NULL,
                    binding_id TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    updated_at_utc TEXT NOT NULL,
                    attention_json TEXT NOT NULL,
                    PRIMARY KEY (realm_id, namespace, binding_id)
                );
                ",
            )
            .expect("create legacy table");
            let legacy = WorkAttentionBinding {
                binding_id: WorkAttentionBindingId::new("legacy-binding").expect("binding id"),
                work_ref: crate::WorkItemRef {
                    realm_id: "realm".to_string(),
                    namespace: WorkNamespace::default(),
                    item_id: WorkItemId::generated(),
                },
                target: crate::WorkAttentionTarget::Session {
                    session_id: session_id.clone(),
                },
                mode: WorkAttentionMode::Pursue,
                status: WorkAttentionStatus::Active,
                machine_state: Default::default(),
                delegated_authority: AttentionDelegatedAuthority::AddEvidence,
                projection_policy: AttentionProjectionPolicy::default(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            conn.execute(
                "INSERT INTO workgraph_attention
                    (realm_id, namespace, binding_id, revision, updated_at_utc, attention_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    legacy.work_ref.realm_id,
                    legacy.work_ref.namespace.as_str(),
                    legacy.binding_id.as_str(),
                    legacy.machine_state.revision,
                    legacy.updated_at.to_rfc3339(),
                    serde_json::to_string(&legacy).expect("serialize legacy binding"),
                ],
            )
            .expect("insert legacy row");
        }

        // Opening the store migrates + backfills.
        let store = std::sync::Arc::new(crate::SqliteWorkGraphStore::open(&path).expect("open"));
        {
            let conn = Connection::open(&path).expect("reopen raw");
            let (status, target_key): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT status, target_key FROM workgraph_attention
                      WHERE binding_id = 'legacy-binding'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read backfilled columns");
            assert_eq!(status.as_deref(), Some("active"));
            assert_eq!(
                target_key.as_deref(),
                Some(format!("session:{session_id}").as_str())
            );
        }

        // The occupancy guard sees the backfilled legacy row: a new active
        // binding on the same target conflicts.
        let service = WorkGraphService::with_scope(store, "realm", WorkNamespace::default());
        let error = service
            .create_goal(GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "duplicate target".to_string(),
                description: None,
                target: GoalAttentionTarget::Session {
                    session_id: session_id.clone(),
                },
                mode: WorkAttentionMode::Pursue,
                completion_policy: WorkCompletionPolicy::SelfAttest,
                delegated_authority: AttentionDelegatedAuthority::AddEvidence,
                projection_policy: AttentionProjectionPolicy::default(),
            })
            .await
            .expect_err("legacy occupant must conflict with a new active binding");
        assert!(
            matches!(error, WorkGraphError::Conflict(_)),
            "expected typed Conflict, got {error:?}"
        );
    }
}

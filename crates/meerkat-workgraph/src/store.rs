use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(not(target_arch = "wasm32"))]
use rusqlite::{
    Connection, Error, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    params,
};

use crate::WorkGraphError;
use crate::types::{
    AttentionListRequest, AttentionPruneRequest, ClaimWorkItemRequest, ObserveReadinessRequest,
    WorkAttentionBinding, WorkAttentionBindingId, WorkAttentionStatus, WorkEdge,
    WorkExecutionBinding, WorkExecutionBindingFilter, WorkExecutionBindingId, WorkGraphEvent,
    WorkGraphEventKind, WorkGraphFact, WorkItem, WorkItemFilter, WorkItemId, WorkNamespace,
};
use crate::{ChildJoinDisposition, WorkAttentionMachine, WorkGraphMachine};

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

/// One exact namespace snapshot captured under a single store read boundary.
#[derive(Debug, Clone)]
pub struct WorkGraphNamespaceRead {
    pub captured_at: DateTime<Utc>,
    pub event_high_water_mark: Option<i64>,
    pub items: Vec<WorkItem>,
    pub edges: Vec<WorkEdge>,
    pub attention: Vec<WorkAttentionBinding>,
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

    /// Atomically evaluate the current blocker/child graph and admit a claim.
    /// `Ok(None)` means a failed/cancelled child policy must be reconciled by
    /// the service before the caller retries; no claim mutation was committed.
    async fn claim_item_atomically(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _request: ClaimWorkItemRequest,
        _observed_at: DateTime<Utc>,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    /// Atomically evaluate the current blocker/child graph and record the
    /// Schedule-owned readiness observation. WorkGraph owns no clock or loop.
    async fn observe_readiness_atomically(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _request: ObserveReadinessRequest,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    /// Atomically reconcile one parent's failed/cancelled child policy against
    /// the current graph. A terminal transition and attention shutdown commit
    /// together; `None` means no propagation is currently authorized.
    async fn reconcile_child_join_atomically(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _parent_id: &WorkItemId,
        _expected_revision: u64,
        _observed_at: DateTime<Utc>,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    /// Atomically update one item and its attention bindings under one store
    /// mutation boundary.
    ///
    /// This is a primitive, not a composition hook. Implementations that use
    /// per-key locks must acquire the complete item-plus-attention key set in
    /// one deterministic order, validate every expected revision, and apply
    /// every mutation inline while those guards are held. They must not call
    /// [`WorkGraphStore::update_item_cas`] (or another public method that
    /// reacquires any key in that set) from inside the boundary. Async mutexes
    /// are not reentrant, so the otherwise natural acquire-then-delegate shape
    /// silently self-deadlocks instead of returning a store error.
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

    /// Read one namespace's items, edges, and observation time from one store
    /// snapshot. This is observational only and grants no claim authority.
    async fn read_namespace_graph(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
    ) -> Result<(DateTime<Utc>, Vec<WorkItem>, Vec<WorkEdge>), WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn read_namespace_snapshot(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
    ) -> Result<WorkGraphNamespaceRead, WorkGraphError> {
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

    /// Insert attention for an existing item after proving its revision and
    /// nonterminal lifecycle in the same transaction.
    async fn insert_attention_for_existing_item(
        &self,
        _attention: WorkAttentionBinding,
        _expected_item_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
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

    /// Insert one immutable execution binding after proving the referenced
    /// WorkGraph item revision and retry-chain predecessor in the same store
    /// transaction.
    async fn insert_execution_binding(
        &self,
        _commit: crate::WorkExecutionBindCommit,
        _expected_item_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkExecutionBinding, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn get_execution_binding(
        &self,
        _realm_id: &str,
        _namespace: &WorkNamespace,
        _binding_id: &WorkExecutionBindingId,
    ) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    /// Resolve the unique execution binding for one target run in a realm.
    /// This powers reverse linkage from Flow status without scanning a
    /// bounded public binding list.
    async fn get_execution_binding_by_target_run(
        &self,
        _realm_id: &str,
        _run_id: &str,
    ) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn update_execution_binding_cas(
        &self,
        _commit: crate::WorkExecutionObservationCommit,
        _expected_previous_revision: u64,
        _event: WorkGraphEvent,
    ) -> Result<WorkExecutionBinding, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    async fn list_execution_bindings(
        &self,
        _filter: WorkExecutionBindingFilter,
    ) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
        Err(unsupported(self.kind()))
    }

    /// Enumerate only nonterminal execution obligations for host recovery.
    /// Shipping stores override this with an active-queue projection so the
    /// hot recovery path never scans historical terminal bindings.
    async fn list_execution_bindings_for_recovery(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
        let mut bindings = self
            .list_execution_bindings(WorkExecutionBindingFilter {
                realm_id: Some(realm_id.to_string()),
                namespace: Some(namespace.clone()),
                item_id: None,
                current_only: true,
                limit: None,
            })
            .await?;
        let mut active = Vec::with_capacity(bindings.len());
        for binding in bindings.drain(..) {
            if !crate::WorkExecutionMachine::retry_eligible(&binding)? {
                active.push(binding);
            }
        }
        Ok(active)
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

    /// Return at most `limit` rows after applying the public effective-status
    /// contract at `observed_at`.
    ///
    /// This is required because `Active` includes deadline-elapsed Paused rows,
    /// while pending Paused rows do not match. Applying a storage limit before
    /// that machine-owned classification lets terminal history crowd every live
    /// row out of the bounded result. Durable backends must push coarse phase
    /// and scope predicates into their query, then apply the exact machine
    /// classifier before counting a row toward `limit`. Results are ordered by
    /// `updated_at`, then `binding_id`, matching the ordinary list contract.
    async fn list_attention_matching_bounded(
        &self,
        filter: AttentionListRequest,
        observed_at: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError>;

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

    /// Return a bounded public event page while omitting internal execution
    /// lifecycle events before applying the caller's visible limit.
    async fn list_public_events(
        &self,
        mut filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        let visible_limit = filter.limit.unwrap_or(usize::MAX);
        if visible_limit == 0 {
            return Ok(Vec::new());
        }
        // Custom stores get a single bounded read by default. Built-in stores
        // override this method so visibility is filtered before applying the
        // caller's limit.
        filter.limit = Some(visible_limit);
        Ok(self
            .list_events(filter)
            .await?
            .into_iter()
            .filter(|event| !is_internal_execution_event(event.kind))
            .collect())
    }

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

    async fn list_attention_matching_bounded(
        &self,
        _filter: AttentionListRequest,
        _observed_at: DateTime<Utc>,
        _limit: usize,
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
    execution_bindings:
        BTreeMap<(String, WorkNamespace, WorkExecutionBindingId), WorkExecutionBinding>,
    execution_recovery: std::collections::BTreeSet<(String, WorkNamespace, WorkExecutionBindingId)>,
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
        mut item: WorkItem,
        mut event: WorkGraphEvent,
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
        enrich_item_transition_facts(
            None,
            &mut item,
            guard.items.values(),
            guard.edges.iter(),
            &mut event,
        )?;
        guard.items.insert(key, item.clone());
        guard.append_event(event);
        Ok(item)
    }

    async fn update_item_cas(
        &self,
        mut item: WorkItem,
        expected_previous_revision: u64,
        mut event: WorkGraphEvent,
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
        let previous = current.clone();
        enrich_item_transition_facts(
            Some(&previous),
            &mut item,
            guard.items.values(),
            guard.edges.iter(),
            &mut event,
        )?;
        guard.items.insert(key, item.clone());
        guard.append_event(event);
        Ok(item)
    }

    async fn claim_item_atomically(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        mut request: ClaimWorkItemRequest,
        observed_at: DateTime<Utc>,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        let mut guard = self.inner.write().await;
        let key = item_key(realm_id, namespace, &request.id);
        let previous = guard.items.get(&key).cloned().ok_or_else(|| {
            WorkGraphError::not_found(realm_id.to_string(), namespace.clone(), request.id.clone())
        })?;
        if request.expected_revision != previous.revision {
            return Err(WorkGraphError::StaleRevision {
                id: previous.id.clone(),
                expected: request.expected_revision,
                actual: previous.revision,
            });
        }
        WorkGraphMachine::validate_claim_request(&request, &observed_at)?;
        let items = guard
            .items
            .values()
            .filter(|item| item.realm_id == realm_id && &item.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        let edges = guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && &edge.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        let disposition = child_join_disposition_with_graph(&previous, &items, &edges)?;
        if matches!(
            disposition,
            ChildJoinDisposition::PropagateFailure | ChildJoinDisposition::PropagateCancellation
        ) {
            return Ok(None);
        }
        let unresolved = unresolved_blocker_count_with_graph(&previous, &items, &edges)?;
        let (admission_item, refresh_event) =
            match WorkGraphMachine::refresh_eligibility(previous.clone(), unresolved, observed_at)?
            {
                Some((mut refreshed, mut event)) => {
                    enrich_item_transition_facts(
                        Some(&previous),
                        &mut refreshed,
                        items.iter(),
                        edges.iter(),
                        &mut event,
                    )?;
                    (refreshed, Some(event))
                }
                None => (previous.clone(), None),
            };
        request.expected_revision = admission_item.revision;
        let (mut item, mut event) = WorkGraphMachine::claim_item_with_unresolved_blockers(
            admission_item.clone(),
            unresolved,
            matches!(disposition, ChildJoinDisposition::Satisfied),
            request,
            observed_at,
        )?;
        enrich_item_transition_facts(
            Some(&admission_item),
            &mut item,
            items.iter(),
            edges.iter(),
            &mut event,
        )?;
        guard.items.insert(key, item.clone());
        if let Some(refresh_event) = refresh_event {
            guard.append_event(refresh_event);
        }
        guard.append_event(event);
        Ok(Some(item))
    }

    async fn observe_readiness_atomically(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        mut request: ObserveReadinessRequest,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        let mut guard = self.inner.write().await;
        let key = item_key(realm_id, namespace, &request.id);
        let previous = guard.items.get(&key).cloned().ok_or_else(|| {
            WorkGraphError::not_found(realm_id.to_string(), namespace.clone(), request.id.clone())
        })?;
        if request.expected_revision != previous.revision {
            return Err(WorkGraphError::StaleRevision {
                id: previous.id.clone(),
                expected: request.expected_revision,
                actual: previous.revision,
            });
        }
        let items = guard
            .items
            .values()
            .filter(|item| item.realm_id == realm_id && &item.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        let edges = guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && &edge.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        let disposition = child_join_disposition_with_graph(&previous, &items, &edges)?;
        if matches!(
            disposition,
            ChildJoinDisposition::PropagateFailure | ChildJoinDisposition::PropagateCancellation
        ) {
            return Ok(None);
        }
        let unresolved = unresolved_blocker_count_with_graph(&previous, &items, &edges)?;
        let (admission_item, refresh_event) = match WorkGraphMachine::refresh_eligibility(
            previous.clone(),
            unresolved,
            request.observed_at,
        )? {
            Some((mut refreshed, mut event)) => {
                enrich_item_transition_facts(
                    Some(&previous),
                    &mut refreshed,
                    items.iter(),
                    edges.iter(),
                    &mut event,
                )?;
                (refreshed, Some(event))
            }
            None => (previous.clone(), None),
        };
        request.expected_revision = admission_item.revision;
        let (mut item, mut event) = WorkGraphMachine::observe_readiness(
            admission_item.clone(),
            request,
            unresolved,
            matches!(disposition, ChildJoinDisposition::Satisfied),
        )?;
        enrich_item_transition_facts(
            Some(&admission_item),
            &mut item,
            items.iter(),
            edges.iter(),
            &mut event,
        )?;
        guard.items.insert(key, item.clone());
        if let Some(refresh_event) = refresh_event {
            guard.append_event(refresh_event);
        }
        guard.append_event(event);
        Ok(Some(item))
    }

    async fn reconcile_child_join_atomically(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        parent_id: &WorkItemId,
        expected_revision: u64,
        observed_at: DateTime<Utc>,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        let mut guard = self.inner.write().await;
        let key = item_key(realm_id, namespace, parent_id);
        let Some(previous) = guard.items.get(&key).cloned() else {
            return Ok(None);
        };
        if previous.revision != expected_revision {
            return Err(WorkGraphError::StaleRevision {
                id: previous.id.clone(),
                expected: expected_revision,
                actual: previous.revision,
            });
        }
        if WorkGraphMachine::classify_terminality(&previous)? {
            return Ok(Some(previous));
        }
        let items = guard
            .items
            .values()
            .filter(|item| item.realm_id == realm_id && &item.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        let edges = guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && &edge.namespace == namespace)
            .cloned()
            .collect::<Vec<_>>();
        let status = match child_join_disposition_with_graph(&previous, &items, &edges)? {
            ChildJoinDisposition::PropagateFailure => crate::types::WorkStatus::Failed,
            ChildJoinDisposition::PropagateCancellation => crate::types::WorkStatus::Cancelled,
            ChildJoinDisposition::Waiting | ChildJoinDisposition::Satisfied => return Ok(None),
        };
        let (mut terminal, mut item_event) = WorkGraphMachine::close_item(
            previous.clone(),
            crate::types::CloseWorkItemRequest {
                id: previous.id.clone(),
                realm_id: Some(realm_id.to_string()),
                namespace: Some(namespace.clone()),
                expected_revision: previous.revision,
                status,
            },
            observed_at,
        )?;
        enrich_item_transition_facts(
            Some(&previous),
            &mut terminal,
            items.iter(),
            edges.iter(),
            &mut item_event,
        )?;
        let active_attention = guard
            .attention
            .values()
            .filter(|binding| {
                binding.work_ref.realm_id == realm_id
                    && &binding.work_ref.namespace == namespace
                    && &binding.work_ref.item_id == parent_id
                    && !matches!(
                        binding.status,
                        WorkAttentionStatus::Stopped | WorkAttentionStatus::Superseded
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut stopped_attention = Vec::with_capacity(active_attention.len());
        for binding in active_attention {
            let stopped = WorkAttentionMachine::stop(
                binding.clone(),
                binding.machine_state.revision,
                observed_at,
            )?;
            let event = attention_transition_event(&stopped, observed_at);
            stopped_attention.push((stopped, event));
        }
        guard.items.insert(key, terminal.clone());
        guard.append_event(item_event);
        for (binding, event) in stopped_attention {
            let key = attention_key(
                &binding.work_ref.realm_id,
                &binding.work_ref.namespace,
                &binding.binding_id,
            );
            guard.attention.insert(key, binding);
            guard.append_event(event);
        }
        Ok(Some(terminal))
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

    async fn read_namespace_graph(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<(DateTime<Utc>, Vec<WorkItem>, Vec<WorkEdge>), WorkGraphError> {
        let guard = self.inner.read().await;
        let observed_at = Utc::now();
        let items = guard
            .items
            .values()
            .filter(|item| item.realm_id == realm_id && &item.namespace == namespace)
            .cloned()
            .collect();
        let edges = guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && &edge.namespace == namespace)
            .cloned()
            .collect();
        Ok((observed_at, items, edges))
    }

    async fn read_namespace_snapshot(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<WorkGraphNamespaceRead, WorkGraphError> {
        let guard = self.inner.read().await;
        let captured_at = Utc::now();
        let items = guard
            .items
            .values()
            .filter(|item| item.realm_id == realm_id && &item.namespace == namespace)
            .cloned()
            .collect();
        let edges = guard
            .edges
            .iter()
            .filter(|edge| edge.realm_id == realm_id && &edge.namespace == namespace)
            .cloned()
            .collect();
        let attention = guard
            .attention
            .values()
            .filter(|binding| {
                binding.work_ref.realm_id == realm_id && &binding.work_ref.namespace == namespace
            })
            .cloned()
            .collect();
        let event_high_water_mark = guard
            .events
            .iter()
            .filter(|event| event.realm_id == realm_id && &event.namespace == namespace)
            .filter_map(|event| event.seq)
            .max();
        Ok(WorkGraphNamespaceRead {
            captured_at,
            event_high_water_mark,
            items,
            edges,
            attention,
        })
    }

    async fn insert_execution_binding(
        &self,
        commit: crate::WorkExecutionBindCommit,
        expected_item_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkExecutionBinding, WorkGraphError> {
        let (binding, effect) = commit.into_parts();
        binding.validate()?;
        crate::WorkExecutionMachine::validate_projection(&binding)?;
        let (expected_state, expected_effect) =
            crate::WorkExecutionMachine::bind(&binding.binding_id, binding.target.run_id())?;
        if binding.machine_state != expected_state || effect != expected_effect {
            return Err(WorkGraphError::InvalidInput(format!(
                "work execution binding {} lacks canonical bind authority",
                binding.binding_id
            )));
        }
        let mut guard = self.inner.write().await;
        let key = execution_binding_key(
            &binding.work_ref.realm_id,
            &binding.work_ref.namespace,
            &binding.binding_id,
        );
        if let Some(existing) = guard.execution_bindings.get(&key) {
            return if existing == &binding {
                Ok(existing.clone())
            } else {
                Err(WorkGraphError::Conflict(format!(
                    "work execution binding {} already exists with different content",
                    binding.binding_id
                )))
            };
        }
        validate_execution_binding_insert(
            &binding,
            expected_item_revision,
            guard.items.values(),
            guard.execution_bindings.values(),
        )?;
        if !crate::WorkExecutionMachine::retry_eligible(&binding)? {
            guard.execution_recovery.insert(key.clone());
        }
        guard.execution_bindings.insert(key, binding.clone());
        guard.append_event(event);
        Ok(binding)
    }

    async fn get_execution_binding(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        binding_id: &WorkExecutionBindingId,
    ) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .execution_bindings
            .get(&execution_binding_key(realm_id, namespace, binding_id))
            .cloned())
    }

    async fn get_execution_binding_by_target_run(
        &self,
        realm_id: &str,
        run_id: &str,
    ) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .execution_bindings
            .values()
            .find(|binding| {
                binding.work_ref.realm_id == realm_id && binding.target.run_id() == run_id
            })
            .cloned())
    }

    async fn update_execution_binding_cas(
        &self,
        commit: crate::WorkExecutionObservationCommit,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkExecutionBinding, WorkGraphError> {
        let (previous, observation, binding, effect) = commit.into_parts();
        crate::WorkExecutionMachine::validate_projection(&binding)?;
        let mut guard = self.inner.write().await;
        let key = execution_binding_key(
            &binding.work_ref.realm_id,
            &binding.work_ref.namespace,
            &binding.binding_id,
        );
        let current = guard.execution_bindings.get(&key).ok_or_else(|| {
            WorkGraphError::Conflict(format!(
                "work execution binding {} does not exist",
                binding.binding_id
            ))
        })?;
        if current.machine_state.revision != expected_previous_revision {
            return Err(WorkGraphError::Conflict(format!(
                "stale work execution revision for {}: expected {}, actual {}",
                binding.binding_id, expected_previous_revision, current.machine_state.revision
            )));
        }
        if current != &previous {
            return Err(WorkGraphError::Conflict(format!(
                "work execution transition authority for {} was minted from a different predecessor",
                binding.binding_id
            )));
        }
        let (expected_binding, expected_effect) = crate::WorkExecutionMachine::observe(
            current.clone(),
            expected_previous_revision,
            observation,
        )?;
        if binding != expected_binding || effect != expected_effect {
            return Err(WorkGraphError::Conflict(format!(
                "work execution transition authority for {} does not match the generated machine result",
                binding.binding_id
            )));
        }
        if !current.has_same_immutable_spec(&binding) {
            return Err(WorkGraphError::Conflict(format!(
                "immutable work execution specification changed for {}",
                binding.binding_id
            )));
        }
        if crate::WorkExecutionMachine::retry_eligible(&binding)? {
            guard.execution_recovery.remove(&key);
        } else {
            guard.execution_recovery.insert(key.clone());
        }
        guard.execution_bindings.insert(key, binding.clone());
        guard.append_event(event);
        Ok(binding)
    }

    async fn list_execution_bindings(
        &self,
        filter: WorkExecutionBindingFilter,
    ) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        let superseded = guard
            .execution_bindings
            .values()
            .filter_map(|binding| {
                binding.supersedes.clone().map(|supersedes| {
                    (
                        binding.work_ref.realm_id.clone(),
                        binding.work_ref.namespace.clone(),
                        supersedes,
                    )
                })
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut bindings = guard
            .execution_bindings
            .values()
            .filter(|binding| execution_binding_matches_filter(binding, &filter, &superseded))
            .cloned()
            .collect::<Vec<_>>();
        bindings.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.binding_id.cmp(&right.binding_id))
        });
        if let Some(limit) = filter.limit {
            bindings.truncate(limit);
        }
        Ok(bindings)
    }

    async fn list_attention_matching_bounded(
        &self,
        filter: AttentionListRequest,
        observed_at: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let guard = self.inner.read().await;
        let compare = |left: &WorkAttentionBinding, right: &WorkAttentionBinding| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.binding_id.cmp(&right.binding_id))
        };
        let mut bindings = Vec::with_capacity(limit.min(1024));
        for binding in guard.attention.values() {
            if !attention_matches_non_status_filter(binding, &filter) {
                continue;
            }
            if let Some(status) = filter.status.as_ref()
                && (!attention_is_coarse_status_candidate(&binding.status, status)
                    || !WorkAttentionMachine::matches_status_filter_at(
                        binding,
                        status,
                        observed_at,
                    )?)
            {
                continue;
            }
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

    async fn list_execution_bindings_for_recovery(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
        let guard = self.inner.read().await;
        Ok(guard
            .execution_recovery
            .iter()
            .filter(|(realm, candidate_namespace, _)| {
                realm == realm_id && candidate_namespace == namespace
            })
            .filter_map(|key| guard.execution_bindings.get(key).cloned())
            .collect())
    }

    async fn insert_goal(
        &self,
        mut item: WorkItem,
        mut item_event: WorkGraphEvent,
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
        enrich_item_transition_facts(
            None,
            &mut item,
            guard.items.values(),
            guard.edges.iter(),
            &mut item_event,
        )?;
        guard.items.insert(item_key, item.clone());
        guard.attention.insert(attention_key, attention.clone());
        guard.append_event(item_event);
        guard.append_event(attention_event);
        Ok((item, attention))
    }

    async fn insert_attention_for_existing_item(
        &self,
        attention: WorkAttentionBinding,
        expected_item_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        let mut guard = self.inner.write().await;
        let work_key = item_key(
            &attention.work_ref.realm_id,
            &attention.work_ref.namespace,
            &attention.work_ref.item_id,
        );
        let item = guard.items.get(&work_key).ok_or_else(|| {
            WorkGraphError::not_found(
                attention.work_ref.realm_id.clone(),
                attention.work_ref.namespace.clone(),
                attention.work_ref.item_id.clone(),
            )
        })?;
        if item.revision != expected_item_revision {
            return Err(WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected: expected_item_revision,
                actual: item.revision,
            });
        }
        if WorkGraphMachine::classify_terminality(item)? {
            return Err(WorkGraphError::InvalidTransition(format!(
                "cannot bind attention to terminal work item {}",
                item.id
            )));
        }
        let key = attention_key(
            &attention.work_ref.realm_id,
            &attention.work_ref.namespace,
            &attention.binding_id,
        );
        if guard.attention.contains_key(&key) {
            return Err(WorkGraphError::Conflict(format!(
                "work attention binding {} already exists",
                attention.binding_id
            )));
        }
        if let Some(occupant) = active_target_occupant_in(guard.attention.values(), &attention) {
            return Err(active_target_conflict(&attention, &occupant));
        }
        guard.attention.insert(key, attention.clone());
        guard.append_event(event);
        Ok(attention)
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
        mut item: WorkItem,
        expected_previous_revision: u64,
        mut item_event: WorkGraphEvent,
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
        let previous = current.clone();
        enrich_item_transition_facts(
            Some(&previous),
            &mut item,
            guard.items.values(),
            guard.edges.iter(),
            &mut item_event,
        )?;
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

    async fn list_public_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        let limit = filter.limit.unwrap_or(usize::MAX);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let guard = self.inner.read().await;
        Ok(guard
            .events
            .iter()
            .filter(|event| event_matches_filter(event, &filter))
            .filter(|event| !is_internal_execution_event(event.kind))
            .take(limit)
            .cloned()
            .collect())
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

fn execution_binding_key(
    realm_id: &str,
    namespace: &WorkNamespace,
    id: &WorkExecutionBindingId,
) -> (String, WorkNamespace, WorkExecutionBindingId) {
    (realm_id.to_string(), namespace.clone(), id.clone())
}

fn validate_execution_binding_insert<'a>(
    binding: &WorkExecutionBinding,
    expected_item_revision: u64,
    items: impl Iterator<Item = &'a WorkItem>,
    bindings: impl Iterator<Item = &'a WorkExecutionBinding>,
) -> Result<(), WorkGraphError> {
    let item = items
        .filter(|item| {
            item.realm_id == binding.work_ref.realm_id
                && item.namespace == binding.work_ref.namespace
                && item.id == binding.work_ref.item_id
        })
        .last()
        .ok_or_else(|| {
            WorkGraphError::not_found(
                binding.work_ref.realm_id.clone(),
                binding.work_ref.namespace.clone(),
                binding.work_ref.item_id.clone(),
            )
        })?;
    if item.revision != expected_item_revision {
        return Err(WorkGraphError::StaleRevision {
            id: item.id.clone(),
            expected: expected_item_revision,
            actual: item.revision,
        });
    }
    if WorkGraphMachine::classify_terminality(item)? {
        return Err(WorkGraphError::InvalidTransition(format!(
            "terminal work item {} cannot bind a new execution",
            item.id
        )));
    }

    let bindings = bindings.collect::<Vec<_>>();
    if bindings
        .iter()
        .any(|existing| existing.target.run_id() == binding.target.run_id())
    {
        return Err(WorkGraphError::Conflict(format!(
            "work execution binding {} reuses target run id {}",
            binding.binding_id,
            binding.target.run_id()
        )));
    }
    let scoped = bindings
        .into_iter()
        .filter(|existing| {
            existing.work_ref.realm_id == binding.work_ref.realm_id
                && existing.work_ref.namespace == binding.work_ref.namespace
                && existing.work_ref.item_id == binding.work_ref.item_id
        })
        .collect::<Vec<_>>();
    if scoped.iter().any(|existing| {
        existing.idempotency_key == binding.idempotency_key
            || existing.target.run_id() == binding.target.run_id()
    }) {
        return Err(WorkGraphError::Conflict(format!(
            "work execution binding {} reuses an idempotency key or run id",
            binding.binding_id
        )));
    }

    match &binding.supersedes {
        None if !scoped.is_empty() => Err(WorkGraphError::Conflict(format!(
            "work item {} already has an execution chain",
            binding.work_ref.item_id
        ))),
        None => Ok(()),
        Some(predecessor) => {
            if predecessor == &binding.binding_id {
                return Err(WorkGraphError::InvalidInput(
                    "work execution binding cannot supersede itself".to_string(),
                ));
            }
            let Some(predecessor_binding) = scoped
                .iter()
                .copied()
                .find(|existing| &existing.binding_id == predecessor)
            else {
                return Err(WorkGraphError::InvalidInput(format!(
                    "superseded work execution binding {predecessor} is not in the same work item chain"
                )));
            };
            if !crate::WorkExecutionMachine::retry_eligible(predecessor_binding)? {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "work execution binding {predecessor} is not terminal and cannot be superseded"
                )));
            }
            if scoped
                .iter()
                .any(|existing| existing.supersedes.as_ref() == Some(predecessor))
            {
                return Err(WorkGraphError::Conflict(format!(
                    "work execution binding {predecessor} is already superseded"
                )));
            }
            Ok(())
        }
    }
}

fn execution_binding_matches_filter(
    binding: &WorkExecutionBinding,
    filter: &WorkExecutionBindingFilter,
    superseded: &std::collections::BTreeSet<(String, WorkNamespace, WorkExecutionBindingId)>,
) -> bool {
    filter
        .realm_id
        .as_ref()
        .is_none_or(|realm_id| &binding.work_ref.realm_id == realm_id)
        && filter
            .namespace
            .as_ref()
            .is_none_or(|namespace| &binding.work_ref.namespace == namespace)
        && filter
            .item_id
            .as_ref()
            .is_none_or(|item_id| &binding.work_ref.item_id == item_id)
        && (!filter.current_only
            || !superseded.contains(&(
                binding.work_ref.realm_id.clone(),
                binding.work_ref.namespace.clone(),
                binding.binding_id.clone(),
            )))
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

fn enrich_item_transition_facts<'a>(
    previous: Option<&WorkItem>,
    current: &mut WorkItem,
    stored_items: impl Iterator<Item = &'a WorkItem>,
    edges: impl Iterator<Item = &'a WorkEdge>,
    event: &mut WorkGraphEvent,
) -> Result<(), WorkGraphError> {
    let mut items = stored_items
        .filter(|item| {
            item.realm_id == current.realm_id
                && item.namespace == current.namespace
                && item.id != current.id
        })
        .cloned()
        .collect::<Vec<_>>();
    items.push(current.clone());
    let scoped_edges = edges
        .filter(|edge| edge.realm_id == current.realm_id && edge.namespace == current.namespace)
        .cloned()
        .collect::<Vec<_>>();

    let current_ready = item_ready_with_graph(current, event.at, &items, &scoped_edges)?;
    let previous_ready = previous
        .map(|item| item_ready_with_graph(item, event.at, &items, &scoped_edges))
        .transpose()?
        .unwrap_or(false);
    let previous_expired_claim = previous
        .and_then(|item| item.claim.as_ref())
        .filter(|claim| claim.expiry_observed_at.is_none())
        .and_then(|claim| claim.lease_expires_at)
        .is_some_and(|lease_expires_at| lease_expires_at <= event.at);
    if (current_ready && !previous_ready)
        || (matches!(event.kind, WorkGraphEventKind::Claimed) && previous_ready)
        || (matches!(event.kind, WorkGraphEventKind::Released)
            && previous_expired_claim
            && current_ready)
        || (matches!(event.kind, WorkGraphEventKind::ReadinessObserved) && current_ready)
    {
        push_fact_once(
            event,
            WorkGraphFact::ItemReady {
                item_id: current.id.clone(),
                item_revision: previous
                    .filter(|_| matches!(event.kind, WorkGraphEventKind::Claimed))
                    .map_or(current.revision, |item| item.revision),
            },
        );
    }
    if previous.is_some() && matches!(event.kind, WorkGraphEventKind::Closed) {
        for edge in scoped_edges.iter().filter(|edge| {
            edge.kind == crate::types::WorkEdgeKind::Parent && edge.from_id == current.id
        }) {
            if let Some(parent) = items.iter().find(|item| item.id == edge.to_id)
                && item_ready_with_graph(parent, event.at, &items, &scoped_edges)?
            {
                push_fact_once(
                    event,
                    WorkGraphFact::ItemReady {
                        item_id: parent.id.clone(),
                        item_revision: parent.revision,
                    },
                );
            }
        }
    }

    if let Some(previous) = previous
        && let Some(claim) = previous.claim.as_ref()
        && claim.expiry_observed_at.is_none()
        && let Some(lease_expires_at) = claim.lease_expires_at
        && lease_expires_at <= event.at
    {
        push_fact_once(
            event,
            WorkGraphFact::LeaseExpired {
                item_id: current.id.clone(),
                expired_owner: claim.owner.key.clone(),
                lease_expires_at,
                observed_at: event.at,
            },
        );
        if let Some(current_claim) = current.claim.as_mut()
            && current_claim.owner == claim.owner
            && current_claim.claimed_at == claim.claimed_at
        {
            current_claim.expiry_observed_at = Some(event.at);
            if let Some(item_payload) = event
                .payload
                .as_object_mut()
                .and_then(|payload| payload.get_mut("item"))
            {
                *item_payload = serde_json::to_value(&*current)
                    .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            }
        }
    }

    let current_terminal = WorkGraphMachine::classify_terminality(current)?;
    let newly_terminal = previous
        .map(|previous| {
            WorkGraphMachine::classify_terminality(previous)
                .map(|was_terminal| current_terminal && !was_terminal)
        })
        .transpose()?
        .unwrap_or(current_terminal);
    let mut namespace_terminal = newly_terminal;
    if namespace_terminal {
        for item in &items {
            if !WorkGraphMachine::classify_terminality(item)? {
                namespace_terminal = false;
                break;
            }
        }
    }
    if namespace_terminal {
        push_fact_once(
            event,
            WorkGraphFact::NamespaceTerminal {
                namespace: current.namespace.clone(),
                observed_at: event.at,
            },
        );
    }
    Ok(())
}

fn item_ready_with_graph(
    item: &WorkItem,
    now: DateTime<Utc>,
    items: &[WorkItem],
    edges: &[WorkEdge],
) -> Result<bool, WorkGraphError> {
    let joined = matches!(
        child_join_disposition_with_graph(item, items, edges)?,
        ChildJoinDisposition::Satisfied
    );
    WorkGraphMachine::classify_readiness_from_observation(
        item,
        now,
        unresolved_blocker_count_with_graph(item, items, edges)?,
        joined,
    )
}

fn child_join_disposition_with_graph(
    item: &WorkItem,
    items: &[WorkItem],
    edges: &[WorkEdge],
) -> Result<ChildJoinDisposition, WorkGraphError> {
    let children = edges
        .iter()
        .filter(|edge| edge.kind == crate::types::WorkEdgeKind::Parent && edge.to_id == item.id)
        .filter_map(|edge| items.iter().find(|candidate| candidate.id == edge.from_id));
    let mut active = 0u64;
    let mut failed = 0u64;
    let mut cancelled = 0u64;
    for child in children {
        match child.status {
            crate::types::WorkStatus::Completed => {}
            crate::types::WorkStatus::Failed => failed = failed.saturating_add(1),
            crate::types::WorkStatus::Cancelled => cancelled = cancelled.saturating_add(1),
            _ => active = active.saturating_add(1),
        }
    }
    WorkGraphMachine::classify_child_join(item, active, failed, cancelled)
}

fn unresolved_blocker_count_with_graph(
    item: &WorkItem,
    items: &[WorkItem],
    edges: &[WorkEdge],
) -> Result<u64, WorkGraphError> {
    let mut unresolved = 0u64;
    for edge in edges
        .iter()
        .filter(|edge| edge.kind == crate::types::WorkEdgeKind::Blocks && edge.to_id == item.id)
    {
        let blocker = items.iter().find(|candidate| candidate.id == edge.from_id);
        if !WorkGraphMachine::classify_blocker_satisfied(item, blocker)? {
            unresolved = unresolved.saturating_add(1);
        }
    }
    Ok(unresolved)
}

fn push_fact_once(event: &mut WorkGraphEvent, fact: WorkGraphFact) {
    if !event.facts.contains(&fact) {
        event.facts.push(fact);
    }
}

fn attention_transition_event(
    binding: &WorkAttentionBinding,
    observed_at: DateTime<Utc>,
) -> WorkGraphEvent {
    WorkGraphEvent::graph(
        binding.work_ref.realm_id.clone(),
        binding.work_ref.namespace.clone(),
        WorkGraphEventKind::AttentionUpdated,
        observed_at,
        serde_json::json!({ "attention": binding }),
    )
}

fn attention_matches_non_status_filter(
    binding: &WorkAttentionBinding,
    filter: &AttentionListRequest,
) -> bool {
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
    true
}

fn attention_matches_filter(binding: &WorkAttentionBinding, filter: &AttentionListRequest) -> bool {
    attention_matches_non_status_filter(binding, filter)
        && filter
            .status
            .as_ref()
            .is_none_or(|status| attention_status_matches_filter(&binding.status, status))
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

fn attention_is_coarse_status_candidate(
    actual: &WorkAttentionStatus,
    filter: &WorkAttentionStatus,
) -> bool {
    match filter {
        WorkAttentionStatus::Active => {
            matches!(
                actual,
                WorkAttentionStatus::Active | WorkAttentionStatus::Paused { .. }
            )
        }
        WorkAttentionStatus::Paused { .. } => {
            matches!(actual, WorkAttentionStatus::Paused { .. })
        }
        WorkAttentionStatus::Superseded => {
            matches!(actual, WorkAttentionStatus::Superseded)
        }
        WorkAttentionStatus::Stopped => matches!(actual, WorkAttentionStatus::Stopped),
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

fn is_internal_execution_event(kind: WorkGraphEventKind) -> bool {
    matches!(
        kind,
        WorkGraphEventKind::ExecutionBound | WorkGraphEventKind::ExecutionTransitioned
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub struct SqliteWorkGraphStore {
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl SqliteWorkGraphStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, WorkGraphError> {
        let store = Self { path: path.into() };
        let legacy_tables = pre_namespace_tables_with_rows(&store.path)?;
        if !legacy_tables.is_empty() {
            return Err(WorkGraphError::NamespaceAssignmentRequired {
                backend: store.path.display().to_string(),
                tables: legacy_tables,
            });
        }
        // Probe open: `with_connection` brings the schema domain up to date.
        store.with_connection(|_| Ok(()))?;
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
            tx.execute("DELETE FROM workgraph_execution_bindings", [])
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
        // Per-operation fence guard: lives exactly as long as the connection
        // it admits.
        let _guard = meerkat_sqlite::OperationGuard::for_database(&self.path)
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        let mut conn = meerkat_sqlite::open_with(
            &self.path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                schema_preflight: &[&WORKGRAPH_DOMAIN],
                ..Default::default()
            },
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &WORKGRAPH_DOMAIN)
            .map_err(|err| WorkGraphError::Store(err.to_string()))?;
        f(&mut conn)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pre_namespace_tables_with_rows(path: &Path) -> Result<Vec<String>, WorkGraphError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| WorkGraphError::Store(error.to_string()))?;
    let mut legacy = Vec::new();
    for table in [
        "workgraph_items",
        "workgraph_attention",
        "workgraph_edges",
        "workgraph_events",
        "workgraph_execution_bindings",
    ] {
        let exists = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| WorkGraphError::Store(error.to_string()))?;
        if !exists {
            continue;
        }
        let mut columns = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| WorkGraphError::Store(error.to_string()))?;
        let has_namespace = columns
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| WorkGraphError::Store(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| WorkGraphError::Store(error.to_string()))?
            .iter()
            .any(|column| column == "namespace");
        if has_namespace {
            continue;
        }
        let has_rows = connection
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| WorkGraphError::Store(error.to_string()))?;
        if has_rows {
            legacy.push(table.to_string());
        }
    }
    Ok(legacy)
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
        mut item: WorkItem,
        mut event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(item.realm_id.clone()),
                    namespace: Some(item.namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, &item.realm_id, &item.namespace, None)?;
            enrich_item_transition_facts(None, &mut item, items.iter(), edges.iter(), &mut event)?;
            insert_item_tx(&tx, &item)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(item)
        })
    }

    async fn update_item_cas(
        &self,
        mut item: WorkItem,
        expected_previous_revision: u64,
        mut event: WorkGraphEvent,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let previous = select_item(&tx, &item.realm_id, &item.namespace, &item.id)?;
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(item.realm_id.clone()),
                    namespace: Some(item.namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, &item.realm_id, &item.namespace, None)?;
            enrich_item_transition_facts(
                previous.as_ref(),
                &mut item,
                items.iter(),
                edges.iter(),
                &mut event,
            )?;
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

    async fn claim_item_atomically(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        mut request: ClaimWorkItemRequest,
        observed_at: DateTime<Utc>,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let previous =
                select_item(&tx, realm_id, namespace, &request.id)?.ok_or_else(|| {
                    WorkGraphError::not_found(
                        realm_id.to_string(),
                        namespace.clone(),
                        request.id.clone(),
                    )
                })?;
            if request.expected_revision != previous.revision {
                return Err(WorkGraphError::StaleRevision {
                    id: previous.id.clone(),
                    expected: request.expected_revision,
                    actual: previous.revision,
                });
            }
            WorkGraphMachine::validate_claim_request(&request, &observed_at)?;
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, realm_id, namespace, None)?;
            let disposition = child_join_disposition_with_graph(&previous, &items, &edges)?;
            if matches!(
                disposition,
                ChildJoinDisposition::PropagateFailure
                    | ChildJoinDisposition::PropagateCancellation
            ) {
                return Ok(None);
            }
            let unresolved = unresolved_blocker_count_with_graph(&previous, &items, &edges)?;
            let (admission_item, refresh_event) = match WorkGraphMachine::refresh_eligibility(
                previous.clone(),
                unresolved,
                observed_at,
            )? {
                Some((mut refreshed, mut event)) => {
                    enrich_item_transition_facts(
                        Some(&previous),
                        &mut refreshed,
                        items.iter(),
                        edges.iter(),
                        &mut event,
                    )?;
                    (refreshed, Some(event))
                }
                None => (previous.clone(), None),
            };
            request.expected_revision = admission_item.revision;
            let (mut item, mut event) = WorkGraphMachine::claim_item_with_unresolved_blockers(
                admission_item.clone(),
                unresolved,
                matches!(disposition, ChildJoinDisposition::Satisfied),
                request,
                observed_at,
            )?;
            enrich_item_transition_facts(
                Some(&admission_item),
                &mut item,
                items.iter(),
                edges.iter(),
                &mut event,
            )?;
            let changed = update_item_tx(&tx, &item, previous.revision)?;
            if changed == 0 {
                return Err(WorkGraphError::StaleRevision {
                    id: item.id.clone(),
                    expected: previous.revision,
                    actual: current_revision_tx(&tx, realm_id, namespace, &item.id)?
                        .unwrap_or(previous.revision),
                });
            }
            if let Some(refresh_event) = refresh_event {
                insert_event_tx(&tx, &refresh_event)?;
            }
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            Ok(Some(item))
        })
    }

    async fn observe_readiness_atomically(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        mut request: ObserveReadinessRequest,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let previous =
                select_item(&tx, realm_id, namespace, &request.id)?.ok_or_else(|| {
                    WorkGraphError::not_found(
                        realm_id.to_string(),
                        namespace.clone(),
                        request.id.clone(),
                    )
                })?;
            if request.expected_revision != previous.revision {
                return Err(WorkGraphError::StaleRevision {
                    id: previous.id.clone(),
                    expected: request.expected_revision,
                    actual: previous.revision,
                });
            }
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, realm_id, namespace, None)?;
            let disposition = child_join_disposition_with_graph(&previous, &items, &edges)?;
            if matches!(
                disposition,
                ChildJoinDisposition::PropagateFailure
                    | ChildJoinDisposition::PropagateCancellation
            ) {
                return Ok(None);
            }
            let unresolved = unresolved_blocker_count_with_graph(&previous, &items, &edges)?;
            let (admission_item, refresh_event) = match WorkGraphMachine::refresh_eligibility(
                previous.clone(),
                unresolved,
                request.observed_at,
            )? {
                Some((mut refreshed, mut event)) => {
                    enrich_item_transition_facts(
                        Some(&previous),
                        &mut refreshed,
                        items.iter(),
                        edges.iter(),
                        &mut event,
                    )?;
                    (refreshed, Some(event))
                }
                None => (previous.clone(), None),
            };
            request.expected_revision = admission_item.revision;
            let (mut item, mut event) = WorkGraphMachine::observe_readiness(
                admission_item.clone(),
                request,
                unresolved,
                matches!(disposition, ChildJoinDisposition::Satisfied),
            )?;
            enrich_item_transition_facts(
                Some(&admission_item),
                &mut item,
                items.iter(),
                edges.iter(),
                &mut event,
            )?;
            let changed = update_item_tx(&tx, &item, previous.revision)?;
            if changed == 0 {
                return Err(WorkGraphError::StaleRevision {
                    id: item.id.clone(),
                    expected: previous.revision,
                    actual: current_revision_tx(&tx, realm_id, namespace, &item.id)?
                        .unwrap_or(previous.revision),
                });
            }
            if let Some(refresh_event) = refresh_event {
                insert_event_tx(&tx, &refresh_event)?;
            }
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            Ok(Some(item))
        })
    }

    async fn reconcile_child_join_atomically(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        parent_id: &WorkItemId,
        expected_revision: u64,
        observed_at: DateTime<Utc>,
    ) -> Result<Option<WorkItem>, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let Some(previous) = select_item(&tx, realm_id, namespace, parent_id)? else {
                return Ok(None);
            };
            if previous.revision != expected_revision {
                return Err(WorkGraphError::StaleRevision {
                    id: previous.id.clone(),
                    expected: expected_revision,
                    actual: previous.revision,
                });
            }
            if WorkGraphMachine::classify_terminality(&previous)? {
                return Ok(Some(previous));
            }
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, realm_id, namespace, None)?;
            let status = match child_join_disposition_with_graph(&previous, &items, &edges)? {
                ChildJoinDisposition::PropagateFailure => crate::types::WorkStatus::Failed,
                ChildJoinDisposition::PropagateCancellation => crate::types::WorkStatus::Cancelled,
                ChildJoinDisposition::Waiting | ChildJoinDisposition::Satisfied => {
                    return Ok(None);
                }
            };
            let (mut terminal, mut item_event) = WorkGraphMachine::close_item(
                previous.clone(),
                crate::types::CloseWorkItemRequest {
                    id: previous.id.clone(),
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    expected_revision: previous.revision,
                    status,
                },
                observed_at,
            )?;
            enrich_item_transition_facts(
                Some(&previous),
                &mut terminal,
                items.iter(),
                edges.iter(),
                &mut item_event,
            )?;
            let attention = list_sqlite_attention(
                &tx,
                &AttentionListRequest {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    target: None,
                    status: None,
                },
                None,
                None,
            )?;
            let mut stopped_attention = Vec::new();
            for binding in attention.into_iter().filter(|binding| {
                &binding.work_ref.item_id == parent_id
                    && !matches!(
                        binding.status,
                        WorkAttentionStatus::Stopped | WorkAttentionStatus::Superseded
                    )
            }) {
                let expected_revision = binding.machine_state.revision;
                let stopped = WorkAttentionMachine::stop(binding, expected_revision, observed_at)?;
                let event = attention_transition_event(&stopped, observed_at);
                stopped_attention.push((stopped, expected_revision, event));
            }
            let changed = update_item_tx(&tx, &terminal, previous.revision)?;
            if changed == 0 {
                return Err(WorkGraphError::StaleRevision {
                    id: terminal.id.clone(),
                    expected: previous.revision,
                    actual: current_revision_tx(&tx, realm_id, namespace, &terminal.id)?
                        .unwrap_or(previous.revision),
                });
            }
            insert_event_tx(&tx, &item_event)?;
            for (binding, expected_revision, event) in &stopped_attention {
                let changed = update_attention_tx(&tx, binding, *expected_revision)?;
                if changed == 0 {
                    return Err(WorkGraphError::Conflict(format!(
                        "attention binding {} changed during atomic child-join reconciliation",
                        binding.binding_id
                    )));
                }
                insert_event_tx(&tx, event)?;
            }
            tx.commit()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            Ok(Some(terminal))
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

    async fn read_namespace_graph(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<(DateTime<Utc>, Vec<WorkItem>, Vec<WorkEdge>), WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let observed_at = Utc::now();
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, realm_id, namespace, None)?;
            tx.commit()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            Ok((observed_at, items, edges))
        })
    }

    async fn read_namespace_snapshot(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<WorkGraphNamespaceRead, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                // Fence writers before sampling captured_at. A deferred SQLite
                // transaction does not establish its read snapshot until the
                // first SELECT, which could otherwise include a commit newer
                // than the timestamp used for readiness classification.
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let captured_at = Utc::now();
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, realm_id, namespace, None)?;
            let attention = list_sqlite_attention(
                &tx,
                &AttentionListRequest {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    target: None,
                    status: None,
                },
                None,
                None,
            )?;
            let event_high_water_mark = latest_sqlite_event_seq(
                &tx,
                &WorkGraphEventFilter {
                    realm_id: Some(realm_id.to_string()),
                    namespace: Some(namespace.clone()),
                    all_namespaces: false,
                    after_seq: None,
                    limit: Some(1),
                },
            )?;
            tx.commit()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            Ok(WorkGraphNamespaceRead {
                captured_at,
                event_high_water_mark,
                items,
                edges,
                attention,
            })
        })
    }

    async fn insert_execution_binding(
        &self,
        commit: crate::WorkExecutionBindCommit,
        expected_item_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkExecutionBinding, WorkGraphError> {
        let (binding, effect) = commit.into_parts();
        binding.validate()?;
        crate::WorkExecutionMachine::validate_projection(&binding)?;
        let (expected_state, expected_effect) =
            crate::WorkExecutionMachine::bind(&binding.binding_id, binding.target.run_id())?;
        if binding.machine_state != expected_state || effect != expected_effect {
            return Err(WorkGraphError::InvalidInput(format!(
                "work execution binding {} lacks canonical bind authority",
                binding.binding_id
            )));
        }
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            if let Some(existing) = select_execution_binding(
                &tx,
                &binding.work_ref.realm_id,
                &binding.work_ref.namespace,
                &binding.binding_id,
            )? {
                return if existing == binding {
                    Ok(existing)
                } else {
                    Err(WorkGraphError::Conflict(format!(
                        "work execution binding {} already exists with different content",
                        binding.binding_id
                    )))
                };
            }
            let items = select_item(
                &tx,
                &binding.work_ref.realm_id,
                &binding.work_ref.namespace,
                &binding.work_ref.item_id,
            )?
            .into_iter()
            .collect::<Vec<_>>();
            let bindings = list_sqlite_execution_bindings(
                &tx,
                &WorkExecutionBindingFilter {
                    realm_id: Some(binding.work_ref.realm_id.clone()),
                    namespace: Some(binding.work_ref.namespace.clone()),
                    item_id: Some(binding.work_ref.item_id.clone()),
                    current_only: false,
                    limit: None,
                },
            )?;
            validate_execution_binding_insert(
                &binding,
                expected_item_revision,
                items.iter(),
                bindings.iter(),
            )?;
            insert_execution_binding_tx(&tx, &binding)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(binding)
        })
    }

    async fn get_execution_binding(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        binding_id: &WorkExecutionBindingId,
    ) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
        self.with_connection(|conn| select_execution_binding(conn, realm_id, namespace, binding_id))
    }

    async fn get_execution_binding_by_target_run(
        &self,
        realm_id: &str,
        run_id: &str,
    ) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT binding_json FROM workgraph_execution_bindings
                 WHERE realm_id = ?1 AND target_run_id = ?2",
                params![realm_id, run_id],
                |row| row_json(row, 0),
            )
            .optional()
            .map_err(|error| WorkGraphError::Store(error.to_string()))
        })
    }

    async fn update_execution_binding_cas(
        &self,
        commit: crate::WorkExecutionObservationCommit,
        expected_previous_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkExecutionBinding, WorkGraphError> {
        let (previous, observation, binding, effect) = commit.into_parts();
        crate::WorkExecutionMachine::validate_projection(&binding)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let current = select_execution_binding(
                &tx,
                &binding.work_ref.realm_id,
                &binding.work_ref.namespace,
                &binding.binding_id,
            )?
            .ok_or_else(|| {
                WorkGraphError::Conflict(format!(
                    "work execution binding {} does not exist",
                    binding.binding_id
                ))
            })?;
            if current.machine_state.revision != expected_previous_revision {
                return Err(WorkGraphError::Conflict(format!(
                    "stale work execution revision for {}: expected {}, actual {}",
                    binding.binding_id, expected_previous_revision, current.machine_state.revision
                )));
            }
            if current != previous {
                return Err(WorkGraphError::Conflict(format!(
                    "work execution transition authority for {} was minted from a different predecessor",
                    binding.binding_id
                )));
            }
            let (expected_binding, expected_effect) = crate::WorkExecutionMachine::observe(
                current.clone(),
                expected_previous_revision,
                observation,
            )?;
            if binding != expected_binding || effect != expected_effect {
                return Err(WorkGraphError::Conflict(format!(
                    "work execution transition authority for {} does not match the generated machine result",
                    binding.binding_id
                )));
            }
            if !current.has_same_immutable_spec(&binding) {
                return Err(WorkGraphError::Conflict(format!(
                    "immutable work execution specification changed for {}",
                    binding.binding_id
                )));
            }
            let json = serde_json::to_string(&binding)
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let recovery_pending = execution_recovery_pending(&binding)?;
            let changed = tx
                .execute(
                    "UPDATE workgraph_execution_bindings
                     SET revision = ?1, recovery_pending = ?2, binding_json = ?3
                     WHERE realm_id = ?4 AND namespace = ?5 AND binding_id = ?6
                       AND revision = ?7",
                    params![
                        binding.machine_state.revision,
                        recovery_pending,
                        json,
                        binding.work_ref.realm_id,
                        binding.work_ref.namespace.as_str(),
                        binding.binding_id.as_str(),
                        expected_previous_revision,
                    ],
                )
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            if changed == 0 {
                let current = select_execution_binding(
                    &tx,
                    &binding.work_ref.realm_id,
                    &binding.work_ref.namespace,
                    &binding.binding_id,
                )?;
                return match current {
                    Some(current) => Err(WorkGraphError::Conflict(format!(
                        "stale work execution revision for {}: expected {}, actual {}",
                        binding.binding_id,
                        expected_previous_revision,
                        current.machine_state.revision
                    ))),
                    None => Err(WorkGraphError::Conflict(format!(
                        "work execution binding {} does not exist",
                        binding.binding_id
                    ))),
                };
            }
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            Ok(binding)
        })
    }

    async fn list_execution_bindings(
        &self,
        filter: WorkExecutionBindingFilter,
    ) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_execution_bindings(conn, &filter))
    }

    async fn list_execution_bindings_for_recovery(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
    ) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
        self.with_connection(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT binding_json FROM workgraph_execution_bindings
                     WHERE realm_id = ?1 AND namespace = ?2 AND recovery_pending = 1
                     ORDER BY created_at_utc ASC, binding_id ASC",
                )
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            let rows = statement
                .query_map(params![realm_id, namespace.as_str()], |row| {
                    row_json::<WorkExecutionBinding>(row, 0)
                })
                .map_err(|error| WorkGraphError::Store(error.to_string()))?;
            rows.map(|row| row.map_err(|error| WorkGraphError::Store(error.to_string())))
                .collect()
        })
    }

    async fn insert_goal(
        &self,
        mut item: WorkItem,
        mut item_event: WorkGraphEvent,
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
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(item.realm_id.clone()),
                    namespace: Some(item.namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, &item.realm_id, &item.namespace, None)?;
            enrich_item_transition_facts(
                None,
                &mut item,
                items.iter(),
                edges.iter(),
                &mut item_event,
            )?;
            insert_item_tx(&tx, &item)?;
            insert_attention_tx(&tx, &attention)?;
            insert_event_tx(&tx, &item_event)?;
            insert_event_tx(&tx, &attention_event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok((item, attention))
        })
    }

    async fn insert_attention_for_existing_item(
        &self,
        attention: WorkAttentionBinding,
        expected_item_revision: u64,
        event: WorkGraphEvent,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let item = select_item(
                &tx,
                &attention.work_ref.realm_id,
                &attention.work_ref.namespace,
                &attention.work_ref.item_id,
            )?
            .ok_or_else(|| {
                WorkGraphError::not_found(
                    attention.work_ref.realm_id.clone(),
                    attention.work_ref.namespace.clone(),
                    attention.work_ref.item_id.clone(),
                )
            })?;
            if item.revision != expected_item_revision {
                return Err(WorkGraphError::StaleRevision {
                    id: item.id,
                    expected: expected_item_revision,
                    actual: item.revision,
                });
            }
            if WorkGraphMachine::classify_terminality(&item)? {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "cannot bind attention to terminal work item {}",
                    item.id
                )));
            }
            if let Some(occupant) = active_target_occupant_tx(&tx, &attention)? {
                return Err(active_target_conflict(&attention, &occupant));
            }
            insert_attention_tx(&tx, &attention)?;
            insert_event_tx(&tx, &event)?;
            tx.commit()
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            Ok(attention)
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
        mut item: WorkItem,
        expected_previous_revision: u64,
        mut item_event: WorkGraphEvent,
        attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::validate_item_projection(&item)?;
        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| WorkGraphError::Store(err.to_string()))?;
            let previous = select_item(&tx, &item.realm_id, &item.namespace, &item.id)?;
            let items = list_sqlite_items(
                &tx,
                &WorkItemFilter {
                    realm_id: Some(item.realm_id.clone()),
                    namespace: Some(item.namespace.clone()),
                    include_terminal: true,
                    ..WorkItemFilter::default()
                },
            )?;
            let edges = list_sqlite_edges(&tx, &item.realm_id, &item.namespace, None)?;
            enrich_item_transition_facts(
                previous.as_ref(),
                &mut item,
                items.iter(),
                edges.iter(),
                &mut item_event,
            )?;
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
        self.with_connection(|conn| list_sqlite_attention(conn, &filter, None, None))
    }

    async fn list_attention_bounded(
        &self,
        filter: AttentionListRequest,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_attention(conn, &filter, Some(limit), None))
    }

    async fn list_attention_matching_bounded(
        &self,
        filter: AttentionListRequest,
        observed_at: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
        self.with_connection(|conn| {
            list_sqlite_attention(conn, &filter, Some(limit), Some(observed_at))
        })
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

    async fn list_public_events(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
        self.with_connection(|conn| list_sqlite_public_events(conn, &filter))
    }

    async fn latest_event_seq(
        &self,
        filter: WorkGraphEventFilter,
    ) -> Result<Option<i64>, WorkGraphError> {
        self.with_connection(|conn| latest_sqlite_event_seq(conn, &filter))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_released_0_8_15_workgraph_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_workgraph_schema(tx)?;
    migration_0002_attention_query_columns(tx)
}

#[cfg(not(target_arch = "wasm32"))]
const RELEASED_0_8_15_WORKGRAPH_OBJECTS: &[meerkat_sqlite::SchemaObject] = &[
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "workgraph_items",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_workgraph_items_realm_namespace_updated",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "workgraph_attention",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_workgraph_attention_realm_namespace_updated",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_workgraph_attention_scope_status",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "workgraph_edges",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: "workgraph_events",
    },
    meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Index,
        name: "idx_workgraph_events_realm_namespace_seq",
    },
];

#[cfg(not(target_arch = "wasm32"))]
fn verify_released_0_8_15_workgraph_schema(conn: &Connection) -> Result<(), String> {
    meerkat_sqlite::verify_released_schema_fingerprint(
        conn,
        &WORKGRAPH_DOMAIN,
        RELEASED_0_8_15_WORKGRAPH_OBJECTS,
        build_released_0_8_15_workgraph_schema,
    )
}

#[cfg(not(target_arch = "wasm32"))]
/// The workgraph store's schema domain in the per-file migration ledger.
///
/// Migration 0001 is the base DDL; 0002 lifts the historical attention
/// query-column upgrade (previously re-run on every open, idempotent only
/// via "duplicate column name" error matching) into a once-per-file,
/// transaction-wrapped migration with a `table_info` guard.
#[cfg(not(target_arch = "wasm32"))]
pub const WORKGRAPH_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "workgraph",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_workgraph_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "attention-query-columns",
            apply: migration_0002_attention_query_columns,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "execution-bindings",
            apply: migration_0003_execution_bindings,
        },
    ],
    initialize_current: initialize_current_workgraph_schema,
    allowed_existing_versions: &[2, 3],
    bridge_recoverable_versions: &[1, 2],
    released_predecessors: &[meerkat_sqlite::SchemaPredecessor {
        version: 2,
        verify: verify_released_0_8_15_workgraph_schema,
    }],
    owned_objects: &[
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "workgraph_items",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_items_realm_namespace_updated",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "workgraph_attention",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_attention_realm_namespace_updated",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_attention_scope_status",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "workgraph_edges",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "workgraph_execution_bindings",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_execution_bindings_item",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_execution_bindings_root",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_execution_bindings_supersedes",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_execution_bindings_target_run",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_execution_bindings_recovery",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Table,
            name: "workgraph_events",
        },
        meerkat_sqlite::SchemaObject {
            kind: meerkat_sqlite::SchemaObjectKind::Index,
            name: "idx_workgraph_events_realm_namespace_seq",
        },
    ],
    retired_objects: &[],
};

#[cfg(not(target_arch = "wasm32"))]
fn initialize_current_workgraph_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_workgraph_schema(tx)?;
    migration_0002_attention_query_columns(tx)?;
    migration_0003_execution_bindings(tx)
}

#[cfg(not(target_arch = "wasm32"))]
fn migration_0003_execution_bindings(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        r"
        CREATE TABLE IF NOT EXISTS workgraph_execution_bindings (
            realm_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            binding_id TEXT NOT NULL,
            item_id TEXT NOT NULL,
            supersedes_binding_id TEXT,
            idempotency_key TEXT NOT NULL,
            target_run_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            recovery_pending INTEGER NOT NULL CHECK (recovery_pending IN (0, 1)),
            created_at_utc TEXT NOT NULL,
            binding_json TEXT NOT NULL,
            PRIMARY KEY (realm_id, namespace, binding_id),
            UNIQUE (realm_id, namespace, item_id, idempotency_key),
            UNIQUE (realm_id, namespace, target_run_id)
        );
        CREATE INDEX IF NOT EXISTS idx_workgraph_execution_bindings_item
            ON workgraph_execution_bindings
                (realm_id, namespace, item_id, created_at_utc, binding_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_workgraph_execution_bindings_root
            ON workgraph_execution_bindings (realm_id, namespace, item_id)
            WHERE supersedes_binding_id IS NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_workgraph_execution_bindings_supersedes
            ON workgraph_execution_bindings (realm_id, namespace, supersedes_binding_id)
            WHERE supersedes_binding_id IS NOT NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_workgraph_execution_bindings_target_run
            ON workgraph_execution_bindings (target_run_id);
        CREATE INDEX IF NOT EXISTS idx_workgraph_execution_bindings_recovery
            ON workgraph_execution_bindings
                (realm_id, recovery_pending, created_at_utc, binding_id);
        ",
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn migration_0001_workgraph_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
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

/// One-time migration adding the indexed `status` / `target_key` query
/// columns to `workgraph_attention` (SQL filter pushdown + the
/// active-binding-per-target occupancy guard) and backfilling existing rows.
/// The current direct initializer composes this with the historical base DDL.
/// Ledger v1 is below the supported floor and is refused rather than inferred
/// or upgraded.
#[cfg(not(target_arch = "wasm32"))]
fn migration_0002_attention_query_columns(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    let existing: Vec<String> = tx
        .prepare("PRAGMA table_info(workgraph_attention)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    if !existing.iter().any(|name| name == "status") {
        tx.execute("ALTER TABLE workgraph_attention ADD COLUMN status TEXT", [])?;
    }
    if !existing.iter().any(|name| name == "target_key") {
        tx.execute(
            "ALTER TABLE workgraph_attention ADD COLUMN target_key TEXT",
            [],
        )?;
    }
    let backfill: Vec<(String, String, String, WorkAttentionBinding)> = {
        let mut stmt = tx.prepare(
            "SELECT realm_id, namespace, binding_id, attention_json
               FROM workgraph_attention
              WHERE status IS NULL OR target_key IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row_json::<WorkAttentionBinding>(row, 3)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (realm_id, namespace, binding_id, binding) in backfill {
        tx.execute(
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
        )?;
    }
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_workgraph_attention_scope_status
             ON workgraph_attention (realm_id, namespace, status, target_key)",
        [],
    )?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn pre_0_8_10_attention_import_error(
    binding_id: &str,
    detail: impl std::fmt::Display,
) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("pre-v0.8.10 workgraph attention row `{binding_id}`: {detail}"),
    )))
}

/// Reconcile the exact v2 attention projection published before the schema
/// ledger existed.
///
/// The explicit maintenance bridge authenticates the catalog before calling
/// this function. A physical v1 source has no projection columns and is left
/// for migration 0002. A physical v2 source is data-bearing: every non-NULL
/// projection must already agree with its typed `attention_json` authority,
/// while NULL projections written by an older mixed-version process are
/// backfilled. Any disagreement is refused inside the bridge transaction.
#[cfg(not(target_arch = "wasm32"))]
pub fn prepare_pre_0_8_10_workgraph_attention(
    tx: &Transaction<'_>,
) -> Result<meerkat_sqlite::MaintenancePrepareReport, rusqlite::Error> {
    let columns = tx
        .prepare("PRAGMA table_info(workgraph_attention)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let has_status = columns.iter().any(|name| name == "status");
    let has_target_key = columns.iter().any(|name| name == "target_key");
    match (has_status, has_target_key) {
        (false, false) => {
            return Ok(meerkat_sqlite::MaintenancePrepareReport::default());
        }
        (true, true) => {}
        _ => {
            return Err(pre_0_8_10_attention_import_error(
                "<catalog>",
                "status and target_key projection columns are not an exact pair",
            ));
        }
    }

    struct ProjectionRepair {
        realm_id: String,
        namespace: String,
        binding_id: String,
        source_status: Option<String>,
        source_target_key: Option<String>,
        expected_status: String,
        expected_target_key: String,
    }

    let repairs = {
        let mut statement = tx.prepare(
            "SELECT realm_id, namespace, binding_id, attention_json, status, target_key
               FROM workgraph_attention
              ORDER BY realm_id, namespace, binding_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut repairs = Vec::new();
        for row in rows {
            let (realm_id, namespace, binding_id, attention_json, status, target_key) = row?;
            let binding: WorkAttentionBinding = serde_json::from_str(&attention_json)
                .map_err(|error| pre_0_8_10_attention_import_error(&binding_id, error))?;
            let expected_status = binding.status.status_key().to_string();
            let expected_target_key = binding.target.target_key();
            if status
                .as_deref()
                .is_some_and(|value| value != expected_status)
            {
                return Err(pre_0_8_10_attention_import_error(
                    &binding_id,
                    format!(
                        "status projection `{}` disagrees with typed authority `{expected_status}`",
                        status.as_deref().unwrap_or_default()
                    ),
                ));
            }
            if target_key
                .as_deref()
                .is_some_and(|value| value != expected_target_key)
            {
                return Err(pre_0_8_10_attention_import_error(
                    &binding_id,
                    format!(
                        "target_key projection `{}` disagrees with typed authority `{expected_target_key}`",
                        target_key.as_deref().unwrap_or_default()
                    ),
                ));
            }
            if status.is_none() || target_key.is_none() {
                repairs.push(ProjectionRepair {
                    realm_id,
                    namespace,
                    binding_id,
                    source_status: status,
                    source_target_key: target_key,
                    expected_status,
                    expected_target_key,
                });
            }
        }
        repairs
    };

    let changed = repairs.len();
    for repair in repairs {
        let updated = tx.execute(
            "UPDATE workgraph_attention
                SET status = ?4, target_key = ?5
              WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3
                AND status IS ?6 AND target_key IS ?7",
            params![
                repair.realm_id,
                repair.namespace,
                repair.binding_id,
                repair.expected_status,
                repair.expected_target_key,
                repair.source_status,
                repair.source_target_key,
            ],
        )?;
        if updated != 1 {
            return Err(pre_0_8_10_attention_import_error(
                &repair.binding_id,
                "source projection changed inside the maintenance transaction",
            ));
        }
    }

    Ok(meerkat_sqlite::MaintenancePrepareReport::rewrote(changed))
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
fn insert_execution_binding_tx(
    tx: &Transaction<'_>,
    binding: &WorkExecutionBinding,
) -> Result<(), WorkGraphError> {
    let json =
        serde_json::to_string(binding).map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let recovery_pending = execution_recovery_pending(binding)?;
    tx.execute(
        "INSERT INTO workgraph_execution_bindings
            (realm_id, namespace, binding_id, item_id, supersedes_binding_id,
             idempotency_key, target_run_id, revision, recovery_pending,
             created_at_utc, binding_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            binding.work_ref.realm_id,
            binding.work_ref.namespace.as_str(),
            binding.binding_id.as_str(),
            binding.work_ref.item_id.as_str(),
            binding
                .supersedes
                .as_ref()
                .map(WorkExecutionBindingId::as_str),
            binding.idempotency_key,
            binding.target.run_id(),
            binding.machine_state.revision,
            recovery_pending,
            binding.created_at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|error| {
        if sqlite_constraint_violation(&error) {
            WorkGraphError::Conflict(format!(
                "work execution binding {} conflicts with the existing execution chain",
                binding.binding_id
            ))
        } else {
            WorkGraphError::Store(error.to_string())
        }
    })?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn upsert_execution_binding_tx(
    tx: &Transaction<'_>,
    binding: &WorkExecutionBinding,
) -> Result<(), WorkGraphError> {
    let json =
        serde_json::to_string(binding).map_err(|error| WorkGraphError::Store(error.to_string()))?;
    let recovery_pending = execution_recovery_pending(binding)?;
    tx.execute(
        "INSERT INTO workgraph_execution_bindings
            (realm_id, namespace, binding_id, item_id, supersedes_binding_id,
             idempotency_key, target_run_id, revision, recovery_pending,
             created_at_utc, binding_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(realm_id, namespace, binding_id) DO UPDATE SET
            revision = excluded.revision,
            recovery_pending = excluded.recovery_pending,
            binding_json = excluded.binding_json",
        params![
            binding.work_ref.realm_id,
            binding.work_ref.namespace.as_str(),
            binding.binding_id.as_str(),
            binding.work_ref.item_id.as_str(),
            binding
                .supersedes
                .as_ref()
                .map(WorkExecutionBindingId::as_str),
            binding.idempotency_key,
            binding.target.run_id(),
            binding.machine_state.revision,
            recovery_pending,
            binding.created_at.to_rfc3339(),
            json,
        ],
    )
    .map_err(|error| WorkGraphError::Store(error.to_string()))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn execution_recovery_pending(binding: &WorkExecutionBinding) -> Result<i64, WorkGraphError> {
    Ok(i64::from(!crate::WorkExecutionMachine::retry_eligible(
        binding,
    )?))
}

#[cfg(not(target_arch = "wasm32"))]
fn select_execution_binding(
    conn: &Connection,
    realm_id: &str,
    namespace: &WorkNamespace,
    binding_id: &WorkExecutionBindingId,
) -> Result<Option<WorkExecutionBinding>, WorkGraphError> {
    conn.query_row(
        "SELECT binding_json FROM workgraph_execution_bindings
         WHERE realm_id = ?1 AND namespace = ?2 AND binding_id = ?3",
        params![realm_id, namespace.as_str(), binding_id.as_str()],
        |row| row_json(row, 0),
    )
    .optional()
    .map_err(|err| WorkGraphError::Store(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn list_sqlite_execution_bindings(
    conn: &Connection,
    filter: &WorkExecutionBindingFilter,
) -> Result<Vec<WorkExecutionBinding>, WorkGraphError> {
    let mut stmt = conn
        .prepare(
            "SELECT binding_json FROM workgraph_execution_bindings
             ORDER BY created_at_utc ASC, binding_id ASC",
        )
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| row_json::<WorkExecutionBinding>(row, 0))
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let mut all = Vec::new();
    for row in rows {
        all.push(row.map_err(|err| WorkGraphError::Store(err.to_string()))?);
    }
    let superseded = all
        .iter()
        .filter_map(|binding| {
            binding.supersedes.clone().map(|supersedes| {
                (
                    binding.work_ref.realm_id.clone(),
                    binding.work_ref.namespace.clone(),
                    supersedes,
                )
            })
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut bindings = all
        .into_iter()
        .filter(|binding| execution_binding_matches_filter(binding, filter, &superseded))
        .collect::<Vec<_>>();
    if let Some(limit) = filter.limit {
        bindings.truncate(limit);
    }
    Ok(bindings)
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
    effective_at: Option<DateTime<Utc>>,
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
        match (status, effective_at) {
            (WorkAttentionStatus::Active, Some(_)) => {
                clauses.push(
                    "(status NOT IN ('superseded', 'stopped') OR status IS NULL)".to_string(),
                );
            }
            (WorkAttentionStatus::Paused { .. }, Some(_)) => {
                clauses.push(
                    "(status = 'paused' OR status NOT IN ('active', 'paused', 'superseded', 'stopped') OR status IS NULL)"
                        .to_string(),
                );
            }
            (WorkAttentionStatus::Superseded, Some(_)) => {
                clauses.push(
                    "(status = 'superseded' OR status NOT IN ('active', 'paused', 'superseded', 'stopped') OR status IS NULL)"
                        .to_string(),
                );
            }
            (WorkAttentionStatus::Stopped, Some(_)) => {
                clauses.push(
                    "(status = 'stopped' OR status NOT IN ('active', 'paused', 'superseded', 'stopped') OR status IS NULL)"
                        .to_string(),
                );
            }
            _ => {
                params.push(Box::new(status.status_key().to_string()));
                clauses.push(format!("(status = ?{} OR status IS NULL)", params.len()));
            }
        }
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
        let matches = attention_matches_non_status_filter(&binding, filter)
            && match (filter.status.as_ref(), effective_at) {
                (Some(status), Some(observed_at)) => {
                    WorkAttentionMachine::matches_status_filter_at(&binding, status, observed_at)?
                }
                (Some(status), None) => attention_status_matches_filter(&binding.status, status),
                (None, _) => true,
            };
        if matches {
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
fn list_sqlite_public_events(
    conn: &Connection,
    filter: &WorkGraphEventFilter,
) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
    let limit = filter.limit.unwrap_or(usize::MAX);
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut clauses = vec![
        "event_kind != 'ExecutionBound'".to_string(),
        "event_kind != 'ExecutionTransitioned'".to_string(),
    ];
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
    params.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));
    let sql = format!(
        "SELECT seq, event_json FROM workgraph_events WHERE {} ORDER BY seq ASC LIMIT ?{}",
        clauses.join(" AND "),
        params.len()
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            let seq = row.get::<_, i64>(0)?;
            let mut event = row_json::<WorkGraphEvent>(row, 1)?;
            event.seq = Some(seq);
            Ok(event)
        })
        .map_err(|err| WorkGraphError::Store(err.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| WorkGraphError::Store(err.to_string()))
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
        WorkGraphEventKind::ExecutionBound => {
            let binding = payload_field::<WorkExecutionBinding>(event, "execution_binding")?;
            let commit = crate::WorkExecutionMachine::prepare_bind(binding.clone())?;
            if commit.binding() != &binding {
                return Err(WorkGraphError::Store(format!(
                    "execution bind event for {} changed during authority validation",
                    binding.binding_id
                )));
            }
            validate_execution_event_scope(event, &binding)?;
            let item = select_item(
                tx,
                &binding.work_ref.realm_id,
                &binding.work_ref.namespace,
                &binding.work_ref.item_id,
            )?
            .ok_or_else(|| {
                WorkGraphError::Store(format!(
                    "execution bind for {} references a missing work item",
                    binding.binding_id
                ))
            })?;
            let bindings = list_sqlite_execution_bindings(
                tx,
                &WorkExecutionBindingFilter {
                    realm_id: Some(binding.work_ref.realm_id.clone()),
                    namespace: Some(binding.work_ref.namespace.clone()),
                    item_id: Some(binding.work_ref.item_id.clone()),
                    current_only: false,
                    limit: None,
                },
            )?;
            validate_execution_binding_insert(
                &binding,
                item.revision,
                std::iter::once(&item),
                bindings.iter(),
            )?;
            insert_execution_binding_tx(tx, &binding)
        }
        WorkGraphEventKind::ExecutionTransitioned => {
            let binding = payload_field::<WorkExecutionBinding>(event, "execution_binding")?;
            let observation =
                payload_field::<crate::WorkExecutionObservation>(event, "observation")?;
            validate_execution_event_scope(event, &binding)?;
            let current = select_execution_binding(
                tx,
                &binding.work_ref.realm_id,
                &binding.work_ref.namespace,
                &binding.binding_id,
            )?
            .ok_or_else(|| {
                WorkGraphError::Store(format!(
                    "execution transition for {} precedes its bind event",
                    binding.binding_id
                ))
            })?;
            let commit = crate::WorkExecutionMachine::prepare_observation(
                current.clone(),
                current.machine_state.revision,
                observation,
            )?;
            if commit.binding() != &binding {
                return Err(WorkGraphError::Store(format!(
                    "execution transition event for {} is not the exact generated machine result",
                    binding.binding_id
                )));
            }
            upsert_execution_binding_tx(tx, &binding)
        }
        WorkGraphEventKind::Created
        | WorkGraphEventKind::Updated
        | WorkGraphEventKind::ReadinessObserved
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
fn validate_execution_event_scope(
    event: &WorkGraphEvent,
    binding: &WorkExecutionBinding,
) -> Result<(), WorkGraphError> {
    if event.realm_id != binding.work_ref.realm_id
        || event.namespace != binding.work_ref.namespace
        || event.item_id.as_ref() != Some(&binding.work_ref.item_id)
    {
        return Err(WorkGraphError::Store(format!(
            "execution event scope does not match binding {}",
            binding.binding_id
        )));
    }
    Ok(())
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
        WorkCompletionPolicy, WorkEdgeKind, WorkExecutionBinding, WorkExecutionBindingId,
        WorkExecutionMachine, WorkExecutionObservation, WorkExecutionTarget, WorkGraphError,
        WorkGraphEvent, WorkGraphEventFilter, WorkGraphEventKind, WorkGraphService, WorkGraphStore,
        WorkItemFilter, WorkItemId, WorkItemRef, WorkNamespace,
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

    async fn stale_execution_commit_is_refused(store: std::sync::Arc<dyn WorkGraphStore>) {
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let item = service
            .create(CreateWorkItemRequest {
                title: "immutable execution".to_string(),
                ..Default::default()
            })
            .await
            .expect("item");
        let binding_id = WorkExecutionBindingId::new("execution-immutable").expect("binding id");
        let target = WorkExecutionTarget::mob_flow(
            "mob",
            "flow",
            format!("sha256:{}", "c".repeat(64)),
            "46371bce-c308-58a4-bf0b-0a262de45c12",
            crate::WorkExecutionAuthority::TargetOwner,
            json!({}),
        )
        .expect("target");
        let (machine_state, _) =
            WorkExecutionMachine::bind(&binding_id, target.run_id()).expect("bind machine");
        let bound = service
            .bind_execution(
                WorkExecutionBinding {
                    binding_id,
                    work_ref: WorkItemRef {
                        realm_id: item.realm_id.clone(),
                        namespace: item.namespace.clone(),
                        item_id: item.id.clone(),
                    },
                    target,
                    idempotency_key: "original-key".to_string(),
                    correlation_id: "f9ae62da-662f-5c50-940e-442c529d8e1d".to_string(),
                    supersedes: None,
                    machine_state,
                    created_at: Utc::now(),
                },
                item.revision,
            )
            .await
            .expect("bind execution")
            .binding;
        let commit = WorkExecutionMachine::prepare_observation(
            bound.clone(),
            bound.machine_state.revision,
            WorkExecutionObservation::FlowRunning,
        )
        .expect("machine-minted next-state authority");
        service
            .observe_execution(
                Some(bound.work_ref.realm_id.clone()),
                Some(bound.work_ref.namespace.clone()),
                bound.binding_id.clone(),
                bound.machine_state.revision,
                WorkExecutionObservation::FlowRunning,
            )
            .await
            .expect("commit competing transition");
        let event = WorkGraphEvent::item(
            item.realm_id,
            item.namespace,
            item.id,
            WorkGraphEventKind::ExecutionTransitioned,
            Utc::now(),
            json!({
                "execution_binding": commit.binding(),
                "observation": WorkExecutionObservation::FlowRunning,
            }),
        );
        let error = store
            .update_execution_binding_cas(commit, bound.machine_state.revision, event)
            .await
            .expect_err("store must reject a commit minted from a stale predecessor");
        assert!(matches!(error, WorkGraphError::Conflict(_)));
    }

    async fn duplicate_execution_run_is_refused(store: std::sync::Arc<dyn WorkGraphStore>) {
        let service = WorkGraphService::with_scope(store, "realm", WorkNamespace::default());
        let first_item = service
            .create(CreateWorkItemRequest {
                title: "first execution".to_string(),
                ..Default::default()
            })
            .await
            .expect("first item");
        let second_item = service
            .create(CreateWorkItemRequest {
                title: "second execution".to_string(),
                ..Default::default()
            })
            .await
            .expect("second item");
        let run_id = "24d61f25-09db-5327-99e7-63d7390a1e95";

        for (index, item) in [first_item, second_item].into_iter().enumerate() {
            let binding_id =
                WorkExecutionBindingId::new(format!("execution-run-{index}")).expect("binding id");
            let target = WorkExecutionTarget::mob_flow(
                "mob",
                "flow",
                format!("sha256:{}", "d".repeat(64)),
                run_id,
                crate::WorkExecutionAuthority::TargetOwner,
                json!({}),
            )
            .expect("target");
            let (machine_state, _) =
                WorkExecutionMachine::bind(&binding_id, target.run_id()).expect("bind machine");
            let result = service
                .bind_execution(
                    WorkExecutionBinding {
                        binding_id,
                        work_ref: WorkItemRef {
                            realm_id: item.realm_id,
                            namespace: item.namespace,
                            item_id: item.id,
                        },
                        target,
                        idempotency_key: format!("run-key-{index}"),
                        correlation_id: if index == 0 {
                            "e25abdd9-29cf-56e3-9402-e86c78feec27".to_string()
                        } else {
                            "e8c85639-aa77-5d9b-ad77-b13e29675a21".to_string()
                        },
                        supersedes: None,
                        machine_state,
                        created_at: Utc::now(),
                    },
                    item.revision,
                )
                .await;
            if index == 0 {
                result.expect("first run binding");
            } else {
                assert!(matches!(result, Err(WorkGraphError::Conflict(_))));
            }
        }
    }

    #[tokio::test]
    async fn memory_store_rejects_stale_execution_commit() {
        stale_execution_commit_is_refused(std::sync::Arc::new(MemoryWorkGraphStore::new())).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_store_rejects_stale_execution_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        stale_execution_commit_is_refused(std::sync::Arc::new(
            crate::SqliteWorkGraphStore::open(dir.path().join("workgraph.sqlite3"))
                .expect("sqlite store"),
        ))
        .await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_public_event_limit_is_applied_after_internal_visibility_filter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = crate::SqliteWorkGraphStore::open(dir.path().join("workgraph.sqlite3"))
            .expect("sqlite store");
        let namespace = WorkNamespace::default();
        let event = |kind| {
            WorkGraphEvent::graph(
                "realm".to_string(),
                namespace.clone(),
                kind,
                Utc::now(),
                json!({}),
            )
        };
        store
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|error| WorkGraphError::Store(error.to_string()))?;
                super::insert_event_tx(&tx, &event(WorkGraphEventKind::Created))?;
                for _ in 0..300 {
                    super::insert_event_tx(&tx, &event(WorkGraphEventKind::ExecutionTransitioned))?;
                }
                super::insert_event_tx(&tx, &event(WorkGraphEventKind::EvidenceAdded))?;
                tx.commit()
                    .map_err(|error| WorkGraphError::Store(error.to_string()))
            })
            .expect("insert event history");

        let public = store
            .list_public_events(WorkGraphEventFilter {
                realm_id: Some("realm".to_string()),
                namespace: Some(namespace),
                after_seq: Some(1),
                limit: Some(1),
                ..WorkGraphEventFilter::default()
            })
            .await
            .expect("public event page");
        assert_eq!(public.len(), 1);
        assert_eq!(public[0].kind, WorkGraphEventKind::EvidenceAdded);
        assert_eq!(public[0].seq, Some(302));
    }

    #[tokio::test]
    async fn memory_store_rejects_cross_item_run_reuse() {
        duplicate_execution_run_is_refused(std::sync::Arc::new(MemoryWorkGraphStore::new())).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn sqlite_store_rejects_cross_item_run_reuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        duplicate_execution_run_is_refused(std::sync::Arc::new(
            crate::SqliteWorkGraphStore::open(dir.path().join("workgraph.sqlite3"))
                .expect("sqlite store"),
        ))
        .await;
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
    async fn sqlite_rebuild_restores_execution_machine_state_from_events() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = std::sync::Arc::new(
            crate::SqliteWorkGraphStore::open(temp.path().join("workgraph.db"))
                .expect("sqlite store"),
        );
        let service =
            WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
        let item = service
            .create(CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "durable execution".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: Default::default(),
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            })
            .await
            .expect("item");
        let binding_id = WorkExecutionBindingId::new("execution-sqlite").expect("binding id");
        let target = WorkExecutionTarget::mob_flow(
            "mob",
            "flow",
            format!("sha256:{}", "b".repeat(64)),
            "d8bb76bb-40e8-54f7-b859-d02827f7d296",
            crate::WorkExecutionAuthority::TargetOwner,
            json!({}),
        )
        .expect("target");
        let (machine_state, _) =
            WorkExecutionMachine::bind(&binding_id, target.run_id()).expect("machine bind");
        let bound = service
            .bind_execution(
                WorkExecutionBinding {
                    binding_id,
                    work_ref: WorkItemRef {
                        realm_id: item.realm_id.clone(),
                        namespace: item.namespace.clone(),
                        item_id: item.id.clone(),
                    },
                    target,
                    idempotency_key: "sqlite-key".to_string(),
                    correlation_id: "6084cb0d-f5df-5814-aad9-c8c6c763ef54".to_string(),
                    supersedes: None,
                    machine_state,
                    created_at: Utc::now(),
                },
                item.revision,
            )
            .await
            .expect("bind");
        let running = service
            .observe_execution(
                Some(item.realm_id.clone()),
                Some(item.namespace.clone()),
                bound.binding.binding_id,
                1,
                WorkExecutionObservation::FlowRunning,
            )
            .await
            .expect("running");
        assert_eq!(running.binding.machine_state.revision, 2);

        store
            .rebuild_projection_from_events()
            .expect("rebuild projections");
        let restored = service
            .execution_binding(
                Some(item.realm_id),
                Some(item.namespace),
                running.binding.binding_id,
            )
            .await
            .expect("restored binding");
        assert_eq!(restored.machine_state.revision, 2);
        assert_eq!(
            service
                .execution_bindings_for_recovery(Some("realm".to_string()))
                .await
                .expect("active recovery queue")
                .len(),
            1
        );
        let failed = service
            .observe_execution(
                Some(restored.work_ref.realm_id.clone()),
                Some(restored.work_ref.namespace.clone()),
                restored.binding_id.clone(),
                restored.machine_state.revision,
                WorkExecutionObservation::FlowFailed {
                    detail: Some("test failure".to_string()),
                },
            )
            .await
            .expect("observe failure");
        service
            .observe_execution(
                Some(failed.binding.work_ref.realm_id.clone()),
                Some(failed.binding.work_ref.namespace.clone()),
                failed.binding.binding_id,
                failed.binding.machine_state.revision,
                WorkExecutionObservation::FlowFailureEvidenceProjected,
            )
            .await
            .expect("terminal failure");
        assert!(
            service
                .execution_bindings_for_recovery(Some("realm".to_string()))
                .await
                .expect("terminal recovery queue")
                .is_empty()
        );
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
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
    use crate::{AttentionDelegatedAuthority, AttentionProjectionPolicy, WorkAttentionMode};
    use meerkat_core::SessionId;

    fn test_attention(binding_id: &str) -> WorkAttentionBinding {
        WorkAttentionBinding {
            binding_id: WorkAttentionBindingId::new(binding_id).expect("binding id"),
            work_ref: crate::WorkItemRef {
                realm_id: "realm".to_string(),
                namespace: WorkNamespace::default(),
                item_id: WorkItemId::generated(),
            },
            target: crate::WorkAttentionTarget::Session {
                session_id: SessionId::new(),
            },
            mode: WorkAttentionMode::Pursue,
            status: WorkAttentionStatus::Active,
            machine_state: Default::default(),
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn create_unledgered_v2_workgraph(path: &Path) -> Connection {
        let mut conn = Connection::open(path).expect("open raw");
        let tx = conn.transaction().expect("begin schema transaction");
        migration_0001_workgraph_schema(&tx).expect("create v1 workgraph schema");
        migration_0002_attention_query_columns(&tx).expect("create v2 workgraph schema");
        tx.commit().expect("commit v2 workgraph schema");
        conn
    }

    #[test]
    fn explicit_bridge_authenticates_v2_and_repairs_null_attention_projections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let mut conn = create_unledgered_v2_workgraph(&path);
        let binding = test_attention("legacy-v2-binding");
        let expected_status = binding.status.status_key().to_string();
        let expected_target_key = binding.target.target_key();
        conn.execute(
            "INSERT INTO workgraph_attention
                (realm_id, namespace, binding_id, revision, updated_at_utc, attention_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                binding.work_ref.realm_id,
                binding.work_ref.namespace.as_str(),
                binding.binding_id.as_str(),
                binding.machine_state.revision,
                binding.updated_at.to_rfc3339(),
                serde_json::to_string(&binding).expect("serialize binding"),
            ],
        )
        .expect("insert mixed-version row");

        let report = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            &WORKGRAPH_DOMAIN,
            WORKGRAPH_DOMAIN.supported_version(),
            &[1, 2],
            Some(prepare_pre_0_8_10_workgraph_attention),
        )
        .expect("bridge exact v2 catalog");
        assert_eq!(report.from_version, 2);
        assert_eq!(report.to_version, 3);
        assert_eq!(report.prepared, 1);
        let projections = conn
            .query_row(
                "SELECT status, target_key FROM workgraph_attention WHERE binding_id = ?1",
                [binding.binding_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("read repaired projections");
        assert_eq!(projections, (expected_status, expected_target_key));
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, WORKGRAPH_DOMAIN.name).expect("ledger"),
            Some(3)
        );

        let rerun = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            &WORKGRAPH_DOMAIN,
            WORKGRAPH_DOMAIN.supported_version(),
            &[1, 2],
            Some(prepare_pre_0_8_10_workgraph_attention),
        )
        .expect("idempotent target rerun");
        assert_eq!(rerun.from_version, 3);
        assert_eq!(rerun.to_version, 3);
        assert_eq!(rerun.prepared, 0);
    }

    #[test]
    fn explicit_bridge_refuses_non_null_attention_projection_mismatch_without_mutation() {
        for (case, wrong_status, wrong_target) in [
            ("status", Some("stopped"), None),
            ("target_key", None, Some("session:wrong")),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join(format!("workgraph-{case}.sqlite3"));
            let mut conn = create_unledgered_v2_workgraph(&path);
            let binding = test_attention(&format!("legacy-v2-{case}"));
            let expected_status = binding.status.status_key().to_string();
            let expected_target_key = binding.target.target_key();
            let source_status = wrong_status.unwrap_or(&expected_status).to_string();
            let source_target_key = wrong_target.unwrap_or(&expected_target_key).to_string();
            let source_json = serde_json::to_string(&binding).expect("serialize binding");
            conn.execute(
                "INSERT INTO workgraph_attention
                    (realm_id, namespace, binding_id, revision, updated_at_utc, attention_json,
                     status, target_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    binding.work_ref.realm_id,
                    binding.work_ref.namespace.as_str(),
                    binding.binding_id.as_str(),
                    binding.machine_state.revision,
                    binding.updated_at.to_rfc3339(),
                    source_json,
                    source_status,
                    source_target_key,
                ],
            )
            .expect("insert mismatched projection");

            let error = meerkat_sqlite::bridge_unledgered_domain(
                &mut conn,
                &WORKGRAPH_DOMAIN,
                WORKGRAPH_DOMAIN.supported_version(),
                &[1, 2],
                Some(prepare_pre_0_8_10_workgraph_attention),
            )
            .expect_err("non-null projection mismatch must be refused");
            assert!(
                error.to_string().contains("disagrees with typed authority"),
                "unexpected {case} refusal: {error}"
            );
            let unchanged = conn
                .query_row(
                    "SELECT attention_json, status, target_key
                       FROM workgraph_attention WHERE binding_id = ?1",
                    [binding.binding_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .expect("read refused source");
            assert_eq!(unchanged, (source_json, source_status, source_target_key));
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, WORKGRAPH_DOMAIN.name).expect("ledger"),
                None,
                "refused {case} row must not be stamped"
            );
        }
    }

    #[test]
    fn explicit_bridge_refuses_near_miss_v2_catalog_before_preparation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workgraph.sqlite3");
        let mut conn = create_unledgered_v2_workgraph(&path);
        conn.execute_batch(
            "DROP INDEX idx_workgraph_attention_scope_status;
             CREATE INDEX idx_workgraph_attention_scope_status
                 ON workgraph_attention (realm_id, namespace, status);",
        )
        .expect("install near-miss index");

        let error = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            &WORKGRAPH_DOMAIN,
            WORKGRAPH_DOMAIN.supported_version(),
            &[1, 2],
            Some(prepare_pre_0_8_10_workgraph_attention),
        )
        .expect_err("near-miss catalog must be refused");
        assert!(
            error
                .to_string()
                .contains("does not match any authorized source catalog"),
            "unexpected near-miss refusal: {error}"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, WORKGRAPH_DOMAIN.name).expect("ledger"),
            None
        );
        let index_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema
                  WHERE type = 'index' AND name = 'idx_workgraph_attention_scope_status'",
                [],
                |row| row.get(0),
            )
            .expect("near-miss index remains");
        assert!(index_sql.ends_with("(realm_id, namespace, status)"));
    }

    /// The released v2 floor is exact: an unledgered v1 attention table is
    /// refused without schema/data mutation or a ledger stamp.
    #[tokio::test]
    async fn unledgered_legacy_attention_rows_are_refused_unmutated() {
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
                target: crate::WorkAttentionTarget::Session { session_id },
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

        let error = crate::SqliteWorkGraphStore::open(&path)
            .err()
            .expect("unledgered owned workgraph schema must be refused");
        assert!(
            error.to_string().contains("no ledger row"),
            "unexpected refusal: {error}"
        );
        let conn = Connection::open(&path).expect("reopen raw");
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM workgraph_attention", [], |row| {
                row.get(0)
            })
            .expect("legacy row remains");
        assert_eq!(row_count, 1);
        let projected_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('workgraph_attention')
                 WHERE name IN ('status', 'target_key')",
                [],
                |row| row.get(0),
            )
            .expect("legacy columns");
        assert_eq!(projected_columns, 0);
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, WORKGRAPH_DOMAIN.name).expect("ledger"),
            None
        );
    }
}

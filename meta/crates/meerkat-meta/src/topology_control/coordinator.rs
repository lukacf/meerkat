//! Durable same-process coordinator for topology edges spanning two runtimes.
//!
//! Cross-process mutation is intentionally absent. The coordinator receives
//! both live [`UnifiedRuntime`] authorities, evaluates both ABAC policies,
//! journals one decision for both durable intent stores, and uses Meta's
//! namespace-aware bilateral inproc actuator.

use super::*;
use crate::UnifiedRuntime;
use crate::access::{
    ACTION_AGENT_VIEW, ACTION_TOPOLOGY_AUDIT, ACTION_TOPOLOGY_BULK,
    ACTION_TOPOLOGY_CROSS_AUTHORITY, ACTION_TOPOLOGY_VIEW, AccessView,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

const COORDINATOR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, PartialEq, Eq)]
struct LivePairOwner {
    coordinator_id: String,
    path: PathBuf,
}

static LIVE_PAIR_OWNERS: OnceLock<Mutex<BTreeMap<String, LivePairOwner>>> = OnceLock::new();

fn live_pair_owners() -> &'static Mutex<BTreeMap<String, LivePairOwner>> {
    LIVE_PAIR_OWNERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyBilateralPlanRequest {
    pub expected_revisions: BTreeMap<String, u64>,
    pub operations: Vec<TopologyMutation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyBilateralApplyRequest {
    pub expected_revisions: BTreeMap<String, u64>,
    pub idempotency_key: String,
    pub operations: Vec<TopologyMutation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_tier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyBilateralSnapshot {
    pub authorities: Vec<String>,
    pub authority_revisions: BTreeMap<String, u64>,
    pub policies: BTreeMap<String, TopologyControlPolicy>,
    pub nodes: Vec<TopologyNodeSnapshot>,
    pub edges: Vec<TopologyEdgeSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyBilateralPlan {
    pub authority_revisions: BTreeMap<String, u64>,
    pub operations: Vec<TopologyPlannedEdge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CoordinatorDecision {
    Applying,
    RollingBack,
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct CrossIntentMembership {
    added: bool,
    suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BilateralIntentDelta {
    edge: TopologyEdge,
    before: BTreeMap<String, CrossIntentMembership>,
    after: BTreeMap<String, CrossIntentMembership>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BilateralPhysicalBefore {
    edge: TopologyEdge,
    left_half: bool,
    right_half: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingBilateralOperation {
    operation_id: String,
    #[serde(default)]
    audit_record_id: String,
    idempotency_key: String,
    /// The key the apply is filed under, when its scope is not the host's
    /// (absent for the host's keys and for journals from before keys were
    /// scoped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scoped_idempotency_key: Option<String>,
    fingerprint: String,
    actor: String,
    created_at: String,
    reason: Option<String>,
    operations: Vec<TopologyMutation>,
    decision: CoordinatorDecision,
    authority_revisions_before: BTreeMap<String, u64>,
    authority_revisions_after: BTreeMap<String, u64>,
    intent_deltas: Vec<BilateralIntentDelta>,
    physical_before: Vec<BilateralPhysicalBefore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_recovery_error: Option<String>,
    #[serde(default)]
    recovery_attempts: u64,
}

impl PendingBilateralOperation {
    /// The key this operation's idempotency entry is filed under, if any
    /// (see [`filing_key`]).
    fn idempotency_map_key(&self) -> Option<&str> {
        filing_key(
            self.scoped_idempotency_key.as_deref(),
            &self.idempotency_key,
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct BilateralCoordinatorState {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    coordinator_id: String,
    #[serde(default)]
    authorities: Vec<String>,
    #[serde(default)]
    receipts: VecDeque<TopologyOperationReceipt>,
    #[serde(default)]
    operation_records: VecDeque<TopologyOperationRecord>,
    #[serde(default)]
    last_operation_record_seq: u64,
    #[serde(default)]
    idempotency: BTreeMap<String, IdempotencyRecord>,
    #[serde(default)]
    compacted_idempotency: VecDeque<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<PendingBilateralOperation>,
}

struct SameProcessTopologyCoordinatorInner {
    path: PathBuf,
    coordinator_id: String,
    bound_pair_key: Mutex<Option<String>>,
    state: tokio::sync::RwLock<BilateralCoordinatorState>,
    mutation: tokio::sync::Mutex<()>,
    pair_reconciled: AtomicBool,
    last_clean_reconcile_error: tokio::sync::RwLock<Option<String>>,
    _lock_file: std::fs::File,
}

impl Drop for SameProcessTopologyCoordinatorInner {
    fn drop(&mut self) {
        let pair_key = self
            .bound_pair_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(pair_key) = pair_key else {
            return;
        };
        let owner = LivePairOwner {
            coordinator_id: self.coordinator_id.clone(),
            path: self.path.clone(),
        };
        let mut owners = live_pair_owners()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owners.get(&pair_key) == Some(&owner) {
            owners.remove(&pair_key);
        }
    }
}

/// Durable coordinator for a pair of Meta runtimes in the same process.
///
/// Opening the same journal twice fails closed. This guarantees one owner for
/// idempotency and crash recovery and prevents two host components from
/// independently driving the same authority pair.
#[derive(Clone)]
pub struct SameProcessTopologyCoordinator {
    inner: Arc<SameProcessTopologyCoordinatorInner>,
}

impl std::fmt::Debug for SameProcessTopologyCoordinator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SameProcessTopologyCoordinator")
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

impl SameProcessTopologyCoordinator {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TopologyControlError> {
        let path = canonical_coordinator_path(path.into())?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
        }
        let lock_path = path.with_extension("json.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
        lock_file.try_lock_exclusive().map_err(|error| {
            TopologyControlError::Persistence(format!(
                "bilateral topology journal is already owned at {}: {error}",
                path.display()
            ))
        })?;
        let mut state = if path.is_file() {
            let bytes = std::fs::read(&path)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?
        } else {
            BilateralCoordinatorState::default()
        };
        if state.schema_version > COORDINATOR_SCHEMA_VERSION {
            return Err(TopologyControlError::Persistence(format!(
                "unsupported bilateral topology journal schema {} (maximum {})",
                state.schema_version, COORDINATOR_SCHEMA_VERSION
            )));
        }
        state.schema_version = COORDINATOR_SCHEMA_VERSION;
        drop_unscoped_reserved_entries(&mut state.idempotency);
        if state.coordinator_id.is_empty() {
            state.coordinator_id = uuid::Uuid::new_v4().to_string();
        }
        normalize_operation_record_sequences(
            &mut state.operation_records,
            &mut state.last_operation_record_seq,
        );
        let pending_audit = state.pending.as_ref().map(|pending| {
            (
                pending.operation_id.as_str(),
                pending.audit_record_id.as_str(),
                pending.last_recovery_error.is_some(),
            )
        });
        reconcile_persisted_operation_records(
            &mut state.operation_records,
            &state.receipts,
            pending_audit,
        );
        let coordinator_id = state.coordinator_id.clone();
        persist_coordinator_state(&path, &state)?;
        Ok(Self {
            inner: Arc::new(SameProcessTopologyCoordinatorInner {
                path,
                coordinator_id,
                bound_pair_key: Mutex::new(None),
                state: tokio::sync::RwLock::new(state),
                mutation: tokio::sync::Mutex::new(()),
                pair_reconciled: AtomicBool::new(false),
                last_clean_reconcile_error: tokio::sync::RwLock::new(None),
                _lock_file: lock_file,
            }),
        })
    }

    /// Aggregate both authority-local snapshots, including remote node stubs,
    /// cross-edge physical health, and principal-shaped affordances.
    pub async fn query(
        &self,
        left: &UnifiedRuntime,
        right: &UnifiedRuntime,
        principal: Option<&str>,
    ) -> Result<TopologyBilateralSnapshot, TopologyControlError> {
        let _coordinator = self.inner.mutation.lock().await;
        let (first, second) = canonical_runtimes(left, right)?;
        let _first_guard = first.topology_controller().mutation_guard().await;
        let _second_guard = second.topology_controller().mutation_guard().await;
        self.recover_locked(first, second).await?;
        self.query_locked(first, second, principal).await
    }

    pub async fn plan(
        &self,
        left: &UnifiedRuntime,
        right: &UnifiedRuntime,
        request: TopologyBilateralPlanRequest,
        principal: Option<&str>,
    ) -> Result<TopologyBilateralPlan, TopologyControlError> {
        let _coordinator = self.inner.mutation.lock().await;
        let (first, second) = canonical_runtimes(left, right)?;
        let _first_guard = first.topology_controller().mutation_guard().await;
        let _second_guard = second.topology_controller().mutation_guard().await;
        self.recover_locked(first, second).await?;
        let operations = normalize_bilateral_operations(first, second, request.operations)?;
        validate_unique_operations(&operations)?;
        validate_pair_policies(first, second, operations.len(), false)?;
        let snapshot = self.query_locked(first, second, principal).await?;
        validate_expected_revisions(&snapshot, &request.expected_revisions)?;
        validate_bilateral_known_endpoints(&snapshot, &operations)?;
        validate_bilateral_connect_does_not_clear_suppression(&snapshot, &operations)?;
        validate_bilateral_reconnect_targets(&snapshot, &operations)?;
        authorize_bilateral(first, second, principal, &operations)?;
        Ok(plan_bilateral(&snapshot, operations))
    }

    pub async fn apply(
        &self,
        left: &UnifiedRuntime,
        right: &UnifiedRuntime,
        request: TopologyBilateralApplyRequest,
        principal: Option<&str>,
        actor: &str,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        let _coordinator = self.inner.mutation.lock().await;
        let (first, second) = canonical_runtimes(left, right)?;
        let _first_guard = first.topology_controller().mutation_guard().await;
        let _second_guard = second.topology_controller().mutation_guard().await;
        self.recover_locked(first, second).await?;
        let mut normalized = request.clone();
        normalized.idempotency_key = normalized.idempotency_key.trim().to_string();
        normalized.operations =
            normalize_bilateral_operations(first, second, normalized.operations)?;
        // The principal's own keys: the same key from another principal, or
        // from the host without one, names a different operation.
        let scope = TopologyApplyScope::for_host_principal(principal);
        let scoped_key = scope.idempotency_key(
            &[first.mob_id().as_str(), second.mob_id().as_str()],
            &normalized.idempotency_key,
        );
        let preflight = validate_unique_operations(&normalized.operations)
            .and_then(|()| validate_pair_policies(first, second, normalized.operations.len(), true))
            .and_then(|()| authorize_bilateral(first, second, principal, &normalized.operations))
            .and_then(|()| scope.validate_key(&normalized.idempotency_key));
        let mut record = operation_record_for_bilateral_request(
            first,
            second,
            &normalized,
            &scoped_key,
            actor,
            principal,
        )?;
        if let Err(error) = &preflight {
            let (status, _) =
                operation_record_outcome(&Err::<TopologyOperationReceipt, _>(error.clone()));
            set_operation_record_status(&mut record, status, Some(error.kind()));
        }
        let record_id = record.record_id.clone();
        self.upsert_operation_record_locked(record, first, second)
            .await?;
        let mut result = if let Err(error) = preflight {
            Err(error)
        } else if let Some(error) = self.inner.last_clean_reconcile_error.read().await.clone() {
            Err(TopologyControlError::Actuator(error))
        } else {
            self.apply_locked(
                first,
                second,
                normalized,
                &scoped_key,
                principal,
                actor,
                &record_id,
            )
            .await
        };
        let (status, error) = operation_record_outcome(&result);
        self.update_operation_record_locked(&record_id, status, error)
            .await?;
        project_bilateral_result_attribution(first, second, principal, &mut result);
        result
    }

    /// Resolve a coordinator-owned operation after an ambiguous/dropped host
    /// response. Pending recovery remains observable even though neither
    /// authority-local controller owns the bilateral journal.
    pub async fn operation(
        &self,
        left: &UnifiedRuntime,
        right: &UnifiedRuntime,
        operation_id: &str,
        principal: Option<&str>,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        let _coordinator = self.inner.mutation.lock().await;
        let (first, second) = canonical_runtimes(left, right)?;
        let _first_guard = first.topology_controller().mutation_guard().await;
        let _second_guard = second.topology_controller().mutation_guard().await;
        self.recover_locked(first, second).await?;
        let state = self.inner.state.read().await;
        let mut receipt = if let Some(receipt) = state
            .receipts
            .iter()
            .find(|receipt| receipt.operation_id == operation_id)
            .cloned()
        {
            receipt
        } else {
            state
                .pending
                .as_ref()
                .filter(|pending| pending.operation_id == operation_id)
                .map(pending_bilateral_receipt)
                .ok_or_else(|| TopologyControlError::OperationNotFound(operation_id.to_string()))?
        };
        let first_view = access_view(first, principal);
        let second_view = access_view(second, principal);
        if receipt
            .results
            .iter()
            .flat_map(|result| [&result.edge.a, &result.edge.b])
            .any(|endpoint| {
                !endpoint_visible(first_view.as_ref(), endpoint)
                    || !endpoint_visible(second_view.as_ref(), endpoint)
            })
        {
            return Err(TopologyControlError::AccessDenied {
                authority: "bilateral".to_string(),
                action: ACTION_TOPOLOGY_VIEW.to_string(),
                identity: "<redacted>".to_string(),
            });
        }
        if !bilateral_attribution_allowed(first, second, principal, &receipt.results) {
            receipt.actor.clear();
        }
        Ok(receipt)
    }

    pub async fn audit(
        &self,
        left: &UnifiedRuntime,
        right: &UnifiedRuntime,
        principal: Option<&str>,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<TopologyAuditPage, TopologyControlError> {
        let _coordinator = self.inner.mutation.lock().await;
        let (first, second) = canonical_runtimes(left, right)?;
        let _first_guard = first.topology_controller().mutation_guard().await;
        let _second_guard = second.topology_controller().mutation_guard().await;
        self.bind_pair_locked(first, second).await?;
        self.reconcile_operation_records_locked().await?;
        let first_view = access_view(first, principal);
        let second_view = access_view(second, principal);
        if [first_view.as_ref(), second_view.as_ref()]
            .into_iter()
            .flatten()
            .any(|view| view.enforced() && !view.may_perform_anywhere(ACTION_TOPOLOGY_AUDIT))
        {
            return Err(TopologyControlError::AccessDenied {
                authority: "bilateral".to_string(),
                action: ACTION_TOPOLOGY_AUDIT.to_string(),
                identity: "<redacted>".to_string(),
            });
        }
        let state = self.inner.state.read().await;
        let page = audit_page(
            &state.operation_records,
            state.last_operation_record_seq,
            after_seq,
            4096,
        )?;
        drop(state);
        let limit = limit.clamp(1, 1024);
        let mut records = Vec::new();
        let mut next_after_seq = page.next_after_seq;
        let mut consumed_all = true;
        for record in page.records {
            next_after_seq = record.seq;
            let visible = record.operations.iter().all(|operation| {
                [&operation.edge.a, &operation.edge.b]
                    .into_iter()
                    .all(|endpoint| {
                        endpoint_visible(first_view.as_ref(), endpoint)
                            && endpoint_visible(second_view.as_ref(), endpoint)
                            && view_allows(
                                first_view.as_ref(),
                                ACTION_TOPOLOGY_AUDIT,
                                endpoint.identity.as_str(),
                            )
                            && view_allows(
                                second_view.as_ref(),
                                ACTION_TOPOLOGY_AUDIT,
                                endpoint.identity.as_str(),
                            )
                    })
            });
            if visible {
                records.push(record);
                if records.len() == limit {
                    consumed_all = false;
                    break;
                }
            }
        }
        Ok(TopologyAuditPage {
            records,
            next_after_seq,
            oldest_available_seq: page.oldest_available_seq,
            latest_seq: page.latest_seq,
            has_more: page.has_more || !consumed_all,
        })
    }

    /// Explicit startup hook. Query/plan/apply call it automatically; hosts
    /// may call it immediately after reconstructing both runtimes to converge
    /// a crash-window journal before serving any UI.
    pub async fn recover(
        &self,
        left: &UnifiedRuntime,
        right: &UnifiedRuntime,
    ) -> Result<Option<TopologyOperationReceipt>, TopologyControlError> {
        let _coordinator = self.inner.mutation.lock().await;
        let (first, second) = canonical_runtimes(left, right)?;
        let _first_guard = first.topology_controller().mutation_guard().await;
        let _second_guard = second.topology_controller().mutation_guard().await;
        self.inner.pair_reconciled.store(false, Ordering::Release);
        let receipt = self.recover_locked(first, second).await?;
        if let Some(error) = self.inner.last_clean_reconcile_error.read().await.clone() {
            return Err(TopologyControlError::Actuator(error));
        }
        Ok(receipt)
    }

    async fn bind_pair_locked(
        &self,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
    ) -> Result<(), TopologyControlError> {
        let pair = vec![first.mob_id(), second.mob_id()];
        let mut state = self.inner.state.read().await.clone();
        if !state.authorities.is_empty() && state.authorities != pair {
            return Err(TopologyControlError::AuthorityMismatch(format!(
                "journal belongs to {:?}, not {:?}",
                state.authorities, pair
            )));
        }
        if state.authorities.is_empty() {
            state.authorities = pair.clone();
            self.persist(&state)?;
            *self.inner.state.write().await = state;
        }

        // The caller-supplied journal path is not itself an ownership key:
        // two paths could otherwise coordinate the same pair independently.
        // Reserve the canonical pair in both durable authority stores. This
        // survives process restart and makes a second path fail closed before
        // it can journal or actuate an operation.
        let pair_key = authority_pair_key(&pair);
        let coordinator_state = self.inner.state.read().await.clone();
        let coordinator_identity = coordinator_state.coordinator_id.clone();
        let expected_binding = BilateralCoordinatorBinding {
            coordinator_id: coordinator_identity.clone(),
            journal_path: self.inner.path.display().to_string(),
        };
        let mut controller_states = Vec::with_capacity(2);
        for runtime in [first, second] {
            let mut controller_state = runtime
                .topology_controller()
                .inner
                .state
                .read()
                .await
                .clone();
            if let Some(existing) = controller_state
                .bilateral_coordinator_bindings
                .get(&pair_key)
            {
                if existing != &expected_binding {
                    return Err(TopologyControlError::AuthorityMismatch(format!(
                        "authority pair {pair:?} has a different bilateral coordinator lease or journal path"
                    )));
                }
                continue;
            }
            controller_state
                .bilateral_coordinator_bindings
                .insert(pair_key.clone(), expected_binding.clone());
            controller_states.push((runtime, controller_state));
        }
        let owner = LivePairOwner {
            coordinator_id: coordinator_identity.clone(),
            path: self.inner.path.clone(),
        };
        {
            let mut owners = live_pair_owners()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = owners.get(&pair_key)
                && existing != &owner
            {
                return Err(TopologyControlError::AuthorityMismatch(format!(
                    "authority pair {pair:?} already has a live bilateral coordinator"
                )));
            }
            owners.insert(pair_key.clone(), owner);
        }
        *self
            .inner
            .bound_pair_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pair_key);
        for (runtime, controller_state) in &controller_states {
            runtime
                .topology_controller()
                .persist_candidate(controller_state)?;
        }
        for (runtime, controller_state) in controller_states {
            *runtime.topology_controller().inner.state.write().await = controller_state;
        }
        Ok(())
    }

    async fn query_locked(
        &self,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
        principal: Option<&str>,
    ) -> Result<TopologyBilateralSnapshot, TopologyControlError> {
        self.bind_pair_locked(first, second).await?;
        // A bilateral snapshot must not surface either authority's
        // uncommitted local write-ahead intent. Both authority locks are
        // already held by the caller, so recover through the unlocked seam.
        first
            .topology_runtime_handle()
            .recover_pending_unlocked()
            .await?;
        second
            .topology_runtime_handle()
            .recover_pending_unlocked()
            .await?;
        let first_snapshot = first.topology_runtime_handle().query_unlocked().await?;
        let second_snapshot = second.topology_runtime_handle().query_unlocked().await?;
        let mut nodes = first_snapshot.nodes;
        nodes.extend(second_snapshot.nodes);
        nodes.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
        nodes.dedup_by(|a, b| a.endpoint == b.endpoint);

        let mut edge_map = BTreeMap::<TopologyEdge, TopologyEdgeSnapshot>::new();
        for edge in first_snapshot
            .edges
            .into_iter()
            .chain(second_snapshot.edges)
        {
            edge_map
                .entry(edge.edge.clone())
                .and_modify(|existing| {
                    existing.actual |= edge.actual;
                    existing.declared |= edge.declared;
                    existing.operator_added |= edge.operator_added;
                    existing.suppressed |= edge.suppressed;
                    existing.desired |= edge.desired;
                })
                .or_insert(edge);
        }
        for edge in edge_map.values_mut() {
            if !edge.edge.is_local_to(&first.mob_id())
                && !edge.edge.is_local_to(&second.mob_id())
                && edge_belongs_to_pair(&edge.edge, first, second)
            {
                let (first_endpoint, second_endpoint) =
                    endpoints_for_pair(&edge.edge, first, second)?;
                let physical = first
                    .bilateral_same_process_state(
                        second,
                        &first_endpoint.identity,
                        &second_endpoint.identity,
                    )
                    .await;
                edge.actual = physical
                    .map(|(first_half, second_half)| first_half && second_half)
                    .unwrap_or(false);
            }
        }

        // Durable intent remains visible when a member is temporarily
        // absent. Materialize a denied-by-default stub instead of filtering
        // the incident edge out of the operator's degraded snapshot.
        let live_endpoints = nodes
            .iter()
            .map(|node| node.endpoint.clone())
            .collect::<BTreeSet<_>>();
        for endpoint in edge_map
            .keys()
            .flat_map(|edge| [&edge.a, &edge.b])
            .filter(|endpoint| !live_endpoints.contains(*endpoint))
        {
            nodes.push(TopologyNodeSnapshot {
                endpoint: endpoint.clone(),
                role: "unavailable".to_string(),
                labels: BTreeMap::from([("topology_status".to_string(), "missing".to_string())]),
                affordances: Some(TopologyNodeAffordances {
                    can_connect: false,
                    can_disconnect: false,
                    can_reconnect: false,
                    can_bulk: false,
                    can_cross_authority: false,
                }),
            });
        }
        nodes.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
        nodes.dedup_by(|a, b| a.endpoint == b.endpoint);

        let first_view = access_view(first, principal);
        let second_view = access_view(second, principal);
        nodes.retain(|node| {
            endpoint_visible(first_view.as_ref(), &node.endpoint)
                && endpoint_visible(second_view.as_ref(), &node.endpoint)
        });
        let visible = nodes
            .iter()
            .map(|node| node.endpoint.clone())
            .collect::<BTreeSet<_>>();
        let first_policy = first.topology_controller().policy();
        let second_policy = second.topology_controller().policy();
        for node in &mut nodes {
            if node.labels.get("topology_status").map(String::as_str) == Some("missing") {
                continue;
            }
            let identity = node.endpoint.identity.as_str();
            let action_allowed = |action: &str| {
                first_policy.mode == TopologyControlMode::Editable
                    && second_policy.mode == TopologyControlMode::Editable
                    && first_policy.allow_cross_authority
                    && second_policy.allow_cross_authority
                    && view_allows(first_view.as_ref(), action, identity)
                    && view_allows(second_view.as_ref(), action, identity)
                    && view_allows(
                        first_view.as_ref(),
                        ACTION_TOPOLOGY_CROSS_AUTHORITY,
                        identity,
                    )
                    && view_allows(
                        second_view.as_ref(),
                        ACTION_TOPOLOGY_CROSS_AUTHORITY,
                        identity,
                    )
            };
            node.affordances = Some(TopologyNodeAffordances {
                can_connect: action_allowed(crate::access::ACTION_TOPOLOGY_CONNECT),
                can_disconnect: action_allowed(crate::access::ACTION_TOPOLOGY_DISCONNECT),
                can_reconnect: action_allowed(crate::access::ACTION_TOPOLOGY_RECONNECT),
                can_bulk: first_policy.allow_bulk
                    && second_policy.allow_bulk
                    && action_allowed(ACTION_TOPOLOGY_BULK),
                can_cross_authority: action_allowed(ACTION_TOPOLOGY_CROSS_AUTHORITY),
            });
        }
        let mut edges = edge_map
            .into_values()
            .filter(|edge| visible.contains(&edge.edge.a) && visible.contains(&edge.edge.b))
            .collect::<Vec<_>>();
        edges.sort_by(|a, b| a.edge.cmp(&b.edge));
        let mut authority_revisions = BTreeMap::new();
        authority_revisions.insert(first.mob_id(), first.topology_controller().revision().await);
        authority_revisions.insert(
            second.mob_id(),
            second.topology_controller().revision().await,
        );
        let mut policies = BTreeMap::new();
        policies.insert(first.mob_id(), first_policy);
        policies.insert(second.mob_id(), second_policy);
        Ok(TopologyBilateralSnapshot {
            authorities: vec![first.mob_id(), second.mob_id()],
            authority_revisions,
            policies,
            nodes,
            edges,
        })
    }

    fn persist(&self, state: &BilateralCoordinatorState) -> Result<(), TopologyControlError> {
        persist_coordinator_state(&self.inner.path, state)
    }

    /// Reconcile every durable non-terminal attempt against the one pending
    /// WAL entry and terminal receipt set. Public methods call this while
    /// holding the coordinator and both authority mutation locks, so a
    /// Requested attempt abandoned before it became pending is deterministically
    /// closed as Interrupted in the same live process.
    async fn reconcile_operation_records_locked(&self) -> Result<(), TopologyControlError> {
        let mut state_guard = self.inner.state.write().await;
        let mut next = state_guard.clone();
        let pending_audit = next.pending.as_ref().map(|pending| {
            (
                pending.operation_id.clone(),
                pending.audit_record_id.clone(),
                pending.last_recovery_error.is_some(),
            )
        });
        let before = next.operation_records.clone();
        reconcile_persisted_operation_records(
            &mut next.operation_records,
            &next.receipts,
            pending_audit
                .as_ref()
                .map(|(operation_id, audit_record_id, degraded)| {
                    (operation_id.as_str(), audit_record_id.as_str(), *degraded)
                }),
        );
        if next.operation_records != before {
            self.persist(&next)?;
            *state_guard = next;
        }
        Ok(())
    }

    async fn upsert_operation_record_locked(
        &self,
        mut record: TopologyOperationRecord,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
    ) -> Result<(), TopologyControlError> {
        let mut state_guard = self.inner.state.write().await;
        let mut state = state_guard.clone();
        if state
            .operation_records
            .iter()
            .any(|existing| existing.record_id == record.record_id)
        {
            return Ok(());
        }
        state.last_operation_record_seq = state.last_operation_record_seq.saturating_add(1);
        record.seq = state.last_operation_record_seq;
        state.operation_records.push_back(record);
        let limit = first
            .topology_controller()
            .policy()
            .receipt_limit
            .min(second.topology_controller().policy().receipt_limit)
            .saturating_mul(4)
            .clamp(16, 4096);
        while state.operation_records.len() > limit {
            state.operation_records.pop_front();
        }
        self.persist(&state)?;
        *state_guard = state;
        Ok(())
    }

    async fn update_operation_record_locked(
        &self,
        record_id: &str,
        status: TopologyOperationRecordStatus,
        error: Option<&TopologyControlError>,
    ) -> Result<(), TopologyControlError> {
        let mut state_guard = self.inner.state.write().await;
        let mut state = state_guard.clone();
        let Some(record) = state
            .operation_records
            .iter_mut()
            .find(|record| record.record_id == record_id)
        else {
            return Err(TopologyControlError::Persistence(format!(
                "bilateral topology audit record disappeared before terminal update: {record_id}"
            )));
        };
        set_operation_record_status(record, status, error.map(TopologyControlError::kind));
        self.persist(&state)?;
        *state_guard = state;
        Ok(())
    }

    // Implemented below, kept separate so public methods all establish the
    // same coordinator -> authority-A -> authority-B lock order.
    #[allow(clippy::too_many_arguments)]
    async fn apply_locked(
        &self,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
        mut request: TopologyBilateralApplyRequest,
        scoped_key: &str,
        principal: Option<&str>,
        actor: &str,
        audit_record_id: &str,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        request.idempotency_key = request.idempotency_key.trim().to_string();
        if request.idempotency_key.is_empty() || actor.trim().is_empty() {
            return Err(TopologyControlError::InvalidRequest(
                "idempotency_key and actor must not be empty".to_string(),
            ));
        }
        if let Some(tier) = request.risk_tier.as_deref()
            && !tier.trim().is_empty()
            && !tier.eq_ignore_ascii_case("r0")
        {
            return Err(TopologyControlError::ApprovalUnsupported(tier.to_string()));
        }
        request.operations = normalize_bilateral_operations(first, second, request.operations)?;
        validate_unique_operations(&request.operations)?;
        validate_pair_policies(first, second, request.operations.len(), true)?;
        authorize_bilateral(first, second, principal, &request.operations)?;
        self.bind_pair_locked(first, second).await?;

        let fingerprint = bilateral_fingerprint(&request)?;
        let coordinator_before = self.inner.state.read().await.clone();
        if let Some(pending) = coordinator_before.pending.as_ref() {
            return Err(TopologyControlError::OperationInProgress(
                pending.operation_id.clone(),
            ));
        }
        if let Some(record) = coordinator_before.idempotency.get(scoped_key) {
            if record.fingerprint != fingerprint {
                return Err(TopologyControlError::IdempotencyConflict(
                    request.idempotency_key,
                ));
            }
            return coordinator_before
                .receipts
                .iter()
                .find(|receipt| receipt.operation_id == record.operation_id)
                .cloned()
                .ok_or_else(|| TopologyControlError::IdempotencyReceiptExpired {
                    key: request.idempotency_key,
                    operation_id: record.operation_id.clone(),
                });
        }
        if coordinator_before
            .compacted_idempotency
            .contains(&compacted_key_fingerprint(scoped_key))
        {
            return Err(TopologyControlError::IdempotencyHistoryCompacted(
                request.idempotency_key,
            ));
        }

        let snapshot = self.query_locked(first, second, principal).await?;
        validate_expected_revisions(&snapshot, &request.expected_revisions)?;
        validate_bilateral_known_endpoints(&snapshot, &request.operations)?;
        validate_bilateral_connect_does_not_clear_suppression(&snapshot, &request.operations)?;
        validate_bilateral_reconnect_targets(&snapshot, &request.operations)?;
        let plan = plan_bilateral(&snapshot, request.operations.clone());
        let mut states_before = BTreeMap::new();
        states_before.insert(
            first.mob_id(),
            first.topology_controller().inner.state.read().await.clone(),
        );
        states_before.insert(
            second.mob_id(),
            second
                .topology_controller()
                .inner
                .state
                .read()
                .await
                .clone(),
        );
        if let Some(pending) = states_before
            .values()
            .find_map(|state| state.pending.as_ref())
        {
            return Err(TopologyControlError::OperationInProgress(
                pending.operation_id.clone(),
            ));
        }
        let mut states_after = states_before.clone();
        let mut physical_before = Vec::with_capacity(request.operations.len());
        for operation in &request.operations {
            let (first_endpoint, second_endpoint) =
                endpoints_for_pair(&operation.edge, first, second)?;
            let (first_half, second_half) = first
                .bilateral_same_process_state(
                    second,
                    &first_endpoint.identity,
                    &second_endpoint.identity,
                )
                .await
                .map_err(|error| TopologyControlError::Actuator(error.to_string()))?;
            physical_before.push(BilateralPhysicalBefore {
                edge: operation.edge.clone(),
                left_half: first_half,
                right_half: second_half,
            });
            for state in states_after.values_mut() {
                match operation.action {
                    TopologyAction::Connect | TopologyAction::Reconnect => {
                        state.cross_suppressions.remove(&operation.edge);
                        state.cross_additions.insert(operation.edge.clone());
                    }
                    TopologyAction::Disconnect => {
                        state.cross_additions.remove(&operation.edge);
                        state.cross_suppressions.insert(operation.edge.clone());
                    }
                }
            }
        }
        let physical_change = plan
            .operations
            .iter()
            .any(|operation| operation.requires_physical_change);
        let intent_change = states_after != states_before;
        let changed = intent_change || physical_change;
        for (authority, state) in &mut states_after {
            let before = states_before
                .get(authority)
                .ok_or_else(|| TopologyControlError::AuthorityMismatch(authority.clone()))?;
            state.revision = if changed {
                before.revision.saturating_add(1)
            } else {
                before.revision
            };
        }
        let authority_revisions_before = states_before
            .iter()
            .map(|(authority, state)| (authority.clone(), state.revision))
            .collect::<BTreeMap<_, _>>();
        let authority_revisions_after = states_after
            .iter()
            .map(|(authority, state)| (authority.clone(), state.revision))
            .collect::<BTreeMap<_, _>>();
        let intent_deltas = request
            .operations
            .iter()
            .map(|operation| BilateralIntentDelta {
                edge: operation.edge.clone(),
                before: states_before
                    .iter()
                    .map(|(authority, state)| {
                        (
                            authority.clone(),
                            CrossIntentMembership {
                                added: state.cross_additions.contains(&operation.edge),
                                suppressed: state.cross_suppressions.contains(&operation.edge),
                            },
                        )
                    })
                    .collect(),
                after: states_after
                    .iter()
                    .map(|(authority, state)| {
                        (
                            authority.clone(),
                            CrossIntentMembership {
                                added: state.cross_additions.contains(&operation.edge),
                                suppressed: state.cross_suppressions.contains(&operation.edge),
                            },
                        )
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        let operation_id = operation_id(scoped_key, &fingerprint);
        let created_at = chrono::Utc::now().to_rfc3339();
        if !changed {
            let receipt = bilateral_receipt(
                &operation_id,
                &request.idempotency_key,
                actor,
                TopologyOperationStatus::Noop,
                &created_at,
                request.reason.clone(),
                &plan,
                &authority_revisions_before,
                &authority_revisions_after,
                None,
            );
            self.commit_terminal_receipt(
                coordinator_before,
                states_before,
                receipt.clone(),
                fingerprint,
                Some(scoped_key),
                first,
                second,
            )
            .await?;
            return Ok(receipt);
        }

        let pending = PendingBilateralOperation {
            operation_id: operation_id.clone(),
            audit_record_id: audit_record_id.to_string(),
            idempotency_key: request.idempotency_key.clone(),
            scoped_idempotency_key: (scoped_key != request.idempotency_key)
                .then(|| scoped_key.to_string()),
            fingerprint: fingerprint.clone(),
            actor: actor.to_string(),
            created_at: created_at.clone(),
            reason: request.reason.clone(),
            operations: request.operations.clone(),
            decision: CoordinatorDecision::Applying,
            authority_revisions_before: authority_revisions_before.clone(),
            authority_revisions_after: authority_revisions_after.clone(),
            intent_deltas,
            physical_before: physical_before.clone(),
            last_recovery_error: None,
            recovery_attempts: 0,
        };
        let mut applying = coordinator_before.clone();
        applying.pending = Some(pending.clone());
        if let Some(record) = applying
            .operation_records
            .iter_mut()
            .find(|record| record.record_id == audit_record_id)
        {
            set_operation_record_status(record, TopologyOperationRecordStatus::Pending, None);
        }
        self.persist(&applying)?;
        *self.inner.state.write().await = applying;

        let mut failure = None;
        for operation in &request.operations {
            if let Err(error) = actuate(first, second, operation).await {
                failure = Some(error.to_string());
                break;
            }
        }
        if let Some(error) = failure {
            let mut rolling_back = self.inner.state.read().await.clone();
            if let Some(pending) = rolling_back.pending.as_mut() {
                pending.decision = CoordinatorDecision::RollingBack;
            }
            self.persist(&rolling_back)?;
            *self.inner.state.write().await = rolling_back;
            let rollback_failures = rollback_physical_pair(first, second, &physical_before).await;
            let status = if rollback_failures.is_empty() {
                TopologyOperationStatus::RolledBack
            } else {
                TopologyOperationStatus::PartialDegraded
            };
            let detail = if rollback_failures.is_empty() {
                error
            } else {
                format!(
                    "{error}; rollback failures: {}",
                    rollback_failures.join("; ")
                )
            };
            let receipt = bilateral_receipt(
                &operation_id,
                &request.idempotency_key,
                actor,
                status,
                &created_at,
                request.reason,
                &plan,
                &authority_revisions_before,
                &authority_revisions_before,
                Some(detail.clone()),
            );
            if rollback_failures.is_empty() {
                let coordinator = self.inner.state.read().await.clone();
                self.commit_terminal_receipt(
                    coordinator,
                    states_before,
                    receipt.clone(),
                    fingerprint,
                    Some(scoped_key),
                    first,
                    second,
                )
                .await?;
            } else {
                let mut retryable = self.inner.state.read().await.clone();
                if let Some(pending) = retryable.pending.as_mut() {
                    pending.recovery_attempts = pending.recovery_attempts.saturating_add(1);
                    pending.last_recovery_error = Some(detail.clone());
                }
                if let Some(record) = retryable
                    .operation_records
                    .iter_mut()
                    .find(|record| record.record_id == audit_record_id)
                {
                    set_operation_record_status(
                        record,
                        TopologyOperationRecordStatus::PartialDegraded,
                        Some("topology_actuator_failed"),
                    );
                }
                self.persist(&retryable)?;
                *self.inner.state.write().await = retryable;
            }
            return Err(TopologyControlError::ApplyFailed {
                message: detail,
                receipt: Box::new(receipt),
            });
        }

        // The global commit decision is fsynced before either authority-local
        // intent file. Recovery therefore rolls back Applying and converges
        // Committed, never guessing from a half-written pair.
        let mut committed = self.inner.state.read().await.clone();
        if let Some(pending) = committed.pending.as_mut() {
            pending.decision = CoordinatorDecision::Committed;
        }
        self.persist(&committed)?;
        *self.inner.state.write().await = committed.clone();
        let receipt = bilateral_receipt(
            &operation_id,
            &request.idempotency_key,
            actor,
            TopologyOperationStatus::Applied,
            &created_at,
            request.reason,
            &plan,
            &authority_revisions_before,
            &authority_revisions_after,
            None,
        );
        self.commit_terminal_receipt(
            committed,
            states_after,
            receipt.clone(),
            fingerprint,
            Some(scoped_key),
            first,
            second,
        )
        .await?;
        Ok(receipt)
    }

    async fn recover_locked(
        &self,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
    ) -> Result<Option<TopologyOperationReceipt>, TopologyControlError> {
        self.bind_pair_locked(first, second).await?;
        self.reconcile_operation_records_locked().await?;
        let state = self.inner.state.read().await.clone();
        let Some(pending) = state.pending.clone() else {
            if !self.inner.pair_reconciled.load(Ordering::Acquire) {
                match self
                    .reconcile_committed_pair_intent_locked(first, second)
                    .await
                {
                    Ok(()) => {
                        self.inner.pair_reconciled.store(true, Ordering::Release);
                        *self.inner.last_clean_reconcile_error.write().await = None;
                    }
                    Err(error) => {
                        self.inner.pair_reconciled.store(false, Ordering::Release);
                        // Ordinary committed-intent self-heal is best effort
                        // for read paths: the snapshot must stay available so
                        // operators can inspect desired=true/actual=false.
                        *self.inner.last_clean_reconcile_error.write().await =
                            Some(error.to_string());
                    }
                }
            }
            return Ok(None);
        };
        self.inner.pair_reconciled.store(false, Ordering::Release);
        if pending.decision == CoordinatorDecision::Applying {
            let mut rolling_back = state.clone();
            if let Some(pending) = rolling_back.pending.as_mut() {
                pending.decision = CoordinatorDecision::RollingBack;
            }
            self.persist(&rolling_back)?;
            *self.inner.state.write().await = rolling_back;
        }
        let committed = pending.decision == CoordinatorDecision::Committed;
        let status = if committed {
            TopologyOperationStatus::Applied
        } else {
            TopologyOperationStatus::RolledBack
        };
        let failures = match pending.decision {
            CoordinatorDecision::Applying | CoordinatorDecision::RollingBack => {
                rollback_physical_pair(first, second, &pending.physical_before).await
            }
            CoordinatorDecision::Committed => {
                let mut failures = Vec::new();
                for operation in &pending.operations {
                    if let Err(error) = actuate(first, second, operation).await {
                        failures.push(error.to_string());
                    }
                }
                failures
            }
        };
        if !failures.is_empty() {
            // Keep the journal intact so every subsequent startup/query retries
            // convergence. Never erase the only durable recovery authority.
            let detail = format!(
                "bilateral topology recovery incomplete: {}",
                failures.join("; ")
            );
            let mut retryable = self.inner.state.read().await.clone();
            if let Some(pending) = retryable.pending.as_mut() {
                // A fsynced Committed decision is irrevocable. A transient
                // actuator failure must leave recovery on the commit path;
                // only pre-commit work is ever changed to RollingBack.
                if pending.decision == CoordinatorDecision::Applying {
                    pending.decision = CoordinatorDecision::RollingBack;
                }
                pending.recovery_attempts = pending.recovery_attempts.saturating_add(1);
                pending.last_recovery_error = Some(detail.clone());
            }
            if let Some(record) = retryable.operation_records.iter_mut().find(|record| {
                (!pending.audit_record_id.is_empty() && record.record_id == pending.audit_record_id)
                    || (pending.audit_record_id.is_empty()
                        && record.operation_id == pending.operation_id
                        && !operation_record_status_is_terminal(record.status))
            }) {
                set_operation_record_status(
                    record,
                    TopologyOperationRecordStatus::RecoveryFailed,
                    Some("topology_actuator_failed"),
                );
            }
            self.persist(&retryable)?;
            *self.inner.state.write().await = retryable;
            return Err(TopologyControlError::Actuator(detail));
        }
        let mut current_states = BTreeMap::new();
        current_states.insert(
            first.mob_id(),
            first.topology_controller().inner.state.read().await.clone(),
        );
        current_states.insert(
            second.mob_id(),
            second
                .topology_controller()
                .inner
                .state
                .read()
                .await
                .clone(),
        );
        let revisions_before_merge = current_states
            .iter()
            .map(|(authority, state)| (authority.clone(), state.revision))
            .collect::<BTreeMap<_, _>>();
        merge_pending_intent(&mut current_states, &pending, committed)?;
        let revisions_after_merge = current_states
            .iter()
            .map(|(authority, state)| (authority.clone(), state.revision))
            .collect::<BTreeMap<_, _>>();
        let plan = TopologyBilateralPlan {
            authority_revisions: pending.authority_revisions_before.clone(),
            operations: pending
                .operations
                .iter()
                .map(|operation| TopologyPlannedEdge {
                    action: operation.action,
                    edge: operation.edge.clone(),
                    actual_before: false,
                    desired_before: false,
                    desired_after: !matches!(operation.action, TopologyAction::Disconnect),
                    declared: false,
                    operator_added: false,
                    suppressed: false,
                    requires_physical_change: true,
                    requires_intent_change: true,
                })
                .collect(),
        };
        let idempotency_map_key = pending.idempotency_map_key().map(str::to_string);
        let receipt = bilateral_receipt(
            &pending.operation_id,
            &pending.idempotency_key,
            &pending.actor,
            status,
            &pending.created_at,
            pending.reason,
            &plan,
            &revisions_before_merge,
            &revisions_after_merge,
            None,
        );
        self.commit_terminal_receipt(
            self.inner.state.read().await.clone(),
            current_states,
            receipt.clone(),
            pending.fingerprint,
            idempotency_map_key.as_deref(),
            first,
            second,
        )
        .await?;
        self.inner.pair_reconciled.store(true, Ordering::Release);
        *self.inner.last_clean_reconcile_error.write().await = None;
        Ok(Some(receipt))
    }

    /// Reinstall durable cross-authority intent after an ordinary clean
    /// process restart. Inproc namespaces and aliases are process-local, so
    /// an empty coordinator WAL does not imply the physical graph is already
    /// converged.
    async fn reconcile_committed_pair_intent_locked(
        &self,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
    ) -> Result<(), TopologyControlError> {
        let first_state = first.topology_controller().inner.state.read().await.clone();
        let second_state = second
            .topology_controller()
            .inner
            .state
            .read()
            .await
            .clone();
        let pair_additions = |state: &TopologyIntentState| {
            state
                .cross_additions
                .iter()
                .filter(|edge| edge_belongs_to_pair(edge, first, second))
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let pair_suppressions = |state: &TopologyIntentState| {
            state
                .cross_suppressions
                .iter()
                .filter(|edge| edge_belongs_to_pair(edge, first, second))
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let first_additions = pair_additions(&first_state);
        let second_additions = pair_additions(&second_state);
        let first_suppressions = pair_suppressions(&first_state);
        let second_suppressions = pair_suppressions(&second_state);
        if first_additions != second_additions || first_suppressions != second_suppressions {
            return Err(TopologyControlError::AuthorityMismatch(format!(
                "bilateral intent diverged between {:?} and {:?}; refusing to guess",
                first.mob_id(),
                second.mob_id()
            )));
        }

        for edge in first_additions.difference(&first_suppressions) {
            let operation = TopologyMutation {
                action: TopologyAction::Connect,
                edge: edge.clone(),
            };
            actuate(first, second, &operation).await?;
        }
        for edge in &first_suppressions {
            let operation = TopologyMutation {
                action: TopologyAction::Disconnect,
                edge: edge.clone(),
            };
            actuate(first, second, &operation).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_terminal_receipt(
        &self,
        mut coordinator: BilateralCoordinatorState,
        mut controller_states: BTreeMap<String, TopologyIntentState>,
        receipt: TopologyOperationReceipt,
        fingerprint: String,
        idempotency_map_key: Option<&str>,
        first: &UnifiedRuntime,
        second: &UnifiedRuntime,
    ) -> Result<(), TopologyControlError> {
        let audit_status = match receipt.status {
            TopologyOperationStatus::Pending => TopologyOperationRecordStatus::Pending,
            TopologyOperationStatus::Applied => TopologyOperationRecordStatus::Applied,
            TopologyOperationStatus::Noop => TopologyOperationRecordStatus::Noop,
            TopologyOperationStatus::RolledBack => TopologyOperationRecordStatus::RolledBack,
            TopologyOperationStatus::PartialDegraded => {
                TopologyOperationRecordStatus::PartialDegraded
            }
        };
        let operation_id = receipt.operation_id.clone();
        let audit_record_id = coordinator
            .pending
            .as_ref()
            .filter(|pending| pending.operation_id == operation_id)
            .map(|pending| pending.audit_record_id.clone());
        for runtime in [first, second] {
            let authority = runtime.mob_id();
            let state = controller_states
                .get_mut(&authority)
                .ok_or_else(|| TopologyControlError::AuthorityMismatch(authority.clone()))?;
            // Bilateral receipts stay coordinator-owned. Copying them into
            // one authority's local store would let local operation/get
            // bypass the other authority's ABAC decision.
            runtime.topology_controller().persist_candidate(state)?;
        }
        for runtime in [first, second] {
            let authority = runtime.mob_id();
            let state = controller_states
                .remove(&authority)
                .ok_or_else(|| TopologyControlError::AuthorityMismatch(authority.clone()))?;
            *runtime.topology_controller().inner.state.write().await = state;
        }
        coordinator.pending = None;
        if let Some(record) = coordinator.operation_records.iter_mut().find(|record| {
            audit_record_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .is_some_and(|id| record.record_id == id)
                || (audit_record_id.as_deref().is_none_or(str::is_empty)
                    && record.operation_id == operation_id
                    && !operation_record_status_is_terminal(record.status))
        }) {
            set_operation_record_status(record, audit_status, None);
        }
        if let Some(key) = idempotency_map_key {
            coordinator.idempotency.insert(
                key.to_string(),
                IdempotencyRecord {
                    fingerprint,
                    operation_id: receipt.operation_id.clone(),
                    scoped: key != receipt.idempotency_key,
                },
            );
        }
        if !coordinator
            .receipts
            .iter()
            .any(|existing| existing.operation_id == receipt.operation_id)
        {
            coordinator.receipts.push_back(receipt);
        }
        let receipt_limit = first
            .topology_controller()
            .policy()
            .receipt_limit
            .min(second.topology_controller().policy().receipt_limit);
        while coordinator.receipts.len() > receipt_limit {
            let Some(evicted) = coordinator.receipts.pop_front() else {
                break;
            };
            // A receipt that no entry names compacts and removes nothing.
            let Some(evicted_key) = filed_idempotency_key(&coordinator.idempotency, &evicted)
            else {
                continue;
            };
            let compacted = compacted_key_fingerprint(&evicted_key);
            if !coordinator.compacted_idempotency.contains(&compacted) {
                coordinator.compacted_idempotency.push_back(compacted);
            }
            coordinator.idempotency.remove(&evicted_key);
        }
        let history_limit = first
            .topology_controller()
            .policy()
            .idempotency_history_limit
            .min(
                second
                    .topology_controller()
                    .policy()
                    .idempotency_history_limit,
            );
        while coordinator.compacted_idempotency.len() > history_limit {
            coordinator.compacted_idempotency.pop_front();
        }
        self.persist(&coordinator)?;
        *self.inner.state.write().await = coordinator;
        Ok(())
    }
}

fn canonical_coordinator_path(path: PathBuf) -> Result<PathBuf, TopologyControlError> {
    if std::fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(TopologyControlError::Persistence(format!(
            "bilateral topology journal must not be a symlink: {}",
            path.display()
        )));
    }
    let file_name = path.file_name().ok_or_else(|| {
        TopologyControlError::Persistence(format!(
            "bilateral topology journal path has no file name: {}",
            path.display()
        ))
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = match parent {
        Some(parent) => {
            std::fs::create_dir_all(parent)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
            std::fs::canonicalize(parent)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?
        }
        None => std::env::current_dir()
            .map_err(|error| TopologyControlError::Persistence(error.to_string()))?,
    };
    Ok(parent.join(file_name))
}

fn canonical_runtimes<'a>(
    left: &'a UnifiedRuntime,
    right: &'a UnifiedRuntime,
) -> Result<(&'a UnifiedRuntime, &'a UnifiedRuntime), TopologyControlError> {
    let left_id = left.mob_id();
    let right_id = right.mob_id();
    if left_id == right_id {
        return Err(TopologyControlError::AuthorityMismatch(
            "bilateral coordinator requires two distinct authorities".to_string(),
        ));
    }
    if left_id < right_id {
        Ok((left, right))
    } else {
        Ok((right, left))
    }
}

fn access_view(runtime: &UnifiedRuntime, principal: Option<&str>) -> Option<AccessView> {
    runtime
        .access_controller()
        .map(|controller| controller.view_for_subject(principal))
}

fn view_allows(view: Option<&AccessView>, action: &str, identity: &str) -> bool {
    view.is_none_or(|view| !view.enforced() || view.allows_agent(action, identity))
}

fn endpoint_visible(view: Option<&AccessView>, endpoint: &TopologyEndpoint) -> bool {
    view.is_none_or(|view| {
        !view.enforced()
            || (view.can_view_agent(&endpoint.identity)
                && view.allows_agent(ACTION_AGENT_VIEW, &endpoint.identity)
                && view.allows_agent(ACTION_TOPOLOGY_VIEW, &endpoint.identity))
    })
}

fn bilateral_attribution_allowed(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    principal: Option<&str>,
    results: &[TopologyEdgeResult],
) -> bool {
    let first_view = access_view(first, principal);
    let second_view = access_view(second, principal);
    results.iter().all(|result| {
        [&result.edge.a, &result.edge.b]
            .into_iter()
            .all(|endpoint| {
                endpoint_visible(first_view.as_ref(), endpoint)
                    && endpoint_visible(second_view.as_ref(), endpoint)
                    && view_allows(
                        first_view.as_ref(),
                        ACTION_TOPOLOGY_AUDIT,
                        endpoint.identity.as_str(),
                    )
                    && view_allows(
                        second_view.as_ref(),
                        ACTION_TOPOLOGY_AUDIT,
                        endpoint.identity.as_str(),
                    )
            })
    })
}

fn project_bilateral_result_attribution(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    principal: Option<&str>,
    result: &mut Result<TopologyOperationReceipt, TopologyControlError>,
) {
    match result {
        Ok(receipt) => {
            if !bilateral_attribution_allowed(first, second, principal, &receipt.results) {
                receipt.actor.clear();
            }
        }
        Err(TopologyControlError::ApplyFailed { receipt, .. }) => {
            if !bilateral_attribution_allowed(first, second, principal, &receipt.results) {
                receipt.actor.clear();
            }
        }
        Err(_) => {}
    }
}

fn validate_pair_policies(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    count: usize,
    mutation: bool,
) -> Result<(), TopologyControlError> {
    let policies = [
        first.topology_controller().policy(),
        second.topology_controller().policy(),
    ];
    for policy in policies {
        if policy.mode == TopologyControlMode::Disabled {
            return Err(TopologyControlError::FeatureDisabled);
        }
        if mutation && policy.mode != TopologyControlMode::Editable {
            return Err(TopologyControlError::ReadOnly);
        }
        if !policy.allow_cross_authority {
            return Err(TopologyControlError::CrossAuthorityDisabled);
        }
        validate_batch(&policy, count)?;
    }
    Ok(())
}

fn normalize_bilateral_operations(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    operations: Vec<TopologyMutation>,
) -> Result<Vec<TopologyMutation>, TopologyControlError> {
    let authorities = BTreeSet::from([first.mob_id(), second.mob_id()]);
    operations
        .into_iter()
        .map(|operation| {
            let normalize_endpoint = |endpoint: TopologyEndpoint| {
                let authority = endpoint.authority.ok_or_else(|| {
                    TopologyControlError::InvalidRequest(
                        "bilateral endpoints require an explicit authority".to_string(),
                    )
                })?;
                if !authorities.contains(&authority) {
                    return Err(TopologyControlError::AuthorityMismatch(authority));
                }
                let identity = crate::identity_first::AgentIdentity::parse(endpoint.identity.trim())
                    .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))?
                    .to_string();
                Ok(TopologyEndpoint {
                    authority: Some(authority),
                    identity,
                })
            };
            let edge = TopologyEdge::new(
                normalize_endpoint(operation.edge.a)?,
                normalize_endpoint(operation.edge.b)?,
            )?;
            if edge.a.authority == edge.b.authority {
                return Err(TopologyControlError::InvalidRequest(
                    "bilateral API accepts cross-authority edges only; use the authority-local API for same-mob edges"
                        .to_string(),
                ));
            }
            Ok(TopologyMutation {
                action: operation.action,
                edge,
            })
        })
        .collect()
}

fn edge_belongs_to_pair(
    edge: &TopologyEdge,
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
) -> bool {
    let authorities = BTreeSet::from([first.mob_id(), second.mob_id()]);
    BTreeSet::from([
        edge.a.authority.clone().unwrap_or_default(),
        edge.b.authority.clone().unwrap_or_default(),
    ]) == authorities
}

fn endpoints_for_pair<'a>(
    edge: &'a TopologyEdge,
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
) -> Result<(&'a TopologyEndpoint, &'a TopologyEndpoint), TopologyControlError> {
    let first_id = first.mob_id();
    let second_id = second.mob_id();
    if edge.a.authority.as_deref() == Some(first_id.as_str())
        && edge.b.authority.as_deref() == Some(second_id.as_str())
    {
        Ok((&edge.a, &edge.b))
    } else if edge.b.authority.as_deref() == Some(first_id.as_str())
        && edge.a.authority.as_deref() == Some(second_id.as_str())
    {
        Ok((&edge.b, &edge.a))
    } else {
        Err(TopologyControlError::AuthorityMismatch(format!(
            "edge {:?} <-> {:?} does not belong to {} <-> {}",
            edge.a.authority, edge.b.authority, first_id, second_id
        )))
    }
}

fn validate_expected_revisions(
    snapshot: &TopologyBilateralSnapshot,
    expected: &BTreeMap<String, u64>,
) -> Result<(), TopologyControlError> {
    if expected.len() != snapshot.authority_revisions.len() {
        return Err(TopologyControlError::InvalidRequest(
            "expected_revisions must name exactly both authorities".to_string(),
        ));
    }
    for (authority, actual) in &snapshot.authority_revisions {
        let wanted = expected.get(authority).ok_or_else(|| {
            TopologyControlError::AuthorityMismatch(format!(
                "expected revision missing for {authority}"
            ))
        })?;
        if wanted != actual {
            return Err(TopologyControlError::RevisionConflict {
                expected: *wanted,
                actual: *actual,
            });
        }
    }
    Ok(())
}

fn validate_bilateral_known_endpoints(
    snapshot: &TopologyBilateralSnapshot,
    operations: &[TopologyMutation],
) -> Result<(), TopologyControlError> {
    let known = snapshot
        .nodes
        .iter()
        .map(|node| node.endpoint.clone())
        .collect::<BTreeSet<_>>();
    for operation in operations {
        for endpoint in [&operation.edge.a, &operation.edge.b] {
            if !known.contains(endpoint) {
                return Err(TopologyControlError::MemberNotFound(
                    endpoint.identity.clone(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_bilateral_reconnect_targets(
    snapshot: &TopologyBilateralSnapshot,
    operations: &[TopologyMutation],
) -> Result<(), TopologyControlError> {
    let historical = snapshot
        .edges
        .iter()
        .map(|edge| edge.edge.clone())
        .collect::<BTreeSet<_>>();
    for operation in operations {
        if !historical.contains(&operation.edge) {
            match operation.action {
                TopologyAction::Reconnect => {
                    return Err(TopologyControlError::ReconnectTargetMissing(
                        operation.edge.clone(),
                    ));
                }
                TopologyAction::Disconnect => {
                    return Err(TopologyControlError::DisconnectTargetMissing(
                        operation.edge.clone(),
                    ));
                }
                TopologyAction::Connect => {}
            }
        }
    }
    Ok(())
}

fn validate_bilateral_connect_does_not_clear_suppression(
    snapshot: &TopologyBilateralSnapshot,
    operations: &[TopologyMutation],
) -> Result<(), TopologyControlError> {
    let reconnect_required = snapshot
        .edges
        .iter()
        .filter(|edge| edge.suppressed || (edge.desired && !edge.actual))
        .map(|edge| edge.edge.clone())
        .collect::<BTreeSet<_>>();
    for operation in operations {
        if matches!(operation.action, TopologyAction::Connect)
            && reconnect_required.contains(&operation.edge)
        {
            return Err(TopologyControlError::ReconnectRequired(
                operation.edge.clone(),
            ));
        }
    }
    Ok(())
}

fn authorize_bilateral(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    principal: Option<&str>,
    operations: &[TopologyMutation],
) -> Result<(), TopologyControlError> {
    let views = [
        (first.mob_id(), access_view(first, principal)),
        (second.mob_id(), access_view(second, principal)),
    ];
    for operation in operations {
        for (authority, view) in &views {
            for endpoint in [&operation.edge.a, &operation.edge.b] {
                for action in [
                    operation.action.access_action(),
                    ACTION_TOPOLOGY_CROSS_AUTHORITY,
                ] {
                    if !view_allows(view.as_ref(), action, &endpoint.identity) {
                        return Err(TopologyControlError::AccessDenied {
                            authority: authority.clone(),
                            action: action.to_string(),
                            identity: endpoint.identity.clone(),
                        });
                    }
                }
                if operations.len() > 1
                    && !view_allows(view.as_ref(), ACTION_TOPOLOGY_BULK, &endpoint.identity)
                {
                    return Err(TopologyControlError::AccessDenied {
                        authority: authority.clone(),
                        action: ACTION_TOPOLOGY_BULK.to_string(),
                        identity: endpoint.identity.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn plan_bilateral(
    snapshot: &TopologyBilateralSnapshot,
    operations: Vec<TopologyMutation>,
) -> TopologyBilateralPlan {
    let by_edge = snapshot
        .edges
        .iter()
        .map(|edge| (edge.edge.clone(), edge))
        .collect::<BTreeMap<_, _>>();
    TopologyBilateralPlan {
        authority_revisions: snapshot.authority_revisions.clone(),
        operations: operations
            .into_iter()
            .map(|operation| {
                let before = by_edge.get(&operation.edge).copied();
                let actual = before.is_some_and(|edge| edge.actual);
                let desired = before.is_some_and(|edge| edge.desired);
                let desired_after = !matches!(operation.action, TopologyAction::Disconnect);
                TopologyPlannedEdge {
                    action: operation.action,
                    edge: operation.edge,
                    actual_before: actual,
                    desired_before: desired,
                    desired_after,
                    declared: before.is_some_and(|edge| edge.declared),
                    operator_added: before.is_some_and(|edge| edge.operator_added),
                    suppressed: before.is_some_and(|edge| edge.suppressed),
                    requires_physical_change: matches!(operation.action, TopologyAction::Reconnect)
                        || actual != desired_after,
                    requires_intent_change: desired != desired_after,
                }
            })
            .collect(),
    }
}

async fn actuate(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    operation: &TopologyMutation,
) -> Result<(), TopologyControlError> {
    let (first_endpoint, second_endpoint) = endpoints_for_pair(&operation.edge, first, second)?;
    match operation.action {
        // Always call the idempotent bilateral primitive for Connect. Besides
        // the two trust halves it repairs process-local cross-namespace
        // aliases, whose loss is not visible in `wired_to` alone.
        TopologyAction::Connect => first
            .wire_bilateral_same_process(
                second,
                &first_endpoint.identity,
                &second_endpoint.identity,
            )
            .await
            .map_err(|error| TopologyControlError::Actuator(error.to_string())),
        // The locked primitive re-observes both concrete generations. An
        // unlocked state preflight can race reset/reconcile and incorrectly
        // skip a newly materialized edge.
        TopologyAction::Disconnect => first
            .unwire_bilateral_same_process(
                second,
                &first_endpoint.identity,
                &second_endpoint.identity,
            )
            .await
            .map_err(|error| TopologyControlError::Actuator(error.to_string())),
        TopologyAction::Reconnect => {
            first
                .unwire_bilateral_same_process(
                    second,
                    &first_endpoint.identity,
                    &second_endpoint.identity,
                )
                .await
                .map_err(|error| TopologyControlError::Actuator(error.to_string()))?;
            first
                .wire_bilateral_same_process(
                    second,
                    &first_endpoint.identity,
                    &second_endpoint.identity,
                )
                .await
                .map_err(|error| TopologyControlError::Actuator(error.to_string()))
        }
    }
}

async fn rollback_physical_pair(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    before: &[BilateralPhysicalBefore],
) -> Vec<String> {
    let mut failures = Vec::new();
    for prior in before.iter().rev() {
        let action = if prior.left_half && prior.right_half {
            TopologyAction::Connect
        } else {
            // A pre-existing orphan is degraded, not a valid logical edge.
            // Recovery cleans it rather than preserving an unroutable half.
            TopologyAction::Disconnect
        };
        let operation = TopologyMutation {
            action,
            edge: prior.edge.clone(),
        };
        if let Err(error) = actuate(first, second, &operation).await {
            failures.push(error.to_string());
        }
    }
    failures
}

fn merge_pending_intent(
    states: &mut BTreeMap<String, TopologyIntentState>,
    pending: &PendingBilateralOperation,
    committed: bool,
) -> Result<(), TopologyControlError> {
    for (authority, state) in states {
        let revision_before = state.revision;
        let mut changed = false;
        for delta in &pending.intent_deltas {
            let membership = if committed {
                delta.after.get(authority)
            } else {
                delta.before.get(authority)
            }
            .ok_or_else(|| {
                TopologyControlError::AuthorityMismatch(format!(
                    "bilateral journal has no intent delta for authority {authority}"
                ))
            })?;
            let had_added = state.cross_additions.contains(&delta.edge);
            let had_suppressed = state.cross_suppressions.contains(&delta.edge);
            if membership.added {
                state.cross_additions.insert(delta.edge.clone());
            } else {
                state.cross_additions.remove(&delta.edge);
            }
            if membership.suppressed {
                state.cross_suppressions.insert(delta.edge.clone());
            } else {
                state.cross_suppressions.remove(&delta.edge);
            }
            changed |= had_added != membership.added || had_suppressed != membership.suppressed;
        }
        let journal_before = pending
            .authority_revisions_before
            .get(authority)
            .copied()
            .ok_or_else(|| TopologyControlError::AuthorityMismatch(authority.clone()))?;
        let journal_target = if committed {
            pending.authority_revisions_after.get(authority).copied()
        } else {
            pending.authority_revisions_before.get(authority).copied()
        }
        .ok_or_else(|| TopologyControlError::AuthorityMismatch(authority.clone()))?;
        state.revision = if revision_before == journal_before {
            journal_target
        } else if changed {
            revision_before.saturating_add(1)
        } else {
            revision_before
        };
    }
    Ok(())
}

fn bilateral_fingerprint(
    request: &TopologyBilateralApplyRequest,
) -> Result<String, TopologyControlError> {
    let bytes = serde_json::to_vec(request)
        .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn operation_record_for_bilateral_request(
    first: &UnifiedRuntime,
    second: &UnifiedRuntime,
    request: &TopologyBilateralApplyRequest,
    scoped_key: &str,
    actor: &str,
    principal: Option<&str>,
) -> Result<TopologyOperationRecord, TopologyControlError> {
    let fingerprint = bilateral_fingerprint(request)?;
    let operation_id = operation_id(scoped_key, &fingerprint);
    let now = chrono::Utc::now().to_rfc3339();
    Ok(TopologyOperationRecord {
        version: TOPOLOGY_OPERATION_RECORD_VERSION,
        seq: 0,
        record_id: new_audit_record_id(),
        operation_id,
        idempotency_key_fingerprint: idempotency_key_fingerprint(&request.idempotency_key),
        actor: actor.to_string(),
        principal: principal.map(str::to_string),
        authorities: vec![first.mob_id(), second.mob_id()],
        operations: request.operations.clone(),
        status: TopologyOperationRecordStatus::Requested,
        requested_at: now.clone(),
        updated_at: now,
        error_kind: None,
        error_message: None,
    })
}

fn pending_bilateral_receipt(pending: &PendingBilateralOperation) -> TopologyOperationReceipt {
    let degraded = pending.last_recovery_error.is_some();
    TopologyOperationReceipt {
        operation_id: pending.operation_id.clone(),
        idempotency_key: pending.idempotency_key.clone(),
        actor: pending.actor.clone(),
        status: if degraded {
            TopologyOperationStatus::PartialDegraded
        } else {
            TopologyOperationStatus::Pending
        },
        base_revision: pending
            .authority_revisions_before
            .values()
            .copied()
            .min()
            .unwrap_or(0),
        revision: pending
            .authority_revisions_after
            .values()
            .copied()
            .max()
            .unwrap_or(0),
        created_at: pending.created_at.clone(),
        reason: pending.reason.clone(),
        results: pending
            .operations
            .iter()
            .map(|operation| TopologyEdgeResult {
                action: operation.action,
                edge: operation.edge.clone(),
                status: if degraded {
                    TopologyEdgeResultStatus::RollbackFailed
                } else {
                    TopologyEdgeResultStatus::Pending
                },
                actual_before: false,
                actual_after: false,
                error: pending.last_recovery_error.clone(),
            })
            .collect(),
        authority_revisions: pending
            .authority_revisions_before
            .iter()
            .filter_map(|(authority, before)| {
                pending
                    .authority_revisions_after
                    .get(authority)
                    .map(|after| {
                        (
                            authority.clone(),
                            TopologyRevisionTransition {
                                base_revision: *before,
                                revision: *after,
                            },
                        )
                    })
            })
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn bilateral_receipt(
    operation_id: &str,
    idempotency_key: &str,
    actor: &str,
    status: TopologyOperationStatus,
    created_at: &str,
    reason: Option<String>,
    plan: &TopologyBilateralPlan,
    revisions_before: &BTreeMap<String, u64>,
    revisions_after: &BTreeMap<String, u64>,
    error: Option<String>,
) -> TopologyOperationReceipt {
    let authority_revisions = revisions_before
        .iter()
        .filter_map(|(authority, before)| {
            revisions_after.get(authority).map(|after| {
                (
                    authority.clone(),
                    TopologyRevisionTransition {
                        base_revision: *before,
                        revision: *after,
                    },
                )
            })
        })
        .collect();
    let edge_status = match status {
        TopologyOperationStatus::Applied => TopologyEdgeResultStatus::Applied,
        TopologyOperationStatus::Noop => TopologyEdgeResultStatus::Noop,
        TopologyOperationStatus::RolledBack => TopologyEdgeResultStatus::RolledBack,
        TopologyOperationStatus::PartialDegraded => TopologyEdgeResultStatus::RollbackFailed,
        TopologyOperationStatus::Pending => TopologyEdgeResultStatus::Pending,
    };
    let base_revision = revisions_before.values().copied().min().unwrap_or(0);
    let revision = revisions_after.values().copied().max().unwrap_or(0);
    TopologyOperationReceipt {
        operation_id: operation_id.to_string(),
        idempotency_key: idempotency_key.to_string(),
        actor: actor.to_string(),
        status,
        base_revision,
        revision,
        created_at: created_at.to_string(),
        reason,
        results: plan
            .operations
            .iter()
            .map(|planned| TopologyEdgeResult {
                action: planned.action,
                edge: planned.edge.clone(),
                status: edge_status,
                actual_before: planned.actual_before,
                actual_after: match status {
                    TopologyOperationStatus::Applied => planned.desired_after,
                    TopologyOperationStatus::Noop => planned.actual_before,
                    _ => planned.actual_before,
                },
                error: error.clone(),
            })
            .collect(),
        authority_revisions,
    }
}

fn authority_pair_key(authorities: &[String]) -> String {
    authorities
        .iter()
        .map(|authority| format!("{}:{authority}", authority.len()))
        .collect::<Vec<_>>()
        .join("|")
}

fn persist_coordinator_state(
    path: &Path,
    state: &BilateralCoordinatorState,
) -> Result<(), TopologyControlError> {
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    let mut temp = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .ok_or_else(|| {
            TopologyControlError::Persistence(format!(
                "bilateral topology journal path has no file name: {}",
                path.display()
            ))
        })?;
    name.push(".tmp");
    temp.set_file_name(name);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    file.write_all(&bytes)
        .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    file.sync_all()
        .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    std::fs::rename(&temp, path)
        .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    }
    Ok(())
}

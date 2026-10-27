//! Optional, policy-gated topology control plane.
//!
//! The low-level mob runtime owns member wiring. This module owns the durable
//! operator intent layered on top of definition/provider-declared topology:
//! explicit additions and explicit suppression tombstones. The control plane
//! is disabled by default and never invents a bulk operation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use fs2::FileExt;
use meerkat_mob::runtime::MobMemberListEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::member_comms_id::{mob_member_id, runtime_alias_str};
use crate::unified_runtime::edge_reconcile::reconcile_edges_over_members;
use crate::unified_runtime::edge_types::{DesiredPeerEdge, EdgeDiscovery, EdgeMemberView};
use crate::unified_runtime::types::UnifiedRuntimeReconcileEdgesReport;

mod coordinator;
pub use coordinator::*;

const MAX_POLICY_BATCH_SIZE: usize = 1024;
const DEFAULT_RECEIPT_LIMIT: usize = 256;
const DEFAULT_IDEMPOTENCY_HISTORY_LIMIT: usize = 4096;
const TOPOLOGY_STATE_SCHEMA_VERSION: u32 = 1;

/// Whether the optional topology control plane may mutate desired state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyControlMode {
    /// Querying live topology remains available, but planning and mutation are
    /// not advertised. This is the default for existing Meta consumers.
    #[default]
    Disabled,
    /// Query and plan are available. Apply is denied and not advertised.
    ReadOnly,
    /// Query, plan, and explicitly authorized apply are available.
    Editable,
}

/// Deployment policy for the optional topology control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyControlPolicy {
    #[serde(default)]
    pub mode: TopologyControlMode,
    /// Permit requests containing more than one explicit operation.
    #[serde(default)]
    pub allow_bulk: bool,
    /// Hard request bound. It is enforced even when `allow_bulk` is true.
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    /// Cross-authority mutation is a separate opt-in. It is available only
    /// through a bilateral coordinator that authorizes the principal against
    /// both runtime authorities; authority-local RPCs continue to fail closed.
    #[serde(default)]
    pub allow_cross_authority: bool,
    /// Number of durable operation receipts retained for audit/idempotency.
    #[serde(default = "default_receipt_limit")]
    pub receipt_limit: usize,
    /// Exact replay-rejection horizon after full receipts age out. The oldest
    /// key fingerprints are evicted once this bound is reached; retrying a key
    /// older than the configured horizon is contractually a new request.
    #[serde(default = "default_idempotency_history_limit")]
    pub idempotency_history_limit: usize,
}

const fn default_max_batch_size() -> usize {
    1
}

const fn default_receipt_limit() -> usize {
    DEFAULT_RECEIPT_LIMIT
}

const fn default_idempotency_history_limit() -> usize {
    DEFAULT_IDEMPOTENCY_HISTORY_LIMIT
}

impl Default for TopologyControlPolicy {
    fn default() -> Self {
        Self {
            mode: TopologyControlMode::Disabled,
            allow_bulk: false,
            max_batch_size: default_max_batch_size(),
            allow_cross_authority: false,
            receipt_limit: default_receipt_limit(),
            idempotency_history_limit: default_idempotency_history_limit(),
        }
    }
}

impl TopologyControlPolicy {
    pub fn validate(&self) -> Result<(), TopologyControlError> {
        if self.max_batch_size == 0 || self.max_batch_size > MAX_POLICY_BATCH_SIZE {
            return Err(TopologyControlError::InvalidPolicy(format!(
                "max_batch_size must be in 1..={MAX_POLICY_BATCH_SIZE}"
            )));
        }
        if self.receipt_limit == 0 || self.receipt_limit > 65_536 {
            return Err(TopologyControlError::InvalidPolicy(
                "receipt_limit must be in 1..=65536".to_string(),
            ));
        }
        if self.idempotency_history_limit == 0 || self.idempotency_history_limit > 65_536 {
            return Err(TopologyControlError::InvalidPolicy(
                "idempotency_history_limit must be in 1..=65536".to_string(),
            ));
        }
        Ok(())
    }
}

/// Legacy-bootstrap topology configuration.
///
/// Existing embedders remain byte-for-byte opt-out via [`Default`]: mutation
/// is disabled and intent is in-memory only. Hosts that need reconnect and
/// suppression tombstones to survive process restart must provide a stable
/// `state_path` (normally `<app-state>/topology-control.json`).
#[derive(Debug, Clone, Default)]
pub struct TopologyBootstrapConfig {
    pub policy: TopologyControlPolicy,
    pub state_path: Option<PathBuf>,
}

/// A stable logical endpoint. `authority = None` means the current mob.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TopologyEndpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    pub identity: String,
}

impl TopologyEndpoint {
    pub fn local(identity: impl Into<String>) -> Self {
        Self {
            authority: None,
            identity: identity.into(),
        }
    }

    fn normalize(mut self, local_authority: &str) -> Result<Self, TopologyControlError> {
        self.identity = self.identity.trim().to_string();
        if self.identity.is_empty() {
            return Err(TopologyControlError::InvalidRequest(
                "topology endpoint identity must not be empty".to_string(),
            ));
        }
        self.identity = crate::identity_first::AgentIdentity::parse(&self.identity)
            .map_err(|error| {
                TopologyControlError::InvalidRequest(format!(
                    "invalid topology endpoint identity {:?}: {error}",
                    self.identity
                ))
            })?
            .to_string();
        self.authority = self
            .authority
            .take()
            .map(|authority| authority.trim().to_string())
            .filter(|authority| !authority.is_empty())
            .or_else(|| Some(local_authority.to_string()));
        Ok(self)
    }
}

/// Canonical undirected logical edge. Endpoints are sorted at construction.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TopologyEdge {
    pub a: TopologyEndpoint,
    pub b: TopologyEndpoint,
}

impl TopologyEdge {
    pub fn new(a: TopologyEndpoint, b: TopologyEndpoint) -> Result<Self, TopologyControlError> {
        if a.identity.trim().is_empty() || b.identity.trim().is_empty() {
            return Err(TopologyControlError::InvalidRequest(
                "topology edge endpoints must not be empty".to_string(),
            ));
        }
        if a == b {
            return Err(TopologyControlError::InvalidRequest(
                "topology self-edges are not allowed".to_string(),
            ));
        }
        if a < b {
            Ok(Self { a, b })
        } else {
            Ok(Self { a: b, b: a })
        }
    }

    fn normalize(self, local_authority: &str) -> Result<Self, TopologyControlError> {
        Self::new(
            self.a.normalize(local_authority)?,
            self.b.normalize(local_authority)?,
        )
    }

    pub fn is_local_to(&self, authority: &str) -> bool {
        self.a.authority.as_deref() == Some(authority)
            && self.b.authority.as_deref() == Some(authority)
    }

    fn local_desired_edge(&self, authority: &str) -> Option<DesiredPeerEdge> {
        if !self.is_local_to(authority) {
            return None;
        }
        DesiredPeerEdge::new(self.a.identity.clone(), self.b.identity.clone()).ok()
    }
}

impl<'de> Deserialize<'de> for TopologyEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            a: TopologyEndpoint,
            b: TopologyEndpoint,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::new(raw.a, raw.b).map_err(serde::de::Error::custom)
    }
}

/// Explicit topology intent. There is intentionally no `connect_all` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyAction {
    Connect,
    Disconnect,
    Reconnect,
}

impl TopologyAction {
    pub const fn access_action(self) -> &'static str {
        match self {
            Self::Connect => crate::access::ACTION_TOPOLOGY_CONNECT,
            Self::Disconnect => crate::access::ACTION_TOPOLOGY_DISCONNECT,
            Self::Reconnect => crate::access::ACTION_TOPOLOGY_RECONNECT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyMutation {
    pub action: TopologyAction,
    pub edge: TopologyEdge,
}

/// A side-effect-free plan request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyPlanRequest {
    pub expected_revision: u64,
    pub operations: Vec<TopologyMutation>,
}

/// An apply request. `idempotency_key` is mandatory and durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyApplyRequest {
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub operations: Vec<TopologyMutation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Only immediate/r0 operations are currently supported. Higher tiers
    /// fail closed until Meta can bind an approval decision and mutation
    /// under one admission lock (avoiding a gating TOCTOU seam).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_tier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyNodeAffordances {
    pub can_connect: bool,
    pub can_disconnect: bool,
    pub can_reconnect: bool,
    pub can_bulk: bool,
    pub can_cross_authority: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyNodeSnapshot {
    pub endpoint: TopologyEndpoint,
    pub role: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Request-principal-specific mutation affordances. Runtime-only callers
    /// leave this absent; RPC projections must populate it server-side so a
    /// client never guesses endpoint authority from a broad method flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affordances: Option<TopologyNodeAffordances>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyEdgeSnapshot {
    pub edge: TopologyEdge,
    pub actual: bool,
    pub declared: bool,
    pub operator_added: bool,
    pub suppressed: bool,
    pub desired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologySnapshot {
    pub authority: String,
    pub revision: u64,
    pub policy: TopologyControlPolicy,
    pub nodes: Vec<TopologyNodeSnapshot>,
    pub edges: Vec<TopologyEdgeSnapshot>,
}

impl TopologySnapshot {
    /// Redact endpoints and incident edges the principal cannot both view and
    /// inspect through the topology action family.
    pub fn retain_visible_to(&mut self, view: &crate::access::AccessView) {
        let visible = |endpoint: &TopologyEndpoint| {
            view.can_view_agent(endpoint.identity.as_str())
                && view.allows_agent(
                    crate::access::ACTION_TOPOLOGY_VIEW,
                    endpoint.identity.as_str(),
                )
        };
        self.nodes.retain(|node| visible(&node.endpoint));
        self.edges
            .retain(|edge| visible(&edge.edge.a) && visible(&edge.edge.b));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyPlannedEdge {
    pub action: TopologyAction,
    pub edge: TopologyEdge,
    pub actual_before: bool,
    pub desired_before: bool,
    pub desired_after: bool,
    pub declared: bool,
    pub operator_added: bool,
    pub suppressed: bool,
    pub requires_physical_change: bool,
    pub requires_intent_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyPlan {
    pub authority: String,
    pub base_revision: u64,
    pub operations: Vec<TopologyPlannedEdge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyEdgeResultStatus {
    Pending,
    Applied,
    Noop,
    Failed,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyEdgeResult {
    pub action: TopologyAction,
    pub edge: TopologyEdge,
    pub status: TopologyEdgeResultStatus,
    pub actual_before: bool,
    pub actual_after: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyOperationStatus {
    Pending,
    Applied,
    Noop,
    RolledBack,
    PartialDegraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyOperationReceipt {
    pub operation_id: String,
    pub idempotency_key: String,
    /// Sensitive attribution. Unprivileged wire projections clear this field;
    /// serde then omits it while internal Rust receipts retain the actor.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    pub status: TopologyOperationStatus,
    pub base_revision: u64,
    pub revision: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub results: Vec<TopologyEdgeResult>,
    /// Present for a bilateral same-process transaction. Local operations
    /// leave this empty and continue using `base_revision`/`revision`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub authority_revisions: BTreeMap<String, TopologyRevisionTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyRevisionTransition {
    pub base_revision: u64,
    pub revision: u64,
}

pub const TOPOLOGY_OPERATION_RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyOperationRecordStatus {
    Requested,
    Denied,
    Invalid,
    Conflict,
    Pending,
    Applied,
    Noop,
    RolledBack,
    PartialDegraded,
    RecoveryFailed,
    Interrupted,
}

/// Durable, versioned audit authority for one normalized mutation attempt.
/// Replays upsert this same record; receipts are the client-facing projection
/// rather than a separate audit history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyOperationRecord {
    pub version: u32,
    /// Authority-local, durable, strictly increasing audit cursor.
    /// Sequence zero is reserved for records decoded from a pre-cursor
    /// schema and is assigned before the state is served.
    #[serde(default)]
    pub seq: u64,
    pub record_id: String,
    pub operation_id: String,
    pub idempotency_key_fingerprint: String,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    pub authorities: Vec<String>,
    pub operations: Vec<TopologyMutation>,
    pub status: TopologyOperationRecordStatus,
    pub requested_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Forward-only audit page. `next_after_seq` can be supplied verbatim on the
/// next request. A non-zero cursor older than retained history fails closed
/// with [`TopologyControlError::AuditCursorExpired`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyAuditPage {
    pub records: Vec<TopologyOperationRecord>,
    pub next_after_seq: u64,
    pub oldest_available_seq: Option<u64>,
    pub latest_seq: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IdempotencyRecord {
    fingerprint: String,
    operation_id: String,
    /// Filed under a scoped key. An unmarked entry whose key begins with the
    /// reserved prefix was filed under a raw key before keys were scoped and
    /// names no caller's apply.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    scoped: bool,
}

/// Versioned domain of the scoped apply idempotency digest.
const SCOPED_APPLY_KEY_DOMAIN: &str = "meta-topology-apply-v1";

/// Prefix of a scoped apply idempotency key, reserved: a host key may not
/// begin with it.
const SCOPED_APPLY_KEY_PREFIX: &str = "apply-scoped:";

/// The caller a topology apply's idempotency key belongs to. The same key
/// under two scopes names two different operations: a retry replays only its
/// own caller's apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TopologyApplyScope {
    /// The host's own Rust API without a principal. Its keys keep their
    /// shape, so a key it used before keys were scoped still names its apply.
    Host,
    /// A principal the host names on its Rust API.
    HostPrincipal { principal: String },
    /// The gateway's JSON-RPC surface: the embedding application.
    LocalHost,
    /// An anonymous caller of an open Console: the shared local account.
    Anonymous,
    /// A verified Console human, by OIDC issuer and subject.
    Human { issuer: String, subject: String },
    /// An authenticated Console principal with no verified human behind it
    /// (a service identity, or a token without a subject), by its trust
    /// source too: the provider its token was classified as and the issuer
    /// it verified against, so one name from two sources is two callers.
    Principal {
        principal: String,
        provider: String,
        issuer: String,
    },
}

impl TopologyApplyScope {
    /// The scope of the host's Rust API: the principal it names, else the
    /// host itself.
    pub(crate) fn for_host_principal(principal: Option<&str>) -> Self {
        match principal {
            Some(principal) => Self::HostPrincipal {
                principal: principal.to_string(),
            },
            None => Self::Host,
        }
    }

    /// Refuse a host key that would read as a scoped key.
    fn validate_key(&self, key: &str) -> Result<(), TopologyControlError> {
        if matches!(self, Self::Host) && key.starts_with(SCOPED_APPLY_KEY_PREFIX) {
            return Err(TopologyControlError::InvalidRequest(format!(
                "idempotency_key must not begin with the reserved prefix \
                 {SCOPED_APPLY_KEY_PREFIX:?}"
            )));
        }
        Ok(())
    }

    /// The key `key` is filed under at `authorities` in this scope.
    ///
    /// A scoped key is one digest over a typed JSON array of every
    /// component, so it carries neither the scope nor the key in clear, a
    /// component boundary cannot shift, and no principal's spelling can name
    /// another scope. The host keeps `key` itself.
    pub(crate) fn idempotency_key(&self, authorities: &[&str], key: &str) -> String {
        let owner = match self {
            Self::Host => return key.to_string(),
            Self::HostPrincipal { principal } => serde_json::json!(["host_principal", principal]),
            Self::LocalHost => serde_json::json!(["local_host"]),
            Self::Anonymous => serde_json::json!(["anonymous"]),
            Self::Human { issuer, subject } => serde_json::json!(["human", issuer, subject]),
            Self::Principal {
                principal,
                provider,
                issuer,
            } => serde_json::json!(["principal", provider, issuer, principal]),
        };
        let input = serde_json::json!([SCOPED_APPLY_KEY_DOMAIN, authorities, key, owner]);
        format!(
            "{SCOPED_APPLY_KEY_PREFIX}{:x}",
            Sha256::digest(input.to_string().as_bytes())
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PendingTopologyPhase {
    Applying,
    RollingBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingTopologyOperation {
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
    base_revision: u64,
    target_revision: u64,
    created_at: String,
    reason: Option<String>,
    operations: Vec<TopologyMutation>,
    phase: PendingTopologyPhase,
    /// Pre-operation intent. An `Applying` journal is not a commit record:
    /// crash recovery first restores this state, then reconciles physical
    /// wiring and closes the receipt as rolled back.
    #[serde(default)]
    rollback_additions: BTreeSet<DesiredPeerEdge>,
    #[serde(default)]
    rollback_suppressions: BTreeSet<DesiredPeerEdge>,
    #[serde(default)]
    rollback_cross_additions: BTreeSet<TopologyEdge>,
    #[serde(default)]
    rollback_cross_suppressions: BTreeSet<TopologyEdge>,
    #[serde(default)]
    rollback_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_recovery_error: Option<String>,
    #[serde(default)]
    recovery_attempts: u64,
}

impl PendingTopologyOperation {
    /// The key this operation's idempotency entry is filed under, if any
    /// (see [`filing_key`]).
    fn idempotency_map_key(&self) -> Option<&str> {
        filing_key(
            self.scoped_idempotency_key.as_deref(),
            &self.idempotency_key,
        )
    }

    /// What an apply filed under `scoped_key` gets while this operation is
    /// pending: for its own key and request, this operation's receipt; for
    /// its own key and another request, a conflict; otherwise, that an
    /// operation is in progress.
    fn answer_apply(
        &self,
        scoped_key: &str,
        fingerprint: &str,
        idempotency_key: &str,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        if self.idempotency_map_key() != Some(scoped_key) {
            return Err(TopologyControlError::OperationInProgress(
                self.operation_id.clone(),
            ));
        }
        if self.fingerprint != fingerprint {
            return Err(TopologyControlError::IdempotencyConflict(
                idempotency_key.to_string(),
            ));
        }
        Ok(pending_receipt(self))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TopologyIntentState {
    #[serde(default)]
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authority: Option<String>,
    revision: u64,
    additions: BTreeSet<DesiredPeerEdge>,
    suppressions: BTreeSet<DesiredPeerEdge>,
    #[serde(default)]
    cross_additions: BTreeSet<TopologyEdge>,
    #[serde(default)]
    cross_suppressions: BTreeSet<TopologyEdge>,
    receipts: VecDeque<TopologyOperationReceipt>,
    #[serde(default)]
    operation_records: VecDeque<TopologyOperationRecord>,
    #[serde(default)]
    last_operation_record_seq: u64,
    idempotency: BTreeMap<String, IdempotencyRecord>,
    #[serde(default)]
    compacted_idempotency: VecDeque<String>,
    /// Canonical authority-pair key -> coordinator journal path. This durable
    /// reservation prevents two host components from opening independent
    /// bilateral WALs for the same runtimes under different filenames.
    #[serde(default)]
    bilateral_coordinator_bindings: BTreeMap<String, BilateralCoordinatorBinding>,
    /// Durable write-ahead record. Desired intent is persisted together with
    /// this entry before any wire/unwire side effect. On restart, ordinary
    /// reconciliation converges that intent and closes the receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<PendingTopologyOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BilateralCoordinatorBinding {
    coordinator_id: String,
    /// Canonical path is part of the lease. Moving a journal requires an
    /// explicit adoption protocol; implicit copies fail closed.
    journal_path: String,
}

struct TopologyControllerInner {
    policy: RwLock<TopologyControlPolicy>,
    state: tokio::sync::RwLock<TopologyIntentState>,
    mutation: tokio::sync::Mutex<()>,
    persist_path: RwLock<Option<PathBuf>>,
    _lock_file: Option<std::fs::File>,
    /// Concurrent topology reads share one snapshot computation (see
    /// `TopologyRuntimeHandle::query`). Shared by every cloned handle.
    query_flights: std::sync::Mutex<TopologyQueryFlights>,
}

type TopologyQueryFlight = futures::future::Shared<
    futures::future::BoxFuture<'static, Result<TopologySnapshot, TopologyControlError>>,
>;

/// The snapshot computation now running and, while it runs, the one fresh
/// computation every caller arriving meanwhile joins. A caller never shares a
/// computation that started before it arrived, so a joined result is never
/// older than the call; nothing is cached once a computation finishes.
#[derive(Default)]
struct TopologyQueryFlights {
    seq: u64,
    running: Option<(u64, TopologyQueryFlight)>,
    next: Option<(u64, TopologyQueryFlight)>,
}

/// Durable topology intent as committed: an in-flight or interrupted mutation
/// journal is not durable intent, so its pre-operation view stands until the
/// operation commits. Read under one state lock, so a reader observes the
/// state before or after a mutation, never a half-applied one.
struct CommittedTopologyIntent {
    revision: u64,
    additions: BTreeSet<DesiredPeerEdge>,
    suppressions: BTreeSet<DesiredPeerEdge>,
    cross_additions: BTreeSet<TopologyEdge>,
    cross_suppressions: BTreeSet<TopologyEdge>,
}

/// Shared durable topology intent and operation receipt store.
#[derive(Clone)]
pub struct TopologyController {
    inner: Arc<TopologyControllerInner>,
}

impl std::fmt::Debug for TopologyController {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopologyController")
            .field("policy", &self.policy())
            .finish_non_exhaustive()
    }
}

impl Default for TopologyController {
    fn default() -> Self {
        Self::new(TopologyControlPolicy::default())
            .unwrap_or_else(|_| unreachable!("default topology policy must validate"))
    }
}

impl TopologyController {
    pub fn new(policy: TopologyControlPolicy) -> Result<Self, TopologyControlError> {
        policy.validate()?;
        if policy.mode == TopologyControlMode::Editable {
            return Err(TopologyControlError::DurableStateRequired);
        }
        Ok(Self {
            inner: Arc::new(TopologyControllerInner {
                policy: RwLock::new(policy),
                state: tokio::sync::RwLock::new(TopologyIntentState::default()),
                mutation: tokio::sync::Mutex::new(()),
                persist_path: RwLock::new(None),
                _lock_file: None,
                query_flights: std::sync::Mutex::new(TopologyQueryFlights::default()),
            }),
        })
    }

    pub fn load_or_default(
        policy: TopologyControlPolicy,
        path: impl Into<PathBuf>,
    ) -> Result<Self, TopologyControlError> {
        policy.validate()?;
        let path = path.into();
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
                "topology state is already owned at {}: {error}",
                path.display()
            ))
        })?;
        let mut state = if path.is_file() {
            let bytes = std::fs::read(&path)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| TopologyControlError::Persistence(error.to_string()))?
        } else {
            TopologyIntentState::default()
        };
        if state.schema_version > TOPOLOGY_STATE_SCHEMA_VERSION {
            return Err(TopologyControlError::Persistence(format!(
                "unsupported topology state schema version {} (maximum {})",
                state.schema_version, TOPOLOGY_STATE_SCHEMA_VERSION
            )));
        }
        drop_unscoped_reserved_entries(&mut state.idempotency);
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
        persist_state(&path, &state)?;
        Ok(Self {
            inner: Arc::new(TopologyControllerInner {
                policy: RwLock::new(policy),
                state: tokio::sync::RwLock::new(state),
                mutation: tokio::sync::Mutex::new(()),
                persist_path: RwLock::new(Some(path)),
                _lock_file: Some(lock_file),
                query_flights: std::sync::Mutex::new(TopologyQueryFlights::default()),
            }),
        })
    }

    pub(crate) async fn bind_authority(
        &self,
        authority: impl Into<String>,
    ) -> Result<(), TopologyControlError> {
        let authority = authority.into();
        let mut state = self.inner.state.read().await.clone();
        if let Some(existing) = state.authority.as_deref()
            && existing != authority
        {
            return Err(TopologyControlError::Persistence(format!(
                "topology state belongs to authority {existing:?}, not {authority:?}"
            )));
        }
        state.schema_version = TOPOLOGY_STATE_SCHEMA_VERSION;
        state.authority = Some(authority);
        self.persist_candidate(&state)?;
        *self.inner.state.write().await = state;
        Ok(())
    }

    pub fn policy(&self) -> TopologyControlPolicy {
        self.inner
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_policy(&self, policy: TopologyControlPolicy) -> Result<(), TopologyControlError> {
        policy.validate()?;
        if policy.mode == TopologyControlMode::Editable
            && self
                .inner
                .persist_path
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        {
            return Err(TopologyControlError::DurableStateRequired);
        }
        *self
            .inner
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
        Ok(())
    }

    pub async fn revision(&self) -> u64 {
        self.inner.state.read().await.revision
    }

    pub(crate) async fn has_pending(&self) -> bool {
        self.inner.state.read().await.pending.is_some()
    }

    /// Return the same-authority logical edges named by the durable recovery
    /// journal. These edges remain Meta-owned across a process restart even
    /// though the in-memory managed-edge set starts empty.
    pub(crate) async fn pending_local_recovery_edges(
        &self,
    ) -> Result<
        BTreeSet<(
            crate::identity_first::AgentIdentity,
            crate::identity_first::AgentIdentity,
        )>,
        TopologyControlError,
    > {
        let state = self.inner.state.read().await;
        let Some(pending) = state.pending.as_ref() else {
            return Ok(BTreeSet::new());
        };
        let authority = state.authority.as_deref().ok_or_else(|| {
            TopologyControlError::Persistence(
                "pending topology recovery has no bound authority".to_string(),
            )
        })?;

        pending
            .operations
            .iter()
            .filter_map(|mutation| mutation.edge.local_desired_edge(authority))
            .map(|edge| {
                let (a, b) = edge.endpoints();
                let a = crate::identity_first::AgentIdentity::parse(a).map_err(|error| {
                    TopologyControlError::InvalidRequest(format!(
                        "invalid persisted topology identity {a:?}: {error}"
                    ))
                })?;
                let b = crate::identity_first::AgentIdentity::parse(b).map_err(|error| {
                    TopologyControlError::InvalidRequest(format!(
                        "invalid persisted topology identity {b:?}: {error}"
                    ))
                })?;
                Ok(if a <= b { (a, b) } else { (b, a) })
            })
            .collect()
    }

    pub(crate) async fn pending_operation_id(&self) -> Option<String> {
        self.inner
            .state
            .read()
            .await
            .pending
            .as_ref()
            .map(|pending| pending.operation_id.clone())
    }

    pub async fn operation(
        &self,
        operation_id: &str,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        let _admission = self.mutation_guard().await;
        self.reconcile_operation_records_unlocked().await?;
        let state = self.inner.state.read().await;
        if let Some(receipt) = state
            .receipts
            .iter()
            .find(|receipt| receipt.operation_id == operation_id)
            .cloned()
        {
            return Ok(receipt);
        }
        state
            .pending
            .as_ref()
            .filter(|pending| pending.operation_id == operation_id)
            .map(pending_receipt)
            .ok_or_else(|| TopologyControlError::OperationNotFound(operation_id.to_string()))
    }

    pub async fn operation_records(
        &self,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<TopologyAuditPage, TopologyControlError> {
        let _admission = self.mutation_guard().await;
        self.reconcile_operation_records_unlocked().await?;
        let state = self.inner.state.read().await;
        audit_page(
            &state.operation_records,
            state.last_operation_record_seq,
            after_seq,
            limit,
        )
    }

    pub(crate) async fn all_operation_records_after(
        &self,
        after_seq: Option<u64>,
    ) -> Result<TopologyAuditPage, TopologyControlError> {
        let _admission = self.mutation_guard().await;
        self.reconcile_operation_records_unlocked().await?;
        let state = self.inner.state.read().await;
        audit_page(
            &state.operation_records,
            state.last_operation_record_seq,
            after_seq,
            4096,
        )
    }

    #[allow(dead_code)]
    pub(crate) async fn operation_records_newest(
        &self,
        limit: usize,
    ) -> Vec<TopologyOperationRecord> {
        let state = self.inner.state.read().await;
        state
            .operation_records
            .iter()
            .rev()
            .take(limit.clamp(1, 4096))
            .cloned()
            .collect()
    }

    async fn upsert_operation_record(
        &self,
        mut record: TopologyOperationRecord,
    ) -> Result<(), TopologyControlError> {
        let _admission = self.mutation_guard().await;
        let mut state_guard = self.inner.state.write().await;
        let mut state = state_guard.clone();
        if state
            .operation_records
            .iter()
            .any(|existing| existing.record_id == record.record_id)
        {
            return Ok(());
        }
        self.append_operation_record(&mut state, &mut record);
        // Audit admission is fail-closed: no actuator is called until this
        // record is in the same fsynced authority state.
        self.persist_candidate(&state)?;
        *state_guard = state;
        Ok(())
    }

    /// Close every durable non-terminal audit attempt against the current
    /// pending journal and receipt set. Callers must hold `mutation` so the
    /// snapshot cannot race a newly admitted mutation. Persistence happens
    /// while the in-memory write guard is already held, leaving no await gap
    /// between fsync and publishing the reconciled state in this process.
    async fn reconcile_operation_records_unlocked(&self) -> Result<(), TopologyControlError> {
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
            self.persist_candidate(&next)?;
            *state_guard = next;
        }
        Ok(())
    }

    fn append_operation_record(
        &self,
        state: &mut TopologyIntentState,
        record: &mut TopologyOperationRecord,
    ) {
        state.last_operation_record_seq = state.last_operation_record_seq.saturating_add(1);
        record.seq = state.last_operation_record_seq;
        state.operation_records.push_back(record.clone());
        let limit = self
            .policy()
            .receipt_limit
            .saturating_mul(4)
            .clamp(16, 4096);
        while state.operation_records.len() > limit {
            state.operation_records.pop_front();
        }
    }

    async fn persist_operation_record_outcome_candidate(
        &self,
        mut state: TopologyIntentState,
        record_id: &str,
        status: TopologyOperationRecordStatus,
        error: Option<&TopologyControlError>,
    ) -> Result<(), TopologyControlError> {
        let record = state
            .operation_records
            .iter_mut()
            .find(|record| record.record_id == record_id)
            .ok_or_else(|| {
                TopologyControlError::Persistence(format!(
                    "topology audit record disappeared before outcome: {record_id}"
                ))
            })?;
        set_operation_record_status(record, status, error.map(TopologyControlError::kind));
        self.persist_candidate(&state)?;
        *self.inner.state.write().await = state;
        Ok(())
    }

    pub(crate) async fn compose_declared(
        &self,
        declared: Vec<DesiredPeerEdge>,
    ) -> Vec<DesiredPeerEdge> {
        let state = self.inner.state.read().await;
        let mut desired = declared.into_iter().collect::<BTreeSet<_>>();
        desired.extend(state.additions.iter().cloned());
        for suppressed in &state.suppressions {
            desired.remove(suppressed);
        }
        desired.into_iter().collect()
    }

    pub(crate) async fn compose_managed_peer_edges(
        &self,
        declared: &[crate::identity_first::ManagedPeerEdge],
    ) -> Result<Vec<crate::identity_first::ManagedPeerEdge>, TopologyControlError> {
        let mut desired = Vec::with_capacity(declared.len());
        for edge in declared {
            desired.push(
                DesiredPeerEdge::new(edge.a().to_string(), edge.b().to_string())
                    .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))?,
            );
        }
        self.compose_declared(desired)
            .await
            .into_iter()
            .map(|edge| {
                let (a, b) = edge.endpoints();
                let a = crate::identity_first::AgentIdentity::parse(a).map_err(|error| {
                    TopologyControlError::InvalidRequest(format!(
                        "invalid persisted topology identity {a:?}: {error}"
                    ))
                })?;
                let b = crate::identity_first::AgentIdentity::parse(b).map_err(|error| {
                    TopologyControlError::InvalidRequest(format!(
                        "invalid persisted topology identity {b:?}: {error}"
                    ))
                })?;
                crate::identity_first::ManagedPeerEdge::new(a, b)
                    .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))
            })
            .collect()
    }

    async fn committed_intent(&self) -> CommittedTopologyIntent {
        let state = self.inner.state.read().await;
        match state.pending.as_ref() {
            Some(pending) => CommittedTopologyIntent {
                revision: pending.rollback_revision,
                additions: pending.rollback_additions.clone(),
                suppressions: pending.rollback_suppressions.clone(),
                cross_additions: pending.rollback_cross_additions.clone(),
                cross_suppressions: pending.rollback_cross_suppressions.clone(),
            },
            None => CommittedTopologyIntent {
                revision: state.revision,
                additions: state.additions.clone(),
                suppressions: state.suppressions.clone(),
                cross_additions: state.cross_additions.clone(),
                cross_suppressions: state.cross_suppressions.clone(),
            },
        }
    }

    /// Typed check for a reader: whether an interrupted mutation journal or a
    /// non-terminal audit record must be resolved before a snapshot.
    async fn needs_recovery_or_reconcile(&self) -> bool {
        let state = self.inner.state.read().await;
        state.pending.is_some()
            || state
                .operation_records
                .iter()
                .any(|record| !operation_record_status_is_terminal(record.status))
    }

    fn persist_candidate(&self, state: &TopologyIntentState) -> Result<(), TopologyControlError> {
        let path = self
            .inner
            .persist_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(path) = path else {
            return Ok(());
        };
        persist_state(&path, state)
    }

    fn record_receipt(
        &self,
        state: &mut TopologyIntentState,
        receipt: TopologyOperationReceipt,
        fingerprint: String,
        idempotency_map_key: Option<&str>,
    ) -> Result<(), TopologyControlError> {
        let policy = self.policy();
        if let Some(key) = idempotency_map_key {
            state.idempotency.insert(
                key.to_string(),
                IdempotencyRecord {
                    fingerprint,
                    operation_id: receipt.operation_id.clone(),
                    scoped: key != receipt.idempotency_key,
                },
            );
        }
        state.receipts.push_back(receipt);
        while state.receipts.len() > policy.receipt_limit {
            let Some(evicted) = state.receipts.pop_front() else {
                break;
            };
            // A receipt that no entry names compacts and removes nothing.
            let Some(evicted_key) = filed_idempotency_key(&state.idempotency, &evicted) else {
                continue;
            };
            let compacted = compacted_key_fingerprint(&evicted_key);
            if !state.compacted_idempotency.contains(&compacted) {
                state.compacted_idempotency.push_back(compacted);
            }
            state.idempotency.remove(&evicted_key);
        }
        while state.compacted_idempotency.len() > policy.idempotency_history_limit {
            state.compacted_idempotency.pop_front();
        }
        Ok(())
    }

    fn compacted_idempotency_contains(&self, state: &TopologyIntentState, key: &str) -> bool {
        state
            .compacted_idempotency
            .contains(&compacted_key_fingerprint(key))
    }

    pub(crate) async fn mutation_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.mutation.lock().await
    }

    /// The mutation lock if no plan, apply or recovery holds it. A mutation
    /// holds it from journal admission to commit or rollback, so a pending
    /// journal observed while the lock is free belongs to an interrupted
    /// operation.
    fn try_mutation_guard(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.inner.mutation.try_lock().ok()
    }

    pub(crate) async fn prepare_pending_recovery(&self) -> Result<(), TopologyControlError> {
        let mut state = self.inner.state.read().await.clone();
        let Some(mut pending) = state.pending.clone() else {
            return Ok(());
        };
        if pending.phase == PendingTopologyPhase::Applying {
            state.additions = pending.rollback_additions.clone();
            state.suppressions = pending.rollback_suppressions.clone();
            state.cross_additions = pending.rollback_cross_additions.clone();
            state.cross_suppressions = pending.rollback_cross_suppressions.clone();
            state.revision = pending.rollback_revision;
            pending.phase = PendingTopologyPhase::RollingBack;
            pending.target_revision = pending.rollback_revision;
            state.pending = Some(pending);
            self.persist_candidate(&state)?;
            *self.inner.state.write().await = state;
        }
        Ok(())
    }

    pub(crate) async fn finalize_recovered_pending(
        &self,
        reconcile_complete: bool,
    ) -> Result<(), TopologyControlError> {
        let mut state = self.inner.state.read().await.clone();
        let Some(mut pending) = state.pending.take() else {
            return Ok(());
        };
        if !reconcile_complete {
            pending.recovery_attempts = pending.recovery_attempts.saturating_add(1);
            pending.last_recovery_error =
                Some("topology recovery reconciliation was incomplete".to_string());
            if let Some(record) = state.operation_records.iter_mut().find(|record| {
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
            state.pending = Some(pending);
            self.persist_candidate(&state)?;
            *self.inner.state.write().await = state;
            return Ok(());
        }
        let status = TopologyOperationStatus::RolledBack;
        let receipt = terminal_receipt_from_pending(&pending, status, None);
        if let Some(record) = state.operation_records.iter_mut().find(|record| {
            (!pending.audit_record_id.is_empty() && record.record_id == pending.audit_record_id)
                || (pending.audit_record_id.is_empty()
                    && record.operation_id == pending.operation_id
                    && !operation_record_status_is_terminal(record.status))
        }) {
            set_operation_record_status(record, TopologyOperationRecordStatus::RolledBack, None);
        }
        let idempotency_map_key = pending.idempotency_map_key().map(str::to_string);
        self.record_receipt(
            &mut state,
            receipt,
            pending.fingerprint,
            idempotency_map_key.as_deref(),
        )?;
        self.persist_candidate(&state)?;
        *self.inner.state.write().await = state;
        Ok(())
    }
}

fn persist_state(path: &Path, state: &TopologyIntentState) -> Result<(), TopologyControlError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    }
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| TopologyControlError::Persistence(error.to_string()))?;
    let mut temp = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(std::ffi::OsString::from)
        .ok_or_else(|| {
            TopologyControlError::Persistence(format!(
                "topology state path has no file name: {}",
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

/// Cloneable runtime seam captured by HTTP routers and SDK-facing RPC code.
#[derive(Clone)]
pub struct TopologyRuntimeHandle {
    mob_handle: meerkat_mob::MobHandle,
    edge_discovery: Option<Arc<dyn EdgeDiscovery>>,
    managed_edges: Arc<tokio::sync::RwLock<BTreeSet<(String, String)>>>,
    controller: TopologyController,
    // Exported to routers and the host: the owning public view.
    identity_context: Option<crate::identity_first::PublicIdentityFirstContext>,
    /// Whether this handle serves a candidate launch: it then rewires no
    /// member and writes no topology journal, and its reconcile is a dry
    /// run (see [`Self::reconcile`]).
    candidate_launch: bool,
}

impl std::fmt::Debug for TopologyRuntimeHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopologyRuntimeHandle")
            .field("authority", &self.authority())
            .field("policy", &self.controller.policy())
            .finish_non_exhaustive()
    }
}

impl TopologyRuntimeHandle {
    pub(crate) fn new(
        mob_handle: meerkat_mob::MobHandle,
        edge_discovery: Option<Arc<dyn EdgeDiscovery>>,
        managed_edges: Arc<tokio::sync::RwLock<BTreeSet<(String, String)>>>,
        controller: TopologyController,
        identity_context: Option<crate::identity_first::PublicIdentityFirstContext>,
        candidate_launch: bool,
    ) -> Self {
        Self {
            mob_handle,
            edge_discovery,
            managed_edges,
            controller,
            identity_context,
            candidate_launch,
        }
    }

    pub fn authority(&self) -> String {
        self.mob_handle.mob_id().to_string()
    }

    pub fn controller(&self) -> &TopologyController {
        &self.controller
    }

    pub fn policy(&self) -> TopologyControlPolicy {
        self.controller.policy()
    }

    /// Reconcile the live wiring with the desired topology. On a candidate
    /// launch this is a dry run: it writes no topology journal and wires or
    /// unwires nothing, and the report says what an authoritative launch's
    /// reconcile would wire, unwire and prune, with a census line in the log.
    pub async fn reconcile(&self) -> UnifiedRuntimeReconcileEdgesReport {
        let _admission = self.controller.mutation_guard().await;
        if self.candidate_launch {
            return self.reconcile_current_unlocked().await;
        }
        if let Err(error) = self.controller.reconcile_operation_records_unlocked().await {
            tracing::error!(error = %error, "failed to reconcile topology audit attempts");
            return recovery_failure_report(error);
        }
        if let Err(error) = self.controller.prepare_pending_recovery().await {
            tracing::error!(error = %error, "failed to prepare topology recovery rollback");
            return recovery_failure_report(error);
        }
        let report = self.reconcile_current_unlocked().await;
        if let Err(error) = self
            .controller
            .finalize_recovered_pending(report.is_complete())
            .await
        {
            tracing::error!(error = %error, "failed to finalize recovered topology operation");
        }
        report
    }

    async fn reconcile_current_unlocked(&self) -> UnifiedRuntimeReconcileEdgesReport {
        if let Some(context) = self.identity_context.as_ref() {
            if self.candidate_launch {
                return Self::dry_run_identity_first(&self.controller, context).await;
            }
            return self.reconcile_identity_first(context).await;
        }
        let members = self.mob_handle.list_members_including_retiring().await;
        reconcile_edges_over_members(
            &self.mob_handle,
            self.edge_discovery.as_deref(),
            &self.managed_edges,
            Some(&self.controller),
            members,
            self.candidate_launch,
        )
        .await
    }

    /// A candidate launch's identity-first reconcile: what an authoritative
    /// launch's reconcile would wire and unwire, in the report's own shape,
    /// with nothing applied and no topology journal touched.
    async fn dry_run_identity_first(
        controller: &TopologyController,
        context: &crate::identity_first::IdentityFirstRuntimeContext,
    ) -> UnifiedRuntimeReconcileEdgesReport {
        let (_, provider_edges) = match context.topology_snapshot_inputs().await {
            Ok(inputs) => inputs,
            Err(error) => {
                return identity_reconcile_failure("discover", error.to_string());
            }
        };
        let desired_edges = match controller.compose_managed_peer_edges(&provider_edges).await {
            Ok(edges) => edges,
            Err(error) => return recovery_failure_report(error),
        };
        let desired = desired_edges
            .iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect::<BTreeSet<_>>();
        let census = context
            .runtime
            .census_managed_peer_edges(&desired, None)
            .await;
        census.log("topology reconcile dry run");
        let logical = |edges: &BTreeSet<(
            crate::identity_first::AgentIdentity,
            crate::identity_first::AgentIdentity,
        )>| {
            edges
                .iter()
                .filter_map(|(a, b)| DesiredPeerEdge::new(a.to_string(), b.to_string()).ok())
                .collect::<Vec<_>>()
        };
        UnifiedRuntimeReconcileEdgesReport {
            desired_edges: logical(&desired),
            wired_edges: logical(&census.would_wire),
            unwired_edges: logical(&census.would_unwire),
            retained_edges: logical(&census.retained),
            skipped_missing_members: logical(&census.pending),
            ..Default::default()
        }
    }

    /// Resolve an interrupted local mutation before exposing a snapshot or a
    /// plan derived from it. An `Applying` journal is an uncommitted target,
    /// so readers must never observe it as durable topology intent.
    async fn recover_pending_unlocked(&self) -> Result<(), TopologyControlError> {
        let Some(operation_id) = self.controller.pending_operation_id().await else {
            return Ok(());
        };
        // A candidate launch recovers nothing: the recovery rewires
        // members, and the interrupted operation is the authoritative
        // launch's to finish.
        if self.candidate_launch {
            return Err(TopologyControlError::OperationInProgress(operation_id));
        }
        self.controller.prepare_pending_recovery().await?;
        let report = self.reconcile_current_unlocked().await;
        self.controller
            .finalize_recovered_pending(report.is_complete())
            .await?;
        if report.is_complete() {
            Ok(())
        } else {
            Err(TopologyControlError::OperationInProgress(operation_id))
        }
    }

    async fn reconcile_identity_first(
        &self,
        context: &crate::identity_first::IdentityFirstRuntimeContext,
    ) -> UnifiedRuntimeReconcileEdgesReport {
        Self::reconcile_identity_first_with_controller(&self.controller, context).await
    }

    async fn reconcile_identity_first_with_controller(
        controller: &TopologyController,
        context: &crate::identity_first::IdentityFirstRuntimeContext,
    ) -> UnifiedRuntimeReconcileEdgesReport {
        let (_, provider_edges) = match context.topology_snapshot_inputs().await {
            Ok(inputs) => inputs,
            Err(error) => {
                return identity_reconcile_failure("discover", error.to_string());
            }
        };
        let desired_edges = match controller.compose_managed_peer_edges(&provider_edges).await {
            Ok(edges) => edges,
            Err(error) => return recovery_failure_report(error),
        };
        let pending_recovery_edges = match controller.pending_local_recovery_edges().await {
            Ok(edges) => edges,
            Err(error) => return recovery_failure_report(error),
        };
        context
            .runtime
            .retain_managed_peer_edges(&pending_recovery_edges)
            .await;
        let actual_edges = match context.runtime.logical_peer_edges().await {
            Ok(edges) => edges,
            Err(error) => return identity_reconcile_failure("inspect", error.to_string()),
        };
        let managed = context.runtime.managed_peer_edges_snapshot().await;
        let desired = desired_edges
            .iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect::<BTreeSet<_>>();
        let actual = actual_edges
            .iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect::<BTreeSet<_>>();
        let mut report = UnifiedRuntimeReconcileEdgesReport {
            desired_edges: desired_edges
                .iter()
                .filter_map(|edge| {
                    DesiredPeerEdge::new(edge.a().to_string(), edge.b().to_string()).ok()
                })
                .collect(),
            ..Default::default()
        };
        if let Err(error) = context
            .runtime
            .reconcile_managed_peer_edges_admitted(&desired_edges)
            .await
        {
            for edge in &desired_edges {
                if let Ok(logical) =
                    DesiredPeerEdge::new(edge.a().to_string(), edge.b().to_string())
                {
                    report.failures.push(
                        crate::unified_runtime::edge_types::EdgeReconcileFailure {
                            edge: logical,
                            operation: "reconcile".to_string(),
                            error: error.to_string(),
                        },
                    );
                }
            }
            return report;
        }
        let actual_after = match context.runtime.logical_peer_edges().await {
            Ok(edges) => edges
                .iter()
                .map(|edge| (edge.a().clone(), edge.b().clone()))
                .collect::<BTreeSet<_>>(),
            Err(error) => return identity_reconcile_failure("inspect_after", error.to_string()),
        };
        let actual_any_half_after = match context.runtime.logical_peer_edges_any_half().await {
            Ok(edges) => edges
                .iter()
                .map(|edge| (edge.a().clone(), edge.b().clone()))
                .collect::<BTreeSet<_>>(),
            Err(error) => {
                return identity_reconcile_failure("inspect_any_half_after", error.to_string());
            }
        };
        for edge in &desired_edges {
            let logical = match DesiredPeerEdge::new(edge.a().to_string(), edge.b().to_string()) {
                Ok(edge) => edge,
                Err(error) => {
                    return identity_reconcile_failure("validate", error.to_string());
                }
            };
            let key = (edge.a().clone(), edge.b().clone());
            if actual_after.contains(&key) && actual.contains(&key) {
                report.retained_edges.push(logical);
            } else if actual_after.contains(&key) {
                report.wired_edges.push(logical);
            } else {
                // Lazy identity bootstrap deliberately leaves topology edges
                // pending until both endpoints materialize. Do not claim a
                // concrete wire that the bridge never created.
                report.skipped_missing_members.push(logical);
            }
        }
        // Reconcile unwires every managed edge that is not desired and, when
        // a topology provider owns the topology, every live identity edge
        // that no declared owner wants (one wired before a restart is live
        // but unmanaged; one the definition's wiring declares is kept).
        let reconciled_away = match context.runtime.identity_edge_ownership() {
            crate::identity_first::runtime::IdentityEdgeOwnership::TopologyProvider => {
                let definition_declared =
                    context.runtime.definition_declared_identity_edges().await;
                managed
                    .iter()
                    .chain(actual.difference(&definition_declared))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            }
            crate::identity_first::runtime::IdentityEdgeOwnership::WiredByThisProcess => {
                managed.clone()
            }
        };
        for (a, b) in reconciled_away.difference(&desired) {
            let logical = match DesiredPeerEdge::new(a.to_string(), b.to_string()) {
                Ok(edge) => edge,
                Err(_) => continue,
            };
            if actual_any_half_after.contains(&(a.clone(), b.clone())) {
                report.skipped_missing_members.push(logical);
            } else {
                report.unwired_edges.push(logical);
            }
        }
        if !pending_recovery_edges.is_empty() {
            let recovery_complete = match context
                .runtime
                .pending_recovery_is_physically_complete(&desired_edges, &pending_recovery_edges)
                .await
            {
                Ok(complete) => complete,
                Err(error) => {
                    return identity_reconcile_failure("inspect_recovery", error.to_string());
                }
            };
            if !recovery_complete {
                for (a, b) in &pending_recovery_edges {
                    let Ok(logical) = DesiredPeerEdge::new(a.to_string(), b.to_string()) else {
                        continue;
                    };
                    if !report.skipped_missing_members.contains(&logical) {
                        report.skipped_missing_members.push(logical);
                    }
                }
            }
        }
        report
    }

    /// A read never takes the mutation lock to observe state: it reports the
    /// committed intent (see `CommittedTopologyIntent`), so it neither waits
    /// for a plan or apply nor makes them wait. Interrupted work is resolved
    /// first, under the mutation lock, only when it exists and no mutation is
    /// in flight. Concurrent reads share one snapshot computation, so N
    /// callers cost at most two edge discoveries instead of N.
    pub async fn query(&self) -> Result<TopologySnapshot, TopologyControlError> {
        self.recover_interrupted_for_read().await?;
        self.coalesced_query().await
    }

    async fn recover_interrupted_for_read(&self) -> Result<(), TopologyControlError> {
        // A candidate launch's reads serve the committed view and write no
        // journal.
        if self.candidate_launch || !self.controller.needs_recovery_or_reconcile().await {
            return Ok(());
        }
        // A plan, apply or another reader's recovery holds the lock and owns
        // its journal and records; this read serves the committed view.
        let Some(_admission) = self.controller.try_mutation_guard() else {
            return Ok(());
        };
        self.controller
            .reconcile_operation_records_unlocked()
            .await?;
        self.recover_pending_unlocked().await
    }

    async fn coalesced_query(&self) -> Result<TopologySnapshot, TopologyControlError> {
        let flight = {
            let mut flights = self
                .controller
                .inner
                .query_flights
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((_, next)) = flights.next.as_ref() {
                next.clone()
            } else if let Some((_, running)) = flights.running.clone() {
                let next = self.query_flight(&mut flights, Some(running));
                flights.next = Some(next.clone());
                next.1
            } else {
                let running = self.query_flight(&mut flights, None);
                flights.running = Some(running.clone());
                running.1
            }
        };
        flight.await
    }

    /// One snapshot computation. A queued flight first awaits the running one
    /// (driving it if its callers went away), and the running flight promotes
    /// the queued one as it finishes, so at most one discovery runs at a time.
    fn query_flight(
        &self,
        flights: &mut TopologyQueryFlights,
        after: Option<TopologyQueryFlight>,
    ) -> (u64, TopologyQueryFlight) {
        use futures::FutureExt as _;
        flights.seq = flights.seq.wrapping_add(1);
        let id = flights.seq;
        let handle = self.clone();
        let flight = async move {
            if let Some(after) = after {
                let _ = after.await;
            }
            let result = handle.query_unlocked().await;
            let mut flights = handle
                .controller
                .inner
                .query_flights
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if flights
                .running
                .as_ref()
                .is_some_and(|(running, _)| *running == id)
            {
                flights.running = flights.next.take();
            }
            result
        }
        .boxed()
        .shared();
        (id, flight)
    }

    async fn query_unlocked(&self) -> Result<TopologySnapshot, TopologyControlError> {
        if let Some(context) = self.identity_context.as_ref() {
            return self.query_identity_first(context).await;
        }
        let authority = self.authority();
        let members = self.mob_handle.list_members_including_retiring().await;
        let (nodes, actual, member_views) = project_members(&authority, &members)?;
        let declared = match self.edge_discovery.as_deref() {
            Some(discovery) => discovery
                .discover_edges(member_views)
                .await
                .into_iter()
                .collect::<BTreeSet<_>>(),
            None => BTreeSet::new(),
        };
        let CommittedTopologyIntent {
            revision,
            additions,
            suppressions,
            cross_additions,
            cross_suppressions,
        } = self.controller.committed_intent().await;
        let mut all = declared.clone();
        all.extend(additions.iter().cloned());
        all.extend(suppressions.iter().cloned());
        all.extend(actual.iter().cloned());
        let mut edges = all
            .into_iter()
            .map(|edge| {
                let logical = local_topology_edge(&authority, &edge)?;
                let declared_edge = declared.contains(&edge);
                let operator_added = additions.contains(&edge);
                let suppressed = suppressions.contains(&edge);
                Ok(TopologyEdgeSnapshot {
                    edge: logical,
                    actual: actual.contains(&edge),
                    declared: declared_edge,
                    operator_added,
                    suppressed,
                    desired: (declared_edge || operator_added) && !suppressed,
                })
            })
            .collect::<Result<Vec<_>, TopologyControlError>>()?;
        let mut cross = cross_additions.clone();
        cross.extend(cross_suppressions.iter().cloned());
        edges.extend(
            cross
                .into_iter()
                .filter(|edge| {
                    edge.a.authority.as_deref() == Some(authority.as_str())
                        || edge.b.authority.as_deref() == Some(authority.as_str())
                })
                .map(|edge| {
                    let operator_added = cross_additions.contains(&edge);
                    let suppressed = cross_suppressions.contains(&edge);
                    TopologyEdgeSnapshot {
                        edge,
                        // A single runtime cannot attest the other half. The
                        // bilateral coordinator reports physical status in
                        // its receipt; RPC never guesses it locally.
                        actual: false,
                        declared: false,
                        operator_added,
                        suppressed,
                        desired: operator_added && !suppressed,
                    }
                }),
        );
        edges.sort_by(|left, right| left.edge.cmp(&right.edge));
        Ok(TopologySnapshot {
            authority,
            revision,
            policy: self.policy(),
            nodes,
            edges,
        })
    }

    async fn query_identity_first(
        &self,
        context: &crate::identity_first::IdentityFirstRuntimeContext,
    ) -> Result<TopologySnapshot, TopologyControlError> {
        let authority = self.authority();
        let (roster, provider_edges) = context
            .topology_snapshot_inputs()
            .await
            .map_err(|error| TopologyControlError::Actuator(error.to_string()))?;
        let actual_edges = context
            .runtime
            .logical_peer_edges()
            .await
            .map_err(|error| TopologyControlError::Actuator(error.to_string()))?;
        let to_desired = |edge: &crate::identity_first::ManagedPeerEdge| {
            DesiredPeerEdge::new(edge.a().to_string(), edge.b().to_string())
                .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))
        };
        let declared = provider_edges
            .iter()
            .map(to_desired)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let actual = actual_edges
            .iter()
            .map(to_desired)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let intent = self.controller.committed_intent().await;
        let revision = intent.revision;
        let (additions, suppressions) = (&intent.additions, &intent.suppressions);
        let mut all = declared.clone();
        all.extend(actual.iter().cloned());
        all.extend(additions.iter().cloned());
        all.extend(suppressions.iter().cloned());
        let mut edges = all
            .into_iter()
            .map(|edge| {
                let logical = local_topology_edge(&authority, &edge)?;
                let declared_edge = declared.contains(&edge);
                let operator_added = additions.contains(&edge);
                let suppressed = suppressions.contains(&edge);
                Ok(TopologyEdgeSnapshot {
                    edge: logical,
                    actual: actual.contains(&edge),
                    declared: declared_edge,
                    operator_added,
                    suppressed,
                    desired: (declared_edge || operator_added) && !suppressed,
                })
            })
            .collect::<Result<Vec<_>, TopologyControlError>>()?;
        let mut nodes = roster
            .into_iter()
            .map(|spec| TopologyNodeSnapshot {
                endpoint: TopologyEndpoint {
                    authority: Some(authority.clone()),
                    identity: spec.identity.to_string(),
                },
                role: spec.profile.to_string(),
                labels: spec.labels,
                affordances: None,
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| left.endpoint.cmp(&right.endpoint));
        append_cross_intent(&intent, &authority, &mut edges);
        edges.sort_by(|left, right| left.edge.cmp(&right.edge));
        Ok(TopologySnapshot {
            authority,
            revision,
            policy: self.policy(),
            nodes,
            edges,
        })
    }

    pub async fn plan(
        &self,
        request: TopologyPlanRequest,
    ) -> Result<TopologyPlan, TopologyControlError> {
        let policy = self.policy();
        if policy.mode == TopologyControlMode::Disabled {
            return Err(TopologyControlError::FeatureDisabled);
        }
        validate_batch(&policy, request.operations.len())?;
        let operations = normalize_operations(request.operations, &self.authority(), &policy)?;
        validate_unique_operations(&operations)?;
        let _admission = self.controller.mutation_guard().await;
        if !self.candidate_launch {
            self.controller
                .reconcile_operation_records_unlocked()
                .await?;
        }
        self.recover_pending_unlocked().await?;
        let snapshot = self.query_unlocked().await?;
        if request.expected_revision != snapshot.revision {
            return Err(TopologyControlError::RevisionConflict {
                expected: request.expected_revision,
                actual: snapshot.revision,
            });
        }
        validate_known_endpoints(&snapshot, &operations)?;
        validate_connect_does_not_clear_suppression(&snapshot, &operations)?;
        validate_reconnect_targets(&snapshot, &operations)?;
        Ok(plan_from_snapshot(&snapshot, operations))
    }

    pub(crate) fn normalize_for_authorization(
        &self,
        operations: Vec<TopologyMutation>,
    ) -> Result<Vec<TopologyMutation>, TopologyControlError> {
        let policy = self.policy();
        normalize_operations(operations, &self.authority(), &policy)
    }

    pub async fn apply(
        &self,
        request: TopologyApplyRequest,
        actor: impl Into<String>,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        self.apply_as(request, None, actor).await
    }

    /// Apply as `principal`. Its idempotency keys are that principal's own:
    /// the same key from another principal, or from the host's [`Self::apply`],
    /// names a different operation.
    pub async fn apply_as(
        &self,
        request: TopologyApplyRequest,
        principal: Option<&str>,
        actor: impl Into<String>,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        self.apply_in_scope(
            request,
            &TopologyApplyScope::for_host_principal(principal),
            principal,
            actor,
        )
        .await
    }

    /// [`Self::apply_as`] for a caller whose surface derives its idempotency
    /// scope.
    pub(crate) async fn apply_in_scope(
        &self,
        request: TopologyApplyRequest,
        scope: &TopologyApplyScope,
        principal: Option<&str>,
        actor: impl Into<String>,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        let actor = actor.into();
        let mut normalized = request.clone();
        normalized.idempotency_key = normalized.idempotency_key.trim().to_string();
        // Parsing and non-normalizable endpoint syntax are adapter-boundary
        // failures. Every normalized mutation attempt below is durably
        // audited before policy/CAS/actuator admission.
        normalized.operations = self.normalize_for_authorization(normalized.operations)?;
        let scoped_key =
            scope.idempotency_key(&[self.authority().as_str()], &normalized.idempotency_key);
        let preflight = validate_local_apply_preflight(&self.policy(), &normalized)
            .and_then(|()| scope.validate_key(&normalized.idempotency_key));
        let mut record = operation_record_for_local_request(
            &self.authority(),
            &normalized,
            &scoped_key,
            &actor,
            principal,
            &format!("mode:{:?}", self.policy().mode),
            TopologyOperationRecordStatus::Requested,
        )?;
        if let Err(error) = &preflight {
            let (status, _) =
                operation_record_outcome(&Err::<TopologyOperationReceipt, _>(error.clone()));
            set_operation_record_status(&mut record, status, Some(error.kind()));
        }
        let record_id = record.record_id.clone();
        if let Err(error) = preflight {
            self.controller.upsert_operation_record(record).await?;
            return Err(error);
        }
        let _ = record_id;
        self.apply_inner(normalized, scoped_key, actor, record)
            .await
    }

    pub(crate) async fn record_denied_apply(
        &self,
        request: &TopologyApplyRequest,
        scope: &TopologyApplyScope,
        principal: Option<&str>,
        actor: &str,
        error: &TopologyControlError,
    ) -> Result<(), TopologyControlError> {
        let scoped_key =
            scope.idempotency_key(&[self.authority().as_str()], request.idempotency_key.trim());
        let mut record = operation_record_for_local_request(
            &self.authority(),
            request,
            &scoped_key,
            actor,
            principal,
            &format!(
                "denied:mode:{:?}:{}",
                self.policy().mode,
                chrono::Utc::now().to_rfc3339()
            ),
            TopologyOperationRecordStatus::Denied,
        )?;
        record.error_kind = Some(error.kind().to_string());
        record.error_message = Some(format!("topology operation failed ({})", error.kind()));
        self.controller.upsert_operation_record(record).await
    }

    /// Apply `request`, keyed for idempotency by `scoped_key`, the key its
    /// caller's scope files it under.
    async fn apply_inner(
        &self,
        mut request: TopologyApplyRequest,
        scoped_key: String,
        actor: String,
        mut audit_record: TopologyOperationRecord,
    ) -> Result<TopologyOperationReceipt, TopologyControlError> {
        let audit_record_id = audit_record.record_id.clone();
        let policy = self.policy();
        request.idempotency_key = request.idempotency_key.trim().to_string();
        let fingerprint = request_fingerprint(&request)?;
        let _admission = self.controller.mutation_guard().await;
        self.controller
            .reconcile_operation_records_unlocked()
            .await?;
        let mut state_guard = self.controller.inner.state.write().await;
        let mut current = state_guard.clone();
        self.controller
            .append_operation_record(&mut current, &mut audit_record);
        // Admission is durably visible before any endpoint query or actuator
        // await. Keeping the write guard across fsync means an aborted future
        // cannot leave disk ahead of this process's in-memory authority.
        self.controller.persist_candidate(&current)?;
        *state_guard = current.clone();
        drop(state_guard);

        macro_rules! audited_error {
            ($error:expr) => {{
                let error = $error;
                let (status, _) =
                    operation_record_outcome(&Err::<TopologyOperationReceipt, _>(error.clone()));
                self.controller
                    .persist_operation_record_outcome_candidate(
                        current,
                        &audit_record_id,
                        status,
                        Some(&error),
                    )
                    .await?;
                return Err(error);
            }};
        }

        if let Err(error) = validate_local_apply_preflight(&policy, &request) {
            audited_error!(error);
        }

        let pending_answer = current.pending.as_ref().map(|pending| {
            pending.answer_apply(&scoped_key, &fingerprint, &request.idempotency_key)
        });
        match pending_answer {
            Some(Ok(receipt)) => {
                self.controller
                    .persist_operation_record_outcome_candidate(
                        current,
                        &audit_record_id,
                        TopologyOperationRecordStatus::Noop,
                        None,
                    )
                    .await?;
                return Ok(receipt);
            }
            Some(Err(error)) => audited_error!(error),
            None => {}
        }
        if let Some(record) = current.idempotency.get(&scoped_key).cloned() {
            if record.fingerprint != fingerprint {
                audited_error!(TopologyControlError::IdempotencyConflict(
                    request.idempotency_key.clone(),
                ));
            }
            let receipt = current
                .receipts
                .iter()
                .find(|receipt| receipt.operation_id == record.operation_id)
                .cloned()
                .ok_or_else(|| TopologyControlError::IdempotencyReceiptExpired {
                    key: request.idempotency_key.clone(),
                    operation_id: record.operation_id.clone(),
                });
            match receipt {
                Ok(receipt) => {
                    self.controller
                        .persist_operation_record_outcome_candidate(
                            current,
                            &audit_record_id,
                            TopologyOperationRecordStatus::Noop,
                            None,
                        )
                        .await?;
                    return Ok(receipt);
                }
                Err(error) => audited_error!(error),
            }
        }
        if self
            .controller
            .compacted_idempotency_contains(&current, &scoped_key)
        {
            audited_error!(TopologyControlError::IdempotencyHistoryCompacted(
                request.idempotency_key.clone(),
            ));
        }
        if request.expected_revision != current.revision {
            audited_error!(TopologyControlError::RevisionConflict {
                expected: request.expected_revision,
                actual: current.revision,
            });
        }

        let snapshot = match self.query_unlocked().await {
            Ok(snapshot) => snapshot,
            Err(error) => audited_error!(error),
        };
        if let Err(error) = validate_known_endpoints(&snapshot, &request.operations) {
            audited_error!(error);
        }
        if let Err(error) =
            validate_connect_does_not_clear_suppression(&snapshot, &request.operations)
        {
            audited_error!(error);
        }
        if let Err(error) = validate_reconnect_targets(&snapshot, &request.operations) {
            audited_error!(error);
        }
        let plan = plan_from_snapshot(&snapshot, request.operations.clone());
        let operation_id = operation_id(&scoped_key, &fingerprint);
        let mut next = current.clone();
        let declared = snapshot
            .edges
            .iter()
            .filter(|edge| edge.declared)
            .filter_map(|edge| edge.edge.local_desired_edge(&snapshot.authority))
            .collect::<BTreeSet<_>>();
        // Compute and persist desired intent *before* touching the mob graph.
        // This is a write-ahead topology journal: a crash at any later point
        // leaves enough durable state for startup reconciliation to converge.
        for planned in &plan.operations {
            let Some(edge) = planned.edge.local_desired_edge(&snapshot.authority) else {
                audited_error!(TopologyControlError::CrossAuthorityUnsupported);
            };
            match planned.action {
                TopologyAction::Connect | TopologyAction::Reconnect => {
                    next.suppressions.remove(&edge);
                    if !declared.contains(&edge) {
                        next.additions.insert(edge);
                    }
                }
                TopologyAction::Disconnect => {
                    next.additions.remove(&edge);
                    next.suppressions.insert(edge);
                }
            }
        }
        let intent_changed =
            next.additions != current.additions || next.suppressions != current.suppressions;
        let physical_expected = plan
            .operations
            .iter()
            .any(|operation| operation.requires_physical_change);
        let changed = intent_changed || physical_expected;
        if !changed {
            let receipt = TopologyOperationReceipt {
                operation_id,
                idempotency_key: request.idempotency_key,
                actor,
                status: TopologyOperationStatus::Noop,
                base_revision: current.revision,
                revision: current.revision,
                created_at: chrono::Utc::now().to_rfc3339(),
                reason: request.reason,
                results: plan
                    .operations
                    .into_iter()
                    .map(|planned| TopologyEdgeResult {
                        action: planned.action,
                        edge: planned.edge,
                        status: TopologyEdgeResultStatus::Noop,
                        actual_before: planned.actual_before,
                        actual_after: planned.actual_before,
                        error: None,
                    })
                    .collect(),
                authority_revisions: BTreeMap::new(),
            };
            let mut audited = current.clone();
            if let Some(record) = audited
                .operation_records
                .iter_mut()
                .find(|record| record.record_id == audit_record_id)
            {
                set_operation_record_status(record, TopologyOperationRecordStatus::Noop, None);
            }
            self.controller.record_receipt(
                &mut audited,
                receipt.clone(),
                fingerprint,
                Some(&scoped_key),
            )?;
            self.controller.persist_candidate(&audited)?;
            *self.controller.inner.state.write().await = audited;
            return Ok(receipt);
        }

        next.revision = current.revision.saturating_add(1);
        let pending = PendingTopologyOperation {
            operation_id: operation_id.clone(),
            audit_record_id: audit_record_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            scoped_idempotency_key: (scoped_key != request.idempotency_key)
                .then(|| scoped_key.clone()),
            fingerprint: fingerprint.clone(),
            actor: actor.clone(),
            base_revision: current.revision,
            target_revision: next.revision,
            created_at: chrono::Utc::now().to_rfc3339(),
            reason: request.reason.clone(),
            operations: request.operations.clone(),
            phase: PendingTopologyPhase::Applying,
            rollback_additions: current.additions.clone(),
            rollback_suppressions: current.suppressions.clone(),
            rollback_cross_additions: current.cross_additions.clone(),
            rollback_cross_suppressions: current.cross_suppressions.clone(),
            rollback_revision: current.revision,
            last_recovery_error: None,
            recovery_attempts: 0,
        };
        next.pending = Some(pending.clone());
        if let Some(record) = next
            .operation_records
            .iter_mut()
            .find(|record| record.record_id == audit_record_id)
        {
            set_operation_record_status(record, TopologyOperationRecordStatus::Pending, None);
        }
        self.controller.persist_candidate(&next)?;
        *self.controller.inner.state.write().await = next.clone();

        let mut results = Vec::with_capacity(plan.operations.len());
        let mut physical_changes: Vec<(DesiredPeerEdge, bool)> = Vec::new();
        let mut failed: Option<String> = None;
        for planned in &plan.operations {
            let Some(edge) = planned.edge.local_desired_edge(&snapshot.authority) else {
                failed = Some("cross-authority operation reached local actuator".to_string());
                break;
            };
            let (actual_before, physical) = if let Some(context) = self.identity_context.as_ref() {
                let actual_before = planned.actual_before;
                let managed_edge = managed_peer_edge_from_desired(&edge);
                let physical = match managed_edge {
                    Ok(managed_edge) if planned.requires_physical_change => context
                        .runtime
                        .mutate_managed_peer_edge_admitted(planned.action, &managed_edge)
                        .await
                        .map(|()| true)
                        .map_err(|error| error.to_string()),
                    Ok(_) => Ok(false),
                    Err(error) => Err(error.to_string()),
                };
                (actual_before, physical)
            } else {
                let (a_to_b, b_to_a) = match edge_actual_state(&self.mob_handle, &edge).await {
                    Ok(actual) => actual,
                    Err(error) => {
                        let message = error.to_string();
                        results.push(TopologyEdgeResult {
                            action: planned.action,
                            edge: planned.edge.clone(),
                            status: TopologyEdgeResultStatus::Failed,
                            actual_before: false,
                            actual_after: false,
                            error: Some(message.clone()),
                        });
                        failed = Some(message);
                        break;
                    }
                };
                let actual_before = a_to_b && b_to_a;
                let has_any_half = a_to_b || b_to_a;
                let physical = match planned.action {
                    TopologyAction::Connect if !actual_before => {
                        reconnect_edge(&self.mob_handle, &edge, has_any_half)
                            .await
                            .map(|()| true)
                    }
                    TopologyAction::Reconnect => {
                        reconnect_edge(&self.mob_handle, &edge, has_any_half)
                            .await
                            .map(|()| true)
                    }
                    TopologyAction::Disconnect if has_any_half => {
                        unwire_edge(&self.mob_handle, &edge).await.map(|()| true)
                    }
                    _ => Ok(false),
                };
                (actual_before, physical)
            };
            match physical {
                Ok(physical_changed) => {
                    if physical_changed {
                        physical_changes.push((edge, actual_before));
                    }
                    results.push(TopologyEdgeResult {
                        action: planned.action,
                        edge: planned.edge.clone(),
                        status: if physical_changed || planned.requires_intent_change {
                            TopologyEdgeResultStatus::Applied
                        } else {
                            TopologyEdgeResultStatus::Noop
                        },
                        actual_before,
                        actual_after: !matches!(planned.action, TopologyAction::Disconnect),
                        error: None,
                    });
                }
                Err(error) => {
                    results.push(TopologyEdgeResult {
                        action: planned.action,
                        edge: planned.edge.clone(),
                        status: TopologyEdgeResultStatus::Failed,
                        actual_before,
                        actual_after: actual_before,
                        error: Some(error.clone()),
                    });
                    failed = Some(error);
                    break;
                }
            }
        }

        if let Some(error) = failed {
            // Persist the pre-operation intent and a rolling-back journal
            // before issuing inverse physical operations. A crash now causes
            // restart reconciliation to finish the rollback, not resurrect
            // the failed requested topology.
            let mut rollback_state = current.clone();
            let mut rollback_pending = pending;
            rollback_pending.phase = PendingTopologyPhase::RollingBack;
            rollback_pending.target_revision = current.revision;
            rollback_state.pending = Some(rollback_pending.clone());
            if let Some(record) = rollback_state
                .operation_records
                .iter_mut()
                .find(|record| record.record_id == audit_record_id)
            {
                set_operation_record_status(record, TopologyOperationRecordStatus::Pending, None);
            }
            if let Err(_persist_error) = self.controller.persist_candidate(&rollback_state) {
                let mut degraded = pending_receipt(&rollback_pending);
                degraded.status = TopologyOperationStatus::PartialDegraded;
                degraded.results = results;
                return Ok(degraded);
            }
            *self.controller.inner.state.write().await = rollback_state.clone();

            let rollback_failures = match self.identity_context.as_ref() {
                Some(context) => rollback_identity_physical(context, &physical_changes).await,
                None => rollback_physical(&self.mob_handle, &physical_changes).await,
            };
            for result in &mut results {
                if matches!(result.status, TopologyEdgeResultStatus::Applied) {
                    if let Some((_, rollback_error)) = rollback_failures.iter().find(|(edge, _)| {
                        result.edge.local_desired_edge(&snapshot.authority).as_ref() == Some(edge)
                    }) {
                        result.status = TopologyEdgeResultStatus::RollbackFailed;
                        result.error = Some(rollback_error.clone());
                    } else {
                        result.status = TopologyEdgeResultStatus::RolledBack;
                        result.actual_after = result.actual_before;
                    }
                }
            }
            if !rollback_failures.is_empty() {
                if let Some(pending) = rollback_state.pending.as_mut() {
                    pending.recovery_attempts = pending.recovery_attempts.saturating_add(1);
                    pending.last_recovery_error =
                        Some("topology rollback reconciliation was incomplete".to_string());
                }
                if let Some(record) = rollback_state
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
                self.controller.persist_candidate(&rollback_state)?;
                *self.controller.inner.state.write().await = rollback_state;
                let mut degraded = pending_receipt(&rollback_pending);
                degraded.status = TopologyOperationStatus::PartialDegraded;
                degraded.results = results;
                return Ok(degraded);
            }
            let receipt = TopologyOperationReceipt {
                operation_id,
                idempotency_key: request.idempotency_key,
                actor,
                status: TopologyOperationStatus::RolledBack,
                base_revision: current.revision,
                revision: current.revision,
                created_at: rollback_pending.created_at,
                reason: request.reason,
                results,
                authority_revisions: BTreeMap::new(),
            };
            rollback_state.pending = None;
            if let Some(record) = rollback_state
                .operation_records
                .iter_mut()
                .find(|record| record.record_id == audit_record_id)
            {
                set_operation_record_status(
                    record,
                    TopologyOperationRecordStatus::RolledBack,
                    Some("topology_apply_failed"),
                );
            }
            self.controller.record_receipt(
                &mut rollback_state,
                receipt.clone(),
                fingerprint,
                Some(&scoped_key),
            )?;
            self.controller.persist_candidate(&rollback_state)?;
            *self.controller.inner.state.write().await = rollback_state;
            return Err(TopologyControlError::ApplyFailed {
                message: error,
                receipt: Box::new(receipt),
            });
        }

        let receipt = TopologyOperationReceipt {
            operation_id,
            idempotency_key: request.idempotency_key,
            actor,
            status: TopologyOperationStatus::Applied,
            base_revision: current.revision,
            revision: next.revision,
            created_at: pending.created_at,
            reason: request.reason,
            results,
            authority_revisions: BTreeMap::new(),
        };
        next.pending = None;
        if let Some(record) = next
            .operation_records
            .iter_mut()
            .find(|record| record.record_id == audit_record_id)
        {
            set_operation_record_status(record, TopologyOperationRecordStatus::Applied, None);
        }
        self.controller.record_receipt(
            &mut next,
            receipt.clone(),
            fingerprint,
            Some(&scoped_key),
        )?;
        // If terminal receipt persistence fails, leave the already-persisted
        // applying journal in memory and on disk. Reconciliation can safely
        // close it later; never erase the recovery record after side effects.
        self.controller.persist_candidate(&next)?;
        *self.controller.inner.state.write().await = next;
        Ok(receipt)
    }
}

pub(crate) fn recovery_failure_report(
    error: TopologyControlError,
) -> UnifiedRuntimeReconcileEdgesReport {
    let edge = match DesiredPeerEdge::new("topology-control", "recovery-journal") {
        Ok(edge) => edge,
        Err(_) => return UnifiedRuntimeReconcileEdgesReport::default(),
    };
    UnifiedRuntimeReconcileEdgesReport {
        failures: vec![crate::unified_runtime::edge_types::EdgeReconcileFailure {
            edge,
            operation: "prepare_recovery".to_string(),
            error: error.to_string(),
        }],
        ..Default::default()
    }
}

fn identity_reconcile_failure(
    operation: &str,
    error: String,
) -> UnifiedRuntimeReconcileEdgesReport {
    let edge = match DesiredPeerEdge::new("topology-control", "identity-reconcile") {
        Ok(edge) => edge,
        Err(_) => return UnifiedRuntimeReconcileEdgesReport::default(),
    };
    UnifiedRuntimeReconcileEdgesReport {
        failures: vec![crate::unified_runtime::edge_types::EdgeReconcileFailure {
            edge,
            operation: operation.to_string(),
            error,
        }],
        ..Default::default()
    }
}

fn validate_batch(
    policy: &TopologyControlPolicy,
    operation_count: usize,
) -> Result<(), TopologyControlError> {
    if operation_count == 0 {
        return Err(TopologyControlError::InvalidRequest(
            "operations must not be empty".to_string(),
        ));
    }
    if operation_count > 1 && !policy.allow_bulk {
        return Err(TopologyControlError::BulkDisabled);
    }
    if operation_count > policy.max_batch_size {
        return Err(TopologyControlError::BatchTooLarge {
            count: operation_count,
            max: policy.max_batch_size,
        });
    }
    Ok(())
}

fn validate_local_apply_preflight(
    policy: &TopologyControlPolicy,
    request: &TopologyApplyRequest,
) -> Result<(), TopologyControlError> {
    match policy.mode {
        TopologyControlMode::Disabled => return Err(TopologyControlError::FeatureDisabled),
        TopologyControlMode::ReadOnly => return Err(TopologyControlError::ReadOnly),
        TopologyControlMode::Editable => {}
    }
    validate_batch(policy, request.operations.len())?;
    if request.idempotency_key.trim().is_empty() {
        return Err(TopologyControlError::InvalidRequest(
            "idempotency_key must not be empty".to_string(),
        ));
    }
    if let Some(tier) = request.risk_tier.as_deref()
        && !tier.trim().is_empty()
        && !tier.eq_ignore_ascii_case("r0")
    {
        return Err(TopologyControlError::ApprovalUnsupported(tier.to_string()));
    }
    validate_unique_operations(&request.operations)
}

fn validate_known_endpoints(
    snapshot: &TopologySnapshot,
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

fn validate_unique_operations(operations: &[TopologyMutation]) -> Result<(), TopologyControlError> {
    let mut seen = BTreeSet::new();
    for operation in operations {
        if !seen.insert(operation.edge.clone()) {
            return Err(TopologyControlError::InvalidRequest(format!(
                "topology batch repeats canonical edge {} <-> {}; split sequential intent changes into separate CAS revisions",
                operation.edge.a.identity, operation.edge.b.identity
            )));
        }
    }
    Ok(())
}

fn validate_reconnect_targets(
    snapshot: &TopologySnapshot,
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

fn validate_connect_does_not_clear_suppression(
    snapshot: &TopologySnapshot,
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

fn normalize_operations(
    operations: Vec<TopologyMutation>,
    local_authority: &str,
    policy: &TopologyControlPolicy,
) -> Result<Vec<TopologyMutation>, TopologyControlError> {
    operations
        .into_iter()
        .map(|operation| {
            let edge = operation.edge.normalize(local_authority)?;
            if !edge.is_local_to(local_authority) {
                if !policy.allow_cross_authority {
                    return Err(TopologyControlError::CrossAuthorityDisabled);
                }
                return Err(TopologyControlError::CrossAuthorityUnsupported);
            }
            Ok(TopologyMutation {
                action: operation.action,
                edge,
            })
        })
        .collect()
}

fn plan_from_snapshot(
    snapshot: &TopologySnapshot,
    operations: Vec<TopologyMutation>,
) -> TopologyPlan {
    let by_edge = snapshot
        .edges
        .iter()
        .map(|edge| (edge.edge.clone(), edge))
        .collect::<BTreeMap<_, _>>();
    let operations = operations
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
                requires_intent_change: desired != desired_after
                    || (matches!(operation.action, TopologyAction::Reconnect)
                        && before.is_some_and(|edge| edge.suppressed)),
            }
        })
        .collect();
    TopologyPlan {
        authority: snapshot.authority.clone(),
        base_revision: snapshot.revision,
        operations,
    }
}

type ProjectedMembers = (
    Vec<TopologyNodeSnapshot>,
    BTreeSet<DesiredPeerEdge>,
    Vec<EdgeMemberView>,
);

fn project_members(
    authority: &str,
    members: &[MobMemberListEntry],
) -> Result<ProjectedMembers, TopologyControlError> {
    let alias = |identity: &str| runtime_alias_str(identity).into_owned();
    let mut nodes = Vec::with_capacity(members.len());
    let mut actual = BTreeSet::new();
    let mut directed = BTreeSet::new();
    let mut views = Vec::with_capacity(members.len());
    for member in members {
        let identity = alias(member.agent_identity.as_str());
        for peer in &member.wired_to {
            directed.insert((identity.clone(), alias(peer.as_str())));
        }
        nodes.push(TopologyNodeSnapshot {
            endpoint: TopologyEndpoint {
                authority: Some(authority.to_string()),
                identity: identity.clone(),
            },
            role: member.role.to_string(),
            labels: member.labels.clone(),
            affordances: None,
        });
        views.push(EdgeMemberView {
            agent_identity: identity,
            role: member.role.to_string(),
            wired_to: member
                .wired_to
                .iter()
                .map(|peer| alias(peer.as_str()))
                .collect(),
            labels: member.labels.clone(),
        });
    }
    for (a, b) in &directed {
        if directed.contains(&(b.clone(), a.clone()))
            && let Ok(edge) = DesiredPeerEdge::new(a.clone(), b.clone())
        {
            actual.insert(edge);
        }
    }
    nodes.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
    Ok((nodes, actual, views))
}

fn local_topology_edge(
    authority: &str,
    edge: &DesiredPeerEdge,
) -> Result<TopologyEdge, TopologyControlError> {
    let (a, b) = edge.endpoints();
    TopologyEdge::new(
        TopologyEndpoint {
            authority: Some(authority.to_string()),
            identity: a.to_string(),
        },
        TopologyEndpoint {
            authority: Some(authority.to_string()),
            identity: b.to_string(),
        },
    )
}

fn managed_peer_edge_from_desired(
    edge: &DesiredPeerEdge,
) -> Result<crate::identity_first::ManagedPeerEdge, TopologyControlError> {
    let (a, b) = edge.endpoints();
    let a = crate::identity_first::AgentIdentity::parse(a).map_err(|error| {
        TopologyControlError::InvalidRequest(format!("invalid logical identity {a:?}: {error}"))
    })?;
    let b = crate::identity_first::AgentIdentity::parse(b).map_err(|error| {
        TopologyControlError::InvalidRequest(format!("invalid logical identity {b:?}: {error}"))
    })?;
    crate::identity_first::ManagedPeerEdge::new(a, b)
        .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))
}

fn append_cross_intent(
    intent: &CommittedTopologyIntent,
    authority: &str,
    edges: &mut Vec<TopologyEdgeSnapshot>,
) {
    let (cross_additions, cross_suppressions) =
        (&intent.cross_additions, &intent.cross_suppressions);
    let mut cross = cross_additions.clone();
    cross.extend(cross_suppressions.iter().cloned());
    edges.extend(
        cross
            .into_iter()
            .filter(|edge| {
                edge.a.authority.as_deref() == Some(authority)
                    || edge.b.authority.as_deref() == Some(authority)
            })
            .map(|edge| {
                let operator_added = cross_additions.contains(&edge);
                let suppressed = cross_suppressions.contains(&edge);
                TopologyEdgeSnapshot {
                    edge,
                    actual: false,
                    declared: false,
                    operator_added,
                    suppressed,
                    desired: operator_added && !suppressed,
                }
            }),
    );
}

async fn edge_actual_state(
    handle: &meerkat_mob::MobHandle,
    edge: &DesiredPeerEdge,
) -> Result<(bool, bool), TopologyControlError> {
    let (a, b) = edge.endpoints();
    let a_id = mob_member_id(a);
    let b_id = mob_member_id(b);
    let a_member = handle
        .get_member(&a_id)
        .await
        .map_err(|error| TopologyControlError::Actuator(error.to_string()))?
        .ok_or_else(|| TopologyControlError::MemberNotFound(a.to_string()))?;
    let b_member = handle
        .get_member(&b_id)
        .await
        .map_err(|error| TopologyControlError::Actuator(error.to_string()))?
        .ok_or_else(|| TopologyControlError::MemberNotFound(b.to_string()))?;
    let a_to_b = a_member
        .wired_to
        .iter()
        .any(|peer| runtime_alias_str(peer.as_str()).as_ref() == b);
    let b_to_a = b_member
        .wired_to
        .iter()
        .any(|peer| runtime_alias_str(peer.as_str()).as_ref() == a);
    Ok((a_to_b, b_to_a))
}

async fn reconnect_edge(
    handle: &meerkat_mob::MobHandle,
    edge: &DesiredPeerEdge,
    has_any_half: bool,
) -> Result<(), String> {
    if has_any_half {
        unwire_edge(handle, edge).await?;
    }
    wire_edge(handle, edge).await
}

async fn wire_edge(handle: &meerkat_mob::MobHandle, edge: &DesiredPeerEdge) -> Result<(), String> {
    let (a, b) = edge.endpoints();
    let report = handle
        .wire_members_batch(vec![(mob_member_id(a), mob_member_id(b))])
        .await
        .map_err(|error| error.to_string())?;
    if report.wired.is_empty() && report.already_wired.is_empty() {
        return Err("wire_members_batch omitted edge from report".to_string());
    }
    Ok(())
}

async fn unwire_edge(
    handle: &meerkat_mob::MobHandle,
    edge: &DesiredPeerEdge,
) -> Result<(), String> {
    let (a, b) = edge.endpoints();
    handle
        .unwire(mob_member_id(a), mob_member_id(b))
        .await
        .map_err(|error| error.to_string())
}

async fn rollback_physical(
    handle: &meerkat_mob::MobHandle,
    changes: &[(DesiredPeerEdge, bool)],
) -> Vec<(DesiredPeerEdge, String)> {
    let mut failures = Vec::new();
    for (edge, was_connected) in changes.iter().rev() {
        let result = if *was_connected {
            wire_edge(handle, edge).await
        } else {
            unwire_edge(handle, edge).await
        };
        if let Err(error) = result {
            failures.push((edge.clone(), error));
        }
    }
    failures
}

async fn rollback_identity_physical(
    context: &crate::identity_first::IdentityFirstRuntimeContext,
    changes: &[(DesiredPeerEdge, bool)],
) -> Vec<(DesiredPeerEdge, String)> {
    let mut failures = Vec::new();
    for (edge, was_connected) in changes.iter().rev() {
        let managed = match managed_peer_edge_from_desired(edge) {
            Ok(edge) => edge,
            Err(error) => {
                failures.push((edge.clone(), error.to_string()));
                continue;
            }
        };
        let action = if *was_connected {
            TopologyAction::Connect
        } else {
            TopologyAction::Disconnect
        };
        if let Err(error) = context
            .runtime
            .mutate_managed_peer_edge_admitted(action, &managed)
            .await
        {
            failures.push((edge.clone(), error.to_string()));
        }
    }
    failures
}

fn request_fingerprint(request: &TopologyApplyRequest) -> Result<String, TopologyControlError> {
    let bytes = serde_json::to_vec(request)
        .map_err(|error| TopologyControlError::InvalidRequest(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn operation_record_for_local_request(
    authority: &str,
    request: &TopologyApplyRequest,
    scoped_key: &str,
    actor: &str,
    principal: Option<&str>,
    admission_scope: &str,
    status: TopologyOperationRecordStatus,
) -> Result<TopologyOperationRecord, TopologyControlError> {
    let fingerprint = request_fingerprint(request)?;
    let operation_id = operation_id(scoped_key, &fingerprint);
    let now = chrono::Utc::now().to_rfc3339();
    let _ = admission_scope;
    let record_id = new_audit_record_id();
    Ok(TopologyOperationRecord {
        version: TOPOLOGY_OPERATION_RECORD_VERSION,
        seq: 0,
        record_id,
        operation_id,
        idempotency_key_fingerprint: idempotency_key_fingerprint(request.idempotency_key.trim()),
        actor: actor.to_string(),
        principal: principal.map(str::to_string),
        authorities: vec![authority.to_string()],
        operations: request.operations.clone(),
        status,
        requested_at: now.clone(),
        updated_at: now,
        error_kind: None,
        error_message: None,
    })
}

fn operation_record_outcome(
    result: &Result<TopologyOperationReceipt, TopologyControlError>,
) -> (TopologyOperationRecordStatus, Option<&TopologyControlError>) {
    match result {
        Ok(receipt) => (
            match receipt.status {
                // A returned pending receipt here is an idempotent observation
                // of another attempt's durable WAL, not ownership of it.
                TopologyOperationStatus::Pending => TopologyOperationRecordStatus::Noop,
                TopologyOperationStatus::Applied => TopologyOperationRecordStatus::Applied,
                TopologyOperationStatus::Noop => TopologyOperationRecordStatus::Noop,
                TopologyOperationStatus::RolledBack => TopologyOperationRecordStatus::RolledBack,
                TopologyOperationStatus::PartialDegraded => {
                    TopologyOperationRecordStatus::PartialDegraded
                }
            },
            None,
        ),
        Err(error) => {
            let status = match error {
                TopologyControlError::FeatureDisabled
                | TopologyControlError::ReadOnly
                | TopologyControlError::AccessDenied { .. }
                | TopologyControlError::DurableStateRequired => {
                    TopologyOperationRecordStatus::Denied
                }
                TopologyControlError::RevisionConflict { .. }
                | TopologyControlError::IdempotencyConflict(_)
                | TopologyControlError::IdempotencyReceiptExpired { .. }
                | TopologyControlError::IdempotencyHistoryCompacted(_) => {
                    TopologyOperationRecordStatus::Conflict
                }
                TopologyControlError::OperationInProgress(_) => {
                    TopologyOperationRecordStatus::Conflict
                }
                TopologyControlError::ApplyFailed { receipt, .. } => match receipt.status {
                    TopologyOperationStatus::RolledBack => {
                        TopologyOperationRecordStatus::RolledBack
                    }
                    TopologyOperationStatus::PartialDegraded => {
                        TopologyOperationRecordStatus::PartialDegraded
                    }
                    _ => TopologyOperationRecordStatus::RecoveryFailed,
                },
                TopologyControlError::Actuator(_) | TopologyControlError::Persistence(_) => {
                    TopologyOperationRecordStatus::RecoveryFailed
                }
                _ => TopologyOperationRecordStatus::Invalid,
            };
            (status, Some(error))
        }
    }
}

fn operation_record_status_is_terminal(status: TopologyOperationRecordStatus) -> bool {
    matches!(
        status,
        TopologyOperationRecordStatus::Denied
            | TopologyOperationRecordStatus::Invalid
            | TopologyOperationRecordStatus::Conflict
            | TopologyOperationRecordStatus::Applied
            | TopologyOperationRecordStatus::Noop
            | TopologyOperationRecordStatus::RolledBack
            | TopologyOperationRecordStatus::Interrupted
    )
}

fn set_operation_record_status(
    record: &mut TopologyOperationRecord,
    status: TopologyOperationRecordStatus,
    error_kind: Option<&str>,
) {
    if operation_record_status_is_terminal(record.status) {
        return;
    }
    record.status = status;
    record.updated_at = chrono::Utc::now().to_rfc3339();
    record.error_kind = error_kind.map(str::to_string);
    // Persist only a bounded typed summary. Raw actuator/persistence errors
    // can contain filesystem paths, transport addresses, or provider text.
    record.error_message = error_kind.map(|kind| format!("topology operation failed ({kind})"));
}

fn normalize_operation_record_sequences(
    records: &mut VecDeque<TopologyOperationRecord>,
    last_seq: &mut u64,
) {
    let mut next = (*last_seq).max(records.iter().map(|record| record.seq).max().unwrap_or(0));
    for record in records {
        if record.seq == 0 {
            next = next.saturating_add(1);
            record.seq = next;
        }
    }
    *last_seq = next;
}

fn reconcile_persisted_operation_records(
    records: &mut VecDeque<TopologyOperationRecord>,
    receipts: &VecDeque<TopologyOperationReceipt>,
    pending: Option<(&str, &str, bool)>,
) {
    for record in records {
        if operation_record_status_is_terminal(record.status) {
            continue;
        }
        if let Some((operation_id, audit_record_id, degraded)) = pending
            && ((!audit_record_id.is_empty() && record.record_id == audit_record_id)
                || (audit_record_id.is_empty() && record.operation_id == operation_id))
        {
            set_operation_record_status(
                record,
                if degraded {
                    TopologyOperationRecordStatus::RecoveryFailed
                } else {
                    TopologyOperationRecordStatus::Pending
                },
                degraded.then_some("topology_actuator_failed"),
            );
            continue;
        }
        if let Some(receipt) = receipts
            .iter()
            .find(|receipt| receipt.operation_id == record.operation_id)
        {
            let status = match receipt.status {
                TopologyOperationStatus::Pending => TopologyOperationRecordStatus::Noop,
                TopologyOperationStatus::Applied => TopologyOperationRecordStatus::Applied,
                TopologyOperationStatus::Noop => TopologyOperationRecordStatus::Noop,
                TopologyOperationStatus::RolledBack => TopologyOperationRecordStatus::RolledBack,
                TopologyOperationStatus::PartialDegraded => {
                    TopologyOperationRecordStatus::Interrupted
                }
            };
            set_operation_record_status(record, status, None);
            continue;
        }
        set_operation_record_status(
            record,
            TopologyOperationRecordStatus::Interrupted,
            Some("topology_operation_interrupted"),
        );
    }
}

fn audit_page(
    records: &VecDeque<TopologyOperationRecord>,
    latest_seq: u64,
    after_seq: Option<u64>,
    limit: usize,
) -> Result<TopologyAuditPage, TopologyControlError> {
    let oldest_available_seq = records.front().map(|record| record.seq);
    let requested = after_seq.unwrap_or_else(|| {
        oldest_available_seq
            .map(|oldest| oldest.saturating_sub(1))
            .unwrap_or(latest_seq)
    });
    if after_seq.is_some()
        && requested != 0
        && oldest_available_seq.is_some_and(|oldest| requested.saturating_add(1) < oldest)
    {
        return Err(TopologyControlError::AuditCursorExpired {
            after_seq: requested,
            oldest_available_seq: oldest_available_seq.unwrap_or(0),
        });
    }
    let limit = limit.clamp(1, 4096);
    let mut matching = records.iter().filter(|record| record.seq > requested);
    let page_records = matching.by_ref().take(limit).cloned().collect::<Vec<_>>();
    let has_more = matching.next().is_some();
    let next_after_seq = page_records
        .last()
        .map(|record| record.seq)
        .unwrap_or(requested);
    Ok(TopologyAuditPage {
        records: page_records,
        next_after_seq,
        oldest_available_seq,
        latest_seq,
        has_more,
    })
}

/// The idempotency map key `receipt`'s entry is filed under, found by the
/// operation it names; none when no entry names it.
fn filed_idempotency_key(
    idempotency: &BTreeMap<String, IdempotencyRecord>,
    receipt: &TopologyOperationReceipt,
) -> Option<String> {
    idempotency
        .iter()
        .find(|(_, record)| record.operation_id == receipt.operation_id)
        .map(|(key, _)| key.clone())
}

/// The key an apply's idempotency entry is filed under: its scoped key, else
/// its own key. An unscoped key that begins with the reserved prefix (filed
/// before keys were scoped) names no caller's apply, so such an apply is
/// filed under nothing.
fn filing_key<'a>(scoped_key: Option<&'a str>, key: &'a str) -> Option<&'a str> {
    match scoped_key {
        Some(scoped) => Some(scoped),
        None if key.starts_with(SCOPED_APPLY_KEY_PREFIX) => None,
        None => Some(key),
    }
}

/// Drop the entries filed under a raw key that begins with the reserved
/// prefix, before keys were scoped. No caller presents such a key now (the
/// host may not, and a scoped caller's key is a digest), so none names a
/// caller's apply.
fn drop_unscoped_reserved_entries(idempotency: &mut BTreeMap<String, IdempotencyRecord>) {
    idempotency.retain(|key, record| record.scoped || !key.starts_with(SCOPED_APPLY_KEY_PREFIX));
}

/// The compacted-history entry of an idempotency map key. Only scoped keys
/// are filed with the reserved prefix, and their entries are marked as such,
/// so a bare fingerprint compacted from a raw key before keys were scoped
/// never answers a scoped apply; a host key's entry keeps its bare form.
fn compacted_key_fingerprint(map_key: &str) -> String {
    let fingerprint = idempotency_key_fingerprint(map_key);
    if map_key.starts_with(SCOPED_APPLY_KEY_PREFIX) {
        format!("scoped:{fingerprint}")
    } else {
        fingerprint
    }
}

fn idempotency_key_fingerprint(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

fn new_audit_record_id() -> String {
    format!("topology-audit-{}", uuid::Uuid::new_v4())
}

fn operation_id(idempotency_key: &str, fingerprint: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("meta-topology:{idempotency_key}:{fingerprint}").as_bytes(),
    )
    .to_string()
}

fn pending_receipt(pending: &PendingTopologyOperation) -> TopologyOperationReceipt {
    TopologyOperationReceipt {
        operation_id: pending.operation_id.clone(),
        idempotency_key: pending.idempotency_key.clone(),
        actor: pending.actor.clone(),
        status: if pending.last_recovery_error.is_some() {
            TopologyOperationStatus::PartialDegraded
        } else {
            TopologyOperationStatus::Pending
        },
        base_revision: pending.base_revision,
        revision: pending.target_revision,
        created_at: pending.created_at.clone(),
        reason: pending.reason.clone(),
        results: pending
            .operations
            .iter()
            .map(|operation| TopologyEdgeResult {
                action: operation.action,
                edge: operation.edge.clone(),
                status: if pending.last_recovery_error.is_some() {
                    TopologyEdgeResultStatus::RollbackFailed
                } else {
                    TopologyEdgeResultStatus::Pending
                },
                actual_before: false,
                actual_after: false,
                error: pending.last_recovery_error.clone(),
            })
            .collect(),
        authority_revisions: BTreeMap::new(),
    }
}

fn terminal_receipt_from_pending(
    pending: &PendingTopologyOperation,
    status: TopologyOperationStatus,
    error: Option<String>,
) -> TopologyOperationReceipt {
    let edge_status = match status {
        TopologyOperationStatus::Applied => TopologyEdgeResultStatus::Applied,
        TopologyOperationStatus::RolledBack => TopologyEdgeResultStatus::RolledBack,
        TopologyOperationStatus::Noop => TopologyEdgeResultStatus::Noop,
        TopologyOperationStatus::Pending => TopologyEdgeResultStatus::Pending,
        TopologyOperationStatus::PartialDegraded => TopologyEdgeResultStatus::RollbackFailed,
    };
    TopologyOperationReceipt {
        operation_id: pending.operation_id.clone(),
        idempotency_key: pending.idempotency_key.clone(),
        actor: pending.actor.clone(),
        status,
        base_revision: pending.base_revision,
        revision: pending.target_revision,
        created_at: pending.created_at.clone(),
        reason: pending.reason.clone(),
        results: pending
            .operations
            .iter()
            .map(|operation| TopologyEdgeResult {
                action: operation.action,
                edge: operation.edge.clone(),
                status: edge_status,
                actual_before: false,
                actual_after: matches!(
                    operation.action,
                    TopologyAction::Connect | TopologyAction::Reconnect
                ),
                error: error.clone(),
            })
            .collect(),
        authority_revisions: BTreeMap::new(),
    }
}

/// Typed control-plane failure. RPC adapters preserve `kind()` in error data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyControlError {
    InvalidPolicy(String),
    InvalidRequest(String),
    DurableStateRequired,
    ReconnectTargetMissing(TopologyEdge),
    DisconnectTargetMissing(TopologyEdge),
    ReconnectRequired(TopologyEdge),
    FeatureDisabled,
    ReadOnly,
    BulkDisabled,
    BatchTooLarge {
        count: usize,
        max: usize,
    },
    CrossAuthorityDisabled,
    CrossAuthorityUnsupported,
    ApprovalUnsupported(String),
    AuthorityMismatch(String),
    AccessDenied {
        authority: String,
        action: String,
        identity: String,
    },
    RevisionConflict {
        expected: u64,
        actual: u64,
    },
    IdempotencyConflict(String),
    IdempotencyReceiptExpired {
        key: String,
        operation_id: String,
    },
    IdempotencyHistoryCompacted(String),
    OperationInProgress(String),
    OperationNotFound(String),
    AuditCursorExpired {
        after_seq: u64,
        oldest_available_seq: u64,
    },
    MemberNotFound(String),
    Actuator(String),
    Persistence(String),
    ApplyFailed {
        message: String,
        receipt: Box<TopologyOperationReceipt>,
    },
}

impl TopologyControlError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::InvalidPolicy(_) => "invalid_policy",
            Self::InvalidRequest(_) => "invalid_request",
            Self::DurableStateRequired => "topology_durable_state_required",
            Self::ReconnectTargetMissing(_) => "topology_reconnect_target_missing",
            Self::DisconnectTargetMissing(_) => "topology_disconnect_target_missing",
            Self::ReconnectRequired(_) => "topology_reconnect_required",
            Self::FeatureDisabled => "topology_control_disabled",
            Self::ReadOnly => "topology_control_read_only",
            Self::BulkDisabled => "topology_bulk_disabled",
            Self::BatchTooLarge { .. } => "topology_batch_too_large",
            Self::CrossAuthorityDisabled => "topology_cross_authority_disabled",
            Self::CrossAuthorityUnsupported => "topology_cross_authority_unsupported",
            Self::ApprovalUnsupported(_) => "topology_approval_unsupported",
            Self::AuthorityMismatch(_) => "topology_authority_mismatch",
            Self::AccessDenied { .. } => "topology_access_denied",
            Self::RevisionConflict { .. } => "topology_revision_conflict",
            Self::IdempotencyConflict(_) => "topology_idempotency_conflict",
            Self::IdempotencyReceiptExpired { .. } => "topology_idempotency_receipt_expired",
            Self::IdempotencyHistoryCompacted(_) => "topology_idempotency_history_compacted",
            Self::OperationInProgress(_) => "topology_operation_in_progress",
            Self::OperationNotFound(_) => "topology_operation_not_found",
            Self::AuditCursorExpired { .. } => "topology_audit_cursor_expired",
            Self::MemberNotFound(_) => "topology_member_not_found",
            Self::Actuator(_) => "topology_actuator_failed",
            Self::Persistence(_) => "topology_persistence_failed",
            Self::ApplyFailed { .. } => "topology_apply_failed",
        }
    }

    pub fn receipt(&self) -> Option<&TopologyOperationReceipt> {
        match self {
            Self::ApplyFailed { receipt, .. } => Some(receipt),
            _ => None,
        }
    }
}

impl Display for TopologyControlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPolicy(message) => write!(f, "invalid topology policy: {message}"),
            Self::InvalidRequest(message) => write!(f, "invalid topology request: {message}"),
            Self::DurableStateRequired => write!(
                f,
                "editable topology control requires a durable topology state path"
            ),
            Self::ReconnectTargetMissing(edge) => write!(
                f,
                "reconnect requires an existing desired, suppressed, or historical edge: {} <-> {}",
                edge.a.identity, edge.b.identity
            ),
            Self::DisconnectTargetMissing(edge) => write!(
                f,
                "disconnect requires an existing desired, actual, or suppressed edge: {} <-> {}",
                edge.a.identity, edge.b.identity
            ),
            Self::ReconnectRequired(edge) => write!(
                f,
                "edge {} <-> {} already has topology history; reconnect permission is required to repair or restore it",
                edge.a.identity, edge.b.identity
            ),
            Self::FeatureDisabled => write!(f, "topology control is disabled"),
            Self::ReadOnly => write!(f, "topology control is read-only"),
            Self::BulkDisabled => write!(f, "bulk topology mutation is disabled"),
            Self::BatchTooLarge { count, max } => {
                write!(f, "topology batch has {count} operations; maximum is {max}")
            }
            Self::CrossAuthorityDisabled => write!(f, "cross-authority topology is disabled"),
            Self::CrossAuthorityUnsupported => write!(
                f,
                "cross-authority topology requires bilateral delegated authorization and is not available"
            ),
            Self::ApprovalUnsupported(tier) => write!(
                f,
                "topology risk tier {tier} requires atomic approval admission, which is not available"
            ),
            Self::AuthorityMismatch(message) => {
                write!(f, "topology authority mismatch: {message}")
            }
            Self::AccessDenied {
                authority,
                action,
                identity,
            } => write!(
                f,
                "topology access denied by authority {authority:?}: {action} on {identity:?}"
            ),
            Self::RevisionConflict { expected, actual } => write!(
                f,
                "topology revision conflict: expected {expected}, actual {actual}"
            ),
            Self::IdempotencyConflict(key) => {
                write!(
                    f,
                    "idempotency key {key:?} was reused with a different request"
                )
            }
            Self::IdempotencyReceiptExpired { key, operation_id } => write!(
                f,
                "idempotency key {key:?} was already committed as {operation_id}, but its receipt has expired"
            ),
            Self::IdempotencyHistoryCompacted(key) => write!(
                f,
                "idempotency key {key:?} may refer to compacted topology history; choose a new key only for a genuinely new operation"
            ),
            Self::OperationInProgress(id) => {
                write!(f, "topology operation is still in progress: {id}")
            }
            Self::OperationNotFound(id) => write!(f, "topology operation not found: {id}"),
            Self::AuditCursorExpired {
                after_seq,
                oldest_available_seq,
            } => write!(
                f,
                "topology audit cursor {after_seq} predates retained history; oldest available sequence is {oldest_available_seq}"
            ),
            Self::MemberNotFound(id) => write!(f, "topology member not found: {id}"),
            Self::Actuator(message) => write!(f, "topology actuator failed: {message}"),
            Self::Persistence(message) => write!(f, "topology persistence failed: {message}"),
            Self::ApplyFailed { message, .. } => write!(f, "topology apply failed: {message}"),
        }
    }
}

impl std::error::Error for TopologyControlError {}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::identity_first::{
        AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeError,
        CheckpointVersion, ContinuityGeneration, ContinuityRecord, ContinuityStore,
        DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime,
        IdentityRuntimeConfig, LeaseAcquireResult, LeaseProvider, LocalContinuityStore,
        LocalLeaseProvider, ResumeSessionOutcome, RosterContext, RosterError, RosterProvider,
        SessionBridge, SessionSnapshot,
    };
    use meerkat_core::types::SessionId;
    use tokio::sync::Mutex as AsyncMutex;

    /// A bridge whose Core reads answer from [`Self::core`], the members its
    /// creates, resumes and retires committed.
    struct RecoveryBridge {
        wires: AsyncMutex<BTreeSet<(AgentRuntimeId, AgentRuntimeId)>>,
        wire_calls: AtomicUsize,
        unwire_calls: AtomicUsize,
        core: crate::identity_first::core_test_members::CoreTestMembers,
    }

    struct RecoveryRoster(Vec<DurableAgentSpec>);

    #[async_trait::async_trait]
    impl RosterProvider for RecoveryRoster {
        async fn roster(
            &self,
            _context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            Ok(self.0.clone())
        }
    }

    impl RecoveryBridge {
        fn new(wires: impl IntoIterator<Item = (AgentRuntimeId, AgentRuntimeId)>) -> Self {
            Self {
                wires: AsyncMutex::new(
                    wires
                        .into_iter()
                        .map(|(a, b)| canonical_runtime_edge(a, b))
                        .collect(),
                ),
                wire_calls: AtomicUsize::new(0),
                unwire_calls: AtomicUsize::new(0),
                core: crate::identity_first::core_test_members::CoreTestMembers::default(),
            }
        }

        async fn wires(&self) -> BTreeSet<(AgentRuntimeId, AgentRuntimeId)> {
            self.wires.lock().await.clone()
        }
    }

    fn canonical_runtime_edge(
        a: AgentRuntimeId,
        b: AgentRuntimeId,
    ) -> (AgentRuntimeId, AgentRuntimeId) {
        if a <= b { (a, b) } else { (b, a) }
    }

    #[async_trait::async_trait]
    impl SessionBridge for RecoveryBridge {
        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let session_id = &meerkat_core::types::SessionId::new();
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(session_id.clone()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn resume_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            self.core.bind(
                identity,
                &crate::identity_first::MemberRuntimeBinding {
                    agent_runtime_id: agent_runtime_id.clone(),
                    session_id: Some(session_id.clone()),
                },
                &spec.profile,
            );
            Ok(ResumeSessionOutcome::Resumed {
                session_id: session_id.clone(),
                agent_runtime_id,
            })
        }

        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: crate::identity_first::BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "delivery not used in recovery test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint not used in recovery test".to_string(),
            ))
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.core.retire(runtime_id);
            Ok(())
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn durable_identity_members(
            &self,
        ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.durable_identity_members()
        }

        async fn wire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            self.wire_calls.fetch_add(1, Ordering::SeqCst);
            self.wires
                .lock()
                .await
                .insert(canonical_runtime_edge(a.clone(), b.clone()));
            Ok(())
        }

        async fn current_member_wires(
            &self,
        ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
            Ok(self.wires.lock().await.iter().cloned().collect())
        }

        async fn current_member_wires_any_half(
            &self,
        ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
            self.current_member_wires().await
        }

        async fn unwire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            self.unwire_calls.fetch_add(1, Ordering::SeqCst);
            self.wires
                .lock()
                .await
                .remove(&canonical_runtime_edge(a.clone(), b.clone()));
            Ok(())
        }
    }

    fn recovery_spec(identity: AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
            identity,
            profile: meerkat_mob::ProfileName::from("recovery"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }
    }

    async fn seed_recovery_continuity(
        store: &LocalContinuityStore,
        leases: &LocalLeaseProvider,
        identities: &[(AgentIdentity, AgentRuntimeId)],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let identity_keys = identities
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        let acquired = leases
            .acquire_leases(&identity_keys, "recovery-seed")
            .await?;
        let mut grants = Vec::with_capacity(identities.len());
        for (identity, runtime_id) in identities {
            let grant = match acquired.get(identity) {
                Some(LeaseAcquireResult::Acquired(grant)) => grant.clone(),
                other => {
                    return Err(format!("expected seed lease for {identity}, got {other:?}").into());
                }
            };
            store
                .upsert_continuity_record(
                    &ContinuityRecord {
                        identity: identity.clone(),
                        agent_runtime_id: runtime_id.clone(),
                        session_id: SessionId::new(),
                        generation: ContinuityGeneration::new(0),
                        checkpoint_version: CheckpointVersion::new(0),
                    },
                    grant.fencing_token,
                )
                .await?;
            grants.push(grant);
        }
        leases.release_leases(&grants).await?;
        Ok(())
    }

    async fn restarted_pending_controller(
        path: &Path,
        authority: &str,
        action: TopologyAction,
        a: &AgentIdentity,
        b: &AgentIdentity,
    ) -> Result<TopologyController, Box<dyn std::error::Error>> {
        let controller =
            TopologyController::load_or_default(TopologyControlPolicy::default(), path)?;
        let desired = DesiredPeerEdge::new(a.to_string(), b.to_string())?;
        let operation_edge = TopologyEdge::new(
            TopologyEndpoint {
                authority: Some(authority.to_string()),
                identity: a.to_string(),
            },
            TopologyEndpoint {
                authority: Some(authority.to_string()),
                identity: b.to_string(),
            },
        )?;
        {
            let mut state = controller.inner.state.write().await;
            state.schema_version = TOPOLOGY_STATE_SCHEMA_VERSION;
            state.authority = Some(authority.to_string());
            state.revision = 1;
            match action {
                TopologyAction::Connect | TopologyAction::Reconnect => {
                    state.additions.insert(desired.clone());
                }
                TopologyAction::Disconnect => {
                    state.suppressions.insert(desired.clone());
                }
            }
            state.pending = Some(PendingTopologyOperation {
                operation_id: format!("pending-{action:?}"),
                audit_record_id: String::new(),
                idempotency_key: format!("recovery-{action:?}"),
                scoped_idempotency_key: None,
                fingerprint: format!("fingerprint-{action:?}"),
                actor: "recovery-test".to_string(),
                base_revision: 0,
                target_revision: 1,
                created_at: chrono::Utc::now().to_rfc3339(),
                reason: Some("synthetic interrupted apply".to_string()),
                operations: vec![TopologyMutation {
                    action,
                    edge: operation_edge,
                }],
                phase: PendingTopologyPhase::Applying,
                rollback_additions: if action == TopologyAction::Disconnect {
                    BTreeSet::from([desired])
                } else {
                    BTreeSet::new()
                },
                rollback_suppressions: BTreeSet::new(),
                rollback_cross_additions: BTreeSet::new(),
                rollback_cross_suppressions: BTreeSet::new(),
                rollback_revision: 0,
                last_recovery_error: None,
                recovery_attempts: 0,
            });
            controller.persist_candidate(&state)?;
        }
        drop(controller);
        Ok(TopologyController::load_or_default(
            TopologyControlPolicy::default(),
            path,
        )?)
    }

    async fn assert_pending_recovery_waits_for_materialization(
        action: TopologyAction,
        initially_connected: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let a = AgentIdentity::parse("domain:recovery-a")?;
        let b = AgentIdentity::parse("domain:recovery-b")?;
        let runtime_a = AgentRuntimeId::parse("rt:domain:recovery-a:0")?;
        let runtime_b = AgentRuntimeId::parse("rt:domain:recovery-b:0")?;
        let controller = restarted_pending_controller(
            &temp.path().join("topology.json"),
            "recovery-mob",
            action,
            &a,
            &b,
        )
        .await?;
        let store = Arc::new(LocalContinuityStore::in_memory()?);
        let leases = Arc::new(LocalLeaseProvider::new());
        seed_recovery_continuity(
            store.as_ref(),
            leases.as_ref(),
            &[
                (a.clone(), runtime_a.clone()),
                (b.clone(), runtime_b.clone()),
            ],
        )
        .await?;
        let bridge = Arc::new(RecoveryBridge::new(if initially_connected {
            vec![(runtime_a.clone(), runtime_b.clone())]
        } else {
            Vec::new()
        }));
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: store,
            lease_provider: leases,
            runtime_instance_id: "recovery-runtime".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        let roster = vec![recovery_spec(a.clone()), recovery_spec(b.clone())];
        crate::identity_first::lazy_register_flow(runtime.as_ref(), &roster, None).await?;
        assert!(runtime.managed_peer_edges_snapshot().await.is_empty());

        runtime.set_topology_controller(controller.clone());
        let context = IdentityFirstRuntimeContext::new_with_lazy_materialization(
            runtime.clone(),
            Arc::new(RecoveryRoster(roster)),
            None,
            None,
            None,
            true,
        );
        let handle_report = {
            let _admission = controller.mutation_guard().await;
            controller.prepare_pending_recovery().await?;
            let report = TopologyRuntimeHandle::reconcile_identity_first_with_controller(
                &controller,
                &context,
            )
            .await;
            controller
                .finalize_recovered_pending(report.is_complete())
                .await?;
            report
        };

        assert!(controller.has_pending().await);
        assert!(!handle_report.is_complete());
        // Core attributes a physical wire to identities only through their live
        // members, so while both endpoints are dormant an interrupted connect's
        // wire is not a managed edge. The pending recovery still holds the
        // edge, and the end state below is the same.
        let expected_managed_while_pending = usize::from(action == TopologyAction::Disconnect);
        assert_eq!(
            runtime.managed_peer_edges_snapshot().await.len(),
            expected_managed_while_pending
        );
        assert_eq!(bridge.wire_calls.load(Ordering::SeqCst), 0);
        assert_eq!(bridge.unwire_calls.load(Ordering::SeqCst), 0);

        runtime.materialize(&a).await?;
        assert!(controller.has_pending().await);
        assert_eq!(
            runtime.managed_peer_edges_snapshot().await.len(),
            expected_managed_while_pending
        );
        assert_eq!(bridge.wire_calls.load(Ordering::SeqCst), 0);
        assert_eq!(bridge.unwire_calls.load(Ordering::SeqCst), 0);

        runtime.materialize(&b).await?;
        assert!(!controller.has_pending().await);
        let expected_connected = action == TopologyAction::Disconnect;
        assert_eq!(!bridge.wires().await.is_empty(), expected_connected);
        assert_eq!(
            bridge.wire_calls.load(Ordering::SeqCst),
            usize::from(!initially_connected && expected_connected)
        );
        assert_eq!(
            bridge.unwire_calls.load(Ordering::SeqCst),
            usize::from(initially_connected && !expected_connected)
        );
        Ok(())
    }

    fn audit_record(label: &str) -> TopologyOperationRecord {
        let now = chrono::Utc::now().to_rfc3339();
        TopologyOperationRecord {
            version: TOPOLOGY_OPERATION_RECORD_VERSION,
            seq: 0,
            record_id: format!("record-{label}"),
            operation_id: format!("operation-{label}"),
            idempotency_key_fingerprint: format!("fingerprint-{label}"),
            actor: format!("actor-{label}"),
            principal: Some(format!("principal-{label}")),
            authorities: vec!["audit-authority".to_string()],
            operations: vec![TopologyMutation {
                action: TopologyAction::Connect,
                edge: TopologyEdge::new(
                    TopologyEndpoint::local(format!("{label}-a")),
                    TopologyEndpoint::local(format!("{label}-b")),
                )
                .expect("audit edge"),
            }],
            status: TopologyOperationRecordStatus::Applied,
            requested_at: now.clone(),
            updated_at: now,
            error_kind: None,
            error_message: None,
        }
    }

    fn human(issuer: &str, subject: &str) -> TopologyApplyScope {
        TopologyApplyScope::Human {
            issuer: issuer.to_string(),
            subject: subject.to_string(),
        }
    }

    fn principal(principal: &str, provider: &str, issuer: &str) -> TopologyApplyScope {
        TopologyApplyScope::Principal {
            principal: principal.to_string(),
            provider: provider.to_string(),
            issuer: issuer.to_string(),
        }
    }

    fn host_principal(principal: &str) -> TopologyApplyScope {
        TopologyApplyScope::HostPrincipal {
            principal: principal.to_string(),
        }
    }

    #[test]
    fn scoped_apply_keys_are_stable_and_separate_every_component() {
        let key = |scope: &TopologyApplyScope| scope.idempotency_key(&["authority"], "k1");
        let alice = human("https://issuer.example", "alice-sub");
        assert_eq!(key(&alice), key(&alice));
        assert!(key(&alice).starts_with(SCOPED_APPLY_KEY_PREFIX));
        // Each scope differs from some other in exactly one component (the
        // kind, a subject, an issuer, a principal name or a provider); no two
        // share a key.
        let scopes = [
            alice.clone(),
            human("https://issuer.example", "bob-sub"),
            human("https://other.example", "alice-sub"),
            principal("alice-sub", "generic_oidc", "https://issuer.example"),
            principal("bob-sub", "generic_oidc", "https://issuer.example"),
            principal("alice-sub", "service_identity", "https://issuer.example"),
            principal("alice-sub", "generic_oidc", "https://other.example"),
            host_principal("alice-sub"),
            host_principal("bob-sub"),
            TopologyApplyScope::Anonymous,
            TopologyApplyScope::LocalHost,
            TopologyApplyScope::Host,
        ];
        for (index, scope) in scopes.iter().enumerate() {
            for other in &scopes[index + 1..] {
                assert_ne!(key(scope), key(other), "{scope:?} and {other:?}");
            }
        }
        assert_eq!(
            TopologyApplyScope::for_host_principal(Some("alice-sub")),
            host_principal("alice-sub")
        );
        assert_eq!(
            TopologyApplyScope::for_host_principal(None),
            TopologyApplyScope::Host
        );
        assert_ne!(key(&alice), alice.idempotency_key(&["authority"], "k2"));
        assert_ne!(
            key(&alice),
            alice.idempotency_key(&["other-authority"], "k1")
        );
        assert_ne!(
            key(&alice),
            alice.idempotency_key(&["authority", "second-authority"], "k1")
        );
    }

    #[test]
    fn scoped_apply_keys_do_not_carry_the_scope_or_the_key_in_clear() {
        let key = human("https://issuer.example", "alice-sub")
            .idempotency_key(&["topology-authority"], "operator-key");
        for clear in [
            "alice-sub",
            "issuer.example",
            "topology-authority",
            "operator-key",
        ] {
            assert!(!key.contains(clear), "{key} carries {clear}");
        }
    }

    /// The scope is structural: a principal or human spelled like a reserved
    /// scope, or like another scope's own encoding, is still that principal,
    /// and a component boundary cannot shift; a host key never takes a scoped
    /// key's shape.
    #[test]
    fn no_principal_spelling_lands_in_another_scope() {
        let key = |scope: &TopologyApplyScope| scope.idempotency_key(&["authority"], "k");
        let reserved = [
            TopologyApplyScope::Host,
            TopologyApplyScope::LocalHost,
            TopologyApplyScope::Anonymous,
        ];
        for spelling in [
            "local-host",
            "local_host",
            "anonymous",
            "host",
            "",
            "k",
            r#"["local_host"]"#,
            r#"["anonymous"]"#,
        ] {
            let spelled = [
                principal(spelling, spelling, spelling),
                principal(spelling, "service_identity", "https://issuer.example"),
                host_principal(spelling),
                human(spelling, spelling),
            ];
            for scope in &spelled {
                for reserved in &reserved {
                    assert_ne!(key(scope), key(reserved), "{scope:?} vs {reserved:?}");
                }
            }
        }
        assert_ne!(
            key(&host_principal("a:b")),
            host_principal("a").idempotency_key(&["b:authority"], "k")
        );
        assert_ne!(
            key(&principal("a", "b:c", "d")),
            key(&principal("a", "b", "c:d"))
        );
        assert_ne!(key(&human("issuer", "a:b")), key(&human("issuer:a", "b")));
        assert_eq!(key(&TopologyApplyScope::Host), "k");
        assert!(
            TopologyApplyScope::Host
                .validate_key(&format!("{SCOPED_APPLY_KEY_PREFIX}anything"))
                .is_err()
        );
        assert!(
            TopologyApplyScope::Host
                .validate_key("ordinary-key")
                .is_ok()
        );
        assert!(
            TopologyApplyScope::Anonymous
                .validate_key(&format!("{SCOPED_APPLY_KEY_PREFIX}anything"))
                .is_ok()
        );
    }

    /// A pending apply under `key`, filed under no scoped key.
    fn unscoped_pending(operation_id: &str, key: &str) -> PendingTopologyOperation {
        PendingTopologyOperation {
            operation_id: operation_id.to_string(),
            audit_record_id: String::new(),
            idempotency_key: key.to_string(),
            scoped_idempotency_key: None,
            fingerprint: format!("fingerprint-{operation_id}"),
            actor: "pending-test".to_string(),
            base_revision: 0,
            target_revision: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
            reason: None,
            operations: Vec::new(),
            phase: PendingTopologyPhase::Applying,
            rollback_additions: BTreeSet::new(),
            rollback_suppressions: BTreeSet::new(),
            rollback_cross_additions: BTreeSet::new(),
            rollback_cross_suppressions: BTreeSet::new(),
            rollback_revision: 0,
            last_recovery_error: None,
            recovery_attempts: 0,
        }
    }

    /// While an apply is pending, its own caller's identical request gets its
    /// pending receipt, and that caller's other request under the key a
    /// conflict; any other caller's request under the same key, the host's
    /// included, is told an operation is in progress. An unscoped pending key
    /// that begins with the reserved prefix answers no caller's apply.
    #[test]
    fn a_pending_apply_answers_only_its_own_callers_key() {
        let key = |scope: &TopologyApplyScope| scope.idempotency_key(&["authority"], "k");
        let alice = human("https://issuer.example", "alice-sub");
        let bob = human("https://issuer.example", "bob-sub");
        let mut pending = unscoped_pending("alice-op", "k");
        pending.scoped_idempotency_key = Some(key(&alice));
        let fingerprint = pending.fingerprint.clone();

        let own = pending
            .answer_apply(&key(&alice), &fingerprint, "k")
            .expect("its own pending apply");
        assert_eq!(own.operation_id, "alice-op");
        assert_eq!(own.status, TopologyOperationStatus::Pending);
        assert!(matches!(
            pending.answer_apply(&key(&alice), "another-request", "k"),
            Err(TopologyControlError::IdempotencyConflict(_))
        ));
        for other in [
            key(&bob),
            key(&host_principal("alice-sub")),
            key(&TopologyApplyScope::Anonymous),
            key(&TopologyApplyScope::Host),
        ] {
            assert!(
                matches!(
                    pending.answer_apply(&other, &fingerprint, "k"),
                    Err(TopologyControlError::OperationInProgress(ref id)) if id == "alice-op"
                ),
                "{other}"
            );
        }

        // A host apply pending from before keys were scoped still answers
        // the host, and no scoped caller.
        let host = unscoped_pending("host-op", "k");
        assert_eq!(host.idempotency_map_key(), Some("k"));
        assert!(host.answer_apply("k", &host.fingerprint, "k").is_ok());
        assert!(matches!(
            host.answer_apply(&key(&alice), &host.fingerprint, "k"),
            Err(TopologyControlError::OperationInProgress(_))
        ));

        // An unscoped key with the reserved prefix answers no caller, even
        // the one whose scoped key it spells.
        let reserved = unscoped_pending("earlier-op", &key(&bob));
        assert_eq!(reserved.idempotency_map_key(), None);
        assert!(matches!(
            reserved.answer_apply(&key(&bob), &reserved.fingerprint, "k"),
            Err(TopologyControlError::OperationInProgress(_))
        ));
    }

    /// An idempotency entry filed before keys were scoped, under a key that
    /// begins with the reserved prefix, names no caller's apply: loading the
    /// state drops it and keeps every other entry.
    #[tokio::test]
    async fn loading_state_drops_reserved_prefix_entries_from_before_keys_were_scoped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("topology.json");
        let mut state = TopologyIntentState {
            schema_version: TOPOLOGY_STATE_SCHEMA_VERSION,
            ..TopologyIntentState::default()
        };
        for (key, record) in [
            (
                "apply-scoped:earlier",
                serde_json::json!({"fingerprint": "f1", "operation_id": "o1"}),
            ),
            (
                "host-key",
                serde_json::json!({"fingerprint": "f2", "operation_id": "o2"}),
            ),
            (
                "apply-scoped:filed",
                serde_json::json!({"fingerprint": "f3", "operation_id": "o3", "scoped": true}),
            ),
        ] {
            state.idempotency.insert(
                key.to_string(),
                serde_json::from_value(record).expect("idempotency record"),
            );
        }
        persist_state(&path, &state).expect("persist");

        let controller =
            TopologyController::load_or_default(TopologyControlPolicy::default(), &path)
                .expect("load");
        let keys = controller
            .inner
            .state
            .read()
            .await
            .idempotency
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["apply-scoped:filed", "host-key"]);
    }

    /// An apply pending since before keys were scoped, under a key that
    /// begins with the reserved prefix, names no caller's apply: its recovery
    /// records its receipt and files no idempotency entry.
    #[tokio::test]
    async fn a_reserved_prefix_pending_from_before_keys_were_scoped_files_no_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let controller = TopologyController::load_or_default(
            TopologyControlPolicy::default(),
            temp.path().join("topology.json"),
        )
        .expect("controller");
        let reserved =
            human("https://issuer.example", "alice-sub").idempotency_key(&["authority"], "k");
        {
            let mut state = controller.inner.state.write().await;
            state.pending = Some(unscoped_pending("earlier-op", &reserved));
            controller.persist_candidate(&state).expect("persist");
        }
        controller
            .finalize_recovered_pending(true)
            .await
            .expect("recovery");

        let state = controller.inner.state.read().await;
        assert!(state.pending.is_none());
        assert_eq!(state.receipts.len(), 1);
        assert!(state.idempotency.is_empty(), "{:?}", state.idempotency);
    }

    /// An evicted receipt that no idempotency entry names compacts and
    /// removes nothing.
    #[tokio::test]
    async fn an_evicted_receipt_that_no_entry_names_compacts_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let controller = TopologyController::load_or_default(
            TopologyControlPolicy {
                receipt_limit: 1,
                ..TopologyControlPolicy::default()
            },
            temp.path().join("topology.json"),
        )
        .expect("controller");
        {
            let mut state = controller.inner.state.write().await;
            // A receipt recorded with no idempotency entry.
            state.receipts.push_back(pending_receipt(&unscoped_pending(
                "unfiled-op",
                "unfiled-key",
            )));
            // Its recovery files this receipt and evicts the first.
            state.pending = Some(unscoped_pending("pending-op", "pending-key"));
            controller.persist_candidate(&state).expect("persist");
        }
        controller
            .finalize_recovered_pending(true)
            .await
            .expect("recovery");

        let state = controller.inner.state.read().await;
        assert_eq!(state.receipts.len(), 1);
        assert_eq!(
            state.idempotency.keys().collect::<Vec<_>>(),
            vec!["pending-key"]
        );
        assert!(
            state.compacted_idempotency.is_empty(),
            "{:?}",
            state.compacted_idempotency
        );
    }

    /// Compacted history from before keys were scoped holds the bare
    /// fingerprints of raw keys: one equal to a scoped key's bare fingerprint
    /// answers no apply of that scoped key, while a host key's still answers.
    #[tokio::test]
    async fn a_bare_compacted_fingerprint_answers_no_scoped_apply() {
        let temp = tempfile::tempdir().expect("tempdir");
        let controller = TopologyController::load_or_default(
            TopologyControlPolicy::default(),
            temp.path().join("topology.json"),
        )
        .expect("controller");
        let scoped =
            human("https://issuer.example", "alice-sub").idempotency_key(&["authority"], "k");
        let mut state = TopologyIntentState::default();
        state
            .compacted_idempotency
            .push_back(idempotency_key_fingerprint(&scoped));
        state
            .compacted_idempotency
            .push_back(idempotency_key_fingerprint("host-key"));
        assert!(!controller.compacted_idempotency_contains(&state, &scoped));
        assert!(controller.compacted_idempotency_contains(&state, "host-key"));
    }

    #[test]
    fn defaults_are_disabled_and_single_operation_only() {
        let policy = TopologyControlPolicy::default();
        assert_eq!(policy.mode, TopologyControlMode::Disabled);
        assert!(!policy.allow_bulk);
        assert!(!policy.allow_cross_authority);
        assert_eq!(policy.max_batch_size, 1);
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn topology_edges_are_canonical_and_reject_self_edges() {
        let edge = TopologyEdge::new(TopologyEndpoint::local("b"), TopologyEndpoint::local("a"))
            .expect("edge");
        assert_eq!(edge.a.identity, "a");
        assert_eq!(edge.b.identity, "b");
        assert!(
            TopologyEdge::new(TopologyEndpoint::local("a"), TopologyEndpoint::local("a")).is_err()
        );
    }

    #[tokio::test]
    async fn durable_suppression_composes_over_declared_edges() {
        let controller =
            TopologyController::new(TopologyControlPolicy::default()).expect("controller");
        let declared = DesiredPeerEdge::new("a", "b").expect("declared");
        {
            let mut state = controller.inner.state.write().await;
            state.suppressions.insert(declared.clone());
        }
        assert!(controller.compose_declared(vec![declared]).await.is_empty());
    }

    #[tokio::test]
    async fn suppression_and_receipt_state_survive_reload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("topology.json");
        let controller =
            TopologyController::load_or_default(TopologyControlPolicy::default(), &path)
                .expect("controller");
        let edge = DesiredPeerEdge::new("a", "b").expect("edge");
        let mut state = controller.inner.state.write().await;
        state.revision = 4;
        state.suppressions.insert(edge.clone());
        controller.persist_candidate(&state).expect("persist");
        drop(state);
        // The controller intentionally owns an exclusive process lock for
        // the durable authority. Model a real restart before reopening it.
        drop(controller);
        let reloaded = TopologyController::load_or_default(TopologyControlPolicy::default(), &path)
            .expect("reload");
        let state = reloaded.inner.state.read().await;
        assert_eq!(state.revision, 4);
        assert!(state.suppressions.contains(&edge));
    }

    #[test]
    fn audit_cursor_is_forward_only_and_zero_means_start_of_retained_history() {
        let mut records = VecDeque::new();
        for seq in 10..=12 {
            let mut record = audit_record(&seq.to_string());
            record.seq = seq;
            records.push_back(record);
        }

        let first = audit_page(&records, 12, None, 2).expect("first page");
        assert_eq!(
            first
                .records
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert_eq!(first.next_after_seq, 11);
        assert!(first.has_more);
        let second = audit_page(&records, 12, Some(first.next_after_seq), 2).expect("second page");
        assert_eq!(
            second
                .records
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            vec![12]
        );
        assert_eq!(second.next_after_seq, 12);
        assert!(!second.has_more);
        let at_latest = audit_page(&records, 12, Some(12), 2).expect("latest cursor");
        assert!(at_latest.records.is_empty());
        assert_eq!(at_latest.next_after_seq, 12);

        // Cursor zero is the documented bootstrap escape hatch: it always
        // starts at the first retained row even after compaction.
        let from_zero = audit_page(&records, 12, Some(0), 20).expect("zero cursor");
        assert_eq!(
            from_zero
                .records
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        let oldest_minus_one = audit_page(&records, 12, Some(9), 20)
            .expect("oldest minus one remains a valid frontier");
        assert_eq!(oldest_minus_one.records[0].seq, 10);
        let expired = audit_page(&records, 12, Some(8), 20)
            .expect_err("older nonzero cursor must fail rather than skip retained rows");
        assert!(matches!(
            expired,
            TopologyControlError::AuditCursorExpired {
                after_seq: 8,
                oldest_available_seq: 10
            }
        ));
    }

    #[tokio::test]
    async fn audit_sequences_are_strict_and_survive_reload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("topology-audit.json");
        let controller =
            TopologyController::load_or_default(TopologyControlPolicy::default(), &path)
                .expect("controller");
        controller
            .upsert_operation_record(audit_record("one"))
            .await
            .expect("first record");
        controller
            .upsert_operation_record(audit_record("two"))
            .await
            .expect("second record");
        let before = controller
            .operation_records(Some(0), 20)
            .await
            .expect("audit before reload");
        assert_eq!(
            before
                .records
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        drop(controller);

        let reloaded = TopologyController::load_or_default(TopologyControlPolicy::default(), &path)
            .expect("reload");
        let after = reloaded
            .operation_records(Some(0), 20)
            .await
            .expect("audit after reload");
        assert_eq!(
            after
                .records
                .iter()
                .map(|record| record.seq)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(after.latest_seq, 2);
    }

    #[tokio::test]
    async fn interrupted_connect_handle_recovery_survives_dormancy_until_unwired() {
        assert_pending_recovery_waits_for_materialization(TopologyAction::Connect, true)
            .await
            .expect("connect recovery");
    }

    #[tokio::test]
    async fn physically_complete_recovery_still_waits_for_dormant_endpoints() {
        assert_pending_recovery_waits_for_materialization(TopologyAction::Connect, false)
            .await
            .expect("physically complete dormant recovery");
    }

    #[tokio::test]
    async fn interrupted_disconnect_recovery_survives_dormancy_until_rewired() {
        assert_pending_recovery_waits_for_materialization(TopologyAction::Disconnect, false)
            .await
            .expect("disconnect recovery");
    }

    #[tokio::test]
    async fn identity_topology_mutation_without_bridge_fails_closed() {
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "no-bridge-topology".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        let edge = crate::identity_first::ManagedPeerEdge::new(
            AgentIdentity::parse("domain:no-bridge-a").expect("identity a"),
            AgentIdentity::parse("domain:no-bridge-b").expect("identity b"),
        )
        .expect("edge");

        let error = runtime
            .mutate_managed_peer_edge_admitted(TopologyAction::Connect, &edge)
            .await
            .expect_err("a missing bridge must not report physical mutation success");
        assert!(error.to_string().contains("requires a session bridge"));
        assert!(runtime.managed_peer_edges_snapshot().await.is_empty());
    }
}

mod assistant_history_refresh;
use assistant_history_refresh::AssistantHistoryRefreshReason;
mod tool_application_projection;

#[cfg(test)]
mod assistant_message_identity_tests;
pub(crate) mod faults;
#[cfg(test)]
mod member_frame_access_tests;
#[cfg(test)]
mod provenance_search_tests;
#[cfg(test)]
mod provenance_store_revision_tests;
mod query_error;
#[cfg(test)]
mod runtime_notice_tests;
#[cfg(test)]
mod runtime_plane_writer_tests;
mod send_recovery;
#[cfg(test)]
mod send_recovery_tests;
mod send_scope;
mod state;
mod store;
mod types;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use meerkat_core::types::HandlingMode;
use meerkat_core::{ContentInput, Message};
use meerkat_mob::MobHandle;
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::runtime::MobMemberListEntry;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, broadcast};

use crate::blob_store::BinaryBlobStore;
use crate::console_contracts::SYSTEM_EVENT_IDENTITY;
#[cfg(test)]
use crate::mob_handle_runtime::send_message_on_mob_with_mode;
use crate::mob_handle_runtime::{
    MobRuntime, assert_member_accepts_images_with_config, is_recoverable_lifecycle_cleanup_error,
};
use crate::runtime::ConsoleMember;
use crate::unified_runtime::{ConsoleEventStore, UnifiedRuntime};

pub use query_error::{ConsoleTimelineQueryError, ConsoleTimelineQueryResult};
pub(crate) use send_recovery::{
    ConsoleSendRecovery, NativeSendObservation, ScopedReservation, SendDispatchClaim,
};
use send_recovery::{SendDispatchClaims, SendSweepOutcome};
pub(crate) use send_scope::ConsoleSendScope;
use state::SendEvidence;
pub use state::{
    ReplaySubscriptionEffect, ReplaySubscriptionState, ReplaySubscriptionTransition, SendEffect,
    SendState, SendTransition, SourceIngestionEffect, SourceIngestionState,
    SourceIngestionTransition,
};
use store::stable_frame_id;
pub use store::{
    ConsoleLogError, ConsoleLogResult, ConsoleLogStore, ConsoleSendDedupeDurability,
    InMemoryConsoleLogStore, SqliteConsoleLogStore,
};
pub use types::{
    AppendDisposition, AppendOutcome, ConsoleCursor, ConsoleFrame, ConsoleFrameLineageLink,
    ConsoleFrameMemberProvenance, ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus,
    ConsoleIdentityInspection, ConsoleIdentityRecord, ConsoleInteractionAccepted,
    ConsoleReplayUnavailable, ConsoleSendRequest, ConsoleTimelineEvent, ConsoleTimelineMode,
    ConsoleTimelinePage, ConsoleTimelineQuery, ConsoleTimelineWindowPage,
    ConsoleTimelineWindowQuery, ConsoleTurnOrigin, ConsoleVisibility, IdentityFirstReservation,
    NewConsoleFrame,
};

const TIMELINE_CHANNEL_CAP: usize = 1024;
const SESSION_HISTORY_PAGE_LIMIT: usize = 500;
const SESSION_HISTORY_REFRESH_TTL_MS: u64 = 30_000;
const SESSION_HISTORY_GROWING_REFRESH_TTL_MS: u64 = 2_000;
const SESSION_HISTORY_DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const IDENTITY_FIRST_LIVE_MEMBER_REFRESH_WAIT: Duration = Duration::from_millis(250);
const TIMELINE_RAW_SCAN_PAGE_LIMIT: usize = 1_000;
const TIMELINE_MAX_RAW_SCAN_FRAMES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityCollectionMode {
    CachedOnly,
    IncludeLiveMembers,
}

#[derive(Clone)]
pub struct MetaConsoleAggregator {
    inner: Arc<AggregatorInner>,
}

#[derive(Debug, Clone, Copy)]
pub struct ConsoleAggregatorOptions {
    pub session_history_backfill_enabled: bool,
    pub max_concurrent_session_backfills: usize,
}

impl Default for ConsoleAggregatorOptions {
    fn default() -> Self {
        Self {
            session_history_backfill_enabled: true,
            max_concurrent_session_backfills: 16,
        }
    }
}

const MEMBER_PROVENANCE_CACHE_LIMIT: usize = 2_048;
const NOTICE_OBSERVATION_CACHE_LIMIT: usize = 16;
const NOTICE_OBSERVATION_MAX_BYTES: usize = 8 * 1024 * 1024;

struct BoundedProjectionCache<K, V> {
    entries: BTreeMap<K, V>,
    order: VecDeque<K>,
    limit: usize,
}

impl<K: Ord + Clone, V> BoundedProjectionCache<K, V> {
    fn new(limit: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            limit,
        }
    }

    fn insert(&mut self, key: K, value: V) {
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, value);
        while self.entries.len() > self.limit {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        self.entries.retain(|key, value| keep(key, value));
        self.order.retain(|key| self.entries.contains_key(key));
    }
}

type MemberProvenanceCache =
    BoundedProjectionCache<(String, String, Option<String>), Arc<ConsoleFrameMemberProvenance>>;
type MemberProvenanceSearchCache =
    BoundedProjectionCache<(String, String, Option<String>), MemberProvenanceSearch>;

#[derive(Clone)]
struct MemberProvenanceSearch {
    registration_id: uuid::Uuid,
    prefix_revision: String,
    observed_through: u64,
}
type NoticeObservationCache =
    BoundedProjectionCache<(String, String, String), RuntimeNoticeObservation>;

struct AggregatorInner {
    /// Policy views share one canonical projector and publish to its broadcaster.
    projection_owner: Option<Arc<AggregatorInner>>,
    store: Arc<dyn ConsoleLogStore>,
    runtimes: RwLock<BTreeMap<String, RuntimeEntry>>,
    has_registered_runtime: AtomicBool,
    event_tx: broadcast::Sender<ConsoleTimelineEvent>,
    active_session_backfills:
        tokio::sync::Mutex<BTreeMap<String, Option<AssistantHistoryRefreshReason>>>,
    // Each targeted worker owns one key and at most one pending forced read.
    // Pending targets retain the latest requested registration incarnation.
    targeted_session_backfills: tokio::sync::Mutex<BTreeMap<String, Option<SessionBackfillTarget>>>,
    opportunistic_session_backfills: tokio::sync::Mutex<BTreeSet<String>>,
    // Coalesce live host observations independently of blocking history reads.
    // Keys and pending native targets contain no tool result or UI data.
    tool_application_projections:
        tokio::sync::Mutex<BTreeMap<(uuid::Uuid, String, String), Option<SessionBackfillTarget>>>,
    session_backfill_permits: Arc<Semaphore>,
    // Serialize each session's current-image read and publication. These locks
    // contain no transcript or lifecycle authority.
    session_history_projection_locks:
        std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Per-session committed boundary authority at which the last completed
    /// session-history backfill ran, keyed by the session's watermark runtime
    /// key. While the runtime's durable store answers the same authority for
    /// the session, the periodic discovery loop skips the backfill outright:
    /// nothing committed moved, so the full session read and the watermark
    /// row write would add nothing. Entries are dropped with their runtime; a
    /// missing entry, or a runtime whose store does not answer the authority
    /// as a bounded read, always reads.
    session_backfill_authorities:
        std::sync::Mutex<BTreeMap<String, meerkat_runtime::RuntimeSessionAuthority>>,
    /// Per-session committed boundary authority at which an UNSETTLED
    /// assistant refresh (the member was not yet idle) last completed its
    /// full history read. An unsettled pass restores positive frames only,
    /// so a later unsettled pass over the same authority has nothing to add
    /// and skips the full-document read; a settled pass always reads.
    session_pending_read_authorities:
        std::sync::Mutex<BTreeMap<String, meerkat_runtime::RuntimeSessionAuthority>>,
    member_provenance: std::sync::Mutex<MemberProvenanceCache>,
    member_provenance_searches: std::sync::Mutex<MemberProvenanceSearchCache>,
    notice_observations: std::sync::Mutex<NoticeObservationCache>,
    // Retry intent is independent of the bounded projection cache. It grants
    // no absence authority; every retry rechecks fresh runtime observations.
    assistant_history_retries: std::sync::Mutex<BTreeSet<(uuid::Uuid, String, String, String)>>,
    /// Retry keys with an armed status-unknown re-drive (one waiter per key):
    /// an idle member whose status read did not answer emits no event of its
    /// own, so its next typed change re-drives the retained retry instead of
    /// only the discovery tick.
    status_unknown_redrives: std::sync::Mutex<BTreeSet<(uuid::Uuid, String, String, String)>>,
    #[cfg(test)]
    status_unknown_redrive_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Retry keys with a queued moved-boundary re-drive: a completed pass whose
    /// session's committed boundary moved during its read re-reads once at
    /// the new boundary instead of waiting for a discovery tick and
    /// watermark expiry.
    moved_boundary_redrives: std::sync::Mutex<BTreeSet<(uuid::Uuid, String, String, String)>>,
    #[cfg(test)]
    moved_boundary_redrive_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Session-history backfill jobs spawned or armed and not yet finished
    /// (see [`BackfillActivity`]). Zero means every pass the aggregator owes
    /// has run: [`MetaConsoleAggregator::history_backfill_converged`].
    backfill_activity: tokio::sync::watch::Sender<usize>,
    identity_read_model: ConsoleIdentityReadModel,
    options: ConsoleAggregatorOptions,
    /// Per-runtime shutdown signals for the live-projection tasks spawned by
    /// [`ConsoleAggregator::register_runtime_handles_with_policy`]. Without
    /// them, every registration leaked an immortal projection task (its
    /// broadcast receiver can never observe `Closed` while the runtime Arc
    /// lives) after the runtime was destroyed (issue #254 follow-up).
    live_projection_shutdowns: std::sync::Mutex<BTreeMap<String, tokio::sync::watch::Sender<bool>>>,
    /// The live dispatch owner of each console send frame in this process,
    /// shared with every policy view (see [`SendDispatchClaims`]).
    send_dispatch_claims: Arc<SendDispatchClaims>,
    /// Test seam: replaces the runtime's admission evidence for sends.
    #[cfg(test)]
    send_admission_override: std::sync::Mutex<Option<Arc<dyn send_recovery::SendAdmissionReader>>>,
}

#[derive(Clone)]
struct ConsoleIdentityReadModel {
    inner: Arc<tokio::sync::RwLock<Vec<ConsoleIdentityRecord>>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    primed: Arc<AtomicBool>,
}

impl Default for ConsoleIdentityReadModel {
    fn default() -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            primed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ConsoleIdentityReadModel {
    async fn snapshot(
        &self,
        inner: Arc<AggregatorInner>,
    ) -> ConsoleLogResult<Vec<ConsoleIdentityRecord>> {
        if !self.primed.load(Ordering::Acquire) {
            Box::pin(self.prime_now(inner)).await?;
        }
        Ok(self.inner.read().await.clone())
    }

    async fn current(&self) -> Vec<ConsoleIdentityRecord> {
        self.inner.read().await.clone()
    }

    async fn refresh_now_if_idle(
        &self,
        inner: Arc<AggregatorInner>,
        mode: IdentityCollectionMode,
    ) -> Option<ConsoleLogResult<Vec<ConsoleIdentityRecord>>> {
        let Ok(guard) = self.refresh_lock.clone().try_lock_owned() else {
            return None;
        };
        let result = Box::pin(collect_identity_records(&inner, mode)).await;
        drop(guard);
        match result {
            Ok(identities) => {
                self.replace(identities.clone()).await;
                Some(Ok(identities))
            }
            Err(err) => Some(Err(err)),
        }
    }

    async fn prime_now(&self, inner: Arc<AggregatorInner>) -> ConsoleLogResult<()> {
        if self.primed.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.refresh_lock.clone().lock_owned().await;
        if self.primed.load(Ordering::Acquire) {
            return Ok(());
        }
        let identities = Box::pin(collect_identity_records(
            &inner,
            IdentityCollectionMode::CachedOnly,
        ))
        .await?;
        *self.inner.write().await = identities;
        self.primed.store(true, Ordering::Release);
        Ok(())
    }

    fn refresh_soon(&self, inner: Arc<AggregatorInner>) {
        let Ok(runtime_handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Ok(guard) = self.refresh_lock.clone().try_lock_owned() else {
            return;
        };
        let read_model = self.clone();
        runtime_handle.spawn(async move {
            let _guard = guard;
            match Box::pin(collect_identity_records(
                &inner,
                IdentityCollectionMode::IncludeLiveMembers,
            ))
            .await
            {
                Ok(identities) => {
                    *read_model.inner.write().await = identities;
                    read_model.primed.store(true, Ordering::Release);
                }
                Err(err) => {
                    tracing::warn!(error = %err, "console identity read-model refresh failed");
                }
            }
        });
    }

    async fn replace(&self, identities: Vec<ConsoleIdentityRecord>) {
        *self.inner.write().await = identities;
        self.primed.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
struct RuntimeEntry {
    registration_id: uuid::Uuid,
    runtime_key: String,
    identity_namespace: String,
    runtime: MobRuntime,
    identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
    /// Set for a copy exported to the host (a router-local aggregator or a
    /// host registration): it keeps the public owner of `identity_runtime`.
    /// The runtime's own projection holds only the non-owning back-edge.
    /// Console MCP App routing reads it as the candidate's identity runtime.
    identity_owner: Option<crate::identity_first::PublicIdentityRuntime>,
    memory_panel: Option<Arc<dyn crate::memory::capabilities::MemoryPanelStore>>,
    console_events: ConsoleEventStore,
    visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
}

/// Native service handles from one current registration. No store data is copied.
pub(crate) struct ConsolePanelRuntime {
    pub(crate) key: String,
    pub(crate) namespace: String,
    pub(crate) runtime: MobRuntime,
    pub(crate) memory: Option<Arc<dyn crate::memory::capabilities::MemoryPanelStore>>,
    registration_id: uuid::Uuid,
}

/// Current native route candidate, never derived from a transcript frame.
#[derive(Clone)]
pub(crate) struct ConsoleApplicationRuntime {
    entry: RuntimeEntry,
    pub(crate) identity: String,
}

impl ConsoleApplicationRuntime {
    pub(crate) fn runtime(&self) -> &MobRuntime {
        &self.entry.runtime
    }
    /// The registration's exported public identity view (D67). A runtime's
    /// own projection is a back-edge and has none, so a route through it
    /// gets no identity runtime rather than an ownerless one.
    pub(crate) fn identity_runtime(&self) -> Option<crate::identity_first::PublicIdentityRuntime> {
        self.entry.identity_owner.clone()
    }
    pub(crate) fn visibility_policy(&self) -> Arc<dyn ConsoleVisibilityPolicy> {
        self.entry.visibility_policy.clone()
    }
    pub(crate) fn namespace(&self) -> &str {
        &self.entry.identity_namespace
    }
}

#[derive(Clone)]
struct ResolvedConsoleMember {
    entry: RuntimeEntry,
    handle: MobHandle,
    member: MobMemberListEntry,
    source_mob_id: String,
    runtime_identity: String,
}

/// What a classic send's replay needs to dispatch it again.
struct ClassicResend<'a> {
    resolved: &'a ResolvedConsoleMember,
    host_policy: &'a Arc<dyn ConsoleVisibilityPolicy>,
    content: &'a ContentInput,
    skill_references: Vec<meerkat_core::skills::SkillKey>,
    handling_mode: HandlingMode,
}

impl ClassicResend<'_> {
    /// Whether dispatching `frame` again goes through a keyed delivery pinned
    /// to the session the send recorded: the member is a durable identity's
    /// local session (the identity runtime route of
    /// [`dispatch_message_to_resolved_member`], which carries the send's
    /// delivery identity and expected session). An externally bound or
    /// mob-routed member is never dispatched again: its delivery may carry no
    /// key, so a second submission could duplicate it.
    async fn keyed_and_pinned(&self, frame: &ConsoleFrame) -> bool {
        if frame.session_id.is_none() {
            return false;
        }
        let Some(identity_runtime) = self.resolved.entry.identity_runtime.as_ref() else {
            return false;
        };
        let Some(identity) = identity_runtime
            .identity_for_member_mutation(&self.resolved.runtime_identity)
            .await
        else {
            return false;
        };
        identity_runtime
            .console_human_delivery_is_keyed(&identity)
            .await
    }
}

/// An existing console send found for a replay.
enum ExistingSend {
    /// Settled, or with a live dispatch owner: replayed, nothing dispatched.
    Replay(ConsoleInteractionAccepted),
    /// Unsettled, and now owned by the caller, who reconciles it.
    Unsettled(
        ConsoleInteractionAccepted,
        SendDispatchClaim,
        Box<ConsoleFrame>,
    ),
}

impl From<ExistingSend> for ScopedReservation {
    fn from(existing: ExistingSend) -> Self {
        match existing {
            ExistingSend::Replay(accepted) => Self::Replay(accepted),
            ExistingSend::Unsettled(accepted, claim, frame) => {
                Self::Unsettled(accepted, claim, frame)
            }
        }
    }
}

fn console_member_for_resolved_member(
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> ConsoleMember {
    ConsoleMember {
        agent_identity: crate::member_comms_id::runtime_alias_str(
            resolved.member.agent_identity.as_str(),
        )
        .into_owned(),
        role: resolved.member.role.to_string(),
        state: crate::mob_handle_runtime::member_status_state_string(resolved.member.status),
        model_capabilities: Default::default(),
        runtime_mode: Some(resolved.member.runtime_mode.to_string()),
        session_id: record.session_id.clone(),
        wired_to: resolved
            .member
            .wired_to
            .iter()
            .map(|peer| crate::member_comms_id::runtime_alias_str(peer.as_str()).into_owned())
            .collect(),
        labels: record.labels.clone(),
        progress: None,
    }
}

/// Capture a live member's provenance, with its spawn lineage, and retain it
/// for the member's incarnation (its session).
async fn capture_member_provenance(
    inner: &AggregatorInner,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> ConsoleFrameMemberProvenance {
    let lineage = Box::pin(capture_member_lineage(resolved, record)).await;
    retain_member_provenance_with_lineage(inner, resolved, record, lineage)
}

/// The most links a captured lineage holds: the member and seven ancestors,
/// the bound the access rules walk.
const CAPTURED_LINEAGE_DEPTH: usize = 8;

/// The member's spawn lineage as the console owner captures it with a
/// frame: the member first, then each ancestor its registry-verified
/// `spawned_by` label names, parent first.
///
/// An ancestor is captured only from the incarnation the spawn registry
/// bound to the lineage (`spawned_by_session`: the session that performed
/// the spawn), read from the same roster the member was resolved from. An
/// ancestor that is not live under that incarnation, or a lineage with no
/// bound incarnation, is recorded without attributes and the walk stops
/// there: the access decision then fails closed under any deny its bare
/// identity cannot decide. A later member under the parent's identity is
/// never captured in its place.
async fn capture_member_lineage(
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> Vec<ConsoleFrameLineageLink> {
    let namespace = resolved.entry.identity_namespace.as_str();
    let mut lineage = vec![lineage_link_for_record(
        record,
        namespace,
        resolved.member.role.as_str(),
    )];
    let mut visited = BTreeSet::from([record.identity.clone()]);
    let mut child_labels = record.labels.clone();
    let mut sources: Option<Vec<ResolvedConsoleMember>> = None;
    while lineage.len() < CAPTURED_LINEAGE_DEPTH {
        let Some(parent) = child_labels
            .get(crate::console_spawn::SPAWNED_BY_LABEL)
            .map(|parent| parent.trim().to_string())
            .filter(|parent| !parent.is_empty())
        else {
            break;
        };
        let parent_identity = apply_namespace(&parent, namespace);
        if !visited.insert(parent_identity.clone()) {
            break;
        }
        let bound_session = child_labels
            .get(crate::console_spawn::SPAWNED_BY_SESSION_LABEL)
            .map(|session| session.trim().to_string())
            .filter(|session| !session.is_empty());
        let mut confirmed = None;
        if let Some(bound_session) = bound_session.as_deref() {
            if sources.is_none() {
                sources = Some(
                    Box::pin(member_sources_for_entry_including_hidden(&resolved.entry)).await,
                );
            }
            for candidate in sources.iter().flatten() {
                let roster_id = candidate.member.agent_identity.as_str();
                let alias_name = crate::member_comms_id::runtime_alias_str(roster_id);
                let alias: &str = alias_name.as_ref();
                let durable =
                    crate::member_comms_id::durable_identity_label(&candidate.member.labels)
                        .unwrap_or(alias);
                if roster_id != parent && alias != parent && durable != parent {
                    continue;
                }
                let Some(candidate_record) =
                    Box::pin(identity_record_for_resolved_member(candidate)).await
                else {
                    continue;
                };
                if candidate_record.session_id.as_deref() == Some(bound_session) {
                    confirmed = Some((candidate_record, candidate.member.role.to_string()));
                    break;
                }
            }
        }
        match confirmed {
            Some((parent_record, role)) => {
                lineage.push(lineage_link_for_record(&parent_record, namespace, &role));
                child_labels = parent_record.labels;
            }
            None => {
                lineage.push(ConsoleFrameLineageLink {
                    identity: parent_identity,
                    agent_id: None,
                    role: None,
                    labels: None,
                });
                break;
            }
        }
    }
    lineage
}

/// One captured lineage link, in the namespace the access attribute cache
/// uses for the same member (identity, runtime member id and the
/// `spawned_by` label are namespaced alike).
fn lineage_link_for_record(
    record: &ConsoleIdentityRecord,
    namespace: &str,
    role: &str,
) -> ConsoleFrameLineageLink {
    let mut labels = record.labels.clone();
    if let Some(parent) = labels.get_mut(crate::console_spawn::SPAWNED_BY_LABEL) {
        *parent = apply_namespace(parent, namespace);
    }
    ConsoleFrameLineageLink {
        identity: record.identity.clone(),
        agent_id: Some(apply_namespace(&record.runtime_member_id, namespace)),
        role: Some(role.to_string()),
        labels: Some(labels),
    }
}

/// Retain a provenance whose lineage holds only the member itself, with any
/// parent it names unconfirmed.
#[cfg(test)]
fn retain_member_provenance(
    inner: &AggregatorInner,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> ConsoleFrameMemberProvenance {
    let namespace = resolved.entry.identity_namespace.as_str();
    let mut lineage = vec![lineage_link_for_record(
        record,
        namespace,
        resolved.member.role.as_str(),
    )];
    if let Some(parent) = record
        .labels
        .get(crate::console_spawn::SPAWNED_BY_LABEL)
        .map(|parent| parent.trim().to_string())
        .filter(|parent| !parent.is_empty())
    {
        lineage.push(ConsoleFrameLineageLink {
            identity: apply_namespace(&parent, namespace),
            agent_id: None,
            role: None,
            labels: None,
        });
    }
    retain_member_provenance_with_lineage(inner, resolved, record, lineage)
}

fn retain_member_provenance_with_lineage(
    inner: &AggregatorInner,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
    lineage: Vec<ConsoleFrameLineageLink>,
) -> ConsoleFrameMemberProvenance {
    let provenance = ConsoleFrameMemberProvenance {
        identity: record.clone(),
        member: console_member_for_resolved_member(resolved, record),
        primary_mob_id: resolved.entry.runtime.handle().mob_id().to_string(),
        source_mob_id: resolved.source_mob_id.clone(),
        lineage,
    };
    // A stale identity refresh must not restore a predecessor's witnesses.
    let entries = inner
        .runtimes
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if entries
        .get(&resolved.entry.runtime_key)
        .is_none_or(|entry| entry.registration_id != resolved.entry.registration_id)
    {
        return provenance;
    }
    // Retained per incarnation only: a witness keyed by the identity alone
    // could later be handed to another member under the same identity.
    let Some(session_id) = record.session_id.clone() else {
        return provenance;
    };
    inner
        .member_provenance
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            (
                record.runtime_key.clone(),
                record.identity.clone(),
                Some(session_id),
            ),
            Arc::new(provenance.clone()),
        );
    provenance
}

async fn member_provenance_for_identity(
    inner: &AggregatorInner,
    entry: &RuntimeEntry,
    identity: &str,
    session_id: Option<&str>,
    refresh: bool,
) -> Option<ConsoleFrameMemberProvenance> {
    if crate::console_contracts::is_runtime_plane_identity(identity) {
        return None;
    }
    // A witness belongs to the incarnation that wrote the frame, and only the
    // frame's session names it. A frame that names no session records none,
    // and is decided as uncaptured: a member live under the identity now, a
    // retained witness and the durable search could all be another member
    // under the same identity than the one that wrote it.
    let session_id = session_id?;
    let key = (
        entry.runtime_key.clone(),
        identity.to_string(),
        Some(session_id.to_string()),
    );
    let cached = inner
        .member_provenance
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .get(&key)
        .map(|value| value.as_ref().clone());
    if !refresh && cached.is_some() {
        return cached;
    }
    for resolved in Box::pin(member_sources_for_entry_including_hidden(entry)).await {
        let Some(record) = identity_record_for_resolved_member(&resolved).await else {
            continue;
        };
        if record.identity == identity && record.session_id.as_deref() == Some(session_id) {
            return Some(Box::pin(capture_member_provenance(inner, &resolved, &record)).await);
        }
    }
    if cached.is_some() {
        return cached;
    }
    // A missing live member still needs its durable policy witness. Cache only
    // a verified log prefix, never the absence of the member itself.
    recover_member_provenance(inner, entry, identity, Some(session_id), key).await
}

async fn recover_member_provenance(
    inner: &AggregatorInner,
    entry: &RuntimeEntry,
    identity: &str,
    session_id: Option<&str>,
    key: (String, String, Option<String>),
) -> Option<ConsoleFrameMemberProvenance> {
    // Retry an unsuccessful search once if its prefix changed. A recovered
    // policy witness remains usable across unrelated concurrent appends.
    for _ in 0..2 {
        let revision = inner.store.history_prefix_revision().await.ok()?;
        let observed_through = match inner.store.latest_cursor().await.ok()? {
            Some(cursor) => cursor.seq()?,
            None => 0,
        };
        let cached = inner
            .member_provenance_searches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&key)
            .cloned()
            .filter(|search| {
                search.registration_id == entry.registration_id
                    && Some(&search.prefix_revision) == revision.as_ref()
                    && search.observed_through <= observed_through
            });
        let mut after = cached
            .as_ref()
            .filter(|search| search.observed_through > 0)
            .map(|search| ConsoleCursor::from_seq(search.observed_through));
        let mut recovered = None;
        if cached
            .as_ref()
            .is_none_or(|search| search.observed_through != observed_through)
        {
            loop {
                let page = inner
                    .store
                    .query_frames(ConsoleTimelineQuery {
                        identity: Some(identity.to_string()),
                        after: after.clone(),
                        limit: 1_000,
                        ..Default::default()
                    })
                    .await
                    .ok()?;
                let Some(last) = page.frames.last() else {
                    break;
                };
                let last_sequence = last.cursor.seq()?;
                if after
                    .as_ref()
                    .and_then(ConsoleCursor::seq)
                    .is_some_and(|previous| last_sequence <= previous)
                {
                    return None;
                }
                after = Some(last.cursor.clone());
                for frame in page.frames {
                    let sequence = frame.cursor.seq()?;
                    if sequence > observed_through {
                        break;
                    }
                    if frame.runtime_key == entry.runtime_key
                        && frame.identity == identity
                        && session_id.is_none_or(|id| frame.session_id.as_deref() == Some(id))
                        && let Some(provenance) = frame.source.member_provenance
                        && provenance.identity.runtime_key == entry.runtime_key
                        && provenance.identity.identity == identity
                        && session_id
                            .is_none_or(|id| provenance.identity.session_id.as_deref() == Some(id))
                    {
                        recovered = Some(provenance);
                    }
                }
                if last_sequence >= observed_through {
                    break;
                }
            }
        }
        // Only absence needs a continuous searched prefix. SQLite external
        // appends also change this revision, but cannot erase a witness we
        // already read. Dropping it would admit the next row without the
        // retained member visibility policy.
        if recovered.is_none() && inner.store.history_prefix_revision().await.ok()? != revision {
            continue;
        }
        let entries = inner
            .runtimes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&entry.runtime_key)
            .is_some_and(|current| current.registration_id == entry.registration_id)
        {
            if let Some(provenance) = &recovered {
                inner
                    .member_provenance
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(key.clone(), Arc::new(provenance.clone()));
            } else if let Some(prefix_revision) = revision {
                inner
                    .member_provenance_searches
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        key.clone(),
                        MemberProvenanceSearch {
                            registration_id: entry.registration_id,
                            prefix_revision,
                            observed_through,
                        },
                    );
            }
        }
        return recovered;
    }
    None
}

fn retained_member_visible(
    policy: &dyn ConsoleVisibilityPolicy,
    source: &ConsoleFrameMemberProvenance,
) -> bool {
    (policy.include_implicit_delegate_members() || source.primary_mob_id == source.source_mob_id)
        && policy.identity_visible(&source.identity)
        && policy.member_visible(&source.member)
}

async fn resolved_member_visible(
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> bool {
    if !raw_resolved_member_visible(resolved, record) {
        return false;
    }
    !Box::pin(live_record_shadowed_by_hidden_durable_binding(
        resolved, record,
    ))
    .await
}

fn raw_resolved_member_visible(
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> bool {
    (resolved
        .entry
        .visibility_policy
        .include_implicit_delegate_members()
        || *resolved.entry.runtime.handle().mob_id() == resolved.source_mob_id)
        && resolved.entry.visibility_policy.identity_visible(record)
        && resolved
            .entry
            .visibility_policy
            .member_visible(&console_member_for_resolved_member(resolved, record))
}

pub trait ConsoleVisibilityPolicy: Send + Sync {
    fn include_implicit_delegate_members(&self) -> bool {
        true
    }

    fn member_visible(&self, _member: &ConsoleMember) -> bool {
        true
    }

    fn identity_visible(&self, _record: &ConsoleIdentityRecord) -> bool {
        true
    }

    fn frame_visible(&self, _frame: &ConsoleFrame) -> bool {
        true
    }

    fn redact_payload(&self, _frame: &NewConsoleFrame) -> Option<Value> {
        None
    }
}

#[derive(Debug, Default)]
pub struct AllowAllConsoleVisibilityPolicy;

impl ConsoleVisibilityPolicy for AllowAllConsoleVisibilityPolicy {}

#[derive(Debug, Default)]
pub struct HideImplicitDelegateMembersConsoleVisibilityPolicy;

impl ConsoleVisibilityPolicy for HideImplicitDelegateMembersConsoleVisibilityPolicy {
    fn include_implicit_delegate_members(&self) -> bool {
        false
    }

    fn member_visible(&self, member: &ConsoleMember) -> bool {
        !is_implicit_delegate_member(member.role.as_str(), &member.labels)
    }

    fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
        !is_implicit_delegate_member(
            record
                .labels
                .get("role")
                .map(String::as_str)
                .unwrap_or_default(),
            &record.labels,
        )
    }
}

#[derive(Clone)]
pub struct ConsoleRuntimeRegistration {
    pub runtime_key: String,
    pub runtime: Arc<UnifiedRuntime>,
    pub identity_namespace: String,
    pub visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
}

impl MetaConsoleAggregator {
    /// Resolve once no session-history backfill is running, queued, or armed
    /// to run again: every pass owed after registration, a durable write, or
    /// a member event has finished, including re-drives waiting for a member
    /// to settle or its queued work to land. A converged console re-reads
    /// nothing until something durable changes. Never resolves while a
    /// backfill stays owed; callers bound the wait with their own deadline.
    pub async fn history_backfill_converged(&self) {
        let mut activity = backfill_owner(&self.inner).backfill_activity.subscribe();
        let _ = activity.wait_for(|in_flight| *in_flight == 0).await;
    }

    pub fn new(store: Arc<dyn ConsoleLogStore>) -> Self {
        Self::new_with_options(store, ConsoleAggregatorOptions::default())
    }

    pub fn new_with_options(
        store: Arc<dyn ConsoleLogStore>,
        mut options: ConsoleAggregatorOptions,
    ) -> Self {
        options.max_concurrent_session_backfills = options.max_concurrent_session_backfills.max(1);
        let (event_tx, _) = broadcast::channel(TIMELINE_CHANNEL_CAP);
        Self {
            inner: Arc::new(AggregatorInner {
                projection_owner: None,
                store,
                runtimes: RwLock::new(BTreeMap::new()),
                has_registered_runtime: AtomicBool::new(false),
                event_tx,
                active_session_backfills: tokio::sync::Mutex::new(BTreeMap::new()),
                targeted_session_backfills: tokio::sync::Mutex::new(BTreeMap::new()),
                opportunistic_session_backfills: tokio::sync::Mutex::new(BTreeSet::new()),
                session_backfill_permits: Arc::new(Semaphore::new(
                    options.max_concurrent_session_backfills,
                )),
                session_backfill_authorities: std::sync::Mutex::new(BTreeMap::new()),
                session_pending_read_authorities: std::sync::Mutex::new(BTreeMap::new()),
                session_history_projection_locks: std::sync::Mutex::new(BTreeMap::new()),
                tool_application_projections: tokio::sync::Mutex::new(BTreeMap::new()),
                member_provenance: std::sync::Mutex::new(MemberProvenanceCache::new(
                    MEMBER_PROVENANCE_CACHE_LIMIT,
                )),
                member_provenance_searches: std::sync::Mutex::new(
                    MemberProvenanceSearchCache::new(MEMBER_PROVENANCE_CACHE_LIMIT),
                ),
                assistant_history_retries: std::sync::Mutex::new(BTreeSet::new()),
                status_unknown_redrives: std::sync::Mutex::new(BTreeSet::new()),
                #[cfg(test)]
                status_unknown_redrive_tasks: std::sync::Mutex::new(Vec::new()),
                moved_boundary_redrives: std::sync::Mutex::new(BTreeSet::new()),
                #[cfg(test)]
                moved_boundary_redrive_tasks: std::sync::Mutex::new(Vec::new()),
                backfill_activity: tokio::sync::watch::Sender::new(0),
                notice_observations: std::sync::Mutex::new(NoticeObservationCache::new(
                    NOTICE_OBSERVATION_CACHE_LIMIT,
                )),
                identity_read_model: ConsoleIdentityReadModel::default(),
                options,
                live_projection_shutdowns: std::sync::Mutex::new(BTreeMap::new()),
                send_dispatch_claims: Arc::new(SendDispatchClaims::default()),
                #[cfg(test)]
                send_admission_override: std::sync::Mutex::new(None),
            }),
        }
    }

    pub fn in_memory() -> Self {
        Self::new(Arc::new(InMemoryConsoleLogStore::new()))
    }

    pub fn in_memory_with_options(options: ConsoleAggregatorOptions) -> Self {
        Self::new_with_options(Arc::new(InMemoryConsoleLogStore::new()), options)
    }

    pub(crate) fn update_identity_authority(
        &self,
        runtime_key: &str,
        authority: Option<Arc<crate::identity_first::IdentityRuntime>>,
    ) {
        if let Ok(mut runtimes) = self.inner.runtimes.write()
            && let Some(entry) = runtimes.get_mut(runtime_key)
        {
            entry.identity_runtime = authority;
        }
        self.inner
            .identity_read_model
            .refresh_soon(self.inner.clone());
    }

    /// Build a read/send policy view without starting another source projector.
    /// The canonical owner retains host-redacted frames; each view applies its
    /// additional payload policy only when returning frames to a subscriber.
    pub(crate) fn policy_view(&self, policy: Arc<dyn ConsoleVisibilityPolicy>) -> Self {
        let owner = self
            .inner
            .projection_owner
            .as_ref()
            .unwrap_or(&self.inner)
            .clone();
        let runtimes = owner
            .runtimes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(key, entry)| {
                let mut entry = entry.clone();
                entry.visibility_policy = policy.clone();
                (key.clone(), entry)
            })
            .collect();
        let view = Self {
            inner: Arc::new(AggregatorInner {
                projection_owner: Some(owner.clone()),
                store: owner.store.clone(),
                runtimes: RwLock::new(runtimes),
                has_registered_runtime: AtomicBool::new(
                    owner.has_registered_runtime.load(Ordering::Acquire),
                ),
                event_tx: owner.event_tx.clone(),
                active_session_backfills: tokio::sync::Mutex::new(BTreeMap::new()),
                targeted_session_backfills: tokio::sync::Mutex::new(BTreeMap::new()),
                opportunistic_session_backfills: tokio::sync::Mutex::new(BTreeSet::new()),
                session_backfill_permits: owner.session_backfill_permits.clone(),
                session_backfill_authorities: std::sync::Mutex::new(BTreeMap::new()),
                session_pending_read_authorities: std::sync::Mutex::new(BTreeMap::new()),
                session_history_projection_locks: std::sync::Mutex::new(BTreeMap::new()),
                tool_application_projections: tokio::sync::Mutex::new(BTreeMap::new()),
                member_provenance: std::sync::Mutex::new(MemberProvenanceCache::new(
                    MEMBER_PROVENANCE_CACHE_LIMIT,
                )),
                member_provenance_searches: std::sync::Mutex::new(
                    MemberProvenanceSearchCache::new(MEMBER_PROVENANCE_CACHE_LIMIT),
                ),
                assistant_history_retries: std::sync::Mutex::new(BTreeSet::new()),
                status_unknown_redrives: std::sync::Mutex::new(BTreeSet::new()),
                #[cfg(test)]
                status_unknown_redrive_tasks: std::sync::Mutex::new(Vec::new()),
                moved_boundary_redrives: std::sync::Mutex::new(BTreeSet::new()),
                #[cfg(test)]
                moved_boundary_redrive_tasks: std::sync::Mutex::new(Vec::new()),
                backfill_activity: tokio::sync::watch::Sender::new(0),
                notice_observations: std::sync::Mutex::new(NoticeObservationCache::new(
                    NOTICE_OBSERVATION_CACHE_LIMIT,
                )),
                identity_read_model: ConsoleIdentityReadModel::default(),
                options: owner.options,
                live_projection_shutdowns: std::sync::Mutex::new(BTreeMap::new()),
                send_dispatch_claims: owner.send_dispatch_claims.clone(),
                #[cfg(test)]
                send_admission_override: std::sync::Mutex::new(None),
            }),
        };
        view.inner
            .identity_read_model
            .refresh_soon(view.inner.clone());
        view
    }

    pub(crate) fn project_frame_for_view(&self, mut frame: ConsoleFrame) -> ConsoleFrame {
        let policy = self.inner.projection_owner.as_ref().and_then(|_| {
            self.inner.runtimes.read().ok().and_then(|runtimes| {
                runtimes
                    .get(&frame.runtime_key)
                    .map(|entry| entry.visibility_policy.clone())
            })
        });
        if let Some(policy) = policy {
            let input = NewConsoleFrame {
                id: Some(frame.id.clone()),
                dedupe_key: frame.dedupe_key.clone(),
                timestamp_ms: frame.timestamp_ms,
                runtime_key: frame.runtime_key.clone(),
                identity: frame.identity.clone(),
                conversation_id: frame.conversation_id.clone(),
                session_id: frame.session_id.clone(),
                kind: frame.kind.clone(),
                status: frame.status,
                payload: frame.payload.clone(),
                source: frame.source.clone(),
                source_event_id: frame.source_event_id.clone(),
                interaction_id: frame.interaction_id.clone(),
                turn_id: frame.turn_id.clone(),
                run_id: frame.run_id.clone(),
                parent_frame_id: frame.parent_frame_id.clone(),
                caused_by_frame_id: frame.caused_by_frame_id.clone(),
            };
            if let Some(payload) = policy.redact_payload(&input) {
                frame.payload = payload;
                frame.status = ConsoleFrameStatus::Redacted;
            }
        }
        // Storage and broadcasts keep policy witnesses. Only cloned output is
        // stripped, after visibility checks and policy redaction have used them.
        // Status markers contain a complete stored frame and need the same rule.
        if frame.kind == "frame_updated"
            && let Some(value) = frame.payload.get("frame")
            && let Ok(updated) = serde_json::from_value::<ConsoleFrame>(value.clone())
            && let Ok(projected) = serde_json::to_value(self.project_frame_for_view(updated))
        {
            frame.payload["frame"] = projected;
        }
        frame.source.member_provenance = None;
        frame
    }

    pub(crate) fn project_event_for_view(
        &self,
        event: ConsoleTimelineEvent,
    ) -> ConsoleTimelineEvent {
        match event {
            ConsoleTimelineEvent::ConsoleFrame { frame } => ConsoleTimelineEvent::ConsoleFrame {
                frame: self.project_frame_for_view(frame),
            },
            ConsoleTimelineEvent::FrameUpdated { frame } => ConsoleTimelineEvent::FrameUpdated {
                frame: self.project_frame_for_view(frame),
            },
            other => other,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConsoleTimelineEvent> {
        self.inner.event_tx.subscribe()
    }

    pub fn store(&self) -> Arc<dyn ConsoleLogStore> {
        self.inner.store.clone()
    }

    /// Durability of the send dedupe records in the store this aggregator
    /// actually writes, which decides whether a client may resend an
    /// uncertain send with its original idempotency key.
    #[must_use]
    pub fn send_dedupe_durability(&self) -> ConsoleSendDedupeDurability {
        self.inner.store.send_dedupe_durability()
    }

    pub fn register_runtime(&self, registration: ConsoleRuntimeRegistration) {
        let identity_owner = registration.runtime.identity_runtime();
        self.register_entry(RuntimeEntry {
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: registration.runtime_key,
            identity_namespace: registration.identity_namespace,
            runtime: registration.runtime.mob_runtime().clone(),
            identity_runtime: identity_owner
                .as_ref()
                .map(|owner| Arc::clone(owner.runtime())),
            identity_owner,
            memory_panel: registration.runtime.memory_panel_store(),
            console_events: registration.runtime.console_events(),
            visibility_policy: registration.visibility_policy,
        });
    }

    /// Export the native runtime and its identity owner atomically.
    pub(crate) fn register_owned_runtime_handles_with_policy(
        &self,
        runtime_key: impl Into<String>,
        identity_namespace: impl Into<String>,
        runtime: MobRuntime,
        identity_runtime: Option<crate::identity_first::PublicIdentityRuntime>,
        console_events: ConsoleEventStore,
        visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) {
        self.register_entry(RuntimeEntry {
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: runtime_key.into(),
            identity_namespace: identity_namespace.into(),
            runtime,
            identity_runtime: identity_runtime
                .as_ref()
                .map(|owner| Arc::clone(owner.runtime())),
            identity_owner: identity_runtime,
            memory_panel: None,
            console_events,
            visibility_policy,
        });
    }

    pub(crate) fn panel_runtimes(&self) -> ConsoleLogResult<Vec<ConsolePanelRuntime>> {
        let entries = self
            .inner
            .runtimes
            .read()
            .map_err(|_| std::io::Error::other("Console runtime registry is unavailable"))?;
        Ok(entries
            .values()
            .map(|entry| ConsolePanelRuntime {
                key: entry.runtime_key.clone(),
                namespace: entry.identity_namespace.clone(),
                runtime: entry.runtime.clone(),
                memory: entry.memory_panel.clone(),
                registration_id: entry.registration_id,
            })
            .collect())
    }

    pub(crate) fn panel_runtime_is_current(&self, runtime: &ConsolePanelRuntime) -> bool {
        self.inner.runtimes.read().is_ok_and(|entries| {
            entries
                .get(&runtime.key)
                .is_some_and(|entry| entry.registration_id == runtime.registration_id)
        })
    }

    /// Namespace resolution only. The caller must validate native identity and
    /// session ownership for each candidate and reject ambiguous matches.
    pub(crate) fn application_runtime_candidates(
        &self,
        identity: &str,
    ) -> Vec<ConsoleApplicationRuntime> {
        self.inner
            .runtimes
            .read()
            .map(|entries| {
                entries
                    .values()
                    .flat_map(|entry| {
                        namespace_match_candidates(identity, &entry.identity_namespace)
                            .into_iter()
                            .map(|identity| ConsoleApplicationRuntime {
                                entry: entry.clone(),
                                identity,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn application_runtime_is_current(
        &self,
        runtime: &ConsoleApplicationRuntime,
    ) -> bool {
        runtime_entry_is_current(&self.inner, &runtime.entry)
    }

    pub(crate) fn register_runtime_handles_with_policy(
        &self,
        runtime_key: impl Into<String>,
        identity_namespace: impl Into<String>,
        runtime: MobRuntime,
        identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
        console_events: ConsoleEventStore,
        visibility_policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) {
        let runtime_key = runtime_key.into();
        let identity_namespace = identity_namespace.into();
        let entry = RuntimeEntry {
            registration_id: uuid::Uuid::new_v4(),
            runtime_key,
            identity_namespace,
            runtime,
            identity_runtime,
            identity_owner: None,
            memory_panel: None,
            console_events,
            visibility_policy,
        };
        self.register_entry(entry);
    }

    fn register_entry(&self, entry: RuntimeEntry) {
        let runtime_key = entry.runtime_key.clone();
        let console_events = entry.console_events.clone();
        if let Ok(mut runtimes) = self.inner.runtimes.write() {
            self.inner
                .has_registered_runtime
                .store(true, Ordering::Release);
            runtimes.insert(runtime_key.clone(), entry);
        }
        self.inner
            .member_provenance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(key, _, _), _| key != &runtime_key);
        self.inner
            .assistant_history_retries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, key, _, _)| key != &runtime_key);
        self.inner
            .status_unknown_redrives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, key, _, _)| key != &runtime_key);
        self.inner
            .member_provenance_searches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(key, _, _), _| key != &runtime_key);
        self.inner
            .notice_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(key, _, _), _| key != &runtime_key);
        // Registration replaces any prior runtime under this key, possibly
        // over a different durable store: an authority recorded against the
        // old runtime could equal the new runtime's and wrongly suppress its
        // first backfill, so drop them here as well as on unregister.
        {
            let prefix = format!("{runtime_key}:session-history:");
            self.inner
                .session_backfill_authorities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|key, _| !key.starts_with(&prefix));
            self.inner
                .session_pending_read_authorities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|key, _| !key.starts_with(&prefix));
        }
        // Re-registering a key replaces its live-projection task: signal the
        // old one before spawning the new (otherwise both would project).
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        if let Ok(mut shutdowns) = self.inner.live_projection_shutdowns.lock()
            && let Some(previous) = shutdowns.insert(runtime_key.clone(), shutdown_tx)
        {
            let _ = previous.send(true);
        }
        self.inner
            .identity_read_model
            .refresh_soon(self.inner.clone());
        let inner = self.inner.clone();
        let events_for_live = console_events.clone();
        let events_for_live_recovery = console_events.clone();
        let runtime_key_for_live = runtime_key.clone();
        // Subscribe before spawning the projection task. If subscription is
        // deferred into the task, an event can land after replay completes
        // but before the task is first polled, leaving neither path able to
        // observe it.
        let mut live_rx = events_for_live.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        // Sender dropped or unregister signalled: stop.
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    received = live_rx.recv() => match received {
                        Ok(envelope) => {
                            let _ = project_console_event(
                                inner.clone(),
                                &runtime_key_for_live,
                                envelope,
                            )
                            .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = recover_lagged_source_events(
                                inner.clone(),
                                &runtime_key_for_live,
                                &events_for_live_recovery,
                            )
                            .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        });

        let inner = self.inner.clone();
        let events_for_replay = console_events;
        let runtime_key_for_replay = runtime_key;
        // Registration owes a recovery pass from this moment: count it before
        // the replay task first runs, until that pass has been spawned.
        let registration_activity = BackfillActivity::begin(&inner);
        tokio::spawn(async move {
            let _registration_activity = registration_activity;
            let mut ingestion_state = SourceIngestionState::Registered;
            if let Ok((next, _effects)) =
                ingestion_state.apply(SourceIngestionTransition::StartBackfill)
            {
                ingestion_state = next;
            }
            if let Ok(events) = events_for_replay.replay_all(None).await {
                for envelope in events {
                    let _ = project_console_event(inner.clone(), &runtime_key_for_replay, envelope)
                        .await;
                }
            }
            spawn_session_history_backfill(
                inner.clone(),
                runtime_key_for_replay.clone(),
                AssistantHistoryRefreshReason::Recovery,
            );
            spawn_session_history_discovery_loop(inner.clone(), runtime_key_for_replay.clone());
            if let Ok((next, _effects)) =
                ingestion_state.apply(SourceIngestionTransition::BackfillComplete)
            {
                ingestion_state = next;
            }
            let _ = ingestion_state.apply(SourceIngestionTransition::StartLive);
        });
    }

    /// Project `events` of `runtime_key`'s console event store into the log
    /// and return once each is durable. Runtime shutdown calls it with the
    /// events its final drains appended, right before
    /// [`Self::unregister_runtime`] detaches the live projection, which may
    /// stop before it receives them. Projection dedupes on the source event
    /// id, so an event the live projection already wrote is not written
    /// twice. Every event is attempted; the first failure is returned.
    pub(crate) async fn project_runtime_events(
        &self,
        runtime_key: &str,
        events: Vec<crate::console_contracts::ConsoleIdentityEventEnvelope>,
    ) -> ConsoleLogResult<()> {
        let mut first_error = None;
        for envelope in events {
            if let Err(error) =
                project_console_event(self.inner.clone(), runtime_key, envelope).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Unregister a runtime (issue #254 follow-up): removes the registry
    /// entry (which also terminates the 5s session-history discovery loop on
    /// its next tick — it checks registration), signals the live-projection
    /// task to stop (its broadcast receiver would otherwise never observe
    /// `Closed` while the runtime's event store lives), and refreshes the
    /// identity read model so the dead runtime's identities leave the
    /// console. Frames already projected into the store remain queryable —
    /// unregister detaches the LIVE source, it does not erase history.
    /// Idempotent; unknown keys are a no-op.
    pub fn unregister_runtime(&self, runtime_key: &str) {
        let removed = self
            .inner
            .runtimes
            .write()
            .ok()
            .and_then(|mut runtimes| runtimes.remove(runtime_key))
            .is_some();
        self.inner
            .member_provenance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(key, _, _), _| key != runtime_key);
        self.inner
            .assistant_history_retries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, key, _, _)| key != runtime_key);
        self.inner
            .status_unknown_redrives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(_, key, _, _)| key != runtime_key);
        self.inner
            .member_provenance_searches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(key, _, _), _| key != runtime_key);
        self.inner
            .notice_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(key, _, _), _| key != runtime_key);
        if let Ok(mut shutdowns) = self.inner.live_projection_shutdowns.lock()
            && let Some(shutdown) = shutdowns.remove(runtime_key)
        {
            let _ = shutdown.send(true);
        }
        // Drop this runtime's backfill authorities: a later re-register gets
        // a fresh runtime, possibly over a different durable store, and a
        // stale equal authority would wrongly suppress its first backfill.
        {
            let prefix = format!("{runtime_key}:session-history:");
            self.inner
                .session_backfill_authorities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|key, _| !key.starts_with(&prefix));
            self.inner
                .session_pending_read_authorities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|key, _| !key.starts_with(&prefix));
        }
        if removed {
            self.inner
                .identity_read_model
                .refresh_soon(self.inner.clone());
        }
    }

    /// Whether the registered runtimes run, as their mob owners answer now:
    /// `Some(true)` while at least one runtime is registered and every
    /// registered mob is creating or running with its actor not stopped,
    /// `Some(false)` when at least one registered mob is not running (a
    /// fail-stopped actor included), and `None` when no runtime is
    /// registered, so nothing answers.
    pub(crate) fn registered_runtimes_running(&self) -> Option<bool> {
        let runtimes = self
            .inner
            .runtimes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtimes.is_empty() {
            return None;
        }
        Some(runtimes.values().all(|entry| {
            crate::mob_handle_runtime::mob_observed_running(
                &entry.runtime.handle().lifecycle_observation(),
            )
        }))
    }

    pub async fn list_identities(&self) -> ConsoleLogResult<Vec<ConsoleIdentityRecord>> {
        if let Some(identities) = Box::pin(self.inner.identity_read_model.refresh_now_if_idle(
            self.inner.clone(),
            IdentityCollectionMode::IncludeLiveMembers,
        ))
        .await
        {
            let identities = identities?;
            spawn_identity_backfills_for_records(self.inner.clone(), &identities);
            return Ok(identities);
        }
        self.inner
            .identity_read_model
            .refresh_soon(self.inner.clone());
        let identities =
            Box::pin(self.inner.identity_read_model.snapshot(self.inner.clone())).await?;
        spawn_identity_backfills_for_records(self.inner.clone(), &identities);
        Ok(identities)
    }

    /// Forward recorded activity for already-authorized identity rows without
    /// entering the member actor or inventing activity for unavailable sources.
    pub(crate) async fn response_phases_for_identities(
        &self,
        identities: &[ConsoleIdentityRecord],
    ) -> ConsoleLogResult<HashMap<String, Option<String>>> {
        let entries = self
            .inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut phases = HashMap::new();
        for entry in entries {
            let recorded = entry.console_events.response_phases_snapshot().await;
            for identity in identities
                .iter()
                .filter(|row| row.runtime_key == entry.runtime_key)
            {
                for candidate in
                    namespace_match_candidates(&identity.identity, &entry.identity_namespace)
                {
                    if let Some(phase) = recorded.get(&candidate) {
                        phases.insert(identity.identity.clone(), phase.clone());
                        break;
                    }
                }
            }
        }
        Ok(phases)
    }

    #[cfg(test)]
    pub(crate) async fn list_identities_fresh(
        &self,
    ) -> ConsoleLogResult<Vec<ConsoleIdentityRecord>> {
        let _guard = self
            .inner
            .identity_read_model
            .refresh_lock
            .clone()
            .lock_owned()
            .await;
        let identities =
            collect_identity_records(&self.inner, IdentityCollectionMode::IncludeLiveMembers)
                .await?;
        self.inner
            .identity_read_model
            .replace(identities.clone())
            .await;
        spawn_identity_backfills_for_records(self.inner.clone(), &identities);
        Ok(identities)
    }

    pub async fn inspect_identity(
        &self,
        identity: &str,
    ) -> ConsoleLogResult<Option<ConsoleIdentityInspection>> {
        let entries = self
            .inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let live_records = Box::pin(self.live_records_for_identity(identity)).await;
        let mut durable_matches = Vec::new();
        if !requested_identity_has_runtime_member_alias(identity, &entries) {
            for entry in &entries {
                let Some(identity_runtime) = entry.identity_runtime.clone() else {
                    continue;
                };
                for raw_identity in namespace_match_candidates(identity, &entry.identity_namespace)
                {
                    let Ok(parsed_identity) =
                        crate::identity_first::AgentIdentity::parse(&raw_identity)
                    else {
                        continue;
                    };
                    let Ok(status) = identity_runtime.status(&parsed_identity).await else {
                        continue;
                    };
                    let mut record = identity_record_for_status(entry, &status);
                    let topology_peers =
                        identity_runtime_topology_peers(entry, identity_runtime.as_ref()).await;
                    record.topology_peers = topology_peers
                        .get(&record.identity)
                        .cloned()
                        .unwrap_or_default();
                    if Box::pin(console_identity_record_visible(entry, &record)).await {
                        durable_matches.push((entry.clone(), record));
                    }
                }
            }
        }
        if durable_matches.len() > 1 {
            let candidates = durable_matches
                .iter()
                .map(|(_, record)| record.runtime_member_id.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "ambiguous durable identity alias {identity}: candidates [{candidates}]"
            )
            .into());
        }
        if let Some((entry, record)) = durable_matches.into_iter().next() {
            let durable_live_records = Box::pin(self.live_records_for_durable_record(
                &entry,
                identity,
                &record,
                &live_records,
            ))
            .await;
            let visible_durable_live_records = Box::pin(visible_live_records_for_entry(
                &entry,
                &durable_live_records,
            ))
            .await;
            if let Some(ambiguous_error) =
                ambiguous_live_alias_error(identity, &visible_durable_live_records)
            {
                return Err(ambiguous_error.into());
            }
            // Read-only: a durable/live session split-brain is REPORTED here,
            // never repaired. Repair is a write-path act (delivery, retire),
            // because repairing suspends the identity, re-acquires its lease
            // and advances its continuity fence - authority mutation an
            // `agent.view` grant must never be able to trigger, least of all
            // into `Broken` when that fence advance fails.
            if let Some(stale_error) =
                stale_durable_record_error(identity, &record, &durable_live_records)
            {
                return Err(stale_error.into());
            }
            if let Some(session_id) = record.session_id.clone() {
                spawn_session_history_backfill_target(
                    self.inner.clone(),
                    SessionBackfillTarget {
                        assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
                        provenance: None,
                        entry,
                        record: record.clone(),
                        session_id,
                    },
                    false,
                );
            }
            return Ok(Some(ConsoleIdentityInspection {
                peers: record.topology_peers.clone(),
                identity: record,
            }));
        }
        let mut runtime_id_matches = Vec::new();
        for entry in &entries {
            let Some(identity_runtime) = entry.identity_runtime.clone() else {
                continue;
            };
            let Some(raw_identity) = strip_namespace(identity, &entry.identity_namespace) else {
                continue;
            };
            let Some(status) = identity_runtime
                .statuses()
                .await?
                .into_iter()
                .find(|status| {
                    status
                        .agent_runtime_id
                        .as_ref()
                        .is_some_and(|runtime_id| runtime_id.as_str() == raw_identity)
                })
            else {
                continue;
            };
            let mut record = identity_record_for_status(entry, &status);
            let topology_peers =
                identity_runtime_topology_peers(entry, identity_runtime.as_ref()).await;
            record.topology_peers = topology_peers
                .get(&record.identity)
                .cloned()
                .unwrap_or_default();
            if Box::pin(console_identity_record_visible(entry, &record)).await {
                runtime_id_matches.push((entry.clone(), record));
            }
        }
        if runtime_id_matches.len() > 1 {
            let candidates = runtime_id_matches
                .iter()
                .map(|(_, record)| record.identity.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "ambiguous runtime identity alias {identity}: candidates [{candidates}]"
            )
            .into());
        }
        if let Some((entry, record)) = runtime_id_matches.into_iter().next() {
            let durable_live_records = Box::pin(self.live_records_for_durable_record(
                &entry,
                identity,
                &record,
                &live_records,
            ))
            .await;
            let visible_durable_live_records = Box::pin(visible_live_records_for_entry(
                &entry,
                &durable_live_records,
            ))
            .await;
            if let Some(ambiguous_error) =
                ambiguous_live_alias_error(&record.identity, &visible_durable_live_records)
            {
                return Err(ambiguous_error.into());
            }
            // Read-only: report the split-brain, do not repair it (see the
            // durable-match branch above).
            if let Some(stale_error) =
                stale_durable_record_error(identity, &record, &durable_live_records)
            {
                return Err(stale_error.into());
            }
            if let Some(session_id) = record.session_id.clone() {
                spawn_session_history_backfill_target(
                    self.inner.clone(),
                    SessionBackfillTarget {
                        assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
                        provenance: None,
                        entry: entry.clone(),
                        record: record.clone(),
                        session_id,
                    },
                    false,
                );
            }
            return Ok(Some(ConsoleIdentityInspection {
                peers: record.topology_peers.clone(),
                identity: record,
            }));
        }

        let live_matches = Box::pin(self.resolve_visible_members(identity)).await;
        if live_matches.len() > 1 {
            let candidates = live_matches
                .iter()
                .map(|resolved| resolved.runtime_identity.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "ambiguous live identity alias [via aggregator-inspect] {identity}: candidates [{candidates}]"
            )
            .into());
        }
        if let Some(resolved) = live_matches.into_iter().next() {
            let Some(record) = identity_record_for_resolved_member(&resolved).await else {
                return Ok(None);
            };
            // Read-only counterpart of the reconciling form the write paths
            // use: the console reports the stale binding instead of rebinding
            // it.
            if let Some(stale_error) =
                stale_live_record_binding_error(&resolved.entry, identity, &record).await
            {
                return Err(stale_error.into());
            }
            if let Some(identity_runtime) = resolved.entry.identity_runtime.as_ref()
                && let Some(parsed_identity) = identity_runtime
                    .owned_identity_for_member_alias(&resolved.runtime_identity)
                    .await
                && let Ok(status) = identity_runtime.status(&parsed_identity).await
            {
                let durable_record = identity_record_for_status(&resolved.entry, &status);
                let durable_live_records = Box::pin(self.live_records_for_durable_record(
                    &resolved.entry,
                    identity,
                    &durable_record,
                    &live_records,
                ))
                .await;
                let visible_durable_live_records = Box::pin(visible_live_records_for_entry(
                    &resolved.entry,
                    &durable_live_records,
                ))
                .await;
                if let Some(ambiguous_error) =
                    ambiguous_live_alias_error(&record.identity, &visible_durable_live_records)
                {
                    return Err(ambiguous_error.into());
                }
                // Read-only: report the split-brain, do not repair it (see the
                // durable-match branch above).
                if let Some(stale_error) =
                    stale_durable_record_error(identity, &durable_record, &durable_live_records)
                {
                    return Err(stale_error.into());
                }
                // Identity-level. `resolved.runtime_identity` is the DECODED
                // roster member id, which since the stable-identity lowering is
                // the bare durable identity, so comparing it against an
                // `rt:{identity}:{generation}` spelling was never equal and this
                // gate skipped every healthy member.
                if status.agent_runtime_id.is_some()
                    && !crate::member_comms_id::live_member_names_identity_or_incarnation(
                        &resolved.runtime_identity,
                        status.identity.as_str(),
                        status
                            .agent_runtime_id
                            .as_ref()
                            .map(crate::identity_first::AgentRuntimeId::as_str),
                    )
                {
                    return Ok(None);
                }
            }
            if !Box::pin(resolved_member_visible(&resolved, &record)).await {
                return Ok(None);
            }
            if let Some(session_id) = record.session_id.clone() {
                spawn_session_history_backfill_target(
                    self.inner.clone(),
                    SessionBackfillTarget {
                        assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
                        provenance: Some(
                            Box::pin(capture_member_provenance(&self.inner, &resolved, &record))
                                .await,
                        ),
                        entry: resolved.entry.clone(),
                        record: record.clone(),
                        session_id,
                    },
                    false,
                );
            }
            let peers = resolved
                .member
                .wired_to
                .iter()
                .map(|peer| crate::member_comms_id::runtime_alias_str(peer.as_str()).into_owned())
                .collect();
            return Ok(Some(ConsoleIdentityInspection {
                identity: record,
                peers,
            }));
        }

        Ok(None)
    }

    pub async fn retire_identity(&self, identity: &str) -> ConsoleLogResult<bool> {
        let entries = self
            .inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?
            .clone();
        let live_records = Box::pin(self.live_records_for_identity(identity)).await;
        let entries_vec = entries.values().cloned().collect::<Vec<_>>();
        let mut durable_matches = Vec::new();
        if !requested_identity_has_runtime_member_alias(identity, &entries_vec) {
            for entry in entries.values() {
                let Some(identity_runtime) = entry.identity_runtime.clone() else {
                    continue;
                };
                for raw_identity in namespace_match_candidates(identity, &entry.identity_namespace)
                {
                    let Ok(parsed_identity) =
                        crate::identity_first::AgentIdentity::parse(&raw_identity)
                    else {
                        continue;
                    };
                    let Ok(status) = identity_runtime.status(&parsed_identity).await else {
                        continue;
                    };
                    let record = identity_record_for_status(entry, &status);
                    if !Box::pin(console_identity_record_visible(entry, &record)).await {
                        continue;
                    }
                    durable_matches.push((
                        entry.clone(),
                        identity_runtime.clone(),
                        parsed_identity,
                        record,
                    ));
                }
            }
        }
        if durable_matches.len() > 1 {
            let candidates = durable_matches
                .iter()
                .map(|(_, _, identity, _)| identity.as_str().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "ambiguous durable identity alias {identity}: candidates [{candidates}]"
            )
            .into());
        }
        if let Some((entry, identity_runtime, parsed_identity, record)) =
            durable_matches.into_iter().next()
        {
            let durable_live_records = Box::pin(self.live_records_for_durable_record(
                &entry,
                identity,
                &record,
                &live_records,
            ))
            .await;
            let visible_durable_live_records = Box::pin(visible_live_records_for_entry(
                &entry,
                &durable_live_records,
            ))
            .await;
            if let Some(ambiguous_error) =
                ambiguous_live_alias_error(identity, &visible_durable_live_records)
            {
                return Err(ambiguous_error.into());
            }
            if let Some(stale_error) =
                stale_durable_record_error(identity, &record, &durable_live_records)
            {
                return Err(stale_error.into());
            }
            let cleanup_entry = entry.clone();
            let include_current = !identity_runtime.has_session_bridge();
            let cleanup_error = identity_runtime
                .retire_and_cleanup_live_members_tracked(
                    &parsed_identity,
                    None,
                    move |retired_alias| async move {
                        let retired_alias = retired_alias?;
                        let stale_members = Box::pin(stale_generated_members_for_runtime_entry(
                            &cleanup_entry,
                            retired_alias.as_str(),
                            include_current,
                        ))
                        .await;
                        Box::pin(retire_captured_console_members(stale_members))
                            .await
                            .err()
                    },
                )
                .await
                .map_err(|err| -> ConsoleLogError {
                    format!("retire failed for {identity}: {err}").into()
                })?;
            if let Some(err) = cleanup_error {
                tracing::warn!(
                    identity,
                    error = %err,
                    "stale console member cleanup failed after durable identity retire"
                );
            }
            return Ok(true);
        }
        let matches = Box::pin(self.resolve_visible_members(identity)).await;
        if matches.len() > 1 {
            let candidates = matches
                .iter()
                .map(|resolved| resolved.runtime_identity.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "ambiguous live identity alias [via aggregator-retire] {identity}: candidates [{candidates}]"
            )
            .into());
        }
        let mut live_retired_any = false;
        for resolved in matches {
            let Some(record) = identity_record_for_resolved_member(&resolved).await else {
                continue;
            };
            if !Box::pin(resolved_member_visible(&resolved, &record)).await {
                continue;
            }
            if let Some(stale_error) =
                stale_live_record_binding_error(&resolved.entry, identity, &record).await
            {
                return Err(stale_error.into());
            }
            if let Some(identity_runtime) = resolved.entry.identity_runtime.as_ref()
                && let Some(parsed_identity) = identity_runtime
                    .identity_for_member_mutation(&resolved.runtime_identity)
                    .await
            {
                let status = identity_runtime.status(&parsed_identity).await.map_err(
                    |err| -> ConsoleLogError {
                        format!("retire failed for {identity}: {err}").into()
                    },
                )?;
                let durable_record = identity_record_for_status(&resolved.entry, &status);
                let durable_live_records = Box::pin(self.live_records_for_durable_record(
                    &resolved.entry,
                    identity,
                    &durable_record,
                    &live_records,
                ))
                .await;
                let visible_durable_live_records = Box::pin(visible_live_records_for_entry(
                    &resolved.entry,
                    &durable_live_records,
                ))
                .await;
                if let Some(ambiguous_error) =
                    ambiguous_live_alias_error(&record.identity, &visible_durable_live_records)
                {
                    return Err(ambiguous_error.into());
                }
                if let Some(stale_error) =
                    stale_durable_record_error(identity, &durable_record, &durable_live_records)
                {
                    return Err(stale_error.into());
                }
                // Identity-level. `resolved.runtime_identity` is the DECODED
                // roster member id, which since the stable-identity lowering is
                // the bare durable identity, so comparing it against an
                // `rt:{identity}:{generation}` spelling was never equal and this
                // gate skipped every healthy member.
                if status.agent_runtime_id.is_some()
                    && !crate::member_comms_id::live_member_names_identity_or_incarnation(
                        &resolved.runtime_identity,
                        status.identity.as_str(),
                        status
                            .agent_runtime_id
                            .as_ref()
                            .map(crate::identity_first::AgentRuntimeId::as_str),
                    )
                {
                    continue;
                }
                // Member-id alias rule: a request that named a generated alias
                // re-checks that alias under the lifecycle lock, so a generation
                // that went stale after it was resolved retires nothing.
                let expected_alias =
                    namespace_match_candidates(identity, &resolved.entry.identity_namespace)
                        .into_iter()
                        .map(|raw| crate::member_comms_id::runtime_alias_str(&raw).into_owned())
                        .find(|candidate| {
                            crate::member_comms_id::durable_identity_from_runtime_alias(candidate)
                                .is_some_and(|durable| durable == parsed_identity.as_str())
                        })
                        .unwrap_or_else(|| resolved.runtime_identity.clone());
                identity_runtime
                    .retire_member_alias_tracked(&parsed_identity, &expected_alias)
                    .await
                    .map_err(|err| -> ConsoleLogError {
                        format!("retire failed for {identity}: {err}").into()
                    })?;
                live_retired_any = true;
                continue;
            }
            resolved
                .handle
                .retire(crate::member_comms_id::mob_member_id(
                    resolved.runtime_identity.as_str(),
                ))
                .await
                .map_err(|err| -> ConsoleLogError {
                    format!("retire failed for {identity}: {err}").into()
                })
                .or_else(|err| {
                    if lifecycle_archive_cleanup_completed(&err.to_string()) {
                        Ok(())
                    } else {
                        Err(err)
                    }
                })?;
            live_retired_any = true;
        }
        Ok(live_retired_any)
    }

    pub async fn clear_timeline_frames(&self) -> ConsoleLogResult<()> {
        let owner = self
            .inner
            .projection_owner
            .as_deref()
            .unwrap_or(&self.inner);
        owner.store.clear_frames().await?;
        owner
            .notice_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, _| false);
        Ok(())
    }

    pub async fn query_timeline(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let page = Box::pin(self.query_timeline_windowed(query.into())).await?;
        Ok(ConsoleTimelinePage {
            frames: page.frames,
            next_cursor: page.next_cursor,
        })
    }

    pub async fn query_timeline_windowed(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
        Box::pin(self.query_timeline_windowed_scan(
            query,
            None,
            TIMELINE_MAX_RAW_SCAN_FRAMES,
            FrameProjection::ForView,
        ))
        .await
    }

    /// [`Self::query_timeline_windowed`] for one caller: `frame_filter`
    /// decides each stored frame, with its captured member provenance,
    /// inside the scan and before the frame is projected, so the page limit
    /// counts only frames the caller may see.
    pub(crate) async fn query_timeline_windowed_for(
        &self,
        query: ConsoleTimelineWindowQuery,
        frame_filter: Option<&TimelineFrameFilter<'_>>,
    ) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
        Box::pin(self.query_timeline_windowed_scan(
            query,
            frame_filter,
            TIMELINE_MAX_RAW_SCAN_FRAMES,
            FrameProjection::ForView,
        ))
        .await
    }

    /// [`Self::query_timeline_windowed_for`] returning the stored frames
    /// unprojected, for a stream that decides each frame again, under the
    /// policy committed before it is sent, and projects it then.
    pub(crate) async fn query_timeline_windowed_stored_for(
        &self,
        query: ConsoleTimelineWindowQuery,
        frame_filter: Option<&TimelineFrameFilter<'_>>,
    ) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
        Box::pin(self.query_timeline_windowed_scan(
            query,
            frame_filter,
            TIMELINE_MAX_RAW_SCAN_FRAMES,
            FrameProjection::Stored,
        ))
        .await
    }

    /// [`Self::query_timeline_windowed_for`] under a smaller raw scan budget.
    #[cfg(test)]
    pub(crate) async fn query_timeline_windowed_for_with_scan_budget(
        &self,
        query: ConsoleTimelineWindowQuery,
        frame_filter: Option<&TimelineFrameFilter<'_>>,
        scan_budget: usize,
    ) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
        Box::pin(self.query_timeline_windowed_scan(
            query,
            frame_filter,
            scan_budget,
            FrameProjection::ForView,
        ))
        .await
    }

    async fn query_timeline_windowed_scan(
        &self,
        query: ConsoleTimelineWindowQuery,
        frame_filter: Option<&TimelineFrameFilter<'_>>,
        scan_budget: usize,
        projection: FrameProjection,
    ) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
        self.reject_after_cursor_beyond_store_frontier(query.after.as_ref())
            .await?;
        if query.after.is_none()
            && query.before.is_none()
            && let Some(identity) = query.identity.clone()
        {
            let mut probe_query = query.clone();
            probe_query.limit = TIMELINE_RAW_SCAN_PAGE_LIMIT;
            let page = self.inner.store.query_windowed_frames(probe_query).await?;
            if explicit_identity_query_needs_session_history_backfill(&page.frames) {
                spawn_session_history_backfill_for_identity(
                    self.inner.clone(),
                    identity,
                    true,
                    AssistantHistoryRefreshReason::Recovery,
                );
            }
        }
        Box::pin(self.query_timeline_visible(query, frame_filter, scan_budget, projection)).await
    }

    async fn reject_after_cursor_beyond_store_frontier(
        &self,
        after: Option<&ConsoleCursor>,
    ) -> ConsoleTimelineQueryResult<()> {
        let Some(after_seq) = after.and_then(ConsoleCursor::seq) else {
            return Ok(());
        };
        let latest_cursor = self.inner.store.latest_cursor().await?;
        let latest_seq = latest_cursor
            .as_ref()
            .and_then(ConsoleCursor::seq)
            .unwrap_or(0);
        if after_seq > latest_seq {
            return Err(ConsoleTimelineQueryError::ReplayUnavailable {
                requested_cursor: after.cloned(),
                latest_cursor,
            });
        }
        Ok(())
    }

    async fn query_timeline_visible(
        &self,
        query: ConsoleTimelineWindowQuery,
        frame_filter: Option<&TimelineFrameFilter<'_>>,
        scan_budget: usize,
        projection: FrameProjection,
    ) -> ConsoleTimelineQueryResult<ConsoleTimelineWindowPage> {
        let requested_limit = query.limit.clamp(1, TIMELINE_RAW_SCAN_PAGE_LIMIT);
        let mut scan_query = query.clone();
        scan_query.limit = TIMELINE_RAW_SCAN_PAGE_LIMIT;
        let mut visible_frames = Vec::with_capacity(requested_limit);
        let mut identity_visibility_cache = HashMap::new();
        let identity_records = self.inner.identity_read_model.current().await;
        let mut next_cursor = query.after.clone();
        let mut since_last_scanned_cursor = query.after.clone();
        let mut since_last_delivered_cursor = None;
        let mut since_stopped_at_visible_limit = false;
        let mut latest_cursor = None;
        let mut exhausted = false;
        let mut scanned = 0usize;

        loop {
            let page = self
                .inner
                .store
                .query_windowed_frames(scan_query.clone())
                .await?;
            latest_cursor = latest_cursor.or(page.latest_cursor.clone());
            // Older custom stores leave exhausted at its default even at the
            // frontier. An empty page is terminal; nonempty repeats still fail.
            if page.frames.is_empty() {
                exhausted = true;
                break;
            }
            if !page.exhausted {
                let progressed = match query.mode {
                    ConsoleTimelineMode::Since => page.next_cursor.as_ref().is_some_and(|next| {
                        scan_query.after.as_ref().is_none_or(|after| {
                            match (next.seq(), after.seq()) {
                                (Some(next), Some(after)) => next > after,
                                _ => next != after,
                            }
                        })
                    }),
                    ConsoleTimelineMode::Recent => page.frames.first().is_some_and(|first| {
                        scan_query.before.as_ref().is_none_or(|before| {
                            match (first.cursor.seq(), before.seq()) {
                                (Some(next), Some(before)) => next < before,
                                _ => &first.cursor != before,
                            }
                        })
                    }),
                };
                if !progressed {
                    return Err(ConsoleTimelineQueryError::PaginationNoProgress);
                }
            }
            let raw_len = page.frames.len();
            scanned = scanned.saturating_add(raw_len);
            match query.mode {
                ConsoleTimelineMode::Since => {
                    if let Some(cursor) = page.next_cursor.clone() {
                        since_last_scanned_cursor = Some(cursor.clone());
                        scan_query.after = Some(cursor);
                    }
                }
                ConsoleTimelineMode::Recent => {
                    if let Some(first) = page.frames.first() {
                        scan_query.before = Some(first.cursor.clone());
                    }
                    next_cursor = page.next_cursor.clone().or(next_cursor);
                }
            }

            for frame in page.frames {
                let allow_historical_identity =
                    query.identity.as_deref() == Some(frame.identity.as_str());
                if Box::pin(frame_is_visible_cached(
                    &self.inner,
                    &frame,
                    allow_historical_identity,
                    &mut identity_visibility_cache,
                    &identity_records,
                ))
                .await
                .unwrap_or(false)
                    && frame_filter.is_none_or(|visible| visible(&frame))
                {
                    match query.mode {
                        ConsoleTimelineMode::Since => {
                            if visible_frames.len() >= requested_limit {
                                since_stopped_at_visible_limit = true;
                                break;
                            }
                            since_last_delivered_cursor = Some(frame.cursor.clone());
                            visible_frames.push(frame);
                            if visible_frames.len() >= requested_limit {
                                since_stopped_at_visible_limit = true;
                                exhausted = false;
                                break;
                            }
                        }
                        ConsoleTimelineMode::Recent => {
                            visible_frames.push(frame);
                        }
                    }
                }
            }
            if since_stopped_at_visible_limit {
                break;
            }
            // Store exhaustion is authoritative even when custom stores return
            // shorter pages than requested. Retain it only if no visible older
            // frames are omitted from the bounded recent result below.
            if page.exhausted {
                exhausted = true;
                break;
            }
            if visible_frames.len() >= requested_limit {
                break;
            }
            if scanned >= scan_budget {
                if query.mode == ConsoleTimelineMode::Recent && visible_frames.is_empty() {
                    // Nothing visible within the budget: answer the shape of
                    // an empty visible page, with a before cursor at the
                    // oldest frame scanned so the caller can continue into
                    // older history. Not an error: an error would tell the
                    // caller that frames it may not see exist.
                    return Ok(ConsoleTimelineWindowPage {
                        frames: Vec::new(),
                        next_cursor: scan_query.before.clone().or(next_cursor),
                        latest_cursor,
                        exhausted: false,
                    });
                }
                break;
            }
        }

        if query.mode == ConsoleTimelineMode::Recent {
            visible_frames.sort_by_key(|frame| frame.cursor.seq().unwrap_or(u64::MAX));
            if visible_frames.len() > requested_limit {
                // Keep one contiguous suffix. Mixing early turn anchors into a
                // recent tail makes before-oldest paging skip the middle.
                visible_frames = visible_frames.split_off(visible_frames.len() - requested_limit);
                exhausted = false;
            }
        }

        Ok(ConsoleTimelineWindowPage {
            frames: match projection {
                FrameProjection::ForView => visible_frames
                    .into_iter()
                    .map(|frame| self.project_frame_for_view(frame))
                    .collect(),
                FrameProjection::Stored => visible_frames,
            },
            next_cursor: match query.mode {
                ConsoleTimelineMode::Since if since_stopped_at_visible_limit => {
                    since_last_delivered_cursor.or(since_last_scanned_cursor)
                }
                ConsoleTimelineMode::Since => since_last_scanned_cursor,
                ConsoleTimelineMode::Recent => next_cursor,
            },
            latest_cursor,
            exhausted,
        })
    }

    pub async fn refresh_session_history(&self) -> ConsoleLogResult<()> {
        let runtime_keys = self
            .inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let results = join_all(runtime_keys.into_iter().map(|runtime_key| {
            backfill_session_history(
                self.inner.clone(),
                runtime_key,
                true,
                AssistantHistoryRefreshReason::Recovery,
            )
        }))
        .await;
        for result in results {
            result?;
        }
        Ok(())
    }

    pub async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.store.latest_cursor().await
    }

    pub async fn timeline_event_visible(&self, event: &ConsoleTimelineEvent) -> bool {
        self.timeline_event_visible_for_subscriber(event, None)
            .await
    }

    /// Live-stream visibility gate, subscriber-aware (issue #254).
    ///
    /// The windowed query paths grant an own-identity allowance
    /// (`allow_historical_identity = query.identity == frame.identity`), but
    /// the live gate used to pass `false` unconditionally — so an
    /// identity-scoped SSE stream dropped every frame of a member the
    /// identity read model had not yet observed (`records=0 → Missing`),
    /// starving live tails while reconnect-with-snapshot showed full
    /// history. Two fixes, matching the issue's suggestions:
    ///
    /// 1. An identity-scoped subscriber gets the SAME own-identity
    ///    allowance the query paths grant, so its frames flow immediately.
    /// 2. A frame whose identity is unknown to the read model triggers a
    ///    debounced `ConsoleIdentityReadModel::refresh_soon` - the model
    ///    converges for UNSCOPED streams too (members spawned mid-run via
    ///    `ensure_member` previously stayed invisible until an unrelated
    ///    refresh).
    pub async fn timeline_event_visible_for_subscriber(
        &self,
        event: &ConsoleTimelineEvent,
        subscriber_identity: Option<&str>,
    ) -> bool {
        match event {
            ConsoleTimelineEvent::ConsoleFrame { frame }
            | ConsoleTimelineEvent::FrameUpdated { frame } => {
                let identity_records = self.inner.identity_read_model.current().await;
                let identity_known = identity_records.iter().any(|record| {
                    record.runtime_key == frame.runtime_key
                        && (record.identity == frame.identity
                            || record.runtime_member_id == frame.identity)
                });
                if !identity_known
                    && !crate::console_contracts::is_runtime_plane_identity(&frame.identity)
                {
                    // Self-heal: the read model has never seen this identity
                    // (e.g. ensure_member mid-run). Debounced internally; a
                    // spurious trigger for a namespaced alias mismatch just
                    // refreshes early.
                    self.inner
                        .identity_read_model
                        .refresh_soon(self.inner.clone());
                }
                let allow_historical_identity =
                    subscriber_identity == Some(frame.identity.as_str());
                Box::pin(frame_is_visible(
                    &self.inner,
                    frame,
                    allow_historical_identity,
                    &identity_records,
                ))
                .await
                .unwrap_or(false)
            }
            ConsoleTimelineEvent::SnapshotStarted { .. }
            | ConsoleTimelineEvent::SnapshotComplete { .. }
            | ConsoleTimelineEvent::ReplayUnavailable { .. } => true,
        }
    }

    pub async fn timeline_frame_visible_for_query(
        &self,
        frame: &ConsoleFrame,
        identity: Option<&str>,
    ) -> bool {
        let allow_historical_identity = identity == Some(frame.identity.as_str());
        let identity_records = self.inner.identity_read_model.current().await;
        Box::pin(frame_is_visible(
            &self.inner,
            frame,
            allow_historical_identity,
            &identity_records,
        ))
        .await
        .unwrap_or(false)
    }

    pub async fn send(
        &self,
        request: ConsoleSendRequest,
    ) -> Result<ConsoleInteractionAccepted, ConsoleSendError> {
        Box::pin(self.send_scoped(request, &ConsoleSendScope::Unscoped)).await
    }

    /// [`Self::send`] deduplicated in the sending caller's `scope`. A scoped
    /// replay of a send that has not settled, and whose dispatch has no live
    /// owner, is reconciled from native admission evidence: an admitted input
    /// settles it delivered; an absent one is dispatched again under the
    /// send's own delivery identity, into the session it recorded.
    pub(crate) async fn send_scoped(
        &self,
        request: ConsoleSendRequest,
        scope: &ConsoleSendScope,
    ) -> Result<ConsoleInteractionAccepted, ConsoleSendError> {
        validate_send_request(&request)?;
        let Some(resolved) = Box::pin(self.resolve_send_member(&request.identity)).await? else {
            return Err(ConsoleSendError::UnknownIdentity(request.identity));
        };
        let host_policy = capture_host_payload_policy(
            &self.inner,
            &resolved.entry.runtime_key,
            Some(resolved.entry.registration_id),
        )
        .map_err(ConsoleSendError::Log)?;
        let Some(record) = identity_record_for_resolved_member(&resolved).await else {
            return Err(ConsoleSendError::UnknownIdentity(request.identity));
        };
        if !Box::pin(resolved_member_visible(&resolved, &record)).await {
            return Err(ConsoleSendError::UnknownIdentity(request.identity));
        }
        if !member_is_addressable(&resolved.member) {
            return Err(ConsoleSendError::NotAddressable(request.identity));
        }
        if resolved.member.status == meerkat_mob::MobMemberStatus::Retiring {
            return Err(ConsoleSendError::Retired(request.identity));
        }

        let content = content_input_from_value(&request.content)?;
        let handling_mode = parse_handling_mode(request.handling_mode.as_deref())?;
        // A peer-only member has no local session and so no host-human skill
        // carrier: refuse a selection before the interaction is reserved.
        //
        // Remote placement and an external backend are not refused here.
        // Neither `MobMemberListEntry` nor `RosterEntry` exposes them (a placed
        // member's bridge session names its remote resident session), and the
        // only read that does is the member status observation, an actor
        // round trip that joins the member's status observation and can hold
        // a pending kickoff turn off (meerkat #1226). The member lane already
        // performs that observation once, at dispatch. There the external
        // branch refuses the selection typed (`HostHumanInputError::
        // Unsupported`), and meerkat's host-human admission refuses a
        // remotely hosted member typed (`MobError::UnsupportedForMode`)
        // before any transport delivery. Both land on the `user_input` frame
        // as `delivery_failed` with a typed `message_delivery_failed` frame
        // (`data.kind = "host_human_input_unsupported"`); nothing is
        // delivered to the member.
        if !request.skill_refs.is_empty() {
            let local_session = resolved
                .handle
                .get_member(&resolved.member.agent_identity)
                .await
                .ok()
                .flatten()
                .is_some_and(|entry| entry.bridge_session_id().is_some());
            if !local_session {
                return Err(ConsoleSendError::InvalidRequest(
                    "selected skills require a local session-backed member".to_string(),
                ));
            }
        }
        assert_member_accepts_images_with_config(
            &resolved.handle,
            resolved.entry.runtime.session_service(),
            resolved.entry.runtime.member_model_config().as_deref(),
            &resolved.runtime_identity,
            &content,
        )
        .await
        .map_err(|err| ConsoleSendError::InvalidContent(err.to_string()))?;

        let dedupe_key = scope.dedupe_key(
            &resolved.entry.runtime_key,
            &request.identity,
            &request.origin,
            &request.idempotency_key,
        );
        let handling_mode_value = request
            .handling_mode
            .as_deref()
            .unwrap_or("queue")
            .to_string();
        let request_fingerprint = send_request_fingerprint(
            &request.origin,
            request.origin_kind,
            &request.content,
            &handling_mode_value,
            &request.selected_skill_keys(),
        );
        let resend = ClassicResend {
            resolved: &resolved,
            host_policy: &host_policy,
            content: &content,
            skill_references: request.selected_skill_keys(),
            handling_mode,
        };
        if let Some(existing) = self
            .inner
            .store
            .frame_by_dedupe_key(&dedupe_key)
            .await
            .map_err(ConsoleSendError::Log)?
        {
            let same_request = existing.source.source_cursor.as_deref()
                == Some(request_fingerprint.as_str())
                || existing.source.source_cursor.is_none()
                    && existing.payload.get("origin").and_then(Value::as_str)
                        == Some(request.origin.as_str())
                    && existing.payload.get("content") == Some(&request.content)
                    && existing
                        .payload
                        .get("handling_mode")
                        .and_then(Value::as_str)
                        == Some(handling_mode_value.as_str())
                    && existing.payload.get("origin_kind").and_then(Value::as_str)
                        == request.origin_kind.map(ConsoleTurnOrigin::as_str)
                    && request.skill_refs.is_empty()
                    && existing.payload.get("skill_refs").is_none();
            if !same_request {
                return Err(ConsoleSendError::IdempotencyConflict(
                    request.idempotency_key,
                ));
            }
            return match self.existing_send(existing, scope, None)? {
                ExistingSend::Replay(accepted) => Ok(accepted),
                ExistingSend::Unsettled(accepted, claim, frame) => {
                    Box::pin(self.reconcile_classic_send(resend, accepted, claim, *frame)).await
                }
            };
        }

        // The dispatch claim is taken before the frame exists, so a
        // concurrent identical send finds the frame owned.
        let frame_id = stable_frame_id(&dedupe_key);
        let claim = self.claim_scoped_send(scope, &frame_id);
        let interaction_id = identity_first_interaction_uuid(&dedupe_key);
        // Reserve under the DURABLE console identity with the runtime
        // incarnation as the mapping KEY - the same runtime-id -> identity
        // registration the spawn path makes. Reserving under the runtime
        // identity itself (the pre-fix shape) self-mapped the incarnation in
        // `runtime_to_identity`, clobbering the spawn registration: every
        // live completion event then keyed the `{identity}:{generation}`
        // conversation while the durable conversation - the one the console
        // UI renders - showed only this user_input frame.
        let durable_identity =
            crate::member_comms_id::durable_identity_label(&resolved.member.labels)
                .unwrap_or(resolved.runtime_identity.as_str())
                .to_string();
        resolved
            .entry
            .console_events
            .reserve_interaction_value(
                &durable_identity,
                Some(resolved.runtime_identity.as_str()),
                &interaction_id,
                &request.origin,
                request.content.clone(),
            )
            .await
            .map_err(ConsoleSendError::State)?;
        let session_id = resolved
            .handle
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                resolved.runtime_identity.as_str(),
            ))
            .await
            .map(|sid| sid.to_string());
        let provenance = match identity_record_for_resolved_member(&resolved).await {
            Some(record) => {
                let owner = self
                    .inner
                    .projection_owner
                    .as_deref()
                    .unwrap_or(&self.inner);
                Some(Box::pin(capture_member_provenance(owner, &resolved, &record)).await)
            }
            None => None,
        };
        let mut new_frame = NewConsoleFrame {
            id: Some(frame_id),
            dedupe_key,
            timestamp_ms: current_time_ms(),
            runtime_key: resolved.entry.runtime_key.clone(),
            identity: request.identity.clone(),
            conversation_id: Some(request.identity.clone()),
            session_id: session_id.clone(),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Accepted,
            payload: user_input_payload(&request, &handling_mode_value),
            source: ConsoleFrameSource {
                member_provenance: provenance,
                kind: ConsoleFrameSourceKind::Send,
                source_cursor: Some(request_fingerprint.clone()),
            },
            source_event_id: None,
            interaction_id: Some(interaction_id.clone()),
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };
        redact_frame(host_policy.as_ref(), &mut new_frame);

        let _ = SendState::Requested
            .apply(SendTransition::PersistAccepted)
            .map_err(ConsoleSendError::State)?;
        let outcome = self
            .inner
            .store
            .append_if_absent(new_frame)
            .await
            .map_err(ConsoleSendError::Log)?;
        if outcome.disposition == AppendDisposition::Existing {
            if outcome.frame.source.source_cursor.as_deref() != Some(&request_fingerprint) {
                return Err(ConsoleSendError::IdempotencyConflict(
                    request.idempotency_key,
                ));
            }
            return match self.existing_send(outcome.frame, scope, claim)? {
                ExistingSend::Replay(accepted) => Ok(accepted),
                ExistingSend::Unsettled(accepted, claim, frame) => {
                    Box::pin(self.reconcile_classic_send(resend, accepted, claim, *frame)).await
                }
            };
        }
        let _ = self
            .inner
            .event_tx
            .send(ConsoleTimelineEvent::ConsoleFrame {
                frame: outcome.frame.clone(),
            });
        let accepted = accepted_from_frame(&outcome.frame);
        if scope.is_scoped() && claim.is_none() {
            // A concurrent identical send holds the claim; it finds this
            // frame and owns its dispatch.
            return Ok(accepted);
        }

        let (dispatching, _effects) = SendState::AcceptedPersisted
            .apply(SendTransition::StartDispatch)
            .map_err(ConsoleSendError::State)?;
        update_frame_status_and_emit(
            &self.inner,
            &outcome.frame.id,
            ConsoleFrameStatus::Dispatching,
        )
        .await
        .map_err(ConsoleSendError::Log)?;
        faults::after_console_reserve();

        spawn_console_send_dispatch(
            self.inner.clone(),
            (resolved, host_policy),
            (content, request.selected_skill_keys()),
            handling_mode,
            dispatching,
            (outcome.frame, claim),
            interaction_id,
        );
        Ok(accepted)
    }

    /// Reconcile a classic send this caller now owns from native evidence:
    /// admitted settles it delivered; absent dispatches it again only through
    /// a keyed, session-pinned delivery (a durable identity's local session),
    /// never through an unkeyed external one; otherwise it stays unsettled.
    async fn reconcile_classic_send(
        &self,
        resend: ClassicResend<'_>,
        accepted: ConsoleInteractionAccepted,
        claim: SendDispatchClaim,
        frame: ConsoleFrame,
    ) -> Result<ConsoleInteractionAccepted, ConsoleSendError> {
        match self.observe_native_send(&frame).await {
            NativeSendObservation::Admitted { .. } => {
                self.settle_admitted_send(&frame, &claim).await
            }
            NativeSendObservation::Absent if resend.keyed_and_pinned(&frame).await => {
                let state = self.begin_send_redispatch(&frame, &claim)?;
                let dispatching = if state == SendState::Dispatching {
                    state
                } else {
                    let (dispatching, _effects) = state
                        .apply(SendTransition::StartDispatch)
                        .map_err(ConsoleSendError::State)?;
                    update_frame_status_and_emit(
                        &self.inner,
                        &frame.id,
                        ConsoleFrameStatus::Dispatching,
                    )
                    .await
                    .map_err(ConsoleSendError::Log)?;
                    dispatching
                };
                let interaction_id = accepted.interaction_id.clone();
                spawn_console_send_dispatch(
                    self.inner.clone(),
                    (resend.resolved.clone(), resend.host_policy.clone()),
                    (resend.content.clone(), resend.skill_references),
                    resend.handling_mode,
                    dispatching,
                    (frame, Some(claim)),
                    interaction_id,
                );
                Ok(accepted)
            }
            NativeSendObservation::Absent | NativeSendObservation::Unknown { .. } => Ok(accepted),
        }
    }

    fn claim_scoped_send(
        &self,
        scope: &ConsoleSendScope,
        frame_id: &str,
    ) -> Option<SendDispatchClaim> {
        scope
            .is_scoped()
            .then(|| self.inner.send_dispatch_claims.claim(frame_id))
            .flatten()
    }

    /// An existing send found for a replay. Settled, or with a live dispatch
    /// owner, it is replayed. Unsettled and unowned (or owned by `held`, the
    /// caller's own claim), a scoped caller owns its reconciliation. The
    /// unscoped API only replays.
    fn existing_send(
        &self,
        frame: ConsoleFrame,
        scope: &ConsoleSendScope,
        held: Option<SendDispatchClaim>,
    ) -> Result<ExistingSend, ConsoleSendError> {
        let unsettled =
            SendState::from_send_frame_status(frame.status).is_some_and(SendState::is_unsettled);
        if scope.is_scoped() && unsettled {
            let claim = match held {
                Some(claim) if claim.frame_id() == frame.id => Some(claim),
                _ => self.inner.send_dispatch_claims.claim(&frame.id),
            };
            if let Some(claim) = claim {
                return Ok(ExistingSend::Unsettled(
                    accepted_from_frame(&frame),
                    claim,
                    Box::new(frame),
                ));
            }
        }
        Ok(ExistingSend::Replay(accepted_replay_from_frame(&frame)?))
    }

    /// Settle an unsettled send whose input the runtime admitted: delivered.
    /// Only the frame's dispatch owner settles it.
    pub(crate) async fn settle_admitted_send(
        &self,
        frame: &ConsoleFrame,
        _owner: &SendDispatchClaim,
    ) -> Result<ConsoleInteractionAccepted, ConsoleSendError> {
        let state = SendState::from_send_frame_status(frame.status)
            .ok_or(ConsoleSendError::State("not a console send frame"))?;
        let (_delivered, _effects) = state
            .reconcile(SendEvidence::Admitted)
            .map_err(ConsoleSendError::State)?;
        let updated =
            update_frame_status_and_emit(&self.inner, &frame.id, ConsoleFrameStatus::Delivered)
                .await
                .map_err(ConsoleSendError::Log)?;
        Ok(accepted_from_frame(updated.as_ref().unwrap_or(frame)))
    }

    /// The typed transition of an owned, unsettled send whose input the
    /// runtime does not hold and whose delivery is keyed and pinned: it is
    /// dispatched again, in the state it is in. The caller dispatches.
    pub(crate) fn begin_send_redispatch(
        &self,
        frame: &ConsoleFrame,
        _owner: &SendDispatchClaim,
    ) -> Result<SendState, ConsoleSendError> {
        let state = SendState::from_send_frame_status(frame.status)
            .ok_or(ConsoleSendError::State("not a console send frame"))?;
        let (state, _effects) = state
            .reconcile(SendEvidence::AbsentRedispatchable)
            .map_err(ConsoleSendError::State)?;
        Ok(state)
    }

    /// What the runtime holding `frame`'s recorded session has admitted
    /// under the send's delivery keys.
    pub(crate) async fn observe_native_send(&self, frame: &ConsoleFrame) -> NativeSendObservation {
        #[cfg(test)]
        {
            let reader = self
                .inner
                .send_admission_override
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(reader) = reader {
                return send_recovery::observe_native_send(reader.as_ref(), frame).await;
            }
        }
        let owner = self
            .inner
            .projection_owner
            .as_deref()
            .unwrap_or(&self.inner);
        let service = owner.runtimes.read().ok().and_then(|runtimes| {
            runtimes
                .get(&frame.runtime_key)
                .and_then(|entry| entry.runtime.session_service().cloned())
        });
        let Some(service) = service else {
            return NativeSendObservation::Unknown {
                cause: "no registered runtime serves the send".to_string(),
            };
        };
        let Some(machine) = crate::mob_handle_runtime::observed_runtime_adapter(service.as_ref())
        else {
            return NativeSendObservation::Unknown {
                cause: "the runtime exposes no admission evidence".to_string(),
            };
        };
        send_recovery::observe_native_send(machine.as_ref(), frame).await
    }

    /// The caller's own send under `idempotency_key`, as it is now, with the
    /// runtime's admission evidence for it. A send the evidence shows admitted
    /// but that has not settled is settled delivered; nothing is dispatched.
    /// `None` when the caller has no such send: a key another caller used is
    /// a different key, so it answers exactly as a missing one.
    pub(crate) async fn recover_send(
        &self,
        identities: &[String],
        origin: &str,
        idempotency_key: &str,
        scope: &ConsoleSendScope,
    ) -> Result<Option<ConsoleSendRecovery>, ConsoleSendError> {
        let runtime_keys = self.send_runtime_keys();
        let mut found = None;
        'search: for runtime_key in &runtime_keys {
            for identity in identities {
                let key = scope.dedupe_key(runtime_key, identity, origin, idempotency_key);
                if let Some(frame) = self
                    .inner
                    .store
                    .frame_by_dedupe_key(&key)
                    .await
                    .map_err(ConsoleSendError::Log)?
                {
                    found = Some(frame);
                    break 'search;
                }
            }
        }
        let Some(frame) = found else {
            return Ok(None);
        };
        let native = self.observe_native_send(&frame).await;
        let unsettled =
            SendState::from_send_frame_status(frame.status).is_some_and(SendState::is_unsettled);
        let mut accepted = accepted_from_frame(&frame);
        if unsettled
            && matches!(native, NativeSendObservation::Admitted { .. })
            && let Some(claim) = self.inner.send_dispatch_claims.claim(&frame.id)
        {
            accepted = self.settle_admitted_send(&frame, &claim).await?;
        }
        Ok(Some(ConsoleSendRecovery { accepted, native }))
    }

    /// Settle, from native admission evidence, the console sends a previous
    /// process accepted but did not see settle. Bounded (the newest
    /// [`send_recovery::SEND_SWEEP_WINDOW`] frames, at most
    /// [`send_recovery::SEND_SWEEP_LIMIT`] sends, one overall deadline);
    /// evidence only: a send whose input was admitted is settled delivered,
    /// and nothing is ever dispatched or failed here.
    pub(crate) async fn reconcile_interrupted_sends(&self) -> SendSweepOutcome {
        let deadline = tokio::time::Instant::now() + send_recovery::SEND_SWEEP_DEADLINE;
        let frames = match self
            .inner
            .store
            .unsettled_send_frames(
                send_recovery::SEND_SWEEP_WINDOW,
                send_recovery::SEND_SWEEP_LIMIT,
            )
            .await
        {
            Ok(frames) => frames,
            Err(error) => {
                tracing::warn!(%error, "console send reconciliation could not list unsettled sends");
                return SendSweepOutcome::default();
            }
        };
        let mut outcome = SendSweepOutcome::default();
        for frame in frames {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    examined = outcome.examined,
                    "console send reconciliation stopped at its deadline"
                );
                break;
            }
            outcome.examined += 1;
            let Some(claim) = self.inner.send_dispatch_claims.claim(&frame.id) else {
                continue;
            };
            if matches!(
                self.observe_native_send(&frame).await,
                NativeSendObservation::Admitted { .. }
            ) {
                match self.settle_admitted_send(&frame, &claim).await {
                    Ok(_) => outcome.delivered += 1,
                    Err(error) => tracing::warn!(
                        frame_id = %frame.id,
                        %error,
                        "console send reconciliation could not settle an admitted send"
                    ),
                }
            }
        }
        outcome
    }

    /// How many console sends a previous process accepted and did not see
    /// settle, counted without settling any: the sends
    /// [`Self::reconcile_interrupted_sends`] would examine. A candidate
    /// launch's census of what it leaves to the authoritative launch.
    pub(crate) async fn unsettled_send_count(&self) -> usize {
        match self
            .inner
            .store
            .unsettled_send_frames(
                send_recovery::SEND_SWEEP_WINDOW,
                send_recovery::SEND_SWEEP_LIMIT,
            )
            .await
        {
            Ok(frames) => frames.len(),
            Err(error) => {
                tracing::warn!(%error, "console send census could not list unsettled sends");
                0
            }
        }
    }

    /// The runtime keys a console send can have been recorded under: this
    /// view's registrations and its projection owner's, or the identity-first
    /// key when no runtime is registered.
    fn send_runtime_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for inner in [Some(&self.inner), self.inner.projection_owner.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Ok(runtimes) = inner.runtimes.read() {
                keys.extend(runtimes.keys().cloned());
            }
        }
        if keys.is_empty() {
            keys.insert("identity-first".to_string());
        }
        keys.into_iter().collect()
    }

    /// Reserve the console interaction for an identity-first send.
    ///
    /// Returns [`IdentityFirstReservation::Existing`] for an idempotent replay
    /// (same key, same origin, content and handling mode) so the caller
    /// answers with the original acceptance and does not dispatch the turn
    /// again.
    pub async fn reserve_identity_first_interaction(
        &self,
        request: ConsoleSendRequest,
        session_id: Option<&str>,
    ) -> Result<IdentityFirstReservation, ConsoleSendError> {
        Ok(
            match Box::pin(self.reserve_identity_first_interaction_scoped(
                request,
                session_id,
                &ConsoleSendScope::Unscoped,
            ))
            .await?
            {
                ScopedReservation::Fresh(accepted, _) => IdentityFirstReservation::Fresh(accepted),
                ScopedReservation::Replay(accepted)
                | ScopedReservation::Unsettled(accepted, ..) => {
                    IdentityFirstReservation::Existing(accepted)
                }
            },
        )
    }

    /// [`Self::reserve_identity_first_interaction`] deduplicated in the
    /// sending caller's `scope`. A scoped caller gets the dispatch claim of a
    /// fresh interaction, and owns the reconciliation of a replayed one that
    /// has not settled and whose dispatch has no live owner.
    pub(crate) async fn reserve_identity_first_interaction_scoped(
        &self,
        request: ConsoleSendRequest,
        session_id: Option<&str>,
        scope: &ConsoleSendScope,
    ) -> Result<ScopedReservation, ConsoleSendError> {
        validate_send_request(&request)?;
        let _content = content_input_from_value(&request.content)?;
        let (runtime_key, host_policy) =
            Box::pin(self.runtime_key_for_identity_first_send(&request.identity)).await?;
        let handling_mode_value = request
            .handling_mode
            .as_deref()
            .unwrap_or("queue")
            .to_string();
        let dedupe_key = scope.dedupe_key(
            &runtime_key,
            &request.identity,
            &request.origin,
            &request.idempotency_key,
        );
        let request_fingerprint = send_request_fingerprint(
            &request.origin,
            request.origin_kind,
            &request.content,
            &handling_mode_value,
            &request.selected_skill_keys(),
        );
        if let Some(existing) = self
            .inner
            .store
            .frame_by_dedupe_key(&dedupe_key)
            .await
            .map_err(ConsoleSendError::Log)?
        {
            let same_request = existing.source.source_cursor.as_deref()
                == Some(request_fingerprint.as_str())
                || existing.source.source_cursor.is_none()
                    && existing.payload.get("origin").and_then(Value::as_str)
                        == Some(request.origin.as_str())
                    && existing.payload.get("content") == Some(&request.content)
                    && existing
                        .payload
                        .get("handling_mode")
                        .and_then(Value::as_str)
                        == Some(handling_mode_value.as_str())
                    && existing.payload.get("origin_kind").and_then(Value::as_str)
                        == request.origin_kind.map(ConsoleTurnOrigin::as_str)
                    && request.skill_refs.is_empty()
                    && existing.payload.get("skill_refs").is_none();
            if !same_request {
                return Err(ConsoleSendError::IdempotencyConflict(
                    request.idempotency_key,
                ));
            }
            return Ok(self.existing_send(existing, scope, None)?.into());
        }

        // Claimed before the frame exists: a concurrent identical send finds
        // it owned and replays it.
        let frame_id = stable_frame_id(&dedupe_key);
        let claim = self.claim_scoped_send(scope, &frame_id);
        let interaction_id = identity_first_interaction_uuid(&dedupe_key);
        let owner = self
            .inner
            .projection_owner
            .as_deref()
            .unwrap_or(&self.inner);
        let entry = owner
            .runtimes
            .read()
            .ok()
            .and_then(|entries| entries.get(&runtime_key).cloned());
        let provenance = if let Some(entry) = entry {
            member_provenance_for_identity(owner, &entry, &request.identity, session_id, false)
                .await
        } else {
            None
        };
        let mut new_frame = NewConsoleFrame {
            id: Some(frame_id),
            dedupe_key,
            timestamp_ms: current_time_ms(),
            runtime_key,
            identity: request.identity.clone(),
            conversation_id: Some(request.identity.clone()),
            session_id: session_id.map(ToString::to_string),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Accepted,
            payload: user_input_payload(&request, &handling_mode_value),
            source: ConsoleFrameSource {
                member_provenance: provenance,
                kind: ConsoleFrameSourceKind::Send,
                source_cursor: Some(request_fingerprint.clone()),
            },
            source_event_id: None,
            interaction_id: Some(interaction_id),
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };
        redact_frame(host_policy.as_ref(), &mut new_frame);
        let outcome = self
            .inner
            .store
            .append_if_absent(new_frame)
            .await
            .map_err(ConsoleSendError::Log)?;
        if outcome.disposition == AppendDisposition::Existing {
            if outcome.frame.source.source_cursor.as_deref() != Some(&request_fingerprint) {
                return Err(ConsoleSendError::IdempotencyConflict(
                    request.idempotency_key,
                ));
            }
            // Lost the race with a concurrent identical send: it owns the
            // dispatch, unless this caller holds the claim.
            return Ok(self.existing_send(outcome.frame, scope, claim)?.into());
        }
        let _ = self
            .inner
            .event_tx
            .send(ConsoleTimelineEvent::ConsoleFrame {
                frame: outcome.frame.clone(),
            });
        let accepted = accepted_from_frame(&outcome.frame);
        if scope.is_scoped() && claim.is_none() {
            // A concurrent identical send holds the claim; it finds this
            // frame and owns its dispatch.
            return Ok(ScopedReservation::Replay(accepted));
        }
        Ok(ScopedReservation::Fresh(accepted, claim))
    }

    async fn runtime_key_for_identity_first_send(
        &self,
        identity: &str,
    ) -> Result<(String, Arc<dyn ConsoleVisibilityPolicy>), ConsoleSendError> {
        let entries = self
            .inner
            .runtimes
            .read()
            .map_err(|_| ConsoleSendError::Log(runtime_registry_lock_error()))?
            .clone();
        if entries.is_empty() {
            let policy = capture_host_payload_policy(&self.inner, "identity-first", None)
                .map_err(ConsoleSendError::Log)?;
            return Ok(("identity-first".to_string(), policy));
        }
        let host_policies: BTreeMap<_, _> = entries
            .values()
            .map(|entry| {
                capture_host_payload_policy(
                    &self.inner,
                    &entry.runtime_key,
                    Some(entry.registration_id),
                )
                .map(|policy| (entry.runtime_key.clone(), policy))
            })
            .collect::<ConsoleLogResult<_>>()
            .map_err(ConsoleSendError::Log)?;

        let mut identity_runtime_keys = Vec::new();
        let mut hidden_durable_match = false;
        let mut durable_matches = Vec::new();
        for entry in entries.values() {
            let Some(identity_runtime) = entry.identity_runtime.as_ref() else {
                continue;
            };
            identity_runtime_keys.push(entry.runtime_key.clone());
            if requested_identity_is_runtime_member_alias(identity, &entry.identity_namespace) {
                continue;
            }
            for runtime_identity in namespace_match_candidates(identity, &entry.identity_namespace)
            {
                let Ok(parsed_identity) =
                    crate::identity_first::AgentIdentity::parse(&runtime_identity)
                else {
                    continue;
                };
                if let Ok(status) = identity_runtime.status(&parsed_identity).await {
                    let record = identity_record_for_status(entry, &status);
                    if !Box::pin(console_identity_record_visible(entry, &record)).await {
                        hidden_durable_match = true;
                        continue;
                    }
                    durable_matches.push((
                        entry.clone(),
                        identity_runtime.clone(),
                        parsed_identity,
                        record,
                    ));
                }
            }
        }
        if durable_matches.len() > 1 {
            let candidates = durable_matches
                .iter()
                .map(|(entry, _, _, record)| format!("{}@{}", record.identity, entry.runtime_key))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ConsoleSendError::InvalidRequest(format!(
                "ambiguous durable identity alias {identity}: candidates [{candidates}]"
            )));
        }
        if let Some((entry, _identity_runtime, _parsed_identity, record)) =
            durable_matches.into_iter().next()
        {
            let live_records = Box::pin(self.live_records_for_identity(identity)).await;
            let durable_live_records = Box::pin(self.live_records_for_durable_record(
                &entry,
                identity,
                &record,
                &live_records,
            ))
            .await;
            if let Some(stale_error) =
                stale_durable_record_error(identity, &record, &durable_live_records)
            {
                return Err(ConsoleSendError::InvalidRequest(stale_error));
            }
            return Ok((
                entry.runtime_key.clone(),
                host_policies[&entry.runtime_key].clone(),
            ));
        }
        if entries.values().any(|entry| {
            requested_identity_is_runtime_member_alias(identity, &entry.identity_namespace)
        }) {
            return Err(ConsoleSendError::UnknownIdentity(identity.to_string()));
        }
        if hidden_durable_match {
            return Err(ConsoleSendError::UnknownIdentity(identity.to_string()));
        }

        match identity_runtime_keys.as_slice() {
            [] => Err(ConsoleSendError::UnknownIdentity(identity.to_string())),
            [runtime_key] => Ok((runtime_key.clone(), host_policies[runtime_key].clone())),
            _ => Err(ConsoleSendError::InvalidRequest(format!(
                "identity-first send for '{identity}' did not match exactly one registered runtime"
            ))),
        }
    }

    pub async fn mark_interaction_delivery_failed(
        &self,
        input_frame_id: &str,
    ) -> Result<(), ConsoleSendError> {
        update_frame_status_and_emit(
            &self.inner,
            input_frame_id,
            ConsoleFrameStatus::DeliveryFailed,
        )
        .await
        .map_err(ConsoleSendError::Log)?;
        Ok(())
    }

    pub async fn mark_interaction_delivered(
        &self,
        input_frame_id: &str,
    ) -> Result<(), ConsoleSendError> {
        update_frame_status_and_emit(&self.inner, input_frame_id, ConsoleFrameStatus::Delivered)
            .await
            .map_err(ConsoleSendError::Log)?;
        Ok(())
    }

    pub async fn mark_steer_interaction_delivered(
        &self,
        input_frame_id: &str,
        interaction_id: &str,
    ) -> Result<(), ConsoleSendError> {
        let Some(updated) = update_frame_status_and_emit(
            &self.inner,
            input_frame_id,
            ConsoleFrameStatus::Delivered,
        )
        .await
        .map_err(ConsoleSendError::Log)?
        else {
            return Ok(());
        };
        append_steer_delivery_terminal(&self.inner, &updated, interaction_id)
            .await
            .map_err(ConsoleSendError::Log)?;
        Ok(())
    }

    pub async fn binary_blob_store_for_identity(
        &self,
        identity: &str,
    ) -> Result<Option<Arc<dyn BinaryBlobStore>>, ConsoleSendError> {
        if identity.trim().is_empty() {
            return Err(ConsoleSendError::InvalidRequest(
                "identity must be non-empty".to_string(),
            ));
        }
        let Some(resolved) = Box::pin(self.resolve_send_member(identity)).await? else {
            return Err(ConsoleSendError::UnknownIdentity(identity.to_string()));
        };
        let Some(record) = identity_record_for_resolved_member(&resolved).await else {
            return Err(ConsoleSendError::UnknownIdentity(identity.to_string()));
        };
        if !Box::pin(resolved_member_visible(&resolved, &record)).await {
            return Err(ConsoleSendError::UnknownIdentity(identity.to_string()));
        }
        if !member_is_addressable(&resolved.member) {
            return Err(ConsoleSendError::NotAddressable(identity.to_string()));
        }
        if resolved.member.status == meerkat_mob::MobMemberStatus::Retiring {
            return Err(ConsoleSendError::Retired(identity.to_string()));
        }
        Ok(resolved.entry.runtime.binary_blob_store())
    }

    pub fn binary_blob_stores(&self) -> Vec<Arc<dyn BinaryBlobStore>> {
        self.inner
            .runtimes
            .read()
            .map(|entries| {
                entries
                    .values()
                    .filter_map(|entry| entry.runtime.binary_blob_store())
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn resolve_send_member(
        &self,
        identity: &str,
    ) -> Result<Option<ResolvedConsoleMember>, ConsoleSendError> {
        let mut matches = Box::pin(self.resolve_visible_members(identity)).await;
        // Retiring generations remain visible for history and inspection, but
        // they must not make a durable send alias ambiguous when exactly one
        // active/addressable generation can receive the message. A durable
        // identity-runtime binding is authoritative over stale duplicate live
        // generations left behind by a respawn: select the exact bound runtime
        // member for sends while keeping inspection/control ambiguity strict.
        // Preserve the ambiguity error when multiple genuinely sendable
        // candidates have no single durable binding.
        let sendable_indices = sendable_resolved_member_indices(&matches);
        if sendable_indices.len() > 1 {
            if let Some(index) = Box::pin(authoritative_durable_sendable_member_index(
                identity,
                &matches,
                &sendable_indices,
            ))
            .await?
            {
                let resolved = matches.swap_remove(index);
                if let Some(record) = identity_record_for_resolved_member(&resolved).await
                    && let Some(stale_error) =
                        stale_live_record_binding_error(&resolved.entry, identity, &record).await
                {
                    return Err(ConsoleSendError::InvalidRequest(stale_error));
                }
                if let Some(stale_error) =
                    Box::pin(self.durable_send_binding_error(identity)).await?
                {
                    return Err(ConsoleSendError::InvalidRequest(stale_error));
                }
                return Ok(Some(resolved));
            }
            let candidates = sendable_indices
                .iter()
                .map(|index| matches[*index].runtime_identity.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ConsoleSendError::AmbiguousIdentity {
                identity: identity.to_string(),
                candidates,
            });
        }
        if let Some(index) = sendable_indices.into_iter().next() {
            let resolved = matches.swap_remove(index);
            if let Some(record) = identity_record_for_resolved_member(&resolved).await
                && let Some(stale_error) =
                    stale_live_record_binding_error(&resolved.entry, identity, &record).await
            {
                return Err(ConsoleSendError::InvalidRequest(stale_error));
            }
            if let Some(stale_error) = Box::pin(self.durable_send_binding_error(identity)).await? {
                return Err(ConsoleSendError::InvalidRequest(stale_error));
            }
            return Ok(Some(resolved));
        }
        if matches.len() > 1 {
            let candidates = matches
                .iter()
                .map(|resolved| resolved.runtime_identity.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ConsoleSendError::AmbiguousIdentity {
                identity: identity.to_string(),
                candidates,
            });
        }
        let Some(resolved) = matches.into_iter().next() else {
            if let Some(stale_error) = Box::pin(self.durable_send_binding_error(identity)).await? {
                return Err(ConsoleSendError::InvalidRequest(stale_error));
            }
            return Ok(None);
        };
        if let Some(record) = identity_record_for_resolved_member(&resolved).await
            && let Some(stale_error) =
                stale_live_record_binding_error(&resolved.entry, identity, &record).await
        {
            return Err(ConsoleSendError::InvalidRequest(stale_error));
        }
        if let Some(stale_error) = Box::pin(self.durable_send_binding_error(identity)).await? {
            return Err(ConsoleSendError::InvalidRequest(stale_error));
        }
        Ok(Some(resolved))
    }

    async fn durable_send_binding_error(
        &self,
        identity: &str,
    ) -> Result<Option<String>, ConsoleSendError> {
        let entries = self
            .inner
            .runtimes
            .read()
            .map_err(|_| ConsoleSendError::Log(runtime_registry_lock_error()))?
            .clone();
        if entries.values().any(|entry| {
            requested_identity_is_runtime_member_alias(identity, &entry.identity_namespace)
        }) {
            return Ok(None);
        }
        let live_records = Box::pin(self.live_records_for_identity(identity)).await;
        let mut durable_matches = Vec::new();
        for entry in entries.values() {
            let Some(identity_runtime) = entry.identity_runtime.clone() else {
                continue;
            };
            for raw_identity in namespace_match_candidates(identity, &entry.identity_namespace) {
                let Ok(parsed_identity) =
                    crate::identity_first::AgentIdentity::parse(&raw_identity)
                else {
                    continue;
                };
                let Ok(status) = identity_runtime.status(&parsed_identity).await else {
                    continue;
                };
                let record = identity_record_for_status(entry, &status);
                if !Box::pin(console_identity_record_visible(entry, &record)).await {
                    continue;
                }
                durable_matches.push((
                    entry.clone(),
                    identity_runtime.clone(),
                    parsed_identity,
                    record,
                ));
            }
        }
        if durable_matches.len() > 1 {
            let candidates = durable_matches
                .iter()
                .map(|(_, _, _, record)| record.runtime_member_id.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ConsoleSendError::AmbiguousIdentity {
                identity: identity.to_string(),
                candidates,
            });
        }
        let Some((entry, _identity_runtime, _parsed_identity, durable_record)) =
            durable_matches.into_iter().next()
        else {
            return Ok(None);
        };
        let durable_live_records = Box::pin(self.live_records_for_durable_record(
            &entry,
            identity,
            &durable_record,
            &live_records,
        ))
        .await;
        Ok(stale_durable_record_error(
            identity,
            &durable_record,
            &durable_live_records,
        ))
    }

    async fn resolve_members(&self, identity: &str) -> Vec<ResolvedConsoleMember> {
        let entries = self
            .inner
            .runtimes
            .read()
            .ok()
            .map(|entries| entries.clone())
            .unwrap_or_default();
        let mut exact_matches: Vec<(String, ResolvedConsoleMember)> = Vec::new();
        let mut label_matches: Vec<(String, ResolvedConsoleMember)> = Vec::new();
        for entry in entries.values() {
            // Member-id alias rule: candidates are decoded first, so an encoded
            // roster id is not encoded a second time.
            let raw_identities = namespace_match_candidates(identity, &entry.identity_namespace)
                .into_iter()
                .map(|raw| crate::member_comms_id::runtime_alias_str(&raw).into_owned())
                .collect::<Vec<_>>();
            if raw_identities.is_empty() {
                continue;
            }
            let mut mids = raw_identities
                .iter()
                .map(|raw_identity| crate::member_comms_id::mob_member_id(raw_identity.as_str()))
                .collect::<Vec<_>>();
            // A generated alias names the durable roster row only when the
            // entry's identity authority confirms it is the CURRENT
            // incarnation; a stale generation adds nothing. Retire re-checks
            // that currency under the identity's lifecycle lock.
            if let Some(identity_runtime) = entry.identity_runtime.as_ref() {
                for raw_identity in &raw_identities {
                    if let Some(durable) =
                        current_generated_alias_identity(identity_runtime, raw_identity).await
                    {
                        mids.push(crate::member_comms_id::roster_member_id_for_identity(
                            durable.as_str(),
                        ));
                    }
                }
            }
            for resolved in Box::pin(member_sources_for_entry(entry)).await {
                let session_id = resolved
                    .handle
                    .resolve_bridge_session_id_observation(&resolved.member.agent_identity)
                    .await
                    .map(|sid| sid.to_string())
                    .unwrap_or_default();
                if mids.contains(&resolved.member.agent_identity) {
                    exact_matches.push((session_id, resolved));
                } else if resolved_member_matches_raw_identities(&resolved, &raw_identities, &mids)
                {
                    label_matches.push((session_id, resolved));
                }
            }
        }
        let mut matches = exact_matches;
        matches.extend(label_matches);
        let mut seen_members = BTreeSet::new();
        matches.retain(|(session_id, resolved)| {
            seen_members.insert((
                resolved.entry.runtime_key.clone(),
                resolved.source_mob_id.clone(),
                session_id.clone(),
                resolved.runtime_identity.clone(),
            ))
        });
        matches.sort_by(|left, right| right.0.cmp(&left.0));
        matches.into_iter().map(|(_, resolved)| resolved).collect()
    }

    async fn resolve_visible_members(&self, identity: &str) -> Vec<ResolvedConsoleMember> {
        let mut visible = Vec::new();
        for resolved in Box::pin(self.resolve_members(identity)).await {
            let Some(record) = identity_record_for_resolved_member(&resolved).await else {
                continue;
            };
            if Box::pin(resolved_member_visible(&resolved, &record)).await {
                visible.push(resolved);
            }
        }
        visible
    }

    async fn live_records_for_identity(&self, identity: &str) -> Vec<ConsoleIdentityRecord> {
        let mut records = Vec::new();
        for resolved in Box::pin(self.resolve_members(identity)).await {
            let Some(record) = identity_record_for_resolved_member(&resolved).await else {
                continue;
            };
            if Box::pin(resolved_member_visible(&resolved, &record)).await {
                records.push(record);
            }
        }
        records
    }

    async fn live_records_for_durable_record(
        &self,
        entry: &RuntimeEntry,
        requested_identity: &str,
        durable: &ConsoleIdentityRecord,
        requested_live_records: &[ConsoleIdentityRecord],
    ) -> Vec<ConsoleIdentityRecord> {
        let mut records = if durable.identity == requested_identity {
            requested_live_records.to_vec()
        } else {
            Box::pin(self.live_records_for_identity(&durable.identity)).await
        };
        for runtime_record in Box::pin(live_records_for_runtime_member(
            entry,
            &durable.runtime_member_id,
        ))
        .await
        {
            if records.iter().any(|record| {
                record.runtime_key == runtime_record.runtime_key
                    && record.runtime_member_id == runtime_record.runtime_member_id
                    && record.session_id == runtime_record.session_id
                    && record.identity == runtime_record.identity
            }) {
                continue;
            }
            records.push(runtime_record);
        }
        records
    }
}

async fn console_identity_record_visible(
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
) -> bool {
    if !entry.visibility_policy.identity_visible(record) {
        return false;
    }
    let mut bound_live_member_seen = false;
    let mut wrong_live_projection_seen = false;
    for resolved in Box::pin(member_sources_for_entry_including_hidden(entry)).await {
        // Associate the live row with this durable record by the approved
        // predicate, not by exact equality with the record's runtime_member_id.
        // That field holds the BINDING (an incarnation), while the row is now the
        // encoded durable identity, so exact equality never matched and a hidden
        // row stopped affecting its own record's visibility - which is how a
        // hidden member stayed inspectable.
        if !crate::member_comms_id::live_member_names_identity_or_incarnation(
            &resolved.runtime_identity,
            &record.identity,
            Some(record.runtime_member_id.as_str()),
        ) {
            continue;
        }
        bound_live_member_seen = true;
        let Some(live_record) = identity_record_for_resolved_member(&resolved).await else {
            continue;
        };
        if live_record.identity != record.identity {
            wrong_live_projection_seen = true;
            continue;
        }
        if raw_resolved_member_visible(&resolved, &live_record) {
            return true;
        }
    }
    if wrong_live_projection_seen {
        return true;
    }
    !bound_live_member_seen
}

async fn live_record_shadowed_by_hidden_durable_binding(
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> bool {
    let Some(identity_runtime) = resolved.entry.identity_runtime.as_ref() else {
        return false;
    };
    let Ok(identity) = crate::identity_first::AgentIdentity::parse(&record.identity) else {
        return false;
    };
    let Ok(status) = identity_runtime.status(&identity).await else {
        return false;
    };
    // A binding must exist for this status to shadow anything.
    if status.agent_runtime_id.is_none() {
        return false;
    }
    // Identity-level: `resolved.runtime_identity` is the DECODED roster member
    // id, which since the stable-identity lowering is the bare durable
    // identity.
    if crate::member_comms_id::live_member_is_identity(
        &resolved.runtime_identity,
        status.identity.as_str(),
    ) {
        return false;
    }
    for candidate in Box::pin(member_sources_for_entry(&resolved.entry)).await {
        // Same identity-level rule: find the candidate whose roster identity IS
        // this status's durable identity, not one whose spelling matches an
        // AgentRuntimeId.
        if !crate::member_comms_id::live_member_is_identity(
            &candidate.runtime_identity,
            status.identity.as_str(),
        ) {
            continue;
        }
        let Some(bound_record) = identity_record_for_resolved_member(&candidate).await else {
            continue;
        };
        if bound_record.identity == record.identity
            && !raw_resolved_member_visible(&candidate, &bound_record)
        {
            return true;
        }
    }
    false
}

async fn frame_matches_hidden_member(entry: &RuntimeEntry, frame: &ConsoleFrame) -> bool {
    let runtime_member_id = strip_namespace(&frame.identity, &entry.identity_namespace)
        .unwrap_or_else(|| frame.identity.clone());
    let frame_session_id = frame.session_id.as_deref();
    let mut saw_hidden = false;
    let mut saw_visible = false;
    for resolved in Box::pin(member_sources_for_entry_including_hidden(entry)).await {
        let Some(record) = identity_record_for_resolved_member(&resolved).await else {
            continue;
        };
        let session_matches =
            frame_session_id.is_some() && record.session_id.as_deref() == frame_session_id;
        let runtime_matches = record.runtime_member_id == runtime_member_id;
        let identity_matches = record.identity == frame.identity;
        if !session_matches && !runtime_matches && !identity_matches {
            continue;
        }
        if session_matches && !raw_resolved_member_visible(&resolved, &record) {
            return true;
        }
        if Box::pin(resolved_member_visible(&resolved, &record)).await {
            saw_visible = true;
        } else {
            saw_hidden = true;
        }
    }
    saw_hidden && !saw_visible
}

async fn live_records_for_runtime_member(
    entry: &RuntimeEntry,
    runtime_member_id: &str,
) -> Vec<ConsoleIdentityRecord> {
    let mut records = Vec::new();
    for resolved in Box::pin(member_sources_for_entry(entry)).await {
        if resolved.runtime_identity != runtime_member_id {
            continue;
        }
        let Some(record) = identity_record_for_resolved_member(&resolved).await else {
            continue;
        };
        records.push(record);
    }
    records
}

async fn visible_live_records_for_entry(
    entry: &RuntimeEntry,
    records: &[ConsoleIdentityRecord],
) -> Vec<ConsoleIdentityRecord> {
    let mut visible = Vec::new();
    for record in records {
        if record.runtime_key != entry.runtime_key
            || Box::pin(console_identity_record_visible(entry, record)).await
        {
            visible.push(record.clone());
        }
    }
    visible
}

/// A live console record that names an identity whose Core binding moved
///: the identity's status and the live member are both Core's, so a
/// mismatch is a binding that changed between the two reads. Reads and the
/// delivery/retire write paths report it alike; nothing is rebound, and the
/// caller retries against the binding Core holds now.
async fn stale_live_record_binding_error(
    entry: &RuntimeEntry,
    requested_identity: &str,
    live_record: &ConsoleIdentityRecord,
) -> Option<String> {
    let identity_runtime = entry.identity_runtime.as_ref()?;
    // No binding can be vouched for without Core's member history: refuse
    // rather than act on a record nobody checked.
    let statuses = match identity_runtime.statuses().await {
        Ok(statuses) => statuses,
        Err(error) => return Some(error.to_string()),
    };
    for status in statuses {
        // Identity-level, and a binding must exist. Comparing an
        // AgentRuntimeId spelling against a stable roster id never matched, so
        // this loop skipped every record and the stale-binding paths went
        // silent.
        let matches_runtime = status.agent_runtime_id.is_some()
            && crate::member_comms_id::live_member_is_identity(
                &live_record.runtime_member_id,
                status.identity.as_str(),
            );
        if !matches_runtime {
            continue;
        }
        // `stale_durable_record_error` already covers the session-mismatch
        // case the reconciling form heals, so the read path needs no separate
        // message for it.
        let durable_record = identity_record_for_status(entry, &status);
        if let Some(stale_error) = stale_durable_record_error(
            requested_identity,
            &durable_record,
            std::slice::from_ref(live_record),
        ) {
            return Some(stale_error);
        }
    }
    None
}

fn stale_durable_session_mismatch<'a>(
    durable: &ConsoleIdentityRecord,
    live_records: &'a [ConsoleIdentityRecord],
) -> Option<&'a ConsoleIdentityRecord> {
    live_records.iter().find(|record| {
        live_record_realizes_durable_binding(durable, record)
            && durable.session_id.is_some()
            && record.session_id.is_some()
            && durable.session_id != record.session_id
    })
}

/// Whether a live roster row is the bridge-authorized realization of a
/// durable identity binding.
///
/// Session-backed identities use their generated `rt:<identity>:<generation>`
/// alias directly. External peer bindings are intentionally different: the
/// Meerkat bridge gives the peer a stable durable roster name and records the
/// durable owner in the runtime-authoritative `agent_identity` label. Raw
/// member creation cannot supply that label, so accepting this exact mapping
/// preserves stale-generation fencing without mistaking the external member
/// for an unrelated raw alias.
fn live_record_realizes_durable_binding(
    durable: &ConsoleIdentityRecord,
    live: &ConsoleIdentityRecord,
) -> bool {
    if live.identity != durable.identity {
        return false;
    }
    if live.runtime_member_id == durable.runtime_member_id {
        return true;
    }
    // The row named by the identity realizes the binding. This already intended
    // that, but compared an ENCODED roster member id against a decoded label
    // value, so it never matched once the roster row became the encoded durable
    // identity. Decode before comparing.
    crate::member_comms_id::durable_identity_label(&live.labels).is_some_and(|identity| {
        crate::member_comms_id::live_member_is_identity(&live.runtime_member_id, identity)
    })
}

fn stale_durable_record_error(
    requested_identity: &str,
    durable: &ConsoleIdentityRecord,
    live_records: &[ConsoleIdentityRecord],
) -> Option<String> {
    let matching_live = live_records
        .iter()
        .filter(|record| record.identity == durable.identity)
        .collect::<Vec<_>>();
    if let Some(wrong_projection) = live_records.iter().find(|record| {
        record.runtime_member_id == durable.runtime_member_id && record.identity != durable.identity
    }) {
        return Some(format!(
            "stale durable identity alias {requested_identity}: identity runtime binding for {} points at {}, but live console alias projects identity {}",
            durable.identity, durable.runtime_member_id, wrong_projection.identity
        ));
    }
    if matching_live.is_empty() {
        return None;
    }
    if let Some(session_mismatch) = stale_durable_session_mismatch(durable, live_records) {
        return Some(format!(
            "stale durable identity alias {requested_identity}: identity runtime binding for {} points at {} session {}, but live console alias resolves to session {}",
            durable.identity,
            durable.runtime_member_id,
            durable.session_id.as_deref().unwrap_or("<none>"),
            session_mismatch.session_id.as_deref().unwrap_or("<none>")
        ));
    }
    if matching_live
        .iter()
        .any(|record| live_record_realizes_durable_binding(durable, record))
    {
        return None;
    }
    let live_candidates = matching_live
        .iter()
        .map(|record| record.runtime_member_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "stale durable identity alias {requested_identity}: identity runtime binding for {} points at {}, but live console alias resolves to [{}]",
        durable.identity, durable.runtime_member_id, live_candidates
    ))
}

fn ambiguous_live_alias_error(
    requested_identity: &str,
    live_records: &[ConsoleIdentityRecord],
) -> Option<String> {
    if live_records.len() <= 1 {
        return None;
    }
    // The identity's OWN row disambiguates. Since the stable-identity lowering a
    // durable identity has exactly one roster row of its own, so when that row is
    // among the candidates it IS the answer and rows that merely carry the same
    // agent_identity label are not competitors. Without this, any labelled decoy
    // made a healthy identity unresolvable through every aggregator control.
    let own_rows = live_records
        .iter()
        .filter(|record| {
            crate::member_comms_id::live_member_is_identity(
                &record.runtime_member_id,
                requested_identity,
            )
        })
        .count();
    if own_rows == 1 {
        return None;
    }
    let candidates = live_records
        .iter()
        .map(|record| {
            format!(
                "{}@{}",
                record.runtime_member_id,
                record
                    .labels
                    .get("source_mob_id")
                    .map(String::as_str)
                    .unwrap_or(record.runtime_key.as_str())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "ambiguous live identity alias [via aggregator-resolve] {requested_identity}: candidates [{candidates}]"
    ))
}

#[cfg(test)]
fn member_id_matches_durable_identity(member_id: &str, durable_identity: &str) -> bool {
    // Roster ids are comms-safe encodings of public aliases (meerkat 0.7
    // MemberCommsName); compare in the public alias space.
    crate::member_comms_id::runtime_alias_str(member_id) == durable_identity
}

fn generated_runtime_alias_parts(alias: &str) -> Option<(&str, u64)> {
    let rest = alias.strip_prefix("rt:")?;
    let (identity, generation) = rest.rsplit_once(':')?;
    Some((identity, generation.parse().ok()?))
}

async fn stale_generated_members_for_runtime_entry(
    entry: &RuntimeEntry,
    current_runtime_alias: &str,
    include_current: bool,
) -> Vec<(MobHandle, meerkat_mob::ids::AgentIdentity)> {
    let Some((current_identity, current_generation)) =
        generated_runtime_alias_parts(current_runtime_alias)
    else {
        return Vec::new();
    };
    let primary_handle = entry.runtime.handle();
    let primary_mob_id = primary_handle.mob_id().to_string();
    let mut handles = vec![(primary_mob_id.clone(), primary_handle)];
    if let Some(state) = entry.runtime.agent_mob_mcp_state() {
        for (mob_id, handle) in Box::pin(state.mob_handles_snapshot())
            .await
            .unwrap_or_default()
        {
            if mob_id.as_str() != primary_mob_id {
                handles.push((mob_id.to_string(), handle));
            }
        }
    }
    let mut seen_handles = BTreeSet::new();
    let mut stale_members = Vec::new();
    for (mob_id, handle) in handles {
        if !seen_handles.insert(mob_id) {
            continue;
        }
        for member in handle.list_members_including_retiring().await {
            let public_alias =
                crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str());
            if generated_runtime_alias_parts(public_alias.as_ref()).is_some_and(
                |(identity, generation)| {
                    identity == current_identity
                        && (generation < current_generation
                            || (include_current && generation == current_generation))
                },
            ) {
                stale_members.push((handle.clone(), member.agent_identity));
            }
        }
    }
    stale_members
}

async fn retire_captured_console_members(
    stale_members: Vec<(MobHandle, meerkat_mob::ids::AgentIdentity)>,
) -> Result<(), String> {
    for (handle, member_id) in stale_members {
        match handle.retire(member_id).await {
            Ok(()) => {}
            Err(err) if lifecycle_archive_cleanup_completed(&err.to_string()) => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(())
}

fn lifecycle_archive_cleanup_completed(error: &str) -> bool {
    is_recoverable_lifecycle_cleanup_error(error)
}

fn explicit_identity_query_needs_session_history_backfill(frames: &[ConsoleFrame]) -> bool {
    if frames.is_empty() {
        return true;
    }
    let latest_session_history_timestamp_ms = frames
        .iter()
        .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::SessionHistory)
        .map(|frame| frame.timestamp_ms)
        .max();
    if let Some(latest_timestamp_ms) = latest_session_history_timestamp_ms {
        return current_time_ms().saturating_sub(latest_timestamp_ms)
            >= SESSION_HISTORY_GROWING_REFRESH_TTL_MS;
    }
    frames.iter().any(|frame| {
        matches!(
            frame.kind.as_str(),
            "turn_started"
                | "run_started"
                | "reasoning_delta"
                | "reasoning_complete"
                | "tool_call_requested"
                | "tool_call"
                | "tool_execution_started"
                | "tool_execution_completed"
                | "tool_result_received"
        )
    })
}

fn dedupe_identity_records(records: Vec<ConsoleIdentityRecord>) -> Vec<ConsoleIdentityRecord> {
    // Since the roster lowering a live roster row's member id is its bare
    // durable identity, while the identity's durable row names the generated
    // binding `rt:{identity}:{generation}`. Decode before comparing, as
    // `live_record_realizes_durable_binding` does: a roster row named by its
    // identity realizes the one generated binding of that identity in the
    // same runtime, source mob and session, and lists under that binding's
    // public alias. Different generations never merge.
    let mut bindings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for record in &records {
        if let Some(identity) =
            crate::member_comms_id::durable_identity_from_runtime_alias(&record.runtime_member_id)
        {
            bindings
                .entry(identity_record_dedupe_key(record, &identity))
                .or_default()
                .insert(record.runtime_member_id.clone());
        }
    }
    let mut by_identity: BTreeMap<String, ConsoleIdentityRecord> = BTreeMap::new();
    for mut record in records {
        let realized = crate::member_comms_id::durable_identity_label(&record.labels)
            .filter(|identity| {
                crate::member_comms_id::live_member_is_identity(&record.runtime_member_id, identity)
            })
            .and_then(|identity| bindings.get(&identity_record_dedupe_key(&record, identity)))
            .filter(|aliases| aliases.len() == 1)
            .and_then(|aliases| aliases.first().cloned());
        if let Some(alias) = realized {
            record.runtime_member_id = alias;
        }
        by_identity
            .entry(identity_record_dedupe_key(
                &record,
                &record.runtime_member_id,
            ))
            .and_modify(|current| {
                let member_id = current
                    .member_id
                    .clone()
                    .or_else(|| record.member_id.clone());
                if identity_record_prefer(&record, current) {
                    *current = record.clone();
                }
                // Core's member id of the binding comes from the durable row;
                // the live roster row of the same binding, which the list
                // prefers, carries none, so the merged row keeps it.
                if current.member_id.is_none() {
                    current.member_id = member_id;
                }
            })
            .or_insert(record);
    }
    by_identity.into_values().collect()
}

fn identity_record_dedupe_key(record: &ConsoleIdentityRecord, runtime_member_id: &str) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        record.identity,
        record.runtime_key,
        record
            .labels
            .get("source_mob_id")
            .map(String::as_str)
            .unwrap_or(""),
        runtime_member_id,
        record.session_id.as_deref().unwrap_or("")
    )
}

fn identity_record_prefer(
    candidate: &ConsoleIdentityRecord,
    current: &ConsoleIdentityRecord,
) -> bool {
    let candidate_is_live_label_projection =
        crate::member_comms_id::durable_identity_label(&candidate.labels).is_some_and(|identity| {
            identity == strip_namespace(&candidate.identity, "").unwrap_or_default()
                || candidate.runtime_member_id != candidate.identity
        });
    let current_is_live_label_projection =
        crate::member_comms_id::durable_identity_label(&current.labels).is_some_and(|identity| {
            identity == strip_namespace(&current.identity, "").unwrap_or_default()
                || current.runtime_member_id != current.identity
        });
    if candidate_is_live_label_projection != current_is_live_label_projection {
        return candidate_is_live_label_projection;
    }
    let candidate_has_distinct_runtime_binding = candidate.runtime_member_id != candidate.identity;
    let current_has_distinct_runtime_binding = current.runtime_member_id != current.identity;
    if candidate_has_distinct_runtime_binding != current_has_distinct_runtime_binding {
        return candidate_has_distinct_runtime_binding;
    }
    let candidate_live = candidate.addressable && candidate.health != "retired";
    let current_live = current.addressable && current.health != "retired";
    if candidate_live != current_live {
        return candidate_live;
    }
    candidate.session_id.as_deref().unwrap_or("") > current.session_id.as_deref().unwrap_or("")
}

async fn collect_identity_records(
    inner: &Arc<AggregatorInner>,
    mode: IdentityCollectionMode,
) -> ConsoleLogResult<Vec<ConsoleIdentityRecord>> {
    let entries = inner
        .runtimes
        .read()
        .map_err(|_| runtime_registry_lock_error())?
        .clone();
    let mut identities = Vec::new();
    for entry in entries.values() {
        let mut identity_display_names = BTreeMap::new();
        if let Some(identity_runtime) = entry.identity_runtime.as_ref() {
            let topology_peers =
                identity_runtime_topology_peers(entry, identity_runtime.as_ref()).await;
            for status in identity_runtime.statuses().await? {
                let mut record = identity_record_for_status(entry, &status);
                record.topology_peers = topology_peers
                    .get(&record.identity)
                    .cloned()
                    .unwrap_or_default();
                if let (Some(session), Some(name)) = (&record.session_id, &status.display_name) {
                    identity_display_names.insert(
                        record.identity.clone(),
                        (session.clone(), name.as_str().to_owned()),
                    );
                }
                if Box::pin(console_identity_record_visible(entry, &record)).await {
                    identities.push(record);
                }
            }
            if mode == IdentityCollectionMode::CachedOnly {
                continue;
            }
        }
        let members = if entry.identity_runtime.is_some() {
            match Box::pin(tokio::time::timeout(
                IDENTITY_FIRST_LIVE_MEMBER_REFRESH_WAIT,
                member_sources_for_entry(entry),
            ))
            .await
            {
                Ok(members) => members,
                Err(_) => {
                    tracing::warn!(
                        runtime_key = %entry.runtime_key,
                        "identity-first live member refresh timed out; keeping cached identity read model"
                    );
                    Vec::new()
                }
            }
        } else {
            Box::pin(member_sources_for_entry(entry)).await
        };
        for resolved in members {
            if let Some(mut record) = identity_record_for_resolved_member(&resolved).await {
                record.topology_peers = resolved
                    .member
                    .wired_to
                    .iter()
                    .map(|peer| {
                        project_topology_peer(
                            peer.as_str(),
                            &entry.identity_namespace,
                            Some(&entries),
                        )
                    })
                    .collect();
                // Live binding and wiring come from the mob; display names come
                // from its matching durable identity, never another session.
                if let Some((session, name)) = identity_display_names.get(&record.identity)
                    && record.session_id.as_ref() == Some(session)
                {
                    record.display_name.clone_from(name);
                }
                Box::pin(capture_member_provenance(inner, &resolved, &record)).await;
                if Box::pin(resolved_member_visible(&resolved, &record)).await {
                    identities.push(record);
                }
            }
        }
    }
    Ok(dedupe_identity_records(identities))
}

// A local roster ID and an external canonical comms name are different
// contracts. Resolve the latter through its registered mob, never by an
// unscoped member-name search that could connect two repositories' coders.
fn project_topology_peer(
    peer: &str,
    local_namespace: &str,
    entries: Option<&BTreeMap<String, RuntimeEntry>>,
) -> String {
    let Ok(external) = peer.parse::<meerkat_core::connection::MemberCommsName>() else {
        return apply_namespace(
            &crate::member_comms_id::runtime_alias_str(peer),
            local_namespace,
        );
    };
    if let Some(entries) = entries {
        let mut matches = entries
            .values()
            .filter(|entry| entry.runtime.handle().mob_id().as_str() == external.mob_id());
        if let Some(target) = matches.next()
            && matches.next().is_none()
        {
            return apply_namespace(
                &crate::member_comms_id::runtime_alias_str(external.member()),
                &target.identity_namespace,
            );
        }
    }
    // An unregistered or ambiguously registered remote stays unresolved.
    peer.to_owned()
}

async fn identity_runtime_topology_peers(
    entry: &RuntimeEntry,
    identity_runtime: &crate::identity_first::IdentityRuntime,
) -> BTreeMap<String, Vec<String>> {
    let mut peers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in identity_runtime.desired_peer_edges().await {
        let a = apply_namespace(edge.a().as_str(), &entry.identity_namespace);
        let b = apply_namespace(edge.b().as_str(), &entry.identity_namespace);
        peers.entry(a.clone()).or_default().insert(b.clone());
        peers.entry(b).or_default().insert(a);
    }
    peers
        .into_iter()
        .map(|(identity, peers)| (identity, peers.into_iter().collect()))
        .collect()
}

fn spawn_identity_backfills_for_records(
    inner: Arc<AggregatorInner>,
    records: &[ConsoleIdentityRecord],
) {
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    let entries = match inner.runtimes.read() {
        Ok(entries) => entries.clone(),
        Err(_) => return,
    };
    for record in records {
        if record.health == "dormant" || record.health == "uninitialized" {
            continue;
        }
        let Some(entry) = entries.get(&record.runtime_key).cloned() else {
            continue;
        };
        let Some(session_id) = record.session_id.clone() else {
            continue;
        };
        spawn_session_history_backfill_target(
            inner.clone(),
            SessionBackfillTarget {
                assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
                provenance: None,
                entry,
                record: record.clone(),
                session_id,
            },
            false,
        );
    }
}

async fn member_sources_for_entry(entry: &RuntimeEntry) -> Vec<ResolvedConsoleMember> {
    member_sources_for_entry_with_hidden(entry, false).await
}

async fn member_sources_for_entry_including_hidden(
    entry: &RuntimeEntry,
) -> Vec<ResolvedConsoleMember> {
    member_sources_for_entry_with_hidden(entry, true).await
}

async fn member_sources_for_entry_with_hidden(
    entry: &RuntimeEntry,
    include_hidden: bool,
) -> Vec<ResolvedConsoleMember> {
    let mut resolved = Vec::new();
    let primary_handle = entry.runtime.handle();
    let primary_mob_id = primary_handle.mob_id().to_string();
    for member in primary_handle.list_members_observation_snapshot().await {
        resolved.push(ResolvedConsoleMember {
            entry: entry.clone(),
            handle: primary_handle.clone(),
            // Roster ids are comms-safe encodings (meerkat 0.7); the
            // aggregator's runtime identity is the public alias.
            runtime_identity: crate::member_comms_id::runtime_alias_str(
                member.agent_identity.as_str(),
            )
            .into_owned(),
            source_mob_id: primary_mob_id.clone(),
            member,
        });
    }

    let Some(state) = entry.runtime.agent_mob_mcp_state() else {
        return resolved;
    };
    if !include_hidden && !entry.visibility_policy.include_implicit_delegate_members() {
        return resolved;
    }
    for (mob_id, handle) in Box::pin(state.mob_handles_snapshot())
        .await
        .unwrap_or_default()
    {
        if mob_id.as_str() == primary_mob_id {
            continue;
        }
        for member in handle.list_members_observation_snapshot().await {
            resolved.push(ResolvedConsoleMember {
                entry: entry.clone(),
                handle: handle.clone(),
                // Roster ids are comms-safe encodings (meerkat 0.7); the
                // aggregator's runtime identity is the public alias.
                runtime_identity: crate::member_comms_id::runtime_alias_str(
                    member.agent_identity.as_str(),
                )
                .into_owned(),
                source_mob_id: mob_id.to_string(),
                member,
            });
        }
    }
    resolved
}

async fn dispatch_message_to_resolved_member(
    resolved: &ResolvedConsoleMember,
    content: ContentInput,
    skill_references: &[meerkat_core::skills::SkillKey],
    handling_mode: meerkat_core::types::HandlingMode,
    accepted: &ConsoleInteractionAccepted,
) -> Result<String, ConsoleSendError> {
    if let Some(identity_runtime) = resolved.entry.identity_runtime.as_ref()
        && let Some(identity) = identity_runtime
            .identity_for_member_mutation(&resolved.runtime_identity)
            .await
    {
        identity_runtime
            .send_console_human_input_tracked(
                &identity,
                Some(&resolved.runtime_identity),
                &content,
                handling_mode,
                skill_references,
                accepted,
                None,
            )
            .await
            .map_err(|error| ConsoleSendError::RuntimeOperation(Box::new(error)))?;
        return identity_runtime
            .status(&identity)
            .await
            .map_err(|error| ConsoleSendError::RuntimeOperation(Box::new(error)))?
            .session_id
            .map(|session_id| session_id.to_string())
            .ok_or(ConsoleSendError::State(
                "identity has no bridge session after delivery",
            ));
    }
    crate::mob_handle_runtime::send_console_human_on_mob(
        &resolved.handle,
        &resolved.member,
        content,
        skill_references,
        handling_mode,
        accepted,
    )
    .await
    .map_err(|err| match err {
        crate::mob_handle_runtime::MobRuntimeError::Mob(error) => {
            ConsoleSendError::RuntimeOperation(Box::new(
                crate::identity_first::IdentityRuntimeError::HostHumanInput(
                    crate::identity_first::bridge::HostHumanInputError::Mob(Box::new(error)),
                ),
            ))
        }
        crate::mob_handle_runtime::MobRuntimeError::HostHumanInput(error) => {
            ConsoleSendError::RuntimeOperation(Box::new(
                crate::identity_first::IdentityRuntimeError::HostHumanInput(error),
            ))
        }
        error => ConsoleSendError::Dispatch(error.to_string()),
    })
}

fn spawn_console_send_dispatch(
    inner: Arc<AggregatorInner>,
    producer: (ResolvedConsoleMember, Arc<dyn ConsoleVisibilityPolicy>),
    turn: (ContentInput, Vec<meerkat_core::skills::SkillKey>),
    handling_mode: meerkat_core::types::HandlingMode,
    dispatching: SendState,
    owned: (ConsoleFrame, Option<SendDispatchClaim>),
    interaction_id: String,
) {
    let (resolved, host_policy) = producer;
    let (content, skill_references) = turn;
    let (user_frame, claim) = owned;
    tokio::spawn(async move {
        // The claim names this task the frame's dispatch owner until it
        // settles the frame (dropped at the end of the task).
        let _claim = claim;
        match dispatch_message_to_resolved_member(
            &resolved,
            content,
            &skill_references,
            handling_mode,
            &accepted_from_frame(&user_frame),
        )
        .await
        {
            Ok(_) => {
                faults::after_console_admission();
                let _ = dispatching.apply(SendTransition::MarkDelivered);
                if let Err(err) = update_frame_status_and_emit(
                    &inner,
                    &user_frame.id,
                    ConsoleFrameStatus::Delivered,
                )
                .await
                {
                    tracing::warn!(
                        frame_id = %user_frame.id,
                        error = %err,
                        "failed to update console send delivery status"
                    );
                }
                if handling_mode == HandlingMode::Steer
                    && let Err(err) =
                        append_steer_delivery_terminal(&inner, &user_frame, &interaction_id).await
                {
                    tracing::warn!(
                        frame_id = %user_frame.id,
                        interaction_id = %interaction_id,
                        error = %err,
                        "failed to append console steer terminal frame"
                    );
                }
            }
            Err(err) => {
                let _ = dispatching.apply(SendTransition::MarkDeliveryFailed);
                // The typed failure frame commits before the input frame turns
                // `delivery_failed`: anyone who observes that status (a stream
                // subscriber, a timeline query) can already read why.
                let failure_frame = NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("delivery-failed:{}", user_frame.id),
                    timestamp_ms: current_time_ms(),
                    runtime_key: user_frame.runtime_key.clone(),
                    identity: user_frame.identity.clone(),
                    conversation_id: user_frame.conversation_id.clone(),
                    session_id: user_frame.session_id.clone(),
                    kind: "message_delivery_failed".to_string(),
                    status: ConsoleFrameStatus::DeliveryFailed,
                    payload: json!({ "reason": err.to_string(), "data": err.structured_data() }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::Synthetic,
                        source_cursor: None,
                    },
                    source_event_id: None,
                    interaction_id: Some(interaction_id),
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: Some(user_frame.id.clone()),
                    caused_by_frame_id: Some(user_frame.id.clone()),
                };
                if let Err(append_err) =
                    append_and_emit_with_policy(&inner, failure_frame, host_policy).await
                {
                    tracing::warn!(
                        error = %append_err,
                        "failed to append console send failure frame"
                    );
                }
                if let Err(update_err) = update_frame_status_and_emit(
                    &inner,
                    &user_frame.id,
                    ConsoleFrameStatus::DeliveryFailed,
                )
                .await
                {
                    tracing::warn!(
                        frame_id = %user_frame.id,
                        error = %update_err,
                        "failed to update console send failure status"
                    );
                }
            }
        }
    });
}

async fn append_steer_delivery_terminal(
    inner: &AggregatorInner,
    user_frame: &ConsoleFrame,
    interaction_id: &str,
) -> ConsoleLogResult<AppendOutcome> {
    append_and_emit(
        inner,
        NewConsoleFrame {
            id: None,
            dedupe_key: format!("steer-delivered:{}", user_frame.id),
            timestamp_ms: current_time_ms(),
            runtime_key: user_frame.runtime_key.clone(),
            identity: user_frame.identity.clone(),
            conversation_id: user_frame.conversation_id.clone(),
            session_id: user_frame.session_id.clone(),
            kind: "interaction_complete".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({
                "reason": "steer_delivered",
                "handling_mode": "steer",
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Synthetic,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: Some(interaction_id.to_string()),
            turn_id: None,
            run_id: None,
            parent_frame_id: Some(user_frame.id.clone()),
            caused_by_frame_id: Some(user_frame.id.clone()),
        },
    )
    .await
}

#[derive(Debug)]
pub enum ConsoleSendError {
    UnknownIdentity(String),
    AmbiguousIdentity {
        identity: String,
        candidates: String,
    },
    NotAddressable(String),
    Retired(String),
    InvalidContent(String),
    InvalidHandlingMode(String),
    InvalidRequest(String),
    IdempotencyConflict(String),
    State(&'static str),
    Dispatch(String),
    /// An observation timeout, with no implied execution or retry verdict.
    AdmissionTimeout {
        identity: String,
        operation: &'static str,
        waited: Duration,
        command: Option<crate::identity_first::ActorCommandTimeout>,
    },
    ActorProbe(Box<crate::identity_first::IdentityRuntimeError>),
    RuntimeOperation(Box<crate::identity_first::IdentityRuntimeError>),
    Log(ConsoleLogError),
    /// The trusted host governs the target's Console input and the caller
    /// is not a verified Console human (anonymous, a service identity or a
    /// token without a `sub`).
    UnqualifiedPrincipal(String),
    /// The trusted host governing the target's Console input refused it.
    GovernedRefused(String),
}

impl std::fmt::Display for ConsoleSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownIdentity(identity) => write!(f, "unknown identity: {identity}"),
            Self::AmbiguousIdentity {
                identity,
                candidates,
            } => write!(
                f,
                "ambiguous live identity alias [via aggregator-list] {identity}: candidates [{candidates}]"
            ),
            Self::NotAddressable(identity) => write!(f, "not addressable: {identity}"),
            Self::Retired(identity) => write!(f, "identity retired: {identity}"),
            Self::InvalidContent(message) => write!(f, "invalid content: {message}"),
            Self::InvalidHandlingMode(mode) => write!(f, "invalid handling mode: {mode}"),
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::IdempotencyConflict(key) => write!(f, "idempotency key conflict: {key}"),
            Self::State(message) => write!(f, "console send state error: {message}"),
            Self::Dispatch(message) => write!(f, "dispatch failed: {message}"),
            Self::AdmissionTimeout {
                identity,
                operation,
                waited,
                command,
            } => crate::identity_first::bridge::fmt_actor_timeout(
                f, operation, identity, *waited, *command,
            ),
            Self::ActorProbe(error) => {
                write!(f, "actor probe interrupted send observation: {error}")
            }
            Self::RuntimeOperation(error) => write!(f, "runtime operation failed: {error}"),
            Self::Log(err) => write!(f, "console log error: {err}"),
            Self::UnqualifiedPrincipal(identity) => write!(
                f,
                "a governed send to {identity} requires an authenticated Console human"
            ),
            Self::GovernedRefused(reason) => write!(f, "governed send refused: {reason}"),
        }
    }
}

impl std::error::Error for ConsoleSendError {}

impl ConsoleSendError {
    pub(crate) fn structured_data(&self) -> Option<Value> {
        match self {
            Self::ActorProbe(error) | Self::RuntimeOperation(error) => error.structured_data(),
            Self::AdmissionTimeout {
                operation,
                waited,
                command,
                ..
            } => Some(crate::identity_first::bridge::actor_timeout_data(
                operation, *waited, *command,
            )),
            _ => None,
        }
    }
}

async fn backfill_session_history(
    inner: Arc<AggregatorInner>,
    runtime_key: String,
    force_refresh: bool,
    assistant_refresh: AssistantHistoryRefreshReason,
) -> ConsoleLogResult<()> {
    let inner = inner.projection_owner.as_ref().unwrap_or(&inner).clone();
    if !inner.options.session_history_backfill_enabled {
        return Ok(());
    }
    let Some(entry) = inner
        .runtimes
        .read()
        .ok()
        .and_then(|entries| entries.get(&runtime_key).cloned())
    else {
        return Ok(());
    };
    let members = Box::pin(member_sources_for_entry(&entry)).await;
    let mut targets = Vec::new();
    for resolved in members {
        let Some(record) = identity_record_for_resolved_member(&resolved).await else {
            continue;
        };
        if runtime_plane_history_target(&record) {
            continue;
        }
        if !Box::pin(resolved_member_visible(&resolved, &record)).await {
            continue;
        }
        let Some(session_id) = record.session_id.clone() else {
            continue;
        };
        let provenance =
            Some(Box::pin(capture_member_provenance(&inner, &resolved, &record)).await);
        targets.push(SessionBackfillTarget {
            assistant_refresh: assistant_refresh.clone(),
            provenance,
            entry: entry.clone(),
            record,
            session_id,
        });
    }
    backfill_session_history_targets(inner, targets, force_refresh).await
}

/// Whether a member's session history would be recorded under a
/// runtime-plane identity: such a member (one created before every creation
/// path refused those identities) gets no history frames.
fn runtime_plane_history_target(record: &ConsoleIdentityRecord) -> bool {
    if !crate::console_contracts::is_runtime_plane_identity(&record.identity) {
        return false;
    }
    crate::console_contracts::warn_runtime_plane_member_drop(
        &record.runtime_member_id,
        None,
        &record.identity,
        "session history",
    );
    true
}

#[derive(Clone)]
struct SessionBackfillTarget {
    assistant_refresh: AssistantHistoryRefreshReason,
    provenance: Option<ConsoleFrameMemberProvenance>,
    entry: RuntimeEntry,
    record: ConsoleIdentityRecord,
    session_id: String,
}

async fn backfill_session_history_targets(
    inner: Arc<AggregatorInner>,
    targets: Vec<SessionBackfillTarget>,
    force_refresh: bool,
) -> ConsoleLogResult<()> {
    let mut tasks = tokio::task::JoinSet::new();
    for target in targets {
        tasks.spawn(backfill_one_session_history(
            inner.clone(),
            target,
            force_refresh,
        ));
    }
    let mut first_error = None;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(Box::new(std::io::Error::other(format!(
                        "session backfill task failed: {err}"
                    ))) as ConsoleLogError);
                }
            }
        }
    }
    if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(())
    }
}

#[derive(Clone)]
struct RuntimeNoticeObservation {
    observed_through: u64,
    relevant_frontier: u64,
    attempts: BTreeSet<(uuid::Uuid, uuid::Uuid)>,
    previous_snapshot: Option<ConsoleFrame>,
    previous_assistant_snapshot: Option<ConsoleFrame>,
    assistant_frontier: u64,
    has_assistant_history: bool,
    prefix_revision: Option<String>,
    prefix_digest: Option<String>,
    has_notice_history: bool,
    canonical_frames: Vec<CanonicalHistoryFrame>,
    full_history_positions: bool,
    history_index_complete: bool,
    verified_history_digest: Option<String>,
    verified_relevant_frontier: u64,
    verified_assistant_frontier: u64,
    verified_assistant_settled: bool,
    verified_settled_digest: Option<String>,
    retained_bytes: usize,
    frontier_anchor: Option<(String, String)>,
    restoration_search: Option<HistoryRestorationSearch>,
}

impl Default for RuntimeNoticeObservation {
    fn default() -> Self {
        Self {
            observed_through: 0,
            relevant_frontier: 0,
            attempts: BTreeSet::new(),
            previous_snapshot: None,
            previous_assistant_snapshot: None,
            assistant_frontier: 0,
            has_assistant_history: false,
            prefix_revision: None,
            prefix_digest: None,
            has_notice_history: false,
            canonical_frames: Vec::new(),
            full_history_positions: false,
            history_index_complete: true,
            verified_history_digest: None,
            verified_relevant_frontier: 0,
            verified_assistant_frontier: 0,
            verified_assistant_settled: false,
            verified_settled_digest: None,
            retained_bytes: 0,
            frontier_anchor: None,
            restoration_search: None,
        }
    }
}

// A successful search proves only that this exact restoration request was
// checked through a console cursor. It supplies no canonical history meaning.
#[derive(Clone)]
struct HistoryRestorationSearch {
    request_digest: String,
    observed_through: u64,
}

impl RuntimeNoticeObservation {
    fn attempt_strings(&self) -> BTreeSet<(String, String)> {
        self.attempts
            .iter()
            .map(|(run, input)| (run.to_string(), input.to_string()))
            .collect()
    }
}

fn history_image_digest(messages: &[Message]) -> String {
    to_hex(&Sha256::digest(
        serde_json::to_vec(messages).unwrap_or_default(),
    ))
}

fn observation_verifies_history(
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> bool {
    observation.verified_relevant_frontier >= observation.relevant_frontier
        && observation.verified_assistant_frontier >= observation.assistant_frontier
        && observation.verified_history_digest.as_deref()
            == Some(history_image_digest(messages).as_str())
}

fn settled_attempts_digest(attempts: &BTreeSet<(String, String)>) -> String {
    to_hex(&Sha256::digest(
        serde_json::to_vec(attempts).unwrap_or_default(),
    ))
}

fn reuse_verified_history_image(
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
    settled_attempts: &BTreeSet<(String, String)>,
) -> bool {
    !observation.history_index_complete
        && observation_verifies_history(observation, messages)
        && observation.verified_settled_digest.as_deref()
            == Some(settled_attempts_digest(settled_attempts).as_str())
}

// Compact comparison records retain source identity and digests, never history
// bodies. A host policy may redact payloads while preserving the existing
// canonical dedupe key minted before that policy is applied.
#[derive(Clone)]
struct CanonicalHistoryFrame {
    id: String,
    dedupe_key: String,
    source_cursor: Option<String>,
    current_cursor: Option<String>,
    sequence: u64,
    payload_key: String,
    source_key: Option<String>,
}

fn history_source_key(
    runtime_key: &str,
    session_id: &str,
    dedupe_key: &str,
    kind: &str,
    run: Option<&str>,
    input: Option<&str>,
) -> Option<String> {
    let suffix =
        dedupe_key.strip_prefix(&format!("session-history:{runtime_key}:{session_id}:"))?;
    let (_, suffix) = suffix.split_once(':')?;
    serde_json::to_string(&(kind, suffix, run, input)).ok()
}

fn history_payload_key(
    kind: &str,
    payload: &Value,
    run: Option<&str>,
    input: Option<&str>,
) -> String {
    let bytes = serde_json::to_vec(&(kind, payload, run, input)).unwrap_or_default();
    to_hex(&Sha256::digest(bytes))
}

impl CanonicalHistoryFrame {
    fn from_frame(frame: &ConsoleFrame) -> Self {
        Self {
            id: frame.id.clone(),
            dedupe_key: frame.dedupe_key.clone(),
            source_cursor: frame.source.source_cursor.clone(),
            current_cursor: frame.source.source_cursor.clone(),
            sequence: frame.cursor.seq().unwrap_or(u64::MAX),
            payload_key: history_payload_key(
                &frame.kind,
                &frame.payload,
                frame.run_id.as_deref(),
                frame.interaction_id.as_deref(),
            ),
            source_key: history_source_key(
                &frame.runtime_key,
                frame.session_id.as_deref().unwrap_or_default(),
                &frame.dedupe_key,
                &frame.kind,
                frame.run_id.as_deref(),
                frame.interaction_id.as_deref(),
            ),
        }
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.id.len()
            + self.dedupe_key.len()
            + self.source_cursor.as_ref().map_or(0, String::len)
            + self.current_cursor.as_ref().map_or(0, String::len)
            + self.payload_key.len()
            + self.source_key.as_ref().map_or(0, String::len)
    }
}

fn apply_notice_snapshot(observation: &mut RuntimeNoticeObservation, frame: &ConsoleFrame) {
    // Whole-payload redaction may remove the wire map. Its pre-policy witness
    // still records that this session crossed a complete compaction boundary.
    observation.full_history_positions |= frame
        .dedupe_key
        .strip_prefix(&format!(
            "runtime-notice-snapshot-v2:{}:{}:",
            frame.runtime_key,
            frame.session_id.as_deref().unwrap_or_default()
        ))
        .and_then(|suffix| suffix.split(':').nth(2))
        == Some("full");
    let Some(boundary) = frame
        .payload
        .get("observed_through")
        .and_then(Value::as_str)
        .and_then(|cursor| ConsoleCursor::from(cursor).seq())
    else {
        return;
    };
    let Some(positions) = frame
        .payload
        .get("history_positions")
        .and_then(Value::as_array)
    else {
        return;
    };
    let positions: BTreeMap<_, _> = positions
        .iter()
        .filter_map(|position| {
            Some((
                position.get("frame_id")?.as_str()?,
                position.get("source_cursor")?.as_str()?,
            ))
        })
        .collect();
    let sparse = frame
        .payload
        .get("history_positions_mode")
        .and_then(Value::as_str)
        == Some("sparse");
    let removed: BTreeSet<_> = frame
        .payload
        .get("removed_history_frame_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    observation.full_history_positions |= !sparse || !removed.is_empty();
    observation.canonical_frames.retain_mut(|row| {
        if row.sequence > boundary {
            return true;
        }
        if let Some(cursor) = positions.get(row.id.as_str()) {
            row.current_cursor = Some((*cursor).to_string());
            true
        } else if sparse && !removed.contains(row.id.as_str()) {
            // A sparse image is a delta over the last map. An unchanged row
            // retains its relocated coordinate, including after replay.
            true
        } else {
            false
        }
    });
    if observation.canonical_frames.capacity()
        > observation.canonical_frames.len().saturating_mul(2)
    {
        observation.canonical_frames.shrink_to_fit();
    }
    observation.retained_bytes = observation
        .canonical_frames
        .iter()
        .map(CanonicalHistoryFrame::retained_bytes)
        .sum::<usize>()
        + (observation.canonical_frames.capacity() - observation.canonical_frames.len())
            * std::mem::size_of::<CanonicalHistoryFrame>();
}

async fn observe_runtime_notice_attempts(
    inner: &AggregatorInner,
    runtime_key: &str,
    identity: &str,
    session_id: &str,
) -> ConsoleLogResult<RuntimeNoticeObservation> {
    observe_runtime_notice_attempts_with_cache(inner, runtime_key, identity, session_id, true).await
}

async fn observe_runtime_notice_attempts_with_cache(
    inner: &AggregatorInner,
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    use_cache: bool,
) -> ConsoleLogResult<RuntimeNoticeObservation> {
    let prefix_revision = inner.store.history_prefix_revision().await?;
    let key = (
        runtime_key.to_string(),
        identity.to_string(),
        session_id.to_string(),
    );
    let observed_through = inner
        .store
        .latest_cursor()
        .await?
        .and_then(|cursor| cursor.seq())
        .unwrap_or(0);
    let mut observation = if use_cache {
        inner
            .notice_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&key)
            .cloned()
            .unwrap_or_default()
    } else {
        RuntimeNoticeObservation::default()
    };
    let anchor_valid = if let Some((dedupe, id)) = &observation.frontier_anchor {
        inner
            .store
            .frame_by_dedupe_key(dedupe)
            .await?
            .is_some_and(|frame| frame.id == *id)
    } else {
        observation.observed_through == 0
    };
    if observation.observed_through > observed_through
        || !anchor_valid
        || prefix_revision.is_none()
        || observation.prefix_revision != prefix_revision
    {
        observation = RuntimeNoticeObservation::default();
    }
    observation.prefix_revision = prefix_revision.clone();
    let mut prefix_digest = Sha256::new();
    if observation.observed_through == observed_through {
        if prefix_revision.is_none() {
            observation.prefix_digest = Some(to_hex(&prefix_digest.finalize()));
        }
        return Ok(observation);
    }
    let mut after = (observation.observed_through > 0)
        .then(|| ConsoleCursor::from_seq(observation.observed_through));
    observation.observed_through = observed_through;
    loop {
        let page = inner
            .store
            .query_frames(ConsoleTimelineQuery {
                identity: Some(identity.to_string()),
                after: after.clone(),
                limit: 1_000,
                ..Default::default()
            })
            .await?;
        let Some(last) = page.frames.last() else {
            break;
        };
        let last_cursor = last.cursor.clone();
        if last_cursor.seq().is_none_or(|seq| {
            after
                .as_ref()
                .and_then(ConsoleCursor::seq)
                .is_some_and(|previous| seq <= previous)
        }) {
            return Err(Box::new(std::io::Error::other(
                "runtime notice scan made no cursor progress",
            )));
        }
        for frame in page.frames {
            if prefix_revision.is_none()
                && frame
                    .cursor
                    .seq()
                    .is_some_and(|seq| seq <= observed_through)
            {
                prefix_digest.update(serde_json::to_vec(&frame)?);
            }
            if frame
                .cursor
                .seq()
                .is_some_and(|seq| seq <= observed_through)
            {
                observation.frontier_anchor = Some((frame.dedupe_key.clone(), frame.id.clone()));
            }
            if frame
                .cursor
                .seq()
                .is_none_or(|seq| seq > observation.observed_through)
                || frame.runtime_key != runtime_key
                || frame.session_id.as_deref() != Some(session_id)
            {
                continue;
            }
            if frame.kind == "assistant_history_snapshot"
                && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
            {
                let mut retained = frame;
                retained.payload = Value::Null;
                retained.source.member_provenance = None;
                observation.previous_assistant_snapshot = Some(retained);
                continue;
            }
            if frame.payload.get("assistant_message_id").is_some_and(|id| {
                serde_json::from_value::<meerkat_core::types::AssistantMessageId>(id.clone())
                    .is_ok()
            }) {
                if frame.source.kind == ConsoleFrameSourceKind::ConsoleEvent {
                    observation.assistant_frontier = observation
                        .assistant_frontier
                        .max(frame.cursor.seq().unwrap_or(0));
                } else if frame.source.kind == ConsoleFrameSourceKind::SessionHistory {
                    observation.has_assistant_history = true;
                }
            }
            if frame.kind == "runtime_notice_snapshot"
                && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
            {
                apply_notice_snapshot(&mut observation, &frame);
                // New snapshot dedupe keys retain the pre-policy comparison
                // witness. Notice bodies are unnecessary cache state.
                let mut retained = frame;
                retained.payload = json!({
                    "observed_through": retained.payload.get("observed_through"),
                });
                retained.source.member_provenance = None;
                observation.previous_snapshot = Some(retained);
                continue;
            }
            if frame.kind == "system_notice"
                && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                && frame
                    .payload
                    .get("message")
                    .and_then(|message| message.get("runtime_origin"))
                    .is_some_and(Value::is_object)
            {
                observation.has_notice_history = true;
                observation.relevant_frontier = observation
                    .relevant_frontier
                    .max(frame.cursor.seq().unwrap_or(0));
            }
            if frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                && !(frame.kind == "system_notice"
                    && frame
                        .payload
                        .get("message")
                        .and_then(|message| message.get("runtime_origin"))
                        .is_some_and(Value::is_object))
            {
                let retained = CanonicalHistoryFrame::from_frame(&frame);
                observation.retained_bytes = observation
                    .retained_bytes
                    .saturating_add(retained.retained_bytes());
                observation.canonical_frames.push(retained);
            }
            if frame.kind != "boundary_append_applied"
                || frame.source.kind != ConsoleFrameSourceKind::ConsoleEvent
            {
                continue;
            }
            let Ok(meerkat_core::event::AgentEvent::BoundaryAppendApplied {
                run_id,
                input_id,
                notices,
                ..
            }) = serde_json::from_value::<meerkat_core::event::AgentEvent>(frame.payload.clone())
            else {
                continue;
            };
            if frame.run_id.as_deref() != Some(run_id.to_string().as_str())
                || !notices.iter().any(|notice| {
                    notice.runtime_origin.as_ref().is_some_and(|origin| {
                        origin.session_id.to_string() == session_id
                            && origin.run_id == run_id
                            && origin.input_id == input_id
                    })
                })
            {
                continue;
            }
            observation.relevant_frontier = observation
                .relevant_frontier
                .max(frame.cursor.seq().unwrap_or(0));
            observation.attempts.insert((run_id.0, input_id.0));
        }
        if last_cursor
            .seq()
            .is_none_or(|seq| seq >= observation.observed_through)
        {
            break;
        }
        after = Some(last_cursor);
    }
    if inner.store.history_prefix_revision().await? != prefix_revision {
        return Err(std::io::Error::other(
            "console history prefix changed during assistant history observation; retry required",
        )
        .into());
    }
    if prefix_revision.is_none() {
        observation.prefix_digest = Some(to_hex(&prefix_digest.finalize()));
    }
    cache_notice_observation(inner, key, &observation);
    Ok(observation)
}

fn cache_notice_observation(
    inner: &AggregatorInner,
    key: (String, String, String),
    observation: &RuntimeNoticeObservation,
) {
    let mut cache = inner
        .notice_observations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Attempt identities remain exact and independently retained: the owner
    // can settle an attempt without a corresponding console event. Only the
    // expensive history index is eligible for bounded frontier-only caching.
    let retained_bytes = observation
        .retained_bytes
        .saturating_add(observation.restoration_search.as_ref().map_or(0, |search| {
            std::mem::size_of::<HistoryRestorationSearch>() + search.request_digest.len()
        }))
        .saturating_add(observation.previous_snapshot.as_ref().map_or(0, |frame| {
            std::mem::size_of::<ConsoleFrame>()
                + frame.dedupe_key.len()
                + frame.id.len()
                + frame.runtime_key.len()
                + frame.identity.len()
                + frame.cursor.as_str().len()
        }))
        .saturating_add(
            observation
                .previous_assistant_snapshot
                .as_ref()
                .map_or(0, |frame| {
                    std::mem::size_of::<ConsoleFrame>()
                        + frame.dedupe_key.len()
                        + frame.id.len()
                        + frame.runtime_key.len()
                        + frame.identity.len()
                        + frame.cursor.as_str().len()
                }),
        );
    if retained_bytes <= NOTICE_OBSERVATION_MAX_BYTES {
        cache.insert(key, observation.clone());
    } else {
        let mut frontier = observation.clone();
        frontier.canonical_frames = Vec::new();
        frontier.retained_bytes = 0;
        frontier.history_index_complete = false;
        // A restoration witness was checked with exact active reservations.
        // Dropping that index cannot carry its negative lookup forward.
        frontier.restoration_search = None;
        cache.insert(key, frontier);
    }
}

fn complete_current_history_page(
    session_id: &str,
    page: meerkat_core::service::SessionHistoryPage,
) -> Option<meerkat_core::service::SessionHistoryPage> {
    (page.session_id.to_string() == session_id
        && page.offset == 0
        && !page.has_more
        && page.messages.len() == page.message_count)
        .then_some(page)
}

#[cfg(test)]
fn current_history_positions(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> Vec<Value> {
    current_history_position_changes(runtime_key, identity, session_id, observation, messages).0
}

#[cfg(test)]
fn current_history_position_changes(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> (Vec<Value>, Vec<String>) {
    let (positions, removed) =
        current_history_mapping(runtime_key, identity, session_id, observation, messages);
    let old: BTreeMap<_, _> = observation
        .canonical_frames
        .iter()
        .map(|frame| (frame.id.as_str(), frame.current_cursor.as_deref()))
        .collect();
    (
        positions
            .into_iter()
            .filter(|position| {
                old.get(position["frame_id"].as_str().unwrap_or_default())
                    .copied()
                    .flatten()
                    != position["source_cursor"].as_str()
            })
            .collect(),
        removed,
    )
}

fn current_history_projection(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    messages: &[Message],
) -> Vec<NewConsoleFrame> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(offset, message)| {
            frames_from_session_history_message_with_namespace(
                runtime_key,
                identity,
                "",
                session_id,
                offset,
                serde_json::to_value(message).unwrap_or(Value::Null),
            )
        })
        .filter(|frame| {
            frame.identity == identity
                && !(frame.kind == "system_notice"
                    && frame
                        .payload
                        .get("message")
                        .and_then(|message| message.get("runtime_origin"))
                        .is_some_and(Value::is_object))
        })
        .collect()
}

fn canonical_history_order(
    session_id: &str,
    row: &CanonicalHistoryFrame,
) -> (Vec<(u8, u64, String)>, u64) {
    let prefix = format!("{session_id}:");
    let suffix = row
        .current_cursor
        .as_deref()
        .or(row.source_cursor.as_deref())
        .and_then(|cursor| cursor.strip_prefix(&prefix))
        .unwrap_or("");
    (
        suffix
            .split(':')
            .map(|part| match part.parse::<u64>() {
                Ok(number) => (0, number, String::new()),
                Err(_) => (1, 0, part.to_string()),
            })
            .collect(),
        row.sequence,
    )
}

fn history_can_publish_notice_snapshot(
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> bool {
    history_has_assistant_identity(observation, messages)
        || !observation.attempts.is_empty()
        || observation.has_notice_history
        || observation.previous_snapshot.is_some()
        || messages.iter().any(|message| {
            matches!(message, Message::SystemNotice(notice) if notice.runtime_origin.is_some())
        })
}

fn history_has_assistant_identity(
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> bool {
    observation.assistant_frontier > 0
        || observation.has_assistant_history
        || observation.previous_assistant_snapshot.is_some()
        || messages.iter().any(|message| {
            matches!(message, Message::BlockAssistant(assistant) if assistant.assistant_message_id.is_some())
        })
}

fn assistant_history_snapshot_frame(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> Option<NewConsoleFrame> {
    assistant_history_snapshot_frame_inner(
        runtime_key,
        identity,
        session_id,
        observation,
        messages,
        false,
    )
}

fn assistant_history_snapshot_frame_inner(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
    force: bool,
) -> Option<NewConsoleFrame> {
    if !history_has_assistant_identity(observation, messages) {
        return None;
    }
    let ids: Vec<_> = messages
        .iter()
        .filter_map(|message| {
            let Message::BlockAssistant(assistant) = message else {
                return None;
            };
            assistant.assistant_message_id.as_ref()
        })
        .collect();
    let digest = to_hex(&Sha256::digest(serde_json::to_vec(&ids).ok()?));
    let prefix = format!("assistant-history-snapshot-v1:{runtime_key}:{session_id}:");
    let previous = observation.previous_assistant_snapshot.as_ref();
    // Persist the comparison witness before host redaction. Neither this
    // marker's append nor unrelated live traffic creates a new identity image.
    if !force
        && let Some(witness) = previous.and_then(|frame| frame.dedupe_key.strip_prefix(&prefix))
    {
        let fields: Vec<_> = witness.split(':').collect();
        if fields.len() == 4
            && fields[1].parse::<u64>().ok() == Some(observation.assistant_frontier)
            && fields[3] == digest
        {
            return None;
        }
    }
    Some(NewConsoleFrame {
        id: None,
        dedupe_key: format!(
            "{prefix}{}:{}:{}:{digest}",
            previous.and_then(|frame| frame.cursor.seq()).unwrap_or(0),
            observation.assistant_frontier,
            observation.observed_through
        ),
        timestamp_ms: current_time_ms(),
        runtime_key: runtime_key.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(session_id.to_string()),
        kind: "assistant_history_snapshot".to_string(),
        status: ConsoleFrameStatus::Delivered,
        payload: json!({
            "session_id": session_id, "complete": true,
            "observed_through": ConsoleCursor::from_seq(observation.observed_through),
            "assistant_message_ids": ids,
        }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    })
}

async fn assistant_history_prefix_is_current(
    inner: &AggregatorInner,
    identity: &str,
    observation: &RuntimeNoticeObservation,
) -> ConsoleLogResult<bool> {
    let revision = inner.store.history_prefix_revision().await?;
    if revision.is_some() || observation.prefix_revision.is_some() {
        return Ok(revision == observation.prefix_revision);
    }
    // A custom store without revision support gets a fresh bounded scan, never
    // cached absence. Hash the exact prefix again after the owner history read
    // so resets or replacements cannot silently certify the wrong projection.
    let mut digest = Sha256::new();
    let mut after = None;
    if observation.observed_through > 0 {
        loop {
            let page = inner
                .store
                .query_frames(ConsoleTimelineQuery {
                    identity: Some(identity.to_string()),
                    after: after.clone(),
                    limit: 1_000,
                    ..Default::default()
                })
                .await?;
            let Some(last) = page.frames.last() else {
                break;
            };
            let last_cursor = last.cursor.clone();
            if last_cursor.seq().is_none_or(|sequence| {
                after
                    .as_ref()
                    .and_then(ConsoleCursor::seq)
                    .is_some_and(|old| sequence <= old)
            }) {
                return Err(std::io::Error::other(
                    "assistant history verification made no cursor progress",
                )
                .into());
            }
            for frame in page.frames {
                if frame
                    .cursor
                    .seq()
                    .is_some_and(|seq| seq <= observation.observed_through)
                {
                    digest.update(serde_json::to_vec(&frame)?);
                }
            }
            if last_cursor
                .seq()
                .is_none_or(|seq| seq >= observation.observed_through)
            {
                break;
            }
            after = Some(last_cursor);
        }
    }
    Ok(observation.prefix_digest.as_deref() == Some(to_hex(&digest.finalize()).as_str()))
}

async fn restore_current_history_frames(
    inner: &AggregatorInner,
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &mut RuntimeNoticeObservation,
    messages: &[Message],
    appended_history_start: Option<usize>,
) -> ConsoleLogResult<()> {
    if !observation.full_history_positions
        || !history_can_publish_notice_snapshot(observation, messages)
    {
        return Ok(());
    }
    let mut present: BTreeSet<_> = observation
        .canonical_frames
        .iter()
        .map(|row| row.dedupe_key.clone())
        .collect();
    let (mapped, _) =
        current_history_mapping(runtime_key, identity, session_id, observation, messages);
    let mapped: BTreeSet<_> = mapped
        .iter()
        .filter_map(|row| row["source_cursor"].as_str())
        .collect();
    let mut unmatched = BTreeMap::<String, (Option<String>, String)>::new();
    for candidate in current_history_projection(runtime_key, identity, session_id, messages) {
        if candidate
            .source
            .source_cursor
            .as_deref()
            .is_some_and(|cursor| mapped.contains(cursor))
            || !present.insert(candidate.dedupe_key.clone())
        {
            continue;
        }
        if let Some(frame) = inner
            .store
            .frame_by_dedupe_key(&candidate.dedupe_key)
            .await?
            && frame.runtime_key == runtime_key
            && frame.identity == identity
            && frame.session_id.as_deref() == Some(session_id)
            && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
            && frame
                .cursor
                .seq()
                .is_some_and(|seq| seq <= observation.observed_through)
        {
            let mut retained = CanonicalHistoryFrame::from_frame(&frame);
            // This row was absent from the active map. Reactivating even its
            // original coordinate must publish a new position image.
            retained.current_cursor = None;
            observation.retained_bytes = observation
                .retained_bytes
                .saturating_add(retained.retained_bytes());
            observation.canonical_frames.push(retained);
        } else if let Some(cursor) = candidate.source.source_cursor {
            let offset = cursor
                .strip_prefix(&format!("{session_id}:"))
                .and_then(|suffix| suffix.split(':').next())
                .and_then(|offset| offset.parse::<usize>().ok());
            // New suffix rows are appended by the ordinary history backfill
            // after this snapshot and need no old-coordinate restoration.
            let always_appended =
                history_counterpart_has_typed_owner(
                    candidate.run_id.as_deref(),
                    candidate.interaction_id.as_deref(),
                ) && matches!(
                    history_counterpart_category(&candidate.kind),
                    Some("assistant" | "reasoning" | "user")
                ) || history_counterpart_tool_id(&candidate.kind, &candidate.payload).is_some()
                    || history_counterpart_category(&candidate.kind).is_none();
            if always_appended
                && appended_history_start
                    .is_some_and(|start| offset.is_some_and(|offset| offset >= start))
            {
                continue;
            }
            unmatched.insert(
                cursor,
                (
                    history_source_key(
                        runtime_key,
                        session_id,
                        &candidate.dedupe_key,
                        &candidate.kind,
                        candidate.run_id.as_deref(),
                        candidate.interaction_id.as_deref(),
                    ),
                    history_payload_key(
                        &candidate.kind,
                        &candidate.payload,
                        candidate.run_id.as_deref(),
                        candidate.interaction_id.as_deref(),
                    ),
                ),
            );
        }
    }
    if unmatched.is_empty() {
        return Ok(());
    }
    // Only a restoration missing from the active index needs a historical
    // lookup. Keep at most one old record per unmatched current occurrence;
    // neither removed bodies nor an ever-growing tombstone cache are retained.
    let mut recovered = BTreeMap::<String, Vec<CanonicalHistoryFrame>>::new();
    let mut capacities = BTreeMap::<String, usize>::new();
    for (source, payload) in unmatched.values() {
        *capacities.entry(format!("payload:{payload}")).or_default() += 1;
        if let Some(source) = source {
            *capacities.entry(format!("source:{source}")).or_default() += 1;
        }
    }
    // A request changes when its needed source/payload multiplicities change
    // or a matching active record becomes available for reuse. Unrelated new
    // history rows must not invalidate this completed negative lookup.
    let reserved: BTreeSet<_> = observation
        .canonical_frames
        .iter()
        .filter(|row| {
            capacities.contains_key(&format!("payload:{}", row.payload_key))
                || row
                    .source_key
                    .as_ref()
                    .is_some_and(|key| capacities.contains_key(&format!("source:{key}")))
        })
        .map(|row| (&row.id, &row.dedupe_key, &row.current_cursor))
        .collect();
    let request_digest = to_hex(&Sha256::digest(
        serde_json::to_vec(&(&capacities, reserved)).unwrap_or_default(),
    ));
    let searched_through = observation
        .restoration_search
        .as_ref()
        .filter(|search| search.request_digest == request_digest)
        .map_or(0, |search| search.observed_through);
    if searched_through >= observation.observed_through {
        return Ok(());
    }
    let mut after = (searched_through > 0).then(|| ConsoleCursor::from_seq(searched_through));
    loop {
        let page = inner
            .store
            .query_frames(ConsoleTimelineQuery {
                identity: Some(identity.to_string()),
                after: after.clone(),
                limit: 1_000,
                ..Default::default()
            })
            .await?;
        let Some(last) = page.frames.last() else {
            break;
        };
        let last_cursor = last.cursor.clone();
        if last_cursor.seq().is_none_or(|seq| {
            after
                .as_ref()
                .and_then(ConsoleCursor::seq)
                .is_some_and(|old| seq <= old)
        }) {
            return Err(
                std::io::Error::other("history restoration scan made no cursor progress").into(),
            );
        }
        for frame in page.frames {
            if frame.runtime_key != runtime_key
                || frame.identity != identity
                || frame.session_id.as_deref() != Some(session_id)
                || frame.source.kind != ConsoleFrameSourceKind::SessionHistory
                || frame
                    .cursor
                    .seq()
                    .is_none_or(|seq| seq > observation.observed_through)
                || present.contains(&frame.dedupe_key)
            {
                continue;
            }
            let row = CanonicalHistoryFrame::from_frame(&frame);
            for key in std::iter::once(format!("payload:{}", row.payload_key))
                .chain(row.source_key.as_ref().map(|key| format!("source:{key}")))
            {
                let Some(capacity) = capacities.get(&key) else {
                    continue;
                };
                let candidates = recovered.entry(key).or_default();
                candidates.push(row.clone());
                candidates.sort_by_key(|row| canonical_history_order(session_id, row));
                candidates.truncate(*capacity);
            }
        }
        if last_cursor
            .seq()
            .is_none_or(|seq| seq >= observation.observed_through)
        {
            break;
        }
        after = Some(last_cursor);
    }
    // Failed/incomplete queries above never advance this witness. The enclosing
    // observation cache already scopes it to the runtime, identity and session
    // and validates a durable anchor before reuse, including after store reset.
    let search = HistoryRestorationSearch {
        request_digest,
        observed_through: observation.observed_through,
    };
    let mut recovered_ids = BTreeSet::new();
    for mut row in recovered.into_values().flatten() {
        if !recovered_ids.insert(row.id.clone()) {
            continue;
        }
        row.current_cursor = None;
        observation.retained_bytes = observation
            .retained_bytes
            .saturating_add(row.retained_bytes());
        observation.canonical_frames.push(row);
    }
    // A successful positive search changes reservations. Cache a negative
    // frontier only after a scan recovers nothing under the exact request.
    observation.restoration_search = recovered_ids.is_empty().then_some(search);
    Ok(())
}

fn cache_current_history_image(
    inner: &AggregatorInner,
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &mut RuntimeNoticeObservation,
    messages: &[Message],
    settled_attempts: &BTreeSet<(String, String)>,
) {
    let (positions, removed) =
        current_history_mapping(runtime_key, identity, session_id, observation, messages);
    let positions: BTreeMap<_, _> = positions
        .iter()
        .filter_map(|position| {
            Some((
                position["frame_id"].as_str()?,
                position["source_cursor"].as_str()?,
            ))
        })
        .collect();
    observation.full_history_positions |= !removed.is_empty()
        || observation.canonical_frames.iter().any(|row| {
            positions
                .get(row.id.as_str())
                .is_some_and(|cursor| row.current_cursor.as_deref() != Some(*cursor))
        });
    observation.canonical_frames.retain_mut(|row| {
        let Some(cursor) = positions.get(row.id.as_str()) else {
            return false;
        };
        row.current_cursor = Some((*cursor).to_string());
        true
    });
    if observation.canonical_frames.capacity()
        > observation.canonical_frames.len().saturating_mul(2)
    {
        observation.canonical_frames.shrink_to_fit();
    }
    observation.retained_bytes = observation
        .canonical_frames
        .iter()
        .map(CanonicalHistoryFrame::retained_bytes)
        .sum::<usize>()
        + (observation.canonical_frames.capacity() - observation.canonical_frames.len())
            * std::mem::size_of::<CanonicalHistoryFrame>();
    observation.verified_history_digest = Some(history_image_digest(messages));
    observation.verified_relevant_frontier = observation.relevant_frontier;
    observation.verified_assistant_frontier = observation.assistant_frontier;
    observation.verified_settled_digest = Some(settled_attempts_digest(settled_attempts));
    cache_notice_observation(
        inner,
        (
            runtime_key.to_string(),
            identity.to_string(),
            session_id.to_string(),
        ),
        observation,
    );
}

fn current_history_mapping(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &RuntimeNoticeObservation,
    messages: &[Message],
) -> (Vec<Value>, Vec<String>) {
    let current = current_history_projection(runtime_key, identity, session_id, messages);
    let mut exact = BTreeMap::new();
    let mut by_source: BTreeMap<String, VecDeque<usize>> = BTreeMap::new();
    let mut by_payload: BTreeMap<String, VecDeque<usize>> = BTreeMap::new();
    for (index, frame) in current.iter().enumerate() {
        exact.insert(frame.dedupe_key.as_str(), index);
        if let Some(key) = history_source_key(
            runtime_key,
            session_id,
            &frame.dedupe_key,
            &frame.kind,
            frame.run_id.as_deref(),
            frame.interaction_id.as_deref(),
        ) {
            by_source.entry(key).or_default().push_back(index);
        }
        by_payload
            .entry(history_payload_key(
                &frame.kind,
                &frame.payload,
                frame.run_id.as_deref(),
                frame.interaction_id.as_deref(),
            ))
            .or_default()
            .push_back(index);
    }
    let mut old: Vec<_> = observation.canonical_frames.iter().collect();
    old.sort_by_key(|frame| canonical_history_order(session_id, frame));
    // Reserve exact surviving occurrences before any relocation match.
    let exact_matches: BTreeMap<_, _> = old
        .iter()
        .filter_map(|row| {
            exact
                .get(row.dedupe_key.as_str())
                .map(|index| (row.id.as_str(), *index))
        })
        .collect();
    let mut used: BTreeSet<_> = exact_matches.values().copied().collect();
    fn available(queue: Option<&mut VecDeque<usize>>, used: &mut BTreeSet<usize>) -> Option<usize> {
        let queue = queue?;
        while let Some(index) = queue.pop_front() {
            if used.insert(index) {
                return Some(index);
            }
        }
        None
    }
    let mut positions = Vec::new();
    let mut removed = Vec::new();
    for row in old {
        let index = exact_matches.get(row.id.as_str()).copied().or_else(|| {
            available(
                row.source_key
                    .as_ref()
                    .and_then(|key| by_source.get_mut(key)),
                &mut used,
            )
            .or_else(|| available(by_payload.get_mut(&row.payload_key), &mut used))
        });
        if let Some(cursor) = index.and_then(|index| current[index].source.source_cursor.as_ref()) {
            positions.push(json!({ "frame_id": row.id, "source_cursor": cursor }));
        } else {
            removed.push(row.id.clone());
        }
    }
    (positions, removed)
}

fn runtime_entry_is_current(inner: &AggregatorInner, entry: &RuntimeEntry) -> bool {
    inner.runtimes.read().is_ok_and(|entries| {
        entries
            .get(&entry.runtime_key)
            .is_some_and(|current| current.registration_id == entry.registration_id)
    })
}

fn runtime_notice_snapshot_frame(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    observation: &RuntimeNoticeObservation,
    settled_attempts: &BTreeSet<(String, String)>,
    messages: &[Message],
) -> Option<NewConsoleFrame> {
    // A frontier-only cache is never an absence proof. The production path
    // either reuses its exact verified image or rebuilds before calling here.
    if !observation.history_index_complete {
        return None;
    }
    let notices: Vec<Value> = messages
        .iter()
        .enumerate()
        .filter_map(|(offset, message)| {
            let Message::SystemNotice(notice) = message else {
                return None;
            };
            notice.runtime_origin.as_ref()?;
            Some(json!({ "offset": offset, "message": message }))
        })
        .collect();
    if !history_can_publish_notice_snapshot(observation, messages) {
        return None;
    }
    let settled: Vec<Value> = settled_attempts
        .iter()
        .map(|(run_id, input_id)| json!({ "run_id": run_id, "input_id": input_id }))
        .collect();
    let (all_positions, removed) =
        current_history_mapping(runtime_key, identity, session_id, observation, messages);
    let previous_positions: BTreeMap<_, _> = observation
        .canonical_frames
        .iter()
        .map(|row| (row.id.as_str(), row.current_cursor.as_deref()))
        .collect();
    let changes: Vec<_> = all_positions
        .iter()
        .filter(|position| {
            previous_positions
                .get(position["frame_id"].as_str().unwrap_or_default())
                .copied()
                .flatten()
                != position["source_cursor"].as_str()
        })
        .collect();
    let previous = observation.previous_snapshot.as_ref();
    // Crossing a compaction once enables restoration searches, but does not
    // justify resending that full map on every later append or notice update.
    let full = !removed.is_empty()
        || !changes.is_empty()
        || (previous.is_none() && observation.full_history_positions);
    let state_digest = to_hex(&Sha256::digest(
        serde_json::to_vec(&(notices.clone(), settled.clone())).ok()?,
    ));
    let positions_digest = |boundary: u64| {
        let covered: BTreeSet<_> = observation
            .canonical_frames
            .iter()
            .filter(|row| row.sequence <= boundary)
            .map(|row| row.id.as_str())
            .collect();
        let mut positions: Vec<_> = all_positions
            .iter()
            .filter(|position| {
                position["frame_id"]
                    .as_str()
                    .is_some_and(|id| covered.contains(id))
            })
            .cloned()
            .collect();
        positions.sort_by(|a, b| a["frame_id"].as_str().cmp(&b["frame_id"].as_str()));
        to_hex(&Sha256::digest(
            serde_json::to_vec(&positions).unwrap_or_default(),
        ))
    };
    // The witness is minted before host redaction and contains hashes only.
    // Its covered boundary keeps append-only rows from invalidating a prior
    // compaction image, while changed/restored covered rows still refresh it.
    if let Some(previous) = previous
        && let Some(witness) = previous.dedupe_key.strip_prefix(&format!(
            "runtime-notice-snapshot-v2:{runtime_key}:{session_id}:"
        ))
    {
        let fields: Vec<_> = witness.split(':').collect();
        if fields.len() == 5
            && let Ok(boundary) = fields[1].parse::<u64>()
            && boundary >= observation.relevant_frontier
            && fields[3] == state_digest
            && if fields[2] == "full" {
                fields[4] == positions_digest(boundary)
                    && !changes.iter().any(|position| {
                        observation.canonical_frames.iter().any(|row| {
                            row.sequence > boundary
                                && Some(row.id.as_str()) == position["frame_id"].as_str()
                        })
                    })
                    && !removed.iter().any(|id| {
                        observation
                            .canonical_frames
                            .iter()
                            .any(|row| row.sequence > boundary && row.id == *id)
                    })
            } else {
                !full
            }
        {
            return None;
        }
    }
    let mut payload = json!({
        "session_id": session_id, "complete": true,
        "observed_through": ConsoleCursor::from_seq(observation.observed_through),
        "notices": notices, "settled_attempts": settled,
        "history_positions": if full { all_positions.clone() } else { Vec::new() },
    });
    if !full {
        payload["history_positions_mode"] = json!("sparse");
        payload["removed_history_frame_ids"] = json!([]);
    }
    Some(NewConsoleFrame {
        id: None,
        dedupe_key: format!(
            "runtime-notice-snapshot-v2:{runtime_key}:{session_id}:{}:{}:{}:{state_digest}:{}",
            previous.and_then(|frame| frame.cursor.seq()).unwrap_or(0),
            observation.observed_through,
            if full { "full" } else { "sparse" },
            positions_digest(observation.observed_through),
        ),
        timestamp_ms: current_time_ms(),
        runtime_key: runtime_key.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(session_id.to_string()),
        kind: "runtime_notice_snapshot".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload,
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    })
}

async fn backfill_one_session_history(
    inner: Arc<AggregatorInner>,
    target: SessionBackfillTarget,
    force_refresh: bool,
) -> ConsoleLogResult<()> {
    backfill_one_session_history_with_refresh_observer(
        inner,
        target,
        force_refresh,
        |entry, record, session_id, requested| {
            Box::pin(assistant_history_refresh::observe(
                entry, record, session_id, requested,
            ))
        },
    )
    .await
}

// Keep observation local to this backfill invocation so orchestration tests can
// exercise a pending refresh without manufacturing session-service authority.
async fn backfill_one_session_history_with_refresh_observer<Observe>(
    inner: Arc<AggregatorInner>,
    target: SessionBackfillTarget,
    force_refresh: bool,
    mut observe_refresh: Observe,
) -> ConsoleLogResult<()>
where
    Observe: for<'a> FnMut(
            &'a RuntimeEntry,
            &'a ConsoleIdentityRecord,
            &'a str,
            bool,
        ) -> futures::future::BoxFuture<
            'a,
            assistant_history_refresh::AssistantHistoryRefreshGate,
        > + Send,
{
    // A member under a runtime-plane identity (one created before every
    // creation path refused them) gets no history frames from any target
    // source: the identity list and inspection paths skip it here, before
    // reading anything, as the discovery pass does.
    if runtime_plane_history_target(&target.record) {
        return Ok(());
    }
    let _permit = inner
        .session_backfill_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|err| -> ConsoleLogError {
            Box::new(std::io::Error::other(format!(
                "session backfill limiter closed: {err}"
            )))
        })?;
    let SessionBackfillTarget {
        assistant_refresh,
        provenance,
        entry,
        record,
        session_id,
    } = target;
    let provenance = match provenance {
        Some(provenance) => Some(provenance),
        None => {
            member_provenance_for_identity(
                &inner,
                &entry,
                &record.identity,
                Some(&session_id),
                false,
            )
            .await
        }
    };
    let watermark_runtime_key =
        session_history_watermark_runtime_key(&entry.runtime_key, &session_id);
    let projection_lock = inner
        .session_history_projection_locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(watermark_runtime_key.clone())
        .or_default()
        .clone();
    let _projection_guard = projection_lock.lock().await;
    // A replaced target waiting behind its successor must not overwrite it.
    // An already-running predecessor may finish its append, but this same
    // lock ensures the successor publishes afterward. Completed snapshots
    // remain durable observations across registrations and process restarts.
    if !runtime_entry_is_current(&inner, &entry) {
        return Ok(());
    }
    let assistant_retry_key = (
        entry.registration_id,
        entry.runtime_key.clone(),
        record.identity.clone(),
        session_id.clone(),
    );
    let assistant_retry_requested = inner
        .assistant_history_retries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&assistant_retry_key);
    let assistant_refresh_requested =
        assistant_refresh.permits_session(&session_id) || assistant_retry_requested;
    if assistant_refresh_requested {
        inner
            .assistant_history_retries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(assistant_retry_key.clone());
    }
    // Steady-state gate: read the session's committed boundary authority
    // from the durable runtime store BEFORE any read. If the last completed
    // backfill saw this same authority, nothing committed moved since: skip
    // the watermark read, the full-document session read, and the watermark
    // refresh write entirely. Reading it before the history keeps
    // invalidation safe: a commit racing this backfill moves the authority,
    // so the next pass re-reads.
    let boundary_authority = entry.runtime.session_boundary_authority(&session_id).await;
    if !force_refresh
        && !assistant_refresh_requested
        && let Some(authority) = boundary_authority.as_ref()
    {
        let authorities = inner
            .session_backfill_authorities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if authorities.get(&watermark_runtime_key) == Some(authority) {
            return Ok(());
        }
    }
    let watermark = inner
        .store
        .source_watermark(
            &watermark_runtime_key,
            ConsoleFrameSourceKind::SessionHistory,
        )
        .await?;
    let now_ms = current_time_ms();
    let mut offset = watermark
        .as_deref()
        .and_then(|watermark| parse_session_history_watermark(watermark, &session_id))
        .unwrap_or(0);
    if !force_refresh
        && !assistant_refresh_requested
        && watermark.as_deref().is_some_and(|watermark| {
            session_history_watermark_is_fresh(watermark, &session_id, now_ms)
        })
    {
        return Ok(());
    }
    // A bounded timeline window cannot establish absence. Scan existing exact
    // attempts before reading their runtime state and ONE complete current
    // history image. The cursor prevents this observation erasing later events.
    let mut notice_observation =
        observe_runtime_notice_attempts(&inner, &entry.runtime_key, &record.identity, &session_id)
            .await?;
    if !runtime_entry_is_current(&inner, &entry) {
        return Ok(());
    }
    let mut settled_attempts = entry
        .runtime
        .settled_notice_attempts(&session_id, &notice_observation.attempt_strings())
        .await;
    // Only a failure-free pass may admit the pre-read authority: recording it
    // on a failure would suppress retries until the next commit.
    let mut completed_cleanly = true;
    // A failure is distinct from an unsettled refresh: only a failure-free
    // unsettled pass may record its read authority below.
    let mut backfill_failed = false;
    let mut unsettled_refresh = false;
    loop {
        if !runtime_entry_is_current(&inner, &entry) {
            return Ok(());
        }
        // A complete read is not a commit barrier. An eligible refresh must
        // observe fresh typed idle status and no pending commit before reading.
        let assistant_refresh_gate =
            observe_refresh(&entry, &record, &session_id, assistant_refresh_requested).await;
        let assistant_history_settled = matches!(
            assistant_refresh_gate,
            assistant_history_refresh::AssistantHistoryRefreshGate::Settled
        );
        if !matches!(
            assistant_refresh_gate,
            assistant_history_refresh::AssistantHistoryRefreshGate::PositiveOnly
        ) && runtime_entry_is_current(&inner, &entry)
        {
            inner
                .assistant_history_retries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(assistant_retry_key.clone());
        }
        if matches!(
            assistant_refresh_gate,
            assistant_history_refresh::AssistantHistoryRefreshGate::Pending
                | assistant_history_refresh::AssistantHistoryRefreshGate::StatusUnknown
                | assistant_history_refresh::AssistantHistoryRefreshGate::Draining
                | assistant_history_refresh::AssistantHistoryRefreshGate::Running
        ) {
            completed_cleanly = false;
            unsettled_refresh = true;
        }
        if assistant_refresh_gate
            == assistant_history_refresh::AssistantHistoryRefreshGate::StatusUnknown
            && runtime_entry_is_current(&inner, &entry)
        {
            arm_status_unknown_redrive(
                &inner,
                assistant_retry_key.clone(),
                SessionBackfillTarget {
                    assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
                    provenance: None,
                    entry: entry.clone(),
                    record: record.clone(),
                    session_id: session_id.clone(),
                },
            );
        }
        // Durable work is still landing: the member's next typed change
        // re-drives the settled refresh, and the discovery loop reads again
        // once the committed boundary moves. A draining pass skips its read,
        // which would be read again once that work lands; a running pass
        // still restores positive frames.
        if matches!(
            assistant_refresh_gate,
            assistant_history_refresh::AssistantHistoryRefreshGate::Draining
                | assistant_history_refresh::AssistantHistoryRefreshGate::Running
        ) {
            if runtime_entry_is_current(&inner, &entry) {
                arm_status_unknown_redrive(
                    &inner,
                    assistant_retry_key.clone(),
                    SessionBackfillTarget {
                        assistant_refresh: AssistantHistoryRefreshReason::SessionBoundary {
                            session_id: session_id.clone(),
                        },
                        provenance: None,
                        entry: entry.clone(),
                        record: record.clone(),
                        session_id: session_id.clone(),
                    },
                );
            }
            if assistant_refresh_gate
                == assistant_history_refresh::AssistantHistoryRefreshGate::Draining
            {
                break;
            }
        }
        cache_notice_observation(
            &inner,
            (
                entry.runtime_key.clone(),
                record.identity.clone(),
                session_id.clone(),
            ),
            &notice_observation,
        );
        if assistant_history_settled
            && !notice_observation.history_index_complete
            && !notice_observation.verified_assistant_settled
        {
            // Settlement changed without a new transcript or event frontier.
            // Rebuild the bounded index before the owner read, then recheck
            // runtime status against that new cutoff. This needs one history
            // read, not a speculative read followed by the same read again.
            notice_observation = observe_runtime_notice_attempts_with_cache(
                &inner,
                &entry.runtime_key,
                &record.identity,
                &session_id,
                false,
            )
            .await?;
            settled_attempts = entry
                .runtime
                .settled_notice_attempts(&session_id, &notice_observation.attempt_strings())
                .await;
            continue;
        }
        // An unsettled refresh (the member is not idle yet, e.g. a staged or
        // running turn) can only restore positive frames. When the last
        // failure-free unsettled pass already read at this exact committed
        // boundary, re-reading the whole document every discovery tick adds
        // nothing: skip it and leave the retry armed, so the pass that
        // observes the member settled still performs its read. An explicit
        // forced refresh always reads.
        if !force_refresh
            && matches!(
                assistant_refresh_gate,
                assistant_history_refresh::AssistantHistoryRefreshGate::Pending
                    | assistant_history_refresh::AssistantHistoryRefreshGate::StatusUnknown
                    | assistant_history_refresh::AssistantHistoryRefreshGate::Running
            )
            && let Some(authority) = boundary_authority.as_ref()
            && inner
                .session_pending_read_authorities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&watermark_runtime_key)
                == Some(authority)
        {
            break;
        }
        let page = match entry
            .runtime
            .read_session_history(&session_id, 0, None)
            .await
        {
            Ok(page) => page,
            Err(err) => {
                record_session_history_backfill_failure(
                    &inner,
                    &watermark_runtime_key,
                    &entry,
                    &record.identity,
                    &session_id,
                    offset,
                    err.to_string(),
                )
                .await?;
                completed_cleanly = false;
                backfill_failed = true;
                break;
            }
        };
        let Some(mut page) = complete_current_history_page(&session_id, page) else {
            record_session_history_backfill_failure(
                &inner,
                &watermark_runtime_key,
                &entry,
                &record.identity,
                &session_id,
                offset,
                "session history did not return one complete current image".to_string(),
            )
            .await?;
            completed_cleanly = false;
            backfill_failed = true;
            break;
        };
        if !runtime_entry_is_current(&inner, &entry) {
            return Ok(());
        }
        let assistant_identity_history =
            history_has_assistant_identity(&notice_observation, &page.messages);
        notice_observation.full_history_positions |=
            assistant_identity_history && notice_observation.previous_snapshot.is_none();
        let reuse_verified_image =
            reuse_verified_history_image(&notice_observation, &page.messages, &settled_attempts)
                && (!assistant_history_settled || notice_observation.verified_assistant_settled);
        if !notice_observation.history_index_complete && !reuse_verified_image {
            // A changed image cannot use a truncated index as absence proof.
            // Rebuild once, then obtain settlement before a fresh history read
            // so the original observation ordering remains intact.
            notice_observation = observe_runtime_notice_attempts_with_cache(
                &inner,
                &entry.runtime_key,
                &record.identity,
                &session_id,
                false,
            )
            .await?;
            settled_attempts = entry
                .runtime
                .settled_notice_attempts(&session_id, &notice_observation.attempt_strings())
                .await;
            continue;
        }
        let mut mapped_history_cursors = BTreeSet::new();
        if !reuse_verified_image {
            restore_current_history_frames(
                &inner,
                &entry.runtime_key,
                &record.identity,
                &session_id,
                &mut notice_observation,
                &page.messages,
                Some(if assistant_identity_history {
                    0
                } else {
                    offset
                }),
            )
            .await?;
            if assistant_identity_history {
                if !assistant_history_prefix_is_current(
                    &inner,
                    &record.identity,
                    &notice_observation,
                )
                .await?
                {
                    return Err(std::io::Error::other(
                        "console history prefix changed before assistant snapshot publication; retry required",
                    ).into());
                }
                mapped_history_cursors = current_history_mapping(
                    &entry.runtime_key,
                    &record.identity,
                    &session_id,
                    &notice_observation,
                    &page.messages,
                )
                .0
                .into_iter()
                .filter_map(|position| position["source_cursor"].as_str().map(str::to_string))
                .collect();
            }
            if !runtime_entry_is_current(&inner, &entry) {
                return Ok(());
            }
            let notice_snapshot = runtime_notice_snapshot_frame(
                &entry.runtime_key,
                &record.identity,
                &session_id,
                &notice_observation,
                &settled_attempts,
                &page.messages,
            );
            let mut assistant_snapshot = assistant_history_settled
                .then(|| {
                    assistant_history_snapshot_frame(
                        &entry.runtime_key,
                        &record.identity,
                        &session_id,
                        &notice_observation,
                        &page.messages,
                    )
                })
                .flatten();
            // A changed position image needs a matching settled identity
            // boundary. An append-only identity update does not need another
            // position image; consumers retain the last full map and deltas.
            if assistant_identity_history
                && assistant_history_settled
                && notice_snapshot.is_some()
                && assistant_snapshot.is_none()
            {
                assistant_snapshot = assistant_history_snapshot_frame_inner(
                    &entry.runtime_key,
                    &record.identity,
                    &session_id,
                    &notice_observation,
                    &page.messages,
                    true,
                );
            }
            if let Some(mut snapshot) = notice_snapshot {
                snapshot.source.member_provenance = provenance.clone();
                append_and_emit_with_policy(&inner, snapshot, entry.visibility_policy.clone())
                    .await?;
            }
            if let Some(mut snapshot) = assistant_snapshot {
                if !runtime_entry_is_current(&inner, &entry)
                    || !assistant_history_prefix_is_current(
                        &inner,
                        &record.identity,
                        &notice_observation,
                    )
                    .await?
                {
                    return Err(std::io::Error::other(
                        "assistant history snapshot lost its registration or prefix witness; retry required",
                    ).into());
                }
                snapshot.source.member_provenance = provenance.clone();
                append_and_emit_with_policy(&inner, snapshot, entry.visibility_policy.clone())
                    .await?;
            }
            notice_observation.verified_assistant_settled = assistant_history_settled;
            if assistant_history_settled {
                inner
                    .assistant_history_retries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&assistant_retry_key);
            }
            cache_current_history_image(
                &inner,
                &entry.runtime_key,
                &record.identity,
                &session_id,
                &mut notice_observation,
                &page.messages,
                &settled_attempts,
            );
        }
        if reuse_verified_image && assistant_history_settled {
            inner
                .assistant_history_retries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&assistant_retry_key);
            cache_notice_observation(
                &inner,
                (
                    entry.runtime_key.clone(),
                    record.identity.clone(),
                    session_id.clone(),
                ),
                &notice_observation,
            );
        }
        // The snapshot observes the complete image, while ordinary append-only
        // backfill skips the prefix. Identity-aware images also project changed
        // rows at old positions, including rewrites that deliberately remove IDs.
        offset = if assistant_identity_history && !reuse_verified_image {
            0
        } else {
            offset.min(page.messages.len())
        };
        page.offset = offset;
        page.messages.drain(..offset);
        let page_value = match serde_json::to_value(page) {
            Ok(value) => value,
            Err(err) => {
                record_session_history_backfill_failure(
                    &inner,
                    &watermark_runtime_key,
                    &entry,
                    &record.identity,
                    &session_id,
                    offset,
                    err.to_string(),
                )
                .await?;
                completed_cleanly = false;
                backfill_failed = true;
                break;
            }
        };
        let base_offset = page_value
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(offset as u64) as usize;
        let Some(messages) = page_value.get("messages").and_then(Value::as_array) else {
            record_session_history_backfill_failure(
                &inner,
                &watermark_runtime_key,
                &entry,
                &record.identity,
                &session_id,
                offset,
                "session history page missing messages".to_string(),
            )
            .await?;
            completed_cleanly = false;
            backfill_failed = true;
            break;
        };
        if messages.is_empty() {
            if offset > 0 {
                record_session_history_watermark(
                    &inner,
                    &watermark_runtime_key,
                    &session_id,
                    offset,
                )
                .await?;
            }
            break;
        }
        for (idx, message) in messages.iter().enumerate() {
            let absolute_offset = base_offset + idx;
            let frames = frames_from_session_history_message_with_namespace(
                &entry.runtime_key,
                &record.identity,
                &entry.identity_namespace,
                &session_id,
                absolute_offset,
                message.clone(),
            );
            for mut frame in frames {
                // One member's history can derive a frame under another
                // identity: a spawn call's initial message is projected under
                // the member the call names, whether the spawn ran or was
                // refused. No member holds a runtime-plane identity: drop such
                // a frame and go on with the rest of the history.
                if crate::console_contracts::is_runtime_plane_identity(&frame.identity) {
                    crate::console_contracts::warn_runtime_plane_member_drop(
                        &record.runtime_member_id,
                        None,
                        &frame.identity,
                        "session history",
                    );
                    continue;
                }
                let current_canonical = assistant_identity_history
                    && frame.identity == record.identity
                    && frame.session_id.as_deref() == Some(session_id.as_str());
                if current_canonical
                    && frame
                        .source
                        .source_cursor
                        .as_ref()
                        .is_some_and(|cursor| mapped_history_cursors.contains(cursor))
                {
                    continue;
                }
                frame.source.member_provenance = if frame.identity == record.identity {
                    provenance.clone()
                } else {
                    member_provenance_for_identity(
                        &inner,
                        &entry,
                        &frame.identity,
                        frame.session_id.as_deref(),
                        false,
                    )
                    .await
                };
                if !current_canonical
                    && history_frame_has_existing_counterpart(&inner, &frame).await?
                {
                    continue;
                }
                append_and_emit_with_policy(&inner, frame, entry.visibility_policy.clone()).await?;
            }
        }
        let Some(next_offset) = advance_backfill_offset(offset, base_offset, messages.len()) else {
            // Forward-progress guard: a session store that returns a page whose
            // (base_offset + len) does not advance past the offset we requested
            // — e.g. a cursor that resets to 0 while still reporting has_more —
            // would make this loop re-read the same page forever, re-recording
            // the same watermark (a livelock observed on an orphaned gateway).
            // Stop instead of spinning; the last good watermark stands.
            tracing::warn!(
                session_id = %session_id,
                requested_offset = offset,
                page_base_offset = base_offset,
                page_len = messages.len(),
                "session history backfill made no forward progress; stopping to avoid a livelock"
            );
            break;
        };
        offset = next_offset;
        record_session_history_watermark(&inner, &watermark_runtime_key, &session_id, offset)
            .await?;
        let has_more = page_value
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_more || messages.len() < SESSION_HISTORY_PAGE_LIMIT {
            break;
        }
    }
    if !completed_cleanly
        && unsettled_refresh
        && !backfill_failed
        && let Some(authority) = boundary_authority.clone()
    {
        let registrations = inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?;
        if registrations
            .get(&entry.runtime_key)
            .is_some_and(|current| current.registration_id == entry.registration_id)
        {
            inner
                .session_pending_read_authorities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(watermark_runtime_key.clone(), authority);
        }
    }
    if completed_cleanly && let Some(authority) = boundary_authority {
        let registered = {
            let registrations = inner
                .runtimes
                .read()
                .map_err(|_| runtime_registry_lock_error())?;
            registrations
                .get(&entry.runtime_key)
                .is_some_and(|current| current.registration_id == entry.registration_id)
        };
        if !registered {
            return Ok(());
        }
        inner
            .session_backfill_authorities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(watermark_runtime_key, authority.clone());
        // The image this pass verified is older than the durable document if
        // the session's committed boundary moved during the read: re-read
        // now, not on a later tick once the fresh watermark expires.
        if entry
            .runtime
            .session_boundary_authority(&session_id)
            .await
            .is_some_and(|now| now != authority)
        {
            arm_moved_boundary_redrive(
                &inner,
                assistant_retry_key,
                SessionBackfillTarget {
                    assistant_refresh,
                    provenance: None,
                    entry,
                    record,
                    session_id,
                },
            );
        }
    }
    Ok(())
}

/// Compute the next session-history backfill offset, enforcing strict forward
/// progress. Returns `None` when the page (`base_offset + page_len`) does not
/// advance past `requested_offset` — a non-advancing/looping cursor — which
/// signals the caller to stop rather than re-read the same page forever.
fn advance_backfill_offset(
    requested_offset: usize,
    base_offset: usize,
    page_len: usize,
) -> Option<usize> {
    let next = base_offset.saturating_add(page_len);
    (next > requested_offset).then_some(next)
}

async fn record_session_history_watermark(
    inner: &AggregatorInner,
    watermark_runtime_key: &str,
    session_id: &str,
    offset: usize,
) -> ConsoleLogResult<()> {
    inner
        .store
        .record_source_watermark(
            watermark_runtime_key,
            ConsoleFrameSourceKind::SessionHistory,
            &format_session_history_watermark(session_id, offset, current_time_ms()),
        )
        .await
}

fn spawn_session_history_backfill(
    inner: Arc<AggregatorInner>,
    runtime_key: String,
    mut assistant_refresh: AssistantHistoryRefreshReason,
) {
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    let activity = BackfillActivity::begin(&inner);
    tokio::spawn(async move {
        let _activity = activity;
        {
            let mut active = inner.active_session_backfills.lock().await;
            if !assistant_history_refresh::admit_runtime_refresh(
                &mut active,
                &runtime_key,
                assistant_refresh.clone(),
            ) {
                return;
            }
        }
        loop {
            if let Err(err) = Box::pin(backfill_session_history(
                inner.clone(),
                runtime_key.clone(),
                false,
                assistant_refresh,
            ))
            .await
            {
                tracing::warn!(runtime_key = %runtime_key, error = %err,
                    "console session-history backfill failed");
            }
            let mut active = inner.active_session_backfills.lock().await;
            if let Some(pending) = active.get_mut(&runtime_key).and_then(Option::take) {
                assistant_refresh = pending;
                drop(active);
                continue;
            }
            active.remove(&runtime_key);
            break;
        }
    });
}

fn spawn_session_history_discovery_loop(inner: Arc<AggregatorInner>, runtime_key: String) {
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SESSION_HISTORY_DISCOVERY_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let runtime_still_registered = inner
                .runtimes
                .read()
                .is_ok_and(|entries| entries.contains_key(&runtime_key));
            if !runtime_still_registered {
                break;
            }
            spawn_session_history_backfill(
                inner.clone(),
                runtime_key.clone(),
                AssistantHistoryRefreshReason::PositiveOnly,
            );
        }
    });
}

fn spawn_session_history_backfill_target(
    inner: Arc<AggregatorInner>,
    target: SessionBackfillTarget,
    force_refresh: bool,
) {
    let (inner, target) = if let Some(owner) = inner.projection_owner.as_ref() {
        let Some(entry) = owner
            .runtimes
            .read()
            .ok()
            .and_then(|entries| entries.get(&target.entry.runtime_key).cloned())
        else {
            return;
        };
        (owner.clone(), SessionBackfillTarget { entry, ..target })
    } else {
        (inner, target)
    };
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    let activity = BackfillActivity::begin(&inner);
    tokio::spawn(async move {
        let _activity = activity;
        let active_key = targeted_session_history_active_key(&target, force_refresh);
        let result =
            run_targeted_session_history_backfill(inner.clone(), target, force_refresh).await;
        if let Err(err) = result {
            tracing::warn!(
                active_key = %active_key,
                error = %err,
                "console targeted session-history backfill failed"
            );
        }
    });
}

/// Re-drive a retained assistant-history retry whose gate read found no fresh
/// typed status (`StatusUnknown`): the bounded status read did not answer, for
/// example under contention, while the member may already be idle. An idle
/// member emits no run or commit event of its own, so without this only the
/// discovery tick would retry it. One waiter per retry key wakes on the
/// member's next event or the primary mob's next machine-state change and
/// runs one targeted backfill; the retained retry key makes that pass request
/// the assistant refresh again. The discovery loop stays the backup (a
/// member outside the primary mob, or a mob with no further activity).
/// One spawned or armed session-history backfill job. It counts from the
/// moment it is spawned, before its task first runs, until it finishes, so a
/// convergence wait started right after registration already sees the
/// registration's recovery pass.
struct BackfillActivity(tokio::sync::watch::Sender<usize>);

impl BackfillActivity {
    fn begin(inner: &Arc<AggregatorInner>) -> Self {
        let sender = backfill_owner(inner).backfill_activity.clone();
        sender.send_modify(|in_flight| *in_flight += 1);
        Self(sender)
    }
}

impl Drop for BackfillActivity {
    fn drop(&mut self) {
        self.0
            .send_modify(|in_flight| *in_flight = in_flight.saturating_sub(1));
    }
}

/// The aggregator that owns session-history projection for `inner`.
fn backfill_owner(inner: &Arc<AggregatorInner>) -> &Arc<AggregatorInner> {
    inner.projection_owner.as_ref().unwrap_or(inner)
}

/// Queue one re-read for a completed pass whose session's committed boundary
/// moved during its read: the image it verified is already behind the
/// durable document. The re-drive reads at once (forced past the authority
/// and watermark gates, which would otherwise hold it until a discovery tick
/// and watermark expiry) and runs only because the boundary moved, so it
/// cannot loop: a pass that ends at the boundary it read queues nothing.
fn arm_moved_boundary_redrive(
    inner: &Arc<AggregatorInner>,
    retry_key: (uuid::Uuid, String, String, String),
    target: SessionBackfillTarget,
) {
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    if !inner
        .moved_boundary_redrives
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(retry_key.clone())
    {
        return;
    }
    let redrive_inner = Arc::clone(inner);
    let activity = BackfillActivity::begin(inner);
    let task = tokio::spawn(async move {
        let _activity = activity;
        redrive_inner
            .moved_boundary_redrives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&retry_key);
        if runtime_entry_is_current(&redrive_inner, &target.entry)
            && let Err(error) = Box::pin(run_targeted_session_history_backfill(
                redrive_inner,
                target,
                true,
            ))
            .await
        {
            tracing::warn!(error = %error, "console moved-boundary session-history re-drive failed");
        }
    });
    #[cfg(test)]
    inner
        .moved_boundary_redrive_tasks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(task);
    #[cfg(not(test))]
    drop(task);
}

fn arm_status_unknown_redrive(
    inner: &Arc<AggregatorInner>,
    retry_key: (uuid::Uuid, String, String, String),
    target: SessionBackfillTarget,
) {
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    if !inner
        .status_unknown_redrives
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(retry_key.clone())
    {
        return;
    }
    let handle = target.entry.runtime.handle();
    let member =
        crate::member_comms_id::roster_member_id_for_supplied_id(&target.record.runtime_member_id);
    let mut changes = handle.machine_state_changes();
    // The cloned receiver starts from the handle's last-seen version: consume
    // the change it has not seen yet, so only a change after this arm wakes
    // the waiter (otherwise a persisting unknown status would re-drive
    // back-to-back instead of on the next typed change).
    let _ = futures::FutureExt::now_or_never(changes.changed());
    let redrive_inner = Arc::clone(inner);
    let activity = BackfillActivity::begin(inner);
    let task = tokio::spawn(async move {
        let _activity = activity;
        use futures::StreamExt as _;
        let woke = match handle.subscribe_agent_events(&member).await {
            Ok(mut events) => tokio::select! {
                event = events.next() => event.is_some(),
                changed = changes.changed() => changed.is_ok(),
            },
            // Not a member of the primary mob (or not live there): the mob's
            // own changes still wake it.
            Err(_) => changes.changed().await.is_ok(),
        };
        redrive_inner
            .status_unknown_redrives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&retry_key);
        if woke
            && runtime_entry_is_current(&redrive_inner, &target.entry)
            && let Err(error) =
                run_targeted_session_history_backfill(redrive_inner, target, false).await
        {
            tracing::warn!(
                error = %error,
                "console status-unknown assistant-history re-drive failed"
            );
        }
    });
    #[cfg(test)]
    inner
        .status_unknown_redrive_tasks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(task);
    #[cfg(not(test))]
    drop(task);
}

async fn run_targeted_session_history_backfill(
    inner: Arc<AggregatorInner>,
    mut target: SessionBackfillTarget,
    force_refresh: bool,
) -> ConsoleLogResult<()> {
    let active_key = targeted_session_history_active_key(&target, force_refresh);
    {
        let mut active = inner.targeted_session_backfills.lock().await;
        // Check while admitting under the scheduler lock: an old request must
        // never replace a pending target belonging to a newer registration.
        let registrations = inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?;
        if registrations
            .get(&target.entry.runtime_key)
            .is_none_or(|entry| entry.registration_id != target.entry.registration_id)
        {
            return Ok(());
        }
        match active.entry(active_key.clone()) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if force_refresh || target.assistant_refresh.permits_session(&target.session_id) {
                    if !target.assistant_refresh.permits_session(&target.session_id)
                        && let Some(previous) = entry.get().as_ref()
                        && previous.entry.registration_id == target.entry.registration_id
                        && previous.record.identity == target.record.identity
                        && previous
                            .assistant_refresh
                            .permits_session(&target.session_id)
                    {
                        target.assistant_refresh = previous.assistant_refresh.clone();
                    }
                    entry.insert(Some(target));
                }
                return Ok(());
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(None);
            }
        }
    }
    let mut first_error = None;
    loop {
        if let Err(error) = backfill_one_session_history(inner.clone(), target, force_refresh).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        let mut active = inner.targeted_session_backfills.lock().await;
        if let Some(pending) = active.get_mut(&active_key).and_then(Option::take) {
            target = pending;
            // A burst during this pass requests one trailing forced read. New
            // triggers during that read can request another, without polling
            // or retaining one job per trigger.
            drop(active);
            continue;
        }
        active.remove(&active_key);
        return first_error.map_or(Ok(()), Err);
    }
}

fn targeted_session_history_active_key(
    target: &SessionBackfillTarget,
    force_refresh: bool,
) -> String {
    let mode = if force_refresh { "force" } else { "refresh" };
    format!(
        "{}:session-history:{}:{mode}",
        target.entry.runtime_key, target.session_id
    )
}

fn spawn_session_history_backfill_for_identity(
    inner: Arc<AggregatorInner>,
    identity: String,
    force_refresh: bool,
    assistant_refresh: AssistantHistoryRefreshReason,
) {
    let inner = inner.projection_owner.as_ref().unwrap_or(&inner).clone();
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    let activity = BackfillActivity::begin(&inner);
    tokio::spawn(async move {
        let _activity = activity;
        for mut target in Box::pin(session_backfill_targets_for_identity(&inner, &identity)).await {
            target.assistant_refresh = assistant_refresh.clone();
            spawn_session_history_backfill_target(inner.clone(), target, force_refresh);
        }
    });
}

fn spawn_opportunistic_session_history_backfill_for_identity(
    inner: Arc<AggregatorInner>,
    identity: String,
) {
    let inner = inner.projection_owner.as_ref().unwrap_or(&inner).clone();
    if !inner.options.session_history_backfill_enabled {
        return;
    }
    let activity = BackfillActivity::begin(&inner);
    tokio::spawn(async move {
        let _activity = activity;
        for target in Box::pin(session_backfill_targets_for_identity(&inner, &identity)).await {
            let active_key = format!(
                "{}:session-history:{}",
                target.entry.runtime_key, target.session_id
            );
            {
                let mut seen = inner.opportunistic_session_backfills.lock().await;
                if !seen.insert(active_key) {
                    continue;
                }
            }
            spawn_session_history_backfill_target(inner.clone(), target, false);
        }
    });
}

#[cfg(test)]
async fn session_backfill_target_for_identity(
    inner: &AggregatorInner,
    identity: &str,
) -> Option<SessionBackfillTarget> {
    session_backfill_targets_for_identity(inner, identity)
        .await
        .into_iter()
        .next()
}

async fn session_backfill_targets_for_identity(
    inner: &AggregatorInner,
    identity: &str,
) -> Vec<SessionBackfillTarget> {
    let entries = inner
        .runtimes
        .read()
        .ok()
        .map(|entries| entries.clone())
        .unwrap_or_default();
    let mut targets = Vec::new();
    for entry in entries.values() {
        let raw_identities = namespace_match_candidates(identity, &entry.identity_namespace);
        if raw_identities.is_empty() {
            continue;
        }
        let mids = raw_identities
            .iter()
            .map(|raw_identity| crate::member_comms_id::mob_member_id(raw_identity.as_str()))
            .collect::<Vec<_>>();
        for resolved in Box::pin(member_sources_for_entry(entry))
            .await
            .into_iter()
            .filter(|candidate| {
                resolved_member_matches_raw_identities(candidate, &raw_identities, &mids)
            })
        {
            let Some(record) = identity_record_for_resolved_member(&resolved).await else {
                continue;
            };
            if runtime_plane_history_target(&record) {
                continue;
            }
            if !Box::pin(resolved_member_visible(&resolved, &record)).await {
                continue;
            }
            let Some(session_id) = record.session_id.clone() else {
                continue;
            };
            let provenance =
                Some(Box::pin(capture_member_provenance(inner, &resolved, &record)).await);
            targets.push(SessionBackfillTarget {
                assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
                provenance,
                entry: entry.clone(),
                record,
                session_id,
            });
        }
    }
    targets
}

fn resolved_member_matches_raw_identities(
    resolved: &ResolvedConsoleMember,
    raw_identities: &[String],
    mids: &[AgentIdentity],
) -> bool {
    mids.contains(&resolved.member.agent_identity)
        || crate::member_comms_id::durable_identity_label(&resolved.member.labels)
            .is_some_and(|agent_identity| raw_identities.iter().any(|raw| agent_identity == raw))
}

async fn recover_lagged_source_events(
    inner: Arc<AggregatorInner>,
    runtime_key: &str,
    console_events: &ConsoleEventStore,
) -> ConsoleLogResult<()> {
    let policy = capture_host_payload_policy(&inner, runtime_key, None)?;
    let watermark = inner
        .store
        .source_watermark(runtime_key, ConsoleFrameSourceKind::ConsoleEvent)
        .await?;
    match console_events.replay_all(watermark.as_deref()).await {
        Ok(events) => {
            for envelope in events {
                project_console_event(inner.clone(), runtime_key, envelope).await?;
            }
        }
        Err(err) => {
            append_source_gap(
                &inner,
                runtime_key,
                format!(
                    "{}:{}:{}",
                    err.error, err.stream, err.requested_last_event_id
                ),
                policy,
            )
            .await?;
        }
    }
    Ok(())
}

async fn append_source_gap(
    inner: &AggregatorInner,
    runtime_key: &str,
    reason: String,
    policy: Arc<dyn ConsoleVisibilityPolicy>,
) -> ConsoleLogResult<()> {
    append_and_emit_runtime_plane(
        inner,
        NewConsoleFrame {
            id: None,
            dedupe_key: format!("source-gap:{runtime_key}:{}", current_time_ms()),
            timestamp_ms: current_time_ms(),
            runtime_key: runtime_key.to_string(),
            identity: crate::console_contracts::CONSOLE_GAP_IDENTITY.to_string(),
            conversation_id: None,
            session_id: None,
            kind: "replay_unavailable".to_string(),
            status: ConsoleFrameStatus::DeliveryFailed,
            payload: json!({
                "reason": reason,
                "source_kind": "console_event",
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Synthetic,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        },
        policy,
    )
    .await?;
    let _ = inner
        .event_tx
        .send(ConsoleTimelineEvent::ReplayUnavailable {
            requested_cursor: format!("source-gap:{runtime_key}"),
            latest_cursor: inner.store.latest_cursor().await.ok().flatten(),
        });
    Ok(())
}

/// Record a session-history backfill failure: stamp a watermark at the current
/// offset so the freshness gate throttles retries to once per
/// `SESSION_HISTORY_REFRESH_TTL_MS` (without it the 5s discovery loop re-reads
/// a persistently-failing session forever), and append a single gap frame.
async fn record_session_history_backfill_failure(
    inner: &AggregatorInner,
    watermark_runtime_key: &str,
    entry: &RuntimeEntry,
    identity: &str,
    session_id: &str,
    offset: usize,
    reason: String,
) -> ConsoleLogResult<()> {
    record_session_history_watermark(inner, watermark_runtime_key, session_id, offset).await?;
    append_backfill_gap(inner, entry, identity, session_id, reason).await
}

/// Stable dedupe key for a session-history backfill gap frame: a persistently
/// failing session collapses to a single gap frame across retries.
fn backfill_gap_dedupe_key(runtime_key: &str, identity: &str, session_id: &str) -> String {
    format!("session-backfill-gap:{runtime_key}:{identity}:{session_id}")
}

async fn append_backfill_gap(
    inner: &AggregatorInner,
    entry: &RuntimeEntry,
    identity: &str,
    session_id: &str,
    reason: String,
) -> ConsoleLogResult<()> {
    let runtime_key = &entry.runtime_key;
    append_and_emit_with_policy(
        inner,
        NewConsoleFrame {
            id: None,
            // Stable per (runtime, identity, session): repeated failures for
            // the same session collapse to one gap frame instead of minting a
            // fresh one (timestamped) on every retry — which grew the timeline
            // store unbounded.
            dedupe_key: backfill_gap_dedupe_key(runtime_key, identity, session_id),
            timestamp_ms: current_time_ms(),
            runtime_key: runtime_key.clone(),
            identity: identity.to_string(),
            conversation_id: Some(identity.to_string()),
            session_id: Some(session_id.to_string()),
            kind: "replay_unavailable".to_string(),
            status: ConsoleFrameStatus::DeliveryFailed,
            payload: json!({
                "reason": reason,
                "source_kind": "session_history",
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Synthetic,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        },
        entry.visibility_policy.clone(),
    )
    .await?;
    Ok(())
}

async fn project_console_event(
    inner: Arc<AggregatorInner>,
    runtime_key: &str,
    envelope: crate::console_contracts::ConsoleIdentityEventEnvelope,
) -> ConsoleLogResult<()> {
    let Some(entry) = inner
        .runtimes
        .read()
        .ok()
        .and_then(|entries| entries.get(runtime_key).cloned())
    else {
        return Ok(());
    };
    let mut frame = frame_from_console_event(&entry, envelope);
    retain_legacy_reasoning_replay_key(inner.store.as_ref(), &mut frame).await?;
    frame.source.member_provenance = member_provenance_for_identity(
        &inner,
        &entry,
        &frame.identity,
        frame.session_id.as_deref(),
        matches!(frame.kind.as_str(), "interaction_started" | "run_started"),
    )
    .await;
    let source_cursor = frame
        .source_event_id
        .clone()
        .unwrap_or_else(|| frame.dedupe_key.clone());
    let assistant_refresh = AssistantHistoryRefreshReason::from_event(
        &frame.kind,
        frame.session_id.as_deref(),
        &frame.payload,
    );
    let refresh_identity = if console_event_should_refresh_session_history(&frame) {
        Some(frame.identity.clone())
    } else {
        None
    };
    let application_refresh_identity =
        (frame.kind == "turn_started").then(|| frame.identity.clone());
    let opportunistic_refresh_identity = if refresh_identity.is_none()
        && console_event_should_start_session_history_backfill(&frame)
    {
        Some(frame.identity.clone())
    } else {
        None
    };
    if frame.identity == SYSTEM_EVENT_IDENTITY {
        // The runtime's event store writes only its own events under the
        // system identity; it refuses every member event under one.
        append_and_emit_runtime_plane(&inner, frame, entry.visibility_policy.clone()).await?;
    } else if crate::console_contracts::is_runtime_plane_identity(&frame.identity) {
        // No runtime event carries the gap-marker identity: drop it, and
        // still advance past it below.
        crate::console_contracts::warn_runtime_plane_member_drop(
            &frame.identity,
            None,
            &frame.identity,
            "runtime console events",
        );
    } else {
        append_and_emit_with_policy(&inner, frame, entry.visibility_policy.clone()).await?;
    }
    inner
        .store
        .record_source_watermark(
            &entry.runtime_key,
            ConsoleFrameSourceKind::ConsoleEvent,
            &source_cursor,
        )
        .await?;
    if let Some(identity) = refresh_identity {
        spawn_session_history_backfill_for_identity(
            inner.clone(),
            identity,
            true,
            assistant_refresh,
        );
    } else if let Some(identity) = opportunistic_refresh_identity {
        spawn_opportunistic_session_history_backfill_for_identity(inner.clone(), identity);
    }
    if let Some(identity) = application_refresh_identity {
        tool_application_projection::spawn_for_identity(inner, identity);
    }
    Ok(())
}

fn console_event_should_refresh_session_history(frame: &NewConsoleFrame) -> bool {
    matches!(
        frame.kind.as_str(),
        "interaction_complete"
            | "interaction_failed"
            | "message_delivery_failed"
            | "run_completed"
            | "run_failed"
            | "run_cancelled"
            | "boundary_append_applied"
            | "boundary_appends_discarded"
            | "transcript_rewrite_committed"
            | "transcript_rewrite_audit_receipt_committed"
            | "compaction_completed"
            | "extraction_succeeded"
            | "extraction_failed"
            | "stream_truncated"
    )
}

fn console_event_should_start_session_history_backfill(frame: &NewConsoleFrame) -> bool {
    if crate::console_contracts::is_runtime_plane_identity(&frame.identity) {
        return false;
    }
    matches!(
        frame.kind.as_str(),
        "turn_started"
            | "run_started"
            | "reasoning_complete"
            | "tool_call_requested"
            | "tool_call"
            | "tool_execution_started"
            | "text_delta"
            | "system_notice"
    )
}

fn capture_host_payload_policy(
    inner: &AggregatorInner,
    runtime_key: &str,
    expected_registration: Option<uuid::Uuid>,
) -> ConsoleLogResult<Arc<dyn ConsoleVisibilityPolicy>> {
    let owner = inner.projection_owner.as_deref().unwrap_or(inner);
    let entries = owner
        .runtimes
        .read()
        .map_err(|_| runtime_registry_lock_error())?;
    if let Some(entry) = entries.get(runtime_key) {
        owner.has_registered_runtime.store(true, Ordering::Release);
        if expected_registration.is_none_or(|id| entry.registration_id == id) {
            return Ok(entry.visibility_policy.clone());
        }
    } else if entries.is_empty() && !owner.has_registered_runtime.load(Ordering::Acquire) {
        // Standalone aggregators can reserve identity-first sends before any
        // runtime is attached. Unregistered producers never use this fallback.
        return Ok(Arc::new(AllowAllConsoleVisibilityPolicy));
    }
    Err(Box::new(std::io::Error::other(
        "console producer registration is no longer current",
    )))
}

fn redact_frame(policy: &dyn ConsoleVisibilityPolicy, frame: &mut NewConsoleFrame) {
    if let Some(payload) = policy.redact_payload(frame) {
        frame.payload = payload;
        frame.status = ConsoleFrameStatus::Redacted;
    }
}

async fn append_and_emit(
    inner: &AggregatorInner,
    frame: NewConsoleFrame,
) -> ConsoleLogResult<AppendOutcome> {
    let policy = capture_host_payload_policy(inner, &frame.runtime_key, None)?;
    append_and_emit_with_policy(inner, frame, policy).await
}

/// Append a frame attributed to a member. A runtime-plane identity is
/// refused: the console treats frames under one as the runtime's own, and
/// only [`append_and_emit_runtime_plane`] writes those.
async fn append_and_emit_with_policy(
    inner: &AggregatorInner,
    frame: NewConsoleFrame,
    policy: Arc<dyn ConsoleVisibilityPolicy>,
) -> ConsoleLogResult<AppendOutcome> {
    if crate::console_contracts::is_runtime_plane_identity(&frame.identity) {
        return Err(format!(
            "refusing a member frame under the runtime-plane identity '{}'",
            frame.identity
        )
        .into());
    }
    append_and_emit_frame(inner, frame, policy).await
}

/// Append one of the runtime's own frames under a runtime-plane identity: a
/// source-gap marker, or a runtime event the runtime's event store recorded
/// under the system identity.
async fn append_and_emit_runtime_plane(
    inner: &AggregatorInner,
    frame: NewConsoleFrame,
    policy: Arc<dyn ConsoleVisibilityPolicy>,
) -> ConsoleLogResult<AppendOutcome> {
    append_and_emit_frame(inner, frame, policy).await
}

async fn append_and_emit_frame(
    inner: &AggregatorInner,
    mut frame: NewConsoleFrame,
    policy: Arc<dyn ConsoleVisibilityPolicy>,
) -> ConsoleLogResult<AppendOutcome> {
    if frame.source.member_provenance.is_none() {
        let owner = inner.projection_owner.as_deref().unwrap_or(inner);
        let entry = owner
            .runtimes
            .read()
            .ok()
            .and_then(|entries| entries.get(&frame.runtime_key).cloned());
        if let Some(entry) = entry {
            frame.source.member_provenance = member_provenance_for_identity(
                owner,
                &entry,
                &frame.identity,
                frame.session_id.as_deref(),
                false,
            )
            .await;
        }
    }
    redact_frame(policy.as_ref(), &mut frame);
    let outcome = inner.store.append_if_absent(frame).await?;
    if outcome.disposition == AppendDisposition::Inserted {
        let _ = inner.event_tx.send(ConsoleTimelineEvent::ConsoleFrame {
            frame: outcome.frame.clone(),
        });
    }
    Ok(outcome)
}

async fn update_frame_status_and_emit(
    inner: &AggregatorInner,
    frame_id: &str,
    status: ConsoleFrameStatus,
) -> ConsoleLogResult<Option<ConsoleFrame>> {
    let Some(updated) = inner.store.update_frame_status(frame_id, status).await? else {
        return Ok(None);
    };
    let update_marker = NewConsoleFrame {
        id: None,
        dedupe_key: format!("frame-update:{}:{}", updated.id, updated.frame_version),
        timestamp_ms: updated.updated_at_ms.unwrap_or_else(current_time_ms),
        runtime_key: updated.runtime_key.clone(),
        identity: updated.identity.clone(),
        conversation_id: updated.conversation_id.clone(),
        session_id: updated.session_id.clone(),
        kind: "frame_updated".to_string(),
        status: updated.status,
        payload: json!({ "frame": updated.clone() }),
        source: ConsoleFrameSource {
            member_provenance: updated.source.member_provenance.clone(),
            kind: ConsoleFrameSourceKind::Synthetic,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: updated.interaction_id.clone(),
        turn_id: updated.turn_id.clone(),
        run_id: updated.run_id.clone(),
        parent_frame_id: Some(updated.id.clone()),
        caused_by_frame_id: Some(updated.id.clone()),
    };
    let outcome = inner.store.append_if_absent(update_marker).await?;
    if outcome.disposition == AppendDisposition::Inserted {
        let _ = inner.event_tx.send(ConsoleTimelineEvent::ConsoleFrame {
            frame: outcome.frame,
        });
    }
    Ok(Some(updated))
}

// Older logs keyed reasoning by text. Reuse that key only for the exact
// already-stored source event, so replay cannot collide with its unique row ID.
// Equal fragments from different events must retain their new event keys.
async fn retain_legacy_reasoning_replay_key(
    store: &dyn ConsoleLogStore,
    frame: &mut NewConsoleFrame,
) -> ConsoleLogResult<()> {
    let Some(text) = reasoning_payload_text(&frame.kind, &frame.payload) else {
        return Ok(());
    };
    let Some(event_id) = frame.source_event_id.as_deref() else {
        return Ok(());
    };
    let scope = frame
        .interaction_id
        .as_deref()
        .or(frame.turn_id.as_deref())
        .or(frame.run_id.as_deref())
        .unwrap_or(event_id);
    let legacy_key = format!(
        "console-reasoning:{}:{}:{}",
        frame.runtime_key,
        scope,
        hash_short(&normalize_transcript_fingerprint_text(text))
    );
    if let Some(existing) = store.frame_by_dedupe_key(&legacy_key).await?
        && Some(existing.id.as_str()) == frame.id.as_deref()
        && existing.source_event_id == frame.source_event_id
        && existing.runtime_key == frame.runtime_key
        && existing.identity == frame.identity
        && existing.kind == frame.kind
        && existing.source.kind == ConsoleFrameSourceKind::ConsoleEvent
    {
        frame.dedupe_key = legacy_key;
    }
    Ok(())
}

fn frame_from_console_event(
    entry: &RuntimeEntry,
    envelope: crate::console_contracts::ConsoleIdentityEventEnvelope,
) -> NewConsoleFrame {
    let event_id = envelope.event_id;
    let envelope_interaction_id = envelope.interaction_id;
    let event_type = envelope.event_type;
    let payload = envelope.data;
    let turn_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let lineage = payload
        .get("identity")
        .cloned()
        .and_then(|value| {
            serde_json::from_value::<meerkat_core::types::TranscriptMessageIdentity>(value).ok()
        })
        .filter(|identity| !identity.is_empty());
    let has_lineage = payload.get("identity").is_some_and(|value| {
        !value.is_null() && value.as_object().is_none_or(|value| !value.is_empty())
    });
    let interaction_id = match lineage.as_ref() {
        Some(identity) => identity.interaction_id.map(|id| id.to_string()),
        None if !has_lineage => envelope_interaction_id,
        None => None,
    };
    let run_id = match lineage.as_ref() {
        Some(identity) => identity.run_id.as_ref().map(ToString::to_string),
        None if !has_lineage => payload
            .get("run_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        None => None,
    };
    let status = match event_type.as_str() {
        "interaction_started" => ConsoleFrameStatus::Accepted,
        "interaction_failed" | "run_failed" => ConsoleFrameStatus::DeliveryFailed,
        "interaction_complete" | "run_completed" => ConsoleFrameStatus::Completed,
        _ => ConsoleFrameStatus::Delivered,
    };
    // The system identity is runtime-plane, not agent-plane: namespacing it
    // would defeat the global-timeline visibility exemption in
    // `frame_is_visible_cached` on namespaced runtimes (memory.* events
    // attributed to `_system` would become `ns/_system` and vanish again).
    let identity = if envelope.identity == SYSTEM_EVENT_IDENTITY {
        envelope.identity
    } else {
        apply_namespace(&envelope.identity, &entry.identity_namespace)
    };
    // Reasoning deltas are fragments: identical words may occur repeatedly
    // within one turn. Only a replay of the same source event is a duplicate.
    let dedupe_key = format!("console-event:{}:{}", entry.runtime_key, event_id);
    NewConsoleFrame {
        id: Some(event_id.clone()),
        dedupe_key,
        timestamp_ms: envelope.timestamp_ms,
        runtime_key: entry.runtime_key.clone(),
        identity: identity.clone(),
        conversation_id: Some(identity),
        session_id: payload
            .get("session_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        kind: event_type,
        status,
        payload,
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: Some(event_id),
        interaction_id,
        turn_id,
        run_id,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

#[cfg(test)]
fn frame_from_session_history_message(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    offset: usize,
    message: Value,
) -> Option<NewConsoleFrame> {
    frames_from_session_history_message(runtime_key, identity, session_id, offset, message)
        .into_iter()
        .next()
}

#[cfg(test)]
fn frames_from_session_history_message(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    offset: usize,
    message: Value,
) -> Vec<NewConsoleFrame> {
    frames_from_session_history_message_with_namespace(
        runtime_key,
        identity,
        "",
        session_id,
        offset,
        message,
    )
}

/// Derive `assistant_image` frames from a session-history tool result's text,
/// mirroring the live ingest edge (`parse_generate_image_tool_result` ->
/// per-image `assistant_image` envelopes). The serialized
/// `ImageGenerationToolResult` is self-describing, so — unlike the live path —
/// no tool name is needed: we parse the result text directly. Returns an empty
/// vec for non-image tool results. The console dedupes images by blob/reference,
/// so an overlap with a live `assistant_image` frame collapses to one image.
#[allow(clippy::too_many_arguments)]
fn session_history_assistant_image_frames(
    runtime_key: &str,
    identity: &str,
    session_id: &str,
    offset: usize,
    result_idx: usize,
    timestamp_ms: u64,
    tool_use_id: &str,
    result_text: &str,
) -> Vec<NewConsoleFrame> {
    let Ok(image_result) = serde_json::from_str::<
        meerkat_core::image_generation::ImageGenerationToolResult,
    >(result_text) else {
        return Vec::new();
    };
    image_result
        .images
        .iter()
        .enumerate()
        .map(|(image_idx, image)| NewConsoleFrame {
            id: None,
            // Key on the stable image identity so a backfill frame and any live
            // `assistant_image` frame for the same image collapse on dedupe.
            dedupe_key: format!(
                "console-assistant-image:{runtime_key}:{}",
                image.blob_ref.blob_id
            ),
            timestamp_ms,
            runtime_key: runtime_key.to_string(),
            identity: identity.to_string(),
            conversation_id: Some(identity.to_string()),
            session_id: Some(session_id.to_string()),
            kind: "assistant_image".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({
                "source_event_type": "session_history",
                "tool_call_id": tool_use_id,
                "image_id": image.image_id.0.to_string(),
                "blob_id": image.blob_ref.blob_id,
                "media_type": image.media_type.as_str(),
                "width": image.width,
                "height": image.height,
                "revised_prompt": image_result.revised_prompt.clone(),
                "type": "session_history",
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::SessionHistory,
                source_cursor: Some(format!(
                    "{session_id}:{offset}:{result_idx}:image:{image_idx}"
                )),
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        })
        .collect()
}

fn frames_from_session_history_message_with_namespace(
    runtime_key: &str,
    identity: &str,
    identity_namespace: &str,
    session_id: &str,
    offset: usize,
    message: Value,
) -> Vec<NewConsoleFrame> {
    let payload_hash = hash_short(&serde_json::to_string(&message).unwrap_or_default());
    let Some(parsed) = serde_json::from_value::<Message>(message.clone()).ok() else {
        return Vec::new();
    };
    if let Message::ToolResults {
        results,
        created_at,
    } = parsed
    {
        return results
            .into_iter()
            .enumerate()
            .flat_map(|(idx, result)| {
                let content = serde_json::to_value(&result.content).unwrap_or(Value::Null);
                let result_text = result.text_content();
                let tool_use_id = result.tool_use_id.clone();
                let timestamp_ms = created_at.timestamp_millis().max(0) as u64;
                let mut payload = json!({
                    "id": tool_use_id,
                    "tool_call_id": tool_use_id,
                    "result": result_text,
                    "content": content,
                    "is_error": result.is_error,
                    "source_event_type": "session_history",
                    "type": "session_history",
                });
                if !result.settlement_failures.is_empty() {
                    payload["settlement_failures"] = json!(result.settlement_failures);
                }
                let mut frames = vec![NewConsoleFrame {
                    id: None,
                    dedupe_key: format!(
                        "session-history:{runtime_key}:{session_id}:{offset}:{idx}:{payload_hash}"
                    ),
                    timestamp_ms,
                    runtime_key: runtime_key.to_string(),
                    identity: identity.to_string(),
                    conversation_id: Some(identity.to_string()),
                    session_id: Some(session_id.to_string()),
                    kind: "tool_execution_completed".to_string(),
                    status: ConsoleFrameStatus::Completed,
                    payload,
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::SessionHistory,
                        source_cursor: Some(format!("{session_id}:{offset}:{idx}")),
                    },
                    source_event_id: None,
                    interaction_id: None,
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                }];
                // Only the native, committed host carrier can create an app
                // locator. Standard result bodies and UI bytes stay behind
                // the authenticated session lookup and out of this frame log.
                if result.host_metadata.get(meerkat_mcp::apps::MCP_APPS_EXTENSION)
                    .and_then(|value| serde_json::from_value::<meerkat_mcp::apps::McpAppInvocation>(value.clone()).ok())
                    .is_some_and(|invocation| meerkat_mcp::apps::tool_ui_resource_uri(&invocation.tool).is_some())
                {
                    let mut app = frames[0].clone();
                    app.dedupe_key = format!("session-history-app:{runtime_key}:{session_id}:{offset}:{idx}:{payload_hash}");
                    app.kind = "mcp_app".to_string();
                    app.payload = json!({
                        "session_id": session_id,
                        "tool_call_id": tool_use_id,
                        "fallback": result_text,
                    });
                    frames.push(app);
                }
                // Backfill parity with the live ingest edge: a generate_image
                // tool result serializes an `ImageGenerationToolResult` into the
                // result text (which carries NO tool name on meerkat 0.7.x). The
                // live path emits per-image `assistant_image` frames; without
                // this the image renders as raw tool JSON after a reload /
                // history backfill. Parse the result directly and mirror those
                // frames (the console dedupes images by blob/reference).
                frames.extend(session_history_assistant_image_frames(
                    runtime_key,
                    identity,
                    session_id,
                    offset,
                    idx,
                    timestamp_ms,
                    &tool_use_id,
                    &result_text,
                ));
                frames
            })
            .collect();
    }
    // meerkat 0.7.25 persists `TranscriptMessageIdentity` onto committed
    // transcript messages (ask 15 addendum). Stamping it here gives history
    // frames the SAME interaction identity the live pipeline stamped (for
    // identity-first sends the host-minted id round-trips through runtime
    // admission), so the console can join live↔history twins exactly instead
    // of by content heuristics.
    let transcript_identity = match &parsed {
        Message::User(user) => Some(&user.identity),
        Message::BlockAssistant(assistant) => Some(&assistant.identity),
        _ => None,
    };
    let history_interaction_id = transcript_identity
        .and_then(|identity| identity.interaction_id.as_ref())
        .map(|id| id.0.to_string());
    let history_run_id = transcript_identity
        .and_then(|identity| identity.run_id.as_ref())
        .map(|id| id.0.to_string());
    let (kind, timestamp_ms, payload) = match &parsed {
        Message::User(user) => {
            // Runtime summaries and injected context share the user channel,
            // not human authorship. The runtime's typed role is authoritative;
            // an ordinary user's identical text remains a conversational row.
            if !user.transcript_role.is_conversational()
                || session_history_user_message_is_scaffold(&message)
            {
                return Vec::new();
            }
            (
                "user_input",
                user.created_at.timestamp_millis().max(0) as u64,
                json!({
                    "content": user.content,
                    "message": message,
                }),
            )
        }
        // Meerkat 0.7 removed the legacy flat-text `Message::Assistant`
        // variant; all assistant history is block-shaped now.
        Message::BlockAssistant(assistant) => {
            let text = assistant.text_blocks().collect::<Vec<_>>().join("");
            if text.is_empty() && assistant.assistant_message_id.is_none() {
                // A text-less step is not a terminal. A tool-only step
                // continues after its tool results; re-emitted as
                // `interaction_complete` with `result: ""` it had no live twin
                // (meerkat emits `text_complete` only for non-empty text and
                // the run's own terminal carries the final answer), so every
                // tool-only step reached the timeline as an empty completion
                // stamped with the live turn's interaction id, and
                // kind-driven clients closed the turn before its answer. It
                // projects as the live edge's `tool_call_requested` frames
                // instead; a silent step projects nothing.
                let step = HistoryAssistantStep {
                    runtime_key,
                    identity,
                    session_id,
                    offset,
                    assistant,
                    payload_hash: &payload_hash,
                    interaction_id: history_interaction_id.as_deref(),
                    run_id: history_run_id.as_deref(),
                };
                let mut frames = tool_only_step_tool_call_frames(step);
                frames.extend(spawn_initial_message_frames_from_assistant(
                    runtime_key,
                    identity,
                    identity_namespace,
                    session_id,
                    offset,
                    assistant,
                    &payload_hash,
                ));
                frames.extend(outgoing_comms_tool_call_frames_from_assistant(step));
                return frames;
            }
            // One committed assistant message can be an intermediate step.
            // Preserve authored text without fabricating a run terminal.
            (
                if text.is_empty() {
                    "assistant_message"
                } else {
                    "text_complete"
                },
                assistant.created_at.timestamp_millis().max(0) as u64,
                json!({
                    "result": text,
                    "text": text,
                    "message": message,
                    "source_event_type": "session_history",
                    "type": "session_history",
                }),
            )
        }
        Message::SystemNotice(notice) => (
            "system_notice",
            notice.created_at.timestamp_millis().max(0) as u64,
            json!({
                "message": message,
                "kind": notice.kind,
                "render_class": notice.kind.render_class(),
                "body": notice.body,
                "blocks": notice.blocks,
                "source_event_type": "session_history",
                "type": "session_history",
            }),
        ),
        Message::System(_) | Message::ToolResults { .. } => return Vec::new(),
    };
    let mut frames = vec![NewConsoleFrame {
        id: None,
        dedupe_key: format!("session-history:{runtime_key}:{session_id}:{offset}:{payload_hash}"),
        timestamp_ms,
        runtime_key: runtime_key.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(session_id.to_string()),
        kind: kind.to_string(),
        status: ConsoleFrameStatus::Completed,
        payload,
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: Some(format!("{session_id}:{offset}")),
        },
        source_event_id: None,
        interaction_id: history_interaction_id.clone(),
        turn_id: None,
        run_id: history_run_id.clone(),
        parent_frame_id: None,
        caused_by_frame_id: None,
    }];
    if let Message::BlockAssistant(assistant) = &parsed {
        if assistant.text_blocks().all(str::is_empty) {
            frames.extend(tool_only_step_tool_call_frames(HistoryAssistantStep {
                runtime_key,
                identity,
                session_id,
                offset,
                assistant,
                payload_hash: &payload_hash,
                interaction_id: history_interaction_id.as_deref(),
                run_id: history_run_id.as_deref(),
            }));
        }
        frames.extend(spawn_initial_message_frames_from_assistant(
            runtime_key,
            identity,
            identity_namespace,
            session_id,
            offset,
            assistant,
            &payload_hash,
        ));
        frames.extend(outgoing_comms_tool_call_frames_from_assistant(
            HistoryAssistantStep {
                runtime_key,
                identity,
                session_id,
                offset,
                assistant,
                payload_hash: &payload_hash,
                interaction_id: history_interaction_id.as_deref(),
                run_id: history_run_id.as_deref(),
            },
        ));
        if let Some(assistant_message_id) = assistant.assistant_message_id.as_ref() {
            for frame in &mut frames {
                // A child spawn input is a different message in another session.
                if frame.identity == identity && frame.session_id.as_deref() == Some(session_id) {
                    frame.payload["assistant_message_id"] = json!(assistant_message_id);
                }
            }
        }
    }
    frames
}

/// Agent-facing peer-comms send tools whose calls the sender's conversation
/// must render as outgoing communications (mirrors the console front-end's
/// peer-tool vocabulary).
const OUTGOING_COMMS_SEND_TOOLS: [&str; 3] = ["send_message", "send_request", "send_response"];

/// Backfill parity for the SENDER side of peer comms.
///
/// An outgoing peer communication exists in the sender's transcript only as
/// an assistant tool call (`send_message`/`send_request`/`send_response`)
/// plus a nameless tool result — neither of which the base history
/// projection preserved as an identifiable comms item (assistant history
/// keeps text blocks only; tool results carry no tool name). The RECIPIENT's
/// transcript, by contrast, records a typed incoming comms notice that DOES
/// survive history projection. Any history-rebuilt conversation therefore
/// showed arrivals but silently dropped the sender's own outgoing comms.
///
/// Re-emit the live edge's `tool_call_requested` frame ({id, tool_call_id,
/// name, args}) for each comms send call: the console pairs it with the
/// backfilled tool result by `tool_call_id` exactly like a live turn, and a
/// live twin (same tool_use_id) collapses through the tool-call arm of
/// [`transcript_fingerprint`].
fn outgoing_comms_tool_call_frames_from_assistant(
    step: HistoryAssistantStep<'_>,
) -> Vec<NewConsoleFrame> {
    let mut frames = Vec::new();
    for (tool_idx, tool) in step.assistant.tool_calls().enumerate() {
        if !OUTGOING_COMMS_SEND_TOOLS.contains(&tool.name) {
            continue;
        }
        let Ok(args) = serde_json::from_str::<Value>(tool.args.get()) else {
            continue;
        };
        frames.push(history_tool_call_frame(
            step,
            "comms-out",
            tool_idx,
            tool.id,
            tool.name,
            args,
        ));
    }
    frames
}

/// Transcript coordinates of one committed assistant message, shared by every
/// history frame projected from it. The interaction and run ids are the
/// persisted `TranscriptMessageIdentity`, the same identity the live pipeline
/// stamped.
#[derive(Clone, Copy)]
struct HistoryAssistantStep<'a> {
    runtime_key: &'a str,
    identity: &'a str,
    session_id: &'a str,
    offset: usize,
    assistant: &'a meerkat_core::types::BlockAssistantMessage,
    payload_hash: &'a str,
    interaction_id: Option<&'a str>,
    run_id: Option<&'a str>,
}

/// Every tool call of a text-less (tool-only) assistant step as a
/// `tool_call_requested` history frame. The peer-comms send tools are left to
/// [`outgoing_comms_tool_call_frames_from_assistant`], which runs for every
/// assistant step, so a comms call never projects twice.
fn tool_only_step_tool_call_frames(step: HistoryAssistantStep<'_>) -> Vec<NewConsoleFrame> {
    let mut frames = Vec::new();
    for (tool_idx, tool) in step.assistant.tool_calls().enumerate() {
        if OUTGOING_COMMS_SEND_TOOLS.contains(&tool.name) {
            continue;
        }
        let Ok(args) = serde_json::from_str::<Value>(tool.args.get()) else {
            continue;
        };
        frames.push(history_tool_call_frame(
            step,
            "tool-call",
            tool_idx,
            tool.id,
            tool.name,
            args,
        ));
    }
    frames
}

/// One `tool_call_requested` history frame in the live edge's shape ({id,
/// tool_call_id, name, args}): the console pairs it with the backfilled tool
/// result by `tool_call_id` exactly like a live turn, and a live twin (same
/// tool_use_id) collapses through the tool-call arm of
/// [`transcript_fingerprint`].
fn history_tool_call_frame(
    step: HistoryAssistantStep<'_>,
    cursor_tag: &str,
    tool_idx: usize,
    tool_call_id: &str,
    tool_name: &str,
    args: Value,
) -> NewConsoleFrame {
    let HistoryAssistantStep {
        runtime_key,
        identity,
        session_id,
        offset,
        assistant,
        payload_hash,
        interaction_id,
        run_id,
    } = step;
    NewConsoleFrame {
        id: None,
        dedupe_key: format!(
            "session-history:{runtime_key}:{session_id}:{offset}:{cursor_tag}:{tool_idx}:{payload_hash}"
        ),
        timestamp_ms: assistant.created_at.timestamp_millis().max(0) as u64,
        runtime_key: runtime_key.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: Some(session_id.to_string()),
        kind: "tool_call_requested".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({
            "id": tool_call_id,
            "tool_call_id": tool_call_id,
            "name": tool_name,
            "args": args,
            "source_event_type": "session_history",
            "type": "session_history",
        }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: Some(format!("{session_id}:{offset}:{cursor_tag}:{tool_idx}")),
        },
        source_event_id: None,
        interaction_id: interaction_id.map(str::to_string),
        turn_id: None,
        run_id: run_id.map(str::to_string),
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// The subset of [`crate::console_spawn::MOB_SPAWN_TOOL_VOCABULARY`] whose
/// argument shapes `spawn_initial_messages_from_tool_args` can read.
const SPAWN_INITIAL_MESSAGE_TOOLS: &[&str] = &["mob_spawn_member", "spawn_member"];

fn spawn_initial_message_frames_from_assistant(
    runtime_key: &str,
    parent_identity: &str,
    identity_namespace: &str,
    session_id: &str,
    offset: usize,
    assistant: &meerkat_core::types::BlockAssistantMessage,
    payload_hash: &str,
) -> Vec<NewConsoleFrame> {
    let mut frames = Vec::new();
    for (tool_idx, tool) in assistant.tool_calls().enumerate() {
        // DELIBERATELY NARROWER than
        // `console_spawn::MOB_SPAWN_TOOL_VOCABULARY`. This path feeds
        // `spawn_initial_messages_from_tool_args`, which understands the
        // single-record and `specs`/`members` argument shapes those two tools
        // use. `delegate`, `fork_off` and `spawn_many_members` also spawn, but
        // their argument shapes are not what that parser reads, so accepting
        // them here would produce empty or wrong initial-message frames rather
        // than more of them.
        //
        // Asserted as a genuine SUBSET by
        // `spawn_initial_message_tools_are_a_documented_subset`, so a typo here
        // is a test failure rather than a branch that never fires.
        if !SPAWN_INITIAL_MESSAGE_TOOLS.contains(&tool.name) {
            continue;
        }
        let Ok(args) = serde_json::from_str::<Value>(tool.args.get()) else {
            continue;
        };
        for (spawn_idx, (target_identity, initial_message)) in
            spawn_initial_messages_from_tool_args(&args)
                .into_iter()
                .enumerate()
        {
            let target_identity = apply_namespace(&target_identity, identity_namespace);
            let message_content = match &initial_message {
                Value::String(text) => json!([
                    {
                        "type": "text",
                        "text": text,
                    }
                ]),
                other => other.clone(),
            };
            let message = match &initial_message {
                Value::String(text) => json!({
                    "role": "user",
                    "content": text,
                    "created_at": assistant.created_at,
                }),
                other => json!({
                    "role": "user",
                    "content": other,
                    "created_at": assistant.created_at,
                }),
            };
            frames.push(NewConsoleFrame {
                id: None,
                dedupe_key: format!(
                    "session-history:{runtime_key}:{session_id}:{offset}:spawn-initial:{tool_idx}:{spawn_idx}:{payload_hash}"
                ),
                timestamp_ms: assistant.created_at.timestamp_millis().max(0) as u64,
                runtime_key: runtime_key.to_string(),
                identity: target_identity.clone(),
                conversation_id: Some(target_identity),
                session_id: None,
                kind: "user_input".to_string(),
                status: ConsoleFrameStatus::Completed,
                payload: json!({
                    "content": message_content,
                    "message": message,
                    "source_event_type": "session_history_spawn_initial_message",
                    "type": "session_history",
                    "tool_call_id": tool.id,
                    "parent_identity": parent_identity,
                    "via_tool": tool.name,
                }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::SessionHistory,
                    source_cursor: Some(format!(
                        "{session_id}:{offset}:spawn-initial:{tool_idx}:{spawn_idx}"
                    )),
                },
                source_event_id: None,
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            });
        }
    }
    frames
}

fn spawn_initial_messages_from_tool_args(args: &Value) -> Vec<(String, Value)> {
    let mut messages = Vec::new();
    if let Some(message) = spawn_initial_message_from_record(args) {
        messages.push(message);
    }
    if let Some(specs) = args.get("specs").and_then(Value::as_array) {
        for spec in specs {
            if let Some(message) = spawn_initial_message_from_record(spec) {
                messages.push(message);
            }
        }
    }
    messages
}

fn spawn_initial_message_from_record(record: &Value) -> Option<(String, Value)> {
    let target_identity = record
        .get("member_id")
        .or_else(|| record.get("agent_identity"))
        .and_then(Value::as_str)?
        .trim();
    if target_identity.is_empty() {
        return None;
    }
    let initial_message = record
        .get("initial_message")
        .or_else(|| record.get("task"))?
        .clone();
    Some((target_identity.to_string(), initial_message))
}

fn session_history_user_message_is_scaffold(message: &Value) -> bool {
    message
        .get("content")
        .is_some_and(scaffold_message_content_is_noise)
}

fn scaffold_message_content_is_noise(value: &Value) -> bool {
    match value {
        Value::String(text) => scaffold_message_text_is_noise(text),
        Value::Array(items) => items.iter().any(scaffold_message_content_is_noise),
        Value::Object(map) => ["text", "content", "message"]
            .iter()
            .filter_map(|key| map.get(*key))
            .any(scaffold_message_content_is_noise),
        _ => false,
    }
}

fn scaffold_message_text_is_noise(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("[PEER UPDATE]")
        || trimmed
            .to_ascii_lowercase()
            .starts_with("you have been spawned")
}

fn parse_session_history_watermark(watermark: &str, session_id: &str) -> Option<usize> {
    let rest = watermark.strip_prefix(session_id)?.strip_prefix(':')?;
    rest.split(':').next()?.parse().ok()
}

fn format_session_history_watermark(session_id: &str, offset: usize, checked_at_ms: u64) -> String {
    format!("{session_id}:{offset}:{checked_at_ms}")
}

fn session_history_watermark_is_fresh(watermark: &str, session_id: &str, now_ms: u64) -> bool {
    let Some(checked_at_ms) = watermark
        .rsplit_once(':')
        .and_then(|(_, checked_at_ms)| checked_at_ms.parse::<u64>().ok())
    else {
        return false;
    };
    let offset = parse_session_history_watermark(watermark, session_id).unwrap_or(0);
    let ttl_ms = if offset > 0 {
        SESSION_HISTORY_GROWING_REFRESH_TTL_MS
    } else {
        SESSION_HISTORY_REFRESH_TTL_MS
    };
    now_ms.saturating_sub(checked_at_ms) < ttl_ms
}

async fn history_frame_has_existing_counterpart(
    inner: &AggregatorInner,
    frame: &NewConsoleFrame,
) -> ConsoleLogResult<bool> {
    // An occurrence ID is positive canonical identity, never a content key.
    // Preserve its row even without interaction/run lineage or with equal text.
    if frame.source.kind == ConsoleFrameSourceKind::SessionHistory
        && frame.payload.get("assistant_message_id").is_some_and(|id| {
            serde_json::from_value::<meerkat_core::types::AssistantMessageId>(id.clone()).is_ok()
        })
    {
        return Ok(false);
    }
    let Some(category) = history_counterpart_category(&frame.kind) else {
        return Ok(false);
    };
    let provider_tool_id = history_counterpart_tool_id(&frame.kind, &frame.payload);
    if matches!(category, "tool-call" | "tool-result") && provider_tool_id.is_none() {
        return Ok(false);
    }
    let typed_owner = history_counterpart_has_typed_owner(
        frame.run_id.as_deref(),
        frame.interaction_id.as_deref(),
    );
    // Canonical typed history supplies committed content and transcript order.
    // Keep it beside the live projection for exact adapter reconciliation:
    // dropping it loses the only durable user evidence and tool positions.
    // append_if_absent still rejects replay of the same canonical source row.
    if typed_owner && matches!(category, "assistant" | "reasoning" | "user")
        || provider_tool_id.is_some()
    {
        return Ok(false);
    }
    let exact_content = typed_owner || provider_tool_id.is_some();
    let fingerprint = if exact_content {
        history_counterpart_exact_fingerprint(&frame.kind, &frame.payload)
    } else {
        transcript_fingerprint(&frame.kind, &frame.payload)
    };
    let Some(fingerprint) = fingerprint else {
        return Ok(false);
    };
    let assistant_terminal = category == "assistant";
    let mut delta_text_by_turn =
        BTreeMap::<(Option<String>, Option<String>, Option<String>), String>::new();
    let mut after = None;
    loop {
        let page = inner
            .store
            .query_frames(ConsoleTimelineQuery {
                identity: Some(frame.identity.clone()),
                conversation_id: frame.conversation_id.clone(),
                after,
                limit: 1_000,
            })
            .await?;
        for existing in &page.frames {
            if existing.source.kind == ConsoleFrameSourceKind::SessionHistory
                || history_counterpart_category(&existing.kind) != Some(category)
                || !history_counterpart_owner_matches(frame, existing, provider_tool_id.is_some())
                || (provider_tool_id.is_some()
                    && history_counterpart_tool_id(&existing.kind, &existing.payload)
                        != provider_tool_id)
            {
                continue;
            }
            let existing_fingerprint = if exact_content {
                history_counterpart_exact_fingerprint(&existing.kind, &existing.payload)
            } else {
                transcript_fingerprint(&existing.kind, &existing.payload)
            };
            if existing_fingerprint.as_ref() == Some(&fingerprint) {
                return Ok(true);
            }
            if assistant_terminal
                && let Some(delta) = text_delta_payload_text(&existing.kind, &existing.payload)
            {
                let turn_key = (
                    existing.run_id.clone(),
                    existing.interaction_id.clone(),
                    existing.turn_id.clone(),
                );
                let aggregated = delta_text_by_turn.entry(turn_key).or_default();
                aggregated.push_str(delta);
                if normalize_transcript_fingerprint_text(aggregated) == fingerprint {
                    return Ok(true);
                }
            }
        }
        if page.frames.is_empty() || page.next_cursor.is_none() {
            return Ok(false);
        }
        after = page.next_cursor;
    }
}

fn history_counterpart_category(kind: &str) -> Option<&'static str> {
    match kind {
        "user_input" | "interaction_started" => Some("user"),
        "text_delta" | "text_complete" | "interaction_complete" | "run_completed" => {
            Some("assistant")
        }
        "tool_call_requested" | "tool_call" | "tool_execution_started" => Some("tool-call"),
        "tool_execution_completed" => Some("tool-result"),
        kind if is_reasoning_event_kind(kind) => Some("reasoning"),
        _ => None,
    }
}

fn history_counterpart_tool_id<'a>(kind: &str, payload: &'a Value) -> Option<&'a str> {
    if !matches!(
        history_counterpart_category(kind),
        Some("tool-call" | "tool-result")
    ) {
        return None;
    }
    payload
        .get("tool_call_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

fn history_counterpart_has_typed_owner(run_id: Option<&str>, interaction_id: Option<&str>) -> bool {
    run_id.is_some_and(|id| !id.is_empty())
        || interaction_id.is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

fn history_counterpart_owner_matches(
    history: &NewConsoleFrame,
    live: &ConsoleFrame,
    provider_tool_identity: bool,
) -> bool {
    if history.runtime_key != live.runtime_key || history.identity != live.identity {
        return false;
    }
    if history.session_id.is_some()
        && live.session_id.is_some()
        && history.session_id != live.session_id
    {
        return false;
    }
    if provider_tool_identity {
        // ToolResults messages lack transcript lineage. The provider tool ID
        // is their identity, but cannot overrule contradictory known owners.
        return !(history.run_id.is_some()
            && live.run_id.is_some()
            && history.run_id != live.run_id
            || history.interaction_id.is_some()
                && live.interaction_id.is_some()
                && history.interaction_id != live.interaction_id);
    }
    // A human send reserves its exact interaction before the runtime creates
    // a run. Its later canonical message may fill in that run identity, but
    // another source or a contradictory known run is never a substitute.
    if history.kind == "user_input"
        && live.kind == "user_input"
        && live.source.kind == ConsoleFrameSourceKind::Send
        && live.run_id.is_none()
        && history.session_id.is_some()
        && history.session_id == live.session_id
        && history.interaction_id == live.interaction_id
        && history
            .interaction_id
            .as_deref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
    {
        return true;
    }
    if history_counterpart_has_typed_owner(
        history.run_id.as_deref(),
        history.interaction_id.as_deref(),
    ) || history_counterpart_has_typed_owner(
        live.run_id.as_deref(),
        live.interaction_id.as_deref(),
    ) {
        return history.session_id == live.session_id
            && history.run_id == live.run_id
            && history.interaction_id == live.interaction_id;
    }
    // Synthetic legacy console interaction IDs never reached old transcripts.
    true
}

fn history_counterpart_exact_fingerprint(kind: &str, payload: &Value) -> Option<String> {
    let value = match history_counterpart_category(kind)? {
        "user" => payload.get("content").or_else(|| payload.get("message"))?,
        "tool-call" => return transcript_fingerprint(kind, payload),
        "tool-result" => {
            let tool_id = history_counterpart_tool_id(kind, payload)?;
            // Persisted results have both a text summary and full content.
            // Prefer full content so images/structured output are not erased.
            let result = payload.get("content").or_else(|| payload.get("result"))?;
            return Some(
                json!([
                    tool_id,
                    history_counterpart_content_fingerprint(result),
                    payload
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ])
                .to_string(),
            );
        }
        _ => return None,
    };
    Some(history_counterpart_content_fingerprint(value))
}

fn history_counterpart_content_fingerprint(value: &Value) -> String {
    fn exact_text(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Array(blocks) if !blocks.is_empty() => blocks
                .iter()
                .map(exact_text)
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.concat()),
            Value::Object(block)
                if block.len() == 2
                    && block.get("type").and_then(Value::as_str) == Some("text") =>
            {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }
            _ => None,
        }
    }
    // Only an entirely textual typed value can become plain text. Arbitrary
    // JSON and mixed content retain their complete structure and exact strings.
    match exact_text(value) {
        Some(text) => json!(["text", text]).to_string(),
        None => json!(["content", value]).to_string(),
    }
}

fn session_history_watermark_runtime_key(runtime_key: &str, session_id: &str) -> String {
    format!("{runtime_key}:session-history:{session_id}")
}

fn transcript_fingerprint(kind: &str, payload: &Value) -> Option<String> {
    match kind {
        "user_input" | "interaction_started" => payload
            .get("content")
            .map(stable_value_fingerprint)
            .or_else(|| payload.get("message").map(stable_value_fingerprint)),
        "tool_execution_completed" => tool_result_fingerprint(payload),
        // Tool CALL frames key on the provider-minted tool_use_id: a
        // history-backfilled `tool_call_requested` (outgoing peer comms
        // parity) collapses with its live twin, which carried the same id
        // through the runtime event edge.
        "tool_call_requested" | "tool_call" | "tool_execution_started" => payload
            .get("tool_call_id")
            .or_else(|| payload.get("id"))
            .and_then(Value::as_str)
            .map(|tool_call_id| format!("tool-call:{tool_call_id}")),
        "text_delta" => {
            text_delta_payload_text(kind, payload).map(normalize_transcript_fingerprint_text)
        }
        kind if is_reasoning_event_kind(kind) => {
            reasoning_payload_text(kind, payload).map(normalize_transcript_fingerprint_text)
        }
        "text_complete" | "interaction_complete" | "run_completed" => {
            assistant_terminal_fingerprint(kind, payload)
        }
        _ => None,
    }
}

fn tool_result_fingerprint(payload: &Value) -> Option<String> {
    let id = payload
        .get("tool_call_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let result = payload
        .get("result")
        .or_else(|| payload.get("content"))
        .map(stable_value_fingerprint)?;
    Some(format!("{id}:{result}"))
}

fn assistant_terminal_fingerprint(kind: &str, payload: &Value) -> Option<String> {
    match kind {
        "text_complete" | "interaction_complete" | "run_completed" => payload
            .get("text")
            .or_else(|| payload.get("result"))
            .or_else(|| payload.get("content"))
            .map(stable_value_fingerprint),
        _ => None,
    }
}

fn text_delta_payload_text<'a>(kind: &str, payload: &'a Value) -> Option<&'a str> {
    if kind != "text_delta" {
        return None;
    }
    payload
        .get("delta")
        .or_else(|| payload.get("text"))
        .or_else(|| payload.get("content"))
        .and_then(Value::as_str)
        .or_else(|| payload.as_str())
}

fn reasoning_payload_text<'a>(kind: &str, payload: &'a Value) -> Option<&'a str> {
    if !is_reasoning_event_kind(kind) {
        return None;
    }
    if kind == "reasoning_delta" {
        return payload
            .get("delta")
            .or_else(|| payload.get("text"))
            .or_else(|| payload.get("content"))
            .or_else(|| payload.get("summary"))
            .or_else(|| payload.get("reasoning"))
            .and_then(Value::as_str)
            .or_else(|| payload.as_str());
    }
    payload
        .get("summary")
        .or_else(|| payload.get("text"))
        .or_else(|| payload.get("content"))
        .or_else(|| payload.get("delta"))
        .or_else(|| payload.get("reasoning"))
        .and_then(Value::as_str)
        .or_else(|| payload.as_str())
}

fn is_reasoning_event_kind(kind: &str) -> bool {
    matches!(
        kind,
        "reasoning_delta"
            | "reasoning_complete"
            | "reasoning_summary"
            | "reasoning_summary_delta"
            | "reasoning_summary_complete"
    )
}

fn stable_value_fingerprint(value: &Value) -> String {
    if let Some(text) = content_value_text(value) {
        return normalize_transcript_fingerprint_text(&text);
    }
    match value {
        Value::String(text) => normalize_transcript_fingerprint_text(text),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn content_value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(content_value_text)
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        }
        Value::Object(map) => ["text", "content", "message", "blocks"]
            .iter()
            .filter_map(|key| map.get(*key))
            .find_map(content_value_text),
        _ => None,
    }
}

fn normalize_transcript_fingerprint_text(text: &str) -> String {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("[EVENT via rpc] ")
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// A caller's decision on one stored timeline frame, applied inside the
/// aggregator's scan before the frame is projected.
pub(crate) type TimelineFrameFilter<'a> = dyn Fn(&ConsoleFrame) -> bool + Send + Sync + 'a;

/// Whether a timeline query returns frames projected for the view (policy
/// redaction applied, provenance removed) or as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameProjection {
    ForView,
    Stored,
}

/// `agent.view` for one caller on one stored timeline frame. This is the
/// one decision every timeline surface applies (window, RPC page, stream
/// snapshot and live frames, update markers), always to the frame as
/// stored, before it is projected:
///
/// - a frame under a runtime-plane identity (gap markers, identity-less
///   memory events) is decided by that identity, as before: no member writes
///   under one, since every creation path refuses those names and every
///   member writer drops a member event that would take one;
/// - a member frame with captured provenance is decided by the captured
///   lineage, under the most restrictive rule
///   ([`crate::access::AccessView::can_view_captured_member`]);
/// - a member frame without it fails closed under any applicable deny its
///   bare identity cannot decide
///   ([`crate::access::AccessView::can_view_uncaptured_member`]).
pub(crate) fn timeline_frame_visible_to(
    view: &crate::access::AccessView,
    frame: &ConsoleFrame,
) -> bool {
    if !view.enforced() {
        return true;
    }
    // An update marker cannot reveal a frame the same view hides.
    if frame.kind == "frame_updated" {
        let Some(value) = frame.payload.get("frame") else {
            return false;
        };
        let Ok(mut updated) = serde_json::from_value::<ConsoleFrame>(value.clone()) else {
            return false;
        };
        // Only the owner writes update markers (a synthetic source). A frame
        // embedded in any other frame's payload is ingested data: provenance
        // in it is never accepted as the owner's capture.
        if frame.source.kind != ConsoleFrameSourceKind::Synthetic {
            updated.source.member_provenance = None;
        }
        if !timeline_frame_visible_to(view, &updated) {
            return false;
        }
    }
    if crate::console_contracts::is_runtime_plane_identity(&frame.identity) {
        return view.can_view_agent(&frame.identity);
    }
    match frame
        .source
        .member_provenance
        .as_ref()
        .filter(|provenance| provenance.identity.identity == frame.identity)
    {
        Some(provenance) => view.can_view_captured_member(&captured_lineage(provenance)),
        None => view.can_view_uncaptured_member(&frame.identity),
    }
}

/// The access decision's input for a captured provenance. A provenance
/// captured before lineage was holds only the member's record: the member
/// is decided by it, and any parent it names counts as unconfirmed.
fn captured_lineage(
    provenance: &ConsoleFrameMemberProvenance,
) -> Vec<crate::access::CapturedLineageLink> {
    if provenance.lineage.is_empty() {
        let record = &provenance.identity;
        let mut lineage = vec![crate::access::CapturedLineageLink {
            identity: record.identity.clone(),
            attributes: Some(crate::access::AgentResourceAttributes {
                identity: record.identity.clone(),
                agent_id: Some(record.runtime_member_id.clone()),
                role: Some(provenance.member.role.clone()),
                labels: record.labels.clone(),
            }),
        }];
        if let Some(parent) = record
            .labels
            .get(crate::console_spawn::SPAWNED_BY_LABEL)
            .map(|parent| parent.trim().to_string())
            .filter(|parent| !parent.is_empty())
        {
            lineage.push(crate::access::CapturedLineageLink {
                identity: parent,
                attributes: None,
            });
        }
        return lineage;
    }
    provenance
        .lineage
        .iter()
        .map(|link| crate::access::CapturedLineageLink {
            identity: link.identity.clone(),
            attributes: link
                .labels
                .as_ref()
                .map(|labels| crate::access::AgentResourceAttributes {
                    identity: link.identity.clone(),
                    agent_id: link.agent_id.clone(),
                    role: link.role.clone(),
                    labels: labels.clone(),
                }),
        })
        .collect()
}

/// One caller's [`timeline_frame_visible_to`] over the frames of one
/// request, cached per (runtime, identity, captured provenance): a restricted
/// caller scanning many hidden frames decides each distinct member context
/// once.
pub(crate) struct TimelineFrameDecider {
    view: crate::access::AccessView,
    decisions: std::sync::Mutex<HashMap<FrameDecisionKey, bool>>,
}

/// (runtime key, frame identity, encoded captured provenance).
type FrameDecisionKey = (String, String, Option<String>);

impl TimelineFrameDecider {
    pub(crate) fn new(view: crate::access::AccessView) -> Self {
        Self {
            view,
            decisions: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn visible(&self, frame: &ConsoleFrame) -> bool {
        if !self.view.enforced() {
            return true;
        }
        // Update markers carry a second frame; they are decided whole.
        if frame.kind == "frame_updated" {
            return timeline_frame_visible_to(&self.view, frame);
        }
        let provenance = match frame.source.member_provenance.as_ref() {
            Some(provenance) => match serde_json::to_string(provenance) {
                Ok(encoded) => Some(encoded),
                Err(_) => return timeline_frame_visible_to(&self.view, frame),
            },
            None => None,
        };
        let key = (
            frame.runtime_key.clone(),
            frame.identity.clone(),
            provenance,
        );
        if let Some(visible) = self
            .decisions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .copied()
        {
            return visible;
        }
        let visible = timeline_frame_visible_to(&self.view, frame);
        self.decisions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, visible);
        visible
    }
}

#[derive(Debug, Clone, Copy)]
enum CachedIdentityVisibility {
    Visible,
    Hidden,
    Missing,
}

async fn frame_is_visible(
    inner: &AggregatorInner,
    frame: &ConsoleFrame,
    allow_historical_identity: bool,
    identity_records: &[ConsoleIdentityRecord],
) -> ConsoleLogResult<bool> {
    let mut identity_visibility_cache = HashMap::new();
    Box::pin(frame_is_visible_cached(
        inner,
        frame,
        allow_historical_identity,
        &mut identity_visibility_cache,
        identity_records,
    ))
    .await
}

async fn frame_is_visible_cached(
    inner: &AggregatorInner,
    frame: &ConsoleFrame,
    allow_historical_identity: bool,
    identity_visibility_cache: &mut HashMap<(String, String), CachedIdentityVisibility>,
    identity_records: &[ConsoleIdentityRecord],
) -> ConsoleLogResult<bool> {
    // An update marker cannot reveal a frame hidden by the same policy.
    if frame.kind == "frame_updated" {
        let Some(value) = frame.payload.get("frame") else {
            return Ok(false);
        };
        let Ok(updated) = serde_json::from_value::<ConsoleFrame>(value.clone()) else {
            return Ok(false);
        };
        if !Box::pin(frame_is_visible_cached(
            inner,
            &updated,
            allow_historical_identity,
            identity_visibility_cache,
            identity_records,
        ))
        .await?
        {
            return Ok(false);
        }
    }
    let entry = {
        let entries = inner
            .runtimes
            .read()
            .map_err(|_| runtime_registry_lock_error())?;
        if entries.is_empty() {
            return Ok(true);
        }
        let Some(entry) = entries.get(&frame.runtime_key) else {
            return Ok(false);
        };
        entry.clone()
    };
    if let Some(provenance) = &frame.source.member_provenance
        && !retained_member_visible(entry.visibility_policy.as_ref(), provenance)
    {
        return Ok(false);
    }
    // Runtime-plane frames aren't agent frames: `__console__` (synthetic
    // gap markers) and `_system` (ConsoleMemoryEventSink attributes
    // identity-less §9.3 memory.* events here) have no roster record to
    // resolve, so the per-identity gate below would hide them from every
    // global timeline query. They skip straight to the visibility policy;
    // ABAC still filters them per-caller in the RPC layer
    // (`timeline_frame_visible_to`).
    if !crate::console_contracts::is_runtime_plane_identity(&frame.identity) {
        let cache_key = (frame.runtime_key.clone(), frame.identity.clone());
        let identity_visibility =
            if let Some(cached) = identity_visibility_cache.get(&cache_key).copied() {
                cached
            } else {
                let runtime_member_id = strip_namespace(&frame.identity, &entry.identity_namespace)
                    .unwrap_or_else(|| frame.identity.clone());
                let visibility = match identity_records.iter().find(|record| {
                    record.runtime_key == frame.runtime_key
                        && (record.identity == frame.identity
                            || record.runtime_member_id == runtime_member_id)
                }) {
                    Some(record) => {
                        if Box::pin(console_identity_record_visible(&entry, record)).await {
                            CachedIdentityVisibility::Visible
                        } else {
                            CachedIdentityVisibility::Hidden
                        }
                    }
                    None => CachedIdentityVisibility::Missing,
                };
                identity_visibility_cache.insert(cache_key, visibility);
                visibility
            };
        match identity_visibility {
            CachedIdentityVisibility::Visible => {
                if Box::pin(frame_matches_hidden_member(&entry, frame)).await {
                    return Ok(false);
                }
            }
            CachedIdentityVisibility::Hidden => return Ok(false),
            CachedIdentityVisibility::Missing => {
                if Box::pin(frame_matches_hidden_member(&entry, frame)).await {
                    return Ok(false);
                }
                return Ok(
                    allow_historical_identity && entry.visibility_policy.frame_visible(frame)
                );
            }
        }
    }
    Ok(entry.visibility_policy.frame_visible(frame))
}

async fn identity_record_for_member(
    entry: &RuntimeEntry,
    handle: &MobHandle,
    member: &MobMemberListEntry,
) -> Option<ConsoleIdentityRecord> {
    // Roster ids are comms-safe encodings (meerkat 0.7 MemberCommsName);
    // identity records carry the public alias.
    let runtime_member_id =
        crate::member_comms_id::runtime_alias_str(member.agent_identity.as_str()).into_owned();
    let durable_identity = crate::member_comms_id::durable_identity_label(&member.labels)
        .unwrap_or(runtime_member_id.as_str());
    let identity = apply_namespace(durable_identity, &entry.identity_namespace);
    let addressable = member_is_addressable(member);
    let visibility = match member.status {
        meerkat_mob::MobMemberStatus::Retiring | meerkat_mob::MobMemberStatus::Completed => {
            ConsoleVisibility::RetiredReadable
        }
        // Broken/unknown members have no live runtime binding; mirror the
        // identity-first projection, which marks them unreachable.
        meerkat_mob::MobMemberStatus::Broken | meerkat_mob::MobMemberStatus::Unknown => {
            ConsoleVisibility::Unreachable
        }
        _ if addressable => ConsoleVisibility::Addressable,
        _ => ConsoleVisibility::Hidden,
    };
    let session_id = handle
        .resolve_bridge_session_id_observation(&member.agent_identity)
        .await
        .map(|sid| sid.to_string());
    let mut labels = member.labels.clone();
    labels
        .entry("role".to_string())
        .or_insert_with(|| member.role.to_string());
    // Spawn-time console metadata (group/display labels, spawned_by lineage)
    // registered by the agent-tool spawn path fills gaps the roster does not
    // carry; roster labels win on conflict — except lineage, which only the
    // spawn registry may assert (these records feed aggregate-mode ABAC
    // attribute priming).
    crate::console_spawn::sanitize_unverified_lineage_labels(&mut labels);
    if let Some(registered) = entry.console_events.identity_labels(durable_identity).await {
        crate::console_spawn::merge_registered_labels(&mut labels, &registered);
    }
    let display_name = labels
        .get("display_name")
        .cloned()
        .unwrap_or_else(|| runtime_member_id.clone());
    Some(ConsoleIdentityRecord {
        identity,
        display_name,
        runtime_key: entry.runtime_key.clone(),
        runtime_member_id,
        session_id,
        member_id: None,
        visibility,
        addressable,
        health: if addressable {
            "ready"
        } else {
            "hidden_by_policy"
        }
        .to_string(),
        topology_peers: member
            .wired_to
            .iter()
            .map(|peer| project_topology_peer(peer.as_str(), &entry.identity_namespace, None))
            .collect(),
        labels,
    })
}

async fn identity_record_for_resolved_member(
    resolved: &ResolvedConsoleMember,
) -> Option<ConsoleIdentityRecord> {
    let mut record =
        identity_record_for_member(&resolved.entry, &resolved.handle, &resolved.member).await?;
    if *resolved.entry.runtime.handle().mob_id() != resolved.source_mob_id {
        record
            .labels
            .entry("source_mob_id".to_string())
            .or_insert_with(|| resolved.source_mob_id.clone());
    }
    Some(record)
}

fn identity_record_for_status(
    entry: &RuntimeEntry,
    status: &crate::identity_first::IdentityStatus,
) -> ConsoleIdentityRecord {
    let identity = apply_namespace(status.identity.as_str(), &entry.identity_namespace);
    let runtime_member_id = status
        .agent_runtime_id
        .as_ref()
        .map(crate::identity_first::AgentRuntimeId::as_str)
        .unwrap_or_else(|| status.identity.as_str())
        .to_string();
    let addressable = status.addressability
        == crate::identity_first::AgentAddressability::Addressable
        && matches!(
            status.state,
            crate::identity_first::IdentityLifecycleState::Active
                | crate::identity_first::IdentityLifecycleState::Dormant
        );
    let visibility = match status.state {
        crate::identity_first::IdentityLifecycleState::Broken => ConsoleVisibility::Unreachable,
        _ if addressable => ConsoleVisibility::Addressable,
        _ => ConsoleVisibility::Hidden,
    };
    let health = match status.state {
        crate::identity_first::IdentityLifecycleState::Active => "ready",
        crate::identity_first::IdentityLifecycleState::Dormant => "dormant",
        crate::identity_first::IdentityLifecycleState::Broken => "broken",
    }
    .to_string();
    let display_name = status
        .display_name
        .as_ref()
        .map(crate::identity_first::DisplayName::as_str)
        .unwrap_or_else(|| status.identity.as_str())
        .to_string();
    ConsoleIdentityRecord {
        identity,
        display_name,
        runtime_key: entry.runtime_key.clone(),
        runtime_member_id,
        session_id: status.session_id.as_ref().map(ToString::to_string),
        member_id: status.member_id.clone(),
        visibility,
        addressable,
        health,
        topology_peers: Vec::new(),
        labels: status.labels.clone(),
    }
}

pub(crate) fn is_implicit_delegate_member(
    role: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> bool {
    role.eq_ignore_ascii_case("delegate") && labels.contains_key("source_mob_id")
}

fn member_is_addressable(member: &MobMemberListEntry) -> bool {
    member
        .labels
        .get("addressable")
        .map(|value| !value.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

fn sendable_resolved_member_indices(matches: &[ResolvedConsoleMember]) -> Vec<usize> {
    matches
        .iter()
        .enumerate()
        .filter_map(|(index, resolved)| {
            (member_is_addressable(&resolved.member)
                && resolved.member.status == meerkat_mob::MobMemberStatus::Active)
                .then_some(index)
        })
        .collect()
}

async fn authoritative_durable_sendable_member_index(
    requested_identity: &str,
    matches: &[ResolvedConsoleMember],
    sendable_indices: &[usize],
) -> Result<Option<usize>, ConsoleSendError> {
    if matches.iter().any(|resolved| {
        requested_identity_is_runtime_member_alias(
            requested_identity,
            &resolved.entry.identity_namespace,
        )
    }) {
        return Ok(None);
    }

    let mut visited_runtime_keys = BTreeSet::new();
    let mut bindings = BTreeSet::new();
    for resolved in matches {
        let entry = &resolved.entry;
        if !visited_runtime_keys.insert(entry.runtime_key.clone()) {
            continue;
        }
        let Some(identity_runtime) = entry.identity_runtime.as_ref() else {
            continue;
        };
        for raw_identity in
            namespace_match_candidates(requested_identity, &entry.identity_namespace)
        {
            let Ok(identity) = crate::identity_first::AgentIdentity::parse(&raw_identity) else {
                continue;
            };
            let Ok(status) = identity_runtime.status(&identity).await else {
                continue;
            };
            if status.state != crate::identity_first::IdentityLifecycleState::Active
                || status.addressability != crate::identity_first::AgentAddressability::Addressable
            {
                continue;
            }
            let Some(runtime_id) = status.agent_runtime_id.as_ref() else {
                continue;
            };
            let record = identity_record_for_status(entry, &status);
            if !Box::pin(console_identity_record_visible(entry, &record)).await {
                continue;
            }
            let mut projected_member_ids = BTreeSet::new();
            for index in sendable_indices {
                let resolved = &matches[*index];
                if resolved.entry.runtime_key != entry.runtime_key {
                    continue;
                }
                let Some(live_record) = identity_record_for_resolved_member(resolved).await else {
                    continue;
                };
                if live_record_realizes_durable_binding(&record, &live_record)
                    && stale_durable_session_mismatch(&record, std::slice::from_ref(&live_record))
                        .is_none()
                {
                    projected_member_ids.insert(live_record.runtime_member_id);
                }
            }
            if projected_member_ids.is_empty() {
                bindings.insert((entry.runtime_key.clone(), runtime_id.as_str().to_string()));
            } else {
                bindings.extend(
                    projected_member_ids
                        .into_iter()
                        .map(|member_id| (entry.runtime_key.clone(), member_id)),
                );
            }
        }
    }

    if bindings.len() > 1 {
        let candidates = bindings
            .iter()
            .map(|(runtime_key, runtime_id)| format!("{runtime_id}@{runtime_key}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ConsoleSendError::AmbiguousIdentity {
            identity: requested_identity.to_string(),
            candidates,
        });
    }
    let Some((runtime_key, runtime_id)) = bindings.into_iter().next() else {
        return Ok(None);
    };
    let bound_indices = sendable_indices
        .iter()
        .copied()
        .filter(|index| {
            matches[*index].entry.runtime_key == runtime_key
                && matches[*index].runtime_identity == runtime_id
        })
        .collect::<Vec<_>>();
    match bound_indices.as_slice() {
        [index] => Ok(Some(*index)),
        [] => Ok(None),
        _ => {
            let candidates = bound_indices
                .iter()
                .map(|index| {
                    format!(
                        "{}@{}",
                        matches[*index].runtime_identity, matches[*index].source_mob_id
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(ConsoleSendError::AmbiguousIdentity {
                identity: requested_identity.to_string(),
                candidates,
            })
        }
    }
}

fn apply_namespace(identity: &str, namespace: &str) -> String {
    let namespace = namespace.trim().trim_matches('/');
    if namespace.is_empty() || identity.starts_with(&format!("{namespace}/")) {
        identity.to_string()
    } else {
        format!("{namespace}/{identity}")
    }
}

fn strip_namespace(identity: &str, namespace: &str) -> Option<String> {
    let namespace = namespace.trim().trim_matches('/');
    if namespace.is_empty() {
        return Some(identity.to_string());
    }
    identity
        .strip_prefix(namespace)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(ToString::to_string)
}

/// The durable identity a generated alias names when `identity_runtime`
/// confirms it is that identity's CURRENT incarnation; `None` for a stale
/// generation, an unknown identity, or an id that is not a generated alias.
async fn current_generated_alias_identity(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    alias: &str,
) -> Option<crate::identity_first::AgentIdentity> {
    let durable = crate::member_comms_id::durable_identity_from_runtime_alias(alias)?;
    let identity = crate::identity_first::AgentIdentity::parse(&durable).ok()?;
    let current = identity_runtime
        .status(&identity)
        .await
        .ok()?
        .agent_runtime_id?;
    (current.as_str() == alias).then_some(identity)
}

fn namespace_match_candidates(identity: &str, namespace: &str) -> Vec<String> {
    let namespace = namespace.trim().trim_matches('/');
    if namespace.is_empty() {
        return vec![identity.to_string()];
    }
    let mut candidates = Vec::new();
    if let Some(stripped) = strip_namespace(identity, namespace) {
        candidates.push(stripped);
        candidates.push(identity.to_string());
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn requested_identity_is_runtime_member_alias(identity: &str, namespace: &str) -> bool {
    namespace_match_candidates(identity, namespace)
        .iter()
        .any(|candidate| crate::member_comms_id::is_reserved_generated_alias(candidate))
}

fn requested_identity_has_runtime_member_alias(identity: &str, entries: &[RuntimeEntry]) -> bool {
    entries.iter().any(|entry| {
        requested_identity_is_runtime_member_alias(identity, &entry.identity_namespace)
    })
}

fn validate_send_request(request: &ConsoleSendRequest) -> Result<(), ConsoleSendError> {
    if request.identity.trim().is_empty() {
        return Err(ConsoleSendError::InvalidRequest(
            "identity must be non-empty".to_string(),
        ));
    }
    // No member holds a runtime-plane identity; a send to one would record a
    // member frame under the console's own identity.
    if crate::console_contracts::is_runtime_plane_identity(request.identity.trim()) {
        return Err(ConsoleSendError::InvalidRequest(format!(
            "identity '{}' is reserved for the console's runtime events",
            request.identity.trim()
        )));
    }
    if request.origin.trim().is_empty() {
        return Err(ConsoleSendError::InvalidRequest(
            "origin must be non-empty".to_string(),
        ));
    }
    if request.idempotency_key.trim().is_empty() {
        return Err(ConsoleSendError::InvalidRequest(
            "idempotency_key must be non-empty".to_string(),
        ));
    }
    Ok(())
}

fn content_input_from_value(value: &Value) -> Result<ContentInput, ConsoleSendError> {
    let content: ContentInput = serde_json::from_value(value.clone())
        .map_err(|err| ConsoleSendError::InvalidContent(err.to_string()))?;
    match &content {
        ContentInput::Text(text) if text.trim().is_empty() => Err(
            ConsoleSendError::InvalidContent("content must be non-empty".to_string()),
        ),
        ContentInput::Blocks(blocks) if blocks.is_empty() => Err(ConsoleSendError::InvalidContent(
            "content blocks must be non-empty".to_string(),
        )),
        _ => Ok(content),
    }
}

fn parse_handling_mode(
    value: Option<&str>,
) -> Result<meerkat_core::types::HandlingMode, ConsoleSendError> {
    match value.unwrap_or("queue") {
        "queue" => Ok(meerkat_core::types::HandlingMode::Queue),
        "steer" => Ok(meerkat_core::types::HandlingMode::Steer),
        other => Err(ConsoleSendError::InvalidHandlingMode(other.to_string())),
    }
}

fn accepted_replay_from_frame(
    frame: &ConsoleFrame,
) -> Result<ConsoleInteractionAccepted, ConsoleSendError> {
    if frame.status == ConsoleFrameStatus::DeliveryFailed {
        return Err(ConsoleSendError::RuntimeOperation(Box::new(
            crate::identity_first::IdentityRuntimeError::HostHumanInput(
                crate::identity_first::bridge::HostHumanInputError::ReplayUnavailable {
                    interaction_id: frame.interaction_id.clone().unwrap_or_default(),
                    input_frame_id: frame.id.clone(),
                },
            ),
        )));
    }
    Ok(accepted_from_frame(frame))
}

fn accepted_from_frame(frame: &ConsoleFrame) -> ConsoleInteractionAccepted {
    ConsoleInteractionAccepted {
        interaction_id: frame
            .interaction_id
            .clone()
            .unwrap_or_else(|| format!("console-interaction-{}", hash_short(&frame.dedupe_key))),
        identity: frame.identity.clone(),
        conversation_id: frame.conversation_id.clone(),
        session_id: frame.session_id.clone(),
        input_frame_id: frame.id.clone(),
        cursor: frame.cursor.clone(),
        status: frame.status,
    }
}

#[cfg(test)]
fn send_dedupe_key(
    runtime_key: &str,
    identity: &str,
    origin: &str,
    idempotency_key: &str,
) -> String {
    send_scope::unscoped_send_dedupe_key(runtime_key, identity, origin, idempotency_key)
}

/// Request fingerprint for idempotent replay. A declared `origin_kind` is
/// part of the request; an undeclared one leaves the fingerprint exactly as
/// it was before the field existed, so stored replays keep matching.
fn send_request_fingerprint(
    origin: &str,
    origin_kind: Option<ConsoleTurnOrigin>,
    content: &Value,
    handling_mode: &str,
    skill_references: &[meerkat_core::skills::SkillKey],
) -> String {
    let content_json = serde_json::to_string(content).unwrap_or_default();
    let base = match origin_kind {
        None => format!("{origin}\n{handling_mode}\n{content_json}"),
        Some(kind) => format!(
            "{origin}\n{}\n{handling_mode}\n{content_json}",
            kind.as_str()
        ),
    };
    // An empty selection keeps the pre-selection fingerprint, so replays of
    // sends recorded before skill selection existed still match.
    if skill_references.is_empty() {
        return hash_short(&base);
    }
    let skills_json = serde_json::to_string(skill_references).unwrap_or_default();
    hash_short(&format!("{base}\nskills\n{skills_json}"))
}

/// The `user_input` frame payload for a console send. `origin_kind` is
/// written only when the caller declared one.
fn user_input_payload(request: &ConsoleSendRequest, handling_mode: &str) -> Value {
    let mut payload = json!({
        "content": request.content,
        "origin": request.origin,
        "idempotency_key": request.idempotency_key,
        "handling_mode": handling_mode,
    });
    if let (Some(kind), Some(object)) = (request.origin_kind, payload.as_object_mut()) {
        object.insert("origin_kind".to_string(), Value::from(kind.as_str()));
    }
    if !request.skill_refs.is_empty()
        && let (Ok(skills), Some(object)) = (
            serde_json::to_value(&request.skill_refs),
            payload.as_object_mut(),
        )
    {
        object.insert("skill_refs".to_string(), skills);
    }
    payload
}

/// Namespace for identity-first console interaction ids (UUIDv5 over the
/// send's dedupe key). Deterministic: an idempotent retry mints the SAME id.
/// UUID form is deliberate — the identity-first delivery path threads this id
/// into meerkat runtime admission (`WorkSpec.interaction_id`, 0.7.25 ask 15
/// addendum), so the turn's live frames and its persisted transcript messages
/// carry one joinable identity. The classic send path keeps the legacy
/// `console-interaction-{hash}` string: it cannot thread ids (the external
/// work door has no interaction parameter), and the console's dedup treats
/// only UUID-form ids as authoritative.
const CONSOLE_INTERACTION_UUID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x6d, 0x6f, 0x62, 0x6b, 0x69, 0x74, 0x2d, 0x63, 0x6f, 0x6e, 0x73, 0x6f, 0x6c, 0x65, 0x2d, 0x69,
]);

fn identity_first_interaction_uuid(dedupe_key: &str) -> String {
    uuid::Uuid::new_v5(&CONSOLE_INTERACTION_UUID_NAMESPACE, dedupe_key.as_bytes()).to_string()
}

fn hash_short(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    to_hex(&digest[..8])
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn current_time_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}

fn runtime_registry_lock_error() -> ConsoleLogError {
    Box::new(std::io::Error::other(
        "console runtime registry lock poisoned",
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::large_futures, clippy::panic)]
mod tests {
    /// Golden values: an empty skill selection must keep the exact
    /// fingerprint recorded before skill selection existed, so replays of
    /// sends persisted by older gateways still match. A nonempty selection
    /// changes it, and so does reordering the selection.
    #[test]
    fn console_send_fingerprint_is_pinned_for_an_empty_selection() {
        use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};

        let content = serde_json::json!("hello");
        assert_eq!(
            super::send_request_fingerprint("console:test", None, &content, "queue", &[]),
            "34e67bf50e8d4ee4"
        );
        assert_eq!(
            super::send_request_fingerprint(
                "console:test",
                Some(super::ConsoleTurnOrigin::Operator),
                &content,
                "queue",
                &[],
            ),
            "b9f7d1acd0800a64"
        );
        let source = SourceUuid::parse("5b0c7c2e-4c61-4f2d-9d0e-2f5a8c6b1e11").expect("uuid");
        let a = SkillKey::new(source.clone(), SkillName::parse("alpha").expect("name"));
        let b = SkillKey::new(source, SkillName::parse("beta").expect("name"));
        let selected = super::send_request_fingerprint(
            "console:test",
            None,
            &content,
            "queue",
            &[a.clone(), b.clone()],
        );
        assert_ne!(selected, "34e67bf50e8d4ee4");
        assert_ne!(
            selected,
            super::send_request_fingerprint("console:test", None, &content, "queue", &[b, a]),
            "the selection is order-sensitive"
        );
    }

    /// The initial-message tools must be a genuine subset of the crate-wide
    /// spawn vocabulary. Catches a typo, which would otherwise be a `continue`
    /// that silently never fires for any real tool.
    #[test]
    fn spawn_initial_message_tools_are_a_documented_subset() {
        for name in super::SPAWN_INITIAL_MESSAGE_TOOLS {
            assert!(
                crate::console_spawn::MOB_SPAWN_TOOL_VOCABULARY.contains(name),
                "{name} is not in MOB_SPAWN_TOOL_VOCABULARY - either a typo here \
                 or a tool the crate does not otherwise recognise as spawning",
            );
        }
    }

    /// Regression: a session that keeps failing backfill must collapse to ONE
    /// gap frame. The dedupe key is stable per (runtime, identity, session) —
    /// independent of wall-clock — so repeated failures don't mint a fresh
    /// timestamped frame each retry and grow the timeline store unbounded.
    #[test]
    fn backfill_gap_dedupe_key_is_stable_per_session() {
        use super::backfill_gap_dedupe_key;
        let a = backfill_gap_dedupe_key("default", "worker:1", "sess-9");
        let b = backfill_gap_dedupe_key("default", "worker:1", "sess-9");
        assert_eq!(a, b, "same target must reuse one gap frame");
        assert_ne!(a, backfill_gap_dedupe_key("default", "worker:1", "sess-10"));
        assert_ne!(a, backfill_gap_dedupe_key("default", "worker:2", "sess-9"));
        // The key carries no wall-clock component (the pre-fix bug appended
        // current_time_ms, so every retry was a unique frame).
        assert_eq!(a, "session-backfill-gap:default:worker:1:sess-9");
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use std::time::Instant;

    use futures::StreamExt;
    use meerkat::{AgentFactory, Config, build_ephemeral_service};
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest, TestClient};
    use meerkat_core::{
        AppendSystemContextRequest, AppendSystemContextResult, CommsRuntime, EventStream,
        RunResult, SessionControlError, SessionError, SessionHistoryPage, SessionHistoryQuery,
        SessionId, SessionQuery, SessionService, SessionServiceCommsExt, SessionServiceControlExt,
        SessionServiceHistoryExt, SessionSummary, SessionView, StartTurnRequest, StopReason,
        StreamError,
    };
    use meerkat_mob::{MobDefinition, MobSessionService, MobStorage, SpawnMemberSpec};
    use serde_json::json;

    use super::*;
    use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};
    use crate::mob_handle_runtime::MobBootstrapSpec;

    #[derive(Debug)]
    struct HideHiddenNoiseFrames;

    impl ConsoleVisibilityPolicy for HideHiddenNoiseFrames {
        fn frame_visible(&self, frame: &ConsoleFrame) -> bool {
            frame.kind != "hidden_noise"
        }
    }

    #[derive(Debug)]
    struct HideRuntimeMemberIdentity(&'static str);

    impl ConsoleVisibilityPolicy for HideRuntimeMemberIdentity {
        fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
            record.runtime_member_id != self.0
        }
    }

    #[derive(Debug)]
    struct HideRuntimeMemberOnly(&'static str);

    impl ConsoleVisibilityPolicy for HideRuntimeMemberOnly {
        fn member_visible(&self, member: &ConsoleMember) -> bool {
            member.agent_identity != self.0
        }
    }

    #[derive(Debug)]
    struct HideConsoleIdentity(&'static str);

    impl ConsoleVisibilityPolicy for HideConsoleIdentity {
        fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
            record.identity != self.0
        }
    }

    struct CountingConsoleLogStore {
        inner: InMemoryConsoleLogStore,
        source_watermark_calls: AtomicUsize,
        record_watermark_calls: AtomicUsize,
        window_page_limit: usize,
        legacy_empty_page: bool,
        snapshot_append_gate: std::sync::Mutex<Option<Arc<HistoryReadGate>>>,
    }

    impl CountingConsoleLogStore {
        fn new() -> Self {
            Self {
                inner: InMemoryConsoleLogStore::new(),
                source_watermark_calls: AtomicUsize::new(0),
                record_watermark_calls: AtomicUsize::new(0),
                window_page_limit: usize::MAX,
                legacy_empty_page: false,
                snapshot_append_gate: std::sync::Mutex::new(None),
            }
        }

        fn source_watermark_calls(&self) -> usize {
            self.source_watermark_calls.load(Ordering::SeqCst)
        }
    }

    struct SlowTestClient {
        delay: Duration,
    }

    #[async_trait::async_trait]
    impl LlmClient for SlowTestClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let delay = self.delay;
            let delayed_text = futures::stream::once(async move {
                tokio::time::sleep(delay).await;
                Ok(LlmEvent::TextDelta {
                    delta: "slow ok".to_string(),
                    meta: None,
                })
            });
            // meerkat 0.8.22 rejects a turn whose stream carried no normalized
            // provider accounting, so the terminal `Done` never travels alone.
            let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
                request,
                meerkat_core::Provider::Other,
                StopReason::EndTurn,
            );
            let tail = futures::stream::iter(vec![Ok(usage), Ok(done)]);
            Box::pin(delayed_text.chain(tail))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    pub(super) struct DelayedHistorySessionService {
        inner: Arc<dyn MobSessionService>,
        delay: Duration,
        read_calls: Arc<AtomicUsize>,
        application_read_calls: Arc<AtomicUsize>,
        active_reads: Arc<AtomicUsize>,
        max_active_reads: Arc<AtomicUsize>,
        first_read_wave: Arc<std::sync::Mutex<Option<HistoryReadWave>>>,
        held_first_read: Arc<std::sync::Mutex<Option<Arc<HistoryReadGate>>>>,
        scripted_reads: Arc<std::sync::Mutex<std::collections::VecDeque<ScriptedHistoryRead>>>,
        scripted_application_reads:
            Arc<std::sync::Mutex<std::collections::VecDeque<ScriptedApplicationRead>>>,
    }

    #[derive(Clone)]
    struct HistoryReadWave {
        count: usize,
        barrier: Arc<tokio::sync::Barrier>,
    }

    pub(super) struct ScriptedHistoryRead {
        pub(super) page: Option<SessionHistoryPage>,
        pub(super) gate: Option<Arc<HistoryReadGate>>,
    }

    pub(super) struct ScriptedApplicationRead {
        pub(super) observations: Vec<meerkat_core::ToolApplicationObservation>,
        pub(super) gate: Arc<HistoryReadGate>,
    }

    pub(super) struct HistoryReadGate {
        entered: Semaphore,
        release: Semaphore,
    }

    impl HistoryReadGate {
        pub(super) fn new() -> Arc<Self> {
            Arc::new(Self {
                entered: Semaphore::new(0),
                release: Semaphore::new(0),
            })
        }

        pub(super) fn release(&self) {
            self.release.add_permits(1);
        }

        pub(super) async fn wait_until_entered(&self) {
            tokio::time::timeout(Duration::from_secs(10), self.entered.acquire())
                .await
                .expect("scripted history read entered")
                .expect("entry gate open")
                .forget();
        }
    }

    impl DelayedHistorySessionService {
        fn new(inner: Arc<dyn MobSessionService>, delay: Duration) -> Self {
            Self {
                inner,
                delay,
                read_calls: Arc::new(AtomicUsize::new(0)),
                application_read_calls: Arc::new(AtomicUsize::new(0)),
                active_reads: Arc::new(AtomicUsize::new(0)),
                max_active_reads: Arc::new(AtomicUsize::new(0)),
                first_read_wave: Arc::new(std::sync::Mutex::new(None)),
                held_first_read: Arc::new(std::sync::Mutex::new(None)),
                scripted_reads: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
                scripted_application_reads: Arc::new(std::sync::Mutex::new(
                    std::collections::VecDeque::new(),
                )),
            }
        }

        pub(super) fn script_history(&self, reads: impl IntoIterator<Item = ScriptedHistoryRead>) {
            self.scripted_reads
                .lock()
                .expect("scripted history lock")
                .extend(reads);
        }

        pub(super) fn read_calls(&self) -> usize {
            self.read_calls.load(Ordering::SeqCst)
        }

        pub(super) fn script_application_observations(&self, read: ScriptedApplicationRead) {
            self.scripted_application_reads
                .lock()
                .expect("scripted application read lock")
                .push_back(read);
        }

        pub(super) fn application_read_calls(&self) -> usize {
            self.application_read_calls.load(Ordering::SeqCst)
        }

        fn max_active_reads(&self) -> usize {
            self.max_active_reads.load(Ordering::SeqCst)
        }

        fn synchronize_first_history_reads(&self, count: usize) {
            assert_eq!(self.read_calls(), 0, "configure before any history reads");
            *self.first_read_wave.lock().expect("first read wave lock") = Some(HistoryReadWave {
                count,
                barrier: Arc::new(tokio::sync::Barrier::new(count)),
            });
        }

        /// Hold the first history read open until the returned gate is
        /// released; it then reads the real page like every other read.
        fn hold_first_history_read(&self) -> Arc<HistoryReadGate> {
            assert_eq!(self.read_calls(), 0, "configure before any history reads");
            let gate = HistoryReadGate::new();
            *self.held_first_read.lock().expect("held first read lock") = Some(gate.clone());
            gate
        }

        fn active_reads(&self) -> usize {
            self.active_reads.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl SessionService for DelayedHistorySessionService {
        async fn read_tool_application_observations(
            &self,
            id: &SessionId,
        ) -> Result<Vec<meerkat_core::ToolApplicationObservation>, SessionError> {
            self.application_read_calls.fetch_add(1, Ordering::SeqCst);
            let scripted = self
                .scripted_application_reads
                .lock()
                .expect("scripted application read lock")
                .pop_front();
            if let Some(scripted) = scripted {
                scripted.gate.entered.add_permits(1);
                scripted
                    .gate
                    .release
                    .acquire()
                    .await
                    .expect("release gate open")
                    .forget();
                return Ok(scripted.observations);
            }
            self.inner.read_tool_application_observations(id).await
        }

        async fn create_session(
            &self,
            req: meerkat_core::CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            self.inner.create_session(req).await
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            self.inner.start_turn(id, req).await
        }

        async fn reconcile_runtime_compaction_projections(
            &self,
            id: &SessionId,
            intents: Vec<meerkat_core::CompactionProjectionIntent>,
        ) -> Result<(), SessionError> {
            self.inner
                .reconcile_runtime_compaction_projections(id, intents)
                .await
        }

        async fn abort_uncommitted_compaction_projections(
            &self,
            id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner
                .abort_uncommitted_compaction_projections(id)
                .await
        }

        async fn abort_rejected_runtime_run_projections(
            &self,
            id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner.abort_rejected_runtime_run_projections(id).await
        }

        async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
            self.inner.interrupt(id).await
        }

        // New on `SessionService` in 0.8.22 (exact-run hard cancel). Default is
        // `Err(Unsupported)`; forward so this passthrough reports the inner
        // service's answer rather than refusing on its own authority.
        async fn interrupt_run_if_current(
            &self,
            id: &SessionId,
            expected_run_id: &meerkat_core::RunId,
        ) -> Result<bool, SessionError> {
            self.inner
                .interrupt_run_if_current(id, expected_run_id)
                .await
        }

        async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
            self.inner.cancel_after_boundary(id).await
        }

        async fn cancel_after_boundary_for_run(
            &self,
            id: &SessionId,
            expected_run_id: &meerkat_core::RunId,
        ) -> Result<(), SessionError> {
            self.inner
                .cancel_after_boundary_for_run(id, expected_run_id)
                .await
        }

        async fn set_session_client(
            &self,
            id: &SessionId,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
        ) -> Result<(), SessionError> {
            self.inner.set_session_client(id, client).await
        }

        async fn hot_swap_session_llm_identity(
            &self,
            id: &SessionId,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), SessionError> {
            self.inner
                .hot_swap_session_llm_identity(id, client, identity, request_policy)
                .await
        }

        async fn set_session_tool_visibility_state(
            &self,
            id: &SessionId,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), SessionError> {
            self.inner
                .set_session_tool_visibility_state(id, state)
                .await
        }

        async fn update_session_mob_authority_context(
            &self,
            id: &SessionId,
            authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
        ) -> Result<(), SessionError> {
            self.inner
                .update_session_mob_authority_context(id, authority_context)
                .await
        }

        async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
            self.inner.has_live_session(id).await
        }

        async fn set_session_tool_filter(
            &self,
            id: &SessionId,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), SessionError> {
            self.inner.set_session_tool_filter(id, filter).await
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            self.inner.read(id).await
        }

        async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            self.inner.list(query).await
        }

        async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
            self.inner.archive(id).await
        }

        async fn subscribe_session_events(
            &self,
            id: &SessionId,
        ) -> Result<EventStream, StreamError> {
            SessionService::subscribe_session_events(self.inner.as_ref(), id).await
        }

        async fn record_live_terminal_error(
            &self,
            id: &SessionId,
            cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
        ) -> Result<(), SessionError> {
            self.inner.record_live_terminal_error(id, cause).await
        }

        async fn record_live_output_audio_degraded(
            &self,
            id: &SessionId,
            dropped: u64,
        ) -> Result<(), SessionError> {
            self.inner
                .record_live_output_audio_degraded(id, dropped)
                .await
        }
    }

    #[async_trait::async_trait]
    impl SessionServiceCommsExt for DelayedHistorySessionService {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CommsRuntime>> {
            self.inner.comms_runtime(session_id).await
        }

        async fn event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
            self.inner.event_injector(session_id).await
        }

        async fn interaction_event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            self.inner.interaction_event_injector(session_id).await
        }
    }

    #[async_trait::async_trait]
    impl SessionServiceControlExt for DelayedHistorySessionService {
        async fn append_system_context(
            &self,
            id: &SessionId,
            req: AppendSystemContextRequest,
        ) -> Result<AppendSystemContextResult, SessionControlError> {
            self.inner.append_system_context(id, req).await
        }

        async fn stage_tool_results(
            &self,
            id: &SessionId,
            req: meerkat_core::service::StageToolResultsRequest,
        ) -> Result<meerkat_core::service::StageToolResultsResult, SessionError> {
            self.inner.stage_tool_results(id, req).await
        }
    }

    #[async_trait::async_trait]
    impl SessionServiceHistoryExt for DelayedHistorySessionService {
        async fn read_history(
            &self,
            id: &SessionId,
            query: SessionHistoryQuery,
        ) -> Result<SessionHistoryPage, SessionError> {
            let read_index = self.read_calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active_reads.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_reads.fetch_max(active, Ordering::SeqCst);
            let first_wave = self
                .first_read_wave
                .lock()
                .expect("first read wave lock")
                .clone();
            if let Some(wave) = first_wave.filter(|wave| read_index < wave.count) {
                wave.barrier.wait().await;
            }
            let held = self
                .held_first_read
                .lock()
                .expect("held first read lock")
                .clone();
            if let Some(gate) = held.filter(|_| read_index == 0) {
                gate.entered.add_permits(1);
                gate.release
                    .acquire()
                    .await
                    .expect("release gate open")
                    .forget();
            }
            let scripted = self
                .scripted_reads
                .lock()
                .expect("scripted history lock")
                .pop_front();
            if let Some(scripted) = scripted {
                if let Some(gate) = scripted.gate {
                    gate.entered.add_permits(1);
                    gate.release
                        .acquire()
                        .await
                        .expect("release gate open")
                        .forget();
                }
                self.active_reads.fetch_sub(1, Ordering::SeqCst);
                return scripted
                    .page
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() });
            }
            tokio::time::sleep(self.delay).await;
            let result = self.inner.read_history(id, query).await;
            self.active_reads.fetch_sub(1, Ordering::SeqCst);
            result
        }

        async fn read_transcript_revision(
            &self,
            id: &SessionId,
            query: meerkat_core::service::SessionTranscriptRevisionQuery,
        ) -> Result<meerkat_core::service::SessionTranscriptRevisionPage, SessionError> {
            self.inner.read_transcript_revision(id, query).await
        }

        async fn list_transcript_revisions(
            &self,
            id: &SessionId,
            query: meerkat_core::service::SessionTranscriptRevisionListQuery,
        ) -> Result<meerkat_core::service::SessionTranscriptRevisionList, SessionError> {
            self.inner.list_transcript_revisions(id, query).await
        }
    }

    #[async_trait::async_trait]
    impl MobSessionService for DelayedHistorySessionService {
        async fn subscribe_session_activity(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
            self.inner.subscribe_session_activity(session_id).await
        }

        // Forwarded to the wrapped service, like the production decorators.
        async fn observe_live_durable_source(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
            self.inner.observe_live_durable_source(session_id).await
        }

        async fn observe_member_status_view(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
            self.inner.observe_member_status_view(session_id).await
        }

        async fn commit_live_delegation_final_transcript(
            &self,
            machine: &meerkat_runtime::MeerkatMachine,
            session_id: &meerkat_core::SessionId,
            provisional: meerkat_core::ProvisionalLiveHandoff,
            final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
            self.inner
                .commit_live_delegation_final_transcript(
                    machine,
                    session_id,
                    provisional,
                    final_event,
                )
                .await
        }

        // meerkat 0.8.30 made this REQUIRED. This double wraps an inner service
        // and owns no session authority of its own, so it forwards - the same
        // reasoning as `observe_session_resume_authority` above. Answering
        // locally would report "projected nothing" while a persistent inner had
        // real work owing, which is the silent-hole shape the required-method
        // change exists to remove.
        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            session_id: &meerkat_core::types::SessionId,
            runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, meerkat_core::service::SessionError> {
            self.inner
                .enqueue_committed_parent_session_boundary_after_runtime_turn(
                    session_id,
                    runtime_adapter,
                )
                .await
        }

        async fn observe_session_resume_authority(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
            // This double owns no resume authority of its own, so it reports
            // the inner service's. Minting a local empty bundle here would
            // disagree with the authority embedded in the verdict delegated
            // below, and the trait default of
            // `revalidate_session_resume_authority` (which reads THIS method)
            // would then reject every resume against a persistent inner.
            // Behaviourally identical while inner is ephemeral, which returns
            // the same empty bundle.
            self.inner
                .observe_session_resume_authority(session_id)
                .await
        }

        // meerkat 0.8.22 deleted `prepare_session_for_resume`. In 0.8.21 the
        // trait default of `materialize_session_resume_verdict` opened with
        // `self.prepare_session_for_resume(...)`, so this wrapper's old
        // override of that hook was what carried durable-tail convergence
        // through to `inner`. 0.8.22 drops that statement: the default now
        // routes to `materialize_nonpersistent_session_resume_verdict`, which
        // converges nothing and stamps a `NonPersistent` receipt. Forwarding
        // the verdict seam is therefore the exact behaviour-preserving port of
        // the deleted override. Failing to forward is loud, not silent - the
        // persistent create seams reject a `NonPersistent` receipt - but it
        // would break every resume against a persistent inner.
        async fn materialize_session_resume_verdict(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
            self.inner
                .materialize_session_resume_verdict(session_id)
                .await
        }

        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            session_id: &meerkat_core::types::SessionId,
            authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), meerkat_core::service::SessionError> {
            self.inner
                .acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                    session_id, authority,
                )
                .await
        }
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
            self.inner.load_session_for_resume(session_id).await
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::RunResult, SessionError> {
            self.inner
                .create_session_under_runtime_turn_boundary(req)
                .await
        }

        // 0.8.22 threads the resume-preparation receipt minted by
        // `materialize_session_resume_verdict` into actor creation, so the body
        // is consumed under the exact authority it was authorized against.
        async fn create_session_with_actor_witness_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
            resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<meerkat_core::RunResult, SessionError> {
            self.inner
                .create_session_with_actor_witness_under_runtime_turn_boundary(
                    req,
                    resume_preparation,
                    actor_witness_slot,
                )
                .await
        }

        async fn create_session_with_machine_archived_resume_authority(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
            authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        ) -> Result<meerkat_core::RunResult, SessionError> {
            self.inner
                .create_session_with_machine_archived_resume_authority(req, authorization)
                .await
        }

        async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
            authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        ) -> Result<meerkat_core::RunResult, SessionError> {
            self.inner
                .create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
                    req,
                    authorization,
                )
                .await
        }

        async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
            authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
            resume_preparation: meerkat_mob::SessionResumePreparationReceipt,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<meerkat_core::RunResult, SessionError> {
            self.inner
                .create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                    req,
                    authorization,
                    resume_preparation,
                    actor_witness_slot,
                )
                .await
        }

        async fn authorize_revivable_retired_session(
            &self,
            session_id: &SessionId,
            authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
        ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
            self.inner
                .authorize_revivable_retired_session(session_id, authority)
                .await
        }

        async fn subscribe_session_events(
            &self,
            session_id: &SessionId,
        ) -> Result<EventStream, StreamError> {
            meerkat_mob::MobSessionService::subscribe_session_events(
                self.inner.as_ref(),
                session_id,
            )
            .await
        }

        fn supports_persistent_sessions(&self) -> bool {
            self.inner.supports_persistent_sessions()
        }

        async fn live_session_actor_registered(
            &self,
            session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            self.inner.live_session_actor_registered(session_id).await
        }

        fn acquire_runtime_adapter(
            &self,
            explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        ) -> Result<Option<Arc<meerkat_runtime::MeerkatMachine>>, meerkat_runtime::RuntimeDriverError>
        {
            self.inner.acquire_runtime_adapter(explicit)
        }

        fn supports_runtime_turn_apply(&self) -> bool {
            self.inner.supports_runtime_turn_apply()
        }

        async fn interrupt_with_machine_authority(
            &self,
            session_id: &SessionId,
            authority: meerkat_runtime::MachineSessionControlAuthority,
        ) -> Result<(), SessionError> {
            self.inner
                .interrupt_with_machine_authority(session_id, authority)
                .await
        }

        // New in 0.8.22 (exact-run hard cancel). Its trait default is
        // `Err(Unsupported)`, and the mob provisioner calls it on the service it
        // was handed, so a passthrough that does not forward would answer
        // "unsupported" on its own authority instead of reaching the inner
        // service's exact-run interrupt.
        async fn interrupt_run_with_machine_authority(
            &self,
            session_id: &SessionId,
            expected_run_id: &meerkat_core::RunId,
            authority: meerkat_runtime::MachineSessionControlAuthority,
        ) -> Result<bool, SessionError> {
            self.inner
                .interrupt_run_with_machine_authority(session_id, expected_run_id, authority)
                .await
        }

        async fn cancel_after_boundary_with_machine_authority(
            &self,
            session_id: &SessionId,
            expected_run_id: &meerkat_core::RunId,
            authority: meerkat_runtime::MachineSessionControlAuthority,
        ) -> Result<(), SessionError> {
            self.inner
                .cancel_after_boundary_with_machine_authority(
                    session_id,
                    expected_run_id,
                    authority,
                )
                .await
        }

        async fn cancel_current_after_boundary_with_machine_authority(
            &self,
            session_id: &SessionId,
            authority: meerkat_runtime::MachineSessionControlAuthority,
        ) -> Result<(), SessionError> {
            self.inner
                .cancel_current_after_boundary_with_machine_authority(session_id, authority)
                .await
        }

        async fn execution_snapshot(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SessionError> {
            self.inner.execution_snapshot(session_id).await
        }

        async fn tool_scope_snapshot(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
            self.inner.tool_scope_snapshot(session_id).await
        }

        async fn external_tool_surface_snapshot(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
            self.inner.external_tool_surface_snapshot(session_id).await
        }

        async fn peer_ingress_runtime_snapshot(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::PeerIngressRuntimeSnapshot>, SessionError> {
            self.inner.peer_ingress_runtime_snapshot(session_id).await
        }

        async fn session_known_to_archive_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            self.inner
                .session_known_to_archive_authority(session_id)
                .await
        }

        async fn session_belongs_to_mob(
            &self,
            session_id: &SessionId,
            mob_id: &meerkat_mob::MobId,
        ) -> bool {
            self.inner.session_belongs_to_mob(session_id, mob_id).await
        }

        async fn load_persisted_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::Session>, SessionError> {
            self.inner.load_persisted_session(session_id).await
        }

        async fn load_revivable_retired_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::Session>, SessionError> {
            self.inner.load_revivable_retired_session(session_id).await
        }

        // New in 0.8.22 (durable transcript fork). Default is
        // `Err(Unsupported)`; forward so the wrapper never claims the inner
        // service lacks fork authority on its own behalf.
        async fn fork_persisted_session(
            &self,
            source_session_id: &SessionId,
            message_count: Option<usize>,
            tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            target: meerkat_core::DurableSessionForkTarget,
        ) -> Result<meerkat_core::SessionForkResult, SessionError> {
            self.inner
                .fork_persisted_session(
                    source_session_id,
                    message_count,
                    tool_access_policy,
                    target,
                )
                .await
        }

        // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED so
        // every wrapper forwards the boundary-then-fork as ONE contract to the owner;
        // a default that took the boundary and then called `fork_persisted_session`
        // self-deadlocks on the persistent owner's non-reentrant boundary.
        async fn commit_live_delegation_final_transcript_at_turn_boundary(
            &self,
            machine: &meerkat_runtime::MeerkatMachine,
            session_id: &meerkat_core::types::SessionId,
            provisional: meerkat_core::ProvisionalLiveHandoff,
            final_event: meerkat_core::RealtimeTranscriptEvent,
            bound: std::time::Duration,
        ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
            self.inner
                .commit_live_delegation_final_transcript_at_turn_boundary(
                    machine,
                    session_id,
                    provisional,
                    final_event,
                    bound,
                )
                .await
        }

        async fn fork_persisted_session_at_turn_boundary(
            &self,
            source_session_id: &meerkat_core::types::SessionId,
            message_count: Option<usize>,
            tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            target: meerkat_core::DurableSessionForkTarget,
            bound: std::time::Duration,
        ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
            self.inner
                .fork_persisted_session_at_turn_boundary(
                    source_session_id,
                    message_count,
                    tool_access_policy,
                    target,
                    bound,
                )
                .await
        }

        async fn load_persisted_session_metadata(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
            self.inner.load_persisted_session_metadata(session_id).await
        }

        async fn archive_with_mob_lifecycle_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner
                .archive_with_mob_lifecycle_authority(session_id)
                .await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner
                .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                .await
        }

        // New in 0.8.22 (deadline-aware retirement archive). This is the seam
        // the mob provisioner's member-session disposal actually calls; its
        // trait default is `Err(Unsupported)`, so a passthrough that does not
        // forward would turn every retirement archive into a split-state
        // escalation instead of using the inner service's archive.
        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
            &self,
            session_id: &SessionId,
            deadline: meerkat_core::time_compat::Instant,
        ) -> Result<(), SessionError> {
            self.inner
                .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
                    session_id, deadline,
                )
                .await
        }

        async fn apply_runtime_turn(
            &self,
            session_id: &SessionId,
            run_id: meerkat_core::RunId,
            req: StartTurnRequest,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<meerkat_core::InputId>,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
            self.inner
                .apply_runtime_turn(session_id, run_id, req, boundary, contributing_input_ids)
                .await
        }

        async fn prepare_turn_boundary_delivery_for_active_turn(
            &self,
            session_id: &SessionId,
            expected_run_id: &meerkat_core::RunId,
            delivery: meerkat_core::TurnBoundaryDelivery,
        ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError>
        {
            self.inner
                .prepare_turn_boundary_delivery_for_active_turn(
                    session_id,
                    expected_run_id,
                    delivery,
                )
                .await
        }

        async fn checkpoint_committed_runtime_session_snapshot(
            &self,
            session_id: &SessionId,
            session_snapshot: std::sync::Arc<Vec<u8>>,
        ) -> Result<(), SessionError> {
            self.inner
                .checkpoint_committed_runtime_session_snapshot(session_id, session_snapshot)
                .await
        }

        async fn acquire_runtime_turn_finalization_guard(
            &self,
            session_id: &SessionId,
        ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
        {
            self.inner
                .acquire_runtime_turn_finalization_guard(session_id)
                .await
        }

        async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
            &self,
            session_id: &SessionId,
            session_snapshot: std::sync::Arc<Vec<u8>>,
        ) -> Result<(), SessionError> {
            self.inner
                .checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                    session_id,
                    session_snapshot,
                )
                .await
        }

        async fn discard_live_session_after_runtime_stop_terminalized(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner
                .discard_live_session_after_runtime_stop_terminalized(session_id)
                .await
        }

        async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner
                .discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
                    session_id,
                )
                .await
        }

        // 0.8.22 keys terminal publication by the service-minted actor
        // incarnation instead of the SessionId, so a delayed predecessor
        // callback can no longer land on a successor actor.
        async fn publish_interaction_terminals_for_actor(
            &self,
            actor_witness: &meerkat_session::LiveSessionActorWitness,
            events: &[meerkat_core::event::AgentEvent],
        ) -> Result<
            Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
            SessionError,
        > {
            self.inner
                .publish_interaction_terminals_for_actor(actor_witness, events)
                .await
        }

        async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
            self.inner.discard_live_session(session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.inner
                .discard_live_session_under_runtime_turn_boundary(session_id)
                .await
        }

        async fn discard_live_session_actor_under_runtime_turn_boundary(
            &self,
            witness: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<bool, SessionError> {
            self.inner
                .discard_live_session_actor_under_runtime_turn_boundary(witness)
                .await
        }

        async fn await_event_projection_drain(
            &self,
            session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            self.inner.await_event_projection_drain(session_id).await
        }

        async fn cancel_all_checkpointers(&self) {
            self.inner.cancel_all_checkpointers().await;
        }

        async fn rearm_all_checkpointers(&self) {
            self.inner.rearm_all_checkpointers().await;
        }
    }

    #[async_trait::async_trait]
    impl ConsoleLogStore for CountingConsoleLogStore {
        async fn append_if_absent(
            &self,
            frame: NewConsoleFrame,
        ) -> ConsoleLogResult<AppendOutcome> {
            if frame.kind == "runtime_notice_snapshot" {
                let gate = self
                    .snapshot_append_gate
                    .lock()
                    .expect("append gate lock")
                    .take();
                if let Some(gate) = gate {
                    gate.entered.add_permits(1);
                    gate.release
                        .acquire()
                        .await
                        .expect("append release gate open")
                        .forget();
                }
            }
            self.inner.append_if_absent(frame).await
        }

        async fn update_frame_status(
            &self,
            frame_id: &str,
            status: ConsoleFrameStatus,
        ) -> ConsoleLogResult<Option<ConsoleFrame>> {
            self.inner.update_frame_status(frame_id, status).await
        }

        async fn query_frames(
            &self,
            query: ConsoleTimelineQuery,
        ) -> ConsoleLogResult<ConsoleTimelinePage> {
            self.inner.query_frames(query).await
        }

        async fn query_windowed_frames(
            &self,
            mut query: ConsoleTimelineWindowQuery,
        ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
            query.limit = query.limit.min(self.window_page_limit);
            let mut page = self.inner.query_windowed_frames(query).await?;
            if self.legacy_empty_page {
                page.exhausted = false;
            }
            Ok(page)
        }

        async fn frame_by_dedupe_key(
            &self,
            dedupe_key: &str,
        ) -> ConsoleLogResult<Option<ConsoleFrame>> {
            self.inner.frame_by_dedupe_key(dedupe_key).await
        }

        async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
            self.inner.latest_cursor().await
        }

        async fn clear_frames(&self) -> ConsoleLogResult<()> {
            self.inner.clear_frames().await
        }

        async fn record_source_watermark(
            &self,
            runtime_key: &str,
            source_kind: ConsoleFrameSourceKind,
            source_cursor: &str,
        ) -> ConsoleLogResult<()> {
            self.record_watermark_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .record_source_watermark(runtime_key, source_kind, source_cursor)
                .await
        }

        async fn source_watermark(
            &self,
            runtime_key: &str,
            source_kind: ConsoleFrameSourceKind,
        ) -> ConsoleLogResult<Option<String>> {
            self.source_watermark_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.source_watermark(runtime_key, source_kind).await
        }
    }

    /// `TestClient::default()` DOES synthesize accounting under 0.8.22, but
    /// under `Provider::Other` - and every profile here is `gpt-5.5`, whose
    /// canonical owner is `Provider::OpenAI`, so the turn would fail closed
    /// with `normalized_provider_accounting_identity_mismatch`. See the rule
    /// in `tests/support/llm_usage.rs`.
    /// Per-test mob id: 0.8.23's fail-closed in-proc registration means
    /// concurrently running tests must not share a supervisor participant
    /// name, so every bootstrap gets its own id.
    fn unique_console_mob_id(prefix: &str) -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "{prefix}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    async fn build_single_member_runtime() -> UnifiedRuntime {
        build_single_member_runtime_with_client(Arc::new(TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )))
        .await
    }

    async fn build_single_member_runtime_with_client(client: Arc<dyn LlmClient>) -> UnifiedRuntime {
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
            unique_console_mob_id("console-aggregator-perf-test")
        ))
        .expect("definition parses");
        let runtime = UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(client)
            .build()
            .await
            .expect("runtime builds");
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "agent-a".to_string(),
                Some("You are agent-a.".into()),
                None,
                None,
            ))
            .await
            .expect("member spawns");
        runtime
    }

    async fn build_empty_runtime(mob_id: &str) -> UnifiedRuntime {
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("definition parses");
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::for_provider(
                meerkat_core::Provider::OpenAI,
            )))
            .build()
            .await
            .expect("runtime builds")
    }

    /// Build an intentionally identity-projected lower-plane member for
    /// aggregator authority tests. Public raw spawn APIs reserve
    /// `labels.agent_identity` for the identity runtime; these fixtures are
    /// modeling the already-authorized output of that runtime.
    async fn spawn_trusted_identity_projected_member(
        runtime: &UnifiedRuntime,
        mut spec: SpawnMemberSpec,
    ) {
        let alias = spec.identity.to_string();
        spec.identity = crate::member_comms_id::mob_member_id(&alias);
        runtime
            .mob_handle()
            .spawn_spec(spec)
            .await
            .expect("trusted identity-projected member spawns");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_selection_ignores_a_retiring_generation_when_one_active_generation_remains() {
        let runtime = build_empty_runtime("console-send-retiring-generation-test").await;
        let labels = BTreeMap::from([("agent_identity".to_string(), "builder".to_string())]);
        for generation in 0..=1 {
            spawn_trusted_identity_projected_member(
                &runtime,
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    format!("rt:builder:{generation}"),
                    Some("You are Builder.".into()),
                    None,
                    None,
                )
                .with_labels(labels.clone()),
            )
            .await;
        }

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        let mut matches = aggregator.resolve_visible_members("builder").await;
        assert_eq!(matches.len(), 2);
        assert_eq!(sendable_resolved_member_indices(&matches).len(), 2);

        matches[0].member.status = meerkat_mob::MobMemberStatus::Retiring;
        let sendable = sendable_resolved_member_indices(&matches);
        assert_eq!(sendable.len(), 1);
        assert_ne!(
            matches[sendable[0]].member.status,
            meerkat_mob::MobMemberStatus::Retiring
        );

        let _ = runtime.mob_handle().stop().await;
    }

    /// Regression (downstream app dispatch-path mirroring asymmetry): a console
    /// send targets the DURABLE identity, but the member is rostered under
    /// its fenced runtime incarnation (`rt:{identity}:{generation}`). The
    /// send's interaction reservation must key the pending interaction under
    /// the durable identity and register the incarnation -> durable mapping -
    /// exactly what the spawn path registers. The pre-fix reservation
    /// self-mapped the incarnation, clobbering the spawn registration, so
    /// every live completion event landed on the `identity:...:0`
    /// conversation while the UI thread (the durable conversation) showed
    /// only the user_input frame.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn console_send_keys_live_completion_events_on_the_durable_conversation() {
        let runtime = build_empty_runtime("console-send-dispatch-mirroring-test").await;
        let labels = BTreeMap::from([("agent_identity".to_string(), "builder".to_string())]);
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:builder:0".to_string(),
                Some("You are Builder.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        // The spawn path's registration contract (`project_spawned_member`):
        // the runtime incarnation resolves to the durable console identity.
        runtime
            .console_events()
            .register_runtime_identity("rt:builder:0", "builder")
            .await;

        let accepted = aggregator
            .send(ConsoleSendRequest {
                identity: "builder".to_string(),
                content: serde_json::json!("status sweep"),
                origin: "console:test".to_string(),
                idempotency_key: "dispatch-mirroring-1".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await
            .expect("console send accepted");

        // Admission reserves the interaction; only the matching typed run
        // input establishes which live run owns it. This fixture does not
        // start the mob event drain, so these owner events are projected
        // sequentially without racing a second, real completion.
        runtime
            .console_events()
            .project_unified_event(&crate::types::EventEnvelope {
                event_id: "evt-dispatch-run-started".to_string(),
                source: "test".to_string(),
                timestamp_ms: 9,
                event: crate::types::UnifiedEvent::Agent {
                    agent_id: "rt:builder:0:0".to_string(),
                    event_type: "run_started".to_string(),
                    payload: Some(serde_json::json!({
                        "input": { "kind": "content", "content": "status sweep" },
                    })),
                },
            })
            .await;

        // The completion uses the same member AgentRuntimeId as its start
        // (`{roster alias}:{generation}`), as forwarded by the mob event drain.
        runtime
            .console_events()
            .project_unified_event(&crate::types::EventEnvelope {
                event_id: "evt-dispatch-run-completed".to_string(),
                source: "test".to_string(),
                timestamp_ms: 10,
                event: crate::types::UnifiedEvent::Agent {
                    agent_id: "rt:builder:0:0".to_string(),
                    event_type: "run_completed".to_string(),
                    payload: Some(serde_json::json!({ "result": "done" })),
                },
            })
            .await;

        let replay = runtime
            .console_events()
            .replay_all(None)
            .await
            .expect("console event replay");
        let run_started = replay
            .iter()
            .find(|event| event.event_id == "evt-dispatch-run-started")
            .expect("matching start event projected");
        assert_eq!(run_started.identity, "builder");
        assert_eq!(
            run_started.interaction_id.as_deref(),
            Some(accepted.interaction_id.as_str()),
            "the exact console input must bind the reserved interaction at run start"
        );
        let completion = replay
            .iter()
            .find(|event| event.event_id == "evt-dispatch-run-completed")
            .expect("completion event projected");
        assert_eq!(
            completion.identity, "builder",
            "a console-send-driven completion must key the durable conversation, \
             not the runtime incarnation"
        );
        assert_eq!(
            completion.interaction_id.as_deref(),
            Some(accepted.interaction_id.as_str()),
            "the completion must correlate to the console send's reserved interaction"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    /// Downstream app field report (2026-08-17), and the case neither sibling test
    /// covers: a SCHEDULED turn's completions, arriving on a member that was
    /// addressed through console send EARLIER in the same process.
    ///
    /// The two tests around this one both assert a completion that belongs to
    /// the console send - so they prove the reservation keys its OWN
    /// interaction correctly. The reported symptom is the other half: the
    /// self-map clobbered `runtime_to_identity` PROCESS-WIDE, so every later
    /// completion for that member resolved to the incarnation no matter which
    /// ingress produced it. A recurring scheduled prompt reaches the runtime
    /// through `WorkSpec`/`MobHandle` and reserves nothing on the console
    /// store, so it never keyed anything itself - it was collateral damage,
    /// and it is the path a real deployment actually runs on. The operator-visible
    /// symptom was a scheduled agent that appeared to answer nothing for days
    /// while it was in fact answering.
    ///
    /// The completion here is deliberately UNCORRELATED to the send: a
    /// distinct event id, arriving later, standing in for a turn the console
    /// never initiated.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scheduled_turn_completions_key_the_durable_conversation_after_a_console_send() {
        let runtime = build_empty_runtime("console-scheduled-mirroring-test").await;
        let labels = BTreeMap::from([("agent_identity".to_string(), "builder".to_string())]);
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:builder:0".to_string(),
                Some("You are Builder.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        runtime
            .console_events()
            .register_runtime_identity("rt:builder:0", "builder")
            .await;

        // The operation that used to poison the map for everything after it.
        aggregator
            .send(ConsoleSendRequest {
                identity: "builder".to_string(),
                content: serde_json::json!("status sweep"),
                origin: "console:test".to_string(),
                idempotency_key: "scheduled-mirroring-1".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await
            .expect("console send accepted");

        // A later, independent turn's completion - the shape a scheduled
        // prompt produces, forwarded by the mob event drain with the member's
        // AgentRuntimeId (`{roster alias}:{generation}`).
        runtime
            .console_events()
            .project_unified_event(&crate::types::EventEnvelope {
                event_id: "evt-scheduled-run-completed".to_string(),
                source: "test".to_string(),
                timestamp_ms: 20,
                event: crate::types::UnifiedEvent::Agent {
                    agent_id: "rt:builder:0:0".to_string(),
                    event_type: "run_completed".to_string(),
                    payload: Some(serde_json::json!({ "result": "published" })),
                },
            })
            .await;

        let replay = runtime
            .console_events()
            .replay_all(None)
            .await
            .expect("console event replay");
        let completion = replay
            .iter()
            .find(|event| event.event_id == "evt-scheduled-run-completed")
            .expect("completion event projected");
        assert_eq!(
            completion.identity, "builder",
            "a scheduled turn's completion must key the durable conversation the console \
             renders, even when a console send preceded it in the same process - keying it on \
             the incarnation is what made a working agent look silent for days"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    /// The channel-path twin of the dispatch-mirroring regression: a live
    /// completion for a member that was NOT addressed through console send
    /// (channel deliveries reserve nothing on the console store) resolves
    /// through the spawn registration alone and must land on the durable
    /// conversation. Keeps the two origins provably symmetric.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn channel_origin_completion_events_key_the_durable_conversation() {
        let runtime = build_empty_runtime("console-channel-mirroring-test").await;
        let labels = BTreeMap::from([("agent_identity".to_string(), "builder".to_string())]);
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:builder:0".to_string(),
                Some("You are Builder.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;
        runtime
            .console_events()
            .register_runtime_identity("rt:builder:0", "builder")
            .await;

        runtime
            .console_events()
            .project_unified_event(&crate::types::EventEnvelope {
                event_id: "evt-channel-run-completed".to_string(),
                source: "test".to_string(),
                timestamp_ms: 10,
                event: crate::types::UnifiedEvent::Agent {
                    agent_id: "rt:builder:0:0".to_string(),
                    event_type: "run_completed".to_string(),
                    payload: Some(serde_json::json!({ "result": "done" })),
                },
            })
            .await;

        let replay = runtime
            .console_events()
            .replay_all(None)
            .await
            .expect("console event replay");
        let completion = replay
            .iter()
            .find(|event| event.event_id == "evt-channel-run-completed")
            .expect("completion event projected");
        assert_eq!(
            completion.identity, "builder",
            "a channel-origin completion must key the durable conversation"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    pub(super) async fn build_stress_runtime(
        member_count: usize,
        history_delay: Duration,
    ) -> (
        tempfile::TempDir,
        Arc<UnifiedRuntime>,
        DelayedHistorySessionService,
    ) {
        build_stress_runtime_with_durable_runtime_store(member_count, history_delay, None).await
    }

    /// [`build_stress_runtime`], optionally composed with a durable runtime
    /// store (as both gateways compose one) that answers session-boundary
    /// authority reads. The fixture's sessions never commit through it, so a
    /// session's committed authority moves only when a test commits one
    /// ([`commit_fixture_session_boundary`]).
    pub(super) async fn build_stress_runtime_with_durable_runtime_store(
        member_count: usize,
        history_delay: Duration,
        durable_runtime_store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    ) -> (
        tempfile::TempDir,
        Arc<UnifiedRuntime>,
        DelayedHistorySessionService,
    ) {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let session_path = temp_dir.path().join("sessions");
        std::fs::create_dir_all(&session_path).expect("session path");
        let factory = AgentFactory::new(&session_path).comms(true);
        let base_service = Arc::new(build_ephemeral_service(
            factory,
            Config::default(),
            member_count + 8,
        ));
        let delayed_service = DelayedHistorySessionService::new(base_service, history_delay);
        let session_service: Arc<dyn MobSessionService> = Arc::new(delayed_service.clone());
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
            unique_console_mob_id("console-aggregator-stress-test")
        ))
        .expect("definition parses");
        let mut spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
            .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(TestClient::for_provider(
                    meerkat_core::Provider::OpenAI,
                ))),
            });
        if let Some(store) = durable_runtime_store.as_ref() {
            spec = spec.with_durable_runtime_store(store);
        }
        let runtime = Arc::new(
            UnifiedRuntime::bootstrap(
                spec,
                crate::types::MetaConfig {
                    modules: Vec::new(),
                    discovery: crate::types::DiscoverySpec {
                        namespace: "stress".to_string(),
                        modules: Vec::new(),
                    },
                    pre_spawn: Vec::new(),
                },
                Duration::from_secs(5),
            )
            .await
            .expect("runtime boots"),
        );
        for idx in 0..member_count {
            runtime
                .spawn(SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    format!("agent-{idx}"),
                    Some(format!("You are agent-{idx}.").into()),
                    None,
                    None,
                ))
                .await
                .expect("member spawns");
        }
        (temp_dir, runtime, delayed_service)
    }

    /// Commit one session boundary for `session_id` to a fixture's durable
    /// runtime store, so the session's committed authority moves to a new
    /// revision. `note` varies the committed document.
    pub(super) async fn commit_fixture_session_boundary(
        store: &dyn meerkat_runtime::RuntimeStore,
        session_id: &str,
        note: &str,
    ) {
        let session_id = SessionId::parse(session_id).expect("fixture session id parses");
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(Message::User(meerkat_core::types::UserMessage::text(note)));
        let bytes = session
            .to_persisted_bytes()
            .expect("fixture session serializes");
        meerkat_runtime::RuntimeStore::commit_session_snapshot(
            store,
            &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
            meerkat_runtime::store::SerializedSessionSnapshot {
                session_snapshot: Arc::new(bytes),
            },
        )
        .await
        .expect("fixture session boundary commits");
    }

    pub(super) fn runtime_entry_for_test(
        runtime_key: &str,
        runtime: &UnifiedRuntime,
    ) -> RuntimeEntry {
        RuntimeEntry {
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: runtime_key.to_string(),
            identity_namespace: "test".to_string(),
            runtime: runtime.mob_runtime().clone(),
            identity_runtime: runtime
                .identity_runtime()
                .map(|identity_runtime| Arc::clone(identity_runtime.runtime())),
            identity_owner: None,
            memory_panel: None,
            console_events: runtime.console_events(),
            visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
        }
    }

    fn console_envelope_for_test(
        event_id: &str,
        interaction_id: Option<&str>,
        event_type: &str,
        data: Value,
    ) -> crate::console_contracts::ConsoleIdentityEventEnvelope {
        crate::console_contracts::ConsoleIdentityEventEnvelope {
            event_id: event_id.to_string(),
            interaction_id: interaction_id.map(ToString::to_string),
            identity: "agent-a".to_string(),
            event_type: event_type.to_string(),
            timestamp_ms: 1_000,
            data,
        }
    }

    pub(super) fn identity_record_for_test(identity: &str) -> ConsoleIdentityRecord {
        ConsoleIdentityRecord {
            identity: identity.to_string(),
            display_name: identity.to_string(),
            runtime_key: "runtime-cache".to_string(),
            runtime_member_id: identity.to_string(),
            session_id: Some(format!("session-{identity}")),
            member_id: None,
            visibility: ConsoleVisibility::Addressable,
            addressable: true,
            health: "ready".to_string(),
            topology_peers: Vec::new(),
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn dedupe_identity_records_preserves_stale_durable_and_live_alias_bindings() {
        let mut durable = identity_record_for_test("test/review:singleton");
        durable.runtime_member_id = "rt:review:singleton:1".to_string();
        durable.session_id = Some("session-stale".to_string());
        durable
            .labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());

        let mut live = identity_record_for_test("test/review:singleton");
        live.runtime_member_id = "rt:review:singleton:0".to_string();
        live.session_id = Some("session-live".to_string());
        live.labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());

        let records = dedupe_identity_records(vec![durable, live]);

        assert_eq!(
            records.len(),
            2,
            "list projection must expose stale durable/live alias split-brain instead of picking one record"
        );
        assert!(
            records
                .iter()
                .any(|record| record.runtime_member_id == "rt:review:singleton:0")
        );
        assert!(
            records
                .iter()
                .any(|record| record.runtime_member_id == "rt:review:singleton:1")
        );
    }

    #[test]
    fn dedupe_identity_records_preserves_duplicate_live_aliases() {
        let mut first = identity_record_for_test("test/review:singleton");
        first.runtime_member_id = "rt:review:singleton:0".to_string();
        first
            .labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());

        let mut second = identity_record_for_test("test/review:singleton");
        second.runtime_member_id = "rt:review:singleton:1".to_string();
        second
            .labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());

        let records = dedupe_identity_records(vec![first, second]);

        assert_eq!(
            records.len(),
            2,
            "duplicate live aliases must remain visible so controls and roster agree on ambiguity"
        );
    }

    #[test]
    fn dedupe_identity_records_preserves_same_member_ids_from_different_source_mobs() {
        let mut first = identity_record_for_test("test/review:singleton");
        first.runtime_member_id = "rt:review:singleton:0".to_string();
        first.session_id = None;
        first
            .labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());
        first
            .labels
            .insert("source_mob_id".to_string(), "mob-a".to_string());

        let mut second = first.clone();
        second
            .labels
            .insert("source_mob_id".to_string(), "mob-b".to_string());

        let records = dedupe_identity_records(vec![first, second]);

        assert_eq!(
            records.len(),
            2,
            "same member/session aliases from different source mobs must remain visible"
        );
    }

    #[test]
    fn dedupe_identity_records_merges_matching_durable_and_primary_live_rows() {
        let durable = identity_record_for_test("test/review:singleton");
        let live = durable.clone();

        let records = dedupe_identity_records(vec![durable, live]);

        assert_eq!(
            records.len(),
            1,
            "healthy durable and primary live projections should not duplicate the roster"
        );
    }

    #[test]
    fn dedupe_identity_records_merges_a_live_roster_row_into_its_generated_binding() {
        // Since the roster lowering, a live roster row's member id is the bare
        // durable identity, while the durable row names the generated binding.
        let mut durable = identity_record_for_test("test/review:singleton");
        durable.runtime_member_id = "rt:review:singleton:0".to_string();
        let mut live = identity_record_for_test("test/review:singleton");
        live.runtime_member_id = "review:singleton".to_string();
        live.labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());

        let records = dedupe_identity_records(vec![durable.clone(), live.clone()]);
        assert_eq!(
            records.len(),
            1,
            "a live roster row named by its identity realizes the durable binding: {records:#?}"
        );
        assert_eq!(
            records[0].runtime_member_id, "rt:review:singleton:0",
            "the merged row keeps the binding's public runtime alias"
        );

        let mut other_session = live.clone();
        other_session.session_id = Some("session-other".to_string());
        assert_eq!(
            dedupe_identity_records(vec![durable.clone(), other_session]).len(),
            2,
            "a roster row on another session is a split-brain, not the same binding"
        );

        let mut other_generation = durable.clone();
        other_generation.runtime_member_id = "rt:review:singleton:1".to_string();
        assert_eq!(
            dedupe_identity_records(vec![durable, other_generation, live]).len(),
            3,
            "two generations of the binding stay apart, and the roster row realizes neither alone"
        );
    }

    #[test]
    fn dedupe_identity_records_keeps_the_durable_member_id_when_the_live_row_wins() {
        // The durable row carries Core's member id of the binding (the
        // census and `meta/status_identity` value); the live roster
        // projection of the same binding, which the list prefers, has none.
        let member_id = crate::member_comms_id::roster_member_id_for_identity("review:singleton")
            .as_str()
            .to_string();
        let mut durable = identity_record_for_test("test/review:singleton");
        durable.runtime_member_id = "rt:review:singleton:0".to_string();
        durable.member_id = Some(member_id.clone());
        let mut live = identity_record_for_test("test/review:singleton");
        live.runtime_member_id = "review:singleton".to_string();
        live.labels
            .insert("agent_identity".to_string(), "review:singleton".to_string());

        for records in [
            vec![durable.clone(), live.clone()],
            vec![live.clone(), durable.clone()],
        ] {
            let records = dedupe_identity_records(records);
            assert_eq!(records.len(), 1, "{records:#?}");
            assert_eq!(records[0].runtime_member_id, "rt:review:singleton:0");
            assert_eq!(
                records[0].member_id.as_deref(),
                Some(member_id.as_str()),
                "the merged row keeps the binding's member id: {records:#?}"
            );
        }

        // A roster row on another session is another binding: it never takes
        // the durable row's member id.
        let mut other_session = live;
        other_session.session_id = Some("session-other".to_string());
        let records = dedupe_identity_records(vec![durable, other_session]);
        assert_eq!(records.len(), 2, "{records:#?}");
        assert!(
            records.iter().any(|record| {
                record.session_id.as_deref() == Some("session-other") && record.member_id.is_none()
            }),
            "{records:#?}"
        );
    }

    #[test]
    fn stale_durable_record_error_detects_session_split_brain() {
        let mut durable = identity_record_for_test("test/review:singleton");
        durable.runtime_member_id = "rt:review:singleton:0".to_string();
        durable.session_id = Some("session-durable".to_string());

        let mut live = durable.clone();
        live.session_id = Some("session-live".to_string());

        let err = stale_durable_record_error("review:singleton", &durable, &[live])
            .expect("session mismatch must be stale split-brain");

        assert!(
            err.contains("stale durable identity alias"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("session-durable") && err.contains("session-live"),
            "error should name both sessions: {err}"
        );
    }

    #[test]
    fn stable_external_member_alias_realizes_generated_durable_binding() {
        let mut durable = identity_record_for_test("target-smoke");
        durable.runtime_member_id = "rt:target-smoke:0".to_string();

        let mut live = durable.clone();
        live.runtime_member_id = "target-smoke".to_string();
        live.labels
            .insert("agent_identity".to_string(), "target-smoke".to_string());

        assert!(live_record_realizes_durable_binding(&durable, &live));
        assert_eq!(
            stale_durable_record_error("target-smoke", &durable, &[live]),
            None,
            "the bridge-owned stable alias must not be classified as a stale generated binding"
        );
    }

    #[test]
    fn stable_external_member_alias_still_fences_session_split_brain() {
        let mut durable = identity_record_for_test("target-smoke");
        durable.runtime_member_id = "rt:target-smoke:0".to_string();
        durable.session_id = Some("session-durable".to_string());

        let mut live = durable.clone();
        live.runtime_member_id = "target-smoke".to_string();
        live.session_id = Some("session-live".to_string());
        live.labels
            .insert("agent_identity".to_string(), "target-smoke".to_string());

        let error = stale_durable_record_error("target-smoke", &durable, &[live])
            .expect("stable aliases must not hide a session split-brain");
        assert!(error.contains("session-durable"));
        assert!(error.contains("session-live"));
    }

    /// Register bindings whose session is the LIVE row's session.
    ///
    /// `identity_runtime_for_test` mints a random SessionId, which is fine while
    /// nothing compares it, but a binding whose session does not match its live
    /// row is a genuine split-brain and the aggregator is right to classify it as
    /// stale (see stable_external_member_alias_still_fences_session_split_brain).
    /// Tests that assert on a healthy identity need a truthful binding.
    async fn identity_runtime_for_test_with_sessions(
        identities: &[(&str, SessionId)],
    ) -> Result<Arc<crate::identity_first::IdentityRuntime>, Box<dyn std::error::Error + Send + Sync>>
    {
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-identity-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        for (identity, session_id) in identities {
            let identity = crate::identity_first::AgentIdentity::parse(identity)?;
            register_live(
                &runtime,
                &core_bridge.core,
                crate::identity_first::DurableAgentSpec {
                    identity: identity.clone(),
                    profile: meerkat_mob::ProfileName::from("worker"),
                    addressability: crate::identity_first::AgentAddressability::Addressable,
                    display_name: None,
                    labels: BTreeMap::new(),
                    context: None,
                    additional_instructions: Vec::new(),
                    initial_message: None,
                    runtime_mode_override: None,
                    backend: None,
                    binding: None,
                    placement: None,
                },
                0,
                session_id.clone(),
            )
            .await?;
        }
        Ok(runtime)
    }

    /// An identity runtime over a Core that holds each identity's member at
    /// generation 0, bound to a fresh session.
    async fn identity_runtime_for_test(
        identities: &[&str],
    ) -> Result<Arc<crate::identity_first::IdentityRuntime>, Box<dyn std::error::Error + Send + Sync>>
    {
        let identities = identities
            .iter()
            .map(|identity| (*identity, SessionId::new()))
            .collect::<Vec<_>>();
        identity_runtime_for_test_with_sessions(&identities).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_id_alias_does_not_fall_back_to_durable_rt_identity()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("runtime-id-durable-shadow-test").await;
        let identity_runtime = identity_runtime_for_test(&["rt:review:singleton:0"]).await?;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        assert!(
            aggregator
                .inspect_identity("rt:review:singleton:0")
                .await?
                .is_none(),
            "runtime-member-id shaped requests must not inspect a durable rt:* identity"
        );
        assert!(
            !aggregator.retire_identity("rt:review:singleton:0").await?,
            "runtime-member-id shaped requests must not retire a durable rt:* identity"
        );
        let reserve_err = aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "rt:review:singleton:0".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "rt-shadow-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await
            .expect_err(
                "runtime-member-id shaped reserve must not fall back to a durable rt:* identity",
            );
        assert!(
            reserve_err.to_string().contains("unknown identity"),
            "unexpected reserve error: {reserve_err}"
        );
        let send_err = aggregator
            .send(ConsoleSendRequest {
                identity: "rt:review:singleton:0".to_string(),
                content: json!("hello"),
                origin: "console:test".to_string(),
                idempotency_key: "rt-shadow-send".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await
            .expect_err(
                "runtime-member-id shaped send must not be blocked by a durable rt:* identity",
            );
        assert!(
            send_err.to_string().contains("unknown identity"),
            "unexpected send error: {send_err}"
        );
        let blob_err = match aggregator
            .binary_blob_store_for_identity("rt:review:singleton:0")
            .await
        {
            Ok(_) => panic!(
                "runtime-member-id shaped blob lookup must not fall back to durable rt:* identity"
            ),
            Err(err) => err,
        };
        assert!(
            blob_err.to_string().contains("unknown identity"),
            "unexpected blob error: {blob_err}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespaced_current_runtime_alias_retires_through_durable_authority()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("namespaced-runtime-alias-retire-test").await;
        let runtime_alias = "rt:review:owned:0";
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                runtime_alias.to_string(),
                Some("You are the owned review agent.".into()),
                None,
                None,
            )
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                "review:owned".to_string(),
            )])),
        )
        .await;
        let live_session_id = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                runtime_alias,
            ))
            .await
            .ok_or("live member has no bridge session")?;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "namespaced-runtime-alias-retire-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let durable_identity = crate::identity_first::AgentIdentity::parse("review:owned")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: durable_identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            live_session_id,
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "tenant",
            runtime.mob_runtime().clone(),
            Some(identity_runtime.clone()),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        assert!(
            aggregator
                .retire_identity("tenant/rt:review:owned:0")
                .await?,
            "the namespaced current runtime alias must resolve"
        );
        assert_eq!(
            identity_runtime.status(&durable_identity).await?.state,
            crate::identity_first::IdentityLifecycleState::Dormant,
            "retirement must advance the durable lifecycle authority"
        );
        let raw_member = runtime
            .mob_handle()
            .list_members_including_retiring()
            .await
            .into_iter()
            .find(|entry| {
                crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str())
                    == runtime_alias
            })
            .ok_or("raw member disappeared")?;
        assert_eq!(
            raw_member.status,
            meerkat_mob::MobMemberStatus::Active,
            "the test bridge retires only in Core's history, so an Active raw member proves the \
             aggregator did not fall through to raw Mob"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// The aggregator resolves a CURRENT runtime alias to the durable roster
    /// row its identity owns: the roster is keyed by the encoded durable
    /// identity, so encoding the alias itself found nothing and the console
    /// retire answered "no member" for a healthy one. Only the identity
    /// authority can say which generation is current, so a stale alias still
    /// resolves nothing. An encoded roster id is decoded, not encoded twice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn current_runtime_alias_resolves_the_durable_roster_row()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const DURABLE: &str = "review:aggregator-alias";
        let runtime = build_empty_runtime("aggregator-current-alias-test").await;
        // Labelled as identity-first materialization labels its members: the
        // label binds the live row to the durable record.
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                DURABLE.to_string(),
                Some("You are the aggregator alias review agent.".into()),
                None,
                None,
            )
            .with_labels(BTreeMap::from([(
                "agent_identity".to_string(),
                DURABLE.to_string(),
            )])),
        )
        .await;
        let live_session_id = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(DURABLE))
            .await
            .ok_or("live member has no bridge session")?;
        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "aggregator-current-alias-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let durable_identity = crate::identity_first::AgentIdentity::parse(DURABLE)?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: durable_identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            live_session_id,
        )
        .await?;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime.clone()),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        // Every spelling of the member resolves its one roster row: the
        // durable identity, its encoded roster id, and its current runtime
        // alias. A stale generation resolves nothing.
        let rows_for = async |spelling: String| {
            aggregator
                .resolve_members(&spelling)
                .await
                .into_iter()
                .map(|resolved| resolved.runtime_identity)
                .collect::<Vec<_>>()
        };
        let encoded = crate::member_comms_id::mob_member_id_str(DURABLE).into_owned();
        assert_eq!(
            (
                rows_for(DURABLE.to_string()).await,
                rows_for(encoded).await,
                rows_for(format!("rt:{DURABLE}:0")).await,
                rows_for(format!("rt:{DURABLE}:7")).await,
            ),
            (
                vec![DURABLE.to_string()],
                vec![DURABLE.to_string()],
                vec![DURABLE.to_string()],
                Vec::<String>::new(),
            ),
            "(durable, encoded roster id, current alias, stale alias) resolved rows"
        );

        // A refusal and "no member" both mean the stale alias retired nothing.
        let stale = matches!(
            aggregator.retire_identity(&format!("rt:{DURABLE}:7")).await,
            Ok(true)
        );
        let current = aggregator
            .retire_identity(&format!("rt:{DURABLE}:0"))
            .await?;
        assert_eq!(
            (stale, current),
            (false, true),
            "(stale alias retired, current alias retired)"
        );
        assert_eq!(
            identity_runtime.status(&durable_identity).await?.state,
            crate::identity_first::IdentityLifecycleState::Dormant,
            "the current alias must retire through the durable lifecycle authority"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identity_record_uses_durable_agent_identity_label_for_identity_first_members()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-label-test").await;
        let mut labels = BTreeMap::new();
        labels.insert(
            "agent_identity".to_string(),
            "channel:C0SMOKEOPS".to_string(),
        );
        labels.insert("display_name".to_string(), "C0SMOKEOPS".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:channel:C0SMOKEOPS:0".to_string(),
                Some("You are C0SMOKEOPS.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        let entry = RuntimeEntry {
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: "runtime-a".to_string(),
            identity_namespace: String::new(),
            runtime: runtime.mob_runtime().clone(),
            identity_runtime: runtime
                .identity_runtime()
                .map(|identity_runtime| Arc::clone(identity_runtime.runtime())),
            identity_owner: None,
            memory_panel: None,
            console_events: runtime.console_events(),
            visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
        };
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry);

        let records = aggregator.list_identities_fresh().await?;
        let record = records
            .iter()
            .find(|record| record.identity == "channel:C0SMOKEOPS")
            .expect("durable identity is exposed");
        assert_eq!(record.runtime_member_id, "rt:channel:C0SMOKEOPS:0");

        let inspection = aggregator
            .inspect_identity("channel:C0SMOKEOPS")
            .await?
            .expect("durable identity resolves back to runtime member");
        assert_eq!(inspection.identity.identity, "channel:C0SMOKEOPS");
        assert_eq!(
            inspection.identity.runtime_member_id,
            "rt:channel:C0SMOKEOPS:0"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// Issue #254 (root cause of "flows stuck forever"): the live SSE gate
    /// passed `allow_historical_identity = false` unconditionally, so an
    /// identity-scoped stream dropped every frame of a member the identity
    /// read model had not yet observed — while the windowed query path
    /// passed the same frames via the own-identity allowance. The
    /// subscriber-aware gate grants the same allowance, and an unknown
    /// identity triggers a debounced read-model refresh so UNSCOPED streams
    /// converge too (members spawned mid-run via ensure_member).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identity_scoped_live_stream_sees_members_unknown_to_the_read_model()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("live-tail-unknown-identity-test").await;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_owned_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            runtime.identity_runtime(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        // Prime the read model BEFORE the member exists — the exact state a
        // mid-run ensure_member leaves an already-connected live stream in.
        let _ = aggregator.list_identities_fresh().await?;

        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "late-member".to_string(),
                Some("You are the late member.".into()),
                None,
                None,
            ))
            .await?;
        let outcome = aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "late-member-live-frame".to_string(),
                timestamp_ms: 1,
                runtime_key: "runtime-a".to_string(),
                identity: "late-member".to_string(),
                conversation_id: Some("late-member".to_string()),
                session_id: None,
                kind: "text_delta".to_string(),
                status: ConsoleFrameStatus::Completed,
                payload: json!({"delta": "hi"}),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: None,
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await?;
        let event = ConsoleTimelineEvent::ConsoleFrame {
            frame: outcome.frame,
        };

        // The identity-scoped subscriber sees the frame IMMEDIATELY (the
        // own-identity allowance the query path always had).
        assert!(
            aggregator
                .timeline_event_visible_for_subscriber(&event, Some("late-member"))
                .await,
            "identity-scoped live stream must not starve on a read-model miss"
        );
        // A differently-scoped subscriber does not inherit the allowance.
        // Race note (CI): the spawn's own projections can converge the read
        // model before this line, after which the frame is legitimately
        // visible to everyone because the identity is KNOWN — that is not an
        // allowance leak. The leak we guard against is other-visibility
        // WHILE the unscoped gate still hides the frame (identity unknown).
        let visible_to_other = aggregator
            .timeline_event_visible_for_subscriber(&event, Some("someone-else"))
            .await;
        if visible_to_other {
            assert!(
                aggregator.timeline_event_visible(&event).await,
                "the allowance is strictly own-identity: someone-else saw the \
                 frame while the read model still treats the identity as \
                 unknown (unscoped gate hides it)"
            );
        }

        // The miss triggered refresh_soon: the read model converges, after
        // which even the UNSCOPED gate passes the frame. Convergence is
        // structural - the debounced refresh either lands or it never will -
        // so poll for it rather than budgeting 40 tries * 50ms (a 2s ceiling
        // that full-suite load reached on its own).
        crate::test_wait::poll_until(
            "the unknown-identity miss must self-heal the read model (refresh_soon never made \
             the frame visible to the unscoped gate)",
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || aggregator.timeline_event_visible(&event).await,
        )
        .await;

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// Issue #254 follow-up: registrations leaked an immortal live-projection
    /// task (broadcast receiver never sees Closed while the runtime Arc
    /// lives) plus the discovery loop. unregister_runtime detaches the live
    /// source; already-projected frames stay queryable.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unregister_runtime_stops_live_projection()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("unregister-live-projection-test").await;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_owned_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            runtime.identity_runtime(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        aggregator.unregister_runtime("runtime-a");
        assert!(
            aggregator
                .inner
                .runtimes
                .read()
                .expect("runtime registry")
                .is_empty(),
            "unregister must remove the registry entry"
        );
        assert!(
            aggregator
                .inner
                .live_projection_shutdowns
                .lock()
                .expect("shutdown registry")
                .is_empty(),
            "unregister must consume the live-projection shutdown handle"
        );
        // Idempotent on unknown keys.
        aggregator.unregister_runtime("runtime-a");

        // Re-registration works and replaces cleanly.
        aggregator.register_owned_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            runtime.identity_runtime(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        assert_eq!(
            aggregator
                .inner
                .live_projection_shutdowns
                .lock()
                .expect("shutdown registry")
                .len(),
            1
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn console_transport_omits_member_provenance_from_rest_rpc_and_sse()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use axum::body::{Body, BodyDataStream};
        use axum::http::{Request, StatusCode};
        use futures::StreamExt;
        use tower::ServiceExt;

        async fn receive_until(stream: &mut BodyDataStream, marker: &str) -> String {
            let mut output = String::new();
            tokio::time::timeout(Duration::from_secs(2), async {
                while let Some(chunk) = stream.next().await {
                    output.push_str(
                        std::str::from_utf8(&chunk.expect("SSE bytes")).expect("SSE utf8"),
                    );
                    if output.contains(marker) {
                        return;
                    }
                }
                panic!("SSE ended before {marker}");
            })
            .await
            .expect("SSE progress");
            assert!(
                !output.contains("member_provenance"),
                "private witness leaked: {output}"
            );
            assert!(
                !output.contains("private-label"),
                "private label leaked: {output}"
            );
            output
        }

        let aggregator = MetaConsoleAggregator::in_memory();
        let mut frame = frame_from_session_history_message(
            "runtime-a",
            "member",
            "session",
            0,
            json!({"role": "user", "content": "Exact public content."}),
        )
        .expect("frame");
        frame.source.member_provenance = Some(serde_json::from_value(json!({
            "identity": {"identity":"member", "display_name":"Member", "runtime_key":"runtime-a",
                "runtime_member_id":"member", "session_id":"session", "visibility":"addressable",
                "addressable":true, "health":"ready", "labels":{"private-label":"private"}},
            "member": {"agent_identity":"member", "role":"delegate", "state":"running",
                "session_id":"session", "wired_to":[], "labels":{"private-label":"private"}},
            "primary_mob_id":"primary", "source_mob_id":"lower"
        }))?);
        let stored = append_and_emit(&aggregator.inner, frame.clone())
            .await?
            .frame;
        let app = crate::http_console::console_json_router_with_aggregator(
            crate::RuntimeDecisionState::local_console(
                crate::ConsolePolicy {
                    require_app_auth: false,
                    ..Default::default()
                },
                None,
            ),
            aggregator.clone(),
        );
        for request in [
            Request::builder()
                .uri("/console/timeline?identity=member")
                .body(Body::empty())?,
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(
                    &json!({"jsonrpc":"2.0", "id":1,
                    "method":"meta/console/query_timeline", "params":{"identity":"member"}}),
                )?))?,
        ] {
            let response = app.clone().oneshot(request).await?;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
            let body = String::from_utf8(bytes.to_vec())?;
            assert!(
                body.contains("Exact public content."),
                "frame must be present: {body}"
            );
            assert!(!body.contains("member_provenance"));
            assert!(!body.contains("private-label"));
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/console/timeline/stream?identity=member")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let snapshot = receive_until(&mut stream, "snapshot_complete").await;
        assert!(snapshot.contains("Exact public content."));
        frame.dedupe_key = "live-provenance".to_string();
        frame.payload = json!({"content":"Live public content."});
        append_and_emit(&aggregator.inner, frame).await?;
        receive_until(&mut stream, "Live public content.").await;
        update_frame_status_and_emit(&aggregator.inner, &stored.id, ConsoleFrameStatus::Delivered)
            .await?;
        let marker = receive_until(&mut stream, "frame_updated").await;
        assert!(marker.contains("Exact public content."));
        let raw = aggregator
            .store()
            .query_frames(ConsoleTimelineQuery::default())
            .await?;
        assert!(
            raw.frames
                .iter()
                .all(|frame| frame.source.member_provenance.is_some())
        );
        let marker = raw
            .frames
            .iter()
            .find(|frame| frame.kind == "frame_updated")
            .expect("stored marker");
        assert!(
            marker.payload["frame"]["source"]
                .get("member_provenance")
                .is_some(),
            "transport stripping must not alter nested storage provenance"
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retired_member_history_keeps_policy_provenance_after_store_reopen()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let primary = build_empty_runtime(&unique_console_mob_id("provenance-primary")).await;
        let delegate = build_empty_runtime(&unique_console_mob_id("provenance-lower")).await;
        let alias = "history-reviewer";
        delegate
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                alias.to_string(),
                Some("Retained private review history.".into()),
                None,
                None,
            ))
            .await?;
        primary
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("mob state")
            .mob_insert_handle(
                delegate.mob_handle().mob_id().clone(),
                delegate.mob_handle().clone(),
            )
            .await;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("console.sqlite");
        let store = Arc::new(SqliteConsoleLogStore::open(&path)?);
        let options = ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..Default::default()
        };
        let owner = MetaConsoleAggregator::new_with_options(store.clone(), options);
        let entry = runtime_entry_for_test("runtime-a", &primary);
        owner
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".to_string(), entry.clone());
        let identity = format!("test/{alias}");
        let target = session_backfill_target_for_identity(&owner.inner, &identity)
            .await
            .expect("lower member target");
        wait_for_runtime_session_history_text(
            &delegate,
            &target.session_id,
            "Retained private review history.",
            Duration::from_secs(5),
        )
        .await?;
        // These independent test runtimes have distinct session services. Supply
        // the real lower service's history through the canonical admission seam.
        let page = delegate
            .mob_runtime()
            .read_session_history(&target.session_id, 0, Some(100))
            .await?;
        let page = serde_json::to_value(page)?;
        for (offset, message) in page["messages"]
            .as_array()
            .expect("history messages")
            .iter()
            .enumerate()
        {
            for frame in frames_from_session_history_message_with_namespace(
                "runtime-a",
                &identity,
                "test",
                &target.session_id,
                offset,
                message.clone(),
            ) {
                append_and_emit(&owner.inner, frame).await?;
            }
        }
        let raw = store
            .query_frames(ConsoleTimelineQuery {
                identity: Some(identity.clone()),
                limit: 100,
                ..Default::default()
            })
            .await?;
        assert!(
            !raw.frames.is_empty(),
            "real lower member session history was admitted"
        );
        assert!(
            raw.frames
                .iter()
                .all(|frame| frame.source.member_provenance.is_some())
        );
        assert!(
            raw.frames
                .iter()
                .all(|frame| frame.source.kind == ConsoleFrameSourceKind::SessionHistory)
        );
        let persisted_json = serde_json::to_vec(&raw.frames)?;
        let restored_json: Vec<ConsoleFrame> = serde_json::from_slice(&persisted_json)?;
        assert_eq!(
            restored_json, raw.frames,
            "custom JSON storage preserves policy inputs"
        );
        let projected_frames = raw
            .frames
            .iter()
            .cloned()
            .map(|frame| owner.project_frame_for_view(frame))
            .collect::<Vec<_>>();
        assert!(
            serde_json::to_value(&projected_frames)?
                .as_array()
                .expect("frames")
                .iter()
                .all(|frame| frame["source"].get("member_provenance").is_none()),
            "policy-checked REST/RPC/SSE snapshots must omit private provenance"
        );
        for view in [
            &owner,
            &owner.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy)),
        ] {
            let mut marker = raw.frames[0].clone();
            marker.kind = "frame_updated".to_string();
            marker.payload = json!({"frame": raw.frames[0]});
            let projected =
                view.project_event_for_view(ConsoleTimelineEvent::ConsoleFrame { frame: marker });
            let json = serde_json::to_value(projected)?;
            assert!(json["frame"]["source"].get("member_provenance").is_none());
            assert!(
                json["frame"]["payload"]["frame"]["source"]
                    .get("member_provenance")
                    .is_none(),
                "live SSE nested updates also omit private provenance"
            );
        }
        let hidden_lower =
            owner.policy_view(Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy));
        let hidden_member = owner.policy_view(Arc::new(HideRuntimeMemberOnly("history-reviewer")));
        for view in [&hidden_lower, &hidden_member] {
            assert_retired_history_policy(view, &identity, &raw.frames).await?;
        }
        delegate
            .mob_handle()
            .retire(crate::member_comms_id::mob_member_id(alias))
            .await?;
        assert!(
            member_sources_for_entry_including_hidden(&entry)
                .await
                .into_iter()
                .all(|source| source.runtime_identity != alias),
            "actual member removed from live roster"
        );
        for view in [&hidden_lower, &hidden_member] {
            view.list_identities_fresh().await?;
            assert_retired_history_policy(view, &identity, &raw.frames).await?;
        }
        let new_view =
            owner.policy_view(Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy));
        assert_retired_history_policy(&new_view, &identity, &raw.frames).await?;
        let open = owner
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(identity.clone()),
                limit: 100,
                ..Default::default()
            })
            .await?;
        assert_eq!(
            open.frames, projected_frames,
            "open policy retains exact history without private policy inputs"
        );
        drop(new_view);
        drop(hidden_member);
        drop(hidden_lower);
        drop(owner);
        drop(store);
        let reopened = MetaConsoleAggregator::new_with_options(
            Arc::new(SqliteConsoleLogStore::open(&path)?),
            options,
        );
        reopened
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".to_string(), entry);
        let restricted =
            reopened.policy_view(Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy));
        assert_retired_history_policy(&restricted, &identity, &raw.frames).await?;
        let member_only = reopened.policy_view(Arc::new(HideRuntimeMemberOnly("history-reviewer")));
        assert_retired_history_policy(&member_only, &identity, &raw.frames).await?;
        let legacy_frame = frame_from_session_history_message(
            "runtime-a",
            "legacy-retired",
            "legacy-session",
            0,
            json!({ "role": "user", "content": "Legacy readable history" }),
        )
        .expect("legacy frame");
        reopened.store().append_if_absent(legacy_frame).await?;
        let legacy = restricted
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("legacy-retired".to_string()),
                limit: 100,
                ..Default::default()
            })
            .await?;
        assert_eq!(
            legacy.frames.len(),
            1,
            "pre-provenance unknown historical identities remain readable"
        );
        let restored = reopened
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(identity),
                limit: 100,
                ..Default::default()
            })
            .await?;
        assert_eq!(
            restored.frames, projected_frames,
            "SQLite retains the witness while transport omits private inputs"
        );
        let _ = primary.mob_handle().stop().await;
        let _ = delegate.mob_handle().stop().await;
        Ok(())
    }

    async fn assert_retired_history_policy(
        view: &MetaConsoleAggregator,
        identity: &str,
        frames: &[ConsoleFrame],
    ) -> ConsoleLogResult<()> {
        for mode in [ConsoleTimelineMode::Recent, ConsoleTimelineMode::Since] {
            let page = view
                .query_timeline_windowed(ConsoleTimelineWindowQuery {
                    identity: Some(identity.to_string()),
                    mode,
                    limit: 100,
                    ..Default::default()
                })
                .await?;
            assert!(
                page.frames.is_empty(),
                "restricted recent/since snapshot cannot expose retired history: {page:?}"
            );
        }
        for frame in frames {
            assert!(
                !view
                    .timeline_event_visible_for_subscriber(
                        &ConsoleTimelineEvent::ConsoleFrame {
                            frame: frame.clone()
                        },
                        Some(identity)
                    )
                    .await,
                "identity-scoped SSE must apply retained source policy"
            );
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_visibility_policy_hides_live_alias_from_aggregator_controls()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-member-hidden-policy-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        // The REAL row is the durable identity's roster row now. The policy
        // below hides by ConsoleMember::agent_identity, which is the DECODED
        // roster member id, so its authoritative spelling for this row is the
        // durable identity itself - traced rather than assumed, because hiding
        // the wrong spelling would silently stop this test hiding anything.
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                // RAW public alias. spawn_trusted_identity_projected_member
                // applies mob_member_id itself, and mob_member_id_str is NOT
                // idempotent - pre-encoding here produced a double-encoded
                // roster key, which is why the single-encoded observation query
                // correctly returned None and the aggregator's own projection
                // then looked like a session disagreement.
                "review:singleton".to_string(),
                Some("You are the Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;
        // Asserted, not tolerated: if this ever returns None again the binding
        // below would fall back to a random session and manufacture the very
        // split-brain this test is not about.
        let hidden_live_session = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                "review:singleton",
            ))
            .await
            .ok_or("the hidden roster row must have an observable bridge session")?;
        let hidden_session_id = Some(hidden_live_session.to_string());
        let mut duplicate_labels = BTreeMap::new();
        duplicate_labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:1".to_string(),
                Some("You are a visible duplicate Review Agent.".into()),
                None,
                None,
            )
            .with_labels(duplicate_labels),
        )
        .await;

        // Truthful binding: the registered session IS the live row's session, so
        // the aggregator is exercising a healthy identity rather than a
        // split-brain it would rightly classify as stale.
        // Register the binding against whatever session the live row actually has.
        // When the row has none, fall back to the plain helper: a binding whose
        // session disagrees with its live row is a genuine split-brain and the
        // aggregator is right to classify it stale, so inventing one here would
        // manufacture the very condition this test is not about.
        let identity_runtime =
            identity_runtime_for_test_with_sessions(&[("review:singleton", hidden_live_session)])
                .await?;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime),
            runtime.console_events(),
            Arc::new(HideRuntimeMemberOnly("review:singleton")),
        );

        let inspection = aggregator.inspect_identity("review:singleton").await?;
        assert!(
            inspection.is_none(),
            "member-hidden live alias must not inspect through aggregator; it answered with \
             {inspection:#?}"
        );
        let records = aggregator.list_identities_fresh().await?;
        assert!(
            records
                .iter()
                .all(|record| record.identity != "review:singleton"),
            "member-hidden durable/live alias must not appear in identity list: {records:#?}"
        );
        let appended = aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "member-hidden-frame".to_string(),
                timestamp_ms: 1,
                runtime_key: "runtime-a".to_string(),
                identity: "review:singleton".to_string(),
                conversation_id: Some("review:singleton".to_string()),
                session_id: hidden_session_id,
                kind: "text_delta".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "delta": "hidden" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("member-hidden-frame".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await?;
        assert!(
            !aggregator
                .timeline_event_visible(&ConsoleTimelineEvent::ConsoleFrame {
                    frame: appended.frame.clone()
                })
                .await,
            "member-hidden live alias must not pass timeline event visibility"
        );
        let identity_only = aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "member-hidden-identity-only-frame".to_string(),
                timestamp_ms: 2,
                runtime_key: "runtime-a".to_string(),
                identity: "review:singleton".to_string(),
                conversation_id: Some("review:singleton".to_string()),
                session_id: None,
                kind: "text_delta".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "delta": "identity-only-hidden" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("member-hidden-identity-only-frame".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await?;
        assert!(
            !aggregator
                .timeline_event_visible(&ConsoleTimelineEvent::ConsoleFrame {
                    frame: identity_only.frame.clone()
                })
                .await,
            "member-hidden durable alias must not leak identity-only timeline frames"
        );
        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("review:singleton".to_string()),
                limit: 10,
                ..ConsoleTimelineQuery::default()
            })
            .await?;
        assert!(
            page.frames.is_empty(),
            "member-hidden live alias must not leak through explicit timeline query: {page:#?}"
        );
        assert!(
            !aggregator.retire_identity("review:singleton").await?,
            "member-hidden live alias must not retire through aggregator"
        );
        let reserve_err = aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "review:singleton".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "member-hidden-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await
            .expect_err("member-hidden durable alias must not reserve identity-first interaction");
        assert!(
            reserve_err.to_string().contains("unknown identity"),
            "unexpected reserve error: {reserve_err}"
        );
        let send_err = aggregator
            .send(ConsoleSendRequest {
                identity: "review:singleton".to_string(),
                content: json!("hello"),
                origin: "console:test".to_string(),
                idempotency_key: "member-hidden-send".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await
            .expect_err("member-hidden live alias must not receive sends");
        assert!(
            send_err.to_string().contains("unknown identity"),
            "unexpected send error: {send_err}"
        );
        let blob_result = aggregator
            .binary_blob_store_for_identity("review:singleton")
            .await;
        let Err(blob_err) = blob_result else {
            panic!("member-hidden live alias must not expose blob store");
        };
        assert!(
            blob_err.to_string().contains("unknown identity"),
            "unexpected blob error: {blob_err}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_session_backfill_target_resolves_durable_identity_label()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-label-backfill-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        let entry = RuntimeEntry {
            registration_id: uuid::Uuid::new_v4(),
            runtime_key: "runtime-a".to_string(),
            identity_namespace: String::new(),
            runtime: runtime.mob_runtime().clone(),
            identity_runtime: runtime
                .identity_runtime()
                .map(|identity_runtime| Arc::clone(identity_runtime.runtime())),
            identity_owner: None,
            memory_panel: None,
            console_events: runtime.console_events(),
            visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
        };
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry);

        let target = session_backfill_target_for_identity(&aggregator.inner, "review:singleton")
            .await
            .expect("durable label should resolve targeted session backfill");
        assert_eq!(target.record.identity, "review:singleton");
        assert_eq!(target.record.runtime_member_id, "rt:review:singleton:0");

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspect_identity_rejects_stale_durable_binding_when_live_alias_disagrees()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-stale-durable-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the live Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-stale-durable-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            1,
            SessionId::new(),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "runtime-a".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );

        let err = aggregator
            .inspect_identity("review:singleton")
            .await
            .expect_err("stale durable binding must not inspect as healthy");
        assert!(
            err.to_string().contains("stale durable identity alias"),
            "unexpected error: {err}"
        );
        let retire_err = aggregator
            .retire_identity("review:singleton")
            .await
            .expect_err("stale durable binding must not retire as healthy");
        assert!(
            retire_err
                .to_string()
                .contains("stale durable identity alias"),
            "unexpected retire error: {retire_err}"
        );
        let send_err = aggregator
            .send(ConsoleSendRequest {
                identity: "review:singleton".to_string(),
                content: json!("hello"),
                origin: "console:test".to_string(),
                idempotency_key: "stale-durable-send".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await
            .expect_err("stale durable binding must not send to the wrong live member");
        assert!(
            send_err
                .to_string()
                .contains("stale durable identity alias"),
            "unexpected send error: {send_err}"
        );
        let reserve_err = aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "review:singleton".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "stale-durable-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await
            .expect_err("identity-first reserve must not bypass stale durable binding");
        assert!(
            reserve_err
                .to_string()
                .contains("stale durable identity alias"),
            "unexpected reserve error: {reserve_err}"
        );
        let blob_err = match aggregator
            .binary_blob_store_for_identity("review:singleton")
            .await
        {
            Ok(_) => panic!("stale durable binding must not blob-resolve as healthy"),
            Err(err) => err,
        };
        assert!(
            blob_err
                .to_string()
                .contains("stale durable identity alias"),
            "unexpected blob error: {blob_err}"
        );
        let records = aggregator.list_identities_fresh().await?;
        assert!(
            records
                .iter()
                .any(|record| record.identity == "review:singleton"
                    && record.runtime_member_id == "rt:review:singleton:0"),
            "live alias should still be listed; records: {records:#?}"
        );
        assert!(
            records
                .iter()
                .any(|record| record.identity == "review:singleton"
                    && record.runtime_member_id == "rt:review:singleton:1"),
            "stale durable binding should remain visible as split-brain evidence; records: {records:#?}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// A console READ is a projection, never a repair.
    ///
    /// `inspect_identity` backs three `agent.view`-permissioned console
    /// methods. Healing a durable/live session split-brain from there put a
    /// VIEW grant one failed fence advance away from parking the identity
    /// `Broken` (Suspend -> `acquire_leases` -> `advance_existing_continuity_fence`).
    /// The read now reports the condition and leaves every field of identity
    /// authority untouched; the delivery write path still performs the repair,
    /// so the observable outcome for legitimate callers is preserved.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn console_read_reports_session_split_brain_without_mutating_identity_authority()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-session-read-projection-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the live Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;
        let live_session_id = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                "rt:review:singleton:0",
            ))
            .await
            .expect("live member has bridge session");

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-read-projection-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
        let stale_session_id = SessionId::new();
        assert_ne!(stale_session_id, live_session_id);
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            stale_session_id.clone(),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "runtime-a".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime.clone()),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );

        let before = identity_runtime.status(&identity).await?;

        let read_error = aggregator
            .inspect_identity("review:singleton")
            .await
            .err()
            .map(|err| err.to_string())
            .expect("a console read must report the split-brain, not heal it");
        assert!(
            read_error.contains("stale durable identity alias"),
            "{read_error}"
        );

        let after = identity_runtime.status(&identity).await?;
        assert_eq!(
            after.session_id,
            Some(stale_session_id),
            "a console read must not rebind durable session authority"
        );
        assert_eq!(
            after.state, before.state,
            "a console read must not move the identity lifecycle state"
        );
        assert_eq!(
            after.generation, before.generation,
            "a console read must not advance the continuity generation"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspect_identity_rejects_durable_binding_when_runtime_projects_wrong_identity()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-wrong-projection-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "other:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the mislabeled Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-wrong-projection-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            SessionId::new(),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "runtime-a".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );

        let err = aggregator
            .inspect_identity("review:singleton")
            .await
            .expect_err("wrong projected live identity must not inspect as healthy");
        assert!(
            err.to_string()
                .contains("projects identity other:singleton"),
            "unexpected error: {err}"
        );
        let retire_err = aggregator
            .retire_identity("review:singleton")
            .await
            .expect_err("wrong projected live identity must not retire as healthy");
        assert!(
            retire_err
                .to_string()
                .contains("projects identity other:singleton"),
            "unexpected retire error: {retire_err}"
        );
        let send_err = aggregator
            .send(ConsoleSendRequest {
                identity: "review:singleton".to_string(),
                content: json!("hello"),
                origin: "console:test".to_string(),
                idempotency_key: "wrong-projection-send".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await
            .expect_err("wrong projected live identity must not send as unknown");
        assert!(
            send_err
                .to_string()
                .contains("projects identity other:singleton"),
            "unexpected send error: {send_err}"
        );
        let reserve_err = aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "review:singleton".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "wrong-projection-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await
            .expect_err("identity-first reserve must not bypass wrong live projection");
        assert!(
            reserve_err
                .to_string()
                .contains("projects identity other:singleton"),
            "unexpected reserve error: {reserve_err}"
        );
        let blob_err = match aggregator
            .binary_blob_store_for_identity("review:singleton")
            .await
        {
            Ok(_) => panic!("wrong projected live identity must not blob-resolve as healthy"),
            Err(err) => err,
        };
        assert!(
            blob_err
                .to_string()
                .contains("projects identity other:singleton"),
            "unexpected blob error: {blob_err}"
        );
        let projected_err = aggregator
            .inspect_identity("other:singleton")
            .await
            .expect_err("wrong projected alias must not inspect through live-only fallback");
        assert!(
            projected_err
                .to_string()
                .contains("identity runtime binding for review:singleton"),
            "unexpected projected-alias error: {projected_err}"
        );
        let projected_retire_err = aggregator
            .retire_identity("other:singleton")
            .await
            .expect_err("wrong projected alias must not retire through live-only fallback");
        assert!(
            projected_retire_err
                .to_string()
                .contains("identity runtime binding for review:singleton"),
            "unexpected projected-alias retire error: {projected_retire_err}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspect_identity_rejects_hidden_wrong_projection_for_durable_binding()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-hidden-wrong-projection-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "other:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are a hidden wrong projection.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-hidden-wrong-projection-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            SessionId::new(),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "runtime-a".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(HideConsoleIdentity("other:singleton")),
                },
            );

        let err = aggregator
            .inspect_identity("review:singleton")
            .await
            .expect_err("hidden wrong projection must still fail durable binding validation");
        assert!(
            err.to_string()
                .contains("projects identity other:singleton"),
            "unexpected hidden wrong-projection error: {err}"
        );
        let reserve_err = aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "review:singleton".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "hidden-wrong-projection-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await
            .expect_err("reserve must not hide wrong projection from durable validation");
        assert!(
            reserve_err
                .to_string()
                .contains("projects identity other:singleton"),
            "unexpected reserve error: {reserve_err}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_id_inspect_skips_hidden_match_and_uses_visible_runtime()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let hidden_runtime = build_empty_runtime("runtime-id-hidden-match-test").await;
        let visible_runtime = build_empty_runtime("runtime-id-visible-match-test").await;
        let hidden_core = Arc::new(CoreReadsBridge::default());
        let visible_core = Arc::new(CoreReadsBridge::default());
        let hidden_identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "runtime-id-hidden-match-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(hidden_core.clone()),
                default_timeout: None,
            },
        ));
        let visible_identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "runtime-id-visible-match-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(visible_core.clone()),
                default_timeout: None,
            },
        ));
        for (identity_runtime, core_bridge) in [
            (&hidden_identity_runtime, &hidden_core),
            (&visible_identity_runtime, &visible_core),
        ] {
            let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
            register_live(
                identity_runtime,
                &core_bridge.core,
                crate::identity_first::DurableAgentSpec {
                    identity: identity.clone(),
                    profile: meerkat_mob::ProfileName::from("worker"),
                    addressability: crate::identity_first::AgentAddressability::Addressable,
                    display_name: None,
                    labels: BTreeMap::new(),
                    context: None,
                    additional_instructions: Vec::new(),
                    initial_message: None,
                    runtime_mode_override: None,
                    backend: None,
                    binding: None,
                    placement: None,
                },
                0,
                SessionId::new(),
            )
            .await?;
        }

        let aggregator = MetaConsoleAggregator::in_memory();
        {
            let mut runtimes = aggregator.inner.runtimes.write().expect("runtime registry");
            runtimes.insert(
                "a-hidden".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "a-hidden".to_string(),
                    identity_namespace: String::new(),
                    runtime: hidden_runtime.mob_runtime().clone(),
                    identity_runtime: Some(hidden_identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: hidden_runtime.console_events(),
                    visibility_policy: Arc::new(HideRuntimeMemberIdentity("rt:review:singleton:0")),
                },
            );
            runtimes.insert(
                "b-visible".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "b-visible".to_string(),
                    identity_namespace: String::new(),
                    runtime: visible_runtime.mob_runtime().clone(),
                    identity_runtime: Some(visible_identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: visible_runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );
        }

        let inspection = aggregator
            .inspect_identity("rt:review:singleton:0")
            .await?
            .expect("visible runtime-id match should be returned");
        assert_eq!(inspection.identity.runtime_key, "b-visible");

        let _ = hidden_runtime.mob_handle().stop().await;
        let _ = visible_runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspect_identity_rejects_cross_source_wrong_projection_for_runtime_member()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-cross-source-primary-test").await;
        let mut primary_labels = BTreeMap::new();
        primary_labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the primary Review Agent.".into()),
                None,
                None,
            )
            .with_labels(primary_labels),
        )
        .await;

        let delegate_runtime = build_empty_runtime("identity-cross-source-delegate-test").await;
        let mut delegate_labels = BTreeMap::new();
        delegate_labels.insert("agent_identity".to_string(), "other:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &delegate_runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the wrong projected Review Agent.".into()),
                None,
                None,
            )
            .with_labels(delegate_labels),
        )
        .await;
        runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("runtime should expose mob MCP state")
            .mob_insert_handle(
                meerkat_mob::ids::MobId::from("identity-cross-source-delegate-test"),
                delegate_runtime.mob_handle().clone(),
            )
            .await;

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-cross-source-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            SessionId::new(),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "runtime-a".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );

        let err = aggregator
            .inspect_identity("review:singleton")
            .await
            .expect_err("cross-source wrong projection must not be hidden by primary source");
        assert!(
            err.to_string()
                .contains("projects identity other:singleton")
                || err.to_string().contains("ambiguous live identity alias"),
            "unexpected error: {err}"
        );

        let _ = delegate_runtime.mob_handle().stop().await;
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn durable_send_selects_bound_generation_when_stale_generation_is_still_active()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-respawn-send-authority-test").await;
        let labels = BTreeMap::from([("agent_identity".to_string(), "builder".to_string())]);
        for generation in 0..=1 {
            spawn_trusted_identity_projected_member(
                &runtime,
                SpawnMemberSpec::from_wire(
                    "worker".to_string(),
                    format!("rt:builder:{generation}"),
                    Some(format!("You are Builder generation {generation}.").into()),
                    None,
                    None,
                )
                .with_labels(labels.clone()),
            )
            .await;
        }
        let bound_session_id = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                "rt:builder:1",
            ))
            .await
            .expect("bound generation has a bridge session");

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-respawn-send-authority-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("builder")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            1,
            bound_session_id.clone(),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "default".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "default".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );

        let matches = aggregator.resolve_visible_members("builder").await;
        assert_eq!(
            matches.len(),
            2,
            "both Active generations remain observable"
        );
        assert_eq!(
            sendable_resolved_member_indices(&matches).len(),
            2,
            "the stale generation reproduces the pre-fix send ambiguity"
        );
        let resolved = aggregator
            .resolve_send_member("builder")
            .await?
            .expect("durable builder alias resolves");
        assert_eq!(
            resolved.runtime_identity, "rt:builder:1",
            "continuity binding must shadow the stale Active generation for sends"
        );

        let inspect_error = aggregator
            .inspect_identity("builder")
            .await
            .expect_err("inspection must keep duplicate live aliases explicit");
        assert!(
            inspect_error
                .to_string()
                .contains("ambiguous live identity alias")
        );
        let retire_error = aggregator
            .retire_identity("builder")
            .await
            .expect_err("control must not silently retire through a durable alias");
        assert!(
            retire_error
                .to_string()
                .contains("ambiguous live identity alias")
        );

        let accepted = aggregator
            .send(ConsoleSendRequest {
                identity: "builder".to_string(),
                content: json!("continue after respawn"),
                origin: "console:test".to_string(),
                idempotency_key: "respawn-bound-generation-send".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            })
            .await?;
        assert_eq!(accepted.session_id, Some(bound_session_id.to_string()));

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspect_identity_rejects_duplicate_live_alias_even_when_durable_matches_one()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-ambiguous-durable-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the first Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels.clone()),
        )
        .await;
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:1".to_string(),
                Some("You are the second Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        // Core's binding names the first duplicate's own session, so the
        // durable binding matches exactly one of the two live aliases.
        let bound_session_id = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                "rt:review:singleton:0",
            ))
            .await
            .expect("the first duplicate has a bridge session");

        let core_bridge = Arc::new(CoreReadsBridge::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "console-aggregator-ambiguous-durable-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(core_bridge.clone()),
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("review:singleton")?;
        register_live(
            &identity_runtime,
            &core_bridge.core,
            crate::identity_first::DurableAgentSpec {
                identity: identity.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            },
            0,
            bound_session_id,
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                RuntimeEntry {
                    registration_id: uuid::Uuid::new_v4(),
                    runtime_key: "runtime-a".to_string(),
                    identity_namespace: String::new(),
                    runtime: runtime.mob_runtime().clone(),
                    identity_runtime: Some(identity_runtime),
                    identity_owner: None,
                    memory_panel: None,
                    console_events: runtime.console_events(),
                    visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
                },
            );

        let err = aggregator
            .inspect_identity("review:singleton")
            .await
            .expect_err("duplicate live alias must not inspect via durable fast path");
        assert!(
            err.to_string().contains("ambiguous live identity alias"),
            "unexpected error: {err}"
        );
        let retire_err = aggregator
            .retire_identity("review:singleton")
            .await
            .expect_err("duplicate live alias must not retire via durable fast path");
        assert!(
            retire_err
                .to_string()
                .contains("ambiguous live identity alias"),
            "unexpected retire error: {retire_err}"
        );
        let runtime_id_err = aggregator
            .inspect_identity("rt:review:singleton:0")
            .await
            .expect_err("runtime-id lookup must still reject duplicate durable live aliases");
        assert!(
            runtime_id_err
                .to_string()
                .contains("ambiguous live identity alias"),
            "unexpected runtime-id inspect error: {runtime_id_err}"
        );
        let runtime_id_retire_err = aggregator
            .retire_identity("rt:review:singleton:0")
            .await
            .expect_err("runtime-id retire must still reject duplicate durable live aliases");
        assert!(
            runtime_id_retire_err
                .to_string()
                .contains("ambiguous live identity alias")
                || runtime_id_retire_err
                    .to_string()
                    .contains("stale durable identity alias"),
            "unexpected runtime-id retire error: {runtime_id_retire_err}"
        );

        aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "review:singleton".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "duplicate-live-alias-durable-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await
            .expect("durable identity binding remains authoritative for interaction reserve");

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspect_identity_ignores_hidden_live_alias_candidates()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-hidden-ambiguous-live-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "review:singleton".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:0".to_string(),
                Some("You are the visible Review Agent.".into()),
                None,
                None,
            )
            .with_labels(labels.clone()),
        )
        .await;
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "rt:review:singleton:1".to_string(),
                Some("You are hidden maintenance noise.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(HideRuntimeMemberIdentity("rt:review:singleton:1")),
        );

        let inspection = aggregator
            .inspect_identity("review:singleton")
            .await?
            .expect("visible live alias should inspect without hidden ambiguity");
        assert_eq!(inspection.identity.identity, "review:singleton");
        assert_eq!(
            inspection.identity.runtime_member_id,
            "rt:review:singleton:0"
        );

        let retired = aggregator.retire_identity("review:singleton").await?;
        assert!(
            retired,
            "visible live alias should retire without hidden ambiguity"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identity_first_reserve_skips_hidden_runtime_and_uses_visible_match()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let hidden_runtime = build_empty_runtime("identity-reserve-hidden-first-test").await;
        let hidden_identity_runtime = identity_runtime_for_test(&["review:singleton"]).await?;
        let visible_runtime = build_empty_runtime("identity-reserve-visible-second-test").await;
        let visible_identity_runtime = identity_runtime_for_test(&["review:singleton"]).await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "a-hidden",
            "",
            hidden_runtime.mob_runtime().clone(),
            Some(hidden_identity_runtime),
            hidden_runtime.console_events(),
            Arc::new(HideRuntimeMemberIdentity("rt:review:singleton:0")),
        );
        aggregator.register_runtime_handles_with_policy(
            "b-visible",
            "",
            visible_runtime.mob_runtime().clone(),
            Some(visible_identity_runtime),
            visible_runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        let accepted = aggregator
            .reserve_identity_first_interaction(
                ConsoleSendRequest {
                    identity: "review:singleton".to_string(),
                    content: json!("hello"),
                    origin: "console:test".to_string(),
                    idempotency_key: "visible-after-hidden-reserve".to_string(),
                    handling_mode: Some("queue".to_string()),
                    origin_kind: None,
                    skill_refs: Vec::new(),
                },
                None,
            )
            .await?
            .into_accepted();
        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("review:singleton".to_string()),
                ..ConsoleTimelineQuery::default()
            })
            .await?;
        let frame = page
            .frames
            .iter()
            .find(|frame| frame.id == accepted.input_frame_id)
            .ok_or("accepted frame missing from timeline")?;
        assert_eq!(frame.runtime_key, "b-visible");

        let _ = hidden_runtime.mob_handle().stop().await;
        let _ = visible_runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identity_first_list_identities_projects_cached_topology_peers()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-topology-cache-test").await;
        let identity_runtime = identity_runtime_for_test(&["agent:alpha", "agent:beta"]).await?;
        identity_runtime
            .set_desired_peer_edges(vec![crate::identity_first::ManagedPeerEdge::new(
                crate::identity_first::AgentIdentity::parse("agent:alpha")?,
                crate::identity_first::AgentIdentity::parse("agent:beta")?,
            )?])
            .await;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "identity-first",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        let identities = aggregator.list_identities().await?;
        let alpha = identities
            .iter()
            .find(|record| record.identity == "agent:alpha")
            .ok_or("agent:alpha identity missing")?;
        assert_eq!(alpha.topology_peers, vec!["agent:beta".to_string()]);
        let inspection = aggregator
            .inspect_identity("agent:alpha")
            .await?
            .ok_or("agent:alpha inspection missing")?;
        assert_eq!(inspection.peers, vec!["agent:beta".to_string()]);

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identity_first_list_identities_includes_member_only_spawned_workers()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-member-only-test").await;
        let identity_runtime = identity_runtime_for_test(&["agent:alpha"]).await?;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "identity-first",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "agent:beta".to_string(),
                Some("You are beta.".into()),
                None,
                None,
            ))
            .await?;

        let identities = aggregator.list_identities().await?;
        assert!(
            identities
                .iter()
                .any(|record| record.identity == "agent:beta"),
            "member-only spawned workers should remain visible in identity-first listings: {identities:#?}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn mixed_repository_rosters_keep_names_and_resolvable_peer_endpoints()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let aggregator = MetaConsoleAggregator::in_memory();
        let mut runtimes = Vec::new();
        for namespace in ["repo-a", "repo-b"] {
            let runtime = build_empty_runtime(&format!("mixed-topology-{namespace}")).await;
            let authority = Arc::new(crate::identity_first::IdentityRuntime::new(
                crate::identity_first::IdentityRuntimeConfig {
                    continuity_store: Arc::new(
                        crate::identity_first::LocalContinuityStore::in_memory()?,
                    ),
                    lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                    runtime_instance_id: namespace.to_owned(),
                    has_runtime_store: true,
                    durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                    bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                        runtime.mob_handle(),
                    ))),
                    default_timeout: None,
                },
            ));
            let specialist = crate::identity_first::AgentIdentity::parse("domain:media")?;
            authority
                .register(crate::identity_first::DurableAgentSpec {
                    identity: specialist.clone(),
                    profile: meerkat_mob::ProfileName::from("worker"),
                    addressability: crate::identity_first::AgentAddressability::Addressable,
                    display_name: Some(crate::identity_first::DisplayName::parse(
                        "Media specialist",
                    )?),
                    labels: BTreeMap::new(),
                    context: None,
                    additional_instructions: Vec::new(),
                    initial_message: None,
                    runtime_mode_override: None,
                    backend: None,
                    binding: None,
                    placement: None,
                })
                .await;
            authority.materialize(&specialist).await?;
            runtime
                .mob_handle()
                .ensure_member(SpawnMemberSpec::new("worker", "coder"))
                .await?;
            let specialist_member = crate::member_comms_id::mob_member_id(specialist.as_str());
            runtime
                .mob_handle()
                .wire(
                    specialist_member.clone(),
                    meerkat_mob::AgentIdentity::from("coder"),
                )
                .await?;
            runtime
                .mob_handle()
                .wire(meerkat_mob::AgentIdentity::from("coder"), specialist_member)
                .await?;
            aggregator.register_runtime_handles_with_policy(
                namespace,
                namespace,
                runtime.mob_runtime().clone(),
                Some(authority),
                runtime.console_events(),
                Arc::new(AllowAllConsoleVisibilityPolicy),
            );
            runtimes.push(runtime);
        }
        // The same raw coder ID exists in both mobs. Connect the actual
        // endpoint so the projection must use the remote mob's namespace.
        let coder_id = meerkat_mob::AgentIdentity::from("coder");
        let remote = runtimes[1]
            .mob_handle()
            .member_peer_endpoint(&coder_id)
            .await?
            .ok_or("remote coder endpoint missing")?;
        runtimes[0]
            .mob_handle()
            .wire(
                coder_id,
                meerkat_mob::PeerTarget::External(remote.descriptor),
            )
            .await?;
        let records = aggregator.list_identities_fresh().await?;
        for runtime in runtimes {
            let _ = runtime.mob_handle().stop().await;
        }
        for namespace in ["repo-a", "repo-b"] {
            let specialist = records
                .iter()
                .find(|record| record.identity == format!("{namespace}/domain:media"))
                .ok_or("missing specialist")?;
            let coder = records
                .iter()
                .find(|record| record.identity == format!("{namespace}/coder"))
                .ok_or("missing coder")?;
            assert_eq!(specialist.display_name, "Media specialist");
            assert_eq!(specialist.topology_peers, vec![coder.identity.clone()]);
            let expected: BTreeSet<_> = if namespace == "repo-a" {
                [specialist.identity.clone(), "repo-b/coder".to_owned()]
                    .into_iter()
                    .collect()
            } else {
                [specialist.identity.clone()].into_iter().collect()
            };
            assert_eq!(
                coder
                    .topology_peers
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                expected
            );
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identity_first_list_identities_lists_a_live_identity_once()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-live-once-test").await;
        let mut labels = BTreeMap::new();
        labels.insert("agent_identity".to_string(), "agent:alpha".to_string());
        spawn_trusted_identity_projected_member(
            &runtime,
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "agent:alpha".to_string(),
                Some("You are alpha.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await;
        let live_session = runtime
            .mob_handle()
            .resolve_bridge_session_id_observation(&crate::member_comms_id::mob_member_id(
                "agent:alpha",
            ))
            .await
            .ok_or("the live roster row must have an observable bridge session")?;
        // A healthy binding: the durable row names `rt:agent:alpha:0` on the
        // live row's own session.
        let identity_runtime =
            identity_runtime_for_test_with_sessions(&[("agent:alpha", live_session.clone())])
                .await?;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "identity-first",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        let identities = aggregator.list_identities().await?;
        let alpha = identities
            .iter()
            .filter(|record| record.identity == "agent:alpha")
            .collect::<Vec<_>>();
        assert_eq!(
            alpha.len(),
            1,
            "a live identity lists once, not as its durable binding and its roster row: {identities:#?}"
        );
        assert_eq!(alpha[0].runtime_member_id, "rt:agent:alpha:0");
        assert_eq!(
            alpha[0].session_id.as_deref(),
            Some(live_session.to_string().as_str())
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// The same once-only listing over Core's real member reads: the
    /// identity runtime sits on the mob's own bridge, so the one row carries
    /// Core's runtime alias and session for the identity, and after a reset
    /// the one row moves to the successor's alias and session while the old
    /// alias is listed nowhere.
    #[tokio::test]
    async fn identity_first_list_identities_lists_a_live_identity_once_over_core()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let runtime = build_empty_runtime("identity-live-once-core").await;
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "identity-live-once-core".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(crate::identity_first::MobSessionBridge::new(
                    runtime.mob_handle(),
                ))),
                default_timeout: None,
            },
        ));
        let alpha = crate::identity_first::AgentIdentity::parse("agent:alpha")?;
        identity_runtime
            .register(crate::identity_first::DurableAgentSpec {
                identity: alpha.clone(),
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        identity_runtime
            .materialize(&alpha)
            .await?
            .ok_or("Core holds agent:alpha live after the wake")?;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "identity-first",
            "",
            runtime.mob_runtime().clone(),
            Some(identity_runtime.clone()),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        // Core's runtime alias and session for the identity, as its status
        // reads them.
        let core_binding = |status: crate::identity_first::IdentityStatus| {
            status
                .agent_runtime_id
                .map(|alias| alias.to_string())
                .zip(status.session_id.map(|session| session.to_string()))
                .ok_or("Core holds agent:alpha live on a session")
        };
        let (alias, session) = core_binding(identity_runtime.status(&alpha).await?)?;
        assert!(alias.starts_with("rt:agent:alpha:"), "{alias}");
        let identities = aggregator.list_identities_fresh().await?;
        let rows = identities
            .iter()
            .filter(|record| record.identity == "agent:alpha")
            .collect::<Vec<_>>();
        assert_eq!(
            rows.len(),
            1,
            "a live identity lists once over Core's reads: {identities:#?}"
        );
        assert_eq!(rows[0].runtime_member_id, alias);
        assert_eq!(rows[0].session_id.as_deref(), Some(session.as_str()));

        let reset = identity_runtime.reset(&alpha).await?;
        let (successor_alias, successor_session) =
            core_binding(identity_runtime.status(&alpha).await?)?;
        assert_eq!(
            reset.session_id().map(ToString::to_string).as_deref(),
            Some(successor_session.as_str()),
            "the reset's binding is the one the status reads"
        );
        assert_ne!(successor_alias, alias, "a reset moves Core's generation");
        assert_ne!(successor_session, session, "a reset binds a fresh session");
        let identities = aggregator.list_identities_fresh().await?;
        let rows = identities
            .iter()
            .filter(|record| record.identity == "agent:alpha")
            .collect::<Vec<_>>();
        assert_eq!(
            rows.len(),
            1,
            "the reset identity still lists once: {identities:#?}"
        );
        assert_eq!(rows[0].runtime_member_id, successor_alias);
        assert_eq!(
            rows[0].session_id.as_deref(),
            Some(successor_session.as_str())
        );
        assert!(
            identities
                .iter()
                .all(|record| record.runtime_member_id != alias),
            "the alias the reset retired is listed nowhere: {identities:#?}"
        );

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test]
    async fn list_identities_serves_hot_cache_while_identity_refresh_is_in_flight() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let record = identity_record_for_test("agent-cached");
        *aggregator.inner.identity_read_model.inner.write().await = vec![record.clone()];
        aggregator
            .inner
            .identity_read_model
            .primed
            .store(true, Ordering::Release);
        let _guard = aggregator
            .inner
            .identity_read_model
            .refresh_lock
            .clone()
            .lock_owned()
            .await;

        let identities =
            tokio::time::timeout(Duration::from_millis(50), aggregator.list_identities())
                .await
                .expect("hot identity list should not wait for refresh lock")
                .expect("identity list succeeds");

        assert_eq!(identities, vec![record]);
    }

    #[tokio::test]
    async fn list_identities_waits_for_inflight_identity_refresh_on_cold_cache() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let guard = aggregator
            .inner
            .identity_read_model
            .refresh_lock
            .clone()
            .lock_owned()
            .await;
        let waiter = tokio::spawn({
            let aggregator = aggregator.clone();
            async move { aggregator.list_identities().await }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "cold identity list should wait for the in-flight refresh to finish"
        );

        let record = identity_record_for_test("agent-primed");
        *aggregator.inner.identity_read_model.inner.write().await = vec![record.clone()];
        aggregator
            .inner
            .identity_read_model
            .primed
            .store(true, Ordering::Release);
        drop(guard);

        let identities = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("cold identity list waiter should resume")
            .expect("waiter joins")
            .expect("identity list succeeds");
        assert_eq!(identities, vec![record]);
    }

    #[tokio::test]
    async fn legacy_empty_timeline_pages_finish_since_and_recent_scans() {
        let mut store = CountingConsoleLogStore::new();
        store.legacy_empty_page = true;
        let aggregator = MetaConsoleAggregator::new(Arc::new(store));
        for mode in [ConsoleTimelineMode::Since, ConsoleTimelineMode::Recent] {
            let page = aggregator
                .query_timeline_windowed(ConsoleTimelineWindowQuery {
                    mode,
                    ..Default::default()
                })
                .await
                .expect("empty legacy frontier is valid");
            assert!(page.frames.is_empty());
            assert!(page.exhausted);
        }
        let frame =
            counterpart_lineage_frame("short-page", "text_delta", json!({"delta":"visible"}));
        aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("fixture");
        let recent = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                mode: ConsoleTimelineMode::Recent,
                limit: 10,
                ..Default::default()
            })
            .await
            .expect("short page followed by empty legacy page");
        assert_eq!(recent.frames.len(), 1);
        assert!(recent.exhausted);
    }

    #[test]
    fn projection_caches_evict_without_retaining_duplicate_order_entries() {
        let mut cache = BoundedProjectionCache::new(2);
        cache.insert("first", 1);
        cache.insert("first", 2);
        cache.insert("second", 3);
        cache.insert("third", 4);
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.order.len(), 2);
        assert!(!cache.entries.contains_key("first"));
        cache.retain(|key, _| *key != "second");
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.order.len(), 1);
    }

    #[tokio::test]
    async fn query_timeline_reads_from_aggregate_store() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let frame = NewConsoleFrame {
            id: None,
            dedupe_key: "event-1".to_string(),
            timestamp_ms: 1,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: None,
            kind: "text_delta".to_string(),
            status: ConsoleFrameStatus::Delivered,
            payload: json!({ "delta": "hello" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: Some("event-1".to_string()),
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };
        aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("append frame");

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("agent-a".to_string()),
                limit: 10,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query timeline");
        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].kind, "text_delta");
    }

    /// §9.3: memory.* events with no affected identity are attributed to
    /// `_system` by the ConsoleMemoryEventSink. `_system` has no roster
    /// record, so without an explicit exemption the per-identity visibility
    /// gate hides them from every GLOBAL timeline query (the pre-fix bug:
    /// the rail and the Memory panel live strip never saw dream/quarantine/
    /// promotion events). The exemption must be exact — an arbitrary
    /// unknown identity stays hidden — and must survive identity
    /// namespacing (`frame_from_console_event` must not rewrite `_system`
    /// to `ns/_system`). ABAC is unaffected: under an enforced config the
    /// RPC layer still filters `_system` frames per caller via
    /// `timeline_frame_visible_to` (covered in tests/access_control.rs).
    #[tokio::test]
    async fn host_redaction_precedes_store_broadcast_and_view_send_reservation() {
        struct RedactSecret;
        impl ConsoleVisibilityPolicy for RedactSecret {
            fn redact_payload(&self, frame: &NewConsoleFrame) -> Option<Value> {
                matches!(frame.kind.as_str(), "text_delta" | "user_input")
                    .then(|| json!({"content":"[host redacted]"}))
            }
        }
        let runtime = build_empty_runtime("host-redaction-owner").await;
        let owner = MetaConsoleAggregator::in_memory();
        let mut entry = runtime_entry_for_test("default", &runtime);
        entry.identity_namespace.clear();
        entry.visibility_policy = Arc::new(RedactSecret);
        entry.identity_runtime = Some(
            identity_runtime_for_test(&["agent-a"])
                .await
                .expect("durable identity"),
        );
        owner
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("default".into(), entry);
        let view = owner.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
        let mut receiver = owner.subscribe();
        let mut frame = counterpart_lineage_frame(
            "host-secret",
            "text_delta",
            json!({"delta":"private-token"}),
        );
        frame.runtime_key = "default".into();
        frame.identity = SYSTEM_EVENT_IDENTITY.into();
        let policy = capture_host_payload_policy(&view.inner, &frame.runtime_key, None)
            .expect("host payload policy");
        let stored = append_and_emit_runtime_plane(&view.inner, frame, policy)
            .await
            .expect("append")
            .frame;
        assert_eq!(stored.payload["content"], "[host redacted]");
        assert_eq!(stored.status, ConsoleFrameStatus::Redacted);
        let ConsoleTimelineEvent::ConsoleFrame { frame } =
            receiver.recv().await.expect("broadcast")
        else {
            panic!("frame event");
        };
        assert_eq!(frame.payload, stored.payload);
        let raw = owner
            .store()
            .frame_by_dedupe_key("host-secret")
            .await
            .expect("read")
            .expect("stored");
        assert_eq!(raw.payload, stored.payload);
        let request = ConsoleSendRequest {
            identity: "agent-a".into(),
            content: json!("private-token"),
            origin: "console:host-policy-test".into(),
            idempotency_key: "send-secret".into(),
            handling_mode: Some("queue".into()),
            origin_kind: None,
            skill_refs: Vec::new(),
        };
        view.reserve_identity_first_interaction(request.clone(), None)
            .await
            .expect("reservation");
        let key = send_dedupe_key(
            "default",
            &request.identity,
            &request.origin,
            &request.idempotency_key,
        );
        let send = owner
            .store()
            .frame_by_dedupe_key(&key)
            .await
            .expect("raw send")
            .expect("reserved");
        assert_eq!(
            send.payload, stored.payload,
            "view send must use host policy before persistence"
        );
        let ConsoleTimelineEvent::ConsoleFrame { frame } =
            receiver.recv().await.expect("send broadcast")
        else {
            panic!("send frame event");
        };
        assert_eq!(frame.payload, send.payload);
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn captured_host_policy_survives_unregister_and_replacement_before_append() {
        struct RedactHost;
        impl ConsoleVisibilityPolicy for RedactHost {
            fn redact_payload(&self, _: &NewConsoleFrame) -> Option<Value> {
                Some(json!({"content":"[host redacted]"}))
            }
        }
        let runtime = build_empty_runtime("captured-host-policy").await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let mut entry = runtime_entry_for_test("runtime-a", &runtime);
        entry.visibility_policy = Arc::new(RedactHost);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), entry.clone());
        let policy = capture_host_payload_policy(
            &aggregator.inner,
            "runtime-a",
            Some(entry.registration_id),
        )
        .expect("admit producer");
        let mut receiver = aggregator.subscribe();
        aggregator.unregister_runtime("runtime-a");
        let mut frame = counterpart_lineage_frame(
            "unregistered-source",
            "text_delta",
            json!({"delta":"private-token"}),
        );
        frame.identity = SYSTEM_EVENT_IDENTITY.into();
        assert!(
            append_and_emit(&aggregator.inner, frame.clone())
                .await
                .is_err(),
            "missing producer cannot silently use AllowAll"
        );
        let first = append_and_emit_runtime_plane(&aggregator.inner, frame.clone(), policy.clone())
            .await
            .expect("already admitted source")
            .frame;
        let mut replacement = entry;
        replacement.registration_id = uuid::Uuid::new_v4();
        replacement.visibility_policy = Arc::new(AllowAllConsoleVisibilityPolicy);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), replacement);
        frame.dedupe_key = "replaced-source".into();
        let second = append_and_emit_runtime_plane(&aggregator.inner, frame, policy)
            .await
            .expect("captured policy")
            .frame;
        for stored in [first, second] {
            assert_eq!(stored.payload, json!({"content":"[host redacted]"}));
            let ConsoleTimelineEvent::ConsoleFrame { frame } =
                receiver.recv().await.expect("broadcast")
            else {
                panic!("frame");
            };
            assert_eq!(frame.payload, stored.payload);
            assert_eq!(
                aggregator
                    .store()
                    .frame_by_dedupe_key(&stored.dedupe_key)
                    .await
                    .expect("raw store")
                    .expect("row")
                    .payload,
                stored.payload
            );
        }
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn stale_registration_cannot_replace_current_provenance_cache() {
        let runtime = build_single_member_runtime().await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), entry.clone());
        let resolved = member_sources_for_entry_including_hidden(&entry)
            .await
            .into_iter()
            .next()
            .expect("member");
        let mut old_record = identity_record_for_resolved_member(&resolved)
            .await
            .expect("identity");
        old_record.labels.insert("generation".into(), "old".into());
        retain_member_provenance(&aggregator.inner, &resolved, &old_record);
        let mut replacement = entry;
        replacement.registration_id = uuid::Uuid::new_v4();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), replacement.clone());
        let replacement_resolved = member_sources_for_entry_including_hidden(&replacement)
            .await
            .into_iter()
            .next()
            .expect("replacement member");
        let mut new_record = old_record.clone();
        new_record.labels.insert("generation".into(), "new".into());
        let current =
            retain_member_provenance(&aggregator.inner, &replacement_resolved, &new_record);
        retain_member_provenance(&aggregator.inner, &resolved, &old_record);
        let cached = member_provenance_for_identity(
            &aggregator.inner,
            &replacement,
            &new_record.identity,
            new_record.session_id.as_deref(),
            false,
        )
        .await;
        assert_eq!(
            cached,
            Some(current),
            "stale refresh cannot overwrite the replacement's witness"
        );
        runtime.shutdown().await;
    }

    /// R-d: a console event is ingested data. Provenance-shaped fields in it
    /// never become the stored frame's member provenance; the owner's own
    /// capture is the only source, and no live member is captured here.
    #[tokio::test]
    async fn an_ingested_console_event_cannot_supply_member_provenance() {
        let runtime = build_empty_runtime("ingested-provenance").await;
        let owner = MetaConsoleAggregator::in_memory();
        let mut entry = runtime_entry_for_test("default", &runtime);
        entry.identity_namespace.clear();
        owner
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("default".into(), entry);
        let claimed = json!({
            "identity": {"identity": "claimed-member", "display_name": "claimed",
                "runtime_key": "default", "runtime_member_id": "claimed-member",
                "session_id": "claimed-session", "visibility": "addressable",
                "addressable": true, "health": "ready", "labels": {"tier": "public"}},
            "member": {"agent_identity": "claimed-member", "role": "worker",
                "state": "running", "wired_to": [], "labels": {"tier": "public"}},
            "primary_mob_id": "claimed", "source_mob_id": "claimed"
        });
        project_console_event(
            owner.inner.clone(),
            "default",
            crate::console_contracts::ConsoleIdentityEventEnvelope {
                event_id: "ingested-provenance".into(),
                interaction_id: None,
                identity: "claimed-member".into(),
                event_type: "text_delta".into(),
                timestamp_ms: 1000,
                data: json!({
                    "delta": "hello",
                    "session_id": "claimed-session",
                    "member_provenance": claimed.clone(),
                    "source": {"kind": "synthetic", "member_provenance": claimed},
                }),
            },
        )
        .await
        .expect("ingested");
        let stored = owner
            .store()
            .query_frames(ConsoleTimelineQuery::default())
            .await
            .expect("read")
            .frames;
        let frame = stored
            .iter()
            .find(|frame| frame.source_event_id.as_deref() == Some("ingested-provenance"))
            .expect("stored frame");
        assert_eq!(frame.source.kind, ConsoleFrameSourceKind::ConsoleEvent);
        assert_eq!(frame.source.member_provenance, None);
        runtime.shutdown().await;
    }

    /// Embedder path: a member of a namespaced registered runtime. The owner
    /// captures its attributes under the registration's namespace, and the
    /// captured attributes decide the stored frame for a denied caller whose
    /// shared cache knows nothing of the member.
    #[tokio::test]
    async fn a_namespaced_runtime_member_frame_is_decided_by_the_owner_capture() {
        let runtime = build_single_member_runtime().await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), entry.clone());
        let resolved = member_sources_for_entry_including_hidden(&entry)
            .await
            .into_iter()
            .next()
            .expect("member");
        let mut record = identity_record_for_resolved_member(&resolved)
            .await
            .expect("identity");
        assert!(record.identity.starts_with("test/"), "{record:#?}");
        record.labels.insert("tier".into(), "secret".into());
        let provenance = capture_member_provenance(&aggregator.inner, &resolved, &record).await;
        assert_eq!(provenance.lineage[0].identity, record.identity);
        assert_eq!(
            provenance.lineage[0].agent_id.as_deref(),
            Some(apply_namespace(&record.runtime_member_id, "test").as_str())
        );
        let mut frame = counterpart_lineage_frame("namespaced-secret", "text_delta", json!({}));
        frame.identity = record.identity.clone();
        frame.conversation_id = Some(record.identity.clone());
        frame.session_id = record.session_id.clone();
        frame.source.member_provenance = Some(provenance);
        let stored = aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("frame")
            .frame;
        let controller = crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                crate::access::AccessRule {
                    id: "everyone-views".to_string(),
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["*".to_string()],
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "hide-secret".to_string(),
                    effect: crate::access::AccessEffect::Deny,
                    actions: vec!["agent.view".to_string()],
                    match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
                    ..crate::access::AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("controller");
        let caller = controller.view_for_subject(None);
        assert!(!caller.knows_agent(&record.identity));
        assert!(!timeline_frame_visible_to(&caller, &stored));
        assert!(timeline_frame_visible_to(
            &controller.view_for_subject(Some("root@example.test")),
            &stored
        ));
        runtime.shutdown().await;
    }

    /// R-e: a frame that names no session never takes a witness retained
    /// for an earlier incarnation under the same identity; it records none.
    #[tokio::test]
    async fn a_session_less_frame_never_takes_an_earlier_incarnation_s_witness() {
        let runtime = build_single_member_runtime().await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), entry.clone());
        let resolved = member_sources_for_entry_including_hidden(&entry)
            .await
            .into_iter()
            .next()
            .expect("member");
        let mut earlier = identity_record_for_resolved_member(&resolved)
            .await
            .expect("identity");
        earlier.session_id = Some("earlier-incarnation".into());
        earlier.labels.insert("tier".into(), "public".into());
        let witness = retain_member_provenance(&aggregator.inner, &resolved, &earlier);
        // The identity-only key an earlier owner kept for the same member.
        aggregator
            .inner
            .member_provenance
            .lock()
            .expect("cache")
            .insert(
                ("runtime-a".into(), earlier.identity.clone(), None),
                Arc::new(witness),
            );
        let captured = member_provenance_for_identity(
            &aggregator.inner,
            &entry,
            &earlier.identity,
            None,
            false,
        )
        .await;
        assert!(
            captured.as_ref().is_none_or(|provenance| {
                provenance.identity.session_id.as_deref() != Some("earlier-incarnation")
                    && provenance.identity.labels.get("tier").map(String::as_str) != Some("public")
            }),
            "a session-less frame took an earlier incarnation's witness: {captured:#?}"
        );
        runtime.shutdown().await;
    }

    /// R-e (L1): the member that wrote a session-less frame left, and a later
    /// member with neutral labels is live under the same identity. The frame
    /// records no member provenance, so a label deny refuses it on the
    /// window, the RPC page (the same query) and the stream decision, while
    /// an admin still reads it.
    #[tokio::test]
    async fn a_session_less_frame_records_no_provenance_while_a_namesake_is_live() {
        let runtime = build_single_member_runtime().await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), entry.clone());
        let resolved = member_sources_for_entry_including_hidden(&entry)
            .await
            .into_iter()
            .next()
            .expect("member");
        let live = identity_record_for_resolved_member(&resolved)
            .await
            .expect("identity");
        assert!(!live.labels.contains_key("tier"), "{live:#?}");
        project_console_event(
            aggregator.inner.clone(),
            "runtime-a",
            crate::console_contracts::ConsoleIdentityEventEnvelope {
                event_id: "session-less-frame".into(),
                interaction_id: None,
                identity: live.runtime_member_id.clone(),
                event_type: "text_delta".into(),
                timestamp_ms: 1000,
                data: json!({ "delta": "written by an earlier member" }),
            },
        )
        .await
        .expect("projected");
        let stored = aggregator
            .store()
            .query_frames(ConsoleTimelineQuery::default())
            .await
            .expect("read")
            .frames
            .into_iter()
            .find(|frame| frame.source_event_id.as_deref() == Some("session-less-frame"))
            .expect("stored frame");
        assert_eq!(stored.identity, live.identity);
        assert_eq!(stored.session_id, None);
        assert_eq!(stored.source.member_provenance, None);

        let controller = crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                crate::access::AccessRule {
                    id: "everyone-views".to_string(),
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["*".to_string()],
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "hide-secret".to_string(),
                    effect: crate::access::AccessEffect::Deny,
                    actions: vec!["agent.view".to_string()],
                    match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
                    ..crate::access::AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("controller");
        let caller = controller.view_for_subject(None);
        let admin = controller.view_for_subject(Some("root@example.test"));
        // Stream decision (snapshot and live frames).
        assert!(!timeline_frame_visible_to(&caller, &stored));
        assert!(timeline_frame_visible_to(&admin, &stored));
        // Window and RPC page: the identity-scoped query with the decider.
        let query = ConsoleTimelineWindowQuery {
            identity: Some(live.identity.clone()),
            mode: ConsoleTimelineMode::Recent,
            limit: 200,
            ..ConsoleTimelineWindowQuery::default()
        };
        let window = |view: crate::access::AccessView| {
            let aggregator = aggregator.clone();
            let query = query.clone();
            async move {
                let decider = TimelineFrameDecider::new(view);
                let filter: &TimelineFrameFilter<'_> =
                    &|frame: &ConsoleFrame| decider.visible(frame);
                aggregator
                    .query_timeline_windowed_for(query, Some(filter))
                    .await
                    .expect("window")
                    .frames
                    .into_iter()
                    .filter(|frame| frame.source_event_id.as_deref() == Some("session-less-frame"))
                    .count()
            }
        };
        assert_eq!(window(caller).await, 0);
        assert_eq!(window(admin).await, 1);
        runtime.shutdown().await;
    }

    /// C3 + R-e: an ancestor is captured only from the incarnation the
    /// child's lineage binds (the spawning session). A live member under the
    /// parent's identity in another session is recorded unconfirmed, without
    /// attributes; the bound incarnation is captured with its own.
    #[tokio::test]
    async fn spawn_ancestors_are_captured_only_from_the_bound_incarnation() {
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        let parent = member_sources_for_entry_including_hidden(&entry)
            .await
            .into_iter()
            .next()
            .expect("member");
        let parent_record = identity_record_for_resolved_member(&parent)
            .await
            .expect("parent identity");
        let parent_session = parent_record
            .session_id
            .clone()
            .expect("a live member has a session");
        let parent_name = parent_record.runtime_member_id.clone();
        let child_record = |bound_session: &str| ConsoleIdentityRecord {
            identity: "test/child".into(),
            display_name: "child".into(),
            runtime_key: "runtime-a".into(),
            runtime_member_id: "child".into(),
            session_id: Some("child-session".into()),
            member_id: None,
            visibility: ConsoleVisibility::RetiredReadable,
            addressable: false,
            health: "retired".into(),
            topology_peers: Vec::new(),
            labels: BTreeMap::from([
                (
                    crate::console_spawn::SPAWNED_BY_LABEL.to_string(),
                    parent_name.clone(),
                ),
                (
                    crate::console_spawn::SPAWNED_BY_SESSION_LABEL.to_string(),
                    bound_session.to_string(),
                ),
            ]),
        };

        let bound = capture_member_lineage(&parent, &child_record(&parent_session)).await;
        assert_eq!(bound.len(), 2, "{bound:#?}");
        assert_eq!(bound[1].identity, parent_record.identity);
        assert_eq!(
            bound[1].labels.as_ref(),
            Some(&{
                let mut labels = parent_record.labels.clone();
                if let Some(grandparent) = labels.get_mut(crate::console_spawn::SPAWNED_BY_LABEL) {
                    *grandparent = apply_namespace(grandparent, "test");
                }
                labels
            })
        );

        let other = capture_member_lineage(&parent, &child_record("an-earlier-parent")).await;
        assert_eq!(other.len(), 2, "{other:#?}");
        assert_eq!(other[1].identity, parent_record.identity);
        assert_eq!(other[1].labels, None, "a later namesake was captured");
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn evicted_provenance_recovers_exact_retired_session_witness() {
        let runtime = build_empty_runtime("retired-provenance-cache").await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("runtime-a".into(), entry.clone());
        let provenance: ConsoleFrameMemberProvenance = serde_json::from_value(json!({
            "identity": {"identity":"agent-a", "display_name":"Agent", "runtime_key":"runtime-a",
                "runtime_member_id":"agent-a", "session_id":"session-a", "visibility":"retired_readable",
                "addressable":false, "health":"retired", "labels":{"private-label":"private"}},
            "member": {"agent_identity":"agent-a", "role":"delegate", "state":"retired",
                "session_id":"session-a", "wired_to":[], "labels":{"private-label":"private"}},
            "primary_mob_id":"primary", "source_mob_id":"lower"
        })).expect("provenance");
        let mut frame =
            counterpart_lineage_frame("retired-witness", "text_delta", json!({"delta":"private"}));
        frame.source.member_provenance = Some(provenance.clone());
        aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("durable witness");
        {
            let mut cache = aggregator.inner.member_provenance.lock().expect("cache");
            for index in 0..MEMBER_PROVENANCE_CACHE_LIMIT + 10 {
                cache.insert(
                    ("runtime-a".into(), format!("temporary-{index}"), None),
                    Arc::new(provenance.clone()),
                );
            }
            assert_eq!(cache.entries.len(), MEMBER_PROVENANCE_CACHE_LIMIT);
        }
        let recovered = member_provenance_for_identity(
            &aggregator.inner,
            &entry,
            "agent-a",
            Some("session-a"),
            false,
        )
        .await;
        assert_eq!(recovered, Some(provenance));
        assert!(
            member_provenance_for_identity(
                &aggregator.inner,
                &entry,
                "agent-a",
                Some("other-session"),
                false
            )
            .await
            .is_none()
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn policy_views_share_canonical_events_without_redacting_each_others_store() {
        struct RedactText;
        impl ConsoleVisibilityPolicy for RedactText {
            fn redact_payload(&self, frame: &NewConsoleFrame) -> Option<Value> {
                (frame.kind == "text_delta").then(|| json!({"delta": "[redacted]"}))
            }
        }
        struct HideText;
        impl ConsoleVisibilityPolicy for HideText {
            fn frame_visible(&self, frame: &ConsoleFrame) -> bool {
                frame.kind != "text_delta"
            }
        }
        let runtime = build_empty_runtime("shared-console-projection").await;
        let owner = MetaConsoleAggregator::in_memory();
        let mut entry = runtime_entry_for_test("default", &runtime);
        entry.identity_namespace.clear();
        owner
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert("default".into(), entry);
        let open = owner.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
        let redacted = owner.policy_view(Arc::new(RedactText));
        let hidden = owner.policy_view(Arc::new(HideText));
        assert!(
            open.inner
                .live_projection_shutdowns
                .lock()
                .expect("tasks")
                .is_empty()
        );
        assert!(
            redacted
                .inner
                .live_projection_shutdowns
                .lock()
                .expect("tasks")
                .is_empty()
        );
        let mut open_rx = open.subscribe();
        let mut redacted_rx = redacted.subscribe();
        for index in 0..40 {
            project_console_event(
                owner.inner.clone(),
                "default",
                crate::console_contracts::ConsoleIdentityEventEnvelope {
                    event_id: format!("shared-{index}"),
                    interaction_id: Some("stream".into()),
                    identity: SYSTEM_EVENT_IDENTITY.into(),
                    event_type: "text_delta".into(),
                    timestamp_ms: 1000 + index,
                    data: json!({"delta": format!("chunk {index}  ")}),
                },
            )
            .await
            .expect("canonical append");
            let a = open_rx.recv().await.expect("open event");
            let b = redacted_rx.recv().await.expect("redacted event");
            assert_eq!(a, b, "views receive the same canonical event");
            assert!(open.timeline_event_visible(&a).await);
            assert!(!hidden.timeline_event_visible(&a).await);
            let ConsoleTimelineEvent::ConsoleFrame { frame: raw } = open.project_event_for_view(a)
            else {
                panic!("frame");
            };
            let ConsoleTimelineEvent::ConsoleFrame { frame: safe } =
                redacted.project_event_for_view(b)
            else {
                panic!("frame");
            };
            assert_eq!(raw.payload["delta"], format!("chunk {index}  "));
            assert_eq!(safe.payload["delta"], "[redacted]");
            assert_eq!(safe.status, ConsoleFrameStatus::Redacted);
            let stored = owner
                .store()
                .frame_by_dedupe_key(&raw.dedupe_key)
                .await
                .expect("read")
                .expect("stored");
            assert_eq!(stored.payload, raw.payload);
        }
        let query = ConsoleTimelineQuery {
            identity: Some(SYSTEM_EVENT_IDENTITY.into()),
            limit: 100,
            ..Default::default()
        };
        assert_eq!(
            open.query_timeline(query.clone())
                .await
                .expect("open query")
                .frames
                .len(),
            40
        );
        assert!(
            hidden
                .query_timeline(query.clone())
                .await
                .expect("hidden query")
                .frames
                .is_empty()
        );
        assert!(
            redacted
                .query_timeline(query)
                .await
                .expect("redacted query")
                .frames
                .iter()
                .all(|frame| frame.payload["delta"] == "[redacted]")
        );
        let sample = open
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(SYSTEM_EVENT_IDENTITY.into()),
                limit: 1,
                ..Default::default()
            })
            .await
            .expect("sample")
            .frames
            .remove(0);
        let mut marker = sample.clone();
        marker.kind = "frame_updated".into();
        marker.payload = json!({ "frame": sample });
        let marker = ConsoleTimelineEvent::ConsoleFrame { frame: marker };
        assert!(
            !hidden.timeline_event_visible(&marker).await,
            "hidden frame stays hidden inside update marker"
        );
        let ConsoleTimelineEvent::ConsoleFrame { frame: projected } =
            redacted.project_event_for_view(marker)
        else {
            panic!("marker");
        };
        assert_eq!(projected.payload["frame"]["payload"]["delta"], "[redacted]");
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn system_identity_frames_are_visible_on_global_timeline_queries() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        // Register a runtime so the per-identity visibility gate engages
        // (an empty registry short-circuits every frame to visible). The
        // test entry carries the "test" identity namespace on purpose.
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry.clone());

        let envelope = |event_id: &str, identity: &str, event_type: &str| {
            crate::console_contracts::ConsoleIdentityEventEnvelope {
                event_id: event_id.to_string(),
                interaction_id: None,
                identity: identity.to_string(),
                event_type: event_type.to_string(),
                timestamp_ms: 1_000,
                data: json!({ "realm": "default" }),
            }
        };

        let system_frame = frame_from_console_event(
            &entry,
            envelope("evt-mem-1", SYSTEM_EVENT_IDENTITY, "memory.dream.completed"),
        );
        assert_eq!(
            system_frame.identity, SYSTEM_EVENT_IDENTITY,
            "projection must not namespace the system identity"
        );
        let ghost_frame = frame_from_console_event(
            &entry,
            envelope("evt-ghost-1", "ghost-agent", "memory.dream.completed"),
        );
        assert_eq!(
            ghost_frame.identity, "test/ghost-agent",
            "ordinary identities keep the namespace"
        );
        for frame in [system_frame, ghost_frame] {
            aggregator
                .store()
                .append_if_absent(frame)
                .await
                .expect("append frame");
        }

        // GLOBAL query: no identity requested, so allow_historical_identity
        // never applies — exactly the lane the pre-fix bug hid frames from.
        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: None,
                limit: 10,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query timeline");
        let identities: Vec<&str> = page
            .frames
            .iter()
            .map(|frame| frame.identity.as_str())
            .collect();
        assert!(
            identities.contains(&SYSTEM_EVENT_IDENTITY),
            "system-attributed memory event must be globally visible: {identities:?}"
        );
        assert!(
            !identities.contains(&"test/ghost-agent"),
            "unknown identities must stay hidden — the exemption is exact: {identities:?}"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn console_frame_uses_canonical_runtime_lineage_for_live_history_join() {
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        let interaction_id = uuid::Uuid::from_u128(0xfeed_8101).to_string();
        let run_id = uuid::Uuid::from_u128(0xfeed_8102).to_string();
        let lineage = json!({"interaction_id": interaction_id, "run_id": run_id});
        let live = frame_from_console_event(
            &entry,
            console_envelope_for_test(
                "live-start",
                Some("unrelated-console-reservation"),
                "run_started",
                json!({"identity": lineage, "run_id": "wrong-top-level", "input":{"kind":"content","content":"same"}}),
            ),
        );
        assert_eq!(
            live.interaction_id.as_deref(),
            Some(interaction_id.as_str())
        );
        assert_eq!(live.run_id.as_deref(), Some(run_id.as_str()));
        let history = frame_from_session_history_message(
            "runtime-a",
            "test/agent-a",
            "session-a",
            0,
            json!({
                "role": "block_assistant", "blocks": [{"block_type":"text","data":{"text":"same"}}],
                "stop_reason":"end_turn", "identity":lineage,
            }),
        )
        .expect("assistant history frame");
        assert_eq!(live.interaction_id, history.interaction_id);
        assert_eq!(live.run_id, history.run_id);
        let run_only = frame_from_console_event(
            &entry,
            console_envelope_for_test(
                "run-only",
                Some("unrelated-console-reservation"),
                "run_started",
                json!({"identity":{"run_id":run_id}}),
            ),
        );
        assert!(run_only.interaction_id.is_none());
        assert_eq!(run_only.run_id.as_deref(), Some(run_id.as_str()));
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn reasoning_console_events_preserve_repeated_fragments_and_completion() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        for (event_id, event_type, text) in [
            ("reasoning-1", "reasoning_delta", "Check "),
            ("reasoning-2", "reasoning_delta", "again "),
            ("reasoning-3", "reasoning_delta", "again "),
            ("reasoning-3", "reasoning_delta", "again "),
            ("reasoning-4", "reasoning_delta", "and finish."),
            (
                "reasoning-5",
                "reasoning_complete",
                "Check again again and finish.",
            ),
            ("reasoning-6", "reasoning_delta", "A complete sentence."),
            ("reasoning-7", "reasoning_complete", "A complete sentence."),
        ] {
            let frame = frame_from_console_event(
                &entry,
                console_envelope_for_test(
                    event_id,
                    Some("interaction-a"),
                    event_type,
                    json!({ "delta": text, "text": text, "turn_id": "turn-a" }),
                ),
            );
            aggregator
                .store()
                .append_if_absent(frame)
                .await
                .expect("append reasoning frame");
        }
        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("test/agent-a".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query timeline");
        assert_eq!(
            page.frames.len(),
            7,
            "only replay of the same source event collapses"
        );
        let streamed: String = page
            .frames
            .iter()
            .filter(|frame| frame.kind == "reasoning_delta")
            .map(|frame| {
                frame.payload["delta"]
                    .as_str()
                    .expect("reasoning delta text")
            })
            .collect();
        assert_eq!(
            streamed,
            "Check again again and finish.A complete sentence."
        );
        assert_eq!(
            page.frames
                .iter()
                .filter(|frame| frame.kind == "reasoning_complete")
                .count(),
            2,
            "completion is retained even when one delta carried the whole segment"
        );
    }

    #[tokio::test]
    async fn reasoning_console_events_replay_legacy_rows_without_dropping_new_fragments() {
        let runtime = build_empty_runtime("reasoning-legacy-replay").await;
        let temp = tempfile::TempDir::new().expect("temp directory");
        for persistent in [false, true] {
            let path = temp.path().join("console.sqlite");
            let store: Arc<dyn ConsoleLogStore> = if persistent {
                Arc::new(SqliteConsoleLogStore::open(&path).expect("open store"))
            } else {
                Arc::new(InMemoryConsoleLogStore::new())
            };
            let mut entry = runtime_entry_for_test("runtime-a", &runtime);
            entry.identity_namespace.clear();
            let envelope =
                |id: &str, kind: &str| crate::console_contracts::ConsoleIdentityEventEnvelope {
                    event_id: id.into(),
                    interaction_id: Some("turn-a".into()),
                    identity: SYSTEM_EVENT_IDENTITY.into(),
                    event_type: kind.into(),
                    timestamp_ms: 1,
                    data: json!({"delta": "again ", "content": "again "}),
                };
            let mut legacy = frame_from_console_event(&entry, envelope("old", "reasoning_delta"));
            legacy.dedupe_key =
                format!("console-reasoning:runtime-a:turn-a:{}", hash_short("again"));
            // Replaying the original event must also match a legitimately
            // redacted stored row without comparing its changed payload.
            legacy.payload = json!({"delta": "[redacted]"});
            legacy.status = ConsoleFrameStatus::Redacted;
            let original = store
                .append_if_absent(legacy)
                .await
                .expect("seed legacy row")
                .frame;
            let store: Arc<dyn ConsoleLogStore> = if persistent {
                drop(store);
                Arc::new(SqliteConsoleLogStore::open(&path).expect("reopen legacy store"))
            } else {
                store
            };
            let owner = MetaConsoleAggregator::new(store.clone());
            owner
                .inner
                .runtimes
                .write()
                .expect("registry")
                .insert("runtime-a".into(), entry);
            let mut events = owner.subscribe();
            for (id, kind) in [
                ("old", "reasoning_delta"),
                ("new", "reasoning_delta"),
                ("complete", "reasoning_complete"),
                ("new", "reasoning_delta"),
                ("complete", "reasoning_complete"),
            ] {
                project_console_event(owner.inner.clone(), "runtime-a", envelope(id, kind))
                    .await
                    .expect("replay exact source event");
            }
            let page = store
                .query_frames(ConsoleTimelineQuery {
                    limit: 20,
                    ..ConsoleTimelineQuery::default()
                })
                .await
                .expect("read frames");
            assert_eq!(page.frames.len(), 3, "distinct equal-text events survive");
            assert_eq!(
                page.frames[0], original,
                "legacy cursor and redaction stay intact"
            );
            assert_eq!(page.frames[1].dedupe_key, "console-event:runtime-a:new");
            assert_eq!(
                page.frames[2].dedupe_key,
                "console-event:runtime-a:complete"
            );
            assert_eq!(
                store
                    .source_watermark("runtime-a", ConsoleFrameSourceKind::ConsoleEvent)
                    .await
                    .expect("watermark"),
                Some("complete".into())
            );
            assert!(events.try_recv().is_ok());
            assert!(events.try_recv().is_ok());
            assert!(
                matches!(
                    events.try_recv(),
                    Err(broadcast::error::TryRecvError::Empty)
                ),
                "exact replays emit no new frames"
            );
        }
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn reasoning_console_events_with_different_text_keep_distinct_frames() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        for (event_id, text) in [
            ("reasoning-1", "I should inspect the file first."),
            ("reasoning-2", "Now I should run the focused test."),
        ] {
            let frame = frame_from_console_event(
                &entry,
                console_envelope_for_test(
                    event_id,
                    Some("interaction-a"),
                    "reasoning_complete",
                    json!({
                        "text": text,
                        "turn_id": "turn-a",
                    }),
                ),
            );
            aggregator
                .store()
                .append_if_absent(frame)
                .await
                .expect("append reasoning frame");
        }

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("test/agent-a".to_string()),
                limit: 10,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query timeline");

        assert_eq!(
            page.frames.len(),
            2,
            "distinct same-turn reasoning segments should not collapse"
        );
        assert_ne!(page.frames[0].dedupe_key, page.frames[1].dedupe_key);
    }

    #[tokio::test]
    async fn identity_recent_pages_keep_all_history_reachable_within_query_limit() {
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "anchored-user-input".to_string(),
                timestamp_ms: 1,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: None,
                kind: "user_input".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "content": "anchor me" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::Synthetic,
                    source_cursor: None,
                },
                source_event_id: Some("anchored-user-input".to_string()),
                interaction_id: Some("turn-a".to_string()),
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append anchor");
        for idx in 2..=40 {
            aggregator
                .store()
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("noisy-tail-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: None,
                    kind: "reasoning_delta".to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "delta": idx }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("noisy-tail-{idx}")),
                    interaction_id: Some("turn-a".to_string()),
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append noisy tail");
        }

        let page = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Recent,
                limit: 5,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query recent identity timeline");

        assert_eq!(
            page.frames.len(),
            5,
            "recent identity pages must respect the requested limit"
        );
        assert_eq!(
            page.frames
                .iter()
                .filter_map(|frame| frame.cursor.seq())
                .collect::<Vec<_>>(),
            vec![36, 37, 38, 39, 40],
            "a recent page must be contiguous so before-oldest paging cannot skip history"
        );
        assert_eq!(
            page.frames.last().and_then(|frame| frame.cursor.seq()),
            Some(40)
        );

        let mut seen = page.frames;
        let mut exhausted = page.exhausted;
        while !exhausted {
            let before = seen.first().expect("nonempty history").cursor.clone();
            let older = aggregator
                .query_timeline_windowed(ConsoleTimelineWindowQuery {
                    identity: Some("agent-a".to_string()),
                    mode: ConsoleTimelineMode::Recent,
                    before: Some(before.clone()),
                    limit: 5,
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await
                .expect("query older identity history");
            assert!(older.frames.len() <= 5);
            assert!(
                older
                    .frames
                    .iter()
                    .all(|frame| frame.cursor.seq() < before.seq())
            );
            assert!(older.exhausted || !older.frames.is_empty());
            exhausted = older.exhausted;
            let mut frames = older.frames;
            frames.extend(seen);
            seen = frames;
            assert!(seen.len() <= 40, "history paging must not duplicate frames");
        }
        assert_eq!(
            seen.iter()
                .filter_map(|frame| frame.cursor.seq())
                .collect::<Vec<_>>(),
            (1..=40).collect::<Vec<_>>(),
            "all history, including the original turn anchor, must remain reachable"
        );
    }

    #[tokio::test]
    async fn query_timeline_recent_pages_cross_hidden_and_short_store_pages() {
        let mut store = CountingConsoleLogStore::new();
        store.window_page_limit = 77;
        let aggregator = MetaConsoleAggregator::new_with_options(
            Arc::new(store),
            ConsoleAggregatorOptions {
                session_history_backfill_enabled: false,
                ..ConsoleAggregatorOptions::default()
            },
        );
        let runtime = build_single_member_runtime().await;
        let mut entry = runtime_entry_for_test("runtime-a", &runtime);
        entry.visibility_policy = Arc::new(HideHiddenNoiseFrames);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry);
        for idx in 1..=2_505 {
            let visible = [1, 1_005, 2_005].contains(&idx);
            aggregator
                .store()
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("recent-hidden-gap-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: None,
                    kind: if visible {
                        "user_input"
                    } else {
                        "hidden_noise"
                    }
                    .to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "content": idx }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("recent-hidden-gap-{idx}")),
                    interaction_id: Some(format!("turn-{idx}")),
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append sparse visible history");
        }
        let query = ConsoleTimelineWindowQuery {
            identity: Some("agent-a".to_string()),
            mode: ConsoleTimelineMode::Recent,
            limit: 2,
            ..ConsoleTimelineWindowQuery::default()
        };
        let first = aggregator
            .query_timeline_windowed(query.clone())
            .await
            .expect("recent page");
        assert_eq!(
            first
                .frames
                .iter()
                .filter_map(|frame| frame.cursor.seq())
                .collect::<Vec<_>>(),
            vec![1_005, 2_005]
        );
        assert!(
            !first.exhausted,
            "the first visible frame remains on an older page"
        );
        assert_eq!(
            first.latest_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(2_505),
            "resume keeps the newest raw frontier across filtered pages"
        );
        let older = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                before: Some(first.frames[0].cursor.clone()),
                ..query.clone()
            })
            .await
            .expect("older page");
        assert_eq!(
            older
                .frames
                .iter()
                .filter_map(|frame| frame.cursor.seq())
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(older.exhausted);
        let exact = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                before: Some(ConsoleCursor::from("console:2006")),
                after: Some(ConsoleCursor::from("console:1")),
                ..query
            })
            .await
            .expect("bounded exact final page");
        assert_eq!(
            exact
                .frames
                .iter()
                .filter_map(|frame| frame.cursor.seq())
                .collect::<Vec<_>>(),
            vec![1_005, 2_005]
        );
        if !exact.exhausted {
            let final_page = aggregator
                .query_timeline_windowed(ConsoleTimelineWindowQuery {
                    identity: Some("agent-a".to_string()),
                    mode: ConsoleTimelineMode::Recent,
                    before: Some(exact.frames[0].cursor.clone()),
                    after: Some(ConsoleCursor::from("console:1")),
                    limit: 2,
                    ..ConsoleTimelineWindowQuery::default()
                })
                .await
                .expect("remaining hidden bounded history");
            assert!(final_page.frames.is_empty());
            assert!(final_page.exhausted);
        }
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn query_timeline_since_skips_hidden_raw_gaps_with_bounded_paging() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let runtime = build_single_member_runtime().await;
        let mut entry = runtime_entry_for_test("runtime-a", &runtime);
        entry.visibility_policy = Arc::new(HideHiddenNoiseFrames);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry);

        for idx in 0..1_500 {
            aggregator
                .store()
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("hidden-gap-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: None,
                    kind: "hidden_noise".to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "delta": idx }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("hidden-gap-{idx}")),
                    interaction_id: None,
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append hidden frame");
        }
        aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "visible-after-gap".to_string(),
                timestamp_ms: 1_501,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: None,
                kind: "interaction_complete".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "text": "visible after hidden gap" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("visible-after-gap".to_string()),
                interaction_id: Some("turn-visible".to_string()),
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append visible frame");

        let page = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Since,
                limit: 1,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query timeline");
        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].dedupe_key, "visible-after-gap");
        assert_eq!(
            page.next_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(1_501)
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn query_timeline_since_cursor_stops_at_last_visible_returned_frame() {
        let aggregator = MetaConsoleAggregator::in_memory();
        for idx in 1..=300 {
            aggregator
                .store()
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("visible-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: None,
                    kind: "interaction_complete".to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "text": format!("visible {idx}") }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("visible-{idx}")),
                    interaction_id: Some(format!("turn-{idx}")),
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append visible frame");
        }

        let first = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Since,
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query first page");
        assert_eq!(first.frames.len(), 10);
        assert_eq!(
            first.next_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(10)
        );

        let second = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Since,
                after: first.next_cursor,
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query second page");
        assert_eq!(second.frames.len(), 10);
        assert_eq!(second.frames[0].dedupe_key, "visible-11");
        assert_eq!(
            second.next_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(20)
        );
    }

    #[tokio::test]
    async fn query_timeline_since_empty_continuation_does_not_force_backfill() {
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let runtime = build_single_member_runtime().await;
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                runtime_entry_for_test("runtime-a", &runtime),
            );
        let inserted = store
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "event-1".to_string(),
                timestamp_ms: 1,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: None,
                kind: "text_delta".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "delta": "hello" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("event-1".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append frame");

        let page = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".to_string()),
                mode: ConsoleTimelineMode::Since,
                after: Some(inserted.frame.cursor),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("query continuation");

        assert!(page.frames.is_empty());
        assert_eq!(
            store.source_watermark_calls(),
            0,
            "empty since continuation must not synchronously force session-history backfill"
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[test]
    fn legacy_timeline_struct_literals_remain_source_compatible() {
        let query = ConsoleTimelineQuery {
            identity: Some("agent-a".to_string()),
            conversation_id: None,
            after: Some(ConsoleCursor::from("console:1")),
            limit: 10,
        };
        let page = ConsoleTimelinePage {
            frames: Vec::new(),
            next_cursor: query.after.clone(),
        };

        assert_eq!(query.limit, 10);
        assert_eq!(
            page.next_cursor.as_ref().and_then(ConsoleCursor::seq),
            Some(1)
        );
    }

    #[tokio::test]
    async fn query_timeline_is_store_local_for_registered_runtimes() {
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let runtime = build_single_member_runtime().await;
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                runtime_entry_for_test("runtime-a", &runtime),
            );
        store
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "event-1".to_string(),
                timestamp_ms: 1,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: None,
                kind: "text_delta".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "delta": "hello" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("event-1".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append frame");

        let page = tokio::time::timeout(
            Duration::from_millis(250),
            aggregator.query_timeline(ConsoleTimelineQuery {
                identity: Some("agent-a".to_string()),
                limit: 10,
                ..ConsoleTimelineQuery::default()
            }),
        )
        .await
        .expect("timeline query should not wait for session history")
        .expect("timeline query succeeds");

        assert_eq!(page.frames.len(), 1);
        assert_eq!(
            store.source_watermark_calls(),
            0,
            "query_timeline must not synchronously touch session-history watermarks"
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn console_send_returns_after_acceptance_without_waiting_for_turn_completion() {
        let runtime = build_single_member_runtime_with_client(Arc::new(SlowTestClient {
            delay: Duration::from_secs(2),
        }))
        .await;
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                runtime_entry_for_test("runtime-a", &runtime),
            );

        let start = Instant::now();
        let accepted = tokio::time::timeout(
            Duration::from_millis(300),
            aggregator.send(ConsoleSendRequest {
                identity: "test/agent-a".to_string(),
                content: json!("hello slow agent"),
                origin: "console:test".to_string(),
                idempotency_key: "nonblocking-send".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            }),
        )
        .await
        .expect("console send should return once the input is accepted")
        .expect("send succeeds");

        assert_eq!(accepted.status, ConsoleFrameStatus::Accepted);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "console send should not wait for the delayed LLM turn"
        );

        wait_for_session_history_text(
            &aggregator,
            "test/agent-a",
            "slow ok",
            Duration::from_secs(5),
        )
        .await
        .expect("background dispatch should still complete and project history");
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovered_late_member_session_backfills_without_manual_refresh() -> Result<(), String>
    {
        let runtime = Arc::new(build_empty_runtime("console-aggregator-late-member-test").await);
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime(ConsoleRuntimeRegistration {
            runtime_key: "runtime-late".to_string(),
            runtime: runtime.clone(),
            identity_namespace: "late".to_string(),
            visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
        });

        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "agent-late".to_string(),
                Some("You are agent-late.".into()),
                None,
                None,
            ))
            .await
            .expect("late member spawns");
        let session_id = send_message_on_mob_with_mode(
            &runtime.mob_handle(),
            "agent-late",
            ContentInput::Text("hello after registration".to_string()),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("direct member send succeeds");
        wait_for_identity_record(
            &aggregator,
            "late/agent-late",
            Some(session_id.as_str()),
            Duration::from_secs(5),
        )
        .await?;

        wait_for_session_history_text(
            &aggregator,
            "late/agent-late",
            "You are agent-late.",
            Duration::from_secs(5),
        )
        .await?;
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_empty_identity_query_force_refreshes_past_fresh_watermark()
    -> Result<(), String> {
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        let resolved = member_sources_for_entry(&entry)
            .await
            .into_iter()
            .find(|candidate| candidate.member.agent_identity.as_str() == "agent-a")
            .expect("agent-a member exists");
        let record = identity_record_for_member(&entry, &resolved.handle, &resolved.member)
            .await
            .expect("identity record exists");
        let session_id = record.session_id.expect("agent-a has a session");
        wait_for_runtime_session_history_text(
            &runtime,
            &session_id,
            "You are agent-a.",
            Duration::from_secs(5),
        )
        .await?;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry);
        let watermark_key = session_history_watermark_runtime_key("runtime-a", &session_id);
        aggregator
            .store()
            .record_source_watermark(
                &watermark_key,
                ConsoleFrameSourceKind::SessionHistory,
                &format_session_history_watermark(&session_id, 0, current_time_ms()),
            )
            .await
            .expect("record fresh empty watermark");

        // Hang/stall backstop only — generous so CPU starvation under full
        // parallel CI load can't false-trip it (matches the sibling timeline
        // query test). The real assertions are on the returned frames below.
        let page = tokio::time::timeout(
            Duration::from_mins(1),
            aggregator.query_timeline(ConsoleTimelineQuery {
                identity: Some("test/agent-a".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            }),
        )
        .await
        .expect("explicit identity query should not stall")
        .expect("query succeeds");

        if !page.frames.is_empty() {
            assert!(
                page.frames.iter().any(|frame| {
                    frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                        && session_history_content_text(frame).as_deref()
                            == Some("You are agent-a.")
                }),
                "query returned frames before the polling assertion; expected them to be the forced session-history refresh, frames: {:#?}",
                page.frames
            );
        }
        wait_for_session_history_text(
            &aggregator,
            "test/agent-a",
            "You are agent-a.",
            Duration::from_secs(5),
        )
        .await?;
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_identity_query_refreshes_stale_existing_session_history() -> Result<(), String>
    {
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let runtime = build_single_member_runtime().await;
        let entry = runtime_entry_for_test("runtime-a", &runtime);
        let resolved = member_sources_for_entry(&entry)
            .await
            .into_iter()
            .find(|candidate| candidate.member.agent_identity.as_str() == "agent-a")
            .expect("agent-a member exists");
        let record = identity_record_for_member(&entry, &resolved.handle, &resolved.member)
            .await
            .expect("identity record exists");
        let session_id = record.session_id.expect("agent-a has a session");
        wait_for_runtime_session_history_text(
            &runtime,
            &session_id,
            "You are agent-a.",
            Duration::from_secs(5),
        )
        .await?;
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert("runtime-a".to_string(), entry);

        store
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "stale-session-history-agent-a".to_string(),
                timestamp_ms: 10,
                runtime_key: "runtime-a".to_string(),
                identity: "test/agent-a".to_string(),
                conversation_id: Some("test/agent-a".to_string()),
                session_id: Some(session_id.clone()),
                kind: "user_input".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({
                    "text": "stale projected history",
                    "type": "user_input",
                }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::SessionHistory,
                    source_cursor: Some("stale-session-history-agent-a".to_string()),
                },
                source_event_id: Some("stale-session-history-agent-a".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append stale history frame");

        let fresh_prompt = "fresh prompt after stale history";
        let sent_session_id = send_message_on_mob_with_mode(
            &runtime.mob_handle(),
            "agent-a",
            ContentInput::Text(fresh_prompt.to_string()),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("direct member send succeeds");
        assert_eq!(sent_session_id, session_id);
        wait_for_runtime_session_history_text(
            &runtime,
            &session_id,
            fresh_prompt,
            Duration::from_secs(5),
        )
        .await?;

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("test/agent-a".to_string()),
                limit: 50,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query child timeline");

        assert!(
            page.frames.iter().any(|frame| {
                frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                    && session_history_content_text(frame).as_deref()
                        == Some("stale projected history")
            }),
            "explicit identity query should return existing history before async refresh; frames: {:#?}",
            page.frames
        );
        wait_for_session_history_text(
            &aggregator,
            "test/agent-a",
            fresh_prompt,
            Duration::from_secs(5),
        )
        .await?;
        assert!(
            store.source_watermark_calls() > 0,
            "stale existing session history should force a targeted source refresh"
        );
        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    async fn wait_for_session_history_text(
        aggregator: &MetaConsoleAggregator,
        identity: &str,
        expected: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            let _ = aggregator.list_identities().await;
            let page = aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(identity.to_string()),
                    limit: 20,
                    ..ConsoleTimelineQuery::default()
                })
                .await
                .expect("query timeline");
            observed = page.frames;
            if observed.iter().any(|frame| {
                frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                    && session_history_content_text(frame).as_deref() == Some(expected)
            }) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        Err(format!(
            "session history text {expected:?} was not backfilled; observed frames: {observed:#?}",
        ))
    }

    async fn wait_for_session_backfill_target(
        aggregator: &MetaConsoleAggregator,
        identity: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(target) =
                session_backfill_target_for_identity(&aggregator.inner, identity).await
            {
                return Ok(target.session_id);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Err(format!(
            "session backfill target for {identity:?} was not resolvable"
        ))
    }

    async fn wait_for_runtime_session_history_text(
        runtime: &UnifiedRuntime,
        session_id: &str,
        expected: &str,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            let page = runtime
                .mob_runtime()
                .read_session_history(session_id, 0, Some(20))
                .await
                .map_err(|err| err.to_string())?;
            observed = page.messages;
            if observed.iter().enumerate().any(|(idx, message)| {
                let Some(message) = serde_json::to_value(message).ok() else {
                    return false;
                };
                frames_from_session_history_message(
                    "runtime-a",
                    "test/agent-a",
                    session_id,
                    idx,
                    message,
                )
                .iter()
                .any(|frame| {
                    matches!(
                        frame.kind.as_str(),
                        "user_input" | "system_notice" | "text_complete"
                    ) && session_history_frame_content_text(frame).as_deref() == Some(expected)
                })
            }) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        Err(format!(
            "runtime session history text {expected:?} was not readable before watermark setup; observed messages: {observed:#?}",
        ))
    }

    async fn wait_for_identity_record(
        aggregator: &MetaConsoleAggregator,
        identity: &str,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            observed = aggregator
                .list_identities()
                .await
                .map_err(|err| err.to_string())?;
            if observed.iter().any(|record| {
                record.identity == identity && record.session_id.as_deref() == session_id
            }) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        Err(format!(
            "identity {identity:?} with session {session_id:?} was not projected; observed identities: {observed:#?}",
        ))
    }

    fn session_history_content_text(frame: &ConsoleFrame) -> Option<String> {
        session_history_payload_text(&frame.payload)
    }

    fn session_history_frame_content_text(frame: &NewConsoleFrame) -> Option<String> {
        session_history_payload_text(&frame.payload)
    }

    fn session_history_payload_text(payload: &Value) -> Option<String> {
        if let Some(text) = payload.get("text").and_then(Value::as_str) {
            return Some(text.to_string());
        }
        if let Some(text) = payload.get("result").and_then(Value::as_str) {
            return Some(text.to_string());
        }
        if let Some(text) = payload.get("body").and_then(Value::as_str) {
            return Some(text.to_string());
        }
        match payload.get("content")? {
            Value::String(text) => Some(text.clone()),
            Value::Array(blocks) => Some(
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        }
    }

    #[tokio::test]
    async fn query_timeline_handles_large_store_without_backfill_calls() {
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        for idx in 0..5_000 {
            store
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("event-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: Some("session-a".to_string()),
                    kind: "text_delta".to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "delta": idx }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("event-{idx}")),
                    interaction_id: None,
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append frame");
        }

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("agent-a".to_string()),
                limit: 1_000,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("large query");

        assert_eq!(page.frames.len(), 1_000);
        assert_eq!(store.source_watermark_calls(), 0);
    }

    #[tokio::test]
    async fn query_timeline_rejects_future_cursor_after_store_reset() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let err = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                after: Some(ConsoleCursor::from("console:99")),
                limit: 10,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect_err("future cursor on empty/reset store must be replay-unavailable");

        assert!(matches!(
            err,
            ConsoleTimelineQueryError::ReplayUnavailable { .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_identity_timeline_backfills_kickoff_when_live_tool_frames_arrive_first() {
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let runtime = build_single_member_runtime().await;
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                runtime_entry_for_test("runtime-a", &runtime),
            );
        let session_id =
            wait_for_session_backfill_target(&aggregator, "test/agent-a", Duration::from_secs(5))
                .await
                .expect("spawned member is resolvable for targeted backfill");
        wait_for_runtime_session_history_text(
            &runtime,
            &session_id,
            "You are agent-a.",
            Duration::from_secs(5),
        )
        .await
        .expect("spawned member kickoff is readable before live-frame race");
        store
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "live-tool-before-history".to_string(),
                timestamp_ms: 10,
                runtime_key: "runtime-a".to_string(),
                identity: "test/agent-a".to_string(),
                conversation_id: Some("test/agent-a".to_string()),
                session_id: None,
                kind: "tool_execution_started".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({
                    "id": "call-live",
                    "name": "index_search",
                    "source_event_type": "tool_execution_started",
                    "type": "tool_execution_started",
                }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: Some("live-tool-before-history".to_string()),
                },
                source_event_id: Some("live-tool-before-history".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append live tool frame");

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("test/agent-a".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query child timeline");

        assert!(
            page.frames
                .iter()
                .any(|frame| frame.kind == "tool_execution_started"),
            "initial explicit child timeline should return existing store frames without waiting for session history"
        );

        // The scheduled backfill either lands the kickoff prompt or it never
        // will: structural, so poll for it instead of budgeting 80 tries * 25ms
        // (a 2s ceiling that measured runner load, not the backfill).
        crate::test_wait::poll_until(
            "scheduled explicit child timeline backfill never included the kickoff prompt (no \
             SessionHistory-sourced user_input frame carrying \"You are agent-a.\")",
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || {
                let page = aggregator
                    .query_timeline(ConsoleTimelineQuery {
                        identity: Some("test/agent-a".to_string()),
                        limit: 20,
                        ..ConsoleTimelineQuery::default()
                    })
                    .await
                    .expect("query child timeline after scheduled backfill");
                page.frames.iter().any(|frame| {
                    frame.kind == "user_input"
                        && frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                        && frame.payload.to_string().contains("You are agent-a.")
                })
            },
        )
        .await;
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn explicit_identity_timeline_query_does_not_resolve_roster_per_frame() {
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let (_temp, runtime, _delayed_service) =
            build_stress_runtime(64, Duration::from_millis(0)).await;
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-a".to_string(),
                runtime_entry_for_test("runtime-a", runtime.as_ref()),
            );
        for idx in 0..1_000 {
            store
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("event-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "test/agent-0".to_string(),
                    conversation_id: Some("test/agent-0".to_string()),
                    session_id: Some("session-a".to_string()),
                    kind: "text_delta".to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "delta": idx }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("event-{idx}")),
                    interaction_id: None,
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append frame");
        }

        // The per-frame roster-rediscovery regression is asserted exactly by
        // `source_watermark_calls() == 0` below (and would also blow this query
        // up to O(frames) async roster resolutions). The timeout is only a
        // hang/O(n^2)-blowup backstop — generous so it never false-trips on CPU
        // starvation under full parallel CI load (the old 2s ceiling flaked the
        // suite); a real regression takes minutes, not seconds, on 1000 frames.
        let page = tokio::time::timeout(
            Duration::from_mins(1),
            aggregator.query_timeline(ConsoleTimelineQuery {
                identity: Some("test/agent-0".to_string()),
                limit: 1_000,
                ..ConsoleTimelineQuery::default()
            }),
        )
        .await
        .expect("identity timeline query should not hang or rediscover the roster per frame")
        .expect("timeline query succeeds");

        assert_eq!(page.frames.len(), 1_000);
        assert_eq!(store.source_watermark_calls(), 0);
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn refresh_session_history_parallelizes_slow_member_backfills_at_scale() {
        const MEMBER_COUNT: usize = 32;
        let (_temp, runtime, delayed_service) =
            build_stress_runtime(MEMBER_COUNT, Duration::ZERO).await;
        // Spawn acknowledges admission, not completion of the initial turn.
        // A recovery correctly defers reads while durable work is draining;
        // settle the fixture before measuring the history-read concurrency.
        let handle = runtime.mob_handle();
        let members = handle.list_members_observation_snapshot().await;
        assert_eq!(members.len(), MEMBER_COUNT);
        let mut sessions = Vec::with_capacity(MEMBER_COUNT);
        let mut unique_sessions = BTreeSet::new();
        for member in members {
            let session = handle
                .resolve_bridge_session_id_observation(&member.agent_identity)
                .await
                .expect("spawned member has its own bound session");
            assert!(
                unique_sessions.insert(session.to_string()),
                "each stress member must own a distinct session"
            );
            sessions.push((member.agent_identity, session));
        }
        let readiness_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        tokio::time::timeout_at(readiness_deadline, async {
            for (member, session) in &sessions {
                let session_id = session.to_string();
                loop {
                    let execution = delayed_service
                        .execution_snapshot(session)
                        .await
                        .expect("initial run execution is observable");
                    let status = crate::member_status_observation::observe_member_status_until(
                        &handle,
                        member,
                        readiness_deadline,
                    )
                    .await
                    .expect("spawned member status is observable");
                    if execution.as_ref().is_some_and(|snapshot| {
                        snapshot.turn_terminal && snapshot.terminal_run_id.is_some()
                    }) && status.current_session_id.as_ref() == Some(session)
                        && status.progress.as_ref().is_some_and(|progress| {
                            progress.run_state == meerkat_mob::MemberRunState::Idle
                                && progress.in_flight_work == 0
                        })
                        && runtime
                            .mob_runtime()
                            .session_commit_pending(&session_id)
                            .await
                            == Some(false)
                        && runtime
                            .mob_runtime()
                            .session_has_active_inputs(&session_id)
                            .await
                            == Some(false)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        })
        .await
        .expect("every initial turn settles before the concurrency measurement");
        let aggregator = MetaConsoleAggregator::in_memory();
        let permit_limit = ConsoleAggregatorOptions::default().max_concurrent_session_backfills;
        assert!(
            MEMBER_COUNT > permit_limit,
            "fixture must oversubscribe the limiter for the peak to be decidable"
        );
        // Hold every read in the first wave until the entire permit pool has
        // entered. This proves overlap independently of scheduler speed; serial
        // or under-filled fan-out cannot release the barrier and times out.
        delayed_service.synchronize_first_history_reads(permit_limit);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-stress".to_string(),
                runtime_entry_for_test("runtime-stress", &runtime),
            );

        tokio::time::timeout(
            Duration::from_secs(10),
            aggregator.refresh_session_history(),
        )
        .await
        .expect("session history backfill must fill the first concurrent read wave")
        .expect("stress refresh");

        assert!(
            delayed_service.read_calls() >= MEMBER_COUNT,
            "expected at least one history read per member, saw {}",
            delayed_service.read_calls()
        );
        assert_eq!(
            delayed_service.max_active_reads(),
            permit_limit,
            "session history backfill must saturate the concurrency limit, not read members \
             serially (peak concurrent reads observed: {})",
            delayed_service.max_active_reads()
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn session_history_backfill_respects_configured_concurrency_limit() {
        const MEMBER_COUNT: usize = 16;
        let (_temp, runtime, delayed_service) =
            build_stress_runtime(MEMBER_COUNT, Duration::from_millis(30)).await;
        let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
            max_concurrent_session_backfills: 4,
            ..ConsoleAggregatorOptions::default()
        });
        aggregator
            .inner
            .runtimes
            .write()
            .expect("runtime registry")
            .insert(
                "runtime-stress".to_string(),
                runtime_entry_for_test("runtime-stress", &runtime),
            );

        aggregator
            .refresh_session_history()
            .await
            .expect("stress refresh");

        assert!(
            delayed_service.max_active_reads() <= 4,
            "configured concurrency limit should bound session history reads, saw {}",
            delayed_service.max_active_reads()
        );
        let _ = runtime.mob_handle().stop().await;
    }

    /// The live pump must keep projecting while a session-history backfill
    /// is in flight. The backfill's first history read is held open on a
    /// gate for the whole burst, so every live frame provably lands while a
    /// backfill read is still running, however fast or slow the machine is.
    /// Only a hang is bounded by time; no pass condition is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_event_burst_reaches_store_while_slow_backfill_is_running() {
        const MEMBER_COUNT: usize = 24;
        const LIVE_EVENT_COUNT: usize = 2_048;
        let (_temp, runtime, delayed_service) =
            build_stress_runtime(MEMBER_COUNT, Duration::ZERO).await;
        let held_read = delayed_service.hold_first_history_read();
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime(ConsoleRuntimeRegistration {
            runtime_key: "runtime-burst".to_string(),
            runtime: runtime.clone(),
            identity_namespace: "stress".to_string(),
            visibility_policy: Arc::new(AllowAllConsoleVisibilityPolicy),
        });
        // Registration replays the (empty) event log, then backfills every
        // member's session history; its first read now waits on the gate.
        held_read.wait_until_entered().await;

        let console_events = runtime.console_events();
        for idx in 0..LIVE_EVENT_COUNT {
            console_events
                .append(
                    "agent-0",
                    Some("burst-turn".to_string()),
                    "text_delta",
                    json!({ "delta": format!("frame-{idx}") }),
                )
                .await;
        }

        // A pump blocked behind the backfill never stores these frames while
        // the read is held, so this bound only turns that hang into a
        // failure; it is far above any machine's projection time.
        let hang_guard = Instant::now() + Duration::from_mins(2);
        let mut observed = 0;
        while observed < LIVE_EVENT_COUNT && Instant::now() < hang_guard {
            tokio::time::sleep(Duration::from_millis(25)).await;
            observed = count_console_event_frames(&aggregator, "stress/agent-0").await;
        }
        assert_eq!(
            observed, LIVE_EVENT_COUNT,
            "every live frame must reach the store while the backfill read is held"
        );
        assert!(
            delayed_service.active_reads() >= 1,
            "the held backfill read must still be in flight when the burst is stored"
        );

        // Once the backfill read finishes, the stored burst is unchanged:
        // the backfill neither drops nor duplicates live frames.
        held_read.release();
        let settled = Instant::now() + Duration::from_mins(2);
        while delayed_service.active_reads() > 0 && Instant::now() < settled {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            delayed_service.active_reads(),
            0,
            "the released read finishes"
        );
        assert_eq!(
            count_console_event_frames(&aggregator, "stress/agent-0").await,
            LIVE_EVENT_COUNT
        );
        let _ = runtime.mob_handle().stop().await;
    }

    async fn count_console_event_frames(
        aggregator: &MetaConsoleAggregator,
        identity: &str,
    ) -> usize {
        let mut after = None;
        let mut count = 0;
        loop {
            let page = aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(identity.to_string()),
                    after,
                    limit: 1_000,
                    ..ConsoleTimelineQuery::default()
                })
                .await
                .expect("query burst timeline");
            if page.frames.is_empty() {
                break;
            }
            count += page
                .frames
                .iter()
                .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::ConsoleEvent)
                .count();
            after = page.next_cursor;
            if after.is_none() {
                break;
            }
        }
        count
    }

    #[tokio::test]
    async fn status_updates_get_replayable_aggregate_cursors() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let frame = NewConsoleFrame {
            id: None,
            dedupe_key: "send-1".to_string(),
            timestamp_ms: 1,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-1".to_string()),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Accepted,
            payload: json!({ "content": "hello" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Send,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: Some("interaction-1".to_string()),
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };
        let inserted = aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("append frame");

        update_frame_status_and_emit(
            &aggregator.inner,
            &inserted.frame.id,
            ConsoleFrameStatus::Delivered,
        )
        .await
        .expect("update status");

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("agent-a".to_string()),
                after: Some(inserted.frame.cursor.clone()),
                limit: 10,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query timeline");
        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].kind, "frame_updated");
        assert_eq!(page.frames[0].parent_frame_id, Some(inserted.frame.id));
        assert_eq!(
            page.frames[0]
                .payload
                .get("frame")
                .and_then(|frame| frame.get("status"))
                .and_then(Value::as_str),
            Some("delivered")
        );
    }

    #[tokio::test]
    async fn steer_delivery_appends_terminal_control_frame() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let frame = NewConsoleFrame {
            id: None,
            dedupe_key: "send-steer-1".to_string(),
            timestamp_ms: 1,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-1".to_string()),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Delivered,
            payload: json!({
                "content": "operator steer",
                "handling_mode": "steer",
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::Send,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: Some("interaction-steer-1".to_string()),
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };
        let inserted = aggregator
            .store()
            .append_if_absent(frame)
            .await
            .expect("append steer input");

        append_steer_delivery_terminal(&aggregator.inner, &inserted.frame, "interaction-steer-1")
            .await
            .expect("append steer terminal");

        let page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("agent-a".to_string()),
                after: Some(inserted.frame.cursor.clone()),
                limit: 10,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query timeline");

        assert_eq!(page.frames.len(), 1);
        let terminal = &page.frames[0];
        assert_eq!(terminal.kind, "interaction_complete");
        assert_eq!(terminal.status, ConsoleFrameStatus::Completed);
        assert_eq!(
            terminal.interaction_id.as_deref(),
            Some("interaction-steer-1")
        );
        assert_eq!(
            terminal.parent_frame_id.as_deref(),
            Some(inserted.frame.id.as_str())
        );
        assert_eq!(
            terminal.payload.get("reason").and_then(Value::as_str),
            Some("steer_delivered")
        );
    }

    #[test]
    fn transcript_fingerprint_matches_equivalent_reasoning_payloads() {
        let live = transcript_fingerprint(
            "reasoning_complete",
            &json!({ "text": "I should inspect the file first." }),
        );
        let replay = transcript_fingerprint(
            "reasoning_complete",
            &json!({ "summary": "I should inspect the file first." }),
        );
        let delta = transcript_fingerprint(
            "reasoning_delta",
            &json!({ "delta": "I should inspect the file first." }),
        );

        assert_eq!(live.as_deref(), Some("I should inspect the file first."));
        assert_eq!(live, replay);
        assert_eq!(live, delta);
    }

    fn runtime_notice_test_message(
        session_id: &meerkat_core::types::SessionId,
        body: &str,
    ) -> Message {
        let mut notice = meerkat_core::types::SystemNoticeMessage::new(
            meerkat_core::types::SystemNoticeKind::Generic,
            body,
        );
        notice.runtime_origin = Some(meerkat_core::types::RuntimeAppendOrigin {
            session_id: session_id.clone(),
            run_id: meerkat_core::lifecycle::RunId(uuid::Uuid::from_u128(0x9201)),
            input_id: meerkat_core::lifecycle::InputId(uuid::Uuid::from_u128(0x9301)),
            append_ordinal: 2,
        });
        Message::SystemNotice(notice)
    }

    fn registration_test_target(entry: RuntimeEntry, session: &SessionId) -> SessionBackfillTarget {
        let mut record = identity_record_for_test("archived-worker");
        record.runtime_key = entry.runtime_key.clone();
        record.session_id = Some(session.to_string());
        record.visibility = ConsoleVisibility::RetiredReadable;
        SessionBackfillTarget {
            assistant_refresh: AssistantHistoryRefreshReason::Recovery,
            provenance: None,
            entry,
            record,
            session_id: session.to_string(),
        }
    }

    fn registration_test_page(session: &SessionId, body: &str) -> SessionHistoryPage {
        SessionHistoryPage::from_messages(
            session.clone(),
            &[runtime_notice_test_message(session, body)],
            SessionHistoryQuery::default(),
        )
    }

    async fn stored_notice_snapshots(aggregator: &MetaConsoleAggregator) -> Vec<ConsoleFrame> {
        aggregator
            .store()
            .query_frames(ConsoleTimelineQuery {
                limit: 100,
                ..Default::default()
            })
            .await
            .expect("stored frames")
            .frames
            .into_iter()
            .filter(|frame| frame.kind == "runtime_notice_snapshot")
            .collect()
    }

    #[tokio::test]
    async fn stale_captured_backfill_target_cannot_read_publish_or_admit_authority_after_replacement()
     {
        // The session has a committed boundary authority, so a current
        // registration's pass would admit it.
        let durable: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let (_temp, runtime, service) = build_stress_runtime_with_durable_runtime_store(
            0,
            Duration::ZERO,
            Some(durable.clone()),
        )
        .await;
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let session = SessionId::new();
        commit_fixture_session_boundary(durable.as_ref(), &session.to_string(), "committed").await;
        let predecessor = runtime_entry_for_test("registration-test", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(predecessor.runtime_key.clone(), predecessor.clone());
        let captured = registration_test_target(predecessor.clone(), &session);
        let mut successor = predecessor.clone();
        successor.registration_id = uuid::Uuid::new_v4();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(successor.runtime_key.clone(), successor.clone());
        service.script_history([ScriptedHistoryRead {
            page: Some(registration_test_page(&session, "Must remain unread.")),
            gate: None,
        }]);
        let reads_before = service.read_calls();
        let watermark_key =
            session_history_watermark_runtime_key(&predecessor.runtime_key, &session.to_string());
        assert!(
            predecessor
                .runtime
                .session_boundary_authority(&session.to_string())
                .await
                .is_some(),
            "precondition: the session's committed authority is readable"
        );
        assert!(!runtime_entry_is_current(&aggregator.inner, &predecessor));
        assert!(runtime_entry_is_current(&aggregator.inner, &successor));
        backfill_one_session_history(aggregator.inner.clone(), captured, true)
            .await
            .expect("stale no-op");
        assert_eq!(service.read_calls(), reads_before);
        assert_eq!(store.source_watermark_calls(), 0);
        assert_eq!(store.record_watermark_calls.load(Ordering::SeqCst), 0);
        assert!(stored_notice_snapshots(&aggregator).await.is_empty());
        assert!(
            !aggregator
                .inner
                .session_backfill_authorities
                .lock()
                .expect("authorities")
                .contains_key(&watermark_key),
            "a stale target admits no completed authority"
        );
        assert!(
            !aggregator
                .inner
                .session_pending_read_authorities
                .lock()
                .expect("authorities")
                .contains_key(&watermark_key),
            "a stale target admits no pending-read authority"
        );
        assert_eq!(
            service.scripted_reads.lock().expect("scripted reads").len(),
            1
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn completed_notice_snapshot_survives_reregister_reopen_and_unavailable_successor_history()
     {
        let (_temp, runtime, service) = build_stress_runtime(0, Duration::ZERO).await;
        let directory = tempfile::tempdir().expect("snapshot store directory");
        let path = directory.path().join("notice-history.sqlite");
        let session = SessionId::new();
        let predecessor = runtime_entry_for_test("registration-test", &runtime);
        let aggregator = MetaConsoleAggregator::new(Arc::new(
            SqliteConsoleLogStore::open(&path).expect("store"),
        ));
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(predecessor.runtime_key.clone(), predecessor.clone());
        let old_message = runtime_notice_test_message(&session, "Removed canonical notice.");
        for row in frames_from_session_history_message_with_namespace(
            &predecessor.runtime_key,
            "archived-worker",
            "",
            &session.to_string(),
            0,
            serde_json::to_value(&old_message).expect("notice JSON"),
        ) {
            aggregator
                .store()
                .append_if_absent(row)
                .await
                .expect("old canonical notice");
        }
        let observation = observe_runtime_notice_attempts(
            &aggregator.inner,
            &predecessor.runtime_key,
            "archived-worker",
            &session.to_string(),
        )
        .await
        .expect("observation");
        let removal = runtime_notice_snapshot_frame(
            &predecessor.runtime_key,
            "archived-worker",
            &session.to_string(),
            &observation,
            &BTreeSet::new(),
            &[],
        )
        .expect("complete removal snapshot");
        let saved = append_and_emit(&aggregator.inner, removal)
            .await
            .expect("valid snapshot append")
            .frame;
        aggregator.unregister_runtime(&predecessor.runtime_key);
        drop(aggregator);

        let reopened = MetaConsoleAggregator::new(Arc::new(
            SqliteConsoleLogStore::open(&path).expect("reopen store"),
        ));
        let mut successor = predecessor;
        successor.registration_id = uuid::Uuid::new_v4();
        reopened
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(successor.runtime_key.clone(), successor.clone());
        service.script_history([ScriptedHistoryRead {
            page: None,
            gate: None,
        }]);
        let reads_before = service.read_calls();
        backfill_one_session_history(
            reopened.inner.clone(),
            registration_test_target(successor, &session),
            true,
        )
        .await
        .expect("unavailable read recorded as gap");
        assert_eq!(service.read_calls(), reads_before + 1);
        assert_eq!(
            stored_notice_snapshots(&reopened).await,
            vec![saved.clone()]
        );
        assert!(
            frame_is_visible_cached(&reopened.inner, &saved, true, &mut HashMap::new(), &[])
                .await
                .expect("snapshot visibility"),
            "a valid durable removal remains visible without a new history image"
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn queued_assistant_boundary_survives_later_positive_only_refreshes() {
        let (_temp, runtime, service) = build_stress_runtime(0, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let session = SessionId::new();
        let entry = runtime_entry_for_test("assistant-refresh-queue", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(entry.runtime_key.clone(), entry.clone());
        let mut target = registration_test_target(entry, &session);
        target.assistant_refresh = AssistantHistoryRefreshReason::PositiveOnly;
        let key = targeted_session_history_active_key(&target, true);
        let gate = HistoryReadGate::new();
        service.script_history([
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "First image")),
                gate: Some(gate.clone()),
            },
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "Settled image")),
                gate: None,
            },
        ]);
        let reads_before = service.read_calls();
        let worker = tokio::spawn(run_targeted_session_history_backfill(
            aggregator.inner.clone(),
            target.clone(),
            true,
        ));
        gate.wait_until_entered().await;
        let mut boundary = target.clone();
        boundary.assistant_refresh = AssistantHistoryRefreshReason::SessionBoundary {
            session_id: session.to_string(),
        };
        run_targeted_session_history_backfill(aggregator.inner.clone(), boundary, true)
            .await
            .expect("queue boundary");
        for _ in 0..8 {
            run_targeted_session_history_backfill(aggregator.inner.clone(), target.clone(), true)
                .await
                .expect("coalesce positive refresh");
        }
        assert!(
            aggregator
                .inner
                .targeted_session_backfills
                .lock()
                .await
                .get(&key)
                .and_then(Option::as_ref)
                .is_some_and(|pending| pending
                    .assistant_refresh
                    .permits_session(&session.to_string())),
            "a later positive-only trigger must preserve the queued negative reconciliation request"
        );
        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("worker finishes")
            .expect("worker joins")
            .expect("history refresh succeeds");
        assert_eq!(
            service.read_calls(),
            reads_before + 2,
            "one trailing read coalesces the burst"
        );
        assert!(
            aggregator
                .inner
                .targeted_session_backfills
                .lock()
                .await
                .is_empty()
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn forced_history_refresh_burst_coalesces_one_trailing_read_and_preserves_final_image() {
        let (_temp, runtime, service) = build_stress_runtime(0, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let session = SessionId::new();
        let entry = runtime_entry_for_test("registration-test", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(entry.runtime_key.clone(), entry.clone());
        let target = registration_test_target(entry, &session);
        let gate = HistoryReadGate::new();
        service.script_history([
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "Before terminal rewrite.")),
                gate: Some(gate.clone()),
            },
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "Final committed image.")),
                gate: None,
            },
        ]);
        let reads_before = service.read_calls();
        let worker = tokio::spawn(run_targeted_session_history_backfill(
            aggregator.inner.clone(),
            target.clone(),
            true,
        ));
        gate.wait_until_entered().await;
        for _ in 0..32 {
            run_targeted_session_history_backfill(aggregator.inner.clone(), target.clone(), true)
                .await
                .expect("overlapping forced trigger coalesces");
        }
        assert_eq!(service.read_calls(), reads_before + 1);
        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("worker finishes")
            .expect("worker joins")
            .expect("both history passes succeed");
        assert_eq!(service.read_calls(), reads_before + 2);
        assert_eq!(service.max_active_reads(), 1);
        assert!(
            aggregator
                .inner
                .targeted_session_backfills
                .lock()
                .await
                .is_empty()
        );
        let snapshots = stored_notice_snapshots(&aggregator).await;
        assert_eq!(snapshots.len(), 2);
        assert_eq!(
            snapshots[0].payload["notices"][0]["message"]["body"],
            "Before terminal rewrite."
        );
        assert_eq!(
            snapshots[1].payload["notices"][0]["message"]["body"],
            "Final committed image."
        );
        assert!(snapshots[0].cursor.seq() < snapshots[1].cursor.seq());

        service.script_history([ScriptedHistoryRead {
            page: Some(registration_test_page(&session, "Later refresh.")),
            gate: None,
        }]);
        run_targeted_session_history_backfill(aggregator.inner.clone(), target, true)
            .await
            .expect("later refresh starts new worker");
        assert_eq!(service.read_calls(), reads_before + 3);
        assert!(
            aggregator
                .inner
                .targeted_session_backfills
                .lock()
                .await
                .is_empty()
        );
        assert_eq!(stored_notice_snapshots(&aggregator).await.len(), 3);
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn forced_history_refresh_keeps_pending_successor_when_stale_predecessor_requests_again()
    {
        let (_temp, runtime, service) = build_stress_runtime(0, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let session = SessionId::new();
        let predecessor = runtime_entry_for_test("registration-test", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(predecessor.runtime_key.clone(), predecessor.clone());
        let old_target = registration_test_target(predecessor.clone(), &session);
        let gate = HistoryReadGate::new();
        service.script_history([
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "Stale read image.")),
                gate: Some(gate.clone()),
            },
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "Successor image.")),
                gate: None,
            },
        ]);
        let reads_before = service.read_calls();
        let worker = tokio::spawn(run_targeted_session_history_backfill(
            aggregator.inner.clone(),
            old_target.clone(),
            true,
        ));
        gate.wait_until_entered().await;
        let mut successor = predecessor;
        successor.registration_id = uuid::Uuid::new_v4();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(successor.runtime_key.clone(), successor.clone());
        let successor_target = registration_test_target(successor, &session);
        run_targeted_session_history_backfill(aggregator.inner.clone(), successor_target, true)
            .await
            .expect("successor schedules trailing pass");
        for _ in 0..8 {
            run_targeted_session_history_backfill(
                aggregator.inner.clone(),
                old_target.clone(),
                true,
            )
            .await
            .expect("stale request ignored");
        }
        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("worker finishes")
            .expect("worker joins")
            .expect("successor converges");
        assert_eq!(service.read_calls(), reads_before + 2);
        let snapshots = stored_notice_snapshots(&aggregator).await;
        assert_eq!(
            snapshots.len(),
            1,
            "old captured read is rejected before publication"
        );
        assert_eq!(
            snapshots[0].payload["notices"][0]["message"]["body"],
            "Successor image."
        );
        assert!(
            aggregator
                .inner
                .targeted_session_backfills
                .lock()
                .await
                .is_empty()
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn failed_held_history_read_still_drains_pending_refresh_and_releases_scheduler() {
        let (_temp, runtime, service) = build_stress_runtime(0, Duration::ZERO).await;
        let aggregator = MetaConsoleAggregator::in_memory();
        let session = SessionId::new();
        let entry = runtime_entry_for_test("registration-test", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(entry.runtime_key.clone(), entry.clone());
        let target = registration_test_target(entry, &session);
        let gate = HistoryReadGate::new();
        service.script_history([
            ScriptedHistoryRead {
                page: None,
                gate: Some(gate.clone()),
            },
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "Recovered final image.")),
                gate: None,
            },
        ]);
        let reads_before = service.read_calls();
        let worker = tokio::spawn(run_targeted_session_history_backfill(
            aggregator.inner.clone(),
            target.clone(),
            true,
        ));
        gate.wait_until_entered().await;
        run_targeted_session_history_backfill(aggregator.inner.clone(), target, true)
            .await
            .expect("pending retry");
        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("worker finishes")
            .expect("worker joins")
            .expect("failed history page recorded as gap");
        assert_eq!(service.read_calls(), reads_before + 2);
        assert!(
            aggregator
                .inner
                .targeted_session_backfills
                .lock()
                .await
                .is_empty()
        );
        let snapshots = stored_notice_snapshots(&aggregator).await;
        assert_eq!(
            snapshots.len(),
            1,
            "failed read grants no invalidating snapshot"
        );
        assert_eq!(
            snapshots[0].payload["notices"][0]["message"]["body"],
            "Recovered final image."
        );
        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test]
    async fn admitted_predecessor_append_finishes_before_successor_snapshot_under_projection_lock()
    {
        let (_temp, runtime, service) = build_stress_runtime(0, Duration::ZERO).await;
        let store = Arc::new(CountingConsoleLogStore::new());
        let aggregator = MetaConsoleAggregator::new(store.clone());
        let session = SessionId::new();
        let predecessor = runtime_entry_for_test("registration-test", &runtime);
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(predecessor.runtime_key.clone(), predecessor.clone());
        let old_target = registration_test_target(predecessor.clone(), &session);
        let gate = HistoryReadGate::new();
        *store.snapshot_append_gate.lock().expect("append gate lock") = Some(gate.clone());
        service.script_history([
            ScriptedHistoryRead {
                page: Some(registration_test_page(
                    &session,
                    "Admitted predecessor image.",
                )),
                gate: None,
            },
            ScriptedHistoryRead {
                page: Some(registration_test_page(&session, "New successor image.")),
                gate: None,
            },
        ]);
        let reads_before = service.read_calls();
        let old_worker = tokio::spawn(backfill_one_session_history(
            aggregator.inner.clone(),
            old_target,
            true,
        ));
        gate.wait_until_entered().await;
        let mut successor = predecessor;
        successor.registration_id = uuid::Uuid::new_v4();
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(successor.runtime_key.clone(), successor.clone());
        let new_worker = backfill_one_session_history(
            aggregator.inner.clone(),
            registration_test_target(successor, &session),
            true,
        );
        tokio::pin!(new_worker);
        assert!(
            matches!(
                futures::poll!(new_worker.as_mut()),
                std::task::Poll::Pending
            ),
            "successor waits on the held session projection lock"
        );
        assert_eq!(service.read_calls(), reads_before + 1);
        gate.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(10), old_worker)
            .await
            .expect("old worker finishes")
            .expect("old worker joins")
            .expect("admitted append succeeds");
        tokio::time::timeout(Duration::from_secs(10), new_worker)
            .await
            .expect("successor finishes")
            .expect("successor append succeeds");
        assert_eq!(service.read_calls(), reads_before + 2);
        let snapshots = stored_notice_snapshots(&aggregator).await;
        assert_eq!(snapshots.len(), 2);
        assert_eq!(
            snapshots[0].payload["notices"][0]["message"]["body"],
            "Admitted predecessor image."
        );
        assert_eq!(
            snapshots[1].payload["notices"][0]["message"]["body"],
            "New successor image."
        );
        assert!(snapshots[0].cursor.seq() < snapshots[1].cursor.seq());
        let _ = runtime.mob_handle().stop().await;
    }

    #[test]
    fn runtime_notice_snapshot_requires_one_complete_exact_session_image() {
        use meerkat_core::service::{SessionHistoryPage, SessionHistoryQuery};
        let session = meerkat_core::types::SessionId::new();
        let messages = vec![runtime_notice_test_message(&session, "Canonical.")];
        let page = SessionHistoryPage::from_messages(
            session.clone(),
            &messages,
            SessionHistoryQuery::default(),
        );
        assert!(complete_current_history_page(&session.to_string(), page.clone()).is_some());
        let mut incomplete = page.clone();
        incomplete.has_more = true;
        assert!(complete_current_history_page(&session.to_string(), incomplete).is_none());
        let mut shifted = page.clone();
        shifted.offset = 1;
        assert!(complete_current_history_page(&session.to_string(), shifted).is_none());
        let mut short = page.clone();
        short.message_count += 1;
        assert!(complete_current_history_page(&session.to_string(), short).is_none());
        assert!(
            complete_current_history_page(&meerkat_core::types::SessionId::new().to_string(), page)
                .is_none()
        );
    }

    #[tokio::test]
    async fn runtime_notice_snapshot_preserves_fork_scope_and_publishes_image_replacement() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let original = meerkat_core::types::SessionId::new();
        let viewed = meerkat_core::types::SessionId::new().to_string();
        let message = runtime_notice_test_message(&original, "Original canonical body.");
        let mut observation = RuntimeNoticeObservation {
            observed_through: 8,
            ..Default::default()
        };
        let first = runtime_notice_snapshot_frame(
            "runtime-a",
            "agent-a",
            &viewed,
            &observation,
            &BTreeSet::new(),
            std::slice::from_ref(&message),
        )
        .expect("initial snapshot must be emitted");
        assert_eq!(first.payload["notices"][0]["offset"], 0);
        assert_eq!(
            first.payload["notices"][0]["message"]["runtime_origin"]["session_id"],
            original.to_string()
        );
        let first = aggregator
            .store()
            .append_if_absent(first)
            .await
            .expect("initial snapshot append succeeds")
            .frame;
        observation.previous_snapshot = Some(first);
        assert!(
            runtime_notice_snapshot_frame(
                "runtime-a",
                "agent-a",
                &viewed,
                &observation,
                &BTreeSet::new(),
                std::slice::from_ref(&message)
            )
            .is_none()
        );
        let removed = runtime_notice_snapshot_frame(
            "runtime-a",
            "agent-a",
            &viewed,
            &observation,
            &BTreeSet::new(),
            &[],
        )
        .expect("removal snapshot must be emitted");
        assert_eq!(removed.payload["notices"], json!([]));
        let removed = aggregator
            .store()
            .append_if_absent(removed)
            .await
            .expect("removal snapshot append succeeds");
        assert_eq!(removed.disposition, AppendDisposition::Inserted);
        observation.previous_snapshot = Some(removed.frame);
        let restored = runtime_notice_snapshot_frame(
            "runtime-a",
            "agent-a",
            &viewed,
            &observation,
            &BTreeSet::new(),
            &[message],
        )
        .expect("restored snapshot must be emitted");
        assert_eq!(
            aggregator
                .store()
                .append_if_absent(restored)
                .await
                .expect("restored snapshot append succeeds")
                .disposition,
            AppendDisposition::Inserted
        );
    }

    #[tokio::test]
    async fn runtime_notice_snapshot_refreshes_same_content_after_new_live_application() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let session = meerkat_core::types::SessionId::new().to_string();
        let attempt = (
            uuid::Uuid::from_u128(0x9201).to_string(),
            uuid::Uuid::from_u128(0x9301).to_string(),
        );
        let mut observation = RuntimeNoticeObservation {
            observed_through: 5,
            attempts: BTreeSet::from([(
                uuid::Uuid::parse_str(&attempt.0).expect("run uuid"),
                uuid::Uuid::parse_str(&attempt.1).expect("input uuid"),
            )]),
            ..Default::default()
        };
        let settled = BTreeSet::from([attempt]);
        let first = runtime_notice_snapshot_frame(
            "runtime-a",
            "agent-a",
            &session,
            &observation,
            &settled,
            &[],
        )
        .expect("settled snapshot must be emitted");
        observation.previous_snapshot = Some(
            aggregator
                .store()
                .append_if_absent(first)
                .await
                .expect("settled snapshot append succeeds")
                .frame,
        );
        observation.observed_through = 12;
        observation.relevant_frontier = 9;
        let next = runtime_notice_snapshot_frame(
            "runtime-a",
            "agent-a",
            &session,
            &observation,
            &settled,
            &[],
        )
        .expect("new live application must emit a snapshot");
        assert_eq!(next.payload["observed_through"], "console:12");
    }

    #[tokio::test]
    async fn public_timeline_preserves_distinct_live_and_committed_rows_and_applies_visibility() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        struct HideTools;
        impl ConsoleVisibilityPolicy for HideTools {
            fn frame_visible(&self, frame: &ConsoleFrame) -> bool {
                frame.kind != "tool_execution_completed"
            }
        }
        let runtime = build_empty_runtime("public-canonical-counterparts").await;
        let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..Default::default()
        });
        aggregator.inner.runtimes.write().expect("registry").insert(
            "runtime-a".into(),
            runtime_entry_for_test("runtime-a", &runtime),
        );
        let provenance: ConsoleFrameMemberProvenance = serde_json::from_value(json!({
            "identity": {"identity":"agent-a", "display_name":"Agent", "runtime_key":"runtime-a",
                "runtime_member_id":"agent-a", "session_id":"session-a", "visibility":"addressable",
                "addressable":true, "health":"ready", "labels":{"private-label":"private"}},
            "member": {"agent_identity":"agent-a", "role":"worker", "state":"running",
                "session_id":"session-a", "wired_to":[], "labels":{"private-label":"private"}},
            "primary_mob_id":"primary", "source_mob_id":"primary"
        }))
        .expect("provenance");
        for (kind, payload, owner) in [
            (
                "user_input",
                json!({ "content": "  Exact human input.\\n" }),
                true,
            ),
            (
                "tool_execution_completed",
                json!({ "id": "tool-owned", "tool_call_id": "tool-owned", "result": "Exact tool result", "is_error": false }),
                false,
            ),
        ] {
            let mut live =
                counterpart_lineage_frame(&format!("live-{kind}"), kind, payload.clone());
            live.source.member_provenance = Some(provenance.clone());
            live.interaction_id = owner.then(|| uuid::Uuid::from_u128(0x9301).to_string());
            live.source.kind = if owner {
                ConsoleFrameSourceKind::Send
            } else {
                ConsoleFrameSourceKind::ConsoleEvent
            };
            if !owner {
                live.run_id = None;
            }
            aggregator
                .store()
                .append_if_absent(live.clone())
                .await
                .expect("live counterpart append succeeds");
            let mut history = live;
            history.dedupe_key = format!("canonical-{kind}");
            history.source.kind = ConsoleFrameSourceKind::SessionHistory;
            history.source.source_cursor = Some(format!("session-a:{}", if owner { 1 } else { 3 }));
            history.source_event_id = None;
            assert!(
                !history_frame_has_existing_counterpart(&aggregator.inner, &history)
                    .await
                    .expect("counterpart lookup succeeds")
            );
            assert_eq!(
                aggregator
                    .store()
                    .append_if_absent(history.clone())
                    .await
                    .expect("canonical counterpart append succeeds")
                    .disposition,
                AppendDisposition::Inserted
            );
            assert_eq!(
                aggregator
                    .store()
                    .append_if_absent(history)
                    .await
                    .expect("duplicate canonical append succeeds")
                    .disposition,
                AppendDisposition::Existing
            );
        }
        let page = aggregator
            .query_timeline_windowed(ConsoleTimelineWindowQuery {
                identity: Some("agent-a".into()),
                ..Default::default()
            })
            .await
            .expect("public timeline query succeeds");
        assert_eq!(page.frames.len(), 4);
        assert!(
            page.frames
                .iter()
                .all(|frame| frame.source.member_provenance.is_none())
        );
        assert_eq!(
            page.frames
                .iter()
                .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::SessionHistory)
                .count(),
            2
        );
        let hidden = aggregator.policy_view(Arc::new(HideTools));
        let app = crate::http_console::console_json_router_with_aggregator(
            crate::RuntimeDecisionState::local_console(
                crate::ConsolePolicy {
                    require_app_auth: false,
                    ..Default::default()
                },
                None,
            ),
            hidden,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/console/timeline?identity=agent-a")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("HTTP query");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let response: Value = serde_json::from_slice(&bytes).expect("JSON");
        let frames = response["frames"].as_array().expect("timeline frames");
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|frame| frame["kind"] == "user_input"));
        assert!(!String::from_utf8_lossy(&bytes).contains("member_provenance"));
        assert!(!String::from_utf8_lossy(&bytes).contains("private-label"));
        let stored = aggregator
            .store()
            .query_frames(ConsoleTimelineQuery {
                identity: Some("agent-a".into()),
                ..Default::default()
            })
            .await
            .expect("raw provenance remains");
        assert_eq!(stored.frames.len(), 4);
        assert!(
            stored
                .frames
                .iter()
                .all(|frame| frame.source.member_provenance.as_ref() == Some(&provenance))
        );
        runtime.shutdown().await;
    }

    fn counterpart_lineage_frame(key: &str, kind: &str, payload: Value) -> NewConsoleFrame {
        NewConsoleFrame {
            id: None,
            dedupe_key: key.to_string(),
            timestamp_ms: 2_000,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-a".to_string()),
            kind: kind.to_string(),
            status: ConsoleFrameStatus::Completed,
            payload,
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: Some(key.to_string()),
            interaction_id: None,
            turn_id: None,
            run_id: Some(uuid::Uuid::from_u128(0xfeed_9201).to_string()),
            parent_frame_id: None,
            caused_by_frame_id: None,
        }
    }

    fn counterpart_lineage_history(text: &str) -> NewConsoleFrame {
        let mut frame = counterpart_lineage_frame(
            "history-answer",
            "interaction_complete",
            json!({ "result": text }),
        );
        frame.source.kind = ConsoleFrameSourceKind::SessionHistory;
        frame.source.source_cursor = Some("session-a:3".to_string());
        frame.source_event_id = None;
        frame
    }

    async fn assert_history_counterpart(
        history: &NewConsoleFrame,
        live: Vec<NewConsoleFrame>,
        expected: bool,
    ) {
        let aggregator = MetaConsoleAggregator::in_memory();
        for frame in live {
            aggregator
                .store()
                .append_if_absent(frame)
                .await
                .expect("append counterpart candidate");
        }
        assert_eq!(
            history_frame_has_existing_counterpart(&aggregator.inner, history)
                .await
                .expect("scan counterpart candidates"),
            expected,
            "history: {history:?}",
        );
    }

    #[tokio::test]
    async fn history_counterpart_lineage_keeps_equal_text_from_different_runs() {
        for interaction in [None, Some(uuid::Uuid::from_u128(0xfeed_9301).to_string())] {
            for kind in ["run_completed", "text_delta"] {
                let mut history = counterpart_lineage_history("Ready.");
                history.interaction_id = interaction.clone();
                let mut live = counterpart_lineage_frame(
                    "other-run",
                    kind,
                    json!({ "result": "Ready.", "delta": "Ready." }),
                );
                live.interaction_id = interaction.clone();
                live.run_id = Some(uuid::Uuid::from_u128(0xfeed_9202).to_string());
                assert_history_counterpart(&history, vec![live], false).await;
            }
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_requires_typed_owner_on_both_text_sources() {
        for missing_history_owner in [false, true] {
            let mut history = counterpart_lineage_history("Ready.");
            let mut live = counterpart_lineage_frame(
                "live-answer",
                "run_completed",
                json!({ "result": "Ready." }),
            );
            if missing_history_owner {
                history.run_id = None;
            } else {
                live.run_id = None;
            }
            assert_history_counterpart(&history, vec![live], false).await;
        }
        let mut history = counterpart_lineage_history("Ready.");
        history.run_id = None;
        let mut live = counterpart_lineage_frame(
            "typed-interaction",
            "run_completed",
            json!({ "result": "Ready." }),
        );
        live.run_id = None;
        live.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
        assert_history_counterpart(&history, vec![live], false).await;
    }

    #[tokio::test]
    async fn history_counterpart_lineage_rejects_runtime_session_and_interaction_conflicts() {
        for conflict in [
            "runtime",
            "session",
            "missing-session",
            "interaction",
            "missing-interaction",
        ] {
            let mut history = counterpart_lineage_history("Ready.");
            history.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
            let mut live = counterpart_lineage_frame(
                "conflicting-owner",
                "run_completed",
                json!({ "result": "Ready." }),
            );
            live.interaction_id = history.interaction_id.clone();
            match conflict {
                "runtime" => live.runtime_key = "runtime-b".to_string(),
                "session" => live.session_id = Some("session-b".to_string()),
                "missing-session" => live.session_id = None,
                "interaction" => {
                    live.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9302).to_string());
                }
                "missing-interaction" => live.interaction_id = None,
                _ => unreachable!(),
            }
            assert_history_counterpart(&history, vec![live], false).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_preserves_owned_assistant_terminal_and_stream_history() {
        for interaction in [None, Some(uuid::Uuid::from_u128(0xfeed_9301).to_string())] {
            let mut history = counterpart_lineage_history("  Ready.\n");
            history.interaction_id = interaction.clone();
            let mut terminal = counterpart_lineage_frame(
                "owned-terminal",
                "run_completed",
                json!({ "result": "  Ready.\n" }),
            );
            terminal.interaction_id = interaction.clone();
            assert_history_counterpart(&history, vec![terminal], false).await;
            let mut deltas = Vec::new();
            for (index, delta) in ["  Re", "ady.\n"].into_iter().enumerate() {
                let mut frame = counterpart_lineage_frame(
                    &format!("owned-delta-{index}"),
                    "text_delta",
                    json!({ "delta": delta }),
                );
                frame.interaction_id = interaction.clone();
                deltas.push(frame);
            }
            assert_history_counterpart(&history, deltas, false).await;
        }
        let mut history = counterpart_lineage_history("Interaction-only owner.");
        history.run_id = None;
        history.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
        let mut live = counterpart_lineage_frame(
            "interaction-only",
            "run_completed",
            json!({ "result": "Interaction-only owner." }),
        );
        live.run_id = None;
        live.interaction_id = history.interaction_id.clone();
        assert_history_counterpart(&history, vec![live], false).await;
    }

    #[tokio::test]
    async fn history_counterpart_lineage_keeps_repeated_authored_messages_with_one_live_occurrence()
    {
        for kind in ["interaction_complete", "reasoning_complete"] {
            let aggregator = MetaConsoleAggregator::in_memory();
            let mut live = counterpart_lineage_frame(
                "one-live-occurrence",
                kind,
                json!({ "result": "Again.", "text": "Again." }),
            );
            live.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
            aggregator
                .store()
                .append_if_absent(live)
                .await
                .expect("append live occurrence");
            for offset in [3, 5] {
                let mut history = counterpart_lineage_history("Again.");
                history.kind = kind.to_string();
                history.payload["text"] = json!("Again.");
                history.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
                history.dedupe_key = format!("authored-message-{offset}");
                history.source.source_cursor = Some(format!("session-a:{offset}"));
                if !history_frame_has_existing_counterpart(&aggregator.inner, &history)
                    .await
                    .expect("check history retention")
                {
                    aggregator
                        .store()
                        .append_if_absent(history.clone())
                        .await
                        .expect("append authored history");
                    let replay = aggregator
                        .store()
                        .append_if_absent(history)
                        .await
                        .expect("replay authored history");
                    assert_eq!(replay.disposition, AppendDisposition::Existing);
                }
            }
            let page = aggregator
                .store()
                .query_frames(ConsoleTimelineQuery {
                    identity: Some("agent-a".to_string()),
                    limit: 10,
                    ..ConsoleTimelineQuery::default()
                })
                .await
                .expect("query retained history");
            assert_eq!(
                page.frames
                    .iter()
                    .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::SessionHistory)
                    .count(),
                2,
                "both authored offsets must reach occurrence reconciliation"
            );
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_keeps_committed_human_input_after_acceptance() {
        let mut history = counterpart_lineage_history("");
        history.kind = "user_input".to_string();
        history.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
        history.payload = json!({ "content": [{ "type": "text", "text": "  Human input.\n" }] });
        let mut accepted = counterpart_lineage_frame(
            "accepted-human",
            "user_input",
            json!({ "content": "  Human input.\n" }),
        );
        accepted.source.kind = ConsoleFrameSourceKind::Send;
        accepted.interaction_id = history.interaction_id.clone();
        accepted.run_id = None;
        assert_history_counterpart(&history, vec![accepted.clone()], false).await;
        for mismatch in [
            "runtime",
            "session",
            "run",
            "interaction",
            "source",
            "whitespace",
            "mixed-content",
        ] {
            let mut candidate = accepted.clone();
            match mismatch {
                "runtime" => candidate.runtime_key = "runtime-b".to_string(),
                "session" => candidate.session_id = None,
                "run" => candidate.run_id = Some(uuid::Uuid::from_u128(0xfeed_9202).to_string()),
                "interaction" => {
                    // Two genuine human sends can contain exactly the same text.
                    candidate.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9302).to_string());
                }
                "source" => candidate.source.kind = ConsoleFrameSourceKind::ConsoleEvent,
                "whitespace" => candidate.payload["content"] = json!("Human input."),
                "mixed-content" => {
                    candidate.payload["content"] = json!([
                        { "type": "text", "text": "  Human input.\n" },
                        { "type": "image", "source": "another-image" }
                    ]);
                }
                _ => unreachable!(),
            }
            assert_history_counterpart(&history, vec![candidate], false).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_keeps_canonical_user_input_with_authoritative_owner() {
        let mut history = counterpart_lineage_history("");
        history.kind = "user_input".to_string();
        history.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
        history.payload = json!({ "content": [{ "type": "text", "text": "  Ready.\n" }] });
        let mut live = counterpart_lineage_frame(
            "typed-user",
            "user_input",
            json!({ "content": "  Ready.\n" }),
        );
        live.interaction_id = history.interaction_id.clone();
        assert_history_counterpart(&history, vec![live.clone()], false).await;
        for mismatch in [
            "runtime",
            "session",
            "run",
            "interaction",
            "whitespace",
            "mixed-content",
        ] {
            let mut candidate = live.clone();
            match mismatch {
                "runtime" => candidate.runtime_key = "runtime-b".to_string(),
                "session" => candidate.session_id = None,
                "run" => candidate.run_id = Some(uuid::Uuid::from_u128(0xfeed_9202).to_string()),
                "interaction" => candidate.interaction_id = None,
                "whitespace" => candidate.payload["content"] = json!("Ready."),
                "mixed-content" => {
                    candidate.payload["content"] = json!([{ "type": "text", "text": "  Ready.\n" }, { "type": "image", "source": "different-image" }]);
                }
                _ => unreachable!(),
            }
            assert_history_counterpart(&history, vec![candidate], false).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_tool_results_preserve_full_content_and_outcome() {
        let mut history = counterpart_lineage_history("");
        history.kind = "tool_execution_completed".to_string();
        history.run_id = None;
        history.payload = json!({ "tool_call_id": "call-a", "result": "  File text\n", "content": [{ "type": "text", "text": "  File text\n" }], "is_error": false });
        let live = counterpart_lineage_frame(
            "typed-tool-result",
            "tool_execution_completed",
            json!({ "tool_call_id": "call-a", "result": [{ "type": "text", "text": "  File text\n" }], "is_error": false }),
        );
        assert_history_counterpart(&history, vec![live.clone()], false).await;
        for mismatch in [
            "whitespace",
            "mixed-content",
            "history-mixed-content",
            "error",
        ] {
            let mut candidate = live.clone();
            let mut durable = history.clone();
            match mismatch {
                "whitespace" => {
                    candidate.payload["result"] = json!([{ "type": "text", "text": "File text" }]);
                }
                "mixed-content" => {
                    candidate.payload["result"] = json!([{ "type": "text", "text": "  File text\n" }, { "type": "image", "source": "different-image" }]);
                }
                "history-mixed-content" => {
                    durable.payload["content"] = json!([{ "type": "text", "text": "  File text\n" }, { "type": "image", "source": "history-image" }]);
                }
                "error" => candidate.payload["is_error"] = json!(true),
                _ => unreachable!(),
            }
            assert_history_counterpart(&durable, vec![candidate], false).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_never_joins_deltas_across_runs() {
        let mut history = counterpart_lineage_history("Ready.");
        history.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
        let mut first =
            counterpart_lineage_frame("run-a-delta", "text_delta", json!({ "delta": "Re" }));
        first.interaction_id = history.interaction_id.clone();
        let mut second =
            counterpart_lineage_frame("run-b-delta", "text_delta", json!({ "delta": "ady." }));
        second.interaction_id = history.interaction_id.clone();
        second.run_id = Some(uuid::Uuid::from_u128(0xfeed_9202).to_string());
        assert_history_counterpart(&history, vec![first, second], false).await;
    }

    #[tokio::test]
    async fn history_counterpart_lineage_preserves_exact_authored_whitespace() {
        for history_text in ["    Ready.", "Ready.\n", "[EVENT via rpc] Ready."] {
            let history = counterpart_lineage_history(history_text);
            let live = counterpart_lineage_frame(
                "different-source",
                "run_completed",
                json!({ "result": "Ready." }),
            );
            assert_history_counterpart(&history, vec![live], false).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_does_not_use_user_or_reasoning_text_as_answer_evidence() {
        for kind in ["user_input", "reasoning_complete"] {
            let history = counterpart_lineage_history("Ready.");
            let live = counterpart_lineage_frame(
                "different-role",
                kind,
                json!({ "content": "Ready.", "text": "Ready." }),
            );
            assert_history_counterpart(&history, vec![live], false).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_does_not_match_a_prefix_of_the_collected_stream() {
        let history = counterpart_lineage_history("Ready.");
        let first =
            counterpart_lineage_frame("first-chunk", "text_delta", json!({ "delta": "Ready." }));
        let second =
            counterpart_lineage_frame("second-chunk", "text_delta", json!({ "delta": " More." }));
        assert_history_counterpart(&history, vec![first, second], false).await;
    }

    #[tokio::test]
    async fn history_counterpart_lineage_tool_results_use_provider_identity_without_inventing_lineage()
     {
        for kind in ["tool_execution_completed", "tool_call_requested"] {
            for mismatch in [
                "none",
                "provider-id",
                "run",
                "interaction",
                "runtime",
                "session",
                "missing-provider-id",
            ] {
                let mut history = counterpart_lineage_history("");
                history.kind = kind.to_string();
                history.run_id = None;
                history.payload = json!({ "tool_call_id": "call-a", "result": "Ready." });
                let mut live =
                    counterpart_lineage_frame("tool-evidence", kind, history.payload.clone());
                match mismatch {
                    "none" => {}
                    "provider-id" => live.payload["tool_call_id"] = json!("call-b"),
                    "run" => history.run_id = Some(uuid::Uuid::from_u128(0xfeed_9202).to_string()),
                    "interaction" => {
                        history.interaction_id =
                            Some(uuid::Uuid::from_u128(0xfeed_9301).to_string());
                        live.interaction_id = Some(uuid::Uuid::from_u128(0xfeed_9302).to_string());
                    }
                    "runtime" => live.runtime_key = "runtime-b".to_string(),
                    "session" => live.session_id = Some("session-b".to_string()),
                    "missing-provider-id" => {
                        history.payload = json!({ "result": "Ready." });
                        live.payload = history.payload.clone();
                    }
                    _ => unreachable!(),
                }
                assert_history_counterpart(&history, vec![live], false).await;
            }
        }
    }

    #[tokio::test]
    async fn history_counterpart_lineage_keeps_distinct_persisted_offsets() {
        let history = counterpart_lineage_history("Ready.");
        let mut earlier = history.clone();
        earlier.dedupe_key = "earlier-persisted-answer".to_string();
        earlier.source.source_cursor = Some("session-a:1".to_string());
        assert_history_counterpart(&history, vec![earlier], false).await;
    }

    #[tokio::test]
    async fn history_counterpart_lineage_preserves_identity_free_legacy_matching() {
        let mut history = counterpart_lineage_history("[EVENT via rpc] Ready.");
        history.run_id = None;
        history.kind = "user_input".to_string();
        history.payload = json!({ "content": "[EVENT via rpc] Ready." });
        for other_runtime in [false, true] {
            let mut live = counterpart_lineage_frame(
                "legacy-user",
                "user_input",
                json!({ "content": "Ready." }),
            );
            live.run_id = None;
            live.interaction_id = Some("console-interaction-old".to_string());
            live.session_id = None;
            if other_runtime {
                live.runtime_key = "runtime-b".to_string();
            }
            assert_history_counterpart(&history, vec![live], !other_runtime).await;
        }
    }

    #[tokio::test]
    async fn history_counterpart_scan_is_not_capped_to_one_page() {
        let aggregator = MetaConsoleAggregator::in_memory();
        for idx in 0..1_005 {
            aggregator
                .store()
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("filler-{idx}"),
                    timestamp_ms: idx,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: Some("session-a".to_string()),
                    kind: "text_delta".to_string(),
                    status: ConsoleFrameStatus::Completed,
                    payload: json!({ "delta": idx }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("filler-{idx}")),
                    interaction_id: None,
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append filler");
        }
        aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "live-user-input".to_string(),
                timestamp_ms: 2_000,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: Some("session-a".to_string()),
                kind: "user_input".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "content": "already here" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("live-user-input".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append live input");

        let history = NewConsoleFrame {
            id: None,
            dedupe_key: "history-user-input".to_string(),
            timestamp_ms: 3_000,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-a".to_string()),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "content": "already here" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::SessionHistory,
                source_cursor: Some("session-a:1006".to_string()),
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };

        assert!(
            history_frame_has_existing_counterpart(&aggregator.inner, &history)
                .await
                .expect("counterpart scan")
        );
    }

    #[tokio::test]
    async fn history_counterpart_scan_matches_rpc_wrapped_user_prompts() {
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "live-user-input".to_string(),
                timestamp_ms: 2_000,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: Some("session-a".to_string()),
                kind: "user_input".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "content": "hello from operator" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("live-user-input".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append live input");

        let history = NewConsoleFrame {
            id: None,
            dedupe_key: "history-user-input".to_string(),
            timestamp_ms: 3_000,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-a".to_string()),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "content": "[EVENT via rpc] hello from operator" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::SessionHistory,
                source_cursor: Some("session-a:2".to_string()),
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };

        assert!(
            history_frame_has_existing_counterpart(&aggregator.inner, &history)
                .await
                .expect("counterpart scan")
        );
    }

    #[tokio::test]
    async fn history_counterpart_scan_matches_content_block_user_prompts() {
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "live-user-input".to_string(),
                timestamp_ms: 2_000,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: Some("session-a".to_string()),
                kind: "user_input".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({ "content": "hello from operator" }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("live-user-input".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append live input");

        let history = NewConsoleFrame {
            id: None,
            dedupe_key: "history-user-input".to_string(),
            timestamp_ms: 3_000,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-a".to_string()),
            kind: "user_input".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({
                "content": [{ "type": "text", "text": "hello from operator" }]
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::SessionHistory,
                source_cursor: Some("session-a:2".to_string()),
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };

        assert!(
            history_frame_has_existing_counterpart(&aggregator.inner, &history)
                .await
                .expect("counterpart scan")
        );
    }

    #[tokio::test]
    async fn history_counterpart_scan_keeps_canonical_tool_result_with_live_twin() {
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .store()
            .append_if_absent(NewConsoleFrame {
                id: None,
                dedupe_key: "live-tool-result".to_string(),
                timestamp_ms: 2_000,
                runtime_key: "runtime-a".to_string(),
                identity: "agent-a".to_string(),
                conversation_id: Some("agent-a".to_string()),
                session_id: Some("session-a".to_string()),
                kind: "tool_execution_completed".to_string(),
                status: ConsoleFrameStatus::Delivered,
                payload: json!({
                    "id": "call-1",
                    "tool_call_id": "call-1",
                    "result": "{ \"count\": 70 }"
                }),
                source: ConsoleFrameSource {
                    member_provenance: None,
                    kind: ConsoleFrameSourceKind::ConsoleEvent,
                    source_cursor: None,
                },
                source_event_id: Some("live-tool-result".to_string()),
                interaction_id: None,
                turn_id: None,
                run_id: None,
                parent_frame_id: None,
                caused_by_frame_id: None,
            })
            .await
            .expect("append live tool result");

        let history = NewConsoleFrame {
            id: None,
            dedupe_key: "history-tool-result".to_string(),
            timestamp_ms: 3_000,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-a".to_string()),
            kind: "tool_execution_completed".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({
                "id": "call-1",
                "tool_call_id": "call-1",
                "result": "{ \"count\": 70 }",
                "content": [{ "type": "text", "text": "{ \"count\": 70 }" }]
            }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::SessionHistory,
                source_cursor: Some("session-a:3:0".to_string()),
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };

        assert!(
            !history_frame_has_existing_counterpart(&aggregator.inner, &history)
                .await
                .expect("counterpart scan")
        );
    }

    #[tokio::test]
    async fn history_counterpart_scan_matches_streamed_text_delta_completion() {
        let aggregator = MetaConsoleAggregator::in_memory();
        for (idx, delta) in ["Ready ", "and standing by."].iter().enumerate() {
            aggregator
                .store()
                .append_if_absent(NewConsoleFrame {
                    id: None,
                    dedupe_key: format!("live-delta-{idx}"),
                    timestamp_ms: 2_000 + idx as u64,
                    runtime_key: "runtime-a".to_string(),
                    identity: "agent-a".to_string(),
                    conversation_id: Some("agent-a".to_string()),
                    session_id: Some("session-a".to_string()),
                    kind: "text_delta".to_string(),
                    status: ConsoleFrameStatus::Delivered,
                    payload: json!({ "delta": delta }),
                    source: ConsoleFrameSource {
                        member_provenance: None,
                        kind: ConsoleFrameSourceKind::ConsoleEvent,
                        source_cursor: None,
                    },
                    source_event_id: Some(format!("live-delta-{idx}")),
                    interaction_id: Some("turn-a".to_string()),
                    turn_id: None,
                    run_id: None,
                    parent_frame_id: None,
                    caused_by_frame_id: None,
                })
                .await
                .expect("append live delta");
        }

        let history = NewConsoleFrame {
            id: None,
            dedupe_key: "history-assistant-complete".to_string(),
            timestamp_ms: 3_000,
            runtime_key: "runtime-a".to_string(),
            identity: "agent-a".to_string(),
            conversation_id: Some("agent-a".to_string()),
            session_id: Some("session-a".to_string()),
            kind: "interaction_complete".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "result": "Ready and standing by." }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::SessionHistory,
                source_cursor: Some("session-a:3".to_string()),
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        };

        assert!(
            history_frame_has_existing_counterpart(&aggregator.inner, &history)
                .await
                .expect("counterpart scan")
        );
    }

    #[test]
    fn session_history_watermark_key_is_session_scoped() {
        assert_ne!(
            session_history_watermark_runtime_key("runtime-a", "session-1"),
            session_history_watermark_runtime_key("runtime-a", "session-2")
        );
    }

    #[test]
    fn backfill_offset_advances_only_on_strict_forward_progress() {
        // Normal full-page advance from the start.
        assert_eq!(advance_backfill_offset(0, 0, 500), Some(500));
        // Subsequent full page from a correct cursor advances.
        assert_eq!(advance_backfill_offset(500, 500, 500), Some(1000));
        // A partial page that still moves forward advances.
        assert_eq!(advance_backfill_offset(500, 500, 3), Some(503));
        // One element past the requested offset still counts as progress.
        assert_eq!(advance_backfill_offset(499, 0, 500), Some(500));

        // Livelock shapes: the store re-serves a page whose (base + len) does
        // NOT advance past what we asked for. These must stop the loop.
        assert_eq!(advance_backfill_offset(500, 0, 500), None); // cursor reset to 0, full page
        assert_eq!(advance_backfill_offset(500, 0, 400), None); // strictly behind
        assert_eq!(advance_backfill_offset(1000, 500, 500), None); // lands exactly on offset
        assert_eq!(advance_backfill_offset(10, 10, 0), None); // empty page at same offset

        // Saturating add: a pathological base near usize::MAX cannot panic.
        assert_eq!(advance_backfill_offset(0, usize::MAX, 5), Some(usize::MAX));
    }

    #[test]
    fn session_history_watermarks_are_cursor_and_ttl_aware() {
        let legacy = "session:with:colon:42";
        let checked = format_session_history_watermark("session:with:colon", 43, 1_000);
        let empty_checked = format_session_history_watermark("session:with:colon", 0, 1_000);

        assert_eq!(
            parse_session_history_watermark(legacy, "session:with:colon"),
            Some(42)
        );
        assert_eq!(
            parse_session_history_watermark(&checked, "session:with:colon"),
            Some(43)
        );
        assert!(session_history_watermark_is_fresh(
            &checked,
            "session:with:colon",
            1_500
        ));
        assert!(!session_history_watermark_is_fresh(
            &checked,
            "session:with:colon",
            1_000 + SESSION_HISTORY_GROWING_REFRESH_TTL_MS + 1
        ));
        assert!(session_history_watermark_is_fresh(
            &empty_checked,
            "session:with:colon",
            1_500
        ));
        assert!(!session_history_watermark_is_fresh(
            &empty_checked,
            "session:with:colon",
            1_000 + SESSION_HISTORY_REFRESH_TTL_MS + 1
        ));
    }

    #[test]
    fn session_history_projection_retains_authored_text_without_inventing_run_terminals() {
        let interaction = uuid::Uuid::from_u128(0xfeed_9301).to_string();
        let run = uuid::Uuid::from_u128(0xfeed_9201).to_string();
        let frames: Vec<_> = ["tool_use", "end_turn"]
            .into_iter()
            .enumerate()
            .flat_map(|(offset, stop_reason)| {
                frames_from_session_history_message(
                    "runtime-a",
                    "agent-a",
                    "session-a",
                    offset,
                    json!({
                        "role": "block_assistant",
                        "identity": { "interaction_id": interaction, "run_id": run },
                        "blocks": [{ "block_type": "text", "data": { "text": "Again.\n" } }],
                        "stop_reason": stop_reason
                    }),
                )
            })
            .collect();
        assert_eq!(frames.len(), 2, "both equal authored occurrences survive");
        for frame in &frames {
            assert_eq!(
                frame.kind, "text_complete",
                "a stored message is not a run terminal"
            );
            assert_eq!(frame.payload["result"], json!("Again.\n"));
            assert_eq!(frame.interaction_id.as_deref(), Some(interaction.as_str()));
            assert_eq!(frame.run_id.as_deref(), Some(run.as_str()));
        }
        assert_ne!(
            frames[0].source.source_cursor,
            frames[1].source.source_cursor
        );
        assert_ne!(frames[0].dedupe_key, frames[1].dedupe_key);
    }

    #[test]
    fn session_history_messages_project_to_renderable_frames() {
        let user = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "user",
                "content": "hello",
                "timestamp_ms": 10
            }),
        )
        .expect("user history frame");
        // meerkat 0.7 removed the legacy flat-text `Message::Assistant`
        // variant: session stores serialize assistant turns only as
        // `block_assistant` (MessageWire), so the history projection sees the
        // block shape.
        let assistant = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            1,
            json!({
                "role": "block_assistant",
                "blocks": [
                    { "block_type": "text", "data": { "text": "hi there" } }
                ],
                "stop_reason": "end_turn",
                "timestamp_ms": 11
            }),
        )
        .expect("assistant history frame");

        assert_eq!(user.kind, "user_input");
        assert_eq!(user.source.kind, ConsoleFrameSourceKind::SessionHistory);
        assert_eq!(
            user.payload["content"],
            json!([{ "type": "text", "text": "hello" }])
        );
        assert_eq!(assistant.kind, "text_complete");
        assert_eq!(assistant.payload["text"], json!("hi there"));
        assert!(
            assistant
                .dedupe_key
                .starts_with("session-history:runtime-a:session-a:1:")
        );
    }

    #[test]
    fn session_history_projection_filters_scaffold_user_messages() {
        let spawn_notice = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "user",
                "content": "You have been spawned as 'agent-a' (role: worker) in mob 'mob-a'.",
                "timestamp_ms": 10
            }),
        );
        let peer_update = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            1,
            json!({
                "role": "user",
                "content": [{ "type": "text", "text": "[PEER UPDATE] alpha wired to beta" }],
                "timestamp_ms": 11
            }),
        );
        let real_user = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            2,
            json!({
                "role": "user",
                "content": "Please review the incident notes.",
                "timestamp_ms": 12
            }),
        );

        assert!(spawn_notice.is_none());
        assert!(peer_update.is_none());
        assert!(real_user.is_some());
    }

    #[test]
    fn session_history_projection_skips_non_transcript_messages() {
        let skipped = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "content": "internal system prompt"
            }),
        );
        assert!(skipped.is_none());
    }

    #[test]
    fn session_history_projection_extracts_assistant_blocks() {
        let frame = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "block_assistant",
                "blocks": [
                    { "block_type": "text", "data": { "text": "hello " } },
                    { "block_type": "text", "data": { "text": "there" } }
                ],
                "stop_reason": "end_turn"
            }),
        )
        .expect("assistant block history frame");
        assert_eq!(frame.payload["text"], json!("hello there"));
    }

    #[test]
    fn session_history_projection_extracts_nested_text_block_data() {
        let frame = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "text",
                        "data": { "text": "Ready and standing by." }
                    }
                ],
                "stop_reason": "end_turn",
                "created_at": "1970-01-01T00:00:00.010Z"
            }),
        )
        .expect("assistant block history frame");

        assert_eq!(frame.kind, "text_complete");
        assert_eq!(frame.payload["result"], json!("Ready and standing by."));
    }

    #[test]
    fn session_history_projection_drops_reasoning_blocks_from_result_text() {
        let frame = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "reasoning",
                        "data": { "text": "**Planning**\nI should not be rendered." }
                    },
                    {
                        "block_type": "text",
                        "data": { "text": "Visible answer." }
                    }
                ],
                "stop_reason": "end_turn",
                "created_at": "1970-01-01T00:00:00.010Z"
            }),
        )
        .expect("assistant block history frame");

        assert_eq!(frame.kind, "text_complete");
        assert_eq!(frame.payload["result"], json!("Visible answer."));
        assert_eq!(frame.payload["text"], json!("Visible answer."));
    }

    /// A reasoning-plus-tool step with no visible text is not a terminal
    /// (it used to project an `interaction_complete` with `result: ""`, the
    /// empty completion of the R6 repro): it projects its tool call, and the
    /// reasoning text reaches no frame at all.
    #[test]
    fn session_history_projection_never_leaks_reasoning_into_a_text_less_step() {
        let frames = frames_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "reasoning",
                        "data": { "text": "Private planning text." }
                    },
                    {
                        "block_type": "tool_use",
                        "data": { "id": "toolu-1", "name": "peers", "args": {} }
                    }
                ],
                "stop_reason": "end_turn",
                "created_at": "1970-01-01T00:00:00.010Z"
            }),
        );

        assert!(
            frames
                .iter()
                .all(|frame| frame.kind != "interaction_complete"),
            "a text-less step is not a terminal: {frames:#?}"
        );
        let call = frames
            .iter()
            .find(|frame| frame.kind == "tool_call_requested")
            .expect("the tool call projects");
        assert_eq!(call.payload["tool_call_id"], json!("toolu-1"));
        assert!(
            frames
                .iter()
                .all(|frame| !frame.payload.to_string().contains("Private planning text.")),
            "reasoning text must not reach any history frame: {frames:#?}"
        );
    }

    /// Sender-side comms parity: assistant history whose tool calls are the
    /// peer send tools re-emits live-shaped `tool_call_requested` frames (and
    /// only for those tools), and the live↔history twin collapses through
    /// the tool-call fingerprint.
    #[test]
    fn session_history_projection_emits_outgoing_comms_tool_calls() {
        let frames = frames_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            3,
            json!({
                "role": "block_assistant",
                "blocks": [
                    { "block_type": "text", "data": { "text": "Sending updates." } },
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "toolu-send-1",
                            "name": "send_message",
                            "args": { "peer_id": "peer-1", "body": "status updated" }
                        }
                    },
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "toolu-req-1",
                            "name": "send_request",
                            "args": {
                                "peer_id": "peer-1",
                                "intent": "checksum_token",
                                "params": { "subject": "status" }
                            }
                        }
                    },
                    {
                        "block_type": "tool_use",
                        "data": { "id": "toolu-other", "name": "workgraph_get", "args": {} }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.010Z"
            }),
        );

        let tool_frames: Vec<_> = frames
            .iter()
            .filter(|frame| frame.kind == "tool_call_requested")
            .collect();
        assert_eq!(
            tool_frames.len(),
            2,
            "exactly the two comms send calls project (not workgraph_get): {frames:?}"
        );
        let message_frame = tool_frames
            .iter()
            .find(|frame| frame.payload["name"] == json!("send_message"))
            .expect("send_message history frame");
        assert_eq!(message_frame.payload["tool_call_id"], json!("toolu-send-1"));
        assert_eq!(
            message_frame.payload["args"]["body"],
            json!("status updated")
        );
        assert_eq!(message_frame.conversation_id.as_deref(), Some("agent-a"));
        let request_frame = tool_frames
            .iter()
            .find(|frame| frame.payload["name"] == json!("send_request"))
            .expect("send_request history frame");
        assert_eq!(
            request_frame.payload["args"]["params"]["subject"],
            json!("status")
        );

        // A live twin (same provider-minted tool_use_id) shares the
        // counterpart fingerprint, so the backfill skips the duplicate.
        let history_fingerprint =
            transcript_fingerprint(&message_frame.kind, &message_frame.payload);
        let live_fingerprint = transcript_fingerprint(
            "tool_call_requested",
            &json!({
                "id": "toolu-send-1",
                "tool_call_id": "toolu-send-1",
                "name": "send_message",
                "args": { "peer_id": "peer-1", "body": "status updated" },
                "source_event_type": "tool_call_requested",
            }),
        );
        assert_eq!(history_fingerprint, live_fingerprint);
        assert!(history_fingerprint.is_some());
    }

    #[test]
    fn session_history_projection_preserves_tool_results() {
        for is_error in [false, true] {
            for include_diagnostic in [false, true] {
                let failures = if include_diagnostic {
                    vec![crate::test_diagnostics::settlement_failure(is_error)]
                } else {
                    Vec::new()
                };
                let frames = frames_from_session_history_message(
                    "runtime-a",
                    "agent-a",
                    "session-a",
                    5,
                    json!({
                        "role": "tool_results",
                        "results": [
                            {
                                "tool_use_id": "call-peers",
                                "content": "{\"peers\":[{\"peer_id\":\"peer-1\",\"name\":\"mob/worker/peer-1\"}]}",
                                "is_error": is_error,
                                "settlement_failures": failures,
                            }
                        ],
                        "created_at": "1970-01-01T00:00:00.050Z"
                    }),
                );

                assert_eq!(frames.len(), 1);
                let frame = &frames[0];
                assert_eq!(frame.kind, "tool_execution_completed");
                assert_eq!(frame.status, ConsoleFrameStatus::Completed);
                assert_eq!(frame.payload["tool_call_id"], json!("call-peers"));
                assert_eq!(frame.payload["is_error"], json!(is_error));
                assert_eq!(
                    frame.payload["result"],
                    json!("{\"peers\":[{\"peer_id\":\"peer-1\",\"name\":\"mob/worker/peer-1\"}]}")
                );
                if include_diagnostic {
                    assert_eq!(frame.payload["settlement_failures"], json!(failures));
                } else {
                    assert!(frame.payload.get("settlement_failures").is_none());
                }
                assert_eq!(frame.source.kind, ConsoleFrameSourceKind::SessionHistory);
                assert_eq!(frame.timestamp_ms, 50);
            }
        }
    }

    #[test]
    fn session_history_projects_only_native_mcp_app_locators() {
        let record = json!({
            "registration": { "server": "charts", "connection": "physical-1" },
            "tool": { "name": "show_chart", "inputSchema": { "type": "object" },
                "_meta": { "ui": { "resourceUri": "ui://charts/view" } } },
            "arguments": {},
            "result": { "content": [{ "type": "text", "text": "Chart ready" }],
                "_meta": { "private_host_value": "do-not-project" } },
            "resource": { "contents": [{ "uri": "ui://charts/view", "text": "<html>private resource</html>",
                "mimeType": "text/html;profile=mcp-app" }] }
        });
        let message = json!({
            "role": "tool_results", "created_at": "1970-01-01T00:00:00.050Z",
            "results": [{ "tool_use_id": "call-chart", "content": "Chart ready", "is_error": false,
                "host_metadata": { "io.modelcontextprotocol/ui": record } }]
        });
        let frames = frames_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            5,
            message.clone(),
        );
        let apps = frames
            .iter()
            .filter(|frame| frame.kind == "mcp_app")
            .collect::<Vec<_>>();
        assert_eq!(apps.len(), 1);
        assert_eq!(
            apps[0].payload,
            json!({ "session_id": "session-a", "tool_call_id": "call-chart", "fallback": "Chart ready" })
        );
        assert_eq!(apps[0].source.kind, ConsoleFrameSourceKind::SessionHistory);
        let projected = serde_json::to_string(&frames).expect("history frames should serialize");
        assert!(!projected.contains("do-not-project"));
        assert!(!projected.contains("private resource"));
        assert!(!projected.contains("physical-1"));

        let mut forged = message;
        let record = forged["results"][0]["host_metadata"].take();
        forged["results"][0]["content"] = Value::String(record.to_string());
        assert!(
            frames_from_session_history_message("runtime-a", "agent-a", "session-a", 5, forged)
                .iter()
                .all(|frame| frame.kind != "mcp_app")
        );
    }

    #[test]
    fn session_history_projection_derives_assistant_image_from_image_tool_result() {
        // Regression: a generate_image tool result rebuilt from session-history
        // backfill must derive an `assistant_image` frame (image_id/blob_id/
        // media_type/width/height) just like the live ingest edge does. Without
        // it the image renders as raw tool JSON after a reload / fresh store.
        // The serialized ImageGenerationToolResult carries NO tool name, so the
        // backfill parses it directly.
        let image_result = json!({
            "operation_id": "00000000-0000-0000-0000-0000000000aa",
            "terminal": { "terminal": "generated" },
            "images": [
                {
                    "image_id": "00000000-0000-0000-0000-000000000051",
                    "blob_ref": {
                        "blob_id": "sha256:test-generated-image",
                        "media_type": "image/png"
                    },
                    "media_type": "image/png",
                    "width": 1024,
                    "height": 768
                }
            ],
            "provider_text": { "disposition": "not_emitted" },
            "revised_prompt": { "disposition": "not_requested" },
            "native_metadata": { "provider": "not_emitted" }
        });
        // Sanity: the fixture must be a valid ImageGenerationToolResult so the
        // test pins the real wire contract, not an arbitrary blob.
        serde_json::from_value::<meerkat_core::image_generation::ImageGenerationToolResult>(
            image_result.clone(),
        )
        .expect("fixture must be a valid ImageGenerationToolResult");
        let result_text = serde_json::to_string(&image_result).expect("serialize image result");

        let frames = frames_from_session_history_message(
            "runtime-a",
            "artist:singleton",
            "session-a",
            6,
            json!({
                "role": "tool_results",
                "results": [
                    {
                        "tool_use_id": "call-image",
                        "content": result_text,
                        "is_error": false
                    }
                ],
                "created_at": "1970-01-01T00:00:00.060Z"
            }),
        );

        // The generic tool_execution_completed frame is still emitted...
        assert!(
            frames
                .iter()
                .any(|frame| frame.kind == "tool_execution_completed"),
            "tool_execution_completed frame must still be present: {frames:#?}"
        );
        // ...plus a derived assistant_image frame so the picture renders.
        let image_frame = frames
            .iter()
            .find(|frame| frame.kind == "assistant_image")
            .expect("backfill must derive an assistant_image frame");
        assert_eq!(
            image_frame.payload["blob_id"],
            json!("sha256:test-generated-image")
        );
        assert_eq!(
            image_frame.payload["image_id"],
            json!("00000000-0000-0000-0000-000000000051")
        );
        assert_eq!(image_frame.payload["media_type"], json!("image/png"));
        assert_eq!(image_frame.payload["width"], json!(1024));
        assert_eq!(image_frame.payload["height"], json!(768));
        assert_eq!(image_frame.payload["tool_call_id"], json!("call-image"));
        assert_eq!(
            image_frame.source.kind,
            ConsoleFrameSourceKind::SessionHistory
        );
        assert_eq!(image_frame.timestamp_ms, 60);
    }

    #[test]
    fn session_history_projection_surfaces_spawn_initial_message_on_child_timeline() {
        let frames = frames_from_session_history_message(
            "runtime-a",
            "review:singleton",
            "session-a",
            7,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "call-spawn",
                            "name": "mob_spawn_member",
                            "args": {
                                "member_id": "review-worker-alpha",
                                "profile": "review-worker",
                                "initial_message": "Review Initiative Alpha and save the result."
                            }
                        }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.070Z"
            }),
        );

        assert_eq!(frames.len(), 2);
        let child = frames
            .iter()
            .find(|frame| frame.identity == "review-worker-alpha")
            .expect("spawned member initial message frame");
        assert_eq!(child.kind, "user_input");
        assert_eq!(child.timestamp_ms, 70);
        assert_eq!(
            child.payload["message"]["content"],
            "Review Initiative Alpha and save the result."
        );
        assert_eq!(child.payload["parent_identity"], "review:singleton");
        assert_eq!(child.payload["via_tool"], "mob_spawn_member");
        assert_eq!(child.source.kind, ConsoleFrameSourceKind::SessionHistory);
    }

    #[test]
    fn session_history_projection_surfaces_specs_spawn_initial_messages() {
        let frames = frames_from_session_history_message(
            "runtime-a",
            "review:singleton",
            "session-a",
            8,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "call-spawn-many",
                            "name": "mob_spawn_member",
                            "args": {
                                "specs": [
                                    {
                                        "agent_identity": "review-worker-alpha",
                                        "profile": "review-worker",
                                        "initial_message": "Review Initiative Alpha."
                                    },
                                    {
                                        "agent_identity": "review-worker-beta",
                                        "profile": "review-worker",
                                        "initial_message": "Review Initiative Beta."
                                    }
                                ]
                            }
                        }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.080Z"
            }),
        );

        assert_eq!(frames.len(), 3);
        let child_messages = frames
            .iter()
            .filter(|frame| frame.kind == "user_input")
            .map(|frame| {
                (
                    frame.identity.as_str(),
                    frame.payload["message"]["content"]
                        .as_str()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            child_messages,
            vec![
                ("review-worker-alpha", "Review Initiative Alpha."),
                ("review-worker-beta", "Review Initiative Beta."),
            ]
        );
    }

    #[tokio::test]
    async fn query_timeline_surfaces_spawn_initial_message_on_child_identity_only() {
        let aggregator = MetaConsoleAggregator::in_memory();
        let frames = frames_from_session_history_message(
            "runtime-a",
            "review:singleton",
            "session-a",
            9,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "call-spawn-many",
                            "name": "mob_spawn_member",
                            "args": {
                                "specs": [
                                    {
                                        "agent_identity": "review-worker-alpha",
                                        "profile": "review-worker",
                                        "initial_message": "Review Initiative Alpha."
                                    },
                                    {
                                        "agent_identity": "review-worker-beta",
                                        "profile": "review-worker",
                                        "initial_message": "Review Initiative Beta."
                                    }
                                ]
                            }
                        }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.090Z"
            }),
        );
        for frame in frames {
            aggregator
                .store()
                .append_if_absent(frame)
                .await
                .expect("append projected session-history frame");
        }

        let child_page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("review-worker-alpha".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query child timeline");
        assert_eq!(child_page.frames.len(), 1);
        assert_eq!(child_page.frames[0].kind, "user_input");
        assert_eq!(
            child_page.frames[0].payload["message"]["content"],
            "Review Initiative Alpha."
        );
        assert_eq!(
            child_page.frames[0].payload["parent_identity"],
            "review:singleton"
        );

        let parent_page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("review:singleton".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query parent timeline");
        assert!(
            parent_page
                .frames
                .iter()
                .all(|frame| frame.identity == "review:singleton"),
            "child initial messages must not leak into parent timeline: {:#?}",
            parent_page.frames
        );
    }

    #[test]
    fn session_history_frames_carry_transcript_interaction_identity() {
        // meerkat 0.7.25 ask 15 addendum: persisted TranscriptMessageIdentity
        // must reach the projected frames so the console can join
        // live/history twins by id instead of content heuristics.
        let interaction = "6fa459ea-ee8a-3ca4-894e-db77e160355e";
        let run = "886313e1-3b8a-5372-9b90-0c9aee199e5d";
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            3,
            json!({
                "role": "user",
                "content": "please ack",
                "identity": { "interaction_id": interaction, "run_id": run },
                "created_at": "1970-01-01T00:00:00.100Z"
            }),
        );
        assert_eq!(frames.len(), 1, "{frames:#?}");
        assert_eq!(frames[0].interaction_id.as_deref(), Some(interaction));
        assert_eq!(frames[0].run_id.as_deref(), Some(run));

        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            4,
            json!({
                "role": "block_assistant",
                "blocks": [ { "block_type": "text", "data": { "text": "ACK" } } ],
                "identity": { "interaction_id": interaction },
                "stop_reason": "end_turn",
                "created_at": "1970-01-01T00:00:00.200Z"
            }),
        );
        assert!(!frames.is_empty(), "assistant history frame expected");
        assert_eq!(frames[0].interaction_id.as_deref(), Some(interaction));
        assert_eq!(frames[0].run_id, None);

        // Messages without a persisted identity stay unstamped (pre-0.7.25
        // transcripts deserialize with the empty default).
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            5,
            json!({
                "role": "user",
                "content": "legacy row",
                "created_at": "1970-01-01T00:00:00.300Z"
            }),
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].interaction_id, None);
    }

    /// A text-less assistant step is never a terminal. A tool-only step
    /// projects as the live edge's `tool_call_requested` frames (one per
    /// non-comms tool call, comms calls stay with the comms parity helper), a
    /// silent step projects nothing, and a step that says something keeps
    /// its `text_complete` with the authored text.
    #[test]
    fn session_history_text_less_step_never_projects_an_empty_completion() {
        let interaction = "6fa459ea-ee8a-3ca4-894e-db77e160355e";
        let run = "886313e1-3b8a-5372-9b90-0c9aee199e5d";
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            6,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": { "id": "toolu_peers_1", "name": "peers", "args": {} }
                    }
                ],
                "identity": { "interaction_id": interaction, "run_id": run },
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.400Z"
            }),
        );
        assert!(
            frames
                .iter()
                .all(|frame| frame.kind != "interaction_complete"),
            "a tool-only step must not project a terminal: {frames:#?}"
        );
        let call = frames
            .iter()
            .find(|frame| frame.kind == "tool_call_requested")
            .expect("a tool-only step projects its tool call");
        assert_eq!(call.payload["tool_call_id"], json!("toolu_peers_1"));
        assert_eq!(call.payload["name"], json!("peers"));
        assert_eq!(call.payload["args"], json!({}));
        assert_eq!(call.source.kind, ConsoleFrameSourceKind::SessionHistory);
        assert_eq!(call.interaction_id.as_deref(), Some(interaction));
        assert_eq!(call.run_id.as_deref(), Some(run));
        assert_eq!(call.timestamp_ms, 400);

        // A comms send in a tool-only step belongs to the comms parity
        // helper: exactly one tool-call frame, never two.
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            7,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "toolu_send_1",
                            "name": "send_message",
                            "args": { "peer_id": "peer-1", "body": "hi" }
                        }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.500Z"
            }),
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.kind == "tool_call_requested")
                .count(),
            1,
            "{frames:#?}"
        );
        assert!(
            frames
                .iter()
                .all(|frame| frame.kind != "interaction_complete"),
            "{frames:#?}"
        );

        // A silent step (no text, no tool calls) projects nothing at all.
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            8,
            json!({
                "role": "block_assistant",
                "blocks": [],
                "stop_reason": "end_turn",
                "created_at": "1970-01-01T00:00:00.600Z"
            }),
        );
        assert!(frames.is_empty(), "{frames:#?}");

        // Control: a step that says something keeps its authored text,
        // and its non-comms tool call stays inside that message's
        // message (the live edge already projected the call).
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "helper",
            "",
            "session-a",
            9,
            json!({
                "role": "block_assistant",
                "blocks": [
                    { "block_type": "text", "data": { "text": "Checking peers." } },
                    {
                        "block_type": "tool_use",
                        "data": { "id": "toolu_peers_2", "name": "peers", "args": {} }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.700Z"
            }),
        );
        let message = frames
            .iter()
            .find(|frame| frame.kind == "text_complete")
            .expect("a step with text keeps its authored message");
        assert_eq!(message.payload["result"], json!("Checking peers."));
        assert!(
            frames
                .iter()
                .all(|frame| frame.kind != "interaction_complete")
        );
        assert!(
            frames
                .iter()
                .all(|frame| frame.kind != "tool_call_requested"),
            "{frames:#?}"
        );
    }

    #[test]
    fn identity_first_interaction_ids_are_deterministic_uuids() {
        let a = identity_first_interaction_uuid("send:key:1");
        let b = identity_first_interaction_uuid("send:key:1");
        let c = identity_first_interaction_uuid("send:key:2");
        assert_eq!(a, b, "idempotent retries must mint the same id");
        assert_ne!(a, c);
        assert!(
            a.parse::<uuid::Uuid>().is_ok(),
            "identity-first interaction ids must be UUID-form (the console's \
             authoritative twin-identity scheme): {a}"
        );
    }

    #[test]
    fn session_history_projection_applies_namespace_to_spawned_child_identity() {
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "test/review:singleton",
            "test",
            "session-a",
            10,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "call-spawn-many",
                            "name": "mob_spawn_member",
                            "args": {
                                "specs": [
                                    {
                                        "agent_identity": "review-worker-alpha",
                                        "profile": "review-worker",
                                        "initial_message": "Review Initiative Alpha."
                                    },
                                    {
                                        "agent_identity": "test/review-worker-beta",
                                        "profile": "review-worker",
                                        "initial_message": "Review Initiative Beta."
                                    }
                                ]
                            }
                        }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.100Z"
            }),
        );

        let child_identities = frames
            .iter()
            .filter(|frame| frame.kind == "user_input")
            .map(|frame| frame.identity.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            child_identities,
            vec!["test/review-worker-alpha", "test/review-worker-beta"]
        );
    }

    #[tokio::test]
    async fn query_timeline_matches_namespaced_spawn_initial_message_identity() {
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator
            .inner
            .identity_read_model
            .replace(vec![
                identity_record_for_test("test/review:singleton"),
                identity_record_for_test("test/review-worker-alpha"),
            ])
            .await;
        let frames = frames_from_session_history_message_with_namespace(
            "runtime-a",
            "test/review:singleton",
            "test",
            "session-a",
            11,
            json!({
                "role": "block_assistant",
                "blocks": [
                    {
                        "block_type": "tool_use",
                        "data": {
                            "id": "call-spawn",
                            "name": "mob_spawn_member",
                            "args": {
                                "agent_identity": "review-worker-alpha",
                                "profile": "review-worker",
                                "initial_message": "Review Initiative Alpha."
                            }
                        }
                    }
                ],
                "stop_reason": "tool_use",
                "created_at": "1970-01-01T00:00:00.110Z"
            }),
        );
        for frame in frames {
            aggregator
                .store()
                .append_if_absent(frame)
                .await
                .expect("append projected namespaced session-history frame");
        }

        let namespaced_child_page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("test/review-worker-alpha".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query namespaced child timeline");
        assert_eq!(namespaced_child_page.frames.len(), 1);
        assert_eq!(
            namespaced_child_page.frames[0].identity,
            "test/review-worker-alpha"
        );
        assert_eq!(
            namespaced_child_page.frames[0].payload["message"]["content"],
            "Review Initiative Alpha."
        );

        let raw_child_page = aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some("review-worker-alpha".to_string()),
                limit: 20,
                ..ConsoleTimelineQuery::default()
            })
            .await
            .expect("query raw child timeline");
        assert!(
            raw_child_page.frames.is_empty(),
            "raw unnamespaced query must not see namespaced synthetic frames: {:#?}",
            raw_child_page.frames
        );
    }

    #[test]
    fn namespace_helpers_preserve_namespace_named_member_identity() {
        assert_eq!(apply_namespace("test", "test"), "test/test");
        assert_eq!(
            strip_namespace("test/test", "test").as_deref(),
            Some("test")
        );
        assert_eq!(apply_namespace("test/worker", "test"), "test/worker");
        assert_eq!(
            strip_namespace("test/worker", "test").as_deref(),
            Some("worker")
        );
        assert_eq!(strip_namespace("test", "test"), None);
    }

    #[test]
    fn generated_runtime_ids_do_not_match_sibling_colon_identities() {
        assert!(!member_id_matches_durable_identity(
            "rt:review:singleton:0",
            "review:singleton"
        ));
        assert!(!member_id_matches_durable_identity(
            "review:singleton:gen1",
            "review:singleton"
        ));
        assert!(!member_id_matches_durable_identity(
            "review:singleton:1",
            "review:singleton"
        ));
        assert!(!member_id_matches_durable_identity(
            "rt:review:singleton:qa:0",
            "review:singleton"
        ));
        assert!(!member_id_matches_durable_identity(
            "review:singleton:qa",
            "review:singleton"
        ));
    }

    #[test]
    fn session_history_projection_uses_rfc3339_created_at_timestamp() {
        let frame = frame_from_session_history_message(
            "runtime-a",
            "agent-a",
            "session-a",
            0,
            json!({
                "role": "user",
                "content": "hello",
                "created_at": "2026-05-12T05:00:06.564227Z"
            }),
        )
        .expect("user history frame");

        assert_eq!(frame.timestamp_ms, 1_778_562_006_564);
    }

    async fn wait_for_kickoff_frames(
        aggregator: &MetaConsoleAggregator,
        identity: &str,
        timeout: Duration,
    ) -> Vec<ConsoleFrame> {
        let deadline = Instant::now() + timeout;
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            let page = aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(identity.to_string()),
                    limit: 50,
                    ..ConsoleTimelineQuery::default()
                })
                .await
                .expect("query timeline");
            observed = page.frames;
            if observed.iter().any(|frame| frame.kind == "user_input") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        observed
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_registry_primes_access_lineage() {
        let runtime = build_empty_runtime("spawn-access-lineage-test").await;
        let sink = runtime
            .mob_runtime()
            .console_spawn_sink()
            .expect("unified runtime installs the console spawn sink at bootstrap");

        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "worker-3".to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("member spawns");
        sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
            mob_id: Some("spawn-access-lineage-test".to_string()),
            member_id: "worker-3".to_string(),
            identity: "worker-3".to_string(),
            initial_message: None,
            labels: BTreeMap::new(),
            spawned_by: Some("ops-lead".to_string()),
            spawned_by_session: None,
            via_tool: "mob_spawn_member".to_string(),
        })
        .await;

        let controller = crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            approvers: Default::default(),
            admins: vec!["root@example.test".to_string()],
            groups: BTreeMap::new(),
            rules: vec![crate::access::AccessRule {
                id: "bob-ops-lead".to_string(),
                subjects: vec!["bob@example.test".to_string()],
                actions: vec!["agent.view".to_string()],
                agents: vec!["ops-lead".to_string()],
                ..crate::access::AccessRule::default()
            }],
        })
        .expect("controller");

        crate::http_console::prime_access_cache_from_runtime(runtime.mob_runtime(), &controller)
            .await;

        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(
            bob.can_view_agent("worker-3"),
            "spawn registry lineage must let bob see the member ops-lead spawned"
        );
        assert!(
            !bob.can_view_agent("unrelated-agent"),
            "agents outside the granted lineage stay denied"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn roster_label_spoof_cannot_mint_or_evade_lineage() {
        let runtime = build_empty_runtime("spawn-lineage-spoof-test").await;

        // Server-side spawns with caller-controlled labels and NO spawn
        // registry entry: lineage claims in roster labels are untrusted.
        let mut imposter = SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "imposter".to_string(),
            None,
            None,
            None,
        );
        imposter.labels = Some(BTreeMap::from([(
            "spawned_by".to_string(),
            "ops-lead".to_string(),
        )]));
        runtime.spawn(imposter).await.expect("imposter spawns");

        let mut evader = SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "evader".to_string(),
            None,
            None,
            None,
        );
        evader.labels = Some(BTreeMap::from([(
            "spawned_by".to_string(),
            "secret-lead".to_string(),
        )]));
        runtime.spawn(evader).await.expect("evader spawns");

        // The console identity list must not present unverified lineage.
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "",
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        let identities = aggregator
            .list_identities_fresh()
            .await
            .expect("list identities");
        let imposter_record = identities
            .iter()
            .find(|record| record.identity == "imposter")
            .expect("imposter listed");
        assert!(
            !imposter_record.labels.contains_key("spawned_by"),
            "identity records must drop roster-claimed lineage when the spawn registry has none"
        );

        let controller = crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            approvers: Default::default(),
            admins: vec!["root@example.test".to_string()],
            groups: BTreeMap::new(),
            rules: vec![
                crate::access::AccessRule {
                    id: "bob-ops-lead".to_string(),
                    subjects: vec!["bob@example.test".to_string()],
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["ops-lead".to_string()],
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "carol-view-all".to_string(),
                    subjects: vec!["carol@example.test".to_string()],
                    actions: vec!["agent.view".to_string()],
                    agents: vec!["*".to_string()],
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "hide-secret-lead".to_string(),
                    effect: crate::access::AccessEffect::Deny,
                    actions: vec!["agent.*".to_string()],
                    agents: vec!["secret-lead".to_string()],
                    ..crate::access::AccessRule::default()
                },
            ],
        })
        .expect("controller");

        crate::http_console::prime_access_cache_from_runtime(runtime.mob_runtime(), &controller)
            .await;

        let bob = controller.view_for_subject(Some("bob@example.test"));
        assert!(
            !bob.can_view_agent("imposter"),
            "a roster-claimed spawned_by must not mint inherited visibility"
        );
        let carol = controller.view_for_subject(Some("carol@example.test"));
        assert!(
            carol.can_view_agent("evader"),
            "a roster-claimed spawned_by must not let a member hide behind a denied parent"
        );

        let _ = runtime.mob_handle().stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_spawned_member_kickoff_and_live_events_share_one_chat() {
        let runtime = build_empty_runtime("agent-spawn-console-test").await;

        // The unified runtime must hand the agent-tool spawn path a console
        // sink at bootstrap — without it, agent-spawned members stay
        // invisible until their first runtime activity.
        let sink = runtime
            .mob_runtime()
            .console_spawn_sink()
            .expect("unified runtime installs the console spawn sink at bootstrap");

        // The member exists in the roster exactly as an agent-tool spawn
        // would leave it; the server-side spawn carries no initial message
        // so the projected kickoff below is the only chat seed.
        runtime
            .spawn(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "worker-3".to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("member spawns");

        let seed = crate::console_spawn::ConsoleSpawnSeed {
            mob_id: Some("agent-spawn-console-test".to_string()),
            member_id: "worker-3".to_string(),
            identity: "worker-3".to_string(),
            initial_message: Some(json!("Kick off the person hunt")),
            labels: BTreeMap::from([("group".to_string(), "workers".to_string())]),
            spawned_by: Some("ops-lead".to_string()),
            spawned_by_session: None,
            via_tool: "mob_spawn_member".to_string(),
        };
        sink.project_spawned_member(&seed).await;
        // Spawn retries must not fork or duplicate the chat.
        sink.project_spawned_member(&seed).await;

        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "runtime-a",
            "test",
            runtime.mob_runtime().clone(),
            None,
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );

        let frames =
            wait_for_kickoff_frames(&aggregator, "test/worker-3", Duration::from_secs(5)).await;
        let kickoffs = frames
            .iter()
            .filter(|frame| frame.kind == "user_input")
            .collect::<Vec<_>>();
        assert_eq!(
            kickoffs.len(),
            1,
            "member timeline must contain exactly one kickoff frame; got {frames:#?}"
        );
        assert_eq!(
            kickoffs[0].payload["content"][0]["text"],
            "Kick off the person hunt"
        );

        // A live runtime event for the member must aggregate into the same
        // identity/chat as the kickoff, not fork into a second conversation.
        runtime
            .project_console_event_from_unified(&crate::types::EventEnvelope {
                event_id: "evt-live-worker-3".to_string(),
                source: "test".to_string(),
                timestamp_ms: current_time_ms(),
                event: crate::types::UnifiedEvent::Agent {
                    agent_id: "worker-3:0".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "on it" })),
                },
            })
            .await;

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut live_frame = None;
        while Instant::now() < deadline && live_frame.is_none() {
            let page = aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some("test/worker-3".to_string()),
                    limit: 50,
                    ..ConsoleTimelineQuery::default()
                })
                .await
                .expect("query timeline");
            live_frame = page
                .frames
                .into_iter()
                .find(|frame| frame.source_event_id.as_deref() == Some("evt-live-worker-3"));
            if live_frame.is_none() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        let live_frame = live_frame.expect("live runtime event lands in the member chat");
        assert_eq!(live_frame.identity, "test/worker-3");

        // The identity list must carry the registered spawn labels so the
        // sidebar can group the member and ABAC can see its lineage.
        let identities = aggregator
            .list_identities_fresh()
            .await
            .expect("list identities");
        let record = identities
            .iter()
            .find(|record| record.identity == "test/worker-3")
            .unwrap_or_else(|| {
                panic!("spawned member missing from identity list: {identities:#?}")
            });
        assert_eq!(
            record.labels.get("group").map(String::as_str),
            Some("workers")
        );
        assert_eq!(
            record.labels.get("spawned_by").map(String::as_str),
            Some("ops-lead")
        );

        let _ = runtime.mob_handle().stop().await;
    }
}

//! Projection layer for structural mob events.
//!
//! After PR #67 meta kept its own ring buffer and minted process-local
//! cursors via `AtomicU64`. Following the absorption of meerkat #445 the
//! single source of truth is the meerkat ledger: `MobEvent.cursor` is
//! durable, monotonic, and shared by every subscriber. This module is
//! reduced to the minimum projection seam: take a `MobEvent`, label-join
//! it with the runtime's `RuntimeMetadataTable`, and broadcast the
//! resulting `MobStructuralEventEnvelope` to in-process subscribers.
//!
//! Query and SSE paths now read the ledger directly (see
//! `UnifiedRuntime::query_mob_events` and the
//! `/meta/mob_events/stream` SSE route). The broadcast channel kept
//! here serves in-process consumers (tests, embedded controllers); each
//! external SSE client opens its own meerkat subscription so live tail
//! and catch-up share the same ordered stream.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use meerkat_mob::MobError;
use meerkat_mob::event::{AttributedEvent, MobEvent, MobEventKind};
use meerkat_mob::ids::{AgentRuntimeId, FenceToken};
use meerkat_mob::runtime::MobEventsView;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::runtime::{MetadataScope, RuntimeMetadataTable};
use crate::types::MobStructuralEventEnvelope;
use crate::unified_runtime::EventQuery;

/// Batch size used by the ledger-scanning query helpers. Matches the
/// meerkat `MobEventsSubscriptionConfig::default().batch_limit`.
pub(crate) const QUERY_BATCH_SIZE: usize = 128;

/// Default per-call result cap when the caller does not supply `limit`.
pub(crate) const DEFAULT_QUERY_LIMIT: usize = 256;

/// Capacity of the broadcast channel used by in-process subscribers.
const MOB_EVENTS_CHANNEL_CAP: usize = 512;

/// Exact generation-scoped authority carried by structural event variants
/// that persist both binding atoms. Kept as an internal sidecar so the public
/// [`MobStructuralEventEnvelope`] wire contract remains unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MobEventMemberAuthority {
    pub(crate) runtime_id: AgentRuntimeId,
    pub(crate) fence_token: FenceToken,
}

/// Internal projection used by protected console transports. The envelope is
/// still the public payload; `member_authority` is authorization metadata and
/// is deliberately never serialized.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjectedMobEvent {
    pub(crate) envelope: MobStructuralEventEnvelope,
    pub(crate) member_authority: Option<MobEventMemberAuthority>,
}

/// Result of scanning a structural-event snapshot through a caller-provided
/// selector. `resume_after_seq` is the exact raw ledger frontier represented
/// by the page, including structurally filtered or authorization-denied rows.
/// Consumers can therefore resume after it without replaying hidden rows or
/// skipping a visible event that lay beyond them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MobEventsQueryPage<T> {
    pub(crate) items: Vec<T>,
    pub(crate) resume_after_seq: u64,
}

/// Thin projection layer for structural mob events.
///
/// Public so integration tests can construct one directly. Internal
/// callers should obtain the runtime's store via
/// [`crate::unified_runtime::UnifiedRuntime::subscribe_mob_events`].
#[derive(Clone)]
pub struct MobEventsStore {
    event_tx: broadcast::Sender<MobStructuralEventEnvelope>,
    metadata_table: Option<Arc<RuntimeMetadataTable>>,
}

impl Default for MobEventsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MobEventsStore {
    /// Create an empty store with no label provider attached. Events
    /// projected through this store carry empty `mob_labels` /
    /// `run_labels`. Use [`Self::with_metadata_table`] to wire in the
    /// runtime's `RuntimeMetadataTable` so structural events are
    /// label-enriched at projection time.
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(MOB_EVENTS_CHANNEL_CAP);
        Self {
            event_tx,
            metadata_table: None,
        }
    }

    /// Wire a label provider into the store. After this, every projected
    /// structural envelope is enriched with the matching `mob_labels` and
    /// (when the event has a `run_id`) `run_labels` snapshotted at
    /// projection time. Returns the same store with the table attached so
    /// callers can chain.
    #[must_use]
    pub fn with_metadata_table(mut self, table: Arc<RuntimeMetadataTable>) -> Self {
        self.metadata_table = Some(table);
        self
    }

    /// Subscribe to live structural mob events. Each receiver sees every
    /// envelope projected after subscription. Receivers that fall behind
    /// `MOB_EVENTS_CHANNEL_CAP` will see `RecvError::Lagged`; production
    /// SSE clients should subscribe directly to the meerkat ledger via
    /// the `/meta/mob_events/stream` route instead.
    pub fn subscribe(&self) -> broadcast::Receiver<MobStructuralEventEnvelope> {
        self.event_tx.subscribe()
    }

    /// Project an [`AttributedEvent`] into a structural envelope. The
    /// `AttributedEvent` carries an agent-level `EventEnvelope`; we use it
    /// only for the agent identity and timestamp fallback. The bulk of the
    /// structural fields come from the corresponding [`MobEvent`] (see
    /// [`Self::project_mob_event`]).
    ///
    /// Returns `None` because attributed agent events on their own do not
    /// have structural mob fields — only the `MobEvent` stream does. Kept
    /// here so callers wiring both streams have a symmetric API.
    #[allow(
        unknown_lints,
        clippy::unused_async_trait_impl,
        reason = "preserve the public async projection API shared with mob-event projection"
    )]
    pub async fn project_attributed_event(
        &self,
        _event: &AttributedEvent,
    ) -> Option<MobStructuralEventEnvelope> {
        None
    }

    /// Project a [`MobEvent`] into a structural envelope and broadcast it
    /// to in-process subscribers. The envelope's `cursor` is the meerkat
    /// ledger cursor — durable across meta restarts.
    pub async fn project_mob_event(&self, event: &MobEvent) -> MobStructuralEventEnvelope {
        let envelope = self.build_envelope(event).await;
        let _ = self.event_tx.send(envelope.clone());
        envelope
    }

    /// Like [`Self::project_mob_event`] but does not broadcast. Used by
    /// the query path which scans the ledger and projects events without
    /// disturbing the live broadcast.
    pub async fn project_event_for_query(&self, event: &MobEvent) -> MobStructuralEventEnvelope {
        self.build_envelope(event).await
    }

    /// Project one ledger event together with any exact member authority it
    /// durably carries. Variants without both runtime id and fence token stay
    /// unbound and must fail closed on protected transports.
    pub(crate) async fn project_event_with_authority(&self, event: &MobEvent) -> ProjectedMobEvent {
        ProjectedMobEvent {
            envelope: self.build_envelope(event).await,
            member_authority: extract_member_authority(&event.kind),
        }
    }

    async fn build_envelope(&self, event: &MobEvent) -> MobStructuralEventEnvelope {
        let cursor = event.cursor;
        let mob_id = event.mob_id.as_str().to_string();
        let timestamp_ms = event.timestamp.timestamp_millis().max(0) as u64;
        let kind = event_kind_label(&event.kind).to_string();
        let (run_id, step_id, agent_identity) = extract_structural_fields(&event.kind);
        let data = serde_json::to_value(&event.kind).unwrap_or(Value::Null);
        let (mob_labels, run_labels) = self.lookup_labels(&mob_id, run_id.as_deref()).await;
        MobStructuralEventEnvelope {
            event_id: format!("mob-evt-{cursor}"),
            cursor,
            mob_id,
            timestamp_ms,
            kind,
            run_id,
            step_id,
            agent_identity,
            mob_labels,
            run_labels,
            data,
        }
    }

    async fn lookup_labels(
        &self,
        mob_id: &str,
        run_id: Option<&str>,
    ) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        let Some(table) = &self.metadata_table else {
            return (BTreeMap::new(), BTreeMap::new());
        };
        let mob_labels = table
            .get_labels(&MetadataScope::Mob(mob_id.to_string()))
            .await;
        let run_labels = match run_id {
            Some(run_id) => {
                table
                    .get_labels(&MetadataScope::Run(mob_id.to_string(), run_id.to_string()))
                    .await
            }
            None => BTreeMap::new(),
        };
        (mob_labels, run_labels)
    }
}

/// Path of the per-client structural-events SSE route.
pub const MOB_EVENTS_STREAM_PATH: &str = "/meta/mob_events/stream";

/// Build the continuation URL returned by `meta/mob_events/subscribe`.
///
/// `after_seq` (the cursor the SSE handler will resume from) is set to
/// `next_after_seq` if the snapshot returned events, else the
/// caller-supplied `after_seq`, else `latest_cursor` captured at
/// handshake time. This closes the gap between the JSON-RPC snapshot
/// response and the SSE handshake where new events would otherwise be
/// missed. The original filters are echoed back so the SSE client
/// applies the same predicate without restating them.
pub(crate) fn build_subscribe_url(
    query: &EventQuery,
    next_after_seq: Option<u64>,
    fallback_cursor: u64,
) -> String {
    let after_seq = next_after_seq
        .or(query.after_seq)
        .unwrap_or(fallback_cursor);
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("after_seq", &after_seq.to_string());
    if let Some(value) = query.mob_id.as_deref() {
        serializer.append_pair("mob_id", value);
    }
    if let Some(value) = query.run_id.as_deref() {
        serializer.append_pair("run_id", value);
    }
    if let Some(value) = query.step_id.as_deref() {
        serializer.append_pair("step_id", value);
    }
    if let Some(value) = query.identity.as_deref() {
        serializer.append_pair("identity", value);
    }
    if let Some(value) = query.member_id.as_deref() {
        serializer.append_pair("member_id", value);
    }
    if let Some(value) = query.since_ms {
        serializer.append_pair("since_ms", &value.to_string());
    }
    if let Some(value) = query.until_ms {
        serializer.append_pair("until_ms", &value.to_string());
    }
    if !query.event_types.is_empty() {
        serializer.append_pair("event_types", &query.event_types.join(","));
    }
    format!("{MOB_EVENTS_STREAM_PATH}?{}", serializer.finish())
}

/// Errors raised when scanning the meerkat ledger to satisfy a
/// structural-events query. `Stale` is the typed variant the JSON-RPC
/// layer maps to `-32010` with `data: { after_cursor, latest_cursor }`.
#[derive(Debug)]
pub enum MobEventsQueryError {
    /// Caller supplied an `after_seq` past the current ledger frontier.
    Stale {
        after_cursor: u64,
        latest_cursor: u64,
    },
    /// Any other failure surfaced by the meerkat events view.
    Backend(MobError),
}

impl std::fmt::Display for MobEventsQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale {
                after_cursor,
                latest_cursor,
            } => write!(
                f,
                "stale mob event cursor: requested {after_cursor}, latest {latest_cursor}"
            ),
            Self::Backend(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for MobEventsQueryError {}

impl From<MobError> for MobEventsQueryError {
    fn from(err: MobError) -> Self {
        if let MobError::StaleEventCursor {
            after_cursor,
            latest_cursor,
        } = err
        {
            Self::Stale {
                after_cursor,
                latest_cursor,
            }
        } else {
            Self::Backend(err)
        }
    }
}

/// Predicate matching a [`MobStructuralEventEnvelope`] against an
/// [`EventQuery`]'s field filters. Cursor-bound and `limit` are handled
/// by the scan loops, not by this function.
pub(crate) fn envelope_matches(envelope: &MobStructuralEventEnvelope, query: &EventQuery) -> bool {
    if let Some(since) = query.since_ms
        && envelope.timestamp_ms < since
    {
        return false;
    }
    if let Some(until) = query.until_ms
        && envelope.timestamp_ms >= until
    {
        return false;
    }
    if let Some(mob_id) = query.mob_id.as_deref()
        && envelope.mob_id != mob_id
    {
        return false;
    }
    if let Some(run_id) = query.run_id.as_deref()
        && envelope.run_id.as_deref() != Some(run_id)
    {
        return false;
    }
    if let Some(step_id) = query.step_id.as_deref()
        && envelope.step_id.as_deref() != Some(step_id)
    {
        return false;
    }
    let identity_filter = query.identity.as_deref().or(query.member_id.as_deref());
    if let Some(identity) = identity_filter
        && envelope.agent_identity.as_deref() != Some(identity)
    {
        return false;
    }
    if !query.event_types.is_empty() && !query.event_types.iter().any(|ty| ty == &envelope.kind) {
        return false;
    }
    true
}

/// Scan the ledger in batches of [`QUERY_BATCH_SIZE`], project each
/// `MobEvent` via `store`, apply `query`'s field filters, and return
/// results in cursor-ascending order.
///
/// Semantics:
/// - With `after_seq`: scan **forward** from `after_seq`; on
///   `StaleEventCursor` the typed [`MobEventsQueryError::Stale`] is
///   returned so the JSON-RPC layer can surface code `-32010`.
/// - Without `after_seq`: scan **backwards** from `latest_cursor`,
///   accumulating the latest `limit` matching events, then return them
///   in cursor-ascending order.
///
/// `limit` defaults to [`DEFAULT_QUERY_LIMIT`].
pub(crate) async fn query_ledger_with_filter(
    events: &MobEventsView,
    store: &MobEventsStore,
    query: &EventQuery,
) -> Result<Vec<MobStructuralEventEnvelope>, MobEventsQueryError> {
    Ok(
        query_ledger_with_filter_selected(events, store, query, |projection| async move {
            Some(projection.envelope)
        })
        .await?
        .items,
    )
}

/// Scan and select structural events while enforcing `limit` on the selected
/// output, not on the pre-authorization candidate set. Protected transports
/// use this seam to apply generation-bound authorization inside pagination;
/// otherwise denied rows could consume the page budget and starve later
/// visible rows.
pub(crate) async fn query_ledger_with_filter_selected<T, F, Fut>(
    events: &MobEventsView,
    store: &MobEventsStore,
    query: &EventQuery,
    mut selector: F,
) -> Result<MobEventsQueryPage<T>, MobEventsQueryError>
where
    F: FnMut(ProjectedMobEvent) -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let limit = query.limit.unwrap_or(DEFAULT_QUERY_LIMIT);
    if limit == 0 {
        let resume_after_seq = match query.after_seq {
            Some(after_seq) => after_seq,
            None => events.latest_cursor().await?,
        };
        return Ok(MobEventsQueryPage {
            items: Vec::new(),
            resume_after_seq,
        });
    }
    if let Some(after_seq) = query.after_seq {
        return scan_forward(events, store, query, after_seq, limit, &mut selector).await;
    }
    scan_backward(events, store, query, limit, &mut selector).await
}

async fn scan_forward<T, F, Fut>(
    events: &MobEventsView,
    store: &MobEventsStore,
    query: &EventQuery,
    after_seq: u64,
    limit: usize,
    selector: &mut F,
) -> Result<MobEventsQueryPage<T>, MobEventsQueryError>
where
    F: FnMut(ProjectedMobEvent) -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let mut results: Vec<T> = Vec::with_capacity(limit.min(QUERY_BATCH_SIZE));
    let mut cursor = after_seq;
    loop {
        let batch = events.poll_strict(cursor, QUERY_BATCH_SIZE).await?;
        if batch.is_empty() {
            break;
        }
        let cursor_before_batch = cursor;
        for event in batch {
            cursor = cursor.max(event.cursor);
            let projection = store.project_event_with_authority(&event).await;
            if envelope_matches(&projection.envelope, query)
                && let Some(selected) = selector(projection).await
            {
                results.push(selected);
                if results.len() >= limit {
                    return Ok(MobEventsQueryPage {
                        items: results,
                        resume_after_seq: cursor,
                    });
                }
            }
        }
        // Defensive non-progress guard. `poll_strict` is contracted to
        // return events strictly after `cursor_before_batch` when the
        // batch is non-empty, so this branch is unreachable — but
        // bailing instead of looping forever keeps the failure mode
        // bounded if the contract ever changes.
        if cursor <= cursor_before_batch {
            break;
        }
    }
    Ok(MobEventsQueryPage {
        items: results,
        resume_after_seq: cursor,
    })
}

async fn scan_backward<T, F, Fut>(
    events: &MobEventsView,
    store: &MobEventsStore,
    query: &EventQuery,
    limit: usize,
    selector: &mut F,
) -> Result<MobEventsQueryPage<T>, MobEventsQueryError>
where
    F: FnMut(ProjectedMobEvent) -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let latest = events.latest_cursor().await?;
    if latest == 0 {
        return Ok(MobEventsQueryPage {
            items: Vec::new(),
            resume_after_seq: 0,
        });
    }
    let batch_size = QUERY_BATCH_SIZE as u64;
    let mut window_end = latest;
    let mut accumulator: Vec<T> = Vec::new();
    loop {
        let from = window_end.saturating_sub(batch_size);
        let take = (window_end - from) as usize;
        if take == 0 {
            break;
        }
        let batch = events.poll_strict(from, take).await?;
        if batch.is_empty() {
            break;
        }
        let mut window_matches: Vec<T> = Vec::with_capacity(batch.len());
        for event in batch {
            let projection = store.project_event_with_authority(&event).await;
            if envelope_matches(&projection.envelope, query)
                && let Some(selected) = selector(projection).await
            {
                window_matches.push(selected);
            }
        }
        // Prepend (cursor-ascending order preserved across windows).
        let mut combined = Vec::with_capacity(window_matches.len() + accumulator.len());
        combined.append(&mut window_matches);
        combined.append(&mut accumulator);
        accumulator = combined;
        if accumulator.len() >= limit || from == 0 {
            break;
        }
        window_end = from;
    }
    if accumulator.len() > limit {
        let drop = accumulator.len() - limit;
        accumulator.drain(0..drop);
    }
    Ok(MobEventsQueryPage {
        items: accumulator,
        resume_after_seq: latest,
    })
}

fn extract_member_authority(kind: &MobEventKind) -> Option<MobEventMemberAuthority> {
    let (runtime_id, fence_token) = match kind {
        MobEventKind::MemberSpawned(event) => (&event.agent_runtime_id, event.fence_token),
        MobEventKind::MemberReset {
            agent_runtime_id,
            fence_token,
            ..
        }
        | MobEventKind::RemoteMemberRuntimeRetired {
            agent_runtime_id,
            fence_token,
            ..
        }
        | MobEventKind::RemoteMemberSupervisorRevoked {
            agent_runtime_id,
            fence_token,
            ..
        } => (agent_runtime_id, *fence_token),
        _ => return None,
    };
    Some(MobEventMemberAuthority {
        runtime_id: runtime_id.clone(),
        fence_token,
    })
}

/// Snake-case label for a `MobEventKind` matching the `serde(tag="type",
/// rename_all="snake_case")` wire form.
fn event_kind_label(kind: &MobEventKind) -> &'static str {
    match kind {
        MobEventKind::MobCreated { .. } => "mob_created",
        MobEventKind::MobDefinitionUpdated { .. } => "mob_definition_updated",
        MobEventKind::MobOwnerBridgeSessionBound { .. } => "mob_owner_bridge_session_bound",
        MobEventKind::MobCompleted => "mob_completed",
        MobEventKind::MobDestroying => "mob_destroying",
        MobEventKind::MobDestroyStorageFinalizing => "mob_destroy_storage_finalizing",
        MobEventKind::MobReset => "mob_reset",
        MobEventKind::MemberSpawned(_) => "member_spawned",
        MobEventKind::MemberSessionBindingRecovered(_) => "member_session_binding_recovered",
        MobEventKind::ForkJobTerminal(_) => "fork_job_terminal",
        MobEventKind::MemberRetirementStarted { .. } => "member_retirement_started",
        MobEventKind::MemberRetired { .. } => "member_retired",
        MobEventKind::RespawnTopologyAbandoned { .. } => "respawn_topology_abandoned",
        MobEventKind::RemoteMemberRuntimeRetired { .. } => "remote_member_runtime_retired",
        MobEventKind::RemoteMemberSupervisorRevoked { .. } => "remote_member_supervisor_revoked",
        MobEventKind::RemoteMemberReleaseConfirmed { .. } => "remote_member_release_confirmed",
        MobEventKind::MemberReset { .. } => "member_reset",
        MobEventKind::MemberKickoffUpdated { .. } => "member_kickoff_updated",
        MobEventKind::MembersWired { .. } => "members_wired",
        MobEventKind::MembersWiredBatch { .. } => "members_wired_batch",
        MobEventKind::MembersUnwired { .. } => "members_unwired",
        MobEventKind::ExternalPeerWired { .. } => "external_peer_wired",
        MobEventKind::ExternalPeerUnwired { .. } => "external_peer_unwired",
        MobEventKind::FlowStarted { .. } => "flow_started",
        MobEventKind::FlowCompleted { .. } => "flow_completed",
        MobEventKind::FlowFailed { .. } => "flow_failed",
        MobEventKind::FlowCanceled { .. } => "flow_canceled",
        MobEventKind::StepDispatched { .. } => "step_dispatched",
        MobEventKind::StepTargetCompleted { .. } => "step_target_completed",
        MobEventKind::StepTargetFailed { .. } => "step_target_failed",
        MobEventKind::RemoteTurnObligationRecorded { .. } => "remote_turn_obligation_recorded",
        MobEventKind::RemoteTurnOutcomeResolved { .. } => "remote_turn_outcome_resolved",
        MobEventKind::RemoteTurnOutcomeAcknowledged { .. } => "remote_turn_outcome_acknowledged",
        MobEventKind::RemoteTurnOutcomeDisposed { .. } => "remote_turn_outcome_disposed",
        MobEventKind::PlacedCompletionLifecycleQuiesceStarted { .. } => {
            "placed_completion_lifecycle_quiesce_started"
        }
        MobEventKind::MobStopped => "mob_stopped",
        MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. } => {
            "placed_completion_lifecycle_quiesce_ended"
        }
        MobEventKind::PlacedCompletionObligationRecorded { .. } => {
            "placed_completion_obligation_recorded"
        }
        MobEventKind::PlacedCompletionCancellationRequested { .. } => {
            "placed_completion_cancellation_requested"
        }
        MobEventKind::PlacedCompletionOutcomeResolved { .. } => {
            "placed_completion_outcome_resolved"
        }
        MobEventKind::PlacedCompletionOutcomeClosed { .. } => "placed_completion_outcome_closed",
        MobEventKind::PlacedCompletionOutcomeAcknowledged { .. } => {
            "placed_completion_outcome_acknowledged"
        }
        MobEventKind::PlacedCompletionOutcomeDisposed { .. } => {
            "placed_completion_outcome_disposed"
        }
        MobEventKind::PlacedKickoffObligationRecorded { .. } => {
            "placed_kickoff_obligation_recorded"
        }
        MobEventKind::PlacedKickoffOutcomeResolved { .. } => "placed_kickoff_outcome_resolved",
        MobEventKind::PlacedKickoffRejectedNoEffect { .. } => "placed_kickoff_rejected_no_effect",
        MobEventKind::PlacedKickoffOutcomeAcknowledged { .. } => {
            "placed_kickoff_outcome_acknowledged"
        }
        MobEventKind::PlacedKickoffOutcomeDisposed { .. } => "placed_kickoff_outcome_disposed",
        MobEventKind::RemoteHostBindStarted { .. } => "remote_host_bind_started",
        MobEventKind::RemoteHostBindConfirmed { .. } => "remote_host_bind_confirmed",
        MobEventKind::RemoteHostBindCompleted { .. } => "remote_host_bind_completed",
        MobEventKind::RemoteHostBindAbortedNoEffect { .. } => "remote_host_bind_aborted_no_effect",
        MobEventKind::RemoteHostRevokeStarted { .. } => "remote_host_revoke_started",
        MobEventKind::RemoteHostRevokeConfirmed { .. } => "remote_host_revoke_confirmed",
        MobEventKind::RemoteHostRevokeCompleted { .. } => "remote_host_revoke_completed",
        MobEventKind::StepCompleted { .. } => "step_completed",
        MobEventKind::StepFailed { .. } => "step_failed",
        MobEventKind::StepSkipped { .. } => "step_skipped",
        MobEventKind::TopologyViolation { .. } => "topology_violation",
        MobEventKind::SupervisorEscalation { .. } => "supervisor_escalation",
        MobEventKind::SupervisorEscalationFailed { .. } => "supervisor_escalation_failed",
        MobEventKind::OrchestratorResumeNotificationFailed { .. } => {
            "orchestrator_resume_notification_failed"
        }
        MobEventKind::OperatorActionRecorded { .. } => "operator_action_recorded",
        MobEventKind::ObjectiveOwnerBound { .. } => "objective_owner_bound",
        MobEventKind::ObjectiveConcluded { .. } => "objective_concluded",
    }
}

/// Decode a raw mob-roster member id into the public alias space.
///
/// For identity-first members the roster id is the comms-safe encoding
/// (`mk--rt_creview_csingleton_c0`), so every member-id slot surfaced on a
/// projection boundary MUST be decoded before it reaches a console/SDK. This
/// keeps the structural-event surface symmetric with every sibling projection
/// (the agent-event SSE path, console aggregator, etc.) which all decode.
fn decode_member_id(member_id: &str) -> String {
    crate::member_comms_id::runtime_alias_str(member_id).into_owned()
}

/// Pull `(run_id, step_id, agent_identity)` out of variants that carry
/// them. Variants without a given field return `None` for that slot.
///
/// Every `agent_identity` slot is decoded through [`decode_member_id`] so the
/// projected envelope speaks the public alias space, not the comms-safe
/// `mk--` roster encoding.
pub(crate) fn extract_structural_fields(
    kind: &MobEventKind,
) -> (Option<String>, Option<String>, Option<String>) {
    match kind {
        MobEventKind::FlowStarted { run_id, .. }
        | MobEventKind::FlowCompleted { run_id, .. }
        | MobEventKind::FlowFailed { run_id, .. }
        | MobEventKind::FlowCanceled { run_id, .. } => (Some(run_id.to_string()), None, None),
        MobEventKind::StepDispatched {
            run_id,
            step_id,
            target,
        }
        | MobEventKind::StepTargetCompleted {
            run_id,
            step_id,
            target,
            ..
        } => (
            Some(run_id.to_string()),
            Some(step_id.as_str().to_string()),
            Some(decode_member_id(target.identity.as_str())),
        ),
        MobEventKind::StepTargetFailed {
            run_id,
            step_id,
            target,
            ..
        } => (
            Some(run_id.to_string()),
            Some(step_id.as_str().to_string()),
            Some(decode_member_id(target.identity.as_str())),
        ),
        MobEventKind::StepCompleted { run_id, step_id }
        | MobEventKind::StepFailed {
            run_id, step_id, ..
        }
        | MobEventKind::StepSkipped {
            run_id, step_id, ..
        } => (
            Some(run_id.to_string()),
            Some(step_id.as_str().to_string()),
            None,
        ),
        MobEventKind::SupervisorEscalation {
            run_id,
            step_id,
            escalated_to,
        } => (
            Some(run_id.to_string()),
            Some(step_id.as_str().to_string()),
            Some(decode_member_id(escalated_to.as_str())),
        ),
        // Diagnostic only: the resume's planned notification target. Not a
        // flow coordinate.
        MobEventKind::OrchestratorResumeNotificationFailed { member_id, .. } => {
            (None, None, Some(decode_member_id(member_id.as_str())))
        }
        MobEventKind::SupervisorEscalationFailed {
            run_id, step_id, ..
        } => (
            Some(run_id.to_string()),
            Some(step_id.as_str().to_string()),
            None,
        ),
        MobEventKind::RemoteTurnObligationRecorded { obligation }
        | MobEventKind::RemoteTurnOutcomeResolved { obligation }
        | MobEventKind::RemoteTurnOutcomeAcknowledged { obligation }
        | MobEventKind::RemoteTurnOutcomeDisposed { obligation } => (
            Some(obligation.run_id.to_string()),
            Some(obligation.step_id.as_str().to_string()),
            Some(decode_member_id(obligation.agent_identity.as_str())),
        ),
        MobEventKind::PlacedCompletionObligationRecorded { obligation }
        | MobEventKind::PlacedCompletionCancellationRequested { obligation }
        | MobEventKind::PlacedCompletionOutcomeResolved { obligation, .. }
        | MobEventKind::PlacedCompletionOutcomeClosed { obligation, .. }
        | MobEventKind::PlacedCompletionOutcomeAcknowledged { obligation }
        | MobEventKind::PlacedCompletionOutcomeDisposed { obligation } => (
            None,
            None,
            Some(decode_member_id(obligation.agent_identity.as_str())),
        ),
        MobEventKind::PlacedKickoffObligationRecorded { obligation }
        | MobEventKind::PlacedKickoffOutcomeResolved { obligation, .. }
        | MobEventKind::PlacedKickoffRejectedNoEffect { obligation, .. }
        | MobEventKind::PlacedKickoffOutcomeAcknowledged { obligation }
        | MobEventKind::PlacedKickoffOutcomeDisposed { obligation } => (
            None,
            None,
            Some(decode_member_id(obligation.agent_identity.as_str())),
        ),
        MobEventKind::MemberSpawned(event) => (
            None,
            None,
            Some(decode_member_id(event.agent_identity.as_str())),
        ),
        // Crash-recovery rebind fact for a member; attribute it to that member
        // so console/SSE projection keys it under the right identity.
        MobEventKind::MemberSessionBindingRecovered(event) => (
            None,
            None,
            Some(decode_member_id(event.agent_identity.as_str())),
        ),
        // The job was run by the child. Its owner's session and job id are
        // not flow coordinates or a different member identity.
        MobEventKind::ForkJobTerminal(event) => {
            (None, None, Some(decode_member_id(event.child.as_str())))
        }
        MobEventKind::MemberRetirementStarted { agent_identity, .. }
        | MobEventKind::MemberRetired { agent_identity, .. }
        | MobEventKind::RespawnTopologyAbandoned { agent_identity, .. }
        | MobEventKind::MemberReset { agent_identity, .. }
        | MobEventKind::RemoteMemberRuntimeRetired { agent_identity, .. }
        | MobEventKind::RemoteMemberSupervisorRevoked { agent_identity, .. }
        | MobEventKind::RemoteMemberReleaseConfirmed { agent_identity, .. } => {
            (None, None, Some(decode_member_id(agent_identity.as_str())))
        }
        MobEventKind::MemberKickoffUpdated { member, .. } => {
            (None, None, Some(decode_member_id(member.as_str())))
        }
        // Objective lifecycle facts attribute to the bound/concluding member
        // (ask 28: durable kickoff objective-to-outcome correlation).
        MobEventKind::ObjectiveOwnerBound { owner, .. } => {
            (None, None, Some(decode_member_id(owner.as_str())))
        }
        MobEventKind::ObjectiveConcluded { member, .. } => {
            (None, None, Some(decode_member_id(member.as_str())))
        }
        MobEventKind::ExternalPeerWired { local, .. }
        | MobEventKind::ExternalPeerUnwired { local, .. } => {
            (None, None, Some(decode_member_id(local.as_str())))
        }
        // Definition-epoch replacement: carries the new definition and epoch,
        // none of the structural (run, step, member) coordinates.
        MobEventKind::MobDefinitionUpdated { .. } => (None, None, None),
        // MobOwnerBridgeSessionBound is mob-scoped (owner bridge binding):
        // it carries no run/step/member structural fields.
        MobEventKind::MobCreated { .. }
        | MobEventKind::MobOwnerBridgeSessionBound { .. }
        | MobEventKind::MobCompleted
        | MobEventKind::MobDestroying
        | MobEventKind::MobDestroyStorageFinalizing
        | MobEventKind::MobReset
        | MobEventKind::PlacedCompletionLifecycleQuiesceStarted { .. }
        | MobEventKind::MobStopped
        | MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. }
        | MobEventKind::RemoteHostBindStarted { .. }
        | MobEventKind::RemoteHostBindConfirmed { .. }
        | MobEventKind::RemoteHostBindCompleted { .. }
        | MobEventKind::RemoteHostBindAbortedNoEffect { .. }
        | MobEventKind::RemoteHostRevokeStarted { .. }
        | MobEventKind::RemoteHostRevokeConfirmed { .. }
        | MobEventKind::RemoteHostRevokeCompleted { .. }
        | MobEventKind::MembersWired { .. }
        | MobEventKind::MembersWiredBatch { .. }
        | MobEventKind::MembersUnwired { .. }
        | MobEventKind::TopologyViolation { .. }
        | MobEventKind::OperatorActionRecorded { .. } => (None, None, None),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_mob::event::{MemberSpawnedEvent, MemberWireEdge};
    use meerkat_mob::ids::{
        AgentIdentity, AgentRuntimeId, FenceToken, FlowId, Generation, MobId, ProfileName, RunId,
        StepId,
    };

    fn mob_event(cursor: u64, kind: MobEventKind) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: MobId::from("test-mob"),
            kind,
        }
    }

    /// `event_kind_label`'s doc claims it matches the serde wire form
    /// (`tag = "type", rename_all = "snake_case"`), and nothing verified that
    /// claim - the exhaustive match catches a MISSING arm at compile time, but
    /// a mislabeled one ships silently and desynchronizes the console/SDK
    /// event surface from the wire. Serde itself is the oracle here: serialize
    /// the variant, read its `type` tag, compare. Covers the new
    /// `MobDefinitionUpdated` and `ForkJobTerminal` arms plus unit controls.
    #[test]
    fn event_kind_label_matches_the_serde_wire_tag() {
        let fork_outcome = serde_json::json!({"text": "complete"});
        let cases = vec![
            MobEventKind::MobDefinitionUpdated {
                epoch: 2,
                definition: Box::new(meerkat_mob::MobDefinition::explicit("label-oracle")),
            },
            MobEventKind::MobCreated {
                definition: Box::new(meerkat_mob::MobDefinition::explicit("label-oracle")),
            },
            MobEventKind::ForkJobTerminal(meerkat_mob::ForkJobTerminalEvent {
                job_id: "fork-label-oracle".to_string(),
                child: AgentIdentity::from("fork-child"),
                owner_session_id: meerkat_core::SessionId::new(),
                retained_work: None,
                status: meerkat_core::event::BackgroundJobTerminalStatus::Completed,
                result_digest: meerkat_mob::detached_outcome_digest(&fork_outcome),
                outcome: fork_outcome,
            }),
            MobEventKind::OrchestratorResumeNotificationFailed {
                member_id: AgentIdentity::from("orchestrator"),
                session_id: meerkat_core::SessionId::new(),
                reason: "delivery refused".to_string(),
            },
            MobEventKind::MobCompleted,
            MobEventKind::MobReset,
        ];
        for kind in cases {
            let wire = serde_json::to_value(&kind).expect("event kind serializes");
            let tag = wire
                .get("type")
                .and_then(serde_json::Value::as_str)
                .expect("serde form carries a type tag");
            assert_eq!(
                event_kind_label(&kind),
                tag,
                "label diverged from the serde wire tag for {kind:?}"
            );
        }
    }

    /// The definition-epoch event carries no run/step/member coordinates; a
    /// wrong extraction here would attach epoch changes to a phantom member or
    /// run in every console timeline.
    #[test]
    fn definition_updated_extracts_no_structural_fields() {
        let kind = MobEventKind::MobDefinitionUpdated {
            epoch: 2,
            definition: Box::new(meerkat_mob::MobDefinition::explicit("fields-oracle")),
        };
        assert_eq!(extract_structural_fields(&kind), (None, None, None));
    }

    /// The resume-notification failure names its planned target member, in
    /// the public alias space, and no flow coordinates.
    #[test]
    fn orchestrator_resume_notification_failure_extracts_its_member() {
        let alias = "rt:orchestrator:singleton:0";
        let encoded = crate::member_comms_id::mob_member_id_str(alias).into_owned();
        let kind = MobEventKind::OrchestratorResumeNotificationFailed {
            member_id: AgentIdentity::from(encoded.as_str()),
            session_id: meerkat_core::SessionId::new(),
            reason: "delivery refused".to_string(),
        };
        assert_eq!(
            extract_structural_fields(&kind),
            (None, None, Some(alias.to_string()))
        );
    }

    #[test]
    fn fork_terminal_projects_the_child_without_inventing_a_flow_run() {
        let kind = MobEventKind::ForkJobTerminal(meerkat_mob::event::ForkJobTerminalEvent {
            job_id: "fork-job".into(),
            child: AgentIdentity::from("mk--rt_creview_csingleton_c0"),
            owner_session_id: meerkat_core::SessionId::new(),
            retained_work: None,
            status: meerkat_core::event::BackgroundJobTerminalStatus::Completed,
            outcome: serde_json::json!({"text": "done"}),
            result_digest: "retained-digest".into(),
        });
        let wire = serde_json::to_value(&kind).expect("fork terminal serializes");
        assert_eq!(wire["type"], event_kind_label(&kind));
        assert_eq!(
            extract_structural_fields(&kind),
            (None, None, Some("rt:review:singleton:0".to_string()))
        );
    }

    #[tokio::test]
    async fn projects_fork_terminal_for_child_without_inventing_authority() {
        let alias = "rt:fork-child:singleton:0";
        let encoded = crate::member_comms_id::mob_member_id_str(alias).into_owned();
        assert_ne!(encoded, alias);
        let outcome = serde_json::json!({"text": "child result"});
        let kind = MobEventKind::ForkJobTerminal(meerkat_mob::ForkJobTerminalEvent {
            job_id: "fork-projection-job".to_string(),
            child: AgentIdentity::from(encoded.as_str()),
            owner_session_id: meerkat_core::SessionId::new(),
            retained_work: None,
            status: meerkat_core::event::BackgroundJobTerminalStatus::Completed,
            result_digest: meerkat_mob::detached_outcome_digest(&outcome),
            outcome,
        });
        let data = serde_json::to_value(&kind).expect("fork terminal serializes");
        let projected = MobEventsStore::new()
            .project_event_with_authority(&mob_event(12, kind))
            .await;

        assert_eq!(projected.envelope.kind, "fork_job_terminal");
        assert_eq!(projected.envelope.agent_identity.as_deref(), Some(alias));
        assert_eq!(projected.envelope.run_id, None);
        assert_eq!(projected.envelope.step_id, None);
        assert_eq!(projected.member_authority, None);
        assert_eq!(projected.envelope.data, data);
    }

    #[tokio::test]
    async fn projects_flow_started_with_run_id_and_upstream_cursor() {
        let store = MobEventsStore::new();
        let run_id = RunId::new();
        let envelope = store
            .project_mob_event(&mob_event(
                42,
                MobEventKind::FlowStarted {
                    run_id: run_id.clone(),
                    flow_id: FlowId::from("flow-a"),
                    params: serde_json::json!({}),
                },
            ))
            .await;
        assert_eq!(envelope.kind, "flow_started");
        assert_eq!(envelope.cursor, 42);
        assert_eq!(envelope.event_id, "mob-evt-42");
        assert_eq!(
            envelope.run_id.as_deref(),
            Some(run_id.to_string().as_str())
        );
        assert_eq!(envelope.step_id, None);
        assert_eq!(envelope.mob_id, "test-mob");
    }

    #[tokio::test]
    async fn projects_step_dispatched_with_run_step_target() {
        let store = MobEventsStore::new();
        let identity = AgentIdentity::from("worker-1");
        let run_id = RunId::new();
        let envelope = store
            .project_mob_event(&mob_event(
                7,
                MobEventKind::StepDispatched {
                    run_id: run_id.clone(),
                    step_id: StepId::from("step-a"),
                    target: AgentRuntimeId::initial(identity),
                },
            ))
            .await;
        assert_eq!(envelope.kind, "step_dispatched");
        assert_eq!(envelope.cursor, 7);
        assert_eq!(
            envelope.run_id.as_deref(),
            Some(run_id.to_string().as_str())
        );
        assert_eq!(envelope.step_id.as_deref(), Some("step-a"));
        assert_eq!(envelope.agent_identity.as_deref(), Some("worker-1"));
    }

    #[tokio::test]
    async fn projects_member_spawned_with_identity() {
        let store = MobEventsStore::new();
        let identity = AgentIdentity::from("researcher");
        let envelope = store
            .project_mob_event(&mob_event(
                3,
                MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    identity.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    AgentRuntimeId::initial(identity),
                    ProfileName::from("worker"),
                )),
            ))
            .await;
        assert_eq!(envelope.kind, "member_spawned");
        assert_eq!(envelope.agent_identity.as_deref(), Some("researcher"));
    }

    #[tokio::test]
    async fn internal_projection_binds_spawn_to_exact_runtime_and_fence() {
        let store = MobEventsStore::new();
        let identity = AgentIdentity::from("generation-bound");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let fence_token = FenceToken::new(41);
        let event = mob_event(
            9,
            MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                identity,
                Generation::INITIAL,
                fence_token,
                runtime_id.clone(),
                ProfileName::from("worker"),
            )),
        );

        let public = store.project_event_for_query(&event).await;
        let projected = store.project_event_with_authority(&event).await;

        assert_eq!(projected.envelope, public);
        assert_eq!(
            projected.member_authority,
            Some(MobEventMemberAuthority {
                runtime_id,
                fence_token,
            })
        );
        let wire = serde_json::to_value(&projected.envelope).expect("public envelope wire value");
        assert!(
            wire.get("member_authority").is_none(),
            "internal authority must not alter the public envelope contract"
        );
    }

    #[tokio::test]
    async fn internal_projection_leaves_fenceless_agent_event_unbound() {
        let store = MobEventsStore::new();
        let identity = AgentIdentity::from("fenceless-target");
        let projected = store
            .project_event_with_authority(&mob_event(
                10,
                MobEventKind::StepDispatched {
                    run_id: RunId::new(),
                    step_id: StepId::from("step-a"),
                    target: AgentRuntimeId::initial(identity),
                },
            ))
            .await;

        assert_eq!(
            projected.envelope.agent_identity.as_deref(),
            Some("fenceless-target")
        );
        assert_eq!(projected.member_authority, None);
    }

    #[tokio::test]
    async fn projects_identity_first_member_in_public_alias_space_and_filters_round_trip() {
        // Identity-first members carry the comms-safe ENCODED roster id on the
        // raw MobEvent (`mk--rt_creview_csingleton_c0`). The structural surface
        // is a projection boundary, so the envelope's `agent_identity` MUST be
        // decoded back to the public alias and a client filter keyed by that
        // alias MUST match. Regression for the `mk--` leak / dropped-filter bug.
        let alias = "rt:review:singleton:0";
        let encoded = crate::member_comms_id::mob_member_id_str(alias).into_owned();
        assert_ne!(encoded, alias, "alias must actually encode for this test");
        let identity = AgentIdentity::from(encoded.as_str());

        for kind in [
            MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                identity.clone(),
                Generation::INITIAL,
                FenceToken::new(1),
                AgentRuntimeId::initial(identity.clone()),
                ProfileName::from("review"),
            )),
            MobEventKind::MemberRetired {
                agent_identity: identity.clone(),
                generation: Generation::INITIAL,
                role: ProfileName::from("review"),
            },
            MobEventKind::StepDispatched {
                run_id: RunId::new(),
                step_id: StepId::from("step-a"),
                target: AgentRuntimeId::initial(identity.clone()),
            },
        ] {
            let store = MobEventsStore::new();
            let envelope = store.project_mob_event(&mob_event(1, kind)).await;

            // The public surface speaks the alias, never the `mk--` encoding.
            assert_eq!(
                envelope.agent_identity.as_deref(),
                Some(alias),
                "structural envelope must decode the roster id to the alias"
            );
            assert!(
                !envelope
                    .agent_identity
                    .as_deref()
                    .unwrap()
                    .starts_with("mk--"),
                "encoded mk-- id must not leak onto the public surface"
            );

            // A client filtering by the public alias (identity or member_id)
            // matches; the encoded form does not (it is never client-visible).
            let query = EventQuery {
                identity: Some(alias.to_string()),
                ..EventQuery::default()
            };
            assert!(
                envelope_matches(&envelope, &query),
                "identity-filter by the public alias must match"
            );
            let member_query = EventQuery {
                member_id: Some(alias.to_string()),
                ..EventQuery::default()
            };
            assert!(
                envelope_matches(&envelope, &member_query),
                "member_id-filter by the public alias must match"
            );
        }
    }

    #[tokio::test]
    async fn projects_members_wired_batch_as_compact_structural_event() {
        let store = MobEventsStore::new();
        let envelope = store
            .project_mob_event(&mob_event(
                8,
                MobEventKind::MembersWiredBatch {
                    edges: vec![MemberWireEdge {
                        a: AgentIdentity::from("alpha"),
                        b: AgentIdentity::from("beta"),
                    }],
                },
            ))
            .await;
        assert_eq!(envelope.kind, "members_wired_batch");
        assert_eq!(envelope.agent_identity, None);
        assert_eq!(envelope.data["edges"][0]["a"], serde_json::json!("alpha"));
        assert_eq!(envelope.data["edges"][0]["b"], serde_json::json!("beta"));
    }

    #[tokio::test]
    async fn project_event_for_query_does_not_broadcast() {
        let store = MobEventsStore::new();
        let mut rx = store.subscribe();
        let _ = store
            .project_event_for_query(&mob_event(
                1,
                MobEventKind::FlowStarted {
                    run_id: RunId::new(),
                    flow_id: FlowId::from("flow-a"),
                    params: serde_json::json!({}),
                },
            ))
            .await;
        // The query-projection variant is silent; the broadcast channel
        // should not receive anything.
        assert!(rx.try_recv().is_err());
    }
}

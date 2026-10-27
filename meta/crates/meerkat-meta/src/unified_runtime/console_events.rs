use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::console_contracts::{
    ALL_EVENTS_STREAM_NAME, ConsoleIdentityEventEnvelope, ReplayUnavailableError,
    SYSTEM_EVENT_IDENTITY,
};
use crate::types::{EventEnvelope, UnifiedEvent};

const IDENTITY_REPLAY_CAP: usize = 1024;
const ALL_EVENTS_REPLAY_CAP: usize = 4096;
const EVENT_CHANNEL_BASE_CAP: usize = 4096;
const EVENT_CHANNEL_CAP_PER_MEMBER: usize = 128;
const EVENT_CHANNEL_DEFAULT_MEMBER_BUDGET: usize = 256;
const EVENT_CHANNEL_MAX_CAP: usize = 65_536;
pub(crate) const PENDING_INTERACTION_CAP: usize = 256;

pub(crate) fn event_channel_capacity_for_members(member_count: usize) -> usize {
    EVENT_CHANNEL_BASE_CAP
        .saturating_add(member_count.saturating_mul(EVENT_CHANNEL_CAP_PER_MEMBER))
        .clamp(EVENT_CHANNEL_BASE_CAP, EVENT_CHANNEL_MAX_CAP)
}

#[derive(Clone)]
pub(crate) struct ConsoleEventStore {
    next_event_seq: Arc<AtomicU64>,
    state: Arc<RwLock<ConsoleEventReplayState>>,
    projection_lock: Arc<Mutex<()>>,
    event_tx: broadcast::Sender<ConsoleIdentityEventEnvelope>,
    /// The content-addressed store session persistence keeps images in.
    /// Live tool-result images are written there and carried by reference,
    /// as history carries them.
    blob_store: Arc<OnceLock<Arc<dyn meerkat_core::BlobStore>>>,
}

struct ConsoleEventReplayState {
    all_events: VecDeque<ConsoleIdentityEventEnvelope>,
    by_identity: BTreeMap<String, VecDeque<ConsoleIdentityEventEnvelope>>,
    pending_by_identity: BTreeMap<String, VecDeque<PendingInteraction>>,
    // Legacy content-match association, retained only for old boundaries.
    active_interaction_by_identity: BTreeMap<String, String>,
    // Observed runtime ownership is independent of a console reservation.
    active_run_by_identity: BTreeMap<String, meerkat_core::types::TranscriptMessageIdentity>,
    observed_runs_by_identity: BTreeMap<String, VecDeque<meerkat_core::lifecycle::RunId>>,
    callback_pending_by_identity: BTreeSet<String>,
    runtime_to_identity: BTreeMap<String, String>,
    response_phase_by_identity: BTreeMap<String, Option<String>>,
    /// Console identity metadata registered by spawn paths (spawn labels,
    /// `spawned_by` lineage, `via_tool`). Merged into identity projections
    /// and access-control attribute priming; member/roster labels win on
    /// conflict.
    labels_by_identity: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
struct PendingInteraction {
    interaction_id: String,
    origin: String,
    content: Value,
}

impl ConsoleEventReplayState {
    fn resolve_identity_for_runtime_event(&self, runtime_event_id: &str) -> Option<String> {
        if let Some(identity) = self.runtime_to_identity.get(runtime_event_id) {
            return Some(identity.clone());
        }

        self.runtime_to_identity
            .iter()
            .filter_map(|(runtime_member_id, identity)| {
                runtime_event_id
                    .strip_prefix(runtime_member_id)
                    .filter(|suffix| suffix.starts_with(':'))
                    .map(|_| (runtime_member_id.len(), identity.clone()))
            })
            .max_by_key(|(prefix_len, _)| *prefix_len)
            .map(|(_, identity)| identity)
    }
}

impl ConsoleEventStore {
    pub(crate) fn new() -> Self {
        Self::with_member_capacity(EVENT_CHANNEL_DEFAULT_MEMBER_BUDGET)
    }

    pub(crate) fn with_member_capacity(member_count: usize) -> Self {
        let (event_tx, _) = broadcast::channel(event_channel_capacity_for_members(member_count));
        let bootstrap = ConsoleIdentityEventEnvelope {
            event_id: "console-evt-1".to_string(),
            interaction_id: None,
            identity: SYSTEM_EVENT_IDENTITY.to_string(),
            event_type: "runtime_bootstrapped".to_string(),
            timestamp_ms: current_time_ms(),
            data: json!({
                "source": "unified_runtime",
            }),
        };
        let mut by_identity = BTreeMap::new();
        by_identity.insert(
            bootstrap.identity.clone(),
            VecDeque::from([bootstrap.clone()]),
        );
        Self {
            next_event_seq: Arc::new(AtomicU64::new(2)),
            projection_lock: Arc::new(Mutex::new(())),
            state: Arc::new(RwLock::new(ConsoleEventReplayState {
                all_events: VecDeque::from([bootstrap]),
                by_identity,
                pending_by_identity: BTreeMap::new(),
                active_interaction_by_identity: BTreeMap::new(),
                active_run_by_identity: BTreeMap::new(),
                observed_runs_by_identity: BTreeMap::new(),
                callback_pending_by_identity: BTreeSet::new(),
                runtime_to_identity: BTreeMap::new(),
                response_phase_by_identity: BTreeMap::new(),
                labels_by_identity: BTreeMap::new(),
            })),
            event_tx,
            blob_store: Arc::new(OnceLock::new()),
        }
    }

    /// Install the blob store live tool-result images are externalized into.
    /// The first installation wins; without one, images stay inline.
    pub(crate) fn install_blob_store(&self, store: Arc<dyn meerkat_core::BlobStore>) {
        let _ = self.blob_store.set(store);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<ConsoleIdentityEventEnvelope> {
        self.event_tx.subscribe()
    }

    pub(crate) async fn append(
        &self,
        identity: impl Into<String>,
        interaction_id: Option<String>,
        event_type: impl Into<String>,
        data: Value,
    ) -> ConsoleIdentityEventEnvelope {
        let envelope = self.next_envelope(identity.into(), interaction_id, event_type.into(), data);
        self.append_envelope(envelope.clone()).await;
        envelope
    }

    /// Append one of the runtime's own events (an identity-less memory
    /// event) under the system identity.
    pub(crate) async fn append_runtime_plane_event(
        &self,
        event_type: impl Into<String>,
        data: Value,
    ) -> ConsoleIdentityEventEnvelope {
        let envelope = self.next_envelope(
            SYSTEM_EVENT_IDENTITY.to_string(),
            None,
            event_type.into(),
            data,
        );
        self.store_envelope(envelope.clone()).await;
        envelope
    }

    fn next_envelope(
        &self,
        identity: String,
        interaction_id: Option<String>,
        event_type: String,
        data: Value,
    ) -> ConsoleIdentityEventEnvelope {
        ConsoleIdentityEventEnvelope {
            event_id: format!(
                "console-evt-{}",
                self.next_event_seq.fetch_add(1, Ordering::Relaxed)
            ),
            interaction_id,
            identity,
            event_type,
            timestamp_ms: current_time_ms(),
            data,
        }
    }

    /// Append an event attributed to `envelope.identity`. A runtime-plane
    /// identity is refused: it names the runtime's own events, which only
    /// [`Self::append_runtime_plane_event`] writes. Every member-attributed
    /// writer of this store ends here (agent events and their images, spawn
    /// kickoffs, lifecycle records, interaction failures and evictions,
    /// member memory events), so no member event reaches the console under
    /// one.
    pub(crate) async fn append_envelope(&self, envelope: ConsoleIdentityEventEnvelope) -> bool {
        if crate::console_contracts::is_runtime_plane_identity(&envelope.identity) {
            crate::console_contracts::warn_runtime_plane_member_drop(
                &envelope.identity,
                None,
                &envelope.identity,
                "console event store",
            );
            return false;
        }
        self.store_envelope(envelope).await
    }

    async fn store_envelope(&self, envelope: ConsoleIdentityEventEnvelope) -> bool {
        {
            let mut state = self.state.write().await;
            if state
                .all_events
                .iter()
                .any(|existing| existing.event_id == envelope.event_id)
            {
                return false;
            }
            state.all_events.push_back(envelope.clone());
            trim_deque(&mut state.all_events, ALL_EVENTS_REPLAY_CAP);

            let replay = state
                .by_identity
                .entry(envelope.identity.clone())
                .or_insert_with(VecDeque::new);
            replay.push_back(envelope.clone());
            trim_deque(replay, IDENTITY_REPLAY_CAP);
        }
        let _ = self.event_tx.send(envelope);
        true
    }

    pub(crate) async fn register_runtime_identity(
        &self,
        runtime_member_id: impl Into<String>,
        identity: impl Into<String>,
    ) {
        let runtime_member_id = runtime_member_id.into();
        let identity = identity.into();
        if runtime_member_id.trim().is_empty() || identity.trim().is_empty() {
            return;
        }
        let mut state = self.state.write().await;
        state
            .runtime_to_identity
            .insert(runtime_member_id, identity);
    }

    /// Register a runtime-id → identity mapping only when no mapping exists
    /// for that key yet. Used by roster-refresh paths (reconcile) whose
    /// alias self-mapping is a fallback: it must never clobber the durable
    /// console identity that spawn/reserve paths registered for the same
    /// runtime member id.
    pub(crate) async fn register_runtime_identity_fallback(
        &self,
        runtime_member_id: impl Into<String>,
        identity: impl Into<String>,
    ) {
        let runtime_member_id = runtime_member_id.into();
        let identity = identity.into();
        if runtime_member_id.trim().is_empty() || identity.trim().is_empty() {
            return;
        }
        let mut state = self.state.write().await;
        state
            .runtime_to_identity
            .entry(runtime_member_id)
            .or_insert(identity);
    }

    /// Register console identity metadata (spawn labels, lineage). Merges
    /// per key; the latest registration wins so a respawn can update labels.
    pub(crate) async fn register_identity_labels(
        &self,
        identity: impl Into<String>,
        labels: BTreeMap<String, String>,
    ) {
        let identity = identity.into();
        if identity.trim().is_empty() || labels.is_empty() {
            return;
        }
        let mut state = self.state.write().await;
        state
            .labels_by_identity
            .entry(identity)
            .or_default()
            .extend(labels);
    }

    /// Remove one registered console metadata key for an identity, e.g. a
    /// stale `spawned_by` after a respawn whose spawner is unknown.
    pub(crate) async fn unregister_identity_label(&self, identity: &str, key: &str) {
        let mut state = self.state.write().await;
        if let Some(labels) = state.labels_by_identity.get_mut(identity) {
            labels.remove(key);
            if labels.is_empty() {
                state.labels_by_identity.remove(identity);
            }
        }
    }

    /// Registered console metadata for one identity, if any.
    pub(crate) async fn identity_labels(&self, identity: &str) -> Option<BTreeMap<String, String>> {
        self.state
            .read()
            .await
            .labels_by_identity
            .get(identity)
            .cloned()
    }

    /// Snapshot of all registered console identity metadata.
    pub(crate) async fn identity_labels_snapshot(
        &self,
    ) -> BTreeMap<String, BTreeMap<String, String>> {
        self.state.read().await.labels_by_identity.clone()
    }

    pub(crate) async fn replay_all(
        &self,
        last_event_id: Option<&str>,
    ) -> Result<Vec<ConsoleIdentityEventEnvelope>, ReplayUnavailableError> {
        let state = self.state.read().await;
        replay_slice(
            state.all_events.clone(),
            last_event_id,
            ALL_EVENTS_STREAM_NAME,
            state.all_events.back().map(|event| event.event_id.clone()),
        )
    }

    /// The newest buffered event's id: a position for [`Self::replay_all`].
    pub(crate) async fn latest_event_id(&self) -> Option<String> {
        self.state
            .read()
            .await
            .all_events
            .back()
            .map(|event| event.event_id.clone())
    }

    pub(crate) async fn reserve_interaction_value(
        &self,
        identity: &str,
        runtime_member_id: Option<&str>,
        interaction_id: &str,
        origin: &str,
        content: Value,
    ) -> Result<(), &'static str> {
        let evicted = {
            let mut state = self.state.write().await;
            push_pending_interaction(
                &mut state,
                identity,
                runtime_member_id,
                interaction_id,
                origin,
                content,
            )
        };
        self.report_evicted_interaction(identity, evicted).await;
        Ok(())
    }

    /// Reserve a caller-supplied interaction id, refusing one that is still
    /// in flight for this identity: queued, the active interaction, or the
    /// interaction of the run currently tracked. Two in-flight reservations
    /// under one id would make every later frame and terminal of either run
    /// ambiguous. The check and the reservation share one state lock.
    pub(crate) async fn reserve_caller_interaction_value(
        &self,
        identity: &str,
        runtime_member_id: Option<&str>,
        interaction_id: &str,
        origin: &str,
        content: Value,
    ) -> Result<(), InteractionIdInFlight> {
        let evicted = {
            let mut state = self.state.write().await;
            if interaction_in_flight(&state, identity, interaction_id) {
                return Err(InteractionIdInFlight);
            }
            push_pending_interaction(
                &mut state,
                identity,
                runtime_member_id,
                interaction_id,
                origin,
                content,
            )
        };
        self.report_evicted_interaction(identity, evicted).await;
        Ok(())
    }

    async fn report_evicted_interaction(
        &self,
        identity: &str,
        evicted: Option<PendingInteraction>,
    ) {
        if let Some(evicted) = evicted {
            tracing::warn!(
                identity = %identity,
                interaction_id = %evicted.interaction_id,
                "evicting stalled pending interaction: per-identity queue at cap"
            );
            self.append(
                identity,
                Some(evicted.interaction_id),
                "interaction_failed",
                json!({
                    "reason": "queue_overflow",
                    "origin": evicted.origin,
                    "content": evicted.content,
                }),
            )
            .await;
        }
    }

    pub(crate) async fn record_lifecycle(&self, identity: &str, event_type: &str, data: Value) {
        let _projection_guard = self.projection_lock.lock().await;
        let failed = {
            let mut state = self.state.write().await;
            let pending = state
                .pending_by_identity
                .remove(identity)
                .unwrap_or_default();
            state.active_interaction_by_identity.remove(identity);
            state.active_run_by_identity.remove(identity);
            state.callback_pending_by_identity.remove(identity);
            state
                .response_phase_by_identity
                .insert(identity.to_string(), None);
            pending.into_iter().collect::<Vec<_>>()
        };
        for pending in failed {
            self.append(
                identity,
                Some(pending.interaction_id),
                "interaction_failed",
                json!({
                    "reason": "lifecycle_mutation",
                    "origin": pending.origin,
                    "content": pending.content,
                    "lifecycle_event": event_type,
                }),
            )
            .await;
        }
        self.append(identity, None, event_type, data).await;
    }

    /// Terminalize one exact pending interaction without classifying the
    /// failure itself as an identity lifecycle mutation.
    pub(crate) async fn record_interaction_failure(
        &self,
        identity: &str,
        interaction_id: &str,
        data: Value,
    ) {
        let _projection_guard = self.projection_lock.lock().await;
        {
            let mut state = self.state.write().await;
            close_console_interaction(&mut state, identity, interaction_id);
        }
        self.append(
            identity,
            Some(interaction_id.to_string()),
            "interaction_failed",
            data,
        )
        .await;
    }

    pub(crate) async fn project_unified_event(&self, event: &EventEnvelope<UnifiedEvent>) {
        let UnifiedEvent::Agent {
            agent_id,
            event_type,
            payload,
        } = &event.event
        else {
            return;
        };
        if is_empty_web_search_annotations_event(event_type, payload.as_ref()) {
            return;
        }

        // Keep boundary association, append, and terminal settlement in event
        // order even when separate forwarding tasks share this store.
        let _projection_guard = self.projection_lock.lock().await;
        let mut projected_data = payload.clone().unwrap_or_else(|| json!({}));
        if let Some(object) = projected_data.as_object_mut() {
            object
                .entry("source_event_type".to_string())
                .or_insert_with(|| Value::String(event_type.clone()));
        }
        if matches!(
            event_type.as_str(),
            "tool_result_received" | "tool_execution_completed"
        ) && let Some(store) = self.blob_store.get()
        {
            externalize_tool_result_images(store.as_ref(), &mut projected_data).await;
        }

        let (identity, interaction_id, current_run_event) = {
            let mut state = self.state.write().await;
            // Replayed boundaries must not mutate the current run association.
            if state
                .all_events
                .iter()
                .any(|existing| existing.event_id == event.event_id)
            {
                return;
            }
            let registered_identity = state.resolve_identity_for_runtime_event(agent_id);
            let identity = registered_identity
                .clone()
                .or_else(|| derive_identity_from_runtime_id(agent_id));
            let Some(identity) = identity else {
                tracing::warn!(
                    agent_id = %agent_id,
                    event_type = %event_type,
                    "dropping agent event: runtime id did not resolve to a registered identity"
                );
                return;
            };
            // No member's events take a runtime-plane identity. Every
            // creation path refuses such a member; this drops the events of
            // one created before they did.
            if crate::console_contracts::is_runtime_plane_identity(&identity) {
                crate::console_contracts::warn_runtime_plane_member_drop(
                    agent_id
                        .rsplit_once(':')
                        .map_or(agent_id.as_str(), |(member_id, _)| member_id),
                    Some(agent_id),
                    &identity,
                    "agent events",
                );
                return;
            }
            // Cache only registration-backed resolutions. A heuristic
            // derive guess must stay repairable: caching it would make the
            // exact-match lookup permanently shadow a later
            // `register_runtime_identity` (spawn/reserve/reconcile), so a
            // single early event would strand every later interaction.
            if registered_identity.is_some() {
                state
                    .runtime_to_identity
                    .entry(agent_id.clone())
                    .or_insert_with(|| identity.clone());
            }
            let explicit_lineage = payload_transcript_identity(&projected_data);
            let interaction_id = match event_type.as_str() {
                "run_started" => {
                    select_interaction_for_run_started(&mut state, &identity, &projected_data)
                }
                "run_completed" | "run_failed" => explicit_lineage
                    .as_ref()
                    .and_then(lineage_interaction_id)
                    .or_else(|| {
                        if !has_lineage_carrier(&projected_data)
                            && !state.active_run_by_identity.contains_key(&identity)
                        {
                            state.active_interaction_by_identity.get(&identity).cloned()
                        } else {
                            None
                        }
                    }),
                "interaction_complete" | "interaction_failed" | "interaction_callback_pending" => {
                    select_interaction_for_directed_terminal(&state, &identity, &projected_data)
                }
                _ => explicit_lineage
                    .as_ref()
                    .and_then(lineage_interaction_id)
                    .or_else(|| {
                        if !has_lineage_carrier(&projected_data)
                            && event_inherits_run_lineage(event_type)
                        {
                            state
                                .active_run_by_identity
                                .get(&identity)
                                .and_then(lineage_interaction_id)
                                .or_else(|| {
                                    state.active_interaction_by_identity.get(&identity).cloned()
                                })
                        } else {
                            None
                        }
                    }),
            };
            let projected_lineage = explicit_lineage.or_else(|| {
                if has_lineage_carrier(&projected_data) {
                    return None;
                }
                let active = state.active_run_by_identity.get(&identity)?;
                if event_inherits_run_lineage(event_type)
                    || (matches!(
                        event_type.as_str(),
                        "interaction_complete"
                            | "interaction_failed"
                            | "interaction_callback_pending"
                    ) && interaction_id.is_some()
                        && interaction_id == lineage_interaction_id(active))
                {
                    Some(active.clone())
                } else {
                    None
                }
            });
            if let Some(lineage) = projected_lineage.as_ref()
                && let Some(object) = projected_data.as_object_mut()
            {
                object.insert("identity".into(), json!(lineage));
                if let Some(run_id) = lineage.run_id.as_ref() {
                    object.insert("run_id".into(), json!(run_id));
                } else {
                    object.remove("run_id");
                }
            }
            let current_run_event = match state.active_run_by_identity.get(&identity) {
                Some(active) => projected_lineage
                    .as_ref()
                    .is_some_and(|lineage| same_runtime_run(active, lineage)),
                None => true,
            };
            (identity, interaction_id, current_run_event)
        };

        let projected_type = match event_type.as_str() {
            "run_completed" => "interaction_complete",
            "run_failed" => "interaction_failed",
            other => other,
        };

        let inserted = self
            .append_envelope(ConsoleIdentityEventEnvelope {
                event_id: event.event_id.clone(),
                interaction_id: interaction_id.clone(),
                identity: identity.clone(),
                event_type: projected_type.to_string(),
                timestamp_ms: event.timestamp_ms,
                data: projected_data.clone(),
            })
            .await;
        if !inserted {
            return;
        }

        // A generated image is projected once per tool call, from the result
        // event: it alone carries the result body. The execution fact for the
        // same call carries none, and an older payload of it that still does
        // must not project the images a second time.
        if event_type.as_str() == "tool_result_received"
            && let Some(image_result) = parse_generate_image_tool_result(&projected_data)
        {
            for (idx, image) in image_result.images.iter().enumerate() {
                self.append_envelope(ConsoleIdentityEventEnvelope {
                    event_id: format!("{}#assistant_image:{idx}", event.event_id),
                    interaction_id: interaction_id.clone(),
                    identity: identity.clone(),
                    event_type: "assistant_image".to_string(),
                    timestamp_ms: event.timestamp_ms,
                    data: json!({
                        "source_event_type": event_type,
                        "identity": projected_data.get("identity"),
                        "run_id": projected_data.get("run_id"),
                        "tool_call_id": projected_data.get("id").cloned().unwrap_or(Value::Null),
                        "image_id": image.image_id.0.to_string(),
                        "blob_id": image.blob_ref.blob_id,
                        "media_type": image.media_type.as_str(),
                        "width": image.width,
                        "height": image.height,
                        "revised_prompt": image_result.revised_prompt.clone(),
                    }),
                })
                .await;
            }
        }

        let terminal_turn_completed = is_terminal_turn_completed_event(event_type, &projected_data);
        {
            let mut state = self.state.write().await;
            match event_type.as_str() {
                "tool_call_requested" | "tool_call" | "tool_result_received"
                    if current_run_event =>
                {
                    state
                        .response_phase_by_identity
                        .insert(identity.clone(), Some("tool-executing".to_string()));
                }
                "text_delta" | "reasoning_delta" if current_run_event => {
                    state
                        .response_phase_by_identity
                        .insert(identity.clone(), Some("generating".to_string()));
                }
                "run_completed" | "run_failed" => {
                    let terminal = payload_transcript_identity(&projected_data);
                    let closes_current = match state.active_run_by_identity.get(&identity) {
                        Some(active) => terminal
                            .as_ref()
                            .is_some_and(|lineage| same_runtime_run(active, lineage)),
                        None => !has_lineage_carrier(&projected_data),
                    };
                    if closes_current {
                        state.active_run_by_identity.remove(&identity);
                        if let Some(interaction_id) = interaction_id.as_deref() {
                            close_console_interaction(&mut state, &identity, interaction_id);
                        } else {
                            state.active_interaction_by_identity.remove(&identity);
                            state.callback_pending_by_identity.remove(&identity);
                            state
                                .response_phase_by_identity
                                .insert(identity.clone(), None);
                        }
                    } else if terminal.is_some()
                        && let Some(interaction_id) = interaction_id.as_deref()
                        && !state
                            .active_run_by_identity
                            .get(&identity)
                            .is_some_and(|active| {
                                lineage_interaction_id(active).as_deref() == Some(interaction_id)
                            })
                    {
                        // An exact foreign/older terminal can settle only its
                        // own reservation, never another current run.
                        close_console_interaction(&mut state, &identity, interaction_id);
                    } else if interaction_id.is_none()
                        && terminal
                            .as_ref()
                            .is_some_and(|lineage| lineage.run_id.is_some())
                        && !state.active_run_by_identity.contains_key(&identity)
                        && !state.active_interaction_by_identity.contains_key(&identity)
                    {
                        // A typed terminal whose start was never observed
                        // (missed run_started, interaction-less peer/flow/
                        // schedule run) names no reservation to settle. With
                        // no tracked run and no active interaction there is no
                        // current owner it could close, so the phase that run
                        // left behind is stale. Pending reservations remain
                        // queued until their own typed terminals arrive. A
                        // callback pause that run left behind is just as
                        // stale: no current owner can resume it.
                        state.callback_pending_by_identity.remove(&identity);
                        state
                            .response_phase_by_identity
                            .insert(identity.clone(), None);
                    }
                }
                "interaction_callback_pending" => {
                    if current_run_event
                        && interaction_id.as_ref().is_some_and(|interaction| {
                            state.active_interaction_by_identity.get(&identity) == Some(interaction)
                                || state
                                    .active_run_by_identity
                                    .get(&identity)
                                    .and_then(lineage_interaction_id)
                                    .as_ref()
                                    == Some(interaction)
                        })
                    {
                        state.callback_pending_by_identity.insert(identity.clone());
                    }
                }
                "interaction_complete" | "interaction_failed" => {
                    let explicit_run = payload_transcript_identity(&projected_data)
                        .and_then(|lineage| lineage.run_id);
                    if (explicit_run.is_none() || current_run_event)
                        && let Some(interaction_id) = interaction_id.as_deref()
                    {
                        close_console_interaction(&mut state, &identity, interaction_id);
                    }
                }
                "turn_completed" if terminal_turn_completed && current_run_event => {
                    state
                        .response_phase_by_identity
                        .insert(identity.clone(), None);
                }
                "stream_truncated" => {
                    // A typed gap in the member's live stream: whether the
                    // current run ended inside it is unknown, so events after
                    // it must not borrow that run's lineage. Explicit carriers
                    // (the next run_started, a terminal's identity) still
                    // attribute exactly.
                    state.active_run_by_identity.remove(&identity);
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn response_phase_for_identity(&self, identity: &str) -> Option<String> {
        self.state
            .read()
            .await
            .response_phase_by_identity
            .get(identity)
            .cloned()
            .flatten()
    }

    #[cfg(test)]
    pub(crate) async fn pending_interaction_ids(&self, identity: &str) -> Vec<String> {
        self.state
            .read()
            .await
            .pending_by_identity
            .get(identity)
            .map(|queue| {
                queue
                    .iter()
                    .map(|pending| pending.interaction_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Snapshot known activity under one read lock. A present null phase is
    /// known quiet; an absent identity has no recorded activity observation.
    pub(crate) async fn response_phases_snapshot(&self) -> HashMap<String, Option<String>> {
        self.state
            .read()
            .await
            .response_phase_by_identity
            .iter()
            .map(|(identity, phase)| (identity.clone(), phase.clone()))
            .collect()
    }
}

fn payload_transcript_identity(
    payload: &Value,
) -> Option<meerkat_core::types::TranscriptMessageIdentity> {
    payload
        .get("identity")
        .cloned()
        .and_then(|value| {
            serde_json::from_value::<meerkat_core::types::TranscriptMessageIdentity>(value).ok()
        })
        .filter(|identity| !identity.is_empty())
}

fn has_lineage_carrier(payload: &Value) -> bool {
    payload.get("identity").is_some_and(|value| {
        !value.is_null() && value.as_object().is_none_or(|value| !value.is_empty())
    })
}

fn lineage_interaction_id(
    identity: &meerkat_core::types::TranscriptMessageIdentity,
) -> Option<String> {
    identity.interaction_id.map(|id| id.to_string())
}

fn same_runtime_run(
    a: &meerkat_core::types::TranscriptMessageIdentity,
    b: &meerkat_core::types::TranscriptMessageIdentity,
) -> bool {
    a.run_id.is_some() && a.run_id == b.run_id && a.interaction_id == b.interaction_id
}

/// Replace the inline image blocks of a tool result's `content` with
/// references into `store`, as session history carries them. Ids are
/// content-addressed, so the live frames and the persisted transcript name
/// the same blob, but every call still puts it (an object and metadata write
/// on a persistent store, under the projection lock). Content that is not a
/// canonical block list, or a failed put, leaves the payload as it was.
async fn externalize_tool_result_images(store: &dyn meerkat_core::BlobStore, payload: &mut Value) {
    let Some(content) = payload.get_mut("content") else {
        return;
    };
    let has_inline_image = content.as_array().is_some_and(|blocks| {
        blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("image")
                && block.get("source").and_then(Value::as_str) == Some("inline")
        })
    });
    if !has_inline_image {
        return;
    }
    let Ok(mut blocks) = serde_json::from_value::<Vec<meerkat_core::ContentBlock>>(content.clone())
    else {
        return;
    };
    if let Err(error) = meerkat_core::externalize_content_blocks(store, &mut blocks).await {
        tracing::warn!(%error, "console tool-result image kept inline: blob store put failed");
        return;
    }
    if let Ok(externalized) = serde_json::to_value(&blocks) {
        *content = externalized;
    }
}

fn event_inherits_run_lineage(kind: &str) -> bool {
    // Images can be committed callback output from an earlier suspended run.
    // Peer-ingestion notices and admission/hooks likewise do not establish the
    // owner of execution. Preserve an explicit carrier, never guess for them.
    matches!(
        kind,
        "turn_started"
            | "turn_completed"
            | "text_delta"
            | "text_complete"
            | "reasoning_delta"
            | "reasoning_complete"
            | "server_tool_content"
            | "tool_call_requested"
            | "tool_call"
            | "tool_result_received"
            | "tool_execution_started"
            | "tool_execution_completed"
            | "tool_execution_timed_out"
            | "extraction_succeeded"
            | "extraction_failed"
            | "compaction_started"
            | "compaction_completed"
            | "compaction_failed"
            | "budget_warning"
            | "retrying"
    )
}

fn select_interaction_for_run_started(
    state: &mut ConsoleEventReplayState,
    identity: &str,
    payload: &Value,
) -> Option<String> {
    if let Some(lineage) = payload_transcript_identity(payload) {
        let interaction_id = lineage_interaction_id(&lineage);
        if let Some(run_id) = lineage.run_id.as_ref() {
            let seen = state
                .observed_runs_by_identity
                .entry(identity.to_string())
                .or_default();
            if seen.contains(run_id) {
                return interaction_id;
            }
            seen.push_back(run_id.clone());
            trim_deque(seen, IDENTITY_REPLAY_CAP);
            state
                .active_run_by_identity
                .insert(identity.to_string(), lineage);
        } else {
            state.active_run_by_identity.remove(identity);
        }
        state.active_interaction_by_identity.remove(identity);
        state.callback_pending_by_identity.remove(identity);
        return interaction_id;
    }
    // Only old identity-free starts use the historical content contract.
    // An unknown new start must not lend the preceding typed run to output.
    state.active_run_by_identity.remove(identity);
    if has_lineage_carrier(payload) {
        state.active_interaction_by_identity.remove(identity);
        state.callback_pending_by_identity.remove(identity);
        return None;
    }
    let input = payload
        .get("input")
        .cloned()
        .and_then(|input| serde_json::from_value::<meerkat_core::RunInput>(input).ok());
    if matches!(input, Some(meerkat_core::RunInput::PendingToolResults))
        && state.callback_pending_by_identity.remove(identity)
    {
        return state.active_interaction_by_identity.get(identity).cloned();
    }

    // Each content start establishes its own association. Unrelated runs and
    // unassociated continuations preserve pending sends without borrowing one.
    state.active_interaction_by_identity.remove(identity);
    state.callback_pending_by_identity.remove(identity);
    let queue = state.pending_by_identity.get(identity)?;
    let mut matches = queue
        .iter()
        .filter(|pending| pending_matches_run_started(pending, payload));
    let interaction_id = matches.next()?.interaction_id.clone();
    // This wire has no general realizing interaction id. Identical inputs are
    // ambiguous; their exact directed terminal may still settle the reservation.
    if matches.next().is_some() {
        return None;
    }
    state
        .active_interaction_by_identity
        .insert(identity.to_string(), interaction_id.clone());
    Some(interaction_id)
}

/// Directed runtime terminals carry their own interaction identity, including
/// peer/flow interactions with no console reservation. Legacy non-UUID fixture
/// ids remain accepted only when the console actually reserved them.
fn select_interaction_for_directed_terminal(
    state: &ConsoleEventReplayState,
    identity: &str,
    payload: &Value,
) -> Option<String> {
    let directed = payload.get("interaction_id").and_then(Value::as_str)?;
    if has_lineage_carrier(payload)
        && payload_transcript_identity(payload)
            .and_then(|lineage| lineage_interaction_id(&lineage))
            .as_deref()
            != Some(directed)
    {
        return None;
    }
    let runtime_owned = uuid::Uuid::parse_str(directed).is_ok();
    let reserved = state
        .active_interaction_by_identity
        .get(identity)
        .is_some_and(|active| active == directed)
        || state
            .pending_by_identity
            .get(identity)
            .is_some_and(|queue| {
                queue
                    .iter()
                    .any(|pending| pending.interaction_id == directed)
            });
    (runtime_owned || reserved).then(|| directed.to_string())
}

/// A caller-supplied interaction id is already in flight for the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InteractionIdInFlight;

fn interaction_in_flight(
    state: &ConsoleEventReplayState,
    identity: &str,
    interaction_id: &str,
) -> bool {
    state
        .pending_by_identity
        .get(identity)
        .is_some_and(|queue| {
            queue
                .iter()
                .any(|pending| pending.interaction_id == interaction_id)
        })
        || state
            .active_interaction_by_identity
            .get(identity)
            .is_some_and(|active| active == interaction_id)
        || state
            .active_run_by_identity
            .get(identity)
            .and_then(lineage_interaction_id)
            .as_deref()
            == Some(interaction_id)
}

/// Queue one reservation. If projection later fails to resolve an identity
/// (e.g. runtime id format changes), stale pending entries can accumulate.
/// Rather than reject new interactions once the per-identity cap is hit,
/// which would deadlock legitimate traffic behind orphans, evict the oldest
/// entry; the caller surfaces an `interaction_failed` event for it so the
/// client stops waiting.
fn push_pending_interaction(
    state: &mut ConsoleEventReplayState,
    identity: &str,
    runtime_member_id: Option<&str>,
    interaction_id: &str,
    origin: &str,
    content: Value,
) -> Option<PendingInteraction> {
    let queue = state
        .pending_by_identity
        .entry(identity.to_string())
        .or_default();
    let evicted = if queue.len() >= PENDING_INTERACTION_CAP {
        queue.pop_front()
    } else {
        None
    };
    queue.push_back(PendingInteraction {
        interaction_id: interaction_id.to_string(),
        origin: origin.to_string(),
        content,
    });
    if let Some(runtime_member_id) = runtime_member_id.filter(|value| !value.trim().is_empty()) {
        state
            .runtime_to_identity
            .insert(runtime_member_id.to_string(), identity.to_string());
    }
    if !state.active_run_by_identity.contains_key(identity)
        && !state.active_interaction_by_identity.contains_key(identity)
    {
        state
            .response_phase_by_identity
            .insert(identity.to_string(), Some("waiting".to_string()));
    }
    evicted
}

/// Drop one console interaction from the identity's pending queue and, when
/// it is the active one, from the active slot: the identity is idle once the
/// interaction it was serving reached a terminal.
fn close_console_interaction(
    state: &mut ConsoleEventReplayState,
    identity: &str,
    interaction_id: &str,
) {
    if state
        .active_run_by_identity
        .get(identity)
        .and_then(lineage_interaction_id)
        .as_deref()
        == Some(interaction_id)
    {
        state.active_run_by_identity.remove(identity);
        state.callback_pending_by_identity.remove(identity);
    }
    if let Some(queue) = state.pending_by_identity.get_mut(identity) {
        if let Some(position) = queue
            .iter()
            .position(|pending| pending.interaction_id == interaction_id)
        {
            queue.remove(position);
        }
        if queue.is_empty() {
            state.pending_by_identity.remove(identity);
        }
    }
    if state
        .active_interaction_by_identity
        .get(identity)
        .is_some_and(|active| active == interaction_id)
    {
        state.active_interaction_by_identity.remove(identity);
        state.callback_pending_by_identity.remove(identity);
    }
    // An exact terminal may settle a queued reservation while another run is
    // generating or paused for a callback. Only clear idle/own-run state.
    if !state.active_interaction_by_identity.contains_key(identity)
        && !state.active_run_by_identity.contains_key(identity)
    {
        state
            .response_phase_by_identity
            .insert(identity.to_string(), None);
    }
}

/// Match the canonical model input, including every multimodal block. The
/// runtime owns projection and inline-image/blob equivalence; text equality
/// alone would incorrectly bind different images or drop significant spacing.
fn pending_matches_run_started(pending: &PendingInteraction, payload: &Value) -> bool {
    let Some(input) = payload.get("input") else {
        return false;
    };
    let Ok(meerkat_core::RunInput::Content { content }) = serde_json::from_value(input.clone())
    else {
        return false;
    };
    let Ok(pending_content) = serde_json::from_value(pending.content.clone()) else {
        return false;
    };
    match (
        canonical_run_content_digest(pending_content),
        canonical_run_content_digest(content),
    ) {
        (Some(pending), Some(actual)) => pending == actual,
        _ => false,
    }
}

fn canonical_run_content_digest(content: meerkat_core::ContentInput) -> Option<String> {
    use meerkat_core::lifecycle::run_primitive::{
        ConversationAppend, ConversationAppendRole, CoreRenderable,
        model_projection_content_input_from_conversation_appends,
    };
    let content = match content {
        meerkat_core::ContentInput::Text(text) => CoreRenderable::Text { text },
        meerkat_core::ContentInput::Blocks(blocks) => CoreRenderable::Blocks { blocks },
    };
    let canonical =
        model_projection_content_input_from_conversation_appends(&[ConversationAppend {
            runtime_source: None,
            role: ConversationAppendRole::User,
            content,
            identity: None,
        }]);
    meerkat_runtime::input::run_started_content_digest(&canonical).ok()
}

/// Whether a `turn_completed` event ends its turn.
///
/// meerkat publishes `turn_completed` with `stop_reason: tool_use` after every
/// tool-loop model call, not only at the end of the turn, so the event kind
/// alone does not prove terminality. Only the typed `stop_reason` decides:
/// `tool_use` means the turn continues with tool results; any other reason
/// ends it. A payload without a decodable stop reason is terminal, which keeps
/// the older one-event-per-turn shape working. This is the same rule as the
/// console's `isTerminalTurnCompletedData`.
pub(crate) fn is_terminal_turn_completed_event(event_type: &str, payload: &Value) -> bool {
    event_type == "turn_completed" && !payload_stop_reason_is_tool_use(payload)
}

/// Whether a payload carries the typed stop reason `tool_use`, the model
/// stopping to run tools inside a turn that continues.
pub(crate) fn payload_stop_reason_is_tool_use(payload: &Value) -> bool {
    use serde::Deserialize as _;

    payload
        .get("stop_reason")
        .or_else(|| payload.get("stopReason"))
        .and_then(|value| meerkat_core::StopReason::deserialize(value).ok())
        == Some(meerkat_core::StopReason::ToolUse)
}

fn text_from_content_block(value: &Value) -> Option<&str> {
    value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
}

fn parse_generate_image_tool_result(
    payload: &Value,
) -> Option<meerkat_core::image_generation::ImageGenerationToolResult> {
    if payload.get("name").and_then(Value::as_str) != Some("generate_image") {
        return None;
    }
    if let Some(result_value) = payload.get("result") {
        return if let Some(result_text) = result_value.as_str() {
            serde_json::from_str(result_text).ok()
        } else {
            serde_json::from_value(result_value.clone()).ok()
        };
    }
    // Meta's ingest edges derive the flat `result` on the result event; fall
    // back to its typed `content` blocks so a raw result payload without it
    // still projects image frames.
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(text_from_content_block)
        .collect::<String>();
    serde_json::from_str(&text).ok()
}

pub(crate) fn is_empty_web_search_annotations_event(
    event_type: &str,
    payload: Option<&Value>,
) -> bool {
    if event_type != "server_tool_content" {
        return false;
    }
    let Some(payload) = payload else {
        return false;
    };
    payload.get("name").and_then(Value::as_str) == Some("web_search_annotations")
        && payload
            .get("content")
            .and_then(|content| content.get("type"))
            .and_then(Value::as_str)
            == Some("message_annotations")
        && payload
            .get("content")
            .and_then(|content| content.get("annotations"))
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
}

fn trim_deque<T>(deque: &mut VecDeque<T>, cap: usize) {
    while deque.len() > cap {
        deque.pop_front();
    }
}

fn replay_slice(
    events: VecDeque<ConsoleIdentityEventEnvelope>,
    last_event_id: Option<&str>,
    stream: &str,
    latest_event_id: Option<String>,
) -> Result<Vec<ConsoleIdentityEventEnvelope>, ReplayUnavailableError> {
    let Some(last_event_id) = last_event_id.filter(|value| !value.trim().is_empty()) else {
        // Fresh connection (no Last-Event-ID): return all buffered events
        // so new subscribers see the conversation so far.
        return Ok(events.into_iter().collect());
    };
    let mut replay = events.into_iter().collect::<Vec<_>>();
    let Some(start_idx) = replay
        .iter()
        .position(|event| event.event_id == last_event_id)
    else {
        return Err(ReplayUnavailableError {
            error: "replay_unavailable".to_string(),
            stream: stream.to_string(),
            requested_last_event_id: last_event_id.to_string(),
            latest_event_id: latest_event_id.unwrap_or_default(),
        });
    };
    // Inclusive replay: include the checkpoint event so clients can
    // verify continuity and deduplicate by event_id.
    Ok(replay.split_off(start_idx))
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// Strip a runtime-generation suffix from a runtime id, returning the
/// durable identity. Real agent events from meerkat-mob 0.6 use
/// `{identity}:{N}` (`AgentRuntimeId`'s Display form).
///
/// Identities themselves often contain colons (e.g. `personal:alice@example.com`),
/// so we only strip the LAST colon-delimited segment and only when that
/// segment parses as a generation suffix. If the format changes, this
/// returns `None` and the caller must fall back to explicit
/// runtime-to-identity registration instead of guessing.
fn derive_identity_from_runtime_id(runtime_id: &str) -> Option<String> {
    let (identity, suffix) = runtime_id.rsplit_once(':')?;
    if identity.is_empty() || suffix.is_empty() {
        return None;
    }
    if !suffix.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let identity = identity.strip_prefix("rt:").unwrap_or(identity);
    if identity.is_empty() {
        return None;
    }
    Some(identity.to_string())
}

/// The identity this store attributes the agent events of `runtime_id` to
/// when no spawn registration names it: the runtime id projected into the
/// public alias space (as every agent event's source is), then
/// [`derive_identity_from_runtime_id`]. The runtime-plane member checks
/// evaluate a member with this same code.
pub(crate) fn attributed_identity_for_runtime(
    runtime_id: &meerkat_mob::ids::AgentRuntimeId,
) -> Option<String> {
    derive_identity_from_runtime_id(&crate::member_comms_id::runtime_event_alias(runtime_id))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn turn_completed_terminality_reads_the_typed_stop_reason() {
        // Newer meerkat emits a `tool_use` turn_completed after each tool-loop call.
        assert!(!is_terminal_turn_completed_event(
            "turn_completed",
            &json!({ "type": "turn_completed", "stop_reason": "tool_use" })
        ));
        assert!(!is_terminal_turn_completed_event(
            "turn_completed",
            &json!({ "stopReason": "tool_use" })
        ));
        // Every other typed stop reason ends the turn.
        for stop_reason in [
            "end_turn",
            "max_tokens",
            "stop_sequence",
            "content_filter",
            "cancelled",
        ] {
            assert!(
                is_terminal_turn_completed_event(
                    "turn_completed",
                    &json!({ "type": "turn_completed", "stop_reason": stop_reason })
                ),
                "{stop_reason} must be terminal"
            );
        }
        // A payload without a decodable stop reason keeps the older
        // one-event-per-turn shape terminal.
        assert!(is_terminal_turn_completed_event(
            "turn_completed",
            &json!({})
        ));
        assert!(is_terminal_turn_completed_event(
            "turn_completed",
            &json!({ "stop_reason": 7 })
        ));
        // Only `turn_completed` is judged here.
        assert!(!is_terminal_turn_completed_event(
            "text_complete",
            &json!({ "stop_reason": "end_turn" })
        ));
    }

    #[tokio::test]
    async fn replay_all_retains_latest_4096_events() {
        let store = ConsoleEventStore::new();
        for idx in 0..(ALL_EVENTS_REPLAY_CAP + 8) {
            store
                .append(
                    "identity:luka",
                    Some("turn-1".to_string()),
                    "text_delta",
                    json!({ "idx": idx }),
                )
                .await;
        }

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        assert_eq!(replay.len(), ALL_EVENTS_REPLAY_CAP);
        assert_eq!(
            replay.first().and_then(|event| event.data["idx"].as_u64()),
            Some(8)
        );
    }

    #[tokio::test]
    async fn response_phases_snapshot_matches_per_identity_lookups() {
        let store = ConsoleEventStore::new();
        // "waiting": reserved interaction, no runtime frames yet.
        store
            .reserve_interaction_value("waiting-worker", None, "turn-1", "console", json!({}))
            .await
            .expect("reserve waiting interaction");
        // "generating": runtime text frame after reservation.
        store
            .register_runtime_identity("rt:generating:1", "generating-worker")
            .await;
        store
            .reserve_interaction_value(
                "generating-worker",
                Some("rt:generating:1"),
                "turn-2",
                "console",
                json!({}),
            )
            .await
            .expect("reserve generating interaction");
        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-gen".to_string(),
                source: "test".to_string(),
                timestamp_ms: 1,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:generating:1".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "working" })),
                },
            })
            .await;
        // `None`: lifecycle event clears the phase but keeps the identity key.
        store
            .record_lifecycle("idle-worker", "member_retired", json!({}))
            .await;

        let snapshot = store.response_phases_snapshot().await;

        assert_eq!(
            snapshot
                .get("waiting-worker")
                .and_then(|phase| phase.as_deref()),
            Some("waiting")
        );
        assert_eq!(
            snapshot
                .get("generating-worker")
                .and_then(|phase| phase.as_deref()),
            Some("generating")
        );
        assert_eq!(snapshot.get("idle-worker"), Some(&None));
        assert_eq!(snapshot.get("unknown"), None);
        assert_eq!(snapshot.len(), 3);
        for identity in [
            "waiting-worker",
            "generating-worker",
            "idle-worker",
            "unknown",
        ] {
            assert_eq!(
                snapshot.get(identity).cloned().flatten(),
                store.response_phase_for_identity(identity).await,
                "snapshot diverges from per-identity lookup for {identity}"
            );
        }
    }

    #[tokio::test]
    async fn terminal_turn_completed_clears_phase_without_stealing_run_correlation() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "turn-1",
                "console",
                json!("console prompt"),
            )
            .await
            .expect("reserve first interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "console prompt"}}),
            ))
            .await;

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-1".to_string(),
                source: "test".to_string(),
                timestamp_ms: 1,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "working" })),
                },
            })
            .await;
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some("generating")
        );

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-2".to_string(),
                source: "test".to_string(),
                timestamp_ms: 2,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "turn_completed".to_string(),
                    payload: Some(json!({ "stop_reason": "max_tokens" })),
                },
            })
            .await;
        assert_eq!(store.response_phase_for_identity("worker").await, None);

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-3".to_string(),
                source: "test".to_string(),
                timestamp_ms: 3,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "run_completed".to_string(),
                    payload: Some(json!({ "result": "done" })),
                },
            })
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let run_completed = replay
            .iter()
            .find(|event| event.event_id == "evt-3")
            .expect("run completion should be replayed");
        assert_eq!(run_completed.interaction_id.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn duplicate_unified_event_ids_are_projected_once() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "turn-1",
                "console",
                json!("console prompt"),
            )
            .await
            .expect("reserve interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "console prompt"}}),
            ))
            .await;

        let event = EventEnvelope {
            event_id: "evt-duplicate".to_string(),
            source: "test".to_string(),
            timestamp_ms: 1,
            event: UnifiedEvent::Agent {
                agent_id: "rt:worker:1".to_string(),
                event_type: "run_completed".to_string(),
                payload: Some(json!({ "result": "done" })),
            },
        };
        store.project_unified_event(&event).await;
        store.project_unified_event(&event).await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let projected = replay
            .iter()
            .filter(|event| event.event_id == "evt-duplicate")
            .collect::<Vec<_>>();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].interaction_id.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn exact_delivery_failure_preserves_error_and_other_pending_interactions() {
        let store = ConsoleEventStore::new();
        for interaction_id in ["turn-1", "turn-2"] {
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    interaction_id,
                    "console",
                    json!({ "interaction": interaction_id }),
                )
                .await
                .expect("reserve interaction");
        }

        store
            .record_interaction_failure(
                "worker",
                "turn-1",
                json!({ "error": "typed delivery failure" }),
            )
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let failure = replay
            .iter()
            .find(|event| event.interaction_id.as_deref() == Some("turn-1"))
            .expect("failed interaction should be projected");
        assert_eq!(failure.event_type, "interaction_failed");
        assert_eq!(failure.data["error"], "typed delivery failure");
        assert!(
            store
                .state
                .read()
                .await
                .pending_by_identity
                .get("worker")
                .is_some_and(|pending| pending
                    .iter()
                    .any(|entry| entry.interaction_id == "turn-2")),
            "an unrelated queued interaction must remain pending"
        );
    }

    async fn assert_pending_terminal_preserves_active_run(terminal_kind: &str, callback: bool) {
        let store = ConsoleEventStore::new();
        for (interaction, content) in [("pending-a", "first"), ("active-b", "second")] {
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    interaction,
                    "console",
                    json!(content),
                )
                .await
                .expect("reserve");
        }
        store
            .project_unified_event(&agent_event_with_payload(
                "active-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "second"}}),
            ))
            .await;
        let expected_phase = if callback {
            "tool-executing"
        } else {
            "generating"
        };
        let (event_type, payload) = if callback {
            (
                "tool_call_requested",
                json!({"id": "call-b", "name": "host_tool"}),
            )
        } else {
            ("text_delta", json!({"delta": "still working"}))
        };
        store
            .project_unified_event(&agent_event_with_payload(
                "active-progress",
                "rt:worker:1",
                event_type,
                payload,
            ))
            .await;
        if callback {
            store
                .project_unified_event(&agent_event_with_payload(
                    "active-callback",
                    "rt:worker:1",
                    "interaction_callback_pending",
                    json!({"interaction_id": "active-b", "tool_name": "host_tool", "args": {}}),
                ))
                .await;
        }
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some(expected_phase)
        );

        if terminal_kind == "dispatch_failure" {
            store
                .record_interaction_failure(
                    "worker",
                    "pending-a",
                    json!({"error": "delivery failed"}),
                )
                .await;
        } else {
            store
                .project_unified_event(&agent_event_with_payload(
                    "pending-terminal",
                    "rt:worker:1",
                    terminal_kind,
                    json!({"interaction_id": "pending-a", "result": "first settled"}),
                ))
                .await;
        }

        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some(expected_phase),
            "another input's terminal must not clear the current run's phase"
        );
        {
            let state = store.state.read().await;
            assert_eq!(
                state
                    .active_interaction_by_identity
                    .get("worker")
                    .map(String::as_str),
                Some("active-b")
            );
            assert_eq!(
                state.callback_pending_by_identity.contains("worker"),
                callback
            );
            assert_eq!(state.pending_by_identity["worker"].len(), 1);
            assert_eq!(
                state.pending_by_identity["worker"][0].interaction_id,
                "active-b"
            );
        }
        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            replay
                .iter()
                .any(|frame| frame.interaction_id.as_deref() == Some("pending-a")
                    && matches!(
                        frame.event_type.as_str(),
                        "interaction_complete" | "interaction_failed"
                    ))
        );

        if callback {
            store
                .project_unified_event(&agent_event_with_payload(
                    "active-resume",
                    "rt:worker:1",
                    "run_started",
                    json!({"input": {"kind": "pending_tool_results"}}),
                ))
                .await;
            let replay = store.replay_all(None).await.expect("replay");
            assert_eq!(
                replay
                    .iter()
                    .find(|frame| frame.event_id == "active-resume")
                    .expect("resume")
                    .interaction_id
                    .as_deref(),
                Some("active-b")
            );
        }
        store
            .project_unified_event(&agent_event_with_payload(
                "active-terminal",
                "rt:worker:1",
                "interaction_complete",
                json!({"interaction_id": "active-b", "result": "second done"}),
            ))
            .await;
        assert_eq!(
            store.response_phase_for_identity("worker").await,
            None,
            "the active interaction's own terminal must still clear its phase"
        );
        let state = store.state.read().await;
        assert!(!state.active_interaction_by_identity.contains_key("worker"));
        assert!(!state.callback_pending_by_identity.contains("worker"));
        assert!(!state.pending_by_identity.contains_key("worker"));
    }

    #[tokio::test]
    async fn pending_directed_terminal_preserves_another_active_run_phase_and_callback() {
        for terminal_kind in ["interaction_complete", "interaction_failed"] {
            for callback in [false, true] {
                assert_pending_terminal_preserves_active_run(terminal_kind, callback).await;
            }
        }
    }

    #[tokio::test]
    async fn pending_dispatch_failure_preserves_another_active_run_phase_and_callback() {
        for callback in [false, true] {
            assert_pending_terminal_preserves_active_run("dispatch_failure", callback).await;
        }
    }

    #[tokio::test]
    async fn last_pending_failure_without_active_run_clears_waiting_phase() {
        let store = ConsoleEventStore::new();
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "pending-a",
                "console",
                json!("hello"),
            )
            .await
            .expect("reserve");
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some("waiting")
        );
        store
            .record_interaction_failure("worker", "pending-a", json!({"error": "delivery failed"}))
            .await;
        assert_eq!(store.response_phase_for_identity("worker").await, None);
        assert!(
            !store
                .state
                .read()
                .await
                .pending_by_identity
                .contains_key("worker")
        );
    }

    #[tokio::test]
    async fn run_started_matches_pending_prompt_and_preserves_other_accepted_inputs() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "stale-turn",
                "console",
                json!("debug probe that was superseded"),
            )
            .await
            .expect("reserve stale interaction");
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "real-turn",
                "console",
                json!("reply with this exact token"),
            )
            .await
            .expect("reserve real interaction");

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-run-started".to_string(),
                source: "test".to_string(),
                timestamp_ms: 1,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "run_started".to_string(),
                    payload: Some(json!({
                        "session_id": "session-1",
                        "input": { "kind": "content", "content": "reply with this exact token" }
                    })),
                },
            })
            .await;
        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-run-completed".to_string(),
                source: "test".to_string(),
                timestamp_ms: 2,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "run_completed".to_string(),
                    payload: Some(json!({ "result": "reply with this exact token" })),
                },
            })
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        assert!(
            !replay
                .iter()
                .any(|event| event.interaction_id.as_deref() == Some("stale-turn"))
        );
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"][0].interaction_id,
            "stale-turn"
        );

        let run_started = replay
            .iter()
            .find(|event| event.event_id == "evt-run-started")
            .expect("run_started should be replayed");
        assert_eq!(run_started.interaction_id.as_deref(), Some("real-turn"));
        let run_completed = replay
            .iter()
            .find(|event| event.event_id == "evt-run-completed")
            .expect("run completion should be replayed");
        assert_eq!(run_completed.interaction_id.as_deref(), Some("real-turn"));
    }

    #[tokio::test]
    async fn runtime_event_child_id_uses_registered_identity_alias() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:review:singleton:0", "review:singleton")
            .await;
        store
            .reserve_interaction_value(
                "review:singleton",
                Some("rt:review:singleton:0"),
                "turn-1",
                "console",
                json!("console prompt"),
            )
            .await
            .expect("reserve interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:review:singleton:0:0",
                "run_started",
                json!({"input": {"kind": "content", "content": "console prompt"}}),
            ))
            .await;

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-1".to_string(),
                source: "test".to_string(),
                timestamp_ms: 1,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:review:singleton:0:0".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "ok" })),
                },
            })
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let projected = replay
            .iter()
            .find(|event| event.event_id == "evt-1")
            .expect("child runtime event should project");
        assert_eq!(projected.identity, "review:singleton");
        assert_eq!(projected.interaction_id.as_deref(), Some("turn-1"));
        assert_eq!(
            store
                .response_phase_for_identity("review:singleton")
                .await
                .as_deref(),
            Some("generating")
        );
    }

    #[tokio::test]
    async fn tool_use_turn_completed_keeps_phase_and_pending_interaction() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "turn-1",
                "console",
                json!("console prompt"),
            )
            .await
            .expect("reserve interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "console prompt"}}),
            ))
            .await;

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-1".to_string(),
                source: "test".to_string(),
                timestamp_ms: 1,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "tool_call".to_string(),
                    payload: Some(json!({ "name": "inspect" })),
                },
            })
            .await;
        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-2".to_string(),
                source: "test".to_string(),
                timestamp_ms: 2,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "turn_completed".to_string(),
                    payload: Some(json!({ "stop_reason": "tool_use" })),
                },
            })
            .await;
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some("tool-executing")
        );

        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-3".to_string(),
                source: "test".to_string(),
                timestamp_ms: 3,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "after tool" })),
                },
            })
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let after_tool_delta = replay
            .iter()
            .find(|event| event.event_id == "evt-3")
            .expect("after-tool delta should be replayed");
        assert_eq!(after_tool_delta.interaction_id.as_deref(), Some("turn-1"));
    }

    fn agent_event(
        event_id: &str,
        agent_id: &str,
        event_type: &str,
    ) -> EventEnvelope<UnifiedEvent> {
        agent_event_with_payload(event_id, agent_id, event_type, json!({}))
    }

    fn screenshot_result(event_type: &str, event_id: &str) -> EventEnvelope<UnifiedEvent> {
        agent_event_with_payload(
            event_id,
            "rt:pilot:1",
            event_type,
            json!({
                "id": "shot",
                "name": "computer_screenshot",
                "is_error": false,
                "content": [
                    { "type": "text", "text": "Captured." },
                    { "type": "image", "media_type": "image/png", "source": "inline", "data": "iVBORw0KGgo=" },
                ],
            }),
        )
    }

    async fn tool_result_frames(store: &ConsoleEventStore) -> Vec<Value> {
        store
            .replay_all(None)
            .await
            .expect("replay")
            .into_iter()
            .filter(|event| {
                matches!(
                    event.event_type.as_str(),
                    "tool_result_received" | "tool_execution_completed"
                )
            })
            .map(|event| event.data)
            .collect()
    }

    #[tokio::test]
    async fn live_tool_result_images_are_carried_by_blob_reference() {
        let binary: Arc<dyn crate::blob_store::BinaryBlobStore> =
            Arc::new(crate::blob_store::ObjectStoreBlobStore::memory());
        let store = ConsoleEventStore::new();
        store.install_blob_store(Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(
            Arc::clone(&binary),
        )));
        store.register_runtime_identity("rt:pilot:1", "pilot").await;
        // The execution fact comes first and carries no body; the result
        // event that follows carries the tool's output.
        store
            .project_unified_event(&completed_without_body("evt-completed", "shot"))
            .await;
        store
            .project_unified_event(&screenshot_result("tool_result_received", "evt-received"))
            .await;

        let frames = tool_result_frames(&store).await;
        assert_eq!(frames.len(), 2);
        let completed = frame_of(&frames, "tool_execution_completed");
        assert!(completed.get("content").is_none(), "{completed:#}");
        assert!(completed.get("result").is_none(), "{completed:#}");
        let received = frame_of(&frames, "tool_result_received");
        let image = &received["content"][1];
        assert_eq!(image["source"], json!("blob"), "{received:#}");
        assert_eq!(received["content"][0]["text"], json!("Captured."));
        let blob_id = image["blob_id"].as_str().expect("blob id").to_string();
        assert!(
            !serde_json::to_string(&frames)
                .expect("frames serialize")
                .contains("iVBORw0KGgo"),
            "no frame carries the image bytes"
        );
        let stored = binary
            .get_bytes(&meerkat_core::BlobId::new(blob_id))
            .await
            .expect("the image is in the session blob store");
        assert_eq!(stored.media_type, "image/png");
    }

    /// An execution-fact payload from an older producer may still carry the
    /// body; its images go by blob reference too, never inline.
    #[tokio::test]
    async fn an_older_completed_payload_with_a_body_carries_images_by_blob_reference() {
        let binary: Arc<dyn crate::blob_store::BinaryBlobStore> =
            Arc::new(crate::blob_store::ObjectStoreBlobStore::memory());
        let store = ConsoleEventStore::new();
        store.install_blob_store(Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(
            Arc::clone(&binary),
        )));
        store.register_runtime_identity("rt:pilot:1", "pilot").await;
        store
            .project_unified_event(&screenshot_result(
                "tool_execution_completed",
                "evt-completed",
            ))
            .await;

        let frames = tool_result_frames(&store).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["content"][1]["source"], json!("blob"));
        assert!(
            !serde_json::to_string(&frames)
                .expect("frames serialize")
                .contains("iVBORw0KGgo"),
            "no frame carries the image bytes"
        );
    }

    fn completed_without_body(event_id: &str, tool_call_id: &str) -> EventEnvelope<UnifiedEvent> {
        agent_event_with_payload(
            event_id,
            "rt:pilot:1",
            "tool_execution_completed",
            json!({
                "id": tool_call_id,
                "name": "computer_screenshot",
                "is_error": false,
                "duration_ms": 12,
            }),
        )
    }

    fn frame_of<'a>(frames: &'a [Value], source_event_type: &str) -> &'a Value {
        frames
            .iter()
            .find(|frame| frame["source_event_type"] == json!(source_event_type))
            .expect("a frame of the event type is projected")
    }

    fn generate_image_payload(with_body: bool) -> Value {
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
        let mut payload = json!({
            "id": "call-image",
            "tool_call_id": "call-image",
            "name": "generate_image",
            "is_error": false,
        });
        if with_body {
            let text = serde_json::to_string(&image_result).expect("serialize image result");
            payload["result"] = json!(text);
            payload["content"] = json!([{ "type": "text", "text": text }]);
        }
        payload
    }

    async fn assistant_image_frames(store: &ConsoleEventStore) -> Vec<Value> {
        store
            .replay_all(None)
            .await
            .expect("replay")
            .into_iter()
            .filter(|event| event.event_type == "assistant_image")
            .map(|event| event.data)
            .collect()
    }

    /// A generated image is projected exactly once per tool call: from the
    /// result event, never from the execution fact, whether or not that
    /// fact carries a body.
    #[tokio::test]
    async fn a_generated_image_is_projected_once_from_the_result_event() {
        for completed_has_body in [false, true] {
            let store = ConsoleEventStore::new();
            store.register_runtime_identity("rt:pilot:1", "pilot").await;
            let mut completed = generate_image_payload(completed_has_body);
            completed["duration_ms"] = json!(12);
            store
                .project_unified_event(&agent_event_with_payload(
                    "evt-completed",
                    "rt:pilot:1",
                    "tool_execution_completed",
                    completed,
                ))
                .await;
            store
                .project_unified_event(&agent_event_with_payload(
                    "evt-received",
                    "rt:pilot:1",
                    "tool_result_received",
                    generate_image_payload(true),
                ))
                .await;

            let images = assistant_image_frames(&store).await;
            assert_eq!(images.len(), 1, "body on completed: {completed_has_body}");
            assert_eq!(
                images[0]["source_event_type"],
                json!("tool_result_received")
            );
            assert_eq!(images[0]["tool_call_id"], json!("call-image"));
            assert_eq!(images[0]["blob_id"], json!("sha256:test-generated-image"));
        }
    }

    #[tokio::test]
    async fn without_a_blob_store_live_tool_result_images_stay_inline() {
        let store = ConsoleEventStore::new();
        store.register_runtime_identity("rt:pilot:1", "pilot").await;
        store
            .project_unified_event(&screenshot_result("tool_result_received", "evt-received"))
            .await;
        let frames = tool_result_frames(&store).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["content"][1]["source"], json!("inline"));
        assert_eq!(frames[0]["content"][1]["data"], json!("iVBORw0KGgo="));
    }

    fn agent_event_with_payload(
        event_id: &str,
        agent_id: &str,
        event_type: &str,
        payload: Value,
    ) -> EventEnvelope<UnifiedEvent> {
        EventEnvelope {
            event_id: event_id.to_string(),
            source: "test".to_string(),
            timestamp_ms: 1,
            event: UnifiedEvent::Agent {
                agent_id: agent_id.to_string(),
                event_type: event_type.to_string(),
                payload: Some(payload),
            },
        }
    }

    /// No member event reaches the console under a runtime-plane identity:
    /// every member-attributed writer of the store refuses one (agent events,
    /// lifecycle records, interaction failures and plain appends).
    #[tokio::test]
    async fn member_attributed_writers_refuse_runtime_plane_identities() {
        let store = ConsoleEventStore::new();
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-system-member",
                "_system:1",
                "text_delta",
                json!({ "delta": "member text" }),
            ))
            .await;
        store
            .register_runtime_identity("console-override", "__console__")
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-console-member",
                "console-override:1",
                "text_delta",
                json!({ "delta": "member text" }),
            ))
            .await;
        store
            .record_lifecycle("__console__", "member_retired", json!({}))
            .await;
        store
            .record_interaction_failure("_system", "interaction-1", json!({ "reason": "test" }))
            .await;
        store
            .append("__console__", None, "memory.harvest.completed", json!({}))
            .await;
        let replay = store.replay_all(None).await.expect("replay");
        let runtime_plane = replay
            .iter()
            .filter(|event| event.identity == "_system" || event.identity == "__console__")
            .map(|event| (event.identity.as_str(), event.event_type.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            runtime_plane,
            vec![("_system", "runtime_bootstrapped")],
            "only the runtime's own bootstrap event carries a runtime-plane identity"
        );
    }

    /// Regression: an agent event that arrives before any registration must
    /// not poison the runtime-id cache. The derive-based guess used to be
    /// cached via `or_insert`, so the exact-match lookup permanently shadowed
    /// the later `register_runtime_identity` and reserved interactions never
    /// completed.
    #[tokio::test]
    async fn early_event_does_not_poison_cache_against_later_registration() {
        let store = ConsoleEventStore::new();

        // Event lands before any spawn/reserve/reconcile registration.
        store
            .project_unified_event(&agent_event(
                "evt-early",
                "rt:review:singleton:0:1",
                "text_delta",
            ))
            .await;

        // Identity-bridge send path: reserve registers the runtime member id
        // against the durable identity.
        store
            .register_runtime_identity("rt:review:singleton:0", "review:singleton")
            .await;
        store
            .reserve_interaction_value(
                "review:singleton",
                Some("rt:review:singleton:0"),
                "turn-1",
                "meta/send_message",
                json!("console prompt"),
            )
            .await
            .expect("reserve interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:review:singleton:0:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "console prompt"}}),
            ))
            .await;

        store
            .project_unified_event(&agent_event(
                "evt-complete",
                "rt:review:singleton:0:1",
                "run_completed",
            ))
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let completion = replay
            .iter()
            .find(|event| event.event_id == "evt-complete")
            .expect("completion event should project");
        assert_eq!(
            completion.identity, "review:singleton",
            "completion must project under the registered durable identity"
        );
        assert_eq!(completion.event_type, "interaction_complete");
        assert_eq!(completion.interaction_id.as_deref(), Some("turn-1"));
    }

    /// Regression: the reconcile roster refresh registers the alias
    /// self-mapping as a fallback only — it must never clobber the durable
    /// identity that an identity-bridge reserve registered for the same
    /// runtime member id.
    #[tokio::test]
    async fn fallback_registration_never_clobbers_durable_identity() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:review:singleton:0", "review:singleton")
            .await;
        // Reconcile tick after the reserve.
        store
            .register_runtime_identity_fallback("rt:review:singleton:0", "rt:review:singleton:0")
            .await;
        store
            .reserve_interaction_value(
                "review:singleton",
                Some("rt:review:singleton:0"),
                "turn-1",
                "meta/send_message",
                json!("console prompt"),
            )
            .await
            .expect("reserve interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:review:singleton:0:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "console prompt"}}),
            ))
            .await;

        store
            .project_unified_event(&agent_event(
                "evt-complete",
                "rt:review:singleton:0:1",
                "run_completed",
            ))
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let completion = replay
            .iter()
            .find(|event| event.event_id == "evt-complete")
            .expect("completion event should project");
        assert_eq!(completion.identity, "review:singleton");
        assert_eq!(completion.interaction_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn event_channel_capacity_scales_with_member_count() {
        assert_eq!(event_channel_capacity_for_members(0), 4096);
        assert!(
            event_channel_capacity_for_members(136) >= 136 * 128,
            "large mobs need broadcast headroom for startup bursts"
        );
        assert_eq!(event_channel_capacity_for_members(10_000), 65_536);
    }

    /// meerkat's `RunStarted` carries `input: RunInput`, never a `prompt`.
    #[test]
    fn run_started_matcher_reads_the_typed_run_input() {
        let pending = PendingInteraction {
            interaction_id: "turn-1".to_string(),
            origin: "console".to_string(),
            content: json!("reply with this exact token"),
        };
        assert!(pending_matches_run_started(
            &pending,
            &json!({
                "session_id": "s-1",
                "input": { "kind": "content", "content": "reply with this exact token" }
            })
        ));
        assert!(pending_matches_run_started(
            &pending,
            &json!({
                "session_id": "s-1",
                "input": {
                    "kind": "content",
                    "content": [ { "type": "text", "text": "reply with this exact token" } ]
                }
            })
        ));
        assert!(!pending_matches_run_started(
            &pending,
            &json!({
                "session_id": "s-1",
                "input": { "kind": "content", "content": "something else" }
            })
        ));
        assert!(!pending_matches_run_started(
            &pending,
            &json!({ "session_id": "s-1", "input": { "kind": "pending_tool_results" } })
        ));
    }

    /// A runtime-minted terminal for an interaction the console never
    /// reserved (peer, flow-step and schedule inputs mint their own ids) must
    /// not close the console's pending send; one that names the reserved id
    /// is attributed to it and closes it.
    #[tokio::test]
    async fn directed_terminals_attribute_only_to_the_interaction_they_name() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "console-turn",
                "console",
                json!("hello"),
            )
            .await
            .expect("reserve console interaction");

        store
            .project_unified_event(&agent_event_with_payload(
                "evt-foreign-complete",
                "rt:worker:1",
                "interaction_complete",
                json!({ "interaction_id": "peer-directed-turn", "result": "" }),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-matching-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "hello"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-delta-while-pending",
                "rt:worker:1",
                "text_delta",
                json!({ "delta": "hi" }),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-own-complete",
                "rt:worker:1",
                "interaction_complete",
                json!({ "interaction_id": "console-turn", "result": "done" }),
            ))
            .await;
        assert_eq!(
            store.response_phase_for_identity("worker").await,
            None,
            "the identity is idle once its own terminal projected"
        );
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-delta-after-close",
                "rt:worker:1",
                "text_delta",
                json!({ "delta": "later" }),
            ))
            .await;

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let by_id = |event_id: &str| {
            replay
                .iter()
                .find(|event| event.event_id == event_id)
                .expect(event_id)
        };
        let foreign = by_id("evt-foreign-complete");
        assert_eq!(foreign.event_type, "interaction_complete");
        assert_eq!(
            foreign.interaction_id, None,
            "a terminal naming another interaction must project unattributed"
        );
        assert_eq!(
            by_id("evt-delta-while-pending").interaction_id.as_deref(),
            Some("console-turn"),
            "the console send is still pending after a foreign terminal"
        );
        assert_eq!(
            by_id("evt-own-complete").interaction_id.as_deref(),
            Some("console-turn"),
            "a terminal naming the reserved interaction is attributed to it"
        );
        assert_eq!(
            by_id("evt-delta-after-close").interaction_id,
            None,
            "the console interaction is closed once its own terminal projected"
        );
    }

    /// `interaction_callback_pending` is a pause, not a terminal: meerkat
    /// documents it as "waiting for tool results before the session can
    /// continue" and later publishes the real terminal under the SAME
    /// interaction id. Closing the console interaction on it left the resumed
    /// run bound to the NEXT queued send and the turn's own terminal
    /// unattributed.
    #[tokio::test]
    async fn callback_pending_names_its_interaction_without_closing_it() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "console-turn",
                "console",
                json!("hello"),
            )
            .await
            .expect("reserve first console interaction");
        // A second send queued behind the first: the resumed run must not
        // bind to it.
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "console-turn-2",
                "console",
                json!("second"),
            )
            .await
            .expect("reserve second console interaction");

        store
            .project_unified_event(&agent_event_with_payload(
                "evt-run-started",
                "rt:worker:1",
                "run_started",
                json!({ "input": { "kind": "content", "content": "hello" } }),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-tool-call",
                "rt:worker:1",
                "tool_call_requested",
                json!({ "id": "call-1", "name": "host_tool", "args": {} }),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-callback-pending",
                "rt:worker:1",
                "interaction_callback_pending",
                json!({
                    "interaction_id": "console-turn",
                    "tool_name": "host_tool",
                    "args": {},
                }),
            ))
            .await;
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some("tool-executing"),
            "a paused interaction is not idle"
        );

        // The host answered the callback; meerkat resumes the SAME
        // interaction with a pending-tool-results run.
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-run-resumed",
                "rt:worker:1",
                "run_started",
                json!({ "input": { "kind": "pending_tool_results" } }),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-delta-after-resume",
                "rt:worker:1",
                "text_delta",
                json!({ "delta": "done" }),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-own-complete",
                "rt:worker:1",
                "interaction_complete",
                json!({ "interaction_id": "console-turn", "result": "done" }),
            ))
            .await;
        assert_eq!(
            store.response_phase_for_identity("worker").await,
            None,
            "the identity is idle once the real terminal projected"
        );

        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let by_id = |event_id: &str| {
            replay
                .iter()
                .find(|event| event.event_id == event_id)
                .expect(event_id)
        };
        for event_id in [
            "evt-callback-pending",
            "evt-run-resumed",
            "evt-delta-after-resume",
            "evt-own-complete",
        ] {
            assert_eq!(
                by_id(event_id).interaction_id.as_deref(),
                Some("console-turn"),
                "{event_id} belongs to the paused-then-resumed interaction"
            );
        }
        assert!(
            !replay.iter().any(|event| {
                event.event_type == "interaction_failed"
                    && event.data["reason"] == json!("superseded_by_later_run")
            }),
            "the resumed run must not supersede the still-pending first send: {replay:#?}"
        );
        // The second send is still queued, untouched, for its own run.
        store
            .project_unified_event(&agent_event_with_payload(
                "evt-second-run-started",
                "rt:worker:1",
                "run_started",
                json!({ "input": { "kind": "content", "content": "second" } }),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("all-events replay should succeed");
        let second = replay
            .iter()
            .find(|event| event.event_id == "evt-second-run-started")
            .expect("evt-second-run-started");
        assert_eq!(second.interaction_id.as_deref(), Some("console-turn-2"));
    }

    async fn assert_foreign_run_preserves_queued_console_input(start_before_reservation: bool) {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        let foreign_start = agent_event_with_payload(
            "foreign-start",
            "rt:worker:1",
            "run_started",
            json!({"input": {"kind": "content", "content": "Peer request: mob.kickoff_started"}}),
        );
        if start_before_reservation {
            store.project_unified_event(&foreign_start).await;
        }
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "operator-a",
                "console",
                json!("Create the WorkGraph"),
            )
            .await
            .expect("reserve operator input");
        if !start_before_reservation {
            store.project_unified_event(&foreign_start).await;
        }
        for (id, kind, payload) in [
            (
                "foreign-tool",
                "tool_call_requested",
                json!({"name": "send_response"}),
            ),
            (
                "foreign-text",
                "text_delta",
                json!({"delta": "Kickoff acknowledged"}),
            ),
            (
                "foreign-end",
                "run_completed",
                json!({"result": "Kickoff acknowledged"}),
            ),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(id, "rt:worker:1", kind, payload))
                .await;
        }
        let replay = store.replay_all(None).await.expect("replay");
        for frame in replay
            .iter()
            .filter(|frame| frame.event_id.starts_with("foreign-"))
        {
            assert_eq!(
                frame.interaction_id, None,
                "foreign event must not steal the operator reservation: {}",
                frame.event_id
            );
        }
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            1
        );
        for (id, kind, payload) in [
            (
                "operator-start",
                "run_started",
                json!({"input": {"kind": "content", "content": "Create the WorkGraph"}}),
            ),
            (
                "operator-tool",
                "tool_call_requested",
                json!({"name": "workgraph_create"}),
            ),
            (
                "operator-end",
                "run_completed",
                json!({"result": "WorkGraph created"}),
            ),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(id, "rt:worker:1", kind, payload))
                .await;
        }
        let replay = store.replay_all(None).await.expect("replay");
        for frame in replay
            .iter()
            .filter(|frame| frame.event_id.starts_with("operator-"))
        {
            assert_eq!(
                frame.interaction_id.as_deref(),
                Some("operator-a"),
                "{}",
                frame.event_id
            );
        }
        assert!(
            !store
                .state
                .read()
                .await
                .pending_by_identity
                .contains_key("worker")
        );
    }

    #[tokio::test]
    async fn foreign_run_started_after_reservation_does_not_steal_console_interaction() {
        assert_foreign_run_preserves_queued_console_input(false).await;
    }

    #[tokio::test]
    async fn reservation_mid_foreign_run_does_not_reassign_its_remaining_events() {
        assert_foreign_run_preserves_queued_console_input(true).await;
    }

    #[tokio::test]
    async fn unassociated_tool_results_continuation_does_not_select_pending_input() {
        let store = ConsoleEventStore::new();
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "operator-a",
                "console",
                json!("hello"),
            )
            .await
            .expect("reserve");
        for (id, kind, payload) in [
            (
                "unknown-continuation",
                "run_started",
                json!({"input": {"kind": "pending_tool_results"}}),
            ),
            (
                "unknown-delta",
                "text_delta",
                json!({"delta": "foreign continuation"}),
            ),
            (
                "unknown-end",
                "run_completed",
                json!({"result": "foreign continuation"}),
            ),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(id, "rt:worker:1", kind, payload))
                .await;
        }
        assert!(
            store
                .replay_all(None)
                .await
                .expect("replay")
                .iter()
                .all(|frame| frame.interaction_id.is_none())
        );
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            1
        );
    }

    #[tokio::test]
    async fn unrelated_content_start_clears_previous_run_association_without_consuming_reservation()
    {
        let store = ConsoleEventStore::new();
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "operator-a",
                "console",
                json!("hello"),
            )
            .await
            .expect("reserve");
        for (id, content) in [("own-start", "hello"), ("foreign-start", "peer work")] {
            store
                .project_unified_event(&agent_event_with_payload(
                    id,
                    "rt:worker:1",
                    "run_started",
                    json!({"input": {"kind": "content", "content": content}}),
                ))
                .await;
        }
        store
            .project_unified_event(&agent_event_with_payload(
                "foreign-end",
                "rt:worker:1",
                "run_completed",
                json!({"result": "peer done"}),
            ))
            .await;
        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            replay
                .iter()
                .filter(|frame| frame.event_id.starts_with("foreign-"))
                .all(|frame| frame.interaction_id.is_none())
        );
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            1
        );
    }

    #[tokio::test]
    async fn identical_pending_inputs_require_attribution_instead_of_queue_order_guess() {
        let store = ConsoleEventStore::new();
        for id in ["operator-a", "operator-b"] {
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    id,
                    "console",
                    json!("same exact input"),
                )
                .await
                .expect("reserve");
        }
        store
            .project_unified_event(&agent_event_with_payload(
                "ambiguous-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "same exact input"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "ambiguous-end",
                "rt:worker:1",
                "run_completed",
                json!({"result": "done"}),
            ))
            .await;
        assert!(
            store
                .replay_all(None)
                .await
                .expect("replay")
                .iter()
                .all(|frame| frame.interaction_id.is_none())
        );
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            2
        );
        store
            .project_unified_event(&agent_event_with_payload(
                "exact-end",
                "rt:worker:1",
                "interaction_complete",
                json!({"interaction_id": "operator-a", "result": "done"}),
            ))
            .await;
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"][0].interaction_id,
            "operator-b"
        );
    }

    #[tokio::test]
    async fn replayed_start_cannot_rebind_a_new_active_run() {
        let store = ConsoleEventStore::new();
        for (id, content) in [("operator-a", "first"), ("operator-b", "second")] {
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    id,
                    "console",
                    json!(content),
                )
                .await
                .expect("reserve");
        }
        let first_start = agent_event_with_payload(
            "first-start",
            "rt:worker:1",
            "run_started",
            json!({"input": {"kind": "content", "content": "first"}}),
        );
        store.project_unified_event(&first_start).await;
        store
            .project_unified_event(&agent_event_with_payload(
                "first-end",
                "rt:worker:1",
                "run_completed",
                json!({"result": "first done"}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "second-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "second"}}),
            ))
            .await;
        store.project_unified_event(&first_start).await;
        store
            .project_unified_event(&agent_event_with_payload(
                "second-delta",
                "rt:worker:1",
                "text_delta",
                json!({"delta": "second result"}),
            ))
            .await;
        let replay = store.replay_all(None).await.expect("replay");
        assert_eq!(
            replay
                .iter()
                .find(|frame| frame.event_id == "second-delta")
                .expect("delta")
                .interaction_id
                .as_deref(),
            Some("operator-b")
        );
    }

    #[test]
    fn run_started_matcher_uses_exact_canonical_multimodal_content() {
        let inline = json!([
            {"type": "text", "text": "  Exact\nUnicode: A\u{030a} 🚀  "},
            {"type": "image", "media_type": "image/png", "source": "inline", "data": "aGVsbG8="}
        ]);
        let blob_id = meerkat_core::blob::content_blob_id("image/png", "aGVsbG8=");
        let blob = json!([
            {"type": "text", "text": "  Exact\nUnicode: A\u{030a} 🚀  "},
            {"type": "image", "media_type": "image/png", "source": "blob", "blob_id": blob_id}
        ]);
        let pending = PendingInteraction {
            interaction_id: "operator-a".into(),
            origin: "console".into(),
            content: inline.clone(),
        };
        assert!(pending_matches_run_started(
            &pending,
            &json!({"input": {"kind": "content", "content": blob}})
        ));
        let mut changed_image = inline.clone();
        changed_image[1]["data"] = json!("ZGlmZmVyZW50");
        assert!(!pending_matches_run_started(
            &pending,
            &json!({"input": {"kind": "content", "content": changed_image}})
        ));
        assert!(!pending_matches_run_started(
            &pending,
            &json!({"input": {"kind": "content", "content": inline[0]["text"]}})
        ));
        let mut reordered = inline.as_array().expect("blocks").clone();
        reordered.reverse();
        assert!(!pending_matches_run_started(
            &pending,
            &json!({"input": {"kind": "content", "content": reordered}})
        ));
        let mut changed_text = inline;
        changed_text[0]["text"] = json!("Exact\nUnicode: A\u{030a} 🚀");
        assert!(!pending_matches_run_started(
            &pending,
            &json!({"input": {"kind": "content", "content": changed_text}})
        ));
    }

    fn typed_lineage(interaction: u128, run: u128) -> Value {
        json!({
            "interaction_id": uuid::Uuid::from_u128(interaction).to_string(),
            "run_id": uuid::Uuid::from_u128(run).to_string(),
        })
    }

    #[tokio::test]
    async fn typed_background_job_completion_preserves_owner_until_matching_run_terminal() {
        use meerkat_core::{AgentEvent, event::BackgroundJobTerminalStatus};

        let store = ConsoleEventStore::new();
        let owner = typed_lineage(171, 271);
        let session_id = meerkat_core::types::SessionId::new();
        let interaction = owner["interaction_id"].as_str().expect("owner interaction");
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                interaction,
                "console",
                json!("review"),
            )
            .await
            .expect("reserve owner interaction");
        for (id, kind, payload) in [
            (
                "owner-start",
                "run_started",
                json!({
                    "session_id": session_id, "identity": owner,
                    "input": {"kind": "content", "content": "review"},
                }),
            ),
            (
                "owner-delta",
                "text_delta",
                json!({"delta": "Review in progress"}),
            ),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(id, "rt:worker:1", kind, payload))
                .await;
        }
        let phase = store.response_phase_for_identity("worker").await;
        assert_eq!(phase.as_deref(), Some("generating"));
        let active = store.state.read().await.active_run_by_identity["worker"].clone();
        let project_typed = |id: &str, event: &AgentEvent| {
            agent_event_with_payload(
                id,
                "rt:worker:1",
                meerkat_core::event::agent_event_type(event),
                crate::mob_handle_runtime::console_agent_event_payload(event),
            )
        };
        let completion = AgentEvent::background_job_completed(
            "review-job-7",
            "background review",
            BackgroundJobTerminalStatus::Completed,
            "Exact result A\u{030a}, \u{00e5} and \u{1f680}.\nSecond paragraph.",
        );
        let job_event = project_typed("job-completed", &completion);
        store.project_unified_event(&job_event).await;
        assert_eq!(store.response_phase_for_identity("worker").await, phase);
        {
            let state = store.state.read().await;
            assert_eq!(state.active_run_by_identity.get("worker"), Some(&active));
            assert_eq!(state.pending_by_identity["worker"].len(), 1);
            assert_eq!(
                state.pending_by_identity["worker"][0].interaction_id,
                interaction
            );
        }
        let terminals = |events: &[ConsoleIdentityEventEnvelope]| {
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event.event_type.as_str(),
                        "interaction_complete" | "interaction_failed"
                    )
                })
                .count()
        };
        let replay = store.replay_all(None).await.expect("job replay");
        assert_eq!(
            terminals(&replay),
            0,
            "a job outcome must not mint any owner terminal"
        );
        let job = replay
            .iter()
            .find(|event| event.event_id == "job-completed")
            .expect("job outcome stays available");
        assert_eq!(job.identity, "worker");
        assert_eq!(job.event_type, "background_job_completed");
        assert_eq!(job.data["terminal_status"], "completed");
        assert_eq!(job.data["source_event_type"], "background_job_completed");
        for (key, value) in crate::mob_handle_runtime::console_agent_event_payload(&completion)
            .as_object()
            .expect("typed job payload")
        {
            assert_eq!(
                job.data.get(key),
                Some(value),
                "job field {key} stays exact"
            );
        }

        let terminal = AgentEvent::RunCompleted {
            assistant_message_id: None,
            session_id,
            identity: active.clone(),
            result: "Owner review finished".into(),
            structured_output: None,
            extraction_required: false,
            usage: Default::default(),
            terminal_cause_kind: None,
        };
        store
            .project_unified_event(&project_typed("owner-completed", &terminal))
            .await;
        assert_eq!(store.response_phase_for_identity("worker").await, None);
        {
            let state = store.state.read().await;
            assert!(!state.active_run_by_identity.contains_key("worker"));
            assert!(
                !state
                    .pending_by_identity
                    .get("worker")
                    .is_some_and(|queue| queue
                        .iter()
                        .any(|pending| pending.interaction_id == interaction))
            );
        }
        let replay = store.replay_all(None).await.expect("owner terminal replay");
        assert_eq!(terminals(&replay), 1);
        let finished = replay
            .iter()
            .find(|event| event.event_id == "owner-completed")
            .expect("actual owner terminal");
        assert_eq!(finished.event_type, "interaction_complete");
        assert_eq!(finished.interaction_id.as_deref(), Some(interaction));
        assert_eq!(finished.data["source_event_type"], "run_completed");
        assert_eq!(finished.data["identity"], owner);
        assert_eq!(
            replay
                .iter()
                .filter(|event| event.event_id == "job-completed")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn typed_peer_start_preserves_canonical_lineage_without_consuming_equal_console_input() {
        let store = ConsoleEventStore::new();
        let operator = uuid::Uuid::from_u128(101).to_string();
        let peer = typed_lineage(102, 202);
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                &operator,
                "console",
                json!("same text"),
            )
            .await
            .expect("reserve exact console interaction");
        for (id, kind, payload) in [
            (
                "peer-start",
                "run_started",
                json!({"identity": peer, "input": {"kind": "content", "content": "same text"}}),
            ),
            ("peer-delta", "text_delta", json!({"delta": "done"})),
            (
                "peer-end",
                "run_completed",
                json!({"identity": peer, "result": "done"}),
            ),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(id, "rt:worker:1", kind, payload))
                .await;
        }
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        for event in replay
            .iter()
            .filter(|event| event.event_id.starts_with("peer-"))
        {
            assert_eq!(
                event.interaction_id.as_deref(),
                peer["interaction_id"].as_str()
            );
            assert_eq!(event.data["identity"], peer);
            assert_eq!(event.data["run_id"], peer["run_id"]);
        }
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"][0].interaction_id,
            operator
        );
    }

    #[tokio::test]
    async fn typed_identical_pending_messages_match_ids_and_replayed_start_cannot_replace_current_run()
     {
        let store = ConsoleEventStore::new();
        let a = typed_lineage(111, 211);
        let b = typed_lineage(112, 212);
        for identity in [&a, &b] {
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    identity["interaction_id"]
                        .as_str()
                        .expect("typed interaction UUID"),
                    "console",
                    json!("same text"),
                )
                .await
                .expect("reserve exact console interaction");
        }
        let start = |id: &str, identity: &Value| {
            agent_event_with_payload(
                id,
                "rt:worker:1",
                "run_started",
                json!({"identity":identity,"input":{"kind":"content","content":"same text"}}),
            )
        };
        let first = start("a-start", &a);
        store.project_unified_event(&first).await;
        store
            .project_unified_event(&agent_event_with_payload(
                "a-end",
                "rt:worker:1",
                "run_completed",
                json!({"identity":a,"result":"same answer"}),
            ))
            .await;
        store.project_unified_event(&start("b-start", &b)).await;
        store.project_unified_event(&first).await;
        store
            .project_unified_event(&start("a-start-replayed-with-new-envelope", &a))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "b-output",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"same answer"}),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        let output = replay
            .iter()
            .find(|event| event.event_id == "b-output")
            .expect("projected b-output frame");
        assert_eq!(output.data["identity"], b);
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            1
        );
    }

    #[tokio::test]
    async fn missing_foreign_or_stale_terminal_does_not_borrow_or_clear_typed_active_run() {
        let store = ConsoleEventStore::new();
        let active = typed_lineage(121, 221);
        let foreign = typed_lineage(122, 222);
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity":active,"input":{"kind":"content","content":"hello"}}),
            ))
            .await;
        for (id, payload) in [
            ("missing", json!({"result":"old"})),
            ("foreign", json!({"identity":foreign,"result":"other"})),
            (
                "same-interaction-old-run",
                json!({"identity":{"interaction_id":active["interaction_id"],"run_id":foreign["run_id"]},"result":"old attempt"}),
            ),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(
                    id,
                    "rt:worker:1",
                    "run_completed",
                    payload,
                ))
                .await;
            store
                .project_unified_event(&agent_event_with_payload(
                    &format!("after-{id}"),
                    "rt:worker:1",
                    "text_delta",
                    json!({"delta":"still running"}),
                ))
                .await;
        }
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        assert!(
            replay
                .iter()
                .find(|event| event.event_id == "missing")
                .expect("projected missing frame")
                .interaction_id
                .is_none()
        );
        for event in replay
            .iter()
            .filter(|event| event.event_id.starts_with("after-"))
        {
            assert_eq!(event.data["identity"], active);
        }
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some("generating")
        );
    }

    /// Events after a typed stream gap never inherit the run that was
    /// current before it: the gap may hold that run's terminal and the start
    /// of another run.
    #[tokio::test]
    async fn stream_truncation_ends_inherited_run_lineage() {
        let store = ConsoleEventStore::new();
        let run = uuid::Uuid::from_u128(241).to_string();
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity":{"run_id":run},"input":{"kind":"content","content":"first"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "before-gap",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"first run"}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "gap",
                "rt:worker:1",
                "stream_truncated",
                json!({"reason":{"kind":"stream_lagged","dropped":7}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "after-gap",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"some run"}),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        let frame = |id: &str| {
            replay
                .iter()
                .find(|event| event.event_id == id)
                .expect("projected frame")
        };
        assert_eq!(frame("before-gap").data["run_id"], run);
        assert!(frame("gap").data.get("run_id").is_none());
        assert!(
            frame("after-gap").data.get("run_id").is_none(),
            "a delta after the gap is not attributed to the run before it"
        );
    }

    #[tokio::test]
    async fn typed_run_without_interaction_still_carries_run_and_cannot_claim_reservation() {
        let store = ConsoleEventStore::new();
        let run = uuid::Uuid::from_u128(231).to_string();
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "operator",
                "console",
                json!("same"),
            )
            .await
            .expect("reserve exact console interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity":{"run_id":run},"input":{"kind":"content","content":"same"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "delta",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"ready"}),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        let delta = replay
            .iter()
            .find(|event| event.event_id == "delta")
            .expect("projected delta frame");
        assert!(delta.interaction_id.is_none());
        assert_eq!(delta.data["run_id"], run);
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            1
        );
    }

    #[tokio::test]
    async fn callback_image_never_inherits_new_run_and_explicit_message_identity_is_preserved() {
        let store = ConsoleEventStore::new();
        let old = typed_lineage(141, 241);
        let current = typed_lineage(142, 242);
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity":current,"input":{"kind":"content","content":"new"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "old-image",
                "rt:worker:1",
                "assistant_image_appended",
                json!({"identity":old,"image":{"image_id":"old-image"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "unknown-image",
                "rt:worker:1",
                "assistant_image_appended",
                json!({"image":{"image_id":"unknown-image"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "current-delta",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"new output"}),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        let by_id = |id: &str| {
            replay
                .iter()
                .find(|event| event.event_id == id)
                .expect("projected image or current delta frame")
        };
        assert_eq!(by_id("old-image").data["identity"], old);
        assert!(by_id("unknown-image").interaction_id.is_none());
        assert!(by_id("unknown-image").data.get("run_id").is_none());
        assert_eq!(by_id("current-delta").data["identity"], current);
    }

    #[tokio::test]
    async fn typed_directed_terminal_settles_its_queued_owner_without_clearing_peer_run() {
        let store = ConsoleEventStore::new();
        let peer = typed_lineage(151, 251);
        let queued = uuid::Uuid::from_u128(152).to_string();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "peer-start",
                "rt:worker:1",
                "run_started",
                json!({"identity":peer,"input":{"kind":"content","content":"peer"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "peer-first",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"still working"}),
            ))
            .await;
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                &queued,
                "console",
                json!("queued"),
            )
            .await
            .expect("reserve exact console interaction");
        assert_eq!(
            store.response_phase_for_identity("worker").await.as_deref(),
            Some("generating")
        );
        store
            .project_unified_event(&agent_event_with_payload(
                "queue-failed",
                "rt:worker:1",
                "interaction_failed",
                json!({"interaction_id":queued,"reason":"not admitted"}),
            ))
            .await;
        assert!(
            !store
                .state
                .read()
                .await
                .pending_by_identity
                .contains_key("worker")
        );
        store
            .project_unified_event(&agent_event_with_payload(
                "peer-second",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"finishing"}),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        assert_eq!(
            replay
                .iter()
                .find(|event| event.event_id == "peer-second")
                .expect("projected peer-second frame")
                .data["identity"],
            peer
        );
        assert!(
            replay
                .iter()
                .find(|event| event.event_id == "queue-failed")
                .expect("projected queue-failed frame")
                .data
                .get("run_id")
                .is_none()
        );
        store
            .project_unified_event(&agent_event_with_payload(
                "peer-done",
                "rt:worker:1",
                "interaction_complete",
                json!({"interaction_id":peer["interaction_id"],"result":"done"}),
            ))
            .await;
        assert!(store.state.read().await.active_run_by_identity.is_empty());
    }

    #[tokio::test]
    async fn invalid_explicit_lineage_cannot_fall_back_to_active_or_matching_prompt() {
        let store = ConsoleEventStore::new();
        let active = typed_lineage(161, 261);
        store
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "legacy-operator",
                "console",
                json!("same"),
            )
            .await
            .expect("reserve exact console interaction");
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity":active,"input":{"kind":"content","content":"same"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "bad-terminal",
                "rt:worker:1",
                "run_completed",
                json!({"identity":{"interaction_id":"not-a-uuid"},"result":"bad"}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "still-current",
                "rt:worker:1",
                "text_delta",
                json!({"delta":"current"}),
            ))
            .await;
        store.project_unified_event(&agent_event_with_payload("bad-start","rt:worker:1","run_started",json!({"identity":{"run_id":"not-a-uuid"},"input":{"kind":"content","content":"same"}}))).await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        assert!(
            replay
                .iter()
                .find(|event| event.event_id == "bad-terminal")
                .expect("projected bad-terminal frame")
                .interaction_id
                .is_none()
        );
        assert_eq!(
            replay
                .iter()
                .find(|event| event.event_id == "still-current")
                .expect("projected still-current frame")
                .data["identity"],
            active
        );
        assert!(
            replay
                .iter()
                .find(|event| event.event_id == "bad-start")
                .expect("projected bad-start frame")
                .interaction_id
                .is_none()
        );
        assert_eq!(
            store.state.read().await.pending_by_identity["worker"].len(),
            1
        );
    }

    /// #469: a run whose start was never observed and whose typed terminal
    /// names no interaction used to leave its phase set (no final
    /// turn_completed after run_failed), parking the console drain gate.
    #[tokio::test]
    async fn unobserved_start_interactionless_terminal_clears_stale_phase() {
        for (phase_event, expected_phase) in [
            ("text_delta", "generating"),
            ("tool_call_requested", "tool-executing"),
        ] {
            let store = ConsoleEventStore::new();
            let run = json!({"run_id": uuid::Uuid::from_u128(301).to_string()});
            store
                .register_runtime_identity("rt:worker:1", "worker")
                .await;
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    "queued-console-send",
                    "console",
                    json!("queued"),
                )
                .await
                .expect("reserve queued console interaction");
            // run_started was missed: the first observed frame is output.
            store
                .project_unified_event(&agent_event_with_payload(
                    "output",
                    "rt:worker:1",
                    phase_event,
                    json!({"identity": run, "delta": "partial"}),
                ))
                .await;
            assert_eq!(
                store.response_phase_for_identity("worker").await.as_deref(),
                Some(expected_phase)
            );
            store
                .project_unified_event(&agent_event_with_payload(
                    "failed",
                    "rt:worker:1",
                    "run_failed",
                    json!({"identity": run, "error": "provider failed"}),
                ))
                .await;
            assert_eq!(store.response_phase_for_identity("worker").await, None);
            // The terminal named no reservation, so it settles none.
            assert_eq!(
                store.pending_interaction_ids("worker").await,
                vec!["queued-console-send".to_string()]
            );
            let replay = store
                .replay_all(None)
                .await
                .expect("replay retained console frames");
            assert!(
                replay
                    .iter()
                    .find(|event| event.event_id == "failed")
                    .expect("projected failed frame")
                    .interaction_id
                    .is_none()
            );
            assert!(!replay.iter().any(|event| {
                event.event_type == "interaction_failed"
                    && event.interaction_id.as_deref() == Some("queued-console-send")
            }));
        }
    }

    /// #479: the unobserved-start phase clear also drops a callback pause the
    /// ended run left behind. Otherwise a later `pending_tool_results` start
    /// would consume that stale flag as if it resumed a live callback.
    #[tokio::test]
    async fn unobserved_start_interactionless_terminal_clears_stale_callback_pending() {
        let store = ConsoleEventStore::new();
        let paused = typed_lineage(321, 421);
        let unobserved = json!({"run_id": uuid::Uuid::from_u128(422).to_string()});
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity": paused, "input": {"kind": "content", "content": "hello"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "callback",
                "rt:worker:1",
                "interaction_callback_pending",
                json!({
                    "interaction_id": paused["interaction_id"],
                    "tool_name": "host_tool",
                    "args": {},
                }),
            ))
            .await;
        assert!(
            store
                .state
                .read()
                .await
                .callback_pending_by_identity
                .contains("worker")
        );
        // The paused run's terminal falls inside a stream gap, and the next
        // observed frames belong to a run whose start was missed.
        store
            .project_unified_event(&agent_event_with_payload(
                "gap",
                "rt:worker:1",
                "stream_truncated",
                json!({"reason": {"kind": "stream_lagged", "dropped": 3}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "output",
                "rt:worker:1",
                "text_delta",
                json!({"identity": unobserved, "delta": "partial"}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "failed",
                "rt:worker:1",
                "run_failed",
                json!({"identity": unobserved, "error": "provider failed"}),
            ))
            .await;
        assert_eq!(store.response_phase_for_identity("worker").await, None);
        assert!(
            !store
                .state
                .read()
                .await
                .callback_pending_by_identity
                .contains("worker"),
            "the phase clear must not leave a stale callback pause behind"
        );
    }

    /// The same interaction-less terminal must never close another current
    /// owner: neither a tracked typed run nor a legacy active interaction.
    #[tokio::test]
    async fn foreign_interactionless_terminal_preserves_active_run_and_interaction_phase() {
        let store = ConsoleEventStore::new();
        let active = typed_lineage(311, 411);
        let foreign = json!({"run_id": uuid::Uuid::from_u128(412).to_string()});
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "start",
                "rt:worker:1",
                "run_started",
                json!({"identity": active, "input": {"kind": "content", "content": "hello"}}),
            ))
            .await;
        store
            .project_unified_event(&agent_event_with_payload(
                "delta",
                "rt:worker:1",
                "text_delta",
                json!({"delta": "working"}),
            ))
            .await;
        for (id, kind) in [
            ("foreign-failed", "run_failed"),
            ("foreign-done", "run_completed"),
        ] {
            store
                .project_unified_event(&agent_event_with_payload(
                    id,
                    "rt:worker:1",
                    kind,
                    json!({"identity": foreign}),
                ))
                .await;
            assert_eq!(
                store.response_phase_for_identity("worker").await.as_deref(),
                Some("generating")
            );
        }
        store
            .project_unified_event(&agent_event_with_payload(
                "still-current",
                "rt:worker:1",
                "text_delta",
                json!({"delta": "more"}),
            ))
            .await;
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        assert_eq!(
            replay
                .iter()
                .find(|event| event.event_id == "still-current")
                .expect("projected still-current frame")
                .data["identity"],
            active
        );

        // Legacy identity-free start bound by content: an active interaction
        // without a typed run still owns the phase.
        let legacy = ConsoleEventStore::new();
        legacy
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        legacy
            .reserve_interaction_value(
                "worker",
                Some("rt:worker:1"),
                "legacy-turn",
                "console",
                json!("legacy prompt"),
            )
            .await
            .expect("reserve legacy console interaction");
        legacy
            .project_unified_event(&agent_event_with_payload(
                "legacy-start",
                "rt:worker:1",
                "run_started",
                json!({"input": {"kind": "content", "content": "legacy prompt"}}),
            ))
            .await;
        legacy
            .project_unified_event(&agent_event_with_payload(
                "legacy-delta",
                "rt:worker:1",
                "text_delta",
                json!({"delta": "working"}),
            ))
            .await;
        legacy
            .project_unified_event(&agent_event_with_payload(
                "legacy-foreign-failed",
                "rt:worker:1",
                "run_failed",
                json!({"identity": foreign}),
            ))
            .await;
        assert_eq!(
            legacy
                .response_phase_for_identity("worker")
                .await
                .as_deref(),
            Some("generating")
        );
        assert_eq!(
            legacy.state.read().await.active_interaction_by_identity["worker"],
            "legacy-turn"
        );
    }

    /// #469: typed reservations close on their own run's terminal, so a
    /// long-lived identity never reaches the pending cap and never evicts a
    /// settled send with a false `queue_overflow`.
    #[tokio::test]
    async fn typed_reservations_close_on_their_run_terminal_past_pending_cap() {
        let store = ConsoleEventStore::new();
        store
            .register_runtime_identity("rt:worker:1", "worker")
            .await;
        let sends = PENDING_INTERACTION_CAP + 8;
        for index in 0..sends {
            let lineage = typed_lineage(10_000 + index as u128, 20_000 + index as u128);
            let interaction = lineage["interaction_id"]
                .as_str()
                .expect("typed interaction id")
                .to_string();
            store
                .reserve_interaction_value(
                    "worker",
                    Some("rt:worker:1"),
                    &interaction,
                    "meta/send_message",
                    json!(format!("probe {index}")),
                )
                .await
                .expect("reserve typed interaction");
            store
                .project_unified_event(&agent_event_with_payload(
                    &format!("start-{index}"),
                    "rt:worker:1",
                    "run_started",
                    json!({"identity": lineage, "input": {"kind": "content", "content": format!("probe {index}")}}),
                ))
                .await;
            // Alternate settled and failed runs; a failed run has no final
            // turn_completed.
            let terminal = if index % 2 == 0 {
                "run_completed"
            } else {
                "run_failed"
            };
            store
                .project_unified_event(&agent_event_with_payload(
                    &format!("terminal-{index}"),
                    "rt:worker:1",
                    terminal,
                    json!({"identity": lineage}),
                ))
                .await;
            assert!(
                store.pending_interaction_ids("worker").await.is_empty(),
                "reservation {index} must close on its own run terminal"
            );
            assert_eq!(store.response_phase_for_identity("worker").await, None);
        }
        let replay = store
            .replay_all(None)
            .await
            .expect("replay retained console frames");
        assert!(
            !replay
                .iter()
                .any(|event| event.data.get("reason") == Some(&json!("queue_overflow"))),
            "settled reservations must never be evicted as queue overflow"
        );
    }
}

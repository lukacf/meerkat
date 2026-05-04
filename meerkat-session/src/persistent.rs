//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.
//!
//! Runtime projection contract:
//!
//! - In runtime-backed mode, `RuntimeStore` snapshot commits own durable session
//!   lifecycle truth. The `SessionStore` row is a compatibility projection.
//! - `save_normalized_session` rebuilds that projection from the normalized
//!   authoritative snapshot immediately after the runtime snapshot commit.
//! - `read` and `list` must never treat raw `SessionStore` rows as canonical
//!   runtime truth. Listing may use projection rows only as discovery keys and
//!   then rebuild summaries from live/runtime authority.
//! - If the projection update fails after a runtime authority commit, the
//!   caller gets an error and the live handle is discarded. The stale row may
//!   remain in `SessionStore`, but consumers must keep honoring runtime
//!   authority or exclude/fail closed when no authority exists.
//! - If a runtime snapshot commit fails after a direct live mutation, the
//!   caller gets an error and the mutated live handle is discarded before it
//!   can drive reads or listing as clean truth.
//!
//! Runtime-less compatibility mode still persists directly to `SessionStore`.
//! Event-log file projection is separate best-effort derived state.

#![cfg_attr(test, allow(dead_code))]

use async_trait::async_trait;
use futures::StreamExt;
use indexmap::IndexMap;
use meerkat_core::BlobStore;
use meerkat_core::PendingSystemContextAppend;
#[allow(unused_imports)] // Used in read() fallback path
use meerkat_core::Session;
use meerkat_core::SessionSystemContextState;
use meerkat_core::error::AgentError;
use meerkat_core::image_content::externalize_deferred_turn_state;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreApplyTerminal};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    MobToolAuthorityContext, SessionControlError, SessionError, SessionHistoryPage,
    SessionHistoryQuery, SessionInfo, SessionQuery, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionUsage, SessionView,
    StageToolResultsRequest, StageToolResultsResult, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, ToolResult};
use meerkat_core::{DeferredFirstTurnPhase, SessionDeferredTurnState};
use meerkat_core::{InputId, RunId};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::store::SessionDelta;
use meerkat_runtime::{MachineSessionControlAuthority, RuntimeMode, RuntimeState, RuntimeStore};
use meerkat_store::{SessionFilter, SessionStore, SessionStoreError};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};

use crate::SESSION_LABELS_KEY;
use crate::ephemeral::{EphemeralSessionService, SessionAgentBuilder};
use crate::event_store::EventStore;
use crate::projector::SessionProjector;

/// Re-export of the crate-root `migrations` module so the canonical
/// path `meerkat_session::persistent::migrations` (as named in the
/// wave-c plan) resolves unchanged.
pub use crate::migrations;

const SESSION_ARCHIVED_KEY: &str = "session_archived";
const SESSION_TRANSIENT_PENDING_ARCHIVE_KEY: &str = "session_transient_pending_archive";
const DEFAULT_ARCHIVED_HISTORY_CAPACITY: usize = 1024;

fn session_id_from_event(event: &meerkat_core::event::AgentEvent) -> Option<SessionId> {
    match event {
        meerkat_core::event::AgentEvent::RunStarted { session_id, .. }
        | meerkat_core::event::AgentEvent::RunCompleted { session_id, .. }
        | meerkat_core::event::AgentEvent::RunFailed { session_id, .. } => Some(session_id.clone()),
        _ => None,
    }
}

async fn append_and_project_event(
    event_store: &Arc<dyn EventStore>,
    projector: &Arc<SessionProjector>,
    session_id: &SessionId,
    event: meerkat_core::event::AgentEvent,
) {
    let events = [event];
    match event_store.append(session_id, &events).await {
        Ok(seq) => {
            if let Err(error) = projector
                .project(event_store.as_ref(), session_id, seq)
                .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to project persistent session events"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to append persistent session event"
            );
        }
    }
}

async fn flush_projected_events(
    event_store: &Arc<dyn EventStore>,
    projector: &Arc<SessionProjector>,
    session_id: &SessionId,
    pending: &mut Vec<meerkat_core::event::AgentEvent>,
) {
    for event in pending.drain(..) {
        append_and_project_event(event_store, projector, session_id, event).await;
    }
}

async fn project_create_time_events(
    event_store: Arc<dyn EventStore>,
    projector: Arc<SessionProjector>,
    mut projection_rx: mpsc::Receiver<
        meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
    >,
    mut session_rx: watch::Receiver<Option<SessionId>>,
    caller_event_tx: Option<
        mpsc::Sender<meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>>,
    >,
) {
    let mut session_id = session_rx.borrow().clone();
    let mut pending = Vec::new();

    loop {
        tokio::select! {
            envelope = projection_rx.recv() => {
                let Some(envelope) = envelope else {
                    break;
                };
                if session_id.is_none() {
                    session_id = session_id_from_event(&envelope.payload);
                }
                let event = envelope.payload.clone();
                if let Some(tx) = caller_event_tx.as_ref()
                    && tx.send(envelope).await.is_err()
                {
                    tracing::warn!("session event stream receiver dropped; continuing event projection");
                }
                if let Some(session_id) = session_id.as_ref() {
                    pending.push(event);
                    flush_projected_events(&event_store, &projector, session_id, &mut pending).await;
                } else {
                    pending.push(event);
                }
            }
            changed = session_rx.changed(), if session_id.is_none() => {
                if changed.is_err() {
                    break;
                }
                session_id = session_rx.borrow().clone();
                if let Some(session_id) = session_id.as_ref() {
                    flush_projected_events(&event_store, &projector, session_id, &mut pending).await;
                }
            }
        }
    }

    if let Some(session_id) = session_id.as_ref() {
        flush_projected_events(&event_store, &projector, session_id, &mut pending).await;
    }
}

fn write_system_context_state(
    session: &mut Session,
    state: SessionSystemContextState,
) -> Result<(), SessionControlError> {
    session.set_system_context_state(state).map_err(|err| {
        SessionControlError::Session(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize system-context state: {err}"
            )),
        ))
    })
}

fn write_deferred_turn_state(
    session: &mut Session,
    state: SessionDeferredTurnState,
) -> Result<(), SessionControlError> {
    session.set_deferred_turn_state(state).map_err(|err| {
        SessionControlError::Session(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize deferred-turn state: {err}"
            )),
        ))
    })
}

fn rollback_tool_visibility_state_snapshot(
    session: &Session,
) -> Result<Option<meerkat_core::SessionToolVisibilityState>, SessionError> {
    // This production rollback path must not promote legacy tool_scope_* metadata
    // into canonical runtime-backed visibility authority.
    session.try_tool_visibility_state().map_err(|err| {
        SessionError::Agent(AgentError::InternalError(format!(
            "invalid canonical tool visibility state: {err}"
        )))
    })
}

fn control_error_into_session_error(err: SessionControlError) -> SessionError {
    match err {
        SessionControlError::Session(session_err) => session_err,
        other => SessionError::Unsupported(other.to_string()),
    }
}

fn validate_tool_result_video(results: &[ToolResult]) -> Result<(), SessionError> {
    if results.iter().any(ToolResult::has_video) {
        return Err(SessionError::Agent(AgentError::ConfigError(
            "video blocks are not supported in tool results".to_string(),
        )));
    }
    Ok(())
}

/// Shared gate between the checkpointer and archive.
///
/// The `Mutex` provides mutual exclusion so that `checkpoint()` cannot
/// race with `archive()`: both acquire the lock before touching the store,
/// and `archive()` sets `cancelled = true` under the lock before deleting.
struct CheckpointerGate {
    cancelled: Mutex<bool>,
}

#[derive(Clone, Copy)]
enum StoreOnlyArchiveMode {
    Reject,
    MachineAuthority,
}

/// Checkpointer that saves sessions to a [`SessionStore`].
///
/// Used by keep-alive agents to persist the session after each interaction
/// without going through `SessionService::start_turn()`.
///
/// Tracks the message count from the last successful save so that
/// back-to-back checkpoints of an unchanged session are skipped.
/// This avoids redundant writes, especially the first checkpoint
/// after `create_session` which already persists an initial snapshot.
struct StoreCheckpointer {
    store: Arc<dyn SessionStore>,
    blob_store: Arc<dyn BlobStore>,
    gate: Arc<CheckpointerGate>,
    last_saved_len: std::sync::atomic::AtomicUsize,
    enabled: bool,
}

#[async_trait]
impl meerkat_core::checkpoint::SessionCheckpointer for StoreCheckpointer {
    async fn checkpoint(&self, session: &Session) {
        if !self.enabled {
            return;
        }
        let guard = self.gate.cancelled.lock().await;
        if *guard {
            return;
        }
        let current_len = session.messages().len();
        let prev_len = self
            .last_saved_len
            .load(std::sync::atomic::Ordering::Acquire);
        if current_len == prev_len {
            return;
        }
        let mut persisted = session.clone();
        if let Err(e) = persisted
            .externalize_media(self.blob_store.as_ref(), 0)
            .await
        {
            tracing::warn!("Host-mode checkpoint blob externalization failed: {e}");
            return;
        }
        if let Some(mut state) = persisted.deferred_turn_state() {
            if let Err(e) =
                externalize_deferred_turn_state(self.blob_store.as_ref(), &mut state).await
            {
                tracing::warn!("Host-mode checkpoint deferred-turn externalization failed: {e}");
                return;
            }
            if let Err(err) = persisted.set_deferred_turn_state(state) {
                tracing::warn!("Host-mode checkpoint deferred-turn serialization failed: {err}");
                return;
            }
        }
        if let Err(e) = self.store.save(&persisted).await {
            tracing::warn!("Host-mode checkpoint failed: {e}");
        } else {
            self.last_saved_len
                .store(current_len, std::sync::atomic::Ordering::Release);
        }
        drop(guard);
    }
}

/// Session service backed by persistent storage.
///
/// Wraps `EphemeralSessionService` and saves session snapshots to a
/// `SessionStore` after each turn completes. On `list` and `read`,
/// merges live sessions with persisted sessions from the store.
pub struct PersistentSessionService<B: SessionAgentBuilder> {
    inner: EphemeralSessionService<B>,
    store: Arc<dyn SessionStore>,
    runtime_store: Option<Arc<dyn RuntimeStore>>,
    blob_store: Arc<dyn BlobStore>,
    event_store: Option<Arc<dyn EventStore>>,
    projector: Option<Arc<SessionProjector>>,
    /// Process-local bounded cache of archived full sessions to avoid
    /// immediately reloading durable archived snapshots on hot history reads.
    archived_sessions: Mutex<IndexMap<SessionId, Session>>,
    archived_history_capacity: usize,
    /// Gates for active keep-alive checkpointers, keyed by session ID.
    /// Archive acquires the gate's lock, sets cancelled, then saves the
    /// archived snapshot -- mutual exclusion prevents a concurrent checkpoint
    /// from overwriting the archived row with a live one.
    checkpointer_gates: Mutex<HashMap<SessionId, Arc<CheckpointerGate>>>,
    /// Gates lazy live-session recovery and archive against each other so a
    /// stored-only session is rebuilt at most once and archived snapshots
    /// cannot become writable again through rehydration races.
    recovery_gates: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
}

/// Extract session labels from a metadata map.
///
/// Looks for `SESSION_LABELS_KEY` and deserializes the value as
/// `BTreeMap<String, String>`. Returns an empty map on missing or
/// malformed data.
#[allow(dead_code)]
fn extract_labels_from_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, String> {
    match metadata.get(SESSION_LABELS_KEY) {
        Some(v) => match serde_json::from_value::<BTreeMap<String, String>>(v.clone()) {
            Ok(labels) => labels,
            Err(e) => {
                tracing::warn!(
                    key = SESSION_LABELS_KEY,
                    error = %e,
                    "failed to deserialize session labels from metadata"
                );
                BTreeMap::new()
            }
        },
        None => BTreeMap::new(),
    }
}

fn summary_from_meta(meta: meerkat_core::SessionMeta) -> SessionSummary {
    SessionSummary {
        session_id: meta.id,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        message_count: meta.message_count,
        total_tokens: meta.total_tokens,
        is_active: false,
        labels: extract_labels_from_metadata(&meta.metadata),
    }
}

fn view_from_authoritative_session(session: &Session) -> SessionView {
    let metadata = session.session_metadata();
    SessionView {
        state: SessionInfo {
            session_id: session.id().clone(),
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            is_active: false,
            model: metadata
                .as_ref()
                .map(|metadata| metadata.model.clone())
                .unwrap_or_default(),
            provider: metadata
                .as_ref()
                .map(|metadata| metadata.provider)
                .unwrap_or(meerkat_core::Provider::Other),
            last_assistant_text: session.last_assistant_text(),
            labels: extract_labels_from_metadata(session.metadata()),
        },
        billing: SessionUsage {
            total_tokens: session.total_tokens(),
            usage: session.total_usage(),
        },
    }
}

fn metadata_marks_archived(metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
    metadata
        .get(SESSION_ARCHIVED_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn metadata_marks_transient_pending_archive(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    metadata
        .get(SESSION_TRANSIENT_PENDING_ARCHIVE_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn clear_transient_pending_archive_marker(session: &mut Session) {
    session.remove_metadata(SESSION_TRANSIENT_PENDING_ARCHIVE_KEY);
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    fn archived_not_found(id: &SessionId) -> SessionControlError {
        SessionControlError::Session(SessionError::NotFound { id: id.clone() })
    }

    async fn reject_if_archived_session(
        &self,
        id: &SessionId,
        session: &Session,
    ) -> Result<(), SessionControlError> {
        if metadata_marks_archived(session.metadata()) {
            self.remember_archived_session(session.clone()).await;
            return Err(Self::archived_not_found(id));
        }
        Ok(())
    }

    fn runtime_id_for_session(id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::for_session(id)
    }

    fn legacy_runtime_id_for_session(id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::legacy_session_uuid_alias(id)
    }

    fn runtime_id_candidates_for_session(id: &SessionId) -> Vec<LogicalRuntimeId> {
        let canonical = Self::runtime_id_for_session(id);
        let legacy = Self::legacy_runtime_id_for_session(id);
        if canonical == legacy {
            vec![canonical]
        } else {
            vec![canonical, legacy]
        }
    }

    async fn runtime_input_updates(
        &self,
        id: &SessionId,
        run_id: &RunId,
        sequence: u64,
        contributing_input_ids: &[InputId],
    ) -> Result<Vec<StoredInputState>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(Vec::new());
        };
        let mut stored_states: Vec<(usize, StoredInputState)> = Vec::new();
        let mut primary_alias_loaded = false;
        for (candidate_index, runtime_id) in Self::runtime_id_candidates_for_session(id)
            .into_iter()
            .enumerate()
        {
            let candidate_states = match runtime_store.load_input_states(&runtime_id).await {
                Ok(states) => {
                    if candidate_index == 0 {
                        primary_alias_loaded = true;
                    }
                    states
                }
                Err(err)
                    if candidate_index > 0
                        && primary_alias_loaded
                        && contributing_input_ids.iter().all(|input_id| {
                            stored_states
                                .iter()
                                .any(|(_, state)| &state.state.input_id == input_id)
                        }) =>
                {
                    tracing::warn!(
                        session_id = %id,
                        runtime_id = %runtime_id,
                        error = %err,
                        "ignoring legacy runtime input-state fallback error because canonical input states were already loaded"
                    );
                    continue;
                }
                Err(err) => {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "failed to load runtime input states: {err}"
                        )),
                    ));
                }
            };
            for state in candidate_states {
                let input_id = state.state.input_id.clone();
                if let Some((existing_index, existing_state)) = stored_states
                    .iter_mut()
                    .find(|(_, existing)| existing.state.input_id == input_id)
                {
                    let candidate_updated_at = state.state.updated_at();
                    let existing_updated_at = existing_state.state.updated_at();
                    let should_replace = if candidate_index == *existing_index {
                        candidate_updated_at > existing_updated_at
                    } else {
                        candidate_index < *existing_index
                    };
                    if should_replace {
                        *existing_index = candidate_index;
                        *existing_state = state;
                    }
                } else {
                    stored_states.push((candidate_index, state));
                }
            }
        }

        Ok(contributing_input_ids
            .iter()
            .filter_map(|input_id| {
                let mut bundle = stored_states
                    .iter()
                    .find(|(_, candidate)| &candidate.state.input_id == input_id)?
                    .1
                    .clone();
                // Stamp receipt metadata and mirror the Consumed terminal on
                // the persisted snapshot. The authoritative DSL transition
                // fires on the live driver via `machine_realize_run_completed`;
                // this clone is only what the store persists alongside the
                // boundary receipt, so `updated_at` tracks that receipt's
                // logical moment rather than wall-clock.
                bundle.seed.last_run_id = Some(run_id.clone());
                bundle.seed.last_boundary_sequence = Some(sequence);
                bundle.seed.phase = InputLifecycleState::Consumed;
                bundle.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                bundle.state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                Some(bundle)
            })
            .collect())
    }

    async fn commit_runtime_apply(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        session: &Session,
        session_snapshot: &[u8],
        contributing_input_ids: &[InputId],
    ) -> Result<RunBoundaryReceipt, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            let persisted = self.save_normalized_session(session.clone()).await?;
            return Self::build_runtime_receipt(
                run_id,
                boundary,
                contributing_input_ids.to_vec(),
                &persisted,
            );
        };
        let runtime_id = Self::runtime_id_for_session(id);
        let input_updates = self
            .runtime_input_updates(id, &run_id, 0, contributing_input_ids)
            .await?;
        let receipt = match runtime_store
            .commit_session_boundary(
                &runtime_id,
                SessionDelta {
                    session_snapshot: session_snapshot.to_vec(),
                },
                run_id.clone(),
                boundary,
                contributing_input_ids.to_vec(),
                input_updates,
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(err) => {
                let _ = self.discard_live_session(id).await;
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "runtime boundary commit failed: {err}"
                    )),
                ));
            }
        };

        Ok(receipt)
    }

    async fn export_session_with_labels(&self, id: &SessionId) -> Result<Session, SessionError> {
        let mut session = self.inner.export_session(id).await?;
        if let Ok(view) = self.inner.read(id).await
            && !view.state.labels.is_empty()
            && let Ok(labels_value) = serde_json::to_value(&view.state.labels)
        {
            session.set_metadata(SESSION_LABELS_KEY, labels_value);
        }
        Ok(session)
    }

    /// Apply a runtime-turn metadata LLM identity update to a live session.
    ///
    /// This is intentionally not exposed through the public
    /// `SessionService::hot_swap_session_llm_identity` trait method: callers
    /// must arrive here through the runtime-owned turn/reconfigure path, after
    /// the runtime has resolved the typed metadata and built the client.
    pub async fn apply_runtime_session_llm_identity(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.inner
            .hot_swap_session_llm_identity(id, client, identity, request_policy)
            .await?;
        self.persist_full_session_or_discard_live(id)
            .await
            .map(|_| ())
    }

    /// Apply a runtime-turn metadata keep-alive update to a live session.
    ///
    /// The public trait method remains blocked so keep-alive changes do not
    /// bypass the runtime turn metadata seam.
    pub async fn apply_runtime_session_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let previous = self
            .export_session_with_labels(id)
            .await
            .ok()
            .and_then(|session| session.session_metadata().map(|meta| meta.keep_alive));
        self.inner.update_session_keep_alive(id, keep_alive).await?;
        match self.persist_full_session(id).await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(previous) = previous {
                    let _ = self.inner.update_session_keep_alive(id, previous).await;
                }
                Err(error)
            }
        }
    }

    pub async fn mark_transient_pending_archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let Some(mut session) = self.load_authoritative_session_base(id).await? else {
            return Ok(());
        };
        if !metadata_marks_archived(session.metadata()) {
            return Ok(());
        }
        session.set_metadata(
            SESSION_TRANSIENT_PENDING_ARCHIVE_KEY,
            serde_json::Value::Bool(true),
        );
        let session = self.save_normalized_session(session).await?;
        self.remember_archived_session(session).await;
        Ok(())
    }

    async fn load_authoritative_session_base(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        if let Some(runtime_store) = self.runtime_store.as_ref() {
            Self::load_runtime_session_snapshot_for_session(runtime_store, id).await
        } else {
            self.store
                .load(id)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))
        }
    }

    async fn load_runtime_session_snapshot(
        runtime_store: &Arc<dyn RuntimeStore>,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Session>, SessionError> {
        runtime_store
            .load_session_snapshot(runtime_id)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to load runtime session snapshot: {err}"
                )))
            })?
            .map(|bytes| {
                meerkat_core::session_migrations::deserialize_session_migrating(&bytes).map_err(
                    |err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("failed to deserialize runtime session snapshot: {err}"),
                        ))
                    },
                )
            })
            .transpose()
    }

    async fn load_runtime_session_snapshot_for_session(
        runtime_store: &Arc<dyn RuntimeStore>,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let mut selected = None;
        for (candidate_index, runtime_id) in Self::runtime_id_candidates_for_session(id)
            .into_iter()
            .enumerate()
        {
            match Self::load_runtime_session_snapshot(runtime_store, &runtime_id).await {
                Ok(Some(session)) => {
                    if candidate_index == 0 {
                        return Ok(Some(session));
                    }
                    selected = Some(session);
                }
                Ok(None) => {}
                Err(err) if candidate_index > 0 && selected.is_some() => {
                    tracing::warn!(
                        session_id = %id,
                        runtime_id = %runtime_id,
                        error = ?err,
                        "ignoring legacy runtime session snapshot fallback error because canonical authority was already loaded"
                    );
                }
                Err(err) => return Err(err),
            }
        }
        Ok(selected)
    }

    async fn load_runtime_state_for_session(
        runtime_store: &Arc<dyn RuntimeStore>,
        id: &SessionId,
    ) -> Result<Option<RuntimeState>, SessionError> {
        let mut selected = None;
        for (candidate_index, runtime_id) in Self::runtime_id_candidates_for_session(id)
            .into_iter()
            .enumerate()
        {
            let loaded = runtime_store
                .load_runtime_state(&runtime_id)
                .await
                .map_err(|err| {
                    SessionError::Agent(AgentError::InternalError(format!(
                        "failed to load runtime state: {err}"
                    )))
                });
            match loaded {
                Ok(Some(state)) => {
                    if candidate_index == 0 {
                        return Ok(Some(state));
                    }
                    selected = Some(state);
                }
                Ok(None) => {}
                Err(err) if candidate_index > 0 && selected.is_some() => {
                    tracing::warn!(
                        session_id = %id,
                        runtime_id = %runtime_id,
                        error = ?err,
                        "ignoring legacy runtime state fallback error because canonical authority was already loaded"
                    );
                }
                Err(err) => return Err(err),
            }
        }
        Ok(selected)
    }

    fn store_only_control_mutation_error(id: &SessionId, operation: &str) -> SessionError {
        SessionError::Unsupported(format!(
            "{operation} cannot mutate store-only compatibility projection for session {id}; session control mutations require an authoritative runtime/session machine snapshot"
        ))
    }

    async fn load_runtime_authority_session_for_control(
        &self,
        id: &SessionId,
        operation: &str,
    ) -> Result<Option<Session>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(None);
        };

        if let Some(runtime) =
            Self::load_runtime_session_snapshot_for_session(runtime_store, id).await?
        {
            return Ok(Some(runtime));
        }

        if self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
        {
            return Err(Self::store_only_control_mutation_error(id, operation));
        }

        Ok(None)
    }

    async fn load_persisted_session_for_control(
        &self,
        id: &SessionId,
        operation: &str,
    ) -> Result<Option<Session>, SessionError> {
        if self.runtime_store.is_some() {
            return self
                .load_runtime_authority_session_for_control(id, operation)
                .await;
        }

        if self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
        {
            return Err(Self::store_only_control_mutation_error(id, operation));
        }

        Ok(None)
    }

    async fn load_machine_authority_session_for_control(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        if let Some(runtime_store) = self.runtime_store.as_ref()
            && let Some(runtime) =
                Self::load_runtime_session_snapshot_for_session(runtime_store, id).await?
        {
            return Ok(Some(runtime));
        }

        self.store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))
    }

    async fn discard_stale_live_session_if_needed(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        let live = match self.export_live_session(id).await {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => return Ok(false),
            Err(err) => return Err(err),
        };

        let stored = if let Some(runtime_store) = self.runtime_store.as_ref() {
            let Some(runtime) =
                Self::load_runtime_session_snapshot_for_session(runtime_store, id).await?
            else {
                return Ok(false);
            };
            runtime
        } else {
            let Some(stored) = self
                .store
                .load(id)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?
            else {
                return Ok(false);
            };
            stored
        };

        let stored_has_more_transcript = stored.messages().len() > live.messages().len();
        let stored_is_archived = metadata_marks_archived(stored.metadata());

        // A durable snapshot timestamp is a projection witness, not live-session
        // authority. Runtime commits can normalize/persist an equivalent
        // transcript a few microseconds after the live session updates itself;
        // evicting the live handle on timestamp alone drops mechanical runtime
        // resources such as comms. Only terminal archive state or a strictly
        // longer durable transcript can evict the live session.
        if !stored_has_more_transcript && !stored_is_archived {
            return Ok(false);
        }

        tracing::debug!(
            session_id = %id,
            live_updated_at = ?live.updated_at(),
            stored_updated_at = ?stored.updated_at(),
            live_message_count = live.messages().len(),
            stored_message_count = stored.messages().len(),
            stored_is_archived,
            "discarding stale live session in favor of newer durable session-store snapshot"
        );
        self.discard_live_session(id).await?;
        Ok(true)
    }

    async fn recover_live_session_from_store_if_needed_locked(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        if self.inner.has_live_session(id).await? {
            return Ok(false);
        }

        let Some(stored) = self.load_authoritative_session_base(id).await? else {
            return Ok(false);
        };
        self.reject_if_archived_session(id, &stored)
            .await
            .map_err(control_error_into_session_error)?;

        let _ = stored;
        Err(SessionError::Agent(AgentError::InternalError(
            "stored-session recovery via non-canonical runtime-binding providers has been deleted; callers must materialize sessions through the canonical runtime-binding seam".to_string(),
        )))
    }

    pub async fn export_live_session(&self, id: &SessionId) -> Result<Session, SessionError> {
        self.export_session_with_labels(id).await
    }

    pub async fn wait_for_session_mutation_after(
        &self,
        id: &SessionId,
        after: std::time::SystemTime,
    ) -> Result<std::time::SystemTime, meerkat_core::comms::StreamError> {
        self.inner.wait_for_session_mutation_after(id, after).await
    }

    pub async fn execution_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SessionError> {
        self.inner.execution_snapshot(id).await
    }

    pub async fn tool_scope_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        self.inner.tool_scope_snapshot(id).await
    }

    pub async fn live_visible_tool_defs(
        &self,
        id: &SessionId,
    ) -> Result<Vec<meerkat_core::ToolDef>, SessionError> {
        self.inner.live_visible_tool_defs(id).await
    }

    pub async fn external_tool_surface_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
        self.inner.external_tool_surface_snapshot(id).await
    }

    pub async fn live_session_llm_identity(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::SessionLlmIdentity, SessionError> {
        self.inner.live_session_llm_identity(id).await
    }

    pub async fn discard_live_session(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(id).await?;
        self.checkpointer_gates.lock().await.remove(id);
        Ok(())
    }

    pub async fn persist_live_session_now(&self, id: &SessionId) -> Result<usize, SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.persist_full_session(id).await
    }

    pub async fn dispatch_external_tool_call(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        self.dispatch_external_tool_call_with_timeout_policy(
            id,
            call,
            meerkat_core::ToolDispatchTimeoutPolicy::Disabled,
        )
        .await
    }

    pub async fn dispatch_external_tool_call_with_timeout_policy(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let outcome = self
            .inner
            .dispatch_external_tool_call_with_timeout_policy(id, call, timeout_policy)
            .await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(outcome)
    }

    pub async fn append_external_user_content(
        &self,
        id: &SessionId,
        content: meerkat_core::types::ContentInput,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.inner.append_external_user_content(id, content).await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn append_external_assistant_output(
        &self,
        id: &SessionId,
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: meerkat_core::types::Usage,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        self.inner
            .append_external_assistant_output(id, blocks, stop_reason, usage)
            .await?;
        if let Err(error) = self.persist_full_session(id).await {
            let _ = self.discard_live_session(id).await;
            return Err(error);
        }
        Ok(())
    }

    /// Create a new persistent session service.
    pub fn new(
        builder: B,
        max_sessions: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_archived_history_capacity(
            builder,
            max_sessions,
            DEFAULT_ARCHIVED_HISTORY_CAPACITY,
            store,
            runtime_store,
            blob_store,
        )
    }

    /// Create a persistent session service with separate active-session and
    /// archived-history cache capacities.
    pub fn new_with_archived_history_capacity(
        builder: B,
        max_sessions: usize,
        archived_history_capacity: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_capacities(
            builder,
            max_sessions,
            archived_history_capacity,
            store,
            runtime_store,
            blob_store,
        )
    }

    /// Create a persistent session service with explicit active-session and
    /// archived-history cache capacities.
    pub fn new_with_capacities(
        builder: B,
        active_session_capacity: usize,
        archived_history_capacity: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, active_session_capacity),
            store,
            runtime_store,
            blob_store,
            event_store: None,
            projector: None,
            archived_sessions: Mutex::new(IndexMap::new()),
            archived_history_capacity: archived_history_capacity.max(1),
            checkpointer_gates: Mutex::new(HashMap::new()),
            recovery_gates: Mutex::new(HashMap::new()),
        }
    }

    /// Attach the append-only event log and derived file projector used by
    /// persistent sessions.
    ///
    /// Projection is best-effort derived state. Append/project failures are
    /// logged and do not fail the session turn; callers must continue to treat
    /// `SessionStore`/`RuntimeStore` as the source of truth, not the projected
    /// JSONL files. The spawned projection tasks exit when their session event
    /// streams close, and create-time events without a correlated session id are
    /// discarded when the create-time stream closes.
    pub fn with_event_projection(
        mut self,
        event_store: Arc<dyn EventStore>,
        projector: Arc<SessionProjector>,
    ) -> Self {
        self.event_store = Some(event_store);
        self.projector = Some(projector);
        self
    }

    pub fn runtime_mode(&self) -> RuntimeMode {
        RuntimeMode::V9Compliant
    }

    pub fn runtime_store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.runtime_store.clone()
    }

    pub async fn persisted_runtime_state(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeState>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(None);
        };
        Self::load_runtime_state_for_session(runtime_store, id).await
    }

    pub fn has_event_projection(&self) -> bool {
        self.event_store.is_some() && self.projector.is_some()
    }

    pub fn ensure_active_capacity_available(&self) -> Result<(), SessionError> {
        self.inner.ensure_active_capacity_available()
    }

    pub async fn reserve_create_session_admission(
        &self,
    ) -> Result<crate::ephemeral::RuntimeContextAdmissionGuard, SessionError> {
        self.inner.acquire_runtime_capacity_admission().await
    }

    pub async fn reserve_runtime_turn_admission(
        &self,
        id: &SessionId,
    ) -> Result<crate::ephemeral::RuntimeContextAdmissionGuard, SessionError> {
        match self.inner.join_active_runtime_context_admission(id).await {
            Ok(Some(admission)) => return Ok(admission),
            Ok(None) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error),
        }

        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        match self.inner.acquire_runtime_context_admission(id).await {
            Ok(admission) => Ok(admission),
            Err(SessionError::NotFound { .. }) => {
                if self.load_authoritative_session_base(id).await?.is_some() {
                    self.inner.acquire_runtime_capacity_admission().await
                } else {
                    Err(SessionError::NotFound { id: id.clone() })
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn create_session_with_reserved_admission(
        &self,
        req: CreateSessionRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(req, Some(admission))
            .await
    }

    pub async fn start_turn_with_reserved_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_with_recoverable_reserved_admission(id, req, admission)
            .await
            .map_err(|(error, _admission)| error)
    }

    pub async fn start_turn_with_recoverable_reserved_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        RunResult,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        self.start_turn_inner_with_admission(id, req, Some(admission))
            .await
    }

    async fn start_turn_inner_with_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        mut admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<
        RunResult,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self
            .discard_stale_live_session_if_needed(id)
            .await
            .map_err(|error| (error, admission.take()))?;
        let _ = self
            .recover_live_session_from_store_if_needed_locked(id)
            .await
            .map_err(|error| (error, admission.take()))?;
        let result = match admission.take() {
            Some(admission) => {
                self.inner
                    .start_turn_with_runtime_context_admission_recovering_not_found(
                        id, req, admission,
                    )
                    .await
            }
            None => self
                .inner
                .start_turn(id, req)
                .await
                .map_err(|error| (error, None)),
        };
        let result = match result {
            Ok(result) => result,
            Err((error, admission)) => {
                if Self::callback_pending_terminal(&error).is_some()
                    && let Err(persist_error) = self.persist_full_session_or_discard_live(id).await
                {
                    return Err((persist_error, admission));
                }
                return Err((error, admission));
            }
        };

        // Always persist after a direct start_turn call. Runtime-backed sessions
        // that go through apply_runtime_turn() have their own atomic boundary
        // commit path and don't call start_turn on PersistentSessionService.
        let _ = self
            .persist_full_session_or_discard_live(id)
            .await
            .map_err(|error| (error, None))?;

        Ok(result)
    }

    pub async fn event_log_latest_seq(&self, id: &SessionId) -> Result<Option<u64>, SessionError> {
        let Some(event_store) = self.event_store.as_ref() else {
            return Ok(None);
        };
        event_store
            .last_seq(id)
            .await
            .map(Some)
            .map_err(|err| SessionError::Store(Box::new(err)))
    }

    pub async fn event_log_read_from(
        &self,
        id: &SessionId,
        from_seq: u64,
    ) -> Result<Option<Vec<crate::event_store::StoredEvent>>, SessionError> {
        let Some(event_store) = self.event_store.as_ref() else {
            return Ok(None);
        };
        event_store
            .read_from(id, from_seq)
            .await
            .map(Some)
            .map_err(|err| SessionError::Store(Box::new(err)))
    }

    pub fn blob_store(&self) -> Arc<dyn BlobStore> {
        self.blob_store.clone()
    }

    fn install_create_time_event_projection(
        &self,
        req: &mut CreateSessionRequest,
    ) -> Option<watch::Sender<Option<SessionId>>> {
        let (Some(event_store), Some(projector)) =
            (self.event_store.clone(), self.projector.clone())
        else {
            return None;
        };

        let caller_event_tx = req.event_tx.take();
        let (projection_tx, projection_rx) = mpsc::channel(128);
        let (session_tx, session_rx) = watch::channel(None);
        req.event_tx = Some(projection_tx);

        tokio::spawn(project_create_time_events(
            event_store,
            projector,
            projection_rx,
            session_rx,
            caller_event_tx,
        ));

        Some(session_tx)
    }

    async fn spawn_event_projection_task(&self, id: &SessionId) {
        let (Some(event_store), Some(projector)) =
            (self.event_store.clone(), self.projector.clone())
        else {
            return;
        };
        let session_id = id.clone();
        let stream = self.inner.subscribe_session_events(&session_id).await;
        let Ok(mut stream) = stream else {
            return;
        };

        tokio::spawn(async move {
            while let Some(envelope) = stream.next().await {
                append_and_project_event(&event_store, &projector, &session_id, envelope.payload)
                    .await;
            }
        });
    }

    async fn normalized_session_for_persistence(
        &self,
        mut session: Session,
    ) -> Result<Session, SessionError> {
        if !metadata_marks_archived(session.metadata()) {
            clear_transient_pending_archive_marker(&mut session);
        }
        session
            .externalize_media(self.blob_store.as_ref(), 0)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to externalize session images for persistence: {err}"
                )))
            })?;
        if let Some(mut state) = session.deferred_turn_state() {
            externalize_deferred_turn_state(self.blob_store.as_ref(), &mut state)
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to externalize deferred-turn images for persistence: {err}"
                    )))
                })?;
            session.set_deferred_turn_state(state).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize deferred-turn state for persistence: {err}"
                )))
            })?;
        }
        Ok(session)
    }

    async fn save_normalized_session(&self, session: Session) -> Result<Session, SessionError> {
        let session = self.normalized_session_for_persistence(session).await?;
        if let Some(runtime_store) = self.runtime_store.as_ref() {
            let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to serialize session snapshot for runtime persistence: {err}"
                )))
            })?;
            runtime_store
                .commit_session_snapshot(
                    &Self::runtime_id_for_session(session.id()),
                    SessionDelta { session_snapshot },
                )
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "runtime snapshot persistence failed: {err}"
                    )))
                })?;
            if let Err(error) = self.store.save(&session).await {
                return Err(self
                    .fail_closed_runtime_projection_update(session.id(), error)
                    .await);
            }
        } else {
            self.store
                .save(&session)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?;
        }
        Ok(session)
    }

    async fn fail_closed_runtime_projection_update(
        &self,
        id: &SessionId,
        error: SessionStoreError,
    ) -> SessionError {
        tracing::error!(
            session_id = %id,
            error = %error,
            "session-store projection update failed after runtime authority commit; failing closed"
        );
        match self.discard_live_session(id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(discard_error) => {
                tracing::warn!(
                    session_id = %id,
                    error = %discard_error,
                    "failed to discard live session after runtime-backed projection update failure"
                );
            }
        }
        SessionError::Store(Box::new(error))
    }

    async fn gate_for_session(&self, id: &SessionId) -> Arc<CheckpointerGate> {
        let mut gates = self.checkpointer_gates.lock().await;
        Arc::clone(gates.entry(id.clone()).or_insert_with(|| {
            Arc::new(CheckpointerGate {
                cancelled: Mutex::new(false),
            })
        }))
    }

    async fn existing_gate_for_session(&self, id: &SessionId) -> Option<Arc<CheckpointerGate>> {
        let gates = self.checkpointer_gates.lock().await;
        gates.get(id).cloned()
    }

    async fn recovery_gate_for_session(&self, id: &SessionId) -> Arc<Mutex<()>> {
        let mut gates = self.recovery_gates.lock().await;
        Arc::clone(
            gates
                .entry(id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn live_persist_mutation_guard(
        &self,
        id: &SessionId,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, SessionError> {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let guard = recovery_gate.lock_owned().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        Ok(guard)
    }

    async fn cached_archived_session(&self, id: &SessionId) -> Option<Session> {
        let cached = self.archived_sessions.lock().await;
        cached.get(id).cloned()
    }

    async fn remember_archived_session(&self, session: Session) {
        let mut cached = self.archived_sessions.lock().await;
        let session_id = session.id().clone();
        cached.shift_remove(&session_id);
        cached.insert(session_id, session);
        while cached.len() > self.archived_history_capacity {
            let _ = cached.shift_remove_index(0);
        }
    }

    async fn forget_archived_session(&self, id: &SessionId) {
        self.archived_sessions.lock().await.shift_remove(id);
    }

    fn build_runtime_receipt(
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        session: &Session,
    ) -> Result<RunBoundaryReceipt, SessionError> {
        let encoded_messages = serde_json::to_vec(session.messages()).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session for runtime receipt digest: {err}"
            )))
        })?;
        let digest = format!("{:x}", Sha256::digest(encoded_messages));

        Ok(RunBoundaryReceipt {
            run_id,
            boundary,
            contributing_input_ids,
            conversation_digest: Some(digest),
            message_count: session.messages().len(),
            sequence: 0,
        })
    }

    fn callback_pending_terminal(error: &SessionError) -> Option<CoreApplyTerminal> {
        match error {
            SessionError::Agent(AgentError::CallbackPending { tool_name, args }) => {
                Some(CoreApplyTerminal::CallbackPending {
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                })
            }
            _ => None,
        }
    }

    async fn build_runtime_output(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        terminal: Option<CoreApplyTerminal>,
        committed_context_events: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let persisted_session = self
            .normalized_session_for_persistence(session.clone())
            .await?;
        let session_snapshot = serde_json::to_vec(&persisted_session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;

        let receipt = self
            .commit_runtime_apply(
                id,
                run_id,
                boundary,
                &persisted_session,
                &session_snapshot,
                &contributing_input_ids,
            )
            .await?;

        if !committed_context_events.is_empty()
            && let Err(error) = self
                .inner
                .publish_runtime_system_context_events(id, committed_context_events)
                .await
        {
            tracing::warn!(
                session_id = %id,
                error = %error,
                "failed to publish committed runtime system-context lifecycle events"
            );
        }

        let output = match terminal {
            Some(CoreApplyTerminal::RunResult(run_result)) => {
                CoreApplyOutput::with_run_result(receipt, Some(session_snapshot), run_result)
            }
            Some(CoreApplyTerminal::CallbackPending { tool_name, args }) => {
                CoreApplyOutput::with_callback_pending(
                    receipt,
                    Some(session_snapshot),
                    tool_name,
                    args,
                )
            }
            Some(CoreApplyTerminal::NoPendingBoundary) => CoreApplyOutput {
                receipt,
                session_snapshot: Some(session_snapshot),
                terminal: Some(CoreApplyTerminal::NoPendingBoundary),
            },
            None => CoreApplyOutput::without_terminal(receipt, Some(session_snapshot)),
        };

        Ok(output)
    }

    async fn build_runtime_output_after_live_mutation(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        terminal: Option<CoreApplyTerminal>,
        committed_context_events: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreApplyOutput, SessionError> {
        match self
            .build_runtime_output(
                id,
                run_id,
                boundary,
                contributing_input_ids,
                terminal,
                committed_context_events,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(error) => {
                if let Err(discard_error) = self.discard_live_session(id).await {
                    tracing::warn!(
                        session_id = %id,
                        error = %discard_error,
                        "failed to discard live session after runtime output build failure"
                    );
                }
                Err(error)
            }
        }
    }

    async fn build_runtime_context_output(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let persisted_session = self
            .normalized_session_for_persistence(session.clone())
            .await?;
        let session_snapshot = serde_json::to_vec(&persisted_session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;

        let receipt = self
            .commit_runtime_apply(
                id,
                run_id,
                boundary,
                &persisted_session,
                &session_snapshot,
                &contributing_input_ids,
            )
            .await?;

        Ok(CoreApplyOutput::without_terminal(
            receipt,
            Some(session_snapshot),
        ))
    }

    async fn build_runtime_context_output_after_live_mutation(
        &self,
        id: &SessionId,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        match self
            .build_runtime_context_output(id, run_id, boundary, contributing_input_ids)
            .await
        {
            Ok(output) => Ok(output),
            Err(error) => {
                if let Err(discard_error) = self.discard_live_session(id).await {
                    tracing::warn!(
                        session_id = %id,
                        error = %discard_error,
                        "failed to discard live session after runtime context output build failure"
                    );
                }
                Err(error)
            }
        }
    }

    /// Apply a runtime-driven turn and return the authoritative boundary receipt.
    ///
    /// In runtime-backed mode, the serialized session snapshot is committed
    /// before this method returns, making the runtime store the sole durable
    /// writer for that turn.
    pub async fn apply_runtime_turn(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_turn_outcome(id, run_id, req, boundary, contributing_input_ids)
            .await
    }

    pub async fn apply_runtime_turn_outcome(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_turn_outcome_with_admission(
            id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
            None,
        )
        .await
    }

    pub async fn apply_runtime_turn_with_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_turn_with_recoverable_reserved_admission(
            id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
        .map_err(|(error, _admission)| error)
    }

    pub async fn apply_runtime_turn_with_recoverable_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        CoreApplyOutput,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        Self::require_runtime_execution_kind_stamp(&req).map_err(|error| (error, None))?;
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self
            .discard_stale_live_session_if_needed(id)
            .await
            .map_err(|error| (error, None))?;
        let pre_turn_context_events = req.pre_turn_context_appends.clone();
        let start_turn_result = self
            .inner
            .start_turn_with_runtime_context_admission_recovering_not_found(id, req, admission)
            .await;
        match start_turn_result {
            Ok(run_result) => self
                .build_runtime_output_after_live_mutation(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(run_result)),
                    pre_turn_context_events,
                )
                .await
                .map_err(|error| (error, None)),
            Err((
                SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary),
                _admission,
            )) => self
                .build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::NoPendingBoundary),
                    Vec::new(),
                )
                .await
                .map_err(|error| (error, None)),
            Err((error @ SessionError::NotFound { .. }, admission)) if admission.is_some() => {
                Err((error, admission))
            }
            Err((error, _admission)) => {
                if let Some(terminal) = Self::callback_pending_terminal(&error) {
                    self.build_runtime_output_after_live_mutation(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(terminal),
                        pre_turn_context_events,
                    )
                    .await
                    .map_err(|error| (error, None))
                } else {
                    if let Err(discard_error) = self.discard_live_session(id).await {
                        tracing::warn!(
                            session_id = %id,
                            error = %discard_error,
                            "failed to discard live session after failed runtime turn"
                        );
                    }
                    Err((error, None))
                }
            }
        }
    }

    async fn apply_runtime_turn_outcome_with_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Self::require_runtime_execution_kind_stamp(&req)?;
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        let pre_turn_context_events = req.pre_turn_context_appends.clone();
        let start_turn_result = match admission {
            Some(admission) => {
                self.inner
                    .start_turn_with_runtime_context_admission(id, req, admission)
                    .await
            }
            None => self.inner.start_turn(id, req).await,
        };
        match start_turn_result {
            Ok(run_result) => {
                self.build_runtime_output_after_live_mutation(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(run_result)),
                    pre_turn_context_events,
                )
                .await
            }
            Err(SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary)) => {
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::NoPendingBoundary),
                    Vec::new(),
                )
                .await
            }
            Err(error) => {
                if let Some(terminal) = Self::callback_pending_terminal(&error) {
                    self.build_runtime_output_after_live_mutation(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(terminal),
                        pre_turn_context_events,
                    )
                    .await
                } else {
                    if let Err(discard_error) = self.discard_live_session(id).await {
                        tracing::warn!(
                            session_id = %id,
                            error = %discard_error,
                            "failed to discard live session after failed runtime turn"
                        );
                    }
                    Err(error)
                }
            }
        }
    }

    fn require_runtime_execution_kind_stamp(req: &StartTurnRequest) -> Result<(), SessionError> {
        if req
            .turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.execution_kind)
            .is_some()
        {
            return Ok(());
        }

        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime_execution_kind not set: runtime-backed turn did not stamp RuntimeTurnMetadata.execution_kind"
                    .to_string(),
            ),
        ))
    }

    pub async fn apply_runtime_context_appends(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_boundary(
            id,
            run_id,
            appends,
            RunApplyBoundary::Immediate,
            contributing_input_ids,
        )
        .await
    }

    /// Apply runtime-owned system context to the live session before a
    /// reaction turn. The subsequent runtime turn commit owns durability and
    /// lifecycle for the combined context+run operation.
    pub async fn apply_runtime_system_context_for_turn(
        &self,
        id: &SessionId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        self.inner.apply_runtime_system_context(id, appends).await
    }

    pub async fn apply_runtime_context_appends_with_boundary(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_admission(
            id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            None,
        )
        .await
    }

    pub async fn apply_runtime_context_appends_with_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<CoreApplyOutput, SessionError> {
        self.apply_runtime_context_appends_with_recoverable_reserved_admission(
            id,
            run_id,
            appends,
            boundary,
            contributing_input_ids,
            admission,
        )
        .await
        .map_err(|(error, _admission)| error)
    }

    pub async fn apply_runtime_context_appends_with_recoverable_reserved_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: crate::ephemeral::RuntimeContextAdmissionGuard,
    ) -> Result<
        CoreApplyOutput,
        (
            SessionError,
            Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
        ),
    > {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self
            .discard_stale_live_session_if_needed(id)
            .await
            .map_err(|error| (error, None))?;
        if let Some(session) = self
            .load_authoritative_session_base(id)
            .await
            .map_err(|error| (error, None))?
        {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)
                .map_err(|error| (error, None))?;
        }
        let mut active_guard = Some(admission);
        if let Err(error) = self
            .inner
            .apply_runtime_system_context(id, appends.clone())
            .await
        {
            let admission = if matches!(error, SessionError::NotFound { .. }) {
                active_guard.take()
            } else {
                None
            };
            return Err((error, admission));
        }

        self.build_runtime_context_output_after_live_mutation(
            id,
            run_id,
            boundary,
            contributing_input_ids,
        )
        .await
        .map_err(|error| (error, None))
    }

    async fn apply_runtime_context_appends_with_admission(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _turn_guard = recovery_gate.lock().await;
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        if let Some(session) = self.load_authoritative_session_base(id).await? {
            self.reject_if_archived_session(id, &session)
                .await
                .map_err(control_error_into_session_error)?;
        }
        let _active_guard = match admission {
            Some(admission) => admission,
            None => self.inner.acquire_runtime_context_admission(id).await?,
        };

        self.inner
            .apply_runtime_system_context(id, appends.clone())
            .await?;

        self.build_runtime_context_output_after_live_mutation(
            id,
            run_id,
            boundary,
            contributing_input_ids,
        )
        .await
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    async fn create_session_with_admission(
        &self,
        mut req: CreateSessionRequest,
        reserved_create_admission: Option<crate::ephemeral::RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, SessionError> {
        // Inject a checkpointer for all sessions. The keep-alive attached loop
        // calls it after each interaction; runtime-backed sessions keep it
        // disabled so they do not bypass runtime boundary persistence.
        let gate = Arc::new(CheckpointerGate {
            cancelled: Mutex::new(false),
        });
        let checkpointer = Arc::new(StoreCheckpointer {
            store: Arc::clone(&self.store),
            blob_store: Arc::clone(&self.blob_store),
            gate: Arc::clone(&gate),
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
            enabled: self.runtime_store.is_none(),
        });
        let resume_session_id = {
            let build = req.build.get_or_insert_with(Default::default);
            if let Some(session) = build.resume_session.as_ref()
                && metadata_marks_archived(session.metadata())
            {
                return Err(SessionError::NotFound {
                    id: session.id().clone(),
                });
            }
            let resume_session_id = build
                .resume_session
                .as_ref()
                .map(|session| session.id().clone());
            build.checkpointer = Some(checkpointer.clone());
            build.blob_store_override = Some(Arc::clone(&self.blob_store));
            resume_session_id
        };
        let resume_session_is_transient_pending_archive = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .is_some_and(|session| metadata_marks_transient_pending_archive(session.metadata()));
        let _resume_recovery_guard = if let Some(resume_session_id) = resume_session_id.as_ref() {
            let recovery_gate = self.recovery_gate_for_session(resume_session_id).await;
            let guard = recovery_gate.lock_owned().await;
            if let Some(session) = self
                .load_authoritative_session_base(resume_session_id)
                .await?
                && metadata_marks_archived(session.metadata())
                && !(resume_session_is_transient_pending_archive
                    && metadata_marks_transient_pending_archive(session.metadata()))
            {
                self.reject_if_archived_session(resume_session_id, &session)
                    .await
                    .map_err(control_error_into_session_error)?;
            }
            Some(guard)
        } else {
            None
        };
        let create_projection_session_tx = self.install_create_time_event_projection(&mut req);
        let callback_session_id = resume_session_id.clone();
        let result = match self
            .inner
            .create_session_with_admission(req, reserved_create_admission)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if Self::callback_pending_terminal(&error).is_some()
                    && let Some(session_id) = callback_session_id
                {
                    self.persist_full_session_or_discard_live(&session_id)
                        .await?;
                }
                return Err(error);
            }
        };
        self.forget_archived_session(&result.session_id).await;

        // Track the gate so archive() can cancel checkpoint writes.
        {
            self.checkpointer_gates
                .lock()
                .await
                .insert(result.session_id.clone(), gate);
        }
        if let Some(session_tx) = create_projection_session_tx {
            let _ = session_tx.send(Some(result.session_id.clone()));
        }
        self.spawn_event_projection_task(&result.session_id).await;

        // Persist the full session snapshot (messages + metadata) after first
        // turn and seed the checkpointer so the next keep-alive checkpoint is
        // skipped if the session hasn't changed since this save.
        let saved_len = self
            .persist_full_session_or_discard_live(&result.session_id)
            .await?;
        checkpointer
            .last_saved_len
            .store(saved_len, std::sync::atomic::Ordering::Release);

        Ok(result)
    }

    async fn archive_with_store_only_mode(
        &self,
        id: &SessionId,
        store_only_mode: StoreOnlyArchiveMode,
    ) -> Result<(), SessionError> {
        let recovery_gate = self.recovery_gate_for_session(id).await;
        let _recovery_guard = recovery_gate.lock().await;

        let archived_snapshot = match self.export_session_with_labels(id).await {
            Ok(session) => Some(session),
            Err(SessionError::NotFound { .. }) => match store_only_mode {
                StoreOnlyArchiveMode::Reject => {
                    self.load_persisted_session_for_control(id, "archive")
                        .await?
                }
                StoreOnlyArchiveMode::MachineAuthority => {
                    self.load_machine_authority_session_for_control(id).await?
                }
            },
            Err(err) => return Err(err),
        };
        if let Some(ref session) = archived_snapshot
            && metadata_marks_archived(session.metadata())
        {
            if metadata_marks_transient_pending_archive(session.metadata()) {
                let mut session = session.clone();
                clear_transient_pending_archive_marker(&mut session);
                let session = self.save_normalized_session(session).await?;
                self.remember_archived_session(session).await;
            } else {
                self.remember_archived_session(session.clone()).await;
            }
            return Err(SessionError::NotFound { id: id.clone() });
        }

        // Acquire the checkpointer gate (if any) and hold it across the
        // archival save. This prevents a concurrent checkpoint() from saving
        // a live snapshot over the archived one. Setting cancelled under the
        // lock ensures all future checkpoints are no-ops.
        let gate = self.existing_gate_for_session(id).await;
        let mut gate_guard = if let Some(ref gate) = gate {
            let mut guard = gate.cancelled.lock().await;
            *guard = true;
            Some(guard)
        } else {
            None
        };

        let had_durable_snapshot = archived_snapshot.is_some();
        let mut saved_archived_snapshot = false;
        if let Some(mut session) = archived_snapshot.clone() {
            session.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
            clear_transient_pending_archive_marker(&mut session);
            match self.save_normalized_session(session).await {
                Ok(session) => {
                    saved_archived_snapshot = true;
                    self.remember_archived_session(session).await;
                }
                Err(err) => {
                    if let Some(ref mut guard) = gate_guard {
                        **guard = false;
                    }
                    return Err(err);
                }
            }
        }

        let live_result = self.inner.archive(id).await;

        // Gate guard is dropped here - any in-flight checkpoint that was
        // blocked on the lock will now see cancelled == true and bail out.
        drop(gate_guard.take());
        self.checkpointer_gates.lock().await.remove(id);

        match (&live_result, saved_archived_snapshot) {
            // At least one side had the session - success.
            (Ok(()), _) | (_, true) => Ok(()),
            (_, false) if had_durable_snapshot => Ok(()),
            // Neither side had it - propagate NotFound from the live service.
            _ => live_result,
        }
    }

    /// Archive through runtime/session machine authority when the service has
    /// no live runtime snapshot yet, such as staged sessions owned by
    /// `MeerkatMachine`.
    pub async fn archive_with_machine_authority(
        &self,
        id: &SessionId,
        _authority: MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.archive_with_store_only_mode(id, StoreOnlyArchiveMode::MachineAuthority)
            .await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(req, None).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_inner_with_admission(id, req, None)
            .await
            .map_err(|(error, _admission)| error)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.cancel_after_boundary(id).await
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        self.inner.set_session_client(id, client).await
    }

    async fn hot_swap_session_llm_identity(
        &self,
        _id: &SessionId,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "hot_swap_session_llm_identity is a bespoke metadata seam that bypasses the canonical RuntimeTurnMetadata carrier; model/provider/provider_params must travel through the single runtime-backed turn seam instead".to_string(),
        ))
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

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let previous_visibility_state = self
            .export_session_with_labels(id)
            .await
            .and_then(|session| rollback_tool_visibility_state_snapshot(&session))?;

        self.inner.set_session_tool_filter(id, filter).await?;

        if let Err(error) = self.persist_full_session(id).await {
            let _ = self
                .inner
                .set_session_tool_visibility_state(id, previous_visibility_state)
                .await;
            return Err(error);
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        match self.inner.read(id).await {
            Ok(view) => Ok(view),
            Err(SessionError::NotFound { .. }) => {
                let Some(session) = self.load_authoritative_session_base(id).await? else {
                    return Err(SessionError::NotFound { id: id.clone() });
                };
                self.reject_if_archived_session(id, &session)
                    .await
                    .map_err(control_error_into_session_error)?;
                Ok(view_from_authoritative_session(&session))
            }
            Err(err) => Err(err),
        }
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let stored = self
            .store
            .list(SessionFilter::default())
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        let live_summaries = self.inner.list(SessionQuery::default()).await?;
        let live_ids: HashSet<SessionId> = live_summaries
            .iter()
            .map(|summary| summary.session_id.clone())
            .collect();
        let mut summaries_by_id: IndexMap<SessionId, SessionSummary> = IndexMap::new();
        for meta in stored {
            if live_ids.contains(&meta.id) {
                continue;
            }
            // Runtime-backed rows are discovery keys only. A stale or
            // store-only projection must not contribute summary metadata unless
            // runtime authority can rebuild the summary below.
            if let Some(session) = self.load_authoritative_session_base(&meta.id).await? {
                if metadata_marks_archived(session.metadata()) {
                    continue;
                }
                let summary = summary_from_meta(meerkat_core::SessionMeta::from(&session));
                summaries_by_id.insert(summary.session_id.clone(), summary);
            }
        }

        for summary in live_summaries {
            summaries_by_id.insert(summary.session_id.clone(), summary);
        }

        let mut summaries: Vec<SessionSummary> = summaries_by_id.into_values().collect();

        // Filter by labels if specified (all k/v pairs must match).
        if let Some(ref filter_labels) = query.labels {
            summaries.retain(|s| {
                filter_labels
                    .iter()
                    .all(|(k, v)| s.labels.get(k) == Some(v))
            });
        }

        // Apply pagination
        if let Some(offset) = query.offset {
            if offset < summaries.len() {
                summaries = summaries.split_off(offset);
            } else {
                summaries.clear();
            }
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        self.inner.has_live_session(id).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.archive_with_store_only_mode(id, StoreOnlyArchiveMode::Reject)
            .await
    }

    async fn update_session_keep_alive(
        &self,
        _id: &SessionId,
        _keep_alive: bool,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "update_session_keep_alive is a bespoke metadata seam that bypasses the canonical RuntimeTurnMetadata carrier; keep_alive intent must travel through the single runtime-backed turn seam instead".to_string(),
        ))
    }

    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        let _mutation_guard = self.live_persist_mutation_guard(id).await?;
        let previous = self
            .export_session_with_labels(id)
            .await?
            .mob_tool_authority_context();

        self.inner
            .update_session_mob_authority_context(id, authority_context.clone())
            .await?;

        if let Err(error) = self.persist_full_session(id).await {
            let _ = self
                .inner
                .update_session_mob_authority_context(id, previous)
                .await;
            return Err(error);
        }
        Ok(())
    }

    async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        self.inner.subscribe_session_events(id).await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceHistoryExt for PersistentSessionService<B> {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        if let Some(session) = self.cached_archived_session(id).await {
            return Ok(SessionHistoryPage::from_messages(
                session.id().clone(),
                session.messages(),
                query,
            ));
        }
        let session = self
            .load_authoritative_session_base(id)
            .await?
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        Ok(SessionHistoryPage::from_messages(
            session.id().clone(),
            session.messages(),
            query,
        ))
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceCommsExt for PersistentSessionService<B> {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionServiceControlExt for PersistentSessionService<B> {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if self.cached_archived_session(id).await.is_some() {
            return Err(Self::archived_not_found(id));
        }

        let existing_gate = self.existing_gate_for_session(id).await;
        if let Some(state_arc) = self.inner.system_context_state(id).await {
            let created_gate = existing_gate.is_none();
            let gate = match existing_gate {
                Some(gate) => gate,
                None => self.gate_for_session(id).await,
            };
            {
                let gate_guard = gate.cancelled.lock().await;
                if *gate_guard {
                    return Err(SessionControlError::Session(SessionError::NotFound {
                        id: id.clone(),
                    }));
                }
            }

            let accepted_at = meerkat_core::time_compat::SystemTime::now();
            let (status, snapshot_state, persisted_state) = {
                let mut guard = match state_arc.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        tracing::warn!(
                            session_id = %id,
                            "system-context state lock poisoned while staging live append"
                        );
                        poisoned.into_inner()
                    }
                };
                let snapshot_state = guard.clone();
                let mut candidate = snapshot_state.clone();
                let status = candidate
                    .stage_append(&req, accepted_at)
                    .map_err(|err| err.into_control_error(id))?;
                *guard = candidate.clone();
                (status, snapshot_state, candidate)
            };

            let mut session = if self.runtime_store.is_some() {
                match self.load_authoritative_session_base(id).await? {
                    Some(session) => session,
                    None => {
                        if created_gate {
                            self.checkpointer_gates.lock().await.remove(id);
                        }
                        return Err(SessionControlError::Session(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(
                                "runtime-backed live session is missing its last committed snapshot"
                                    .to_string(),
                            ),
                        )));
                    }
                }
            } else {
                match self.export_session_with_labels(id).await {
                    Ok(session) => session,
                    Err(err) => {
                        if created_gate && matches!(err, SessionError::NotFound { .. }) {
                            self.checkpointer_gates.lock().await.remove(id);
                        }
                        return Err(SessionControlError::Session(err));
                    }
                }
            };

            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                let rollback_result = {
                    let mut guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "system-context state lock poisoned while rolling back cancelled live append"
                            );
                            poisoned.into_inner()
                        }
                    };
                    if *guard == persisted_state {
                        *guard = snapshot_state;
                        Ok(())
                    } else {
                        Err(())
                    }
                };
                drop(gate_guard);
                if rollback_result.is_ok() {
                    let _ = self.inner.sync_system_context_state(id).await;
                } else {
                    tracing::warn!(
                        session_id = %id,
                        "live system-context state diverged after archive cancelled append; discarding live session"
                    );
                    let _ = self.discard_live_session(id).await;
                }
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }

            let persist_result = async {
                self.reject_if_archived_session(id, &session).await?;
                write_system_context_state(&mut session, persisted_state.clone())?;
                self.save_normalized_session(session)
                    .await
                    .map_err(SessionControlError::Session)?;
                Ok::<(), SessionControlError>(())
            }
            .await;

            if let Err(err) = persist_result {
                let rollback_result = {
                    let mut guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "system-context state lock poisoned while rolling back failed live append"
                            );
                            poisoned.into_inner()
                        }
                    };
                    if *guard == persisted_state {
                        *guard = snapshot_state;
                        Ok(())
                    } else {
                        Err(())
                    }
                };
                if rollback_result.is_ok() {
                    let _ = self.inner.sync_system_context_state(id).await;
                } else {
                    tracing::warn!(
                        session_id = %id,
                        "live system-context state diverged after a failed durable append; discarding the live session to restore authoritative state"
                    );
                    drop(gate_guard);
                    let _ = self.discard_live_session(id).await;
                }
                return Err(err);
            }

            let reconciled_state = {
                let guard = match state_arc.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        tracing::warn!(
                            session_id = %id,
                            "system-context state lock poisoned while reconciling durable append state"
                        );
                        poisoned.into_inner()
                    }
                };
                guard.clone()
            };
            if reconciled_state != persisted_state {
                let mut session = if self.runtime_store.is_some() {
                    match self.load_authoritative_session_base(id).await? {
                        Some(session) => session,
                        None => {
                            drop(gate_guard);
                            let _ = self.discard_live_session(id).await;
                            return Ok(AppendSystemContextResult { status });
                        }
                    }
                } else {
                    self.export_session_with_labels(id)
                        .await
                        .map_err(SessionControlError::Session)?
                };
                self.reject_if_archived_session(id, &session).await?;
                write_system_context_state(&mut session, reconciled_state)?;
                self.save_normalized_session(session)
                    .await
                    .map_err(SessionControlError::Session)?;
            }

            let _ = self.inner.sync_system_context_state(id).await;
            drop(gate_guard);
            return Ok(AppendSystemContextResult { status });
        }

        if let Some(gate) = existing_gate {
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }
            drop(gate_guard);
        }

        let mut session = match self
            .load_persisted_session_for_control(id, "append_system_context")
            .await?
        {
            Some(session) => session,
            None => {
                self.checkpointer_gates.lock().await.remove(id);
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }
        };
        self.reject_if_archived_session(id, &session).await?;
        let mut state = session.system_context_state().unwrap_or_default();
        let status = state
            .stage_append(&req, meerkat_core::time_compat::SystemTime::now())
            .map_err(|err| err.into_control_error(id))?;
        write_system_context_state(&mut session, state)?;
        self.save_normalized_session(session)
            .await
            .map_err(SessionControlError::Session)?;
        Ok(AppendSystemContextResult { status })
    }

    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        validate_tool_result_video(&req.results)?;
        if self.cached_archived_session(id).await.is_some() {
            return Err(SessionError::NotFound { id: id.clone() });
        }

        let existing_gate = self.existing_gate_for_session(id).await;
        if let Some(state_arc) = self.inner.deferred_turn_state(id).await {
            let created_gate = existing_gate.is_none();
            let gate = match existing_gate {
                Some(gate) => gate,
                None => self.gate_for_session(id).await,
            };
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionError::NotFound { id: id.clone() });
            }

            let accepted_at = meerkat_core::time_compat::SystemTime::now();
            let mut attempts = 0usize;
            loop {
                attempts += 1;
                let (accepted, snapshot_state, persisted_state) = {
                    let guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "deferred-turn state lock poisoned while snapshotting staged tool results"
                            );
                            poisoned.into_inner()
                        }
                    };
                    let snapshot_state = guard.clone();
                    let mut candidate = snapshot_state.clone();
                    let accepted = candidate.stage_tool_results(req.results.clone(), accepted_at);
                    (accepted, snapshot_state, candidate)
                };

                if accepted == 0 {
                    drop(gate_guard);
                    return Ok(StageToolResultsResult {
                        accepted_result_count: accepted,
                    });
                }

                let mut session = if self.runtime_store.is_some() {
                    match self.load_authoritative_session_base(id).await? {
                        Some(session) => session,
                        None => {
                            if created_gate {
                                drop(gate_guard);
                                self.checkpointer_gates.lock().await.remove(id);
                            }
                            return Err(SessionError::Agent(
                                meerkat_core::error::AgentError::InternalError(
                                    "runtime-backed live session is missing its last committed snapshot"
                                        .to_string(),
                                ),
                            ));
                        }
                    }
                } else {
                    match self.export_session_with_labels(id).await {
                        Ok(session) => session,
                        Err(err) => {
                            if created_gate && matches!(err, SessionError::NotFound { .. }) {
                                drop(gate_guard);
                                self.checkpointer_gates.lock().await.remove(id);
                            }
                            return Err(err);
                        }
                    }
                };

                self.reject_if_archived_session(id, &session)
                    .await
                    .map_err(control_error_into_session_error)?;
                write_deferred_turn_state(&mut session, persisted_state)
                    .map_err(control_error_into_session_error)?;
                self.save_normalized_session(session).await?;

                let commit_result = {
                    let mut guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "deferred-turn state lock poisoned while committing staged tool results"
                            );
                            poisoned.into_inner()
                        }
                    };
                    if *guard == snapshot_state {
                        Some(guard.stage_tool_results(req.results.clone(), accepted_at))
                    } else {
                        None
                    }
                };

                if let Some(live_accepted) = commit_result {
                    debug_assert_eq!(live_accepted, accepted);
                    drop(gate_guard);
                    return Ok(StageToolResultsResult {
                        accepted_result_count: accepted,
                    });
                }

                if attempts >= 8 {
                    tracing::warn!(
                        session_id = %id,
                        "deferred-turn state kept changing after durable tool-result staging; discarding live session to force authoritative reload"
                    );
                    drop(gate_guard);
                    let _ = self.discard_live_session(id).await;
                    return Ok(StageToolResultsResult {
                        accepted_result_count: accepted,
                    });
                }
            }
        }

        if let Some(gate) = existing_gate {
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            drop(gate_guard);
        }

        let mut session = match self
            .load_persisted_session_for_control(id, "stage_tool_results")
            .await?
        {
            Some(session) => session,
            None => {
                self.checkpointer_gates.lock().await.remove(id);
                return Err(SessionError::NotFound { id: id.clone() });
            }
        };
        self.reject_if_archived_session(id, &session)
            .await
            .map_err(control_error_into_session_error)?;
        let mut state = session.deferred_turn_state().unwrap_or_default();
        let accepted =
            state.stage_tool_results(req.results, meerkat_core::time_compat::SystemTime::now());
        write_deferred_turn_state(&mut session, state).map_err(control_error_into_session_error)?;
        self.save_normalized_session(session).await?;
        Ok(StageToolResultsResult {
            accepted_result_count: accepted,
        })
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Get the event injector for a session, if available.
    pub async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    #[doc(hidden)]
    pub async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    /// Wait for a session to be registered.
    pub async fn wait_session_registered(&self) {
        self.inner.wait_session_registered().await;
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        self.inner.shutdown().await;
    }

    /// Cancel all active checkpointer gates.
    ///
    /// After this call, in-flight checkpoints that are past the gate check
    /// will complete their current save, but subsequent checkpoint calls on
    /// any session will be no-ops. Use this during `stop()` to prevent
    /// checkpoint writes from racing with external cleanup operations.
    pub async fn cancel_all_checkpointers(&self) {
        let gates = self.checkpointer_gates.lock().await;
        for gate in gates.values() {
            let mut cancelled = gate.cancelled.lock().await;
            *cancelled = true;
        }
    }

    /// Re-enable checkpointer gates for all tracked sessions.
    ///
    /// Call this during `resume()` after `cancel_all_checkpointers()` was
    /// used during stop. Gates that were removed by `archive()` are not
    /// affected.
    pub async fn rearm_all_checkpointers(&self) {
        let gates = self.checkpointer_gates.lock().await;
        for gate in gates.values() {
            let mut cancelled = gate.cancelled.lock().await;
            *cancelled = false;
        }
    }

    /// Subscribe to session-wide events from the live inner service.
    pub async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        self.inner.subscribe_session_events(id).await
    }

    /// Whether a live session still has its deferred first turn pending.
    pub async fn live_deferred_first_turn_pending(
        &self,
        id: &SessionId,
    ) -> Result<bool, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        let Some(state) = self.inner.deferred_turn_state(id).await else {
            return Ok(false);
        };
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(matches!(
            state.first_turn_phase,
            DeferredFirstTurnPhase::Pending
        ))
    }

    /// Load the authoritative durable session view.
    ///
    /// Runtime snapshots are the session authority when a runtime store is
    /// available. The `SessionStore` row is only a compatibility projection in
    /// that mode. Store-only rows are not exposed as authoritative runtime
    /// truth; existing runtime snapshots are never merged with projection
    /// metadata.
    pub async fn load_authoritative_session(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        self.load_authoritative_session_base(id).await
    }

    /// Export the full session from the live task and persist it to the store.
    ///
    /// Returns the saved message count so callers can seed a checkpointer's
    /// `last_saved_len` without a second export round-trip.
    async fn persist_full_session(&self, id: &SessionId) -> Result<usize, SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let persisted = self.save_normalized_session(session).await?;
        let message_count = persisted.messages().len();
        Ok(message_count)
    }

    async fn persist_full_session_or_discard_live(
        &self,
        id: &SessionId,
    ) -> Result<usize, SessionError> {
        match self.persist_full_session(id).await {
            Ok(message_count) => Ok(message_count),
            Err(error) => {
                match self.discard_live_session(id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(discard_error) => {
                        tracing::warn!(
                            session_id = %id,
                            error = %discard_error,
                            "failed to discard live session after full-session persistence failure"
                        );
                    }
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ephemeral::{
        EphemeralSessionService, SessionAgent, SessionAgentBuilder, SessionSnapshot,
    };
    use crate::event_store::{EVENT_SCHEMA_VERSION, EventStoreError, StoredEvent};
    use meerkat_core::ToolDispatchOutcome;
    use meerkat_core::checkpoint::SessionCheckpointer;
    use meerkat_core::event::AgentEvent;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
        SessionServiceControlExt, StageToolResultsRequest,
    };
    use meerkat_core::session::SESSION_METADATA_KEY;
    use meerkat_core::types::{
        AssistantBlock, ContentBlock, ContentInput, ImageData, Message, StopReason, ToolCall,
        ToolResult, Usage, UserMessage,
    };
    use meerkat_core::{RunId, lifecycle::run_primitive::RunApplyBoundary};
    use meerkat_runtime::{InMemoryRuntimeStore, RuntimeStore};
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionStoreError};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn memory_blob_store() -> Arc<dyn BlobStore> {
        Arc::new(MemoryBlobStore::new())
    }

    struct RecordingEventStore {
        events: Mutex<HashMap<SessionId, Vec<StoredEvent>>>,
        notify: tokio::sync::Notify,
    }

    impl Default for RecordingEventStore {
        fn default() -> Self {
            Self {
                events: Mutex::new(HashMap::new()),
                notify: tokio::sync::Notify::new(),
            }
        }
    }

    impl RecordingEventStore {
        async fn wait_for_seq(&self, session_id: &SessionId, target_seq: u64) {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if self.last_seq(session_id).await.unwrap() >= target_seq {
                        return;
                    }
                    self.notify.notified().await;
                }
            })
            .await
            .expect("event store projection did not reach expected sequence");
        }
    }

    async fn read_projected_events_after(events_path: &std::path::Path, expected: &str) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(projected) = tokio::fs::read_to_string(events_path).await
                    && projected.contains(expected)
                {
                    return projected;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("projected events.jsonl did not contain expected event")
    }

    #[async_trait::async_trait]
    impl EventStore for RecordingEventStore {
        async fn append(
            &self,
            session_id: &SessionId,
            events: &[AgentEvent],
        ) -> Result<u64, EventStoreError> {
            let mut all_events = self.events.lock().await;
            let session_events = all_events.entry(session_id.clone()).or_default();
            for event in events {
                let seq = session_events.len() as u64 + 1;
                session_events.push(StoredEvent {
                    seq,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: meerkat_core::time_compat::SystemTime::now(),
                    event: event.clone(),
                });
            }
            let last_seq = session_events.len() as u64;
            drop(all_events);
            self.notify.notify_waiters();
            Ok(last_seq)
        }

        async fn read_from(
            &self,
            session_id: &SessionId,
            from_seq: u64,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let all_events = self.events.lock().await;
            Ok(all_events
                .get(session_id)
                .into_iter()
                .flat_map(|events| events.iter())
                .filter(|event| event.seq >= from_seq)
                .cloned()
                .collect())
        }

        async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
            let all_events = self.events.lock().await;
            Ok(all_events
                .get(session_id)
                .map_or(0, |events| events.len() as u64))
        }
    }

    struct FailSaveStore {
        inner: MemoryStore,
        fail_save: AtomicBool,
    }

    impl FailSaveStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                fail_save: AtomicBool::new(false),
            }
        }

        fn set_fail_save(&self, fail: bool) {
            self.fail_save.store(fail, Ordering::Release);
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for FailSaveStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            if self.fail_save.load(Ordering::Acquire) {
                return Err(SessionStoreError::Internal(
                    "forced save failure".to_string(),
                ));
            }
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }
    }

    struct BlockingArchiveSaveStore {
        inner: MemoryStore,
        block_archived_saves: AtomicBool,
        entered_archived_save: tokio::sync::Notify,
        release_archived_save: tokio::sync::Notify,
    }

    impl BlockingArchiveSaveStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                block_archived_saves: AtomicBool::new(false),
                entered_archived_save: tokio::sync::Notify::new(),
                release_archived_save: tokio::sync::Notify::new(),
            }
        }

        fn block_archived_saves(&self) {
            self.block_archived_saves.store(true, Ordering::Release);
        }

        async fn wait_for_archived_save(&self) {
            self.entered_archived_save.notified().await;
        }

        fn release_archived_save(&self) {
            self.release_archived_save.notify_waiters();
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for BlockingArchiveSaveStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            if self.block_archived_saves.load(Ordering::Acquire)
                && metadata_marks_archived(session.metadata())
            {
                self.entered_archived_save.notify_waiters();
                self.release_archived_save.notified().await;
            }
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }
    }

    struct GatedSnapshotRuntimeStore {
        inner: InMemoryRuntimeStore,
        hidden_snapshot_loads: AtomicUsize,
        fail_boundary_commits: AtomicBool,
        fail_snapshot_commits: AtomicBool,
        session_snapshot_overrides: Mutex<HashMap<LogicalRuntimeId, Vec<u8>>>,
        input_state_load_errors: Mutex<HashSet<LogicalRuntimeId>>,
        boundary_commits: Mutex<Vec<meerkat_core::lifecycle::RunBoundaryReceipt>>,
    }

    impl GatedSnapshotRuntimeStore {
        fn new() -> Self {
            Self {
                inner: InMemoryRuntimeStore::new(),
                hidden_snapshot_loads: AtomicUsize::new(0),
                fail_boundary_commits: AtomicBool::new(false),
                fail_snapshot_commits: AtomicBool::new(false),
                session_snapshot_overrides: Mutex::new(HashMap::new()),
                input_state_load_errors: Mutex::new(HashSet::new()),
                boundary_commits: Mutex::new(Vec::new()),
            }
        }

        fn set_fail_boundary_commits(&self, fail: bool) {
            self.fail_boundary_commits.store(fail, Ordering::Release);
        }

        fn set_fail_snapshot_commits(&self, fail: bool) {
            self.fail_snapshot_commits.store(fail, Ordering::Release);
        }

        fn hide_next_session_snapshot_loads(&self, count: usize) {
            self.hidden_snapshot_loads.store(count, Ordering::Release);
        }

        fn should_hide_session_snapshot_load(&self) -> bool {
            let mut remaining = self.hidden_snapshot_loads.load(Ordering::Acquire);
            while remaining > 0 {
                match self.hidden_snapshot_loads.compare_exchange(
                    remaining,
                    remaining - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return true,
                    Err(current) => remaining = current,
                }
            }
            false
        }

        async fn boundary_commits(&self) -> Vec<meerkat_core::lifecycle::RunBoundaryReceipt> {
            self.boundary_commits.lock().await.clone()
        }

        async fn reset_boundary_commits(&self) {
            self.boundary_commits.lock().await.clear();
        }

        async fn override_session_snapshot(&self, runtime_id: LogicalRuntimeId, snapshot: Vec<u8>) {
            self.session_snapshot_overrides
                .lock()
                .await
                .insert(runtime_id, snapshot);
        }

        async fn fail_input_state_load_for(&self, runtime_id: LogicalRuntimeId) {
            self.input_state_load_errors.lock().await.insert(runtime_id);
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for GatedSnapshotRuntimeStore {
        async fn commit_session_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: meerkat_runtime::store::SessionDelta,
            run_id: RunId,
            boundary: RunApplyBoundary,
            contributing_input_ids: Vec<InputId>,
            input_updates: Vec<StoredInputState>,
        ) -> Result<
            meerkat_core::lifecycle::RunBoundaryReceipt,
            meerkat_runtime::store::RuntimeStoreError,
        > {
            if self.fail_boundary_commits.load(Ordering::Acquire) {
                return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                    "synthetic runtime boundary commit failure".to_string(),
                ));
            }
            let receipt = self
                .inner
                .commit_session_boundary(
                    runtime_id,
                    session_delta,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    input_updates,
                )
                .await?;
            self.boundary_commits.lock().await.push(receipt.clone());
            Ok(receipt)
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: meerkat_runtime::store::SessionDelta,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            if self.fail_snapshot_commits.load(Ordering::Acquire) {
                return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                    "synthetic runtime snapshot commit failure".to_string(),
                ));
            }
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<meerkat_runtime::store::SessionDelta>,
            receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
            input_updates: Vec<StoredInputState>,
            session_store_key: Option<SessionId>,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner
                .atomic_apply(
                    runtime_id,
                    session_delta,
                    receipt,
                    input_updates,
                    session_store_key,
                )
                .await
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<StoredInputState>, meerkat_runtime::store::RuntimeStoreError> {
            if self
                .input_state_load_errors
                .lock()
                .await
                .contains(runtime_id)
            {
                return Err(meerkat_runtime::store::RuntimeStoreError::ReadFailed(
                    "synthetic legacy input-state load failure".to_string(),
                ));
            }
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<
            Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
            meerkat_runtime::store::RuntimeStoreError,
        > {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
            if self.should_hide_session_snapshot_load() {
                return Ok(None);
            }
            if let Some(snapshot) = self
                .session_snapshot_overrides
                .lock()
                .await
                .get(runtime_id)
                .cloned()
            {
                return Ok(Some(snapshot));
            }
            self.inner.load_session_snapshot(runtime_id).await
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &StoredInputState,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<StoredInputState>, meerkat_runtime::store::RuntimeStoreError> {
            self.inner.load_input_state(runtime_id, input_id).await
        }

        async fn persist_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: meerkat_runtime::RuntimeState,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner.persist_runtime_state(runtime_id, state).await
        }

        async fn load_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<meerkat_runtime::RuntimeState>, meerkat_runtime::store::RuntimeStoreError>
        {
            self.inner.load_runtime_state(runtime_id).await
        }

        async fn atomic_lifecycle_commit(
            &self,
            runtime_id: &LogicalRuntimeId,
            runtime_state: meerkat_runtime::RuntimeState,
            input_states: &[StoredInputState],
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner
                .atomic_lifecycle_commit(runtime_id, runtime_state, input_states)
                .await
        }

        async fn persist_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
            self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
        }

        async fn load_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<
            Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>,
            meerkat_runtime::store::RuntimeStoreError,
        > {
            self.inner.load_ops_lifecycle(runtime_id).await
        }
    }

    struct DummyAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
        run_failure: Option<String>,
        flow_overlay_failure: Option<String>,
        callback_pending_after_run: bool,
    }

    #[async_trait::async_trait]
    impl SessionAgent for DummyAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            if let Some(message) = self.run_failure.clone() {
                return Err(meerkat_core::error::AgentError::InternalError(message));
            }
            let session_id = self.session_id();
            let result = {
                let mut session = match self.session.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                session.push(meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::text(prompt.text_content()),
                ));
                session.push(meerkat_core::types::Message::Assistant(
                    meerkat_core::types::AssistantMessage {
                        content: "ok".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: meerkat_core::types::StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    },
                ));
                RunResult {
                    text: "ok".to_string(),
                    session_id,
                    usage: meerkat_core::types::Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    terminal_cause_kind: None,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                }
            };
            if self.callback_pending_after_run {
                return Err(meerkat_core::error::AgentError::CallbackPending {
                    tool_name: "test_callback".to_string(),
                    args: serde_json::json!({}),
                });
            }
            Ok(result)
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            if let Some(message) = self.flow_overlay_failure.clone() {
                return Err(meerkat_core::error::AgentError::InternalError(message));
            }
            Ok(())
        }

        fn cancel(&mut self) {}

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: identity.model.clone(),
                        max_tokens: 0,
                        structured_output_retries: 2,
                        provider: identity.provider,
                        self_hosted_server_id: None,
                        provider_params: identity.provider_params.clone(),
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: None,
                        peer_meta: None,
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        connection_ref: None,
                    });
            metadata.apply_llm_identity(&identity);
            session.set_session_metadata(metadata).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update dummy session metadata: {err}"
                ))
            })
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut state = session
                .tool_visibility_state()
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to decode dummy visibility state: {err}"
                    ))
                })?
                .unwrap_or_default();
            state.staged_filter = filter;
            state.staged_revision = state.staged_revision.max(state.active_revision) + 1;
            session.set_tool_visibility_state(state).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update dummy visibility state: {err}"
                ))
            })
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(state) = state {
                session.set_tool_visibility_state(state).map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to replace dummy visibility state: {err}"
                    ))
                })
            } else {
                session.remove_metadata(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY);
                Ok(())
            }
        }

        fn session_id(&self) -> SessionId {
            match self.session.lock() {
                Ok(guard) => guard.id().clone(),
                Err(poisoned) => poisoned.into_inner().id().clone(),
            }
        }

        fn snapshot(&self) -> SessionSnapshot {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            SessionSnapshot {
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
                last_assistant_text: session.last_assistant_text(),
            }
        }

        fn session_clone(&self) -> Session {
            match self.session.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(mut meta) = session.session_metadata() {
                meta.keep_alive = keep_alive;
                let _ = session.set_session_metadata(meta);
            }
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session
                .set_mob_tool_authority_context(authority_context)
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to update dummy mob authority context: {err}"
                    ))
                })
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session.set_system_prompt(system_prompt);
            Ok(())
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            let mut guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.append_system_context_blocks(appends);
            let state = guard.system_context_state().unwrap_or_default();
            self.system_context_state = Arc::new(std::sync::Mutex::new(state));
        }

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            Arc::clone(&self.system_context_state)
        }
    }

    struct EventfulDummyAgent {
        inner: DummyAgent,
    }

    #[async_trait::async_trait]
    impl SessionAgent for EventfulDummyAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let session_id = self.inner.session_id();
            let _ = event_tx
                .send(AgentEvent::RunStarted {
                    session_id: session_id.clone(),
                    prompt: prompt.clone(),
                })
                .await;
            let result = self.inner.run_with_events(prompt, event_tx.clone()).await?;
            let _ = event_tx
                .send(AgentEvent::RunCompleted {
                    session_id,
                    result: result.text.clone(),
                    usage: result.usage.clone(),
                    terminal_cause_kind: result.terminal_cause_kind,
                })
                .await;
            Ok(result)
        }

        fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
            self.inner.set_skill_references(refs);
        }

        fn set_flow_tool_overlay(
            &mut self,
            overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_flow_tool_overlay(overlay)
        }

        fn cancel(&mut self) {
            self.inner.cancel();
        }

        fn hot_swap_llm_identity(
            &mut self,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .hot_swap_llm_identity(client, identity, request_policy)
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.stage_external_tool_filter(filter)
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_tool_visibility_state(state)
        }

        fn session_id(&self) -> SessionId {
            self.inner.session_id()
        }

        fn snapshot(&self) -> SessionSnapshot {
            self.inner.snapshot()
        }

        fn session_clone(&self) -> Session {
            self.inner.session_clone()
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            self.inner.update_keep_alive(keep_alive);
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .update_mob_tool_authority_context(authority_context)
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.update_system_prompt(system_prompt)
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            self.inner.apply_runtime_system_context(appends);
        }

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            self.inner.system_context_state()
        }
    }

    struct DummyBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for DummyBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct CallbackPendingBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CallbackPendingBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: true,
            })
        }
    }

    struct FailingOverlayBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for FailingOverlayBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: Some("synthetic flow overlay failure".to_string()),
                callback_pending_after_run: false,
            })
        }
    }

    #[derive(Clone)]
    struct BlockingBuildBuilder {
        entered_builds: Arc<AtomicUsize>,
        max_concurrent_builds: Arc<AtomicUsize>,
        active_builds: Arc<AtomicUsize>,
        entered_notify: Arc<tokio::sync::Notify>,
        release_notify: Arc<tokio::sync::Notify>,
    }

    impl BlockingBuildBuilder {
        fn new() -> Self {
            Self {
                entered_builds: Arc::new(AtomicUsize::new(0)),
                max_concurrent_builds: Arc::new(AtomicUsize::new(0)),
                active_builds: Arc::new(AtomicUsize::new(0)),
                entered_notify: Arc::new(tokio::sync::Notify::new()),
                release_notify: Arc::new(tokio::sync::Notify::new()),
            }
        }

        fn record_build_start(&self) {
            self.entered_builds.fetch_add(1, Ordering::AcqRel);
            let active = self.active_builds.fetch_add(1, Ordering::AcqRel) + 1;
            let mut observed = self.max_concurrent_builds.load(Ordering::Acquire);
            while active > observed {
                match self.max_concurrent_builds.compare_exchange(
                    observed,
                    active,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(current) => observed = current,
                }
            }
            self.entered_notify.notify_waiters();
        }

        fn record_build_finish(&self) {
            self.active_builds.fetch_sub(1, Ordering::AcqRel);
        }
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for BlockingBuildBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            self.record_build_start();
            self.release_notify.notified().await;
            self.record_build_finish();
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    #[derive(Clone)]
    struct BlockingRunBuilder {
        entered_runs: Arc<AtomicUsize>,
        entered_notify: Arc<tokio::sync::Notify>,
        release_notify: Arc<tokio::sync::Notify>,
    }

    impl BlockingRunBuilder {
        fn new() -> Self {
            Self {
                entered_runs: Arc::new(AtomicUsize::new(0)),
                entered_notify: Arc::new(tokio::sync::Notify::new()),
                release_notify: Arc::new(tokio::sync::Notify::new()),
            }
        }
    }

    struct BlockingRunAgent {
        inner: DummyAgent,
        entered_runs: Arc<AtomicUsize>,
        entered_notify: Arc<tokio::sync::Notify>,
        release_notify: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for BlockingRunBuilder {
        type Agent = BlockingRunAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(BlockingRunAgent {
                inner: DummyAgent {
                    session: Arc::new(std::sync::Mutex::new(session)),
                    system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                    run_failure: None,
                    flow_overlay_failure: None,
                    callback_pending_after_run: false,
                },
                entered_runs: Arc::clone(&self.entered_runs),
                entered_notify: Arc::clone(&self.entered_notify),
                release_notify: Arc::clone(&self.release_notify),
            })
        }
    }

    #[async_trait::async_trait]
    impl SessionAgent for BlockingRunAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            self.entered_runs.fetch_add(1, Ordering::AcqRel);
            self.entered_notify.notify_waiters();
            self.release_notify.notified().await;
            self.inner.run_with_events(prompt, event_tx).await
        }

        fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
            self.inner.set_skill_references(refs);
        }

        fn set_flow_tool_overlay(
            &mut self,
            overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_flow_tool_overlay(overlay)
        }

        fn cancel(&mut self) {
            self.inner.cancel();
        }

        fn hot_swap_llm_identity(
            &mut self,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .hot_swap_llm_identity(client, identity, request_policy)
        }

        fn stage_external_tool_filter(
            &mut self,
            filter: meerkat_core::ToolFilter,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.stage_external_tool_filter(filter)
        }

        fn set_tool_visibility_state(
            &mut self,
            state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.set_tool_visibility_state(state)
        }

        fn session_id(&self) -> SessionId {
            self.inner.session_id()
        }

        fn snapshot(&self) -> SessionSnapshot {
            self.inner.snapshot()
        }

        fn session_clone(&self) -> Session {
            self.inner.session_clone()
        }

        fn update_keep_alive(&mut self, keep_alive: bool) {
            self.inner.update_keep_alive(keep_alive);
        }

        fn update_mob_tool_authority_context(
            &mut self,
            authority_context: Option<MobToolAuthorityContext>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner
                .update_mob_tool_authority_context(authority_context)
        }

        fn update_system_prompt(
            &mut self,
            system_prompt: String,
        ) -> Result<(), meerkat_core::error::AgentError> {
            self.inner.update_system_prompt(system_prompt)
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            self.inner.apply_runtime_system_context(appends);
        }

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            self.inner.system_context_state()
        }
    }

    struct EventfulBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for EventfulBuilder {
        type Agent = EventfulDummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(EventfulDummyAgent {
                inner: DummyAgent {
                    session: Arc::new(std::sync::Mutex::new(session)),
                    system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                    run_failure: None,
                    flow_overlay_failure: None,
                    callback_pending_after_run: false,
                },
            })
        }
    }

    struct FailingRunBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for FailingRunBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: Some("synthetic run failure".to_string()),
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct CapturingBuildBuilder {
        captured_builds: Arc<tokio::sync::Mutex<Vec<SessionBuildOptions>>>,
    }

    impl CapturingBuildBuilder {
        fn new() -> Self {
            Self {
                captured_builds: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CapturingBuildBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            if let Some(build) = req.build.clone() {
                self.captured_builds.lock().await.push(build);
            }
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct ImagePreservingAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    #[async_trait::async_trait]
    impl SessionAgent for ImagePreservingAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session.push(Message::User(UserMessage::with_blocks(
                prompt.into_blocks(),
            )));
            session.push(Message::Assistant(meerkat_core::types::AssistantMessage {
                content: "ok".to_string(),
                tool_calls: vec![],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: session.id().clone(),
                usage: meerkat_core::types::Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: identity.model.clone(),
                        max_tokens: 0,
                        structured_output_retries: 2,
                        provider: identity.provider,
                        self_hosted_server_id: None,
                        provider_params: identity.provider_params.clone(),
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: None,
                        peer_meta: None,
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        connection_ref: None,
                    });
            metadata.apply_llm_identity(&identity);
            session.set_session_metadata(metadata).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update image-preserving session metadata: {err}"
                ))
            })
        }

        fn cancel(&mut self) {}

        fn snapshot(&self) -> SessionSnapshot {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            SessionSnapshot {
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
                last_assistant_text: session.last_assistant_text(),
            }
        }

        fn session_clone(&self) -> Session {
            match self.session.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn session_id(&self) -> SessionId {
            let guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.id().clone()
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            let mut guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.append_system_context_blocks(appends);
            let state = guard.system_context_state().unwrap_or_default();
            self.system_context_state = Arc::new(std::sync::Mutex::new(state));
        }

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            Arc::clone(&self.system_context_state)
        }
    }

    struct ImagePreservingBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for ImagePreservingBuilder {
        type Agent = ImagePreservingAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(ImagePreservingAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
            })
        }
    }

    struct CapturingCheckpointerBuilder {
        captured:
            Arc<tokio::sync::Mutex<Option<Arc<dyn meerkat_core::checkpoint::SessionCheckpointer>>>>,
    }

    impl CapturingCheckpointerBuilder {
        fn new() -> Self {
            Self {
                captured: Arc::new(tokio::sync::Mutex::new(None)),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionAgentBuilder for CapturingCheckpointerBuilder {
        type Agent = DummyAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            *self.captured.lock().await = req
                .build
                .as_ref()
                .and_then(|build| build.checkpointer.clone());

            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(DummyAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
                run_failure: None,
                flow_overlay_failure: None,
                callback_pending_after_run: false,
            })
        }
    }

    struct ToolDispatchAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    fn expected_tool_dispatch_witness(tool_name: &str) -> meerkat_core::ToolVisibilityWitness {
        meerkat_core::ToolVisibilityWitness {
            stable_owner_key: Some(format!("callback:{tool_name}")),
            last_seen_provenance: Some(meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Callback,
                source_id: tool_name.to_string().into(),
            }),
        }
    }

    #[async_trait::async_trait]
    impl SessionAgent for ToolDispatchAgent {
        async fn run_with_events(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let session_id = self.session_id();
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            session.push(meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::text(prompt.text_content()),
            ));
            session.push(meerkat_core::types::Message::Assistant(
                meerkat_core::types::AssistantMessage {
                    content: "ok".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: meerkat_core::types::StopReason::EndTurn,
                    usage: meerkat_core::types::Usage::default(),
                    created_at: meerkat_core::types::message_timestamp_now(),
                },
            ));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id,
                usage: meerkat_core::types::Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        async fn dispatch_external_tool_call(
            &mut self,
            call: ToolCall,
        ) -> Result<ToolDispatchOutcome, meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut state = session
                .tool_visibility_state()
                .map_err(|err| {
                    meerkat_core::error::AgentError::InternalError(format!(
                        "failed to decode dummy visibility state: {err}"
                    ))
                })?
                .unwrap_or_default();
            let requested_name = format!("requested:{}", call.name);
            state
                .staged_requested_deferred_names
                .insert(requested_name.clone());
            state
                .requested_witnesses
                .insert(requested_name, expected_tool_dispatch_witness(&call.name));
            session.set_tool_visibility_state(state).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to persist tool dispatch state: {err}"
                ))
            })?;
            Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
                call.id,
                format!("handled {}", call.name),
                false,
            )))
        }

        fn cancel(&mut self) {}

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: identity.model.clone(),
                        max_tokens: 0,
                        structured_output_retries: 2,
                        provider: identity.provider,
                        self_hosted_server_id: None,
                        provider_params: identity.provider_params.clone(),
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: None,
                        peer_meta: None,
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        connection_ref: None,
                    });
            metadata.apply_llm_identity(&identity);
            session.set_session_metadata(metadata).map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update tool-dispatch session metadata: {err}"
                ))
            })
        }

        fn session_id(&self) -> SessionId {
            match self.session.lock() {
                Ok(guard) => guard.id().clone(),
                Err(poisoned) => poisoned.into_inner().id().clone(),
            }
        }

        fn snapshot(&self) -> SessionSnapshot {
            let session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            SessionSnapshot {
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
                last_assistant_text: session.last_assistant_text(),
            }
        }

        fn session_clone(&self) -> Session {
            match self.session.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn apply_runtime_system_context(
            &mut self,
            appends: &[meerkat_core::PendingSystemContextAppend],
        ) {
            let mut guard = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.append_system_context_blocks(appends);
            let state = guard.system_context_state().unwrap_or_default();
            self.system_context_state = Arc::new(std::sync::Mutex::new(state));
        }

        fn system_context_state(
            &self,
        ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
            Arc::clone(&self.system_context_state)
        }
    }

    struct ToolDispatchBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for ToolDispatchBuilder {
        type Agent = ToolDispatchAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<meerkat_core::event::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let system_context_state = session.system_context_state().unwrap_or_default();
            Ok(ToolDispatchAgent {
                session: Arc::new(std::sync::Mutex::new(session)),
                system_context_state: Arc::new(std::sync::Mutex::new(system_context_state)),
            })
        }
    }

    fn create_request(prompt: &str, initial_turn: InitialTurnPolicy) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "test".to_string(),
            prompt: prompt.to_string().into(),
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn,
            build: None,
            labels: None,
        }
    }

    fn resume_request(session: Session) -> CreateSessionRequest {
        let mut req = create_request("", InitialTurnPolicy::Defer);
        req.build = Some(SessionBuildOptions {
            resume_session: Some(session),
            ..Default::default()
        });
        req
    }

    fn inline_image_block(label: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: ImageData::Inline {
                data: format!("base64-{label}"),
            },
        }
    }

    fn image_prompt(label: &str) -> ContentInput {
        ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: format!("look at {label}"),
            },
            inline_image_block(label),
        ])
    }

    fn assert_no_inline_images_in_session(session: &Session) {
        for message in session.messages() {
            match message {
                Message::User(user) => {
                    for block in &user.content {
                        assert!(
                            !matches!(
                                block,
                                ContentBlock::Image {
                                    data: ImageData::Inline { .. },
                                    ..
                                }
                            ),
                            "persisted session unexpectedly retained inline image bytes: {session:?}"
                        );
                    }
                }
                Message::ToolResults { results, .. } => {
                    for result in results {
                        for block in &result.content {
                            assert!(
                                !matches!(
                                    block,
                                    ContentBlock::Image {
                                        data: ImageData::Inline { .. },
                                        ..
                                    }
                                ),
                                "persisted session unexpectedly retained inline image bytes: {session:?}"
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn start_turn_request(prompt: &str) -> StartTurnRequest {
        StartTurnRequest {
            prompt: prompt.to_string().into(),
            system_prompt: None,
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: None,
            skill_references: None,
            flow_tool_overlay: None,
            pre_turn_context_appends: Vec::new(),
            turn_metadata: None,
        }
    }

    fn runtime_content_turn_request(prompt: &str) -> StartTurnRequest {
        let mut req = start_turn_request(prompt);
        req.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            },
        );
        req
    }

    fn start_turn_request_with_system_prompt(
        prompt: &str,
        system_prompt: Option<&str>,
    ) -> StartTurnRequest {
        StartTurnRequest {
            prompt: prompt.to_string().into(),
            system_prompt: system_prompt.map(str::to_string),
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: None,
            skill_references: None,
            flow_tool_overlay: None,
            pre_turn_context_appends: Vec::new(),
            turn_metadata: None,
        }
    }

    #[tokio::test]
    async fn test_persistent_load_persisted_returns_stored_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify load_persisted returns the session.
        // We can't construct a full PersistentSessionService without a SessionAgentBuilder,
        // so test the store path directly via the same logic.
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id(), &id);
    }

    #[tokio::test]
    async fn test_persistent_load_persisted_returns_none_for_unknown() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let unknown = SessionId::new();
        let loaded = store.load(&unknown).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_persistent_archive_deletes_from_store() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify it exists
        assert!(store.load(&id).await.unwrap().is_some());

        // Delete (simulating archive store cleanup)
        store.delete(&id).await.unwrap();

        // Verify it's gone
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_store_checkpointer_saves_session() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            blob_store: memory_blob_store(),
            gate,
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
            enabled: true,
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));

        // Checkpoint should persist the session
        checkpointer.checkpoint(&session).await;

        let loaded = store.load(session.id()).await.unwrap();
        assert!(
            loaded.is_some(),
            "session should be persisted after checkpoint"
        );
        let loaded = loaded.unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(loaded.messages().len(), session.messages().len());
    }

    #[tokio::test]
    async fn test_store_checkpointer_suppressed_after_cancellation() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            blob_store: memory_blob_store(),
            gate: Arc::clone(&gate),
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
            enabled: true,
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));

        // First checkpoint should persist (message count changed)
        checkpointer.checkpoint(&session).await;
        assert!(store.load(session.id()).await.unwrap().is_some());

        // Simulate archive: acquire gate, set cancelled, delete
        {
            let mut guard = gate.cancelled.lock().await;
            *guard = true;
            store.delete(session.id()).await.unwrap();
        }

        // Checkpoint after cancellation should be a no-op
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("world".to_string()),
        ));
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_none(),
            "cancelled checkpointer should not write session back"
        );
    }

    #[tokio::test]
    async fn test_store_checkpointer_skips_unchanged_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            blob_store: memory_blob_store(),
            gate,
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
            enabled: true,
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));

        // First checkpoint saves (message count changed from 0 -> 1)
        checkpointer.checkpoint(&session).await;
        assert!(store.load(session.id()).await.unwrap().is_some());

        // Delete from store to detect whether the next checkpoint writes
        store.delete(session.id()).await.unwrap();

        // Second checkpoint with same session is skipped (count still 1)
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_none(),
            "unchanged session should not be re-saved"
        );

        // Add a message and checkpoint again — should save
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("world".to_string()),
        ));
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_some(),
            "changed session should be saved"
        );
    }

    #[tokio::test]
    async fn test_create_session_externalizes_inline_images_in_persisted_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let blob_store = memory_blob_store();
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            blob_store.clone(),
        );

        let result = service
            .create_session(CreateSessionRequest {
                prompt: image_prompt("create"),
                ..create_request("ignored", InitialTurnPolicy::RunImmediately)
            })
            .await
            .expect("create_session should succeed");

        let persisted = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("persisted session should exist");

        assert_no_inline_images_in_session(&persisted);
        let blob_id = persisted
            .messages()
            .iter()
            .find_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Image {
                        data: ImageData::Blob { blob_id },
                        ..
                    } => Some(blob_id.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("persisted image should be externalized");
        assert!(
            blob_store
                .get(&blob_id)
                .await
                .expect("blob should be persisted")
                .data
                .contains("base64-create")
        );
    }

    #[tokio::test]
    async fn test_start_turn_externalizes_new_inline_images_in_persisted_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        service
            .start_turn(
                &created.session_id,
                StartTurnRequest {
                    prompt: image_prompt("turn"),
                    ..start_turn_request("ignored")
                },
            )
            .await
            .expect("start_turn should succeed");

        let persisted = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("persisted session should exist");
        assert_no_inline_images_in_session(&persisted);
    }

    #[tokio::test]
    async fn test_apply_runtime_turn_externalizes_inline_images_in_runtime_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            ImagePreservingBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let run_id = RunId::new();
        let mut req = runtime_content_turn_request("ignored");
        req.prompt = image_prompt("runtime");
        service
            .apply_runtime_turn(
                &created.session_id,
                run_id,
                req,
                RunApplyBoundary::RunStart,
                vec![],
            )
            .await
            .expect("apply_runtime_turn should succeed");

        let runtime_id =
            super::PersistentSessionService::<ImagePreservingBuilder>::runtime_id_for_session(
                &created.session_id,
            );
        let snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("runtime snapshot load should succeed")
            .expect("runtime snapshot should exist");
        let persisted: Session =
            serde_json::from_slice(&snapshot).expect("runtime snapshot should deserialize");
        assert_no_inline_images_in_session(&persisted);
    }

    #[tokio::test]
    async fn test_apply_runtime_turn_resume_pending_without_boundary_is_not_run_result() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let run_id = RunId::new();
        let contributing_input_ids = vec![meerkat_core::lifecycle::InputId::new()];
        let mut req = start_turn_request("resume");
        req.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
                ..Default::default()
            },
        );

        let output = service
            .apply_runtime_turn(
                &created.session_id,
                run_id.clone(),
                req,
                RunApplyBoundary::RunStart,
                contributing_input_ids.clone(),
            )
            .await
            .expect("runtime apply should commit typed no-pending terminal");

        assert_eq!(output.receipt.run_id, run_id);
        assert_eq!(
            output.receipt.contributing_input_ids,
            contributing_input_ids
        );
        assert!(matches!(
            output.terminal,
            Some(CoreApplyTerminal::NoPendingBoundary)
        ));
    }

    #[tokio::test]
    async fn test_apply_runtime_turn_rejects_missing_execution_kind_before_no_pending_terminal() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        let error = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                start_turn_request(""),
                RunApplyBoundary::RunStart,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await
            .expect_err(
                "runtime apply must reject missing execution kind before no-pending commit",
            );

        assert!(
            error.to_string().contains("runtime_execution_kind not set"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_failed_runtime_turn_discards_live_pre_turn_context() {
        use futures::StreamExt;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            FailingRunBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let mut events = service
            .subscribe_session_events(&created.session_id)
            .await
            .expect("subscribe_session_events");

        let mut req = start_turn_request("runtime failed turn");
        req.pre_turn_context_appends = vec![PendingSystemContextAppend {
            text: "failed-turn context must not leak".to_string(),
            source: Some("peer_response_terminal:test:req".to_string()),
            idempotency_key: Some("peer_response_terminal:test:req".to_string()),
            accepted_at: meerkat_core::time_compat::SystemTime::now(),
        }];
        req.turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
                ..Default::default()
            },
        );

        let error = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                req,
                RunApplyBoundary::RunStart,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await
            .expect_err("synthetic run failure should propagate");

        assert!(
            error.to_string().contains("synthetic run failure"),
            "unexpected error: {error}"
        );
        let event =
            tokio::time::timeout(std::time::Duration::from_millis(100), events.next()).await;
        assert!(
            matches!(event, Err(_) | Ok(None)),
            "failed runtime turn must not publish pre-turn context lifecycle events: {event:?}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed runtime turn must discard the live session carrying uncommitted pre-turn context"
        );

        let authoritative = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created session should remain durable");
        assert!(
            authoritative
                .system_context_state()
                .is_none_or(|state| state.applied.is_empty()),
            "failed pre-turn context must not be committed to durable session state"
        );
        assert!(
            authoritative.messages().iter().all(|message| {
                !format!("{message:?}").contains("failed-turn context must not leak")
            }),
            "failed pre-turn context must not be committed into durable messages"
        );
    }

    #[tokio::test]
    async fn test_failed_runtime_turn_output_commit_discards_live_session() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");

        store.set_fail_save(true);
        let error = service
            .apply_runtime_turn(
                &created.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime turn with failed commit"),
                RunApplyBoundary::RunStart,
                vec![meerkat_core::lifecycle::InputId::new()],
            )
            .await
            .expect_err("runtime output commit failure should propagate");

        assert!(
            matches!(error, SessionError::Store(_)),
            "expected store error after runtime output commit failure, got {error:?}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed runtime output commit must discard the mutated live session"
        );

        store.set_fail_save(false);
        let persisted = store
            .load(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("deferred session row should remain durable");
        assert!(
            persisted.messages().is_empty(),
            "failed runtime output commit must not persist the mutated turn"
        );
    }

    #[tokio::test]
    async fn test_context_only_runtime_apply_commit_failure_discards_live_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");

        runtime_store.set_fail_boundary_commits(true);
        let error = service
            .apply_runtime_context_appends(
                &created.session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "failed context-only apply must not leak".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("failed-context-only-apply".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect_err("runtime boundary commit failure should propagate");

        assert!(
            error
                .to_string()
                .contains("synthetic runtime boundary commit failure"),
            "unexpected error: {error}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed context-only runtime apply must discard the mutated live session"
        );
        runtime_store.set_fail_boundary_commits(false);
        service
            .create_session(create_request(
                "after failed context apply",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("failed context-only apply should release active capacity");

        let authoritative = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created session should remain durable");
        assert!(
            authoritative.messages().iter().all(|message| {
                !format!("{message:?}").contains("failed context-only apply must not leak")
            }),
            "failed context-only apply must not commit the mutated live context"
        );
    }

    #[tokio::test]
    async fn test_reserved_context_only_runtime_apply_commit_failure_discards_live_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let admission = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("reserve context-only active admission");

        runtime_store.set_fail_boundary_commits(true);
        let error = service
            .apply_runtime_context_appends_with_reserved_admission(
                &created.session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "failed reserved context apply must not leak".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("failed-reserved-context-apply".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                RunApplyBoundary::Immediate,
                vec![InputId::new()],
                admission,
            )
            .await
            .expect_err("runtime boundary commit failure should propagate");

        assert!(
            error
                .to_string()
                .contains("synthetic runtime boundary commit failure"),
            "unexpected error: {error}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed reserved context-only apply must discard the mutated live session"
        );
        runtime_store.set_fail_boundary_commits(false);
        service
            .create_session(create_request(
                "after failed reserved context apply",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("failed reserved context-only apply should release active capacity");
    }

    #[tokio::test]
    async fn test_runtime_backed_create_session_installs_disabled_store_checkpointer() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let builder = CapturingCheckpointerBuilder::new();
        let captured = Arc::clone(&builder.captured);
        let service = PersistentSessionService::new(
            builder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let checkpointer = captured
            .lock()
            .await
            .clone()
            .expect("runtime-backed create_session should still inject a checkpointer");
        let original = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("create_session should persist an authoritative snapshot");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed");

        let mut mutated = original.clone();
        mutated.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "checkpoint should not bypass runtime boundary".to_string(),
            ),
        ));
        checkpointer.checkpoint(&mutated).await;

        let raw_after = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed");
        assert_eq!(
            raw_after.as_ref().map(|session| session.messages().len()),
            raw_before.as_ref().map(|session| session.messages().len()),
            "runtime-backed sessions must not checkpoint directly into the session store before the runtime boundary commits"
        );
    }

    #[tokio::test]
    async fn test_raw_store_delete_removes_seeded_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        assert!(store.load(&id).await.unwrap().is_some());
        store.delete(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_append_system_context_does_not_mutate_archived_store_row() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let created = service
            .create_session(create_request("archived", InitialTurnPolicy::Defer))
            .await
            .expect("create_session should succeed");
        let id = created.session_id.clone();
        service.archive(&id).await.unwrap();
        let archived = store
            .load(&id)
            .await
            .unwrap()
            .expect("archive should retain a durable archived snapshot");

        let err = service
            .append_system_context(
                &id,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-persistent-archive".to_string()),
                },
            )
            .await
            .expect_err("archived session must not be recreated by append");
        assert_eq!(err.code(), "SESSION_NOT_FOUND");
        let persisted = store
            .load(&id)
            .await
            .unwrap()
            .expect("append after archive must preserve the archived store row");
        assert_eq!(persisted.metadata(), archived.metadata());
        assert_eq!(persisted.messages().len(), archived.messages().len());
    }

    #[tokio::test]
    async fn test_persistent_read_history_returns_messages_for_live_and_archived_sessions() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create_session should succeed");
        let id = created.session_id;

        service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect("second turn should succeed");

        let page = service
            .read_history(
                &id,
                SessionHistoryQuery {
                    offset: 1,
                    limit: Some(2),
                },
            )
            .await
            .expect("live history should be readable");
        assert_eq!(page.session_id, id);
        assert_eq!(page.message_count, 4);
        assert_eq!(page.offset, 1);
        assert_eq!(page.limit, Some(2));
        assert!(page.has_more);
        assert_eq!(page.messages.len(), 2);

        service.archive(&id).await.expect("archive should succeed");

        let archived = service
            .read_history(
                &id,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("archived history should remain readable");
        assert_eq!(archived.session_id, id);
        assert_eq!(archived.message_count, 4);
        assert!(!archived.has_more);
        assert_eq!(archived.messages.len(), 4);
    }

    #[tokio::test]
    async fn test_persistent_archived_history_survives_restart_and_cache_eviction() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new_with_archived_history_capacity(
            DummyBuilder,
            1,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let first = service
            .create_session(create_request("first", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create first session");
        service
            .archive(&first.session_id)
            .await
            .expect("archive first session");

        let second = service
            .create_session(create_request("second", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create second session");
        service
            .archive(&second.session_id)
            .await
            .expect("archive second session");

        let restarted = PersistentSessionService::new_with_archived_history_capacity(
            DummyBuilder,
            1,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let archived = restarted
            .read_history(
                &first.session_id,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("archived history should survive restart and cache eviction");
        assert_eq!(archived.session_id, first.session_id);
        assert_eq!(archived.message_count, 2);
        assert_eq!(archived.messages.len(), 2);

        let listed = restarted
            .list(SessionQuery::default())
            .await
            .expect("list sessions");
        assert!(
            listed.is_empty(),
            "archived sessions should remain hidden from list even when stored durably"
        );
    }

    #[tokio::test]
    async fn test_persistent_completed_sessions_do_not_consume_active_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        for index in 0..3 {
            service
                .create_session(create_request(
                    &format!("completed {index}"),
                    InitialTurnPolicy::RunImmediately,
                ))
                .await
                .unwrap_or_else(|err| {
                    panic!("completed session {index} should release active capacity: {err}")
                });
        }

        let sessions = service
            .list(SessionQuery::default())
            .await
            .expect("list sessions");
        assert_eq!(
            sessions.len(),
            3,
            "completed live sessions should remain readable without holding active capacity"
        );
    }

    #[tokio::test]
    async fn test_persistent_deferred_sessions_consume_capacity_until_archived() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("first deferred session should be staged");
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::Defer))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "second deferred session should be rejected while first is staged: {blocked:?}"
        );

        service
            .archive(&staged.session_id)
            .await
            .expect("archiving staged session should release capacity");
        service
            .create_session(create_request("after archive", InitialTurnPolicy::Defer))
            .await
            .expect("deferred capacity should be reusable after archive");
    }

    #[tokio::test]
    async fn test_persistent_context_only_runtime_apply_respects_active_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let candidate = service
            .create_session(create_request(
                "candidate",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("candidate session should complete and release capacity");
        let blocker = service
            .create_session(create_request("blocker", InitialTurnPolicy::Defer))
            .await
            .expect("deferred blocker should hold active capacity");

        let blocked = service
            .apply_runtime_context_appends(
                &candidate.session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "capacity-bounded context append".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("ctx-capacity".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "context-only runtime apply should respect active capacity: {blocked:?}"
        );

        service
            .archive(&blocker.session_id)
            .await
            .expect("archive should release deferred blocker capacity");
        service
            .apply_runtime_context_appends(
                &candidate.session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "capacity-bounded context append after archive".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("ctx-capacity-after-archive".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("context-only runtime apply should proceed after capacity is released");
    }

    #[tokio::test]
    async fn test_reserved_start_turn_preserves_admission_for_not_found_recovery() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let candidate = service
            .create_session(create_request(
                "candidate",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("candidate session should complete and release capacity");
        let persisted = store
            .load(&candidate.session_id)
            .await
            .expect("load candidate")
            .expect("candidate should be persisted");
        let admission = service
            .reserve_runtime_turn_admission(&candidate.session_id)
            .await
            .expect("candidate admission should reserve active capacity");

        store
            .delete(&candidate.session_id)
            .await
            .expect("delete persisted candidate");
        service
            .discard_live_session(&candidate.session_id)
            .await
            .expect("discard live candidate");

        let start_req = StartTurnRequest {
            prompt: "recover".to_string().into(),
            system_prompt: None,
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: None,
            skill_references: None,
            flow_tool_overlay: None,
            pre_turn_context_appends: Vec::new(),
            turn_metadata: None,
        };
        let (error, recovered_admission) = service
            .start_turn_with_recoverable_reserved_admission(
                &candidate.session_id,
                start_req,
                admission,
            )
            .await
            .expect_err("missing live and persisted session should return NotFound");
        assert!(
            matches!(error, SessionError::NotFound { .. }),
            "expected recoverable NotFound, got {error:?}"
        );
        let recovered_admission =
            recovered_admission.expect("NotFound must return the reserved admission");

        let blocker = Session::new();
        store.save(&blocker).await.expect("persist blocker");
        let blocked = service.reserve_runtime_turn_admission(blocker.id()).await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "recovered admission should still hold active capacity"
        );

        service
            .create_session_with_reserved_admission(resume_request(persisted), recovered_admission)
            .await
            .expect("fallback materialization should reuse recovered admission");
    }

    #[tokio::test]
    async fn test_runtime_turn_admission_joins_existing_live_session_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("joiner", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");
        let first = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("first active admission should reserve capacity");
        let second = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("second same-session admission should join existing capacity");

        let blocker = Session::new();
        let blocker_id = blocker.id().clone();
        store.save(&blocker).await.expect("persist blocker");
        let blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "joined same-session admission must still consume only one active slot"
        );

        drop(first);
        let still_blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            still_blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "capacity must remain held until the final same-session lease drops"
        );

        drop(second);
        service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("dropping all same-session leases should release capacity");
    }

    #[tokio::test]
    async fn test_deferred_runtime_admission_restore_survives_joined_active_lease() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            FailingOverlayBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("create deferred session");
        let promoted = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("promote staged admission");
        let joined = service
            .reserve_runtime_turn_admission(&created.session_id)
            .await
            .expect("join promoted active admission");

        let mut start_req = start_turn_request("resume staged");
        start_req.flow_tool_overlay = Some(meerkat_core::service::TurnToolOverlay {
            allowed_tools: Some(vec!["blocked-before-run".to_string()]),
            blocked_tools: None,
        });
        let error = service
            .start_turn_with_reserved_admission(&created.session_id, start_req, promoted)
            .await
            .expect_err("flow overlay setup should fail before the staged turn runs");
        assert!(
            error.to_string().contains("synthetic flow overlay failure"),
            "unexpected start_turn error: {error:?}"
        );

        let blocker = Session::new();
        let blocker_id = blocker.id().clone();
        store.save(&blocker).await.expect("persist blocker");
        let blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "joined active lease should keep capacity busy until it drops"
        );

        drop(joined);
        let still_blocked = service.reserve_runtime_turn_admission(&blocker_id).await;
        assert!(
            still_blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "final joined release should restore the staged session capacity instead of freeing it"
        );
    }

    #[tokio::test]
    async fn test_reserved_create_admission_cancel_during_create_releases_capacity() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingBuildBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let persisted = Session::new();
        let persisted_id = persisted.id().clone();
        store
            .save(&persisted)
            .await
            .expect("persist source session");
        let admission = service
            .reserve_runtime_turn_admission(&persisted_id)
            .await
            .expect("reserve create admission");

        let service_for_task = Arc::clone(&service);
        let create_task = tokio::spawn(async move {
            service_for_task
                .create_session_with_reserved_admission(resume_request(persisted), admission)
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            builder.entered_notify.notified(),
        )
        .await
        .expect("reserved create should reach the blocking builder");

        create_task.abort();
        let aborted = create_task
            .await
            .expect_err("aborted reserved create task should report cancellation");
        assert!(aborted.is_cancelled());

        let blocker = Session::new();
        let blocker_id = blocker.id().clone();
        store.save(&blocker).await.expect("persist blocker");
        service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("aborted reserved create should release active capacity");
    }

    #[tokio::test]
    async fn test_persistent_deferred_create_save_failure_discards_live_capacity() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        );

        store.set_fail_save(true);
        let failed = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await;
        assert!(
            failed
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("forced save failure")),
            "deferred create should surface durable save failure: {failed:?}"
        );

        store.set_fail_save(false);
        service
            .create_session(create_request(
                "after failed create",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("failed deferred create should discard live capacity");
    }

    #[tokio::test]
    async fn test_persistent_archived_resume_session_rejected_without_capacity_leak() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut archived = Session::new();
        archived.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
        store.save(&archived).await.expect("save archived session");

        let rejected = service.create_session(resume_request(archived)).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "archived resume should be rejected before reserving capacity: {rejected:?}"
        );

        service
            .create_session(create_request(
                "after archived resume",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("rejected archived resume should not leak active capacity");
    }

    #[tokio::test]
    async fn test_persistent_stale_resume_rechecks_current_archived_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let stale_resume = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load authoritative")
            .expect("session should exist");

        service
            .archive(&created.session_id)
            .await
            .expect("archive should succeed");

        let rejected = service.create_session(resume_request(stale_resume)).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "resume must recheck the current durable archive state: {rejected:?}"
        );
        assert!(
            !service
                .has_live_session(&created.session_id)
                .await
                .expect("live check should succeed"),
            "stale resume must not recreate an archived live session"
        );
    }

    #[tokio::test]
    async fn test_persistent_transient_archive_marker_requires_marked_resume_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let stale_resume = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load authoritative")
            .expect("session should exist");

        service
            .archive(&created.session_id)
            .await
            .expect("archive should succeed");
        service
            .mark_transient_pending_archive(&created.session_id)
            .await
            .expect("mark transient archived snapshot");
        let stored = service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load transient archived snapshot")
            .expect("transient archived snapshot should exist");
        assert!(metadata_marks_archived(stored.metadata()));
        assert!(metadata_marks_transient_pending_archive(stored.metadata()));

        let rejected = service.create_session(resume_request(stale_resume)).await;
        assert!(
            matches!(rejected, Err(SessionError::NotFound { .. })),
            "unmarked stale resume must not resurrect a transient archived snapshot: {rejected:?}"
        );
        service
            .create_session(create_request(
                "after stale transient resume",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("rejected stale transient resume should not leak active capacity");
    }

    #[tokio::test]
    async fn test_persistent_deferred_capacity_releases_after_first_turn() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("deferred session should be staged");
        service
            .start_turn(&staged.session_id, start_turn_request("materialize"))
            .await
            .expect("first turn should materialize the deferred session");
        service
            .create_session(create_request("next staged", InitialTurnPolicy::Defer))
            .await
            .expect("completed first turn should release deferred capacity");
    }

    #[tokio::test]
    async fn test_persistent_runtime_turn_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let turn_service = Arc::clone(&service);
        let turn_id = created.session_id.clone();
        let mut turn_task = Box::pin(tokio::spawn(async move {
            turn_service
                .apply_runtime_turn(
                    &turn_id,
                    RunId::new(),
                    runtime_content_turn_request("turn during archive"),
                    RunApplyBoundary::RunStart,
                    vec![meerkat_core::lifecycle::InputId::new()],
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut turn_task)
                .await
                .is_err(),
            "runtime turn should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let turn_result = turn_task.await.expect("turn task should join");
        assert!(
            matches!(turn_result, Err(SessionError::NotFound { .. })),
            "turn that waited behind archive should see archived session as not found: {turn_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_context_only_runtime_apply_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let apply_service = Arc::clone(&service);
        let apply_id = created.session_id.clone();
        let mut apply_task = Box::pin(tokio::spawn(async move {
            apply_service
                .apply_runtime_context_appends(
                    &apply_id,
                    RunId::new(),
                    vec![PendingSystemContextAppend {
                        text: "context during archive".to_string(),
                        source: Some("test".to_string()),
                        idempotency_key: Some("archive-race".to_string()),
                        accepted_at: meerkat_core::time_compat::SystemTime::now(),
                    }],
                    vec![meerkat_core::lifecycle::InputId::new()],
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut apply_task)
                .await
                .is_err(),
            "context-only runtime apply should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let apply_result = apply_task.await.expect("apply task should join");
        assert!(
            matches!(apply_result, Err(SessionError::NotFound { .. })),
            "context-only apply that waited behind archive should see archived session as not found: {apply_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_external_user_append_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let append_service = Arc::clone(&service);
        let append_id = created.session_id.clone();
        let mut append_task = Box::pin(tokio::spawn(async move {
            append_service
                .append_external_user_content(
                    &append_id,
                    ContentInput::Text("external user during archive".to_string()),
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut append_task)
                .await
                .is_err(),
            "external user append should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let append_result = append_task.await.expect("append task should join");
        assert!(
            matches!(append_result, Err(SessionError::NotFound { .. })),
            "external user append that waited behind archive should see archived session as not found: {append_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_external_assistant_append_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::RunImmediately))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let append_service = Arc::clone(&service);
        let append_id = created.session_id.clone();
        let mut append_task = Box::pin(tokio::spawn(async move {
            append_service
                .append_external_assistant_output(
                    &append_id,
                    vec![AssistantBlock::Text {
                        text: "external assistant during archive".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage::default(),
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut append_task)
                .await
                .is_err(),
            "external assistant append should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let append_result = append_task.await.expect("append task should join");
        assert!(
            matches!(append_result, Err(SessionError::NotFound { .. })),
            "external assistant append that waited behind archive should see archived session as not found: {append_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_keep_alive_update_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DummyBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request_with_metadata(
                "seed",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let update_service = Arc::clone(&service);
        let update_id = created.session_id.clone();
        let mut update_task = Box::pin(tokio::spawn(async move {
            update_service
                .apply_runtime_session_keep_alive(&update_id, true)
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut update_task)
                .await
                .is_err(),
            "keep-alive update should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let update_result = update_task.await.expect("update task should join");
        assert!(
            matches!(update_result, Err(SessionError::NotFound { .. })),
            "keep-alive update that waited behind archive should see archived session as not found: {update_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_external_tool_dispatch_waits_for_archive_gate() {
        let store = Arc::new(BlockingArchiveSaveStore::new());
        let service = Arc::new(PersistentSessionService::new(
            ToolDispatchBuilder,
            2,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        ));
        let created = service
            .create_session(create_request("seed", InitialTurnPolicy::Defer))
            .await
            .expect("create live session");

        store.block_archived_saves();
        let archive_service = Arc::clone(&service);
        let archive_id = created.session_id.clone();
        let archive_task = tokio::spawn(async move { archive_service.archive(&archive_id).await });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.wait_for_archived_save(),
        )
        .await
        .expect("archive should reach blocked durable save");

        let dispatch_service = Arc::clone(&service);
        let dispatch_id = created.session_id.clone();
        let mut dispatch_task = Box::pin(tokio::spawn(async move {
            dispatch_service
                .dispatch_external_tool_call(
                    &dispatch_id,
                    ToolCall::new(
                        "call-during-archive".to_string(),
                        "tool_catalog_load".to_string(),
                        serde_json::json!({}),
                    ),
                )
                .await
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut dispatch_task)
                .await
                .is_err(),
            "external tool dispatch should wait while archive owns the per-session gate"
        );

        store.release_archived_save();
        archive_task
            .await
            .expect("archive task should join")
            .expect("archive should succeed");
        let dispatch_result = dispatch_task.await.expect("dispatch task should join");
        assert!(
            matches!(dispatch_result, Err(SessionError::NotFound { .. })),
            "external tool dispatch that waited behind archive should see archived session as not found: {dispatch_result:?}"
        );
    }

    #[tokio::test]
    async fn test_persistent_eager_create_bounds_agent_builds() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingBuildBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let service_for_first = Arc::clone(&service);
        let first = tokio::spawn(async move {
            service_for_first
                .create_session(create_request("first", InitialTurnPolicy::RunImmediately))
                .await
        });

        builder.entered_notify.notified().await;
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::RunImmediately))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "second eager create should be rejected before entering build: {blocked:?}"
        );
        assert_eq!(
            builder.entered_builds.load(Ordering::Acquire),
            1,
            "capacity should prevent the second create from starting agent build"
        );
        assert_eq!(
            builder.max_concurrent_builds.load(Ordering::Acquire),
            1,
            "agent build concurrency must stay bounded by max_sessions"
        );

        builder.release_notify.notify_waiters();
        first
            .await
            .expect("first create task should join")
            .expect("first eager create should complete");
    }

    #[tokio::test]
    async fn test_ephemeral_archive_during_deferred_first_turn_keeps_capacity_until_turn_stops() {
        let builder = BlockingRunBuilder::new();
        let service = Arc::new(EphemeralSessionService::new(builder.clone(), 1));

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("deferred session should be staged");
        let service_for_turn = Arc::clone(&service);
        let session_for_turn = staged.session_id.clone();
        let first_turn = tokio::spawn(async move {
            service_for_turn
                .start_turn(&session_for_turn, start_turn_request("materialize"))
                .await
        });

        builder.entered_notify.notified().await;
        service
            .archive(&staged.session_id)
            .await
            .expect("archive should request shutdown while first turn runs");
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::Defer))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "archive during a deferred first turn must not release capacity early: {blocked:?}"
        );

        builder.release_notify.notify_waiters();
        let _ = first_turn.await.expect("first turn task should join");
        service
            .create_session(create_request("after turn", InitialTurnPolicy::Defer))
            .await
            .expect("capacity should release after archived turn stops");
    }

    #[tokio::test]
    async fn test_persistent_discard_during_deferred_first_turn_keeps_capacity_until_turn_stops() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = BlockingRunBuilder::new();
        let service = Arc::new(PersistentSessionService::new(
            builder.clone(),
            1,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        ));

        let staged = service
            .create_session(create_request("staged", InitialTurnPolicy::Defer))
            .await
            .expect("deferred session should be staged");
        let service_for_turn = Arc::clone(&service);
        let session_for_turn = staged.session_id.clone();
        let first_turn = tokio::spawn(async move {
            service_for_turn
                .start_turn(&session_for_turn, start_turn_request("materialize"))
                .await
        });

        builder.entered_notify.notified().await;
        service
            .discard_live_session(&staged.session_id)
            .await
            .expect("discard should request shutdown while first turn runs");
        let blocked = service
            .create_session(create_request("blocked", InitialTurnPolicy::Defer))
            .await;
        assert!(
            blocked
                .as_ref()
                .err()
                .is_some_and(|err| err.to_string().contains("Max sessions")),
            "discard during a deferred first turn must not release capacity early: {blocked:?}"
        );

        builder.release_notify.notify_waiters();
        let _ = first_turn.await.expect("first turn task should join");
        service
            .create_session(create_request("after turn", InitialTurnPolicy::Defer))
            .await
            .expect("capacity should release after discarded turn stops");
    }

    #[tokio::test]
    async fn test_append_system_context_repersist_live_session_when_store_row_missing() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;

        store
            .delete(&id)
            .await
            .expect("test should be able to remove persisted row");
        assert!(
            store.load(&id).await.unwrap().is_none(),
            "store row should be absent before append"
        );

        let result = service
            .append_system_context(
                &id,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-persistent-live".to_string()),
                },
            )
            .await
            .expect("live append should repersist from the live session snapshot");
        assert_eq!(
            result.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let stored = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("append should restore the persisted row");
        let state = stored
            .system_context_state()
            .expect("restored row should contain pending system-context state");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].text, "runtime notice");
    }

    #[tokio::test]
    async fn test_append_system_context_live_save_failure_does_not_mutate_runtime_state() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store) as Arc<dyn SessionStore>,
            None,
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;

        store.set_fail_save(true);
        let err = service
            .append_system_context(
                &id,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-save-failure".to_string()),
                },
            )
            .await
            .expect_err("append should surface the store failure");
        assert_eq!(err.code(), "SESSION_STORE_ERROR");

        let state = service
            .inner
            .system_context_state(&id)
            .await
            .expect("live session should still exist");
        let guard = state.lock().expect("system-context state lock");
        assert!(
            guard.pending.is_empty(),
            "failed append must not mutate live runtime state"
        );
        assert!(
            !guard.seen.contains_key("ctx-save-failure"),
            "failed append must not reserve the idempotency key in live state"
        );
    }

    #[tokio::test]
    async fn test_export_live_session_merges_shared_system_context_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;

        let state = service
            .inner
            .system_context_state(&id)
            .await
            .expect("live session should expose shared system-context state");
        {
            let mut guard = state.lock().expect("system-context state lock");
            guard
                .stage_append(
                    &AppendSystemContextRequest {
                        text: "queued live append".to_string(),
                        source: Some("mob".to_string()),
                        idempotency_key: Some("ctx-export-merge".to_string()),
                    },
                    meerkat_core::time_compat::SystemTime::now(),
                )
                .expect("staging into shared state should succeed");
        }

        let exported = service
            .export_live_session(&id)
            .await
            .expect("export should succeed");
        let exported_state = exported
            .system_context_state()
            .expect("exported session should include merged system-context state");
        assert_eq!(exported_state.pending.len(), 1);
        assert_eq!(exported_state.pending[0].text, "queued live append");
        assert!(
            exported_state.seen.contains_key("ctx-export-merge"),
            "exported session should include the staged idempotency key"
        );
    }

    #[tokio::test]
    async fn test_append_system_context_unknown_session_does_not_allocate_gate() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let unknown = SessionId::new();

        let err = service
            .append_system_context(
                &unknown,
                AppendSystemContextRequest {
                    text: "runtime notice".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-unknown".to_string()),
                },
            )
            .await
            .expect_err("unknown session must fail");
        assert_eq!(err.code(), "SESSION_NOT_FOUND");
        assert!(
            service.checkpointer_gates.lock().await.is_empty(),
            "unknown-session append must not allocate a checkpointer gate"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_create_session_persists_initial_authority_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime-backed create_session should persist an authoritative snapshot");
        assert!(
            stored.messages().len() >= 2,
            "initial turn should be durably persisted for direct SessionService callers"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_list_retains_projected_session_after_live_handle_discard() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should be able to discard the live session");

        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        assert!(
            listed
                .iter()
                .any(|summary| summary.session_id == result.session_id),
            "runtime-backed sessions should remain listable through the session-store projection after the live handle is gone"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_projection_save_failure_fails_closed_and_list_uses_authority() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");
        let raw_message_count = raw_before.messages().len();

        fail_store.set_fail_save(true);
        let error = service
            .start_turn(
                &result.session_id,
                start_turn_request("runtime authority commits before projection failure"),
            )
            .await
            .expect_err("projection save failure after runtime authority commit must fail closed");
        assert!(
            matches!(error, SessionError::Store(_)),
            "projection failure should surface as a store error, got {error:?}"
        );

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should still contain the committed turn");
        assert!(
            authoritative.messages().len() > raw_message_count,
            "runtime authority should be ahead of the stale session-store projection"
        );
        let raw_after = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed after failure")
            .expect("stale projection should remain present");
        assert_eq!(
            raw_after.messages().len(),
            raw_message_count,
            "failed projection update must not be silently refreshed"
        );
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after projection failure"),
            "post-commit projection failure must evict stale live state"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should not fail while authoritative runtime snapshot exists");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("stale projection row should only identify the authoritative session");
        assert_eq!(
            summary.message_count,
            authoritative.messages().len(),
            "list() must report runtime authority, not stale SessionStore projection metadata"
        );
        assert_ne!(
            summary.message_count,
            raw_after.messages().len(),
            "list() must not present stale projection state as fresh canonical state"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_callback_pending_projection_failure_surfaces_store_error() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CallbackPendingBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");

        fail_store.set_fail_save(true);
        let error = service
            .start_turn(
                &result.session_id,
                start_turn_request("callback pending after projection failure"),
            )
            .await
            .expect_err("callback-pending projection failure must surface the store error");
        assert!(
            matches!(error, SessionError::Store(_)),
            "projection failure should win over callback-pending error, got {error:?}"
        );
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after callback projection failure"),
            "callback-pending projection failure must evict stale live state"
        );
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should retain the callback-pending mutation");
        assert!(
            authoritative.messages().len() > raw_before.messages().len(),
            "runtime authority should be ahead of the stale callback projection"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_resume_callback_pending_projection_failure_surfaces_store_error() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            CallbackPendingBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");
        let resume_source = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist");
        let raw_before = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should succeed")
            .expect("initial projection should exist");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should model resume after live handle discard");

        let mut req = create_request(
            "resume callback pending after projection failure",
            meerkat_core::service::InitialTurnPolicy::RunImmediately,
        );
        req.build = Some(SessionBuildOptions {
            resume_session: Some(resume_source),
            ..Default::default()
        });

        fail_store.set_fail_save(true);
        let error = service
            .create_session(req)
            .await
            .expect_err("resume callback-pending projection failure must surface the store error");
        assert!(
            matches!(error, SessionError::Store(_)),
            "projection failure should win over resume callback-pending error, got {error:?}"
        );
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after resume callback projection failure"),
            "resume callback-pending projection failure must evict stale live state"
        );
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should retain the resume callback-pending mutation");
        assert!(
            authoritative.messages().len() > raw_before.messages().len(),
            "runtime authority should be ahead of the stale resume callback projection"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_direct_start_turn_snapshot_commit_failure_discards_live_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let initial_authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist before failed turn");

        runtime_store.set_fail_snapshot_commits(true);
        let error = service
            .start_turn(
                &result.session_id,
                start_turn_request("failed direct snapshot commit must not leak"),
            )
            .await
            .expect_err("runtime snapshot commit failure must propagate");
        assert!(
            error
                .to_string()
                .contains("synthetic runtime snapshot commit failure"),
            "unexpected error: {error}"
        );
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("live-session status should succeed"),
            "failed direct start_turn snapshot commit must discard mutated live state"
        );

        runtime_store.set_fail_snapshot_commits(false);
        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("created session should remain durable");
        assert_eq!(
            authoritative.messages().len(),
            initial_authoritative.messages().len(),
            "failed direct start_turn must not advance runtime authority"
        );
        assert!(
            authoritative.messages().iter().all(|message| {
                !format!("{message:?}").contains("failed direct snapshot commit must not leak")
            }),
            "failed direct start_turn must not leak into durable authority"
        );

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should fall back to durable runtime authority");
        assert_eq!(
            view.state.message_count,
            initial_authoritative.messages().len(),
            "read() must not report the failed live mutation as clean truth"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed after failed snapshot commit");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("durable session should remain listable");
        assert_eq!(
            summary.message_count,
            initial_authoritative.messages().len(),
            "list() must not report the failed live mutation as clean truth"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_control_projection_save_failure_discards_live_state() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let append_result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed before projection failures");

        fail_store.set_fail_save(true);
        let append_error = service
            .append_system_context(
                &append_result.session_id,
                AppendSystemContextRequest {
                    text: "runtime-backed control context".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("control-projection-failure".to_string()),
                },
            )
            .await
            .expect_err("control projection save failure must fail closed");
        assert!(
            matches!(
                append_error,
                SessionControlError::Session(SessionError::Store(_))
            ),
            "projection failure should surface as a store error, got {append_error:?}"
        );
        assert!(
            !service
                .has_live_session(&append_result.session_id)
                .await
                .expect("status should succeed after append projection failure"),
            "append projection failure after runtime commit must evict stale live state"
        );
        let append_authoritative = service
            .load_authoritative_session(&append_result.session_id)
            .await
            .expect("authoritative load should succeed after append failure")
            .expect("runtime authority should retain the committed append");
        let append_state = append_authoritative
            .system_context_state()
            .expect("runtime authority should carry the committed append");
        assert_eq!(append_state.pending.len(), 1);
        assert_eq!(
            append_state.pending[0].text,
            "runtime-backed control context"
        );
        let append_raw = store
            .load(&append_result.session_id)
            .await
            .expect("raw projection load should succeed after append failure")
            .expect("stale append projection should remain present");
        let raw_pending_context_count = append_raw
            .system_context_state()
            .map_or(0, |state| state.pending.len());
        assert_eq!(
            raw_pending_context_count, 0,
            "failed append projection update must not silently refresh raw store state"
        );

        fail_store.set_fail_save(false);
        let stage_result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("second create_session should succeed before projection failures");

        fail_store.set_fail_save(true);
        let stage_error = service
            .stage_tool_results(
                &stage_result.session_id,
                StageToolResultsRequest {
                    results: vec![ToolResult::new(
                        "tool-call-1".to_string(),
                        "callback result".to_string(),
                        false,
                    )],
                },
            )
            .await
            .expect_err("staged tool-result projection save failure must fail closed");
        assert!(
            matches!(stage_error, SessionError::Store(_)),
            "projection failure should surface as a store error, got {stage_error:?}"
        );
        assert!(
            !service
                .has_live_session(&stage_result.session_id)
                .await
                .expect("status should succeed after stage projection failure"),
            "stage projection failure after runtime commit must evict stale live state"
        );
        let stage_authoritative = service
            .load_authoritative_session(&stage_result.session_id)
            .await
            .expect("authoritative load should succeed after stage failure")
            .expect("runtime authority should retain the staged tool results");
        let stage_deferred = stage_authoritative
            .deferred_turn_state()
            .expect("runtime authority should carry staged tool results");
        assert_eq!(stage_deferred.pending_tool_results.len(), 1);
        let stage_raw = store
            .load(&stage_result.session_id)
            .await
            .expect("raw projection load should succeed after stage failure")
            .expect("stale stage projection should remain present");
        let raw_tool_result_count = stage_raw
            .deferred_turn_state()
            .map_or(0, |state| state.pending_tool_results.len());
        assert_eq!(
            raw_tool_result_count, 0,
            "failed stage projection update must not silently refresh raw store state"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_start_turn_persists_follow_up_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;
        let initial_count = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("create_session should persist authoritative snapshot")
            .messages()
            .len();

        service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect("start_turn should succeed");

        let stored = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime-backed start_turn should update authoritative snapshot");
        assert!(
            stored.messages().len() > initial_count,
            "follow-up turn should be durably saved for direct SessionService callers"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_stages_successfully() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        let append = service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "Remember runtime-backed context".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("runtime-backed-ctx".to_string()),
                },
            )
            .await
            .expect("runtime-backed append_system_context should remain available");
        assert_eq!(
            append.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("append should persist updated system-context state");
        let state = stored
            .system_context_state()
            .expect("runtime-backed append should persist pending context");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].text, "Remember runtime-backed context");
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_does_not_persist_uncommitted_live_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .inner
            .apply_runtime_system_context(
                &result.session_id,
                vec![PendingSystemContextAppend {
                    text: "uncommitted live context".to_string(),
                    source: Some("live".to_string()),
                    idempotency_key: Some("uncommitted".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
            )
            .await
            .expect("live runtime context mutation should succeed");

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "durable pending context".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("durable-pending".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session snapshot should exist");
        assert!(
            stored.messages().is_empty(),
            "runtime-backed append_system_context must not snapshot uncommitted live session messages into durable authority"
        );
        let state = stored
            .system_context_state()
            .expect("runtime-backed append should persist pending control state");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].text, "durable pending context");
        let raw = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should be kept for listability");
        assert!(
            raw.messages().is_empty(),
            "projection must not include uncommitted live session messages"
        );
        let raw_state = raw
            .system_context_state()
            .expect("projection should mirror committed control state");
        assert_eq!(raw_state.pending.len(), 1);
        assert_eq!(raw_state.pending[0].text, "durable pending context");
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_persists_snapshot_without_boundary_receipt()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        runtime_store.reset_boundary_commits().await;

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "control snapshot should not mint a receipt".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("no-receipt-control-snapshot".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let boundary_commits = runtime_store.boundary_commits().await;
        assert!(
            boundary_commits.is_empty(),
            "non-run control snapshots must not mint synthetic boundary receipts: {boundary_commits:?}"
        );

        let runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&result.session_id);
        let snapshot = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("runtime snapshot load should succeed")
            .expect("control snapshot should still be durable");
        let stored: Session =
            serde_json::from_slice(&snapshot).expect("runtime snapshot should deserialize");
        let state = stored
            .system_context_state()
            .expect("runtime snapshot should carry pending control state");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(
            state.pending[0].text,
            "control snapshot should not mint a receipt"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_uses_newer_runtime_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");

        let stale_store_row = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed");
        assert!(
            stale_store_row
                .as_ref()
                .is_none_or(|session| session.messages().is_empty()),
            "session store should still lag behind the runtime-backed committed snapshot before append"
        );
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                        &result.session_id
                    )
                )
                .await
                .expect("runtime snapshot load should succeed")
                .is_some(),
            "runtime-backed turn should commit a runtime snapshot"
        );

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "runtime append".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("runtime-base".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("append should persist a refreshed authoritative snapshot");
        assert_eq!(
            stored.messages().len(),
            2,
            "append must preserve the newest runtime-committed conversation state instead of rewinding to the stale session-store row"
        );
        let state = stored
            .system_context_state()
            .expect("append should persist pending control state");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].text, "runtime append");
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_without_live_handle_uses_runtime_authority()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should be able to discard the live session");

        let append = service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "persisted runtime append".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("persisted-runtime-append".to_string()),
                },
            )
            .await
            .expect("persisted runtime-backed append should update runtime authority");
        assert_eq!(
            append.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        assert_eq!(
            authoritative.messages().len(),
            2,
            "persisted append must preserve committed runtime transcript truth"
        );
        let state = authoritative
            .system_context_state()
            .expect("runtime authority should carry pending context");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].text, "persisted runtime append");

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should use durable runtime truth after live discard");
        assert_eq!(view.state.message_count, authoritative.messages().len());
        assert!(!view.state.is_active);
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status should not recreate a live handle"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("list should include persisted runtime-backed session");
        assert_eq!(summary.message_count, authoritative.messages().len());

        let restarted = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );
        let resume_source = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("resume source should load from runtime authority")
            .expect("runtime authority should remain present after restart");
        let resumed = restarted
            .create_session(resume_request(resume_source))
            .await
            .expect("resume should materialize the runtime-authoritative snapshot");
        assert_eq!(resumed.session_id, result.session_id);
        let resumed_live = restarted
            .export_live_session(&result.session_id)
            .await
            .expect("resumed live session should export");
        let resumed_state = resumed_live
            .system_context_state()
            .expect("resumed session should preserve runtime-authoritative pending context");
        assert_eq!(resumed_state.pending[0].text, "persisted runtime append");
    }

    #[tokio::test]
    async fn test_authoritative_runtime_snapshot_ignores_store_owned_session_metadata() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request_with_metadata(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let mut store_session = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("initial runtime snapshot should exist");
        store_session
            .set_build_state(meerkat_core::SessionBuildState {
                system_prompt: Some("store-owned recovery prompt".to_string()),
                ..Default::default()
            })
            .expect("build state should serialize");
        store
            .save(&store_session)
            .await
            .expect("test should seed a compatibility store projection");

        let mut runtime_session = store_session.clone();
        runtime_session.push(Message::User(UserMessage::text_with_render_metadata(
            "runtime-only turn",
            None,
        )));
        runtime_session.remove_metadata(SESSION_METADATA_KEY);
        runtime_session.remove_metadata(meerkat_core::SESSION_BUILD_STATE_KEY);
        let session_snapshot =
            serde_json::to_vec(&runtime_session).expect("runtime snapshot should serialize");
        runtime_store
            .commit_session_boundary(
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(
                    &result.session_id,
                ),
                meerkat_runtime::SessionDelta { session_snapshot },
                RunId::new(),
                RunApplyBoundary::Immediate,
                vec![],
                vec![],
            )
            .await
            .expect("runtime snapshot commit should succeed");

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");

        assert_eq!(
            authoritative.messages().len(),
            runtime_session.messages().len(),
            "newer runtime snapshot still owns committed conversation state"
        );
        assert_eq!(
            authoritative.updated_at(),
            runtime_session.updated_at(),
            "store projection metadata must not change runtime snapshot timestamps"
        );
        assert!(
            authoritative.session_metadata().is_none(),
            "runtime authority must not backfill metadata from the store projection"
        );
        assert!(
            authoritative.build_state().is_none(),
            "runtime authority must not backfill build state from the store projection"
        );
    }

    #[tokio::test]
    async fn test_persisted_runtime_state_loads_runtime_store_state() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );
        let session_id = SessionId::new();
        runtime_store
            .persist_runtime_state(
                &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&session_id),
                meerkat_runtime::RuntimeState::Retired,
            )
            .await
            .expect("runtime state should persist");

        assert_eq!(
            service
                .persisted_runtime_state(&session_id)
                .await
                .expect("runtime state load should succeed"),
            Some(meerkat_runtime::RuntimeState::Retired)
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_authoritative_load_ignores_store_only_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut raw_projection = Session::new();
        raw_projection.push(Message::User(UserMessage::text(
            "legacy raw store row".to_string(),
        )));
        let id = raw_projection.id().clone();
        store
            .save(&raw_projection)
            .await
            .expect("test should seed a raw store projection");

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed");
        assert!(
            authoritative.is_none(),
            "store-only projections must not be promoted into runtime authority"
        );
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id)
                )
                .await
                .expect("runtime snapshot load should succeed")
                .is_none(),
            "authoritative load must not create runtime authority from a store-only projection"
        );
        let raw_store_row = store
            .load(&id)
            .await
            .expect("raw store load should succeed")
            .expect("test projection should remain present");
        assert_eq!(
            raw_store_row.messages().len(),
            raw_projection.messages().len(),
            "compatibility projection should remain inert in the raw store"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_authoritative_load_accepts_legacy_session_uuid_runtime_alias() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut legacy_runtime_session = Session::new();
        legacy_runtime_session.push(Message::User(UserMessage::text(
            "legacy runtime alias row".to_string(),
        )));
        let id = legacy_runtime_session.id().clone();
        let snapshot =
            serde_json::to_vec(&legacy_runtime_session).expect("legacy session should serialize");
        let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&id);
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        assert_ne!(
            canonical_runtime_id, legacy_runtime_alias,
            "canonical runtime id must be distinct from the legacy raw session UUID alias"
        );

        runtime_store
            .commit_session_boundary(
                &legacy_runtime_alias,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: snapshot,
                },
                RunId::new(),
                RunApplyBoundary::Immediate,
                vec![],
                vec![],
            )
            .await
            .expect("test should seed a legacy runtime alias snapshot");

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("legacy runtime alias should remain readable");
        assert_eq!(
            authoritative.messages().len(),
            legacy_runtime_session.messages().len(),
            "runtime-backed resume must preserve legacy raw alias snapshots"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_authoritative_load_keeps_canonical_over_newer_legacy_runtime_alias_snapshot()
     {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut canonical_session = Session::new();
        canonical_session.push(Message::User(UserMessage::text(
            "canonical runtime alias row".to_string(),
        )));
        let id = canonical_session.id().clone();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        runtime_store
            .commit_session_boundary(
                &canonical_runtime_id,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&canonical_session)
                        .expect("canonical session should serialize"),
                },
                RunId::new(),
                RunApplyBoundary::Immediate,
                vec![],
                vec![],
            )
            .await
            .expect("test should seed a canonical runtime alias snapshot");

        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let mut legacy_session = canonical_session.clone();
        legacy_session.push(Message::User(UserMessage::text(
            "newer legacy runtime alias row".to_string(),
        )));
        runtime_store
            .commit_session_boundary(
                &LogicalRuntimeId::legacy_session_uuid_alias(&id),
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&legacy_session)
                        .expect("legacy session should serialize"),
                },
                RunId::new(),
                RunApplyBoundary::Immediate,
                vec![],
                vec![],
            )
            .await
            .expect("test should seed a newer legacy runtime alias snapshot");

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime alias snapshot should exist");
        assert_eq!(
            authoritative.messages().len(),
            canonical_session.messages().len(),
            "canonical runtime authority must not be overwritten by a newer legacy raw alias snapshot"
        );
    }

    #[tokio::test]
    async fn test_runtime_authority_ignores_corrupt_legacy_alias_after_canonical_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut canonical_session = Session::new();
        canonical_session.push(Message::User(UserMessage::text(
            "canonical runtime alias row".to_string(),
        )));
        let id = canonical_session.id().clone();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        runtime_store
            .commit_session_boundary(
                &canonical_runtime_id,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&canonical_session)
                        .expect("canonical session should serialize"),
                },
                RunId::new(),
                RunApplyBoundary::Immediate,
                vec![],
                vec![],
            )
            .await
            .expect("test should seed a canonical runtime snapshot");

        runtime_store
            .override_session_snapshot(
                LogicalRuntimeId::legacy_session_uuid_alias(&id),
                b"{not valid json".to_vec(),
            )
            .await;

        let authoritative = service
            .load_authoritative_session(&id)
            .await
            .expect("valid canonical authority must not be poisoned by corrupt legacy fallback")
            .expect("canonical runtime snapshot should exist");
        assert_eq!(
            authoritative.messages().len(),
            canonical_session.messages().len(),
            "canonical runtime authority must win over corrupt legacy fallback data"
        );
    }

    #[tokio::test]
    async fn test_runtime_input_updates_ignore_legacy_alias_load_error_after_canonical_states() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        let input_id = InputId::new();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        let mut input_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        input_state.durability = Some(meerkat_runtime::InputDurability::Durable);
        let stored = StoredInputState {
            state: input_state,
            seed: meerkat_runtime::input_state::InputStateSeed::new_accepted(),
        };
        runtime_store
            .persist_input_state(&canonical_runtime_id, &stored)
            .await
            .expect("test should seed canonical input state");
        runtime_store
            .fail_input_state_load_for(LogicalRuntimeId::legacy_session_uuid_alias(&id))
            .await;

        let run_id = RunId::new();
        let updates = service
            .runtime_input_updates(&id, &run_id, 7, std::slice::from_ref(&input_id))
            .await
            .expect("legacy input-state load failure must not poison canonical updates");

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].state.input_id, input_id);
        assert_eq!(updates[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(updates[0].seed.last_run_id, Some(run_id));
        assert_eq!(updates[0].seed.last_boundary_sequence, Some(7));
    }

    #[tokio::test]
    async fn test_runtime_input_updates_ignore_legacy_alias_load_error_after_empty_canonical_read()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        runtime_store
            .fail_input_state_load_for(LogicalRuntimeId::legacy_session_uuid_alias(&id))
            .await;

        let updates = service
            .runtime_input_updates(&id, &RunId::new(), 0, &[])
            .await
            .expect("legacy input-state load failure must not poison empty canonical updates");

        assert!(
            updates.is_empty(),
            "no-contributor boundary should not need legacy input-state data"
        );
    }

    #[tokio::test]
    async fn test_runtime_input_updates_require_legacy_alias_when_contributor_missing_from_canonical()
     {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        let input_id = InputId::new();
        runtime_store
            .fail_input_state_load_for(LogicalRuntimeId::legacy_session_uuid_alias(&id))
            .await;

        let err = service
            .runtime_input_updates(&id, &RunId::new(), 0, std::slice::from_ref(&input_id))
            .await
            .expect_err(
                "missing canonical contributor must not be silently dropped when legacy is unreadable",
            );

        assert!(
            err.to_string()
                .contains("synthetic legacy input-state load failure"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_runtime_input_updates_prefer_canonical_duplicate_over_newer_stale_legacy_row() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let id = SessionId::new();
        let input_id = InputId::new();
        let canonical_runtime_id =
            PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id);
        let legacy_runtime_id = LogicalRuntimeId::legacy_session_uuid_alias(&id);

        let mut canonical_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        canonical_state.durability = Some(meerkat_runtime::InputDurability::Durable);
        canonical_state.recovery_count = 7;
        let canonical_stored = StoredInputState {
            state: canonical_state.clone(),
            seed: meerkat_runtime::input_state::InputStateSeed::new_accepted(),
        };
        runtime_store
            .persist_input_state(&canonical_runtime_id, &canonical_stored)
            .await
            .expect("test should seed canonical input state");

        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let mut legacy_state = meerkat_runtime::InputState::new_accepted(input_id.clone());
        legacy_state.durability = Some(meerkat_runtime::InputDurability::Durable);
        legacy_state.recovery_count = 99;
        runtime_store
            .persist_input_state(
                &legacy_runtime_id,
                &StoredInputState {
                    state: legacy_state,
                    seed: meerkat_runtime::input_state::InputStateSeed::new_accepted(),
                },
            )
            .await
            .expect("test should seed newer stale legacy input state");

        let updates = service
            .runtime_input_updates(&id, &RunId::new(), 3, std::slice::from_ref(&input_id))
            .await
            .expect("runtime input updates should merge duplicate aliases");

        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0].state.recovery_count, 7,
            "canonical contributor state must win over newer stale legacy duplicate"
        );
        assert_eq!(updates[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(updates[0].seed.last_boundary_sequence, Some(3));
    }

    #[tokio::test]
    async fn test_runtime_backed_read_and_list_ignore_store_only_projection_without_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let mut raw_projection = Session::new();
        raw_projection.push(Message::User(UserMessage::text(
            "store-only row must stay compatibility-only".to_string(),
        )));
        let id = raw_projection.id().clone();
        store
            .save(&raw_projection)
            .await
            .expect("test should seed a raw store projection");

        let read_err = service
            .read(&id)
            .await
            .expect_err("read must not promote a store-only projection");
        assert!(matches!(read_err, SessionError::NotFound { .. }));

        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        assert!(
            listed.iter().all(|summary| summary.session_id != id),
            "list() must not expose store-only projections as runtime authority"
        );
        assert!(
            runtime_store
                .load_session_snapshot(
                    &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id)
                )
                .await
                .expect("runtime snapshot load should succeed")
                .is_none(),
            "read/list must not create runtime authority from a store-only projection"
        );
    }

    #[tokio::test]
    async fn test_authoritative_load_ignores_newer_raw_store_projection_when_runtime_exists() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let runtime_authority = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist");

        let mut raw_store_projection = runtime_authority.clone();
        raw_store_projection.push(Message::User(UserMessage::text(
            "raw store projection must not become authority".to_string(),
        )));
        store
            .save(&raw_store_projection)
            .await
            .expect("test should be able to write a stale raw projection");

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        assert_eq!(
            authoritative.messages().len(),
            runtime_authority.messages().len(),
            "raw session-store rows must not override an existing runtime authority snapshot"
        );
    }

    #[tokio::test]
    async fn test_live_runtime_read_ignores_stale_raw_store_fallback_when_snapshot_load_misses() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        service
            .start_turn(&result.session_id, start_turn_request("live runtime truth"))
            .await
            .expect("live turn should succeed");
        let live = service
            .export_live_session(&result.session_id)
            .await
            .expect("live session should export");

        let mut raw_store_projection = live.clone();
        raw_store_projection.push(Message::User(UserMessage::text(
            "stale raw compatibility row".to_string(),
        )));
        raw_store_projection.set_metadata(
            SESSION_LABELS_KEY,
            serde_json::json!({
                "source": "raw-store",
            }),
        );
        store
            .save(&raw_store_projection)
            .await
            .expect("test should seed a stale raw store projection");

        runtime_store.hide_next_session_snapshot_loads(1);
        let view = service
            .read(&result.session_id)
            .await
            .expect("read should preserve live runtime truth");
        assert_eq!(
            view.state.message_count,
            live.messages().len(),
            "read() must not expose a raw store row when live runtime truth exists"
        );
        assert!(
            view.state.labels.is_empty(),
            "read() must not expose raw store metadata when live runtime truth exists"
        );
        assert!(
            service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status must not let raw fallback evict a live runtime session"
        );

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should remain present");
        assert_eq!(
            authoritative.messages().len(),
            live.messages().len(),
            "raw fallback must not replace durable runtime authority"
        );
    }

    #[tokio::test]
    async fn test_live_runtime_list_status_and_resume_fail_closed_on_stale_raw_store_metadata() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(GatedSnapshotRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        service
            .start_turn(&result.session_id, start_turn_request("live runtime truth"))
            .await
            .expect("live turn should succeed");
        let live = service
            .export_live_session(&result.session_id)
            .await
            .expect("live session should export");

        let mut raw_store_projection = live.clone();
        raw_store_projection.push(Message::User(UserMessage::text(
            "stale raw compatibility row".to_string(),
        )));
        raw_store_projection.set_metadata(
            SESSION_LABELS_KEY,
            serde_json::json!({
                "source": "raw-store",
            }),
        );
        store
            .save(&raw_store_projection)
            .await
            .expect("test should seed a stale raw store projection");

        runtime_store.hide_next_session_snapshot_loads(1);
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("list should include the live runtime-backed session");
        assert_eq!(
            summary.message_count,
            live.messages().len(),
            "list() must report live/runtime metadata instead of raw store fallback"
        );
        assert!(
            summary.labels.is_empty(),
            "list() must not expose labels from a stale raw store projection"
        );
        assert!(
            service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status must remain live after list inspects stale raw projections"
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should evict the live handle before resume");
        let restarted = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );
        let resume_source = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("resume source should load from runtime authority")
            .expect("runtime authority should remain present");
        assert_eq!(
            resume_source.messages().len(),
            live.messages().len(),
            "resume source must not be rebuilt from stale raw store fallback"
        );
        assert!(
            !resume_source.metadata().contains_key(SESSION_LABELS_KEY),
            "resume source must not inherit raw store metadata"
        );
        let resume_error = restarted
            .create_session(resume_request(resume_source))
            .await
            .expect_err(
                "resume must fail closed when projection refresh would shrink stale raw state",
            );
        assert!(
            matches!(resume_error, SessionError::Store(_)),
            "stale projection refresh failure should surface as a store error, got {resume_error:?}"
        );
        assert!(
            !restarted
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed after failed resume"),
            "failed projection refresh must discard the materialized live session"
        );
        let authoritative_after_failure = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should still succeed after failed resume")
            .expect("runtime authority should remain present after failed resume");
        assert_eq!(
            authoritative_after_failure.messages().len(),
            live.messages().len(),
            "failed projection refresh must not replace runtime-authoritative metadata"
        );
        let raw_after_failure = store
            .load(&result.session_id)
            .await
            .expect("raw projection load should still succeed")
            .expect("stale projection should remain present after failed resume");
        assert_eq!(
            raw_after_failure.messages().len(),
            live.messages().len() + 1,
            "failed projection refresh must not silently rewrite stale raw projection state"
        );
    }

    #[tokio::test]
    async fn test_authoritative_load_ignores_newer_raw_store_metadata_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        let runtime_authority = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("runtime authority should exist");

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mut raw_store_projection = runtime_authority.clone();
        raw_store_projection.set_metadata(
            SESSION_LABELS_KEY,
            serde_json::json!({
                "source": "raw-store",
            }),
        );
        store
            .save(&raw_store_projection)
            .await
            .expect("test should be able to write a stale raw projection");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should evict the live session so read/list use durable state");

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        assert_eq!(
            authoritative.updated_at(),
            runtime_authority.updated_at(),
            "newer raw metadata projections must not advance runtime-authoritative timestamps"
        );
        assert!(
            !authoritative.metadata().contains_key(SESSION_LABELS_KEY),
            "newer raw metadata projections must not be backfilled into runtime authority"
        );

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should use runtime authority");
        assert!(
            view.state.labels.is_empty(),
            "read() must not expose labels from a newer raw store projection"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should use runtime authority");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect(
                "runtime-backed session should still be listed via its compatibility projection",
            );
        assert!(
            summary.labels.is_empty(),
            "list() must not expose labels from a newer raw store projection"
        );
        assert_eq!(
            summary.updated_at,
            runtime_authority.updated_at(),
            "list() must report the runtime-authoritative timestamp"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_uses_runtime_authority_after_direct_turn() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .apply_runtime_turn(
                &result.session_id,
                RunId::new(),
                runtime_content_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");

        service
            .start_turn(&result.session_id, start_turn_request("direct follow-up"))
            .await
            .expect("direct turn should succeed");

        service
            .append_system_context(
                &result.session_id,
                AppendSystemContextRequest {
                    text: "prefer store".to_string(),
                    source: Some("api".to_string()),
                    idempotency_key: Some("store-base".to_string()),
                },
            )
            .await
            .expect("append_system_context should succeed");

        let stored = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("append should preserve the latest direct-service turn");
        assert_eq!(
            stored.messages().len(),
            4,
            "direct SessionService turns must update runtime authority before later control mutations"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_stage_tool_results_without_live_handle_updates_runtime_authority()
    {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store.clone()),
            memory_blob_store(),
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("test should be able to discard the live session");

        let staged = service
            .stage_tool_results(
                &result.session_id,
                StageToolResultsRequest {
                    results: vec![ToolResult::new(
                        "tool-call-1".to_string(),
                        "callback result".to_string(),
                        false,
                    )],
                },
            )
            .await
            .expect("stage_tool_results should update runtime authority");
        assert_eq!(staged.accepted_result_count, 1);

        let authoritative = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("authoritative session should exist");
        let deferred = authoritative
            .deferred_turn_state()
            .expect("runtime authority should carry deferred-turn state");
        assert_eq!(deferred.pending_tool_results.len(), 1);
        assert_eq!(deferred.pending_tool_results[0].results.len(), 1);

        let raw = store
            .load(&result.session_id)
            .await
            .expect("raw store load should succeed")
            .expect("session-store projection should be kept for listability");
        let raw_deferred = raw
            .deferred_turn_state()
            .expect("projection should mirror committed deferred-turn state");
        assert_eq!(raw_deferred.pending_tool_results.len(), 1);

        let view = service
            .read(&result.session_id)
            .await
            .expect("read should use durable runtime truth after live discard");
        assert_eq!(view.state.message_count, authoritative.messages().len());
        assert!(
            !service
                .has_live_session(&result.session_id)
                .await
                .expect("status should succeed"),
            "status should not recreate a live handle"
        );
        let listed = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        let summary = listed
            .iter()
            .find(|summary| summary.session_id == result.session_id)
            .expect("list should include persisted runtime-backed session");
        assert_eq!(summary.message_count, authoritative.messages().len());

        let restarted = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            Some(runtime_store),
            memory_blob_store(),
        );
        let resume_source = restarted
            .load_authoritative_session(&result.session_id)
            .await
            .expect("resume source should load from runtime authority")
            .expect("runtime authority should remain present after restart");
        let resumed = restarted
            .create_session(resume_request(resume_source))
            .await
            .expect("resume should materialize the runtime-authoritative snapshot");
        assert_eq!(resumed.session_id, result.session_id);
        let resumed_live = restarted
            .export_live_session(&result.session_id)
            .await
            .expect("resumed live session should export");
        let resumed_deferred = resumed_live
            .deferred_turn_state()
            .expect("resumed session should preserve staged tool results");
        assert_eq!(resumed_deferred.pending_tool_results.len(), 1);
    }

    #[tokio::test]
    async fn test_store_only_control_mutations_fail_closed_without_runtime_divergence() {
        for runtime_backed in [true, false] {
            let mode = if runtime_backed {
                "runtime-backed"
            } else {
                "runtime-less"
            };
            let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
            let runtime_store = runtime_backed.then(|| Arc::new(InMemoryRuntimeStore::new()));
            let service_runtime_store = runtime_store
                .as_ref()
                .map(|store| Arc::clone(store) as Arc<dyn RuntimeStore>);
            let service = PersistentSessionService::new(
                DummyBuilder,
                4,
                Arc::clone(&store),
                service_runtime_store,
                memory_blob_store(),
            );
            let session = Session::new();
            let id = session.id().clone();
            store
                .save(&session)
                .await
                .expect("test should seed a store-only compatibility projection");

            let append_err = service
                .append_system_context(
                    &id,
                    AppendSystemContextRequest {
                        text: format!("{mode} store-only append must fail closed"),
                        source: Some("api".to_string()),
                        idempotency_key: Some(format!("store-only-{mode}-append")),
                    },
                )
                .await
                .expect_err("store-only projection must not be promoted by append");
            assert!(
                matches!(
                    append_err,
                    SessionControlError::Session(SessionError::Unsupported(ref message))
                        if message.contains("store-only compatibility projection")
                            && message.contains("machine snapshot")
                ),
                "{mode} append error should require machine authority: {append_err:?}"
            );

            let stage_err = service
                .stage_tool_results(
                    &id,
                    StageToolResultsRequest {
                        results: vec![ToolResult::new(
                            "tool-call-1".to_string(),
                            "callback result".to_string(),
                            false,
                        )],
                    },
                )
                .await
                .expect_err("store-only projection must not be promoted by staged tool results");
            assert!(
                matches!(
                    stage_err,
                    SessionError::Unsupported(ref message)
                        if message.contains("store-only compatibility projection")
                            && message.contains("machine snapshot")
                ),
                "{mode} stage error should require machine authority: {stage_err:?}"
            );

            let archive_err = service
                .archive(&id)
                .await
                .expect_err("store-only projection must not own lifecycle archive");
            assert!(
                matches!(
                    archive_err,
                    SessionError::Unsupported(ref message)
                        if message.contains("store-only compatibility projection")
                            && message.contains("machine snapshot")
                ),
                "{mode} archive error should require machine authority: {archive_err:?}"
            );

            let raw = store
                .load(&id)
                .await
                .expect("raw store load should succeed")
                .expect("store-only projection should remain present");
            assert!(
                raw.system_context_state().is_none(),
                "{mode} append rejection must not mutate the store-only projection"
            );
            assert!(
                raw.deferred_turn_state().is_none(),
                "{mode} stage rejection must not mutate the store-only projection"
            );
            assert!(
                !metadata_marks_archived(raw.metadata()),
                "{mode} archive rejection must not mutate lifecycle metadata"
            );
            if let Some(runtime_store) = runtime_store {
                assert!(
                    runtime_store
                        .load_session_snapshot(
                            &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id)
                        )
                        .await
                        .expect("runtime snapshot load should succeed")
                        .is_none(),
                    "store-only control mutations must not create runtime authority"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_machine_authorized_archive_routes_store_only_projection() {
        for runtime_backed in [true, false] {
            let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
            let runtime_store = runtime_backed.then(|| Arc::new(InMemoryRuntimeStore::new()));
            let service_runtime_store = runtime_store
                .as_ref()
                .map(|store| Arc::clone(store) as Arc<dyn RuntimeStore>);
            let service = PersistentSessionService::new(
                DummyBuilder,
                4,
                Arc::clone(&store),
                service_runtime_store,
                memory_blob_store(),
            );
            let session = Session::new();
            let id = session.id().clone();
            store
                .save(&session)
                .await
                .expect("test should seed a store-only compatibility projection");

            let machine = meerkat_runtime::MeerkatMachine::ephemeral();
            service
                .archive_with_machine_authority(&id, machine.session_control_authority())
                .await
                .expect("machine-routed archive should own the store-only transition");

            let raw = store
                .load(&id)
                .await
                .expect("raw store load should succeed")
                .expect("archived projection should remain present");
            assert!(
                metadata_marks_archived(raw.metadata()),
                "machine-routed archive should persist archived lifecycle metadata"
            );
            assert!(
                raw.system_context_state().is_none(),
                "archive must not add control append state"
            );
            assert!(
                raw.deferred_turn_state().is_none(),
                "archive must not add deferred-turn control state"
            );
            if let Some(runtime_store) = runtime_store {
                let archived = runtime_store
                    .load_session_snapshot(
                        &PersistentSessionService::<DummyBuilder>::runtime_id_for_session(&id),
                    )
                    .await
                    .expect("runtime snapshot load should succeed")
                    .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok())
                    .expect("machine-routed archive should write runtime authority");
                assert!(
                    metadata_marks_archived(archived.metadata()),
                    "runtime-backed machine archive should persist archived runtime authority"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_apply_runtime_context_appends_rejects_missing_runtime_bindings() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut session = Session::new();
        let id = session.id().clone();
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: Some("realm-test".to_string()),
                instance_id: Some("instance-test".to_string()),
                backend: Some("sqlite".to_string()),
                config_generation: Some(7),
                connection_ref: None,
            })
            .expect("session metadata should serialize");
        store
            .save(&session)
            .await
            .expect("persisted session should save");

        let error = service
            .apply_runtime_context_appends(
                &id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "recover me".to_string(),
                    source: Some("system-generated:test".to_string()),
                    idempotency_key: None,
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect_err("runtime-backed recovery should reject missing bindings");

        assert!(
            error.to_string().contains("session not found"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_start_turn_recovery_rejects_missing_runtime_bindings() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create deferred session");
        let id = created.session_id;

        service
            .discard_live_session(&id)
            .await
            .expect("discard live session");

        let error = service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect_err("runtime-backed recovery should reject missing bindings");

        assert!(
            error
                .to_string()
                .contains("stored-session recovery via non-canonical runtime-binding providers has been deleted"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn test_metadata_only_projection_does_not_discard_live_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::Defer))
            .await
            .expect("create deferred session");
        let id = created.session_id;

        let mut projected = service
            .export_live_session(&id)
            .await
            .expect("live session should export");
        projected.set_metadata("projection_only", serde_json::json!(true));
        store
            .save(&projected)
            .await
            .expect("projection snapshot should save");

        let discarded = service
            .discard_stale_live_session_if_needed(&id)
            .await
            .expect("discard check should succeed");

        assert!(
            !discarded,
            "metadata/timestamp-only durable projection must not evict live runtime mechanics"
        );
        service
            .export_live_session(&id)
            .await
            .expect("live session should remain available");
    }

    #[tokio::test]
    async fn test_apply_runtime_context_appends_emits_run_lifecycle_events() {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;
        let mut events = service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");

        service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"token\":\"birch seventeen\"}.".to_string(),
                    source: Some("peer_response_terminal:analyst-rt:req-123".to_string()),
                    idempotency_key: Some("peer_response_terminal:analyst-rt:req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("apply_runtime_context_appends");

        let started = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_started timeout")
            .expect("run_started event should exist");
        match started.payload {
            AgentEvent::RunStarted { prompt, .. } => {
                let normalized = prompt.text_content().to_lowercase();
                assert!(
                    normalized.contains("peer_response_terminal:analyst-rt:req-123"),
                    "run_started prompt should expose runtime system-context source: {normalized}"
                );
                assert!(
                    normalized.contains("birch seventeen"),
                    "run_started prompt should expose authoritative terminal peer payload: {normalized}"
                );
            }
            other => panic!("expected run_started, got {other:?}"),
        }

        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_completed timeout")
            .expect("run_completed event should exist");
        match completed.payload {
            AgentEvent::RunCompleted { result, usage, .. } => {
                assert!(
                    result.is_empty(),
                    "context-only runtime apply should not synthesize assistant output: {result:?}"
                );
                assert_eq!(
                    usage,
                    meerkat_core::types::Usage::default(),
                    "context-only runtime apply should not report model usage"
                );
            }
            other => panic!("expected run_completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_event_store_projection_records_persistent_session_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "project this durable event".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("test".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("apply context append");

        event_store.wait_for_seq(&session_id, 2).await;
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 2);
        let events_path = dir
            .path()
            .join(".rkat")
            .join("sessions")
            .join(session_id.to_string())
            .join("events.jsonl");
        let projected = read_projected_events_after(&events_path, "run_completed").await;
        assert!(projected.contains("run_started"));
        assert!(projected.contains("run_completed"));
    }

    #[tokio::test]
    async fn test_event_replay_projection_reads_ordered_session_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );
        let session_id = SessionId::new();

        event_store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 0 },
                    AgentEvent::TextComplete {
                        content: "two".to_string(),
                    },
                ],
            )
            .await
            .expect("append event fixtures");

        assert_eq!(
            service
                .event_log_latest_seq(&session_id)
                .await
                .expect("latest seq"),
            Some(2)
        );
        let events = service
            .event_log_read_from(&session_id, 2)
            .await
            .expect("read event log")
            .expect("event projection enabled");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 2);
        assert!(matches!(events[0].event, AgentEvent::TextComplete { .. }));
    }

    #[tokio::test]
    async fn test_event_replay_projection_reports_unsupported_when_not_installed() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let session_id = SessionId::new();

        assert_eq!(
            service
                .event_log_latest_seq(&session_id)
                .await
                .expect("latest seq"),
            None
        );
        assert!(
            service
                .event_log_read_from(&session_id, 1)
                .await
                .expect("read event log")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_event_store_projection_records_eager_initial_turn_events() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let event_store = Arc::new(RecordingEventStore::default());
        let event_store_trait: Arc<dyn EventStore> = event_store.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let service = PersistentSessionService::new(
            EventfulBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        )
        .with_event_projection(
            event_store_trait,
            Arc::new(SessionProjector::new(dir.path().join(".rkat"))),
        );

        let run = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let session_id = run.session_id;

        event_store.wait_for_seq(&session_id, 2).await;
        assert_eq!(event_store.last_seq(&session_id).await.unwrap(), 2);
        let events_path = dir
            .path()
            .join(".rkat")
            .join("sessions")
            .join(session_id.to_string())
            .join("events.jsonl");
        let projected = read_projected_events_after(&events_path, "run_completed").await;
        assert!(projected.contains("run_started"));
        assert!(projected.contains("run_completed"));
    }

    /// Create a session request that seeds initial SessionMetadata so
    /// update_keep_alive has something to mutate (mirrors what the real factory does).
    fn create_request_with_metadata(
        prompt: &str,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        let mut session = Session::new();
        let metadata = meerkat_core::SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "test".to_string(),
            max_tokens: 1024,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            connection_ref: None,
        };
        session.set_session_metadata(metadata).unwrap();
        let mut req = create_request(prompt, initial_turn);
        req.build = Some(SessionBuildOptions {
            resume_session: Some(session),
            ..Default::default()
        });
        req
    }

    #[tokio::test]
    async fn test_update_keep_alive_persists_to_store() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        // Verify initial keep_alive is false (default).
        let persisted = store.load(&id).await.unwrap().unwrap();
        let meta = persisted.session_metadata().expect("metadata present");
        assert!(!meta.keep_alive, "initial keep_alive should be false");

        // Update keep_alive to true on the live session.
        service
            .apply_runtime_session_keep_alive(&id, true)
            .await
            .expect("runtime keep_alive update should succeed");

        // Verify the store reflects the update.
        let persisted = store.load(&id).await.unwrap().unwrap();
        let meta = persisted.session_metadata().expect("metadata present");
        assert!(
            meta.keep_alive,
            "persisted keep_alive should be true after update"
        );
    }

    #[tokio::test]
    async fn test_update_keep_alive_rolls_back_on_store_failure() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        // --- Transition: false → true with store failure ---
        fail_store.set_fail_save(true);
        let result = service.apply_runtime_session_keep_alive(&id, true).await;
        assert!(result.is_err(), "false→true should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            !exported.session_metadata().unwrap().keep_alive,
            "should roll back to false after failed false→true"
        );

        // --- Bring live state to true successfully ---
        service
            .apply_runtime_session_keep_alive(&id, true)
            .await
            .unwrap();
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(exported.session_metadata().unwrap().keep_alive);

        // --- Transition: true → true with store failure (idempotent retry) ---
        fail_store.set_fail_save(true);
        let result = service.apply_runtime_session_keep_alive(&id, true).await;
        assert!(result.is_err(), "true→true should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported.session_metadata().unwrap().keep_alive,
            "should stay true after failed true→true (not flip to false)"
        );

        // --- Transition: true → false with store failure ---
        fail_store.set_fail_save(true);
        let result = service.apply_runtime_session_keep_alive(&id, false).await;
        assert!(result.is_err(), "true→false should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported.session_metadata().unwrap().keep_alive,
            "should roll back to true after failed true→false (not flip to false)"
        );
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_persists_to_store() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        service
            .set_session_tool_filter(&id, filter.clone())
            .await
            .expect("set_session_tool_filter should succeed");

        let exported = service.export_session_with_labels(&id).await.unwrap();
        let exported_state = exported
            .tool_visibility_state()
            .expect("live visibility state should decode")
            .expect("live session should carry visibility state after staging");
        assert_eq!(exported_state.staged_filter, filter);
        assert_eq!(exported_state.active_filter, meerkat_core::ToolFilter::All);
        assert_eq!(exported_state.staged_revision, 1);
        assert_eq!(exported_state.active_revision, 0);

        let persisted = store.load(&id).await.unwrap().unwrap();
        let persisted_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode")
            .expect("persisted session should carry visibility state after staging");
        assert_eq!(persisted_state, exported_state);
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_rolls_back_on_store_failure() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create session");
        let id = created.session_id;

        let baseline = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            baseline
                .tool_visibility_state()
                .expect("baseline visibility state should decode")
                .is_none(),
            "new sessions should not materialize visibility metadata before updates"
        );

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        fail_store.set_fail_save(true);
        let result = service.set_session_tool_filter(&id, filter).await;
        assert!(
            result.is_err(),
            "store failure should abort the filter update"
        );
        fail_store.set_fail_save(false);

        let exported = service.export_session_with_labels(&id).await.unwrap();
        let exported_visibility_state = exported
            .tool_visibility_state()
            .expect("exported visibility state should decode");
        let baseline_visibility_state = baseline
            .tool_visibility_state()
            .expect("baseline visibility state should decode");
        assert_eq!(
            exported_visibility_state, baseline_visibility_state,
            "live session should roll back to the pre-mutation visibility state"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        let persisted_visibility_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode");
        assert_eq!(
            persisted_visibility_state, baseline_visibility_state,
            "store should retain the pre-mutation visibility state after rollback"
        );
    }

    #[test]
    fn rollback_snapshot_ignores_legacy_filter_metadata_when_canonical_state_is_absent() {
        let mut session = Session::new();
        session.set_metadata(
            meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );
        session.set_metadata(
            meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Allow(
                ["visible".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let snapshot = rollback_tool_visibility_state_snapshot(&session)
            .expect("legacy-only metadata should not fail canonical visibility parsing");

        assert_eq!(
            snapshot, None,
            "legacy-only metadata must not become rollback visibility authority"
        );
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_rollback_does_not_promote_legacy_metadata() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let mut request = create_request_with_metadata("hello", InitialTurnPolicy::RunImmediately);
        let session = request
            .build
            .as_mut()
            .and_then(|build| build.resume_session.as_mut())
            .expect("request should carry a resumable session");
        session.set_metadata(
            meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );
        session.set_metadata(
            meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(meerkat_core::ToolFilter::Allow(
                ["visible".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let created = service
            .create_session(request)
            .await
            .expect("create session");
        let id = created.session_id;

        let baseline = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            baseline
                .tool_visibility_state()
                .expect("canonical visibility metadata should parse")
                .is_none(),
            "legacy-only metadata must not materialize canonical visibility state"
        );
        assert!(
            baseline
                .metadata()
                .contains_key(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY),
            "fixture should retain the stale external filter metadata key"
        );
        assert!(
            baseline
                .metadata()
                .contains_key(meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY),
            "fixture should retain the stale inherited filter metadata key"
        );

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        fail_store.set_fail_save(true);
        let result = service.set_session_tool_filter(&id, filter).await;
        assert!(
            result.is_err(),
            "store failure should abort the filter update"
        );
        fail_store.set_fail_save(false);

        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported
                .tool_visibility_state()
                .expect("canonical visibility metadata should parse")
                .is_none(),
            "failed rollback must not promote stale legacy metadata into canonical visibility"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        assert!(
            persisted
                .tool_visibility_state()
                .expect("canonical visibility metadata should parse")
                .is_none(),
            "store should retain no canonical visibility state after failed rollback"
        );
    }

    #[tokio::test]
    async fn test_set_session_tool_filter_rollback_rejects_malformed_canonical_visibility_state() {
        let fail_store = Arc::new(FailSaveStore::new());
        let store: Arc<dyn SessionStore> = Arc::clone(&fail_store) as Arc<dyn SessionStore>;
        let service =
            PersistentSessionService::new(DummyBuilder, 4, store, None, memory_blob_store());

        let mut request = create_request_with_metadata("hello", InitialTurnPolicy::RunImmediately);
        let malformed_visibility_state = serde_json::json!("not-a-visibility-state");
        let session = request
            .build
            .as_mut()
            .and_then(|build| build.resume_session.as_mut())
            .expect("request should carry a resumable session");
        session.set_metadata(
            meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY,
            malformed_visibility_state.clone(),
        );

        let created = service
            .create_session(request)
            .await
            .expect("create session");
        let id = created.session_id;

        let baseline = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            baseline.try_tool_visibility_state().is_err(),
            "fixture should carry malformed canonical visibility metadata"
        );
        assert_eq!(
            baseline
                .metadata()
                .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            Some(&malformed_visibility_state),
            "fixture should retain the raw malformed canonical metadata"
        );

        let filter =
            meerkat_core::ToolFilter::Deny(["view_image".to_string()].into_iter().collect());

        fail_store.set_fail_save(true);
        let result = service.set_session_tool_filter(&id, filter).await;
        let err = result
            .expect_err("malformed canonical visibility should fail before staging or rollback");
        assert!(
            err.to_string()
                .contains("invalid canonical tool visibility state"),
            "unexpected error: {err}"
        );
        fail_store.set_fail_save(false);

        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert_eq!(
            exported
                .metadata()
                .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            Some(&malformed_visibility_state),
            "failed mutation must preserve malformed canonical visibility metadata"
        );
        assert!(
            exported.try_tool_visibility_state().is_err(),
            "failed mutation must not replace malformed canonical visibility with default state"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        assert_eq!(
            persisted
                .metadata()
                .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            Some(&malformed_visibility_state),
            "store should retain the raw malformed canonical metadata after failed mutation"
        );
    }

    #[tokio::test]
    async fn test_deferred_first_turn_system_prompt_is_applied_and_persisted() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create deferred session");
        let id = created.session_id;

        service
            .start_turn(
                &id,
                start_turn_request_with_system_prompt(
                    "first turn",
                    Some("You are a deferred-session reviewer."),
                ),
            )
            .await
            .expect("deferred first turn should succeed");

        let restored = service
            .export_live_session(&id)
            .await
            .expect("live session");
        let system_prompt = restored
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("restored session should contain a system prompt");
        assert!(system_prompt.contains("deferred-session reviewer"));

        let persisted = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("session should be persisted");
        let persisted_prompt = persisted
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("persisted session should contain a system prompt");
        assert!(persisted_prompt.contains("deferred-session reviewer"));
    }

    #[tokio::test]
    async fn test_deferred_first_turn_system_prompt_overrides_create_time_prompt() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut req = create_request_with_metadata("hello", InitialTurnPolicy::Defer);
        req.system_prompt = Some("You are the old prompt.".to_string());
        let created = service
            .create_session(req)
            .await
            .expect("create deferred session");
        let id = created.session_id;

        service
            .start_turn(
                &id,
                start_turn_request_with_system_prompt(
                    "first turn",
                    Some("You are the new prompt."),
                ),
            )
            .await
            .expect("deferred first turn should succeed");

        let restored = service
            .export_live_session(&id)
            .await
            .expect("live session");
        let system_prompt = restored
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("restored session should contain a system prompt");
        assert!(system_prompt.contains("new prompt"));
        assert!(!system_prompt.contains("old prompt"));
    }

    #[tokio::test]
    async fn test_materialized_start_turn_rejects_system_prompt_override() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create immediate session");
        let id = created.session_id;

        let error = service
            .start_turn(
                &id,
                start_turn_request_with_system_prompt(
                    "follow-up",
                    Some("You are a different prompt."),
                ),
            )
            .await
            .expect_err("materialized session should reject turn-time system_prompt");

        match error {
            SessionError::Unsupported(message) => {
                assert!(message.contains("deferred session's first turn"));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_external_tool_call_persists_live_session_changes() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(
            ToolDispatchBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let id = created.session_id;

        let outcome = service
            .dispatch_external_tool_call(
                &id,
                ToolCall::new(
                    "call-1".to_string(),
                    "callback_probe_tool".to_string(),
                    serde_json::json!({}),
                ),
            )
            .await
            .expect("dispatch external tool call");

        assert_eq!(outcome.result.text_content(), "handled callback_probe_tool");

        let persisted = store
            .load(&id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let visibility_state = persisted
            .tool_visibility_state()
            .expect("persisted visibility state should decode")
            .expect("persistent dispatch should save session mutations");
        assert!(
            visibility_state
                .staged_requested_deferred_names
                .contains("requested:callback_probe_tool"),
            "expected persisted tool-dispatch state to include the requested tool"
        );
    }

    #[tokio::test]
    async fn test_dispatch_external_tool_call_discards_live_session_when_persist_fails() {
        let store = Arc::new(FailSaveStore::new());
        let service = PersistentSessionService::new(
            ToolDispatchBuilder,
            4,
            store.clone(),
            None,
            memory_blob_store(),
        );

        let created = service
            .create_session(create_request("hello", InitialTurnPolicy::Defer))
            .await
            .expect("create session");
        let id = created.session_id;

        store.set_fail_save(true);
        let error = service
            .dispatch_external_tool_call(
                &id,
                ToolCall::new(
                    "call-2".to_string(),
                    "callback_probe_tool".to_string(),
                    serde_json::json!({}),
                ),
            )
            .await
            .expect_err("persist failure should bubble out");

        assert!(
            matches!(error, SessionError::Store(_)),
            "expected store error after persistence failure, got {error:?}"
        );
        let live = service.export_live_session(&id).await;
        assert!(
            matches!(live, Err(SessionError::NotFound { .. })),
            "expected live session to be discarded after persist failure, got {live:?}"
        );
    }
}

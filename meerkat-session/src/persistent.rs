//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.
//!
//! After each turn completes, the session snapshot is saved to the `SessionStore`
//! and events are appended to the `EventStore`. On `read` and `list`, persisted
//! sessions are merged with live (ephemeral) sessions.

use async_trait::async_trait;
use indexmap::{IndexMap, IndexSet};
use meerkat_core::PendingSystemContextAppend;
#[allow(unused_imports)] // Used in read() fallback path
use meerkat_core::Session;
use meerkat_core::SessionSystemContextState;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunApplyBoundary,
};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionBuildOptions, SessionControlError, SessionError, SessionHistoryPage,
    SessionHistoryQuery, SessionInfo, SessionQuery, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionUsage, SessionView,
    StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::{InputId, RunId};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::{
    InputLifecycleState, InputState, InputStateHistoryEntry, InputTerminalOutcome,
};
use meerkat_runtime::store::SessionDelta;
use meerkat_runtime::{RuntimeMode, RuntimeStore};
use meerkat_store::SessionStore;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::SESSION_LABELS_KEY;
use crate::ephemeral::{EphemeralSessionService, SessionAgentBuilder};

const SESSION_ARCHIVED_KEY: &str = "session_archived";

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

/// Shared gate between the checkpointer and archive.
///
/// The `Mutex` provides mutual exclusion so that `checkpoint()` cannot
/// race with `archive()`: both acquire the lock before touching the store,
/// and `archive()` sets `cancelled = true` under the lock before deleting.
struct CheckpointerGate {
    cancelled: Mutex<bool>,
}

/// Checkpointer that saves sessions to a [`SessionStore`].
///
/// Used by host-mode agents to persist the session after each interaction
/// without going through `SessionService::start_turn()`.
///
/// Tracks the message count from the last successful save so that
/// back-to-back checkpoints of an unchanged session are skipped.
/// This avoids redundant writes — particularly the first checkpoint
/// after `create_session` (which already calls `persist_full_session`).
struct StoreCheckpointer {
    store: Arc<dyn SessionStore>,
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
        if let Err(e) = self.store.save(session).await {
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
    /// Process-local bounded cache of archived full sessions to avoid
    /// immediately reloading durable archived snapshots on hot history reads.
    archived_sessions: Mutex<IndexMap<SessionId, Session>>,
    archived_history_capacity: usize,
    /// Gates for active host-mode checkpointers, keyed by session ID.
    /// Archive acquires the gate's lock, sets cancelled, then saves the
    /// archived snapshot — mutual exclusion prevents a concurrent checkpoint
    /// from overwriting the archived row with a live one.
    checkpointer_gates: Mutex<HashMap<SessionId, Arc<CheckpointerGate>>>,
}

/// Extract session labels from a metadata map.
///
/// Looks for `SESSION_LABELS_KEY` and deserializes the value as
/// `BTreeMap<String, String>`. Returns an empty map on missing or
/// malformed data.
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

fn metadata_marks_archived(metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
    metadata
        .get(SESSION_ARCHIVED_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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
        LogicalRuntimeId::new(id.to_string())
    }

    async fn runtime_input_updates(
        &self,
        id: &SessionId,
        run_id: &RunId,
        sequence: u64,
        contributing_input_ids: &[InputId],
    ) -> Result<Vec<InputState>, SessionError> {
        let Some(runtime_store) = self.runtime_store.as_ref() else {
            return Ok(Vec::new());
        };
        let runtime_id = Self::runtime_id_for_session(id);
        let stored_states = runtime_store
            .load_input_states(&runtime_id)
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to load runtime input states: {err}"
                )))
            })?;

        Ok(contributing_input_ids
            .iter()
            .filter_map(|input_id| {
                let mut state = stored_states
                    .iter()
                    .find(|candidate| &candidate.input_id == input_id)?
                    .clone();
                let previous = state.current_state;
                state.last_run_id = Some(run_id.clone());
                state.last_boundary_sequence = Some(sequence);
                state.current_state = InputLifecycleState::Consumed;
                state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                state.history.push(InputStateHistoryEntry {
                    timestamp: state.updated_at,
                    from: previous,
                    to: InputLifecycleState::Consumed,
                    reason: Some("runtime boundary applied and durably committed".into()),
                });
                Some(state)
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
            self.store
                .save(session)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?;
            return Self::build_runtime_receipt(
                run_id,
                boundary,
                contributing_input_ids.to_vec(),
                session,
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

    async fn load_authoritative_session_base(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        let store_snapshot = self
            .store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;

        let runtime_snapshot = if let Some(runtime_store) = self.runtime_store.as_ref() {
            let runtime_id = Self::runtime_id_for_session(id);
            runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .map_err(|err| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to load runtime session snapshot: {err}"
                    )))
                })?
                .map(|bytes| {
                    serde_json::from_slice::<Session>(&bytes).map_err(|err| {
                        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                            format!("failed to deserialize runtime session snapshot: {err}"),
                        ))
                    })
                })
                .transpose()?
        } else {
            None
        };

        Ok(match (store_snapshot, runtime_snapshot) {
            (Some(store_session), Some(runtime_session)) => {
                if runtime_session.updated_at() >= store_session.updated_at() {
                    Some(runtime_session)
                } else {
                    Some(store_session)
                }
            }
            (Some(store_session), None) => Some(store_session),
            (None, Some(runtime_session)) => Some(runtime_session),
            (None, None) => None,
        })
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

        let Some(stored) = self
            .store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
        else {
            return Ok(false);
        };

        let stored_is_newer = stored.updated_at() > live.updated_at()
            || (stored.updated_at() == live.updated_at()
                && stored.messages().len() > live.messages().len());
        let stored_is_archived = metadata_marks_archived(stored.metadata());

        if !stored_is_newer && !stored_is_archived {
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

    pub async fn export_live_session(&self, id: &SessionId) -> Result<Session, SessionError> {
        self.export_session_with_labels(id).await
    }

    pub async fn discard_live_session(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(id).await?;
        self.checkpointer_gates.lock().await.remove(id);
        Ok(())
    }

    /// Create a new persistent session service.
    pub fn new(
        builder: B,
        max_sessions: usize,
        store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
    ) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, max_sessions),
            store,
            runtime_store,
            archived_sessions: Mutex::new(IndexMap::new()),
            archived_history_capacity: max_sessions.max(1),
            checkpointer_gates: Mutex::new(HashMap::new()),
        }
    }

    pub fn runtime_mode(&self) -> RuntimeMode {
        RuntimeMode::V9Compliant
    }

    pub fn runtime_store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.runtime_store.clone()
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

    /// Apply a runtime-driven turn and return the authoritative boundary receipt.
    ///
    /// In runtime-backed mode, the returned serialized session snapshot is meant
    /// to be committed by `RuntimeStore::atomic_apply`, making the runtime store
    /// the sole durable writer for that turn.
    pub async fn apply_runtime_turn(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let (_, output) = self
            .apply_runtime_turn_with_result(id, run_id, req, boundary, contributing_input_ids)
            .await?;
        Ok(output)
    }

    pub async fn apply_runtime_turn_with_result(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<(RunResult, CoreApplyOutput), SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        let run_result = self.inner.start_turn(id, req).await?;

        let session = self.export_session_with_labels(id).await?;
        let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;

        let receipt = self
            .commit_runtime_apply(
                id,
                run_id,
                boundary,
                &session,
                &session_snapshot,
                &contributing_input_ids,
            )
            .await?;

        Ok((
            run_result.clone(),
            CoreApplyOutput {
                receipt,
                session_snapshot: Some(session_snapshot),
                run_result: Some(run_result),
            },
        ))
    }

    pub async fn apply_runtime_context_appends(
        &self,
        id: &SessionId,
        run_id: RunId,
        context_appends: Vec<ConversationContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let appends: Vec<PendingSystemContextAppend> = context_appends
            .into_iter()
            .filter_map(|append| match append.content {
                CoreRenderable::Text { text } => Some(PendingSystemContextAppend {
                    text,
                    source: Some(append.key),
                    idempotency_key: None,
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }),
                _ => None,
            })
            .collect();

        if let Err(SessionError::NotFound { .. }) = self
            .inner
            .apply_runtime_system_context(id, appends.clone())
            .await
        {
            let stored = self
                .load_persisted(id)
                .await?
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            let stored_metadata = stored.session_metadata();
            let tooling = stored_metadata
                .as_ref()
                .map(|meta| meta.tooling.clone())
                .unwrap_or_default();
            let build = SessionBuildOptions {
                provider: stored_metadata.as_ref().map(|meta| meta.provider),
                comms_name: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.comms_name.clone()),
                peer_meta: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.peer_meta.clone()),
                resume_session: Some(stored),
                override_builtins: Some(tooling.builtins),
                override_shell: Some(tooling.shell),
                override_memory: Some(tooling.memory),
                override_mob: Some(tooling.mob),
                realm_id: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.realm_id.clone()),
                instance_id: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.instance_id.clone()),
                backend: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.backend.clone()),
                config_generation: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.config_generation),
                ..SessionBuildOptions::default()
            };

            self.create_session(CreateSessionRequest {
                model: stored_metadata
                    .as_ref()
                    .map(|meta| meta.model.clone())
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() })?,
                prompt: String::new().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(
                    stored_metadata
                        .as_ref()
                        .map(|meta| meta.max_tokens)
                        .unwrap_or_default(),
                ),
                event_tx: None,
                host_mode: stored_metadata.as_ref().is_some_and(|meta| meta.host_mode),
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(build),
                labels: None,
            })
            .await?;

            self.inner
                .apply_runtime_system_context(id, appends.clone())
                .await?;
        }

        let session = self.export_session_with_labels(id).await?;
        let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;

        let receipt = self
            .commit_runtime_apply(
                id,
                run_id,
                RunApplyBoundary::Immediate,
                &session,
                &session_snapshot,
                &contributing_input_ids,
            )
            .await?;

        Ok(CoreApplyOutput {
            receipt,
            session_snapshot: Some(session_snapshot),
            run_result: None,
        })
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(
        &self,
        mut req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        // Inject a checkpointer for all sessions — the agent only calls it
        // inside the host-mode loop, so non-host sessions pay zero cost.
        // This must be unconditional because mob agents create sessions with
        // host_mode=false and start the host loop explicitly later.
        let gate = Arc::new(CheckpointerGate {
            cancelled: Mutex::new(false),
        });
        let checkpointer = Arc::new(StoreCheckpointer {
            store: Arc::clone(&self.store),
            gate: Arc::clone(&gate),
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
            enabled: self.runtime_store.is_none(),
        });
        let build = req.build.get_or_insert_with(Default::default);
        build.checkpointer = Some(checkpointer.clone());

        let result = self.inner.create_session(req).await?;

        // Track the gate so archive() can cancel checkpoint writes.
        {
            self.checkpointer_gates
                .lock()
                .await
                .insert(result.session_id.clone(), gate);
        }

        // Persist the full session snapshot (messages + metadata) after first
        // turn and seed the checkpointer so the next host-mode checkpoint is
        // skipped if the session hasn't changed since this save.
        let saved_len = self.persist_full_session(&result.session_id).await?;
        checkpointer
            .last_saved_len
            .store(saved_len, std::sync::atomic::Ordering::Release);

        Ok(result)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        let result = self.inner.start_turn(id, req).await?;

        // Always persist after a direct start_turn call. Runtime-backed sessions
        // that go through apply_runtime_turn_with_result() have their own atomic
        // boundary commit path and don't call start_turn on PersistentSessionService.
        let _ = self.persist_full_session(id).await?;

        Ok(result)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        self.inner.set_session_client(id, client).await
    }

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        self.inner.set_session_tool_filter(id, filter).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        // Try live session first
        match self.inner.read(id).await {
            Ok(view) => Ok(view),
            Err(SessionError::NotFound { .. }) => {
                // Fall back to persisted session
                let session = self
                    .store
                    .load(id)
                    .await
                    .map_err(|e| SessionError::Store(Box::new(e)))?
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

                let labels = extract_labels_from_metadata(session.metadata());
                Ok(SessionView {
                    state: SessionInfo {
                        session_id: session.id().clone(),
                        created_at: session.created_at(),
                        updated_at: session.updated_at(),
                        message_count: session.messages().len(),
                        is_active: false,
                        last_assistant_text: session.last_assistant_text(),
                        labels,
                    },
                    billing: SessionUsage {
                        total_tokens: session.total_tokens(),
                        usage: session.total_usage(),
                    },
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        // Get live sessions
        let mut summaries = self.inner.list(SessionQuery::default()).await?;
        let live_ids: IndexSet<_> = summaries.iter().map(|s| s.session_id.clone()).collect();

        // Merge persisted sessions not currently live
        let stored = self
            .store
            .list(meerkat_store::SessionFilter::default())
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;

        for meta in stored {
            if !live_ids.contains(&meta.id) && !metadata_marks_archived(&meta.metadata) {
                let labels = extract_labels_from_metadata(&meta.metadata);
                summaries.push(SessionSummary {
                    session_id: meta.id,
                    created_at: meta.created_at,
                    updated_at: meta.updated_at,
                    message_count: meta.message_count,
                    total_tokens: meta.total_tokens,
                    is_active: false,
                    labels,
                });
            }
        }

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

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let archived_snapshot = match self.export_session_with_labels(id).await {
            Ok(session) => Some(session),
            Err(SessionError::NotFound { .. }) => self.load_authoritative_session_base(id).await?,
            Err(err) => return Err(err),
        };

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

        let live_result = self.inner.archive(id).await;

        let in_store = self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        if let Some(mut session) = archived_snapshot.clone() {
            session.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
            self.store
                .save(&session)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?;
            self.remember_archived_session(session).await;
        }

        // Gate guard is dropped here — any in-flight checkpoint that was
        // blocked on the lock will now see cancelled == true and bail out.
        drop(gate_guard.take());
        self.checkpointer_gates.lock().await.remove(id);

        match (&live_result, in_store) {
            // At least one side had the session — success.
            (Ok(()), _) | (_, true) => Ok(()),
            // Neither side had it — propagate NotFound from the live service.
            _ => live_result,
        }
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
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
            }

            let accepted_at = meerkat_core::time_compat::SystemTime::now();
            let mut attempts = 0usize;
            loop {
                attempts += 1;
                let (status, snapshot_state, persisted_state) = {
                    let guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "system-context state lock poisoned while snapshotting live append"
                            );
                            poisoned.into_inner()
                        }
                    };
                    let snapshot_state = guard.clone();
                    let mut candidate = snapshot_state.clone();
                    let status = candidate
                        .stage_append(&req, accepted_at)
                        .map_err(|err| err.into_control_error(id))?;
                    (status, snapshot_state, candidate)
                };

                // Persist the durable control state before mutating the live
                // runtime state so an error never leaves the caller observing a
                // failure while the next LLM boundary still applies the append.
                let mut session = if self.runtime_store.is_some() {
                    match self.load_authoritative_session_base(id).await? {
                        Some(session) => session,
                        None => {
                            if created_gate {
                                drop(gate_guard);
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
                                drop(gate_guard);
                                self.checkpointer_gates.lock().await.remove(id);
                            }
                            return Err(SessionControlError::Session(err));
                        }
                    }
                };

                self.reject_if_archived_session(id, &session).await?;
                write_system_context_state(&mut session, persisted_state)?;
                self.store
                    .save(&session)
                    .await
                    .map_err(|e| SessionControlError::Session(SessionError::Store(Box::new(e))))?;

                let commit_result = {
                    let mut guard = match state_arc.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            tracing::warn!(
                                session_id = %id,
                                "system-context state lock poisoned while committing live append"
                            );
                            poisoned.into_inner()
                        }
                    };
                    if *guard == snapshot_state {
                        let live_status = guard
                            .stage_append(&req, accepted_at)
                            .map_err(|err| err.into_control_error(id))?;
                        Some(live_status)
                    } else {
                        None
                    }
                };

                if let Some(live_status) = commit_result {
                    debug_assert_eq!(live_status, status);
                    drop(gate_guard);
                    return Ok(AppendSystemContextResult { status });
                }

                if attempts >= 8 {
                    tracing::warn!(
                        session_id = %id,
                        "system-context state kept changing after the durable append committed; discarding the live session so the next access reloads the authoritative stored state"
                    );
                    drop(gate_guard);
                    let _ = self.discard_live_session(id).await;
                    return Ok(AppendSystemContextResult { status });
                }
            }
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
            .store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
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
        self.store
            .save(&session)
            .await
            .map_err(|e| SessionControlError::Session(SessionError::Store(Box::new(e))))?;
        Ok(AppendSystemContextResult { status })
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

    /// Load a full session from the persistent store.
    ///
    /// Used by surfaces to resume sessions that aren't currently live.
    /// Returns the complete `Session` including message history.
    pub async fn load_persisted(&self, id: &SessionId) -> Result<Option<Session>, SessionError> {
        self.store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))
    }

    /// Export the full session from the live task and persist it to the store.
    ///
    /// Returns the saved message count so callers can seed a checkpointer's
    /// `last_saved_len` without a second export round-trip.
    async fn persist_full_session(&self, id: &SessionId) -> Result<usize, SessionError> {
        let session = self.export_session_with_labels(id).await?;
        let message_count = session.messages().len();

        self.store
            .save(&session)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;

        Ok(message_count)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ephemeral::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
    use meerkat_core::checkpoint::SessionCheckpointer;
    use meerkat_core::service::{InitialTurnPolicy, SessionService, SessionServiceControlExt};
    use meerkat_runtime::InMemoryRuntimeStore;
    use meerkat_store::MemoryStore;
    use meerkat_store::StoreError;
    use std::sync::atomic::{AtomicBool, Ordering};

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
        async fn save(&self, session: &Session) -> Result<(), StoreError> {
            if self.fail_save.load(Ordering::Acquire) {
                return Err(StoreError::Internal("forced save failure".to_string()));
            }
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
            self.inner.load(id).await
        }

        async fn list(
            &self,
            filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, StoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
            self.inner.delete(id).await
        }
    }

    struct DummyAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    }

    #[async_trait::async_trait]
    impl SessionAgent for DummyAgent {
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
                },
            ));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id,
                usage: meerkat_core::types::Usage::default(),
                turns: 1,
                tool_calls: 0,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn run_host_mode(
            &mut self,
            prompt: meerkat_core::types::ContentInput,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            self.run_with_events(prompt, tokio::sync::mpsc::channel(1).0)
                .await
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

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
            })
        }
    }

    fn create_request(prompt: &str, initial_turn: InitialTurnPolicy) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "test".to_string(),
            prompt: prompt.to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            initial_turn,
            build: None,
            labels: None,
        }
    }

    fn start_turn_request(prompt: &str) -> StartTurnRequest {
        StartTurnRequest {
            prompt: prompt.to_string().into(),
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            flow_tool_overlay: None,
            additional_instructions: None,
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
    async fn test_runtime_backed_create_session_installs_disabled_store_checkpointer() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let builder = CapturingCheckpointerBuilder::new();
        let captured = Arc::clone(&builder.captured);
        let service =
            PersistentSessionService::new(builder, 4, Arc::clone(&store), Some(runtime_store));

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
        let original = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("create_session should persist an initial snapshot");

        let mut mutated = original.clone();
        mutated.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "checkpoint should not bypass runtime boundary".to_string(),
            ),
        ));
        checkpointer.checkpoint(&mutated).await;

        let stored = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("session row should still exist");
        assert_eq!(
            stored.messages().len(),
            original.messages().len(),
            "runtime-backed sessions must not checkpoint directly into the session store before the runtime boundary commits"
        );
    }

    #[tokio::test]
    async fn test_persistent_archive_store_only_session_succeeds() {
        // After restart, sessions exist only in the persistent store —
        // not in the live (inner) ephemeral service. archive() must still
        // succeed by deleting from the store even when inner returns NotFound.
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify the session exists in the store
        assert!(store.load(&id).await.unwrap().is_some());

        // Simulate the archive path: inner.archive() would return NotFound,
        // but store.delete() should still succeed.
        store.delete(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_append_system_context_does_not_mutate_archived_store_row() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), None);
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();
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
        let service = PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), None);

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
        let service = PersistentSessionService::new(DummyBuilder, 1, Arc::clone(&store), None);

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

        let restarted = PersistentSessionService::new(DummyBuilder, 1, Arc::clone(&store), None);
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
    async fn test_append_system_context_repersist_live_session_when_store_row_missing() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), None);

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
    async fn test_append_system_context_unknown_session_does_not_allocate_gate() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), None);
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
    async fn test_runtime_backed_create_session_persists_initial_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service =
            PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), Some(runtime_store));

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");

        let stored = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("runtime-backed create_session should still persist a session snapshot");
        assert!(
            stored.messages().len() >= 2,
            "initial turn should be durably persisted for direct SessionService callers"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_start_turn_persists_follow_up_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service =
            PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), Some(runtime_store));

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("create_session should succeed");
        let id = result.session_id;
        let initial_count = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("create_session should persist snapshot")
            .messages()
            .len();

        service
            .start_turn(&id, start_turn_request("follow up"))
            .await
            .expect("start_turn should succeed");

        let stored = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("runtime-backed start_turn should update persisted snapshot");
        assert!(
            stored.messages().len() > initial_count,
            "follow-up turn should be durably saved for direct SessionService callers"
        );
    }

    #[tokio::test]
    async fn test_runtime_backed_append_system_context_stages_successfully() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service =
            PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), Some(runtime_store));

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

        let stored = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
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
        let service =
            PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), Some(runtime_store));

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

        let stored = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("session row should still exist");
        assert!(
            stored.messages().is_empty(),
            "runtime-backed append_system_context must not snapshot uncommitted live session messages into the durable row"
        );
        let state = stored
            .system_context_state()
            .expect("runtime-backed append should persist pending control state");
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].text, "durable pending context");
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
        );

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .apply_runtime_turn_with_result(
                &result.session_id,
                RunId::new(),
                start_turn_request("runtime committed turn"),
                RunApplyBoundary::Immediate,
                vec![],
            )
            .await
            .expect("runtime-backed turn should succeed");

        let stale_store_row = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("session row should still exist");
        assert!(
            stale_store_row.messages().is_empty(),
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

        let stored = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("append should persist a refreshed session row");
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
    async fn test_runtime_backed_append_system_context_prefers_newer_store_snapshot() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store = Arc::new(InMemoryRuntimeStore::new());
        let service =
            PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), Some(runtime_store));

        let result = service
            .create_session(create_request(
                "hello",
                meerkat_core::service::InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create_session should succeed");

        service
            .apply_runtime_turn_with_result(
                &result.session_id,
                RunId::new(),
                start_turn_request("runtime committed turn"),
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

        let stored = store
            .load(&result.session_id)
            .await
            .expect("load should succeed")
            .expect("append should preserve the latest direct-service turn");
        assert_eq!(
            stored.messages().len(),
            4,
            "append must not rewind newer direct SessionService turns behind an older runtime snapshot"
        );
    }

    #[tokio::test]
    async fn test_apply_runtime_context_appends_recovers_stored_only_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = PersistentSessionService::new(DummyBuilder, 4, Arc::clone(&store), None);

        let mut session = Session::new();
        let id = session.id().clone();
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                model: "test-model".to_string(),
                max_tokens: 1024,
                provider: meerkat_core::Provider::Anthropic,
                tooling: meerkat_core::SessionTooling {
                    builtins: true,
                    shell: false,
                    comms: false,
                    mob: false,
                    memory: true,
                    active_skills: None,
                },
                host_mode: false,
                comms_name: None,
                peer_meta: None,
                realm_id: Some("realm-test".to_string()),
                instance_id: Some("instance-test".to_string()),
                backend: Some("redb".to_string()),
                config_generation: Some(7),
            })
            .expect("session metadata should serialize");
        store
            .save(&session)
            .await
            .expect("persisted session should save");

        let output = service
            .apply_runtime_context_appends(
                &id,
                RunId::new(),
                vec![ConversationContextAppend {
                    key: "system-generated:test".to_string(),
                    content: CoreRenderable::Text {
                        text: "recover me".to_string(),
                    },
                }],
                vec![InputId::new()],
            )
            .await
            .expect("stored-only runtime append should recover the live session");

        assert_eq!(output.receipt.contributing_input_ids.len(), 1);

        let restored = service
            .export_live_session(&id)
            .await
            .expect("runtime append should recreate a live session");
        let system_prompt = restored
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("restored session should contain a system prompt");
        assert!(system_prompt.contains("recover me"));

        let persisted = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("runtime append should repersist the session");
        let persisted_prompt = persisted
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("persisted session should contain a system prompt");
        assert!(persisted_prompt.contains("recover me"));
    }
}

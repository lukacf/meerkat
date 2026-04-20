//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.
//!
//! After each turn completes, the session snapshot is saved to the `SessionStore`
//! and events are appended to the `EventStore`. On `read` and `list`, persisted
//! sessions are merged with live (ephemeral) sessions.

use async_trait::async_trait;
use indexmap::{IndexMap, IndexSet};
use meerkat_core::BlobStore;
use meerkat_core::PendingSystemContextAppend;
#[allow(unused_imports)] // Used in read() fallback path
use meerkat_core::Session;
use meerkat_core::SessionDeferredTurnState;
use meerkat_core::SessionSystemContextState;
use meerkat_core::error::AgentError;
use meerkat_core::image_content::{externalize_deferred_turn_state, externalize_messages_from};
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
use meerkat_core::{InputId, RunId};
use meerkat_core::{
    SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides, build_recovered_session,
};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};
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
) -> Option<meerkat_core::SessionToolVisibilityState> {
    if let Some(state) = session.tool_visibility_state() {
        return Some(state);
    }

    let mut state = meerkat_core::SessionToolVisibilityState::default();
    let mut changed = false;

    if let Some(raw_filter) = session
        .metadata()
        .get(meerkat_core::EXTERNAL_TOOL_FILTER_METADATA_KEY)
        .cloned()
        && let Ok(filter) = serde_json::from_value::<meerkat_core::ToolFilter>(raw_filter)
    {
        state.active_filter = filter.clone();
        state.staged_filter = filter;
        changed = true;
    }

    if let Some(raw_filter) = session
        .metadata()
        .get(meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY)
        .cloned()
        && let Ok(filter) = serde_json::from_value::<meerkat_core::ToolFilter>(raw_filter)
    {
        state.inherited_base_filter = filter;
        changed = true;
    }

    changed.then_some(state)
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

type OpsLifecycleRegistryArc = Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>;
type OpsLifecycleProviderFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Option<OpsLifecycleRegistryArc>> + Send + 'a>,
>;
type OpsLifecycleProvider = dyn Fn(&SessionId) -> OpsLifecycleProviderFuture<'_> + Send + Sync;

type RuntimeBindingsProviderFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Option<meerkat_core::SessionRuntimeBindings>> + Send>,
>;
type RuntimeBindingsProvider = dyn Fn(SessionId) -> RuntimeBindingsProviderFuture + Send + Sync;

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
        if let Err(e) =
            externalize_messages_from(self.blob_store.as_ref(), persisted.messages_mut(), 0).await
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
    /// Process-local bounded cache of archived full sessions to avoid
    /// immediately reloading durable archived snapshots on hot history reads.
    archived_sessions: Mutex<IndexMap<SessionId, Session>>,
    archived_history_capacity: usize,
    /// Gates for active keep-alive checkpointers, keyed by session ID.
    /// Archive acquires the gate's lock, sets cancelled, then saves the
    /// archived snapshot -- mutual exclusion prevents a concurrent checkpoint
    /// from overwriting the archived row with a live one.
    checkpointer_gates: Mutex<HashMap<SessionId, Arc<CheckpointerGate>>>,
    /// Optional provider for ops lifecycle registries used during lazy session
    /// rebuilds. When set, lazy-rebuilt sessions bind to the canonical registry
    /// instead of allocating an orphaned fallback.
    ops_lifecycle_provider: Option<Arc<OpsLifecycleProvider>>,
    /// Optional provider for runtime bindings used during lazy session rebuilds.
    /// When set, preferred over `ops_lifecycle_provider` — supplies both the
    /// canonical ops lifecycle registry and the runtime build mode in one shot.
    runtime_bindings_provider: Option<Arc<RuntimeBindingsProvider>>,
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
    ) -> Result<Vec<StoredInputState>, SessionError> {
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
                let mut bundle = stored_states
                    .iter()
                    .find(|candidate| &candidate.state.input_id == input_id)?
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
        self.persist_full_session(id).await
    }

    pub async fn dispatch_external_tool_call(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        let outcome = self.inner.dispatch_external_tool_call(id, call).await?;
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
        Self {
            inner: EphemeralSessionService::new(builder, max_sessions),
            store,
            runtime_store,
            blob_store,
            archived_sessions: Mutex::new(IndexMap::new()),
            archived_history_capacity: max_sessions.max(1),
            checkpointer_gates: Mutex::new(HashMap::new()),
            ops_lifecycle_provider: None,
            runtime_bindings_provider: None,
        }
    }

    /// Set an ops lifecycle provider for lazy session rebuilds.
    ///
    /// When a persisted session is rehydrated lazily (e.g., runtime context
    /// appends for a session not currently live), this provider supplies the
    /// canonical ops lifecycle registry so the rebuilt agent's async ops are
    /// visible to the runtime adapter.
    pub fn set_ops_lifecycle_provider(&mut self, provider: Arc<OpsLifecycleProvider>) {
        self.ops_lifecycle_provider = Some(provider);
    }

    /// Set a runtime bindings provider for lazy session rebuilds.
    ///
    /// When set, this is preferred over `ops_lifecycle_provider`. On lazy
    /// rebuild the callback is invoked with the session ID; if it returns
    /// `Some(bindings)`, the rebuild sets `runtime_build_mode` to
    /// `SessionOwned(bindings)` and skips the legacy `ops_lifecycle_override`
    /// path.
    pub fn set_runtime_bindings_provider(&mut self, provider: Arc<RuntimeBindingsProvider>) {
        self.runtime_bindings_provider = Some(provider);
    }

    pub fn runtime_mode(&self) -> RuntimeMode {
        RuntimeMode::V9Compliant
    }

    pub fn runtime_store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.runtime_store.clone()
    }

    pub fn blob_store(&self) -> Arc<dyn BlobStore> {
        self.blob_store.clone()
    }

    async fn normalized_session_for_persistence(
        &self,
        mut session: Session,
    ) -> Result<Session, SessionError> {
        externalize_messages_from(self.blob_store.as_ref(), session.messages_mut(), 0)
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
        self.store
            .save(&session)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        Ok(session)
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
            None => CoreApplyOutput::without_terminal(receipt, Some(session_snapshot)),
        };

        Ok(output)
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
        self.apply_runtime_turn_outcome(id, run_id, req, boundary, contributing_input_ids)
            .await
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
        match self.inner.start_turn(id, req).await {
            Ok(run_result) => {
                let output = self
                    .build_runtime_output(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(CoreApplyTerminal::RunResult(run_result.clone())),
                    )
                    .await?;
                Ok((run_result, output))
            }
            Err(SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary)) => {
                // ResumePending with no boundary — no-op through canonical path.
                // Callers (CLI, RPC) discard the RunResult — this is a legacy
                // placeholder.
                let output = self
                    .build_runtime_output(id, run_id, boundary, contributing_input_ids, None)
                    .await?;
                let noop_result = RunResult {
                    text: String::new(),
                    session_id: id.clone(),
                    usage: meerkat_core::types::Usage::default(),
                    turns: 0,
                    tool_calls: 0,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                };
                Ok((noop_result, output))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn apply_runtime_turn_outcome(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        match self.inner.start_turn(id, req).await {
            Ok(run_result) => {
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(run_result)),
                )
                .await
            }
            Err(SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary)) => {
                // ResumePending with no boundary — no-op through canonical path.
                self.build_runtime_output(id, run_id, boundary, contributing_input_ids, None)
                    .await
            }
            Err(error) => {
                if let Some(terminal) = Self::callback_pending_terminal(&error) {
                    self.build_runtime_output(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(terminal),
                    )
                    .await
                } else {
                    Err(error)
                }
            }
        }
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

    pub async fn apply_runtime_context_appends_with_boundary(
        &self,
        id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        if let Err(SessionError::NotFound { .. }) = self
            .inner
            .apply_runtime_system_context(id, appends.clone())
            .await
        {
            let stored = self
                .load_persisted(id)
                .await?
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            let runtime_build_mode = if let Some(provider) = &self.runtime_bindings_provider {
                (provider)(id.clone())
                    .await
                    .map(meerkat_core::RuntimeBuildMode::SessionOwned)
            } else if let Some(provider) = &self.ops_lifecycle_provider {
                (provider)(id).await.map(|ops_lifecycle| {
                    meerkat_core::RuntimeBuildMode::SessionOwned(
                        meerkat_core::SessionRuntimeBindings {
                            session_id: id.clone(),
                            epoch_id: meerkat_core::RuntimeEpochId::new(),
                            ops_lifecycle,
                            cursor_state: Arc::new(meerkat_core::EpochCursorState::new()),
                            tool_visibility_owner: Arc::new(
                                meerkat_core::LocalToolVisibilityOwner::new(),
                            ),
                            turn_state: Arc::new(
                                meerkat_runtime::RuntimeTurnStateHandle::ephemeral(),
                            ),
                            comms_drain: Arc::new(
                                meerkat_runtime::RuntimeCommsDrainHandle::ephemeral(),
                            ),
                            external_tool_surface: Arc::new(
                                meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral(),
                            ),
                            peer_comms: Arc::new(
                                meerkat_runtime::RuntimePeerCommsHandle::ephemeral(),
                            ),
                            session_admission: Arc::new(
                                meerkat_runtime::RuntimeSessionAdmissionHandle::ephemeral(),
                            ),
                            auth_lease: Arc::new(
                                meerkat_runtime::RuntimeAuthLeaseHandle::ephemeral(),
                            ),
                            mcp_server_lifecycle: Arc::new(
                                meerkat_runtime::RuntimeMcpServerLifecycleHandle::ephemeral(),
                            ),
                            // W1-A (#264): recovery path leaves the peer-interaction
                            // handle unset. An ephemeral handle here would start in
                            // `Initializing` without receiving `Initialize` /
                            // `RegisterSession`, so every `request_sent` would fail
                            // the phase guard and silently break PeerRequest sends on
                            // recovered sessions. The factory still treats the handle
                            // as optional — once the runtime epoch rebinds through
                            // normal `prepare_bindings`, a properly-initialized handle
                            // replaces this `None` via the ordinary install path.
                            peer_interaction: None,
                            // W2-E (#264): recovery path uses an ephemeral session
                            // context handle. `AdvanceSessionContext` is per_phase and
                            // survives the `Initializing` phase, so this path does
                            // not have the same phase-guard issue as peer-interaction.
                            session_context: Arc::new(
                                meerkat_runtime::RuntimeSessionContextHandle::ephemeral(),
                            ),
                            // Recovery path: same rationale as `peer_interaction`
                            // above — until the runtime epoch rebinds via
                            // `prepare_bindings`, fall back to the process-global
                            // default registry so bare comms paths still hit a
                            // canonical owner (dogma #2).
                            session_claim_handle:
                                meerkat_core::handles::DefaultSessionClaimRegistry::global()
                                    as Arc<dyn meerkat_core::handles::SessionClaimHandle>,
                        },
                    )
                })
            } else {
                None
            };
            let recovered = build_recovered_session(
                stored,
                &SurfaceSessionRecoveryOverrides::default(),
                SurfaceSessionRecoveryContext {
                    runtime_build_mode,
                    ..Default::default()
                },
            )
            .map_err(|error| {
                SessionError::Agent(AgentError::InternalError(format!(
                    "failed to recover stored-only session for runtime context append: {error}"
                )))
            })?;

            self.create_session(recovered.into_deferred_create_request())
                .await?;

            self.inner
                .apply_runtime_system_context(id, appends.clone())
                .await?;
        }

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
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(
        &self,
        mut req: CreateSessionRequest,
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
        let build = req.build.get_or_insert_with(Default::default);
        build.checkpointer = Some(checkpointer.clone());
        build.blob_store_override = Some(Arc::clone(&self.blob_store));
        let callback_session_id = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone());
        let result = match self.inner.create_session(req).await {
            Ok(result) => result,
            Err(error) => {
                if Self::callback_pending_terminal(&error).is_some()
                    && let Some(session_id) = callback_session_id
                {
                    let _ = self.persist_full_session(&session_id).await;
                }
                return Err(error);
            }
        };

        // Track the gate so archive() can cancel checkpoint writes.
        {
            self.checkpointer_gates
                .lock()
                .await
                .insert(result.session_id.clone(), gate);
        }

        // Persist the full session snapshot (messages + metadata) after first
        // turn and seed the checkpointer so the next keep-alive checkpoint is
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
        let result = match self.inner.start_turn(id, req).await {
            Ok(result) => result,
            Err(error) => {
                if Self::callback_pending_terminal(&error).is_some() {
                    let _ = self.persist_full_session(id).await;
                }
                return Err(error);
            }
        };

        // Always persist after a direct start_turn call. Runtime-backed sessions
        // that go through apply_runtime_turn_with_result() have their own atomic
        // boundary commit path and don't call start_turn on PersistentSessionService.
        let _ = self.persist_full_session(id).await?;

        Ok(result)
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
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
    ) -> Result<(), SessionError> {
        self.inner
            .hot_swap_session_llm_identity(id, client, identity)
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

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        let previous_visibility_state = self
            .export_session_with_labels(id)
            .await
            .map(|session| rollback_tool_visibility_state_snapshot(&session))?;

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
                        model: session
                            .session_metadata()
                            .map(|meta| meta.model)
                            .unwrap_or_else(|| "unknown".to_string()),
                        provider: session
                            .session_metadata()
                            .map(|meta| meta.provider)
                            .unwrap_or(meerkat_core::Provider::Other),
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

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        let _ = self.discard_stale_live_session_if_needed(id).await?;
        self.inner.has_live_session(id).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let archived_snapshot = match self.export_session_with_labels(id).await {
            Ok(session) => Some(session),
            Err(SessionError::NotFound { .. }) => self.load_authoritative_session_base(id).await?,
            Err(err) => return Err(err),
        };
        if let Some(ref session) = archived_snapshot
            && metadata_marks_archived(session.metadata())
        {
            self.remember_archived_session(session.clone()).await;
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

        let live_result = self.inner.archive(id).await;

        let in_store = self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        if let Some(mut session) = archived_snapshot.clone() {
            session.set_metadata(SESSION_ARCHIVED_KEY, serde_json::Value::Bool(true));
            let session = self.save_normalized_session(session).await?;
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

    async fn update_session_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        // Snapshot the previous value so we can roll back on store failure.
        let previous = self
            .export_session_with_labels(id)
            .await?
            .session_metadata()
            .map(|m| m.keep_alive)
            .unwrap_or(false);

        // Update the live in-memory session metadata (persist_full_session
        // reads from live state, so the mutation must happen before the write).
        self.inner.update_session_keep_alive(id, keep_alive).await?;

        // Persist to store so recovery/resume inherits the updated intent.
        // If the store write fails, roll back to the previous value.
        if let Err(e) = self.persist_full_session(id).await {
            let _ = self.inner.update_session_keep_alive(id, previous).await;
            return Err(e);
        }
        Ok(())
    }

    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
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
            let gate_guard = gate.cancelled.lock().await;
            if *gate_guard {
                return Err(SessionControlError::Session(SessionError::NotFound {
                    id: id.clone(),
                }));
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
            .store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?
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

    /// Load the authoritative durable session view.
    ///
    /// In runtime-backed mode, the runtime store can hold a newer canonical
    /// boundary snapshot than the plain session-store row. Recovery and
    /// post-turn inspection must therefore read through the same store/runtime
    /// reconciliation seam instead of assuming the raw session-store row is
    /// authoritative.
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
        let message_count = session.messages().len();
        let _ = self.save_normalized_session(session).await?;

        Ok(message_count)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ephemeral::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
    use meerkat_core::ToolDispatchOutcome;
    use meerkat_core::checkpoint::SessionCheckpointer;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
        SessionServiceControlExt,
    };
    use meerkat_core::types::{
        ContentBlock, ContentInput, ImageData, Message, ToolCall, ToolResult, UserMessage,
    };
    use meerkat_core::{RunId, lifecycle::run_primitive::RunApplyBoundary};
    use meerkat_runtime::InMemoryRuntimeStore;
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionStoreError};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn memory_blob_store() -> Arc<dyn BlobStore> {
        Arc::new(MemoryBlobStore::new())
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

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_flow_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), meerkat_core::error::AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: meerkat_core::SessionLlmIdentity,
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
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
            let mut state = session.tool_visibility_state().unwrap_or_default();
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
            }));
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: session.id().clone(),
                usage: meerkat_core::types::Usage::default(),
                turns: 1,
                tool_calls: 0,
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
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
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
            })
        }
    }

    struct ToolDispatchAgent {
        session: Arc<std::sync::Mutex<Session>>,
        system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
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
            let mut state = session.tool_visibility_state().unwrap_or_default();
            state
                .staged_requested_deferred_names
                .insert(format!("requested:{}", call.name));
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
        ) -> Result<(), meerkat_core::error::AgentError> {
            let mut session = match self.session.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut metadata =
                session
                    .session_metadata()
                    .unwrap_or(meerkat_core::SessionMetadata {
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
                Message::ToolResults { results } => {
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
            additional_instructions: None,
            execution_kind: None,
        }
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
            additional_instructions: None,
            execution_kind: None,
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
        service
            .apply_runtime_turn_with_result(
                &created.session_id,
                run_id,
                StartTurnRequest {
                    prompt: image_prompt("runtime"),
                    ..start_turn_request("ignored")
                },
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
        let service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
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
        let service = PersistentSessionService::new(
            DummyBuilder,
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

        let restarted = PersistentSessionService::new(
            DummyBuilder,
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
    async fn test_runtime_backed_create_session_persists_initial_snapshot() {
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
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling {
                    builtins: meerkat_core::ToolCategoryOverride::Enable,
                    shell: meerkat_core::ToolCategoryOverride::Disable,
                    comms: meerkat_core::ToolCategoryOverride::Disable,
                    mob: meerkat_core::ToolCategoryOverride::Disable,
                    memory: meerkat_core::ToolCategoryOverride::Enable,
                    active_skills: None,
                },
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
        session
            .set_build_state(meerkat_core::SessionBuildState {
                system_prompt: Some("persisted system prompt".to_string()),
                output_schema: None,
                hooks_override: meerkat_core::HookRunOverrides::default(),
                budget_limits: Some(meerkat_core::BudgetLimits {
                    max_tokens: Some(32),
                    max_duration: Some(meerkat_core::time_compat::Duration::from_secs(12)),
                    max_tool_calls: Some(3),
                }),
                recoverable_tool_defs: vec![meerkat_core::ToolDef {
                    name: "persisted_tool".to_string(),
                    description: "persisted callback tool".to_string(),
                    input_schema: serde_json::json!({"type":"object"}),
                    provenance: None,
                }],
                silent_comms_intents: vec!["peer-b".to_string()],
                max_inline_peer_notifications: Some(2),
                app_context: Some(serde_json::json!({ "surface": "persisted" })),
                additional_instructions: Some(vec!["persisted instruction".to_string()]),
                shell_env: Some(std::collections::HashMap::from([(
                    "MEERKAT_MODE".to_string(),
                    "persisted".to_string(),
                )])),
                mob_tool_authority_context: None,
                call_timeout_override: meerkat_core::CallTimeoutOverride::Value(
                    meerkat_core::time_compat::Duration::from_secs(21),
                ),
            })
            .expect("session build state should serialize");
        store
            .save(&session)
            .await
            .expect("persisted session should save");

        let output = service
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
            .expect("stored-only runtime append should recover the live session");

        assert_eq!(output.receipt.contributing_input_ids.len(), 1);

        let restored = service
            .export_live_session(&id)
            .await
            .expect("runtime append should recreate a live session");
        let restored_build = restored
            .build_state()
            .expect("recovered live session should preserve build state");
        let system_prompt = restored
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("restored session should contain a system prompt");
        assert!(system_prompt.contains("recover me"));
        assert_eq!(
            restored_build.system_prompt.as_deref(),
            Some("persisted system prompt")
        );
        assert_eq!(
            restored_build.app_context,
            Some(serde_json::json!({ "surface": "persisted" }))
        );
        assert_eq!(
            restored_build.shell_env,
            Some(std::collections::HashMap::from([(
                "MEERKAT_MODE".to_string(),
                "persisted".to_string(),
            )]))
        );
        assert_eq!(restored_build.recoverable_tool_defs.len(), 1);
        assert_eq!(
            restored_build.recoverable_tool_defs[0].name,
            "persisted_tool"
        );
        assert_eq!(restored_build.max_inline_peer_notifications, Some(2));

        let persisted = store
            .load(&id)
            .await
            .expect("load should succeed")
            .expect("runtime append should repersist the session");
        let persisted_build = persisted
            .build_state()
            .expect("persisted session should keep recovered build state");
        let persisted_prompt = persisted
            .messages()
            .first()
            .and_then(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("persisted session should contain a system prompt");
        assert!(persisted_prompt.contains("recover me"));
        assert_eq!(
            persisted_build.system_prompt.as_deref(),
            Some("persisted system prompt")
        );
        assert_eq!(
            persisted_build.app_context,
            Some(serde_json::json!({ "surface": "persisted" }))
        );
        assert_eq!(
            persisted_build.recoverable_tool_defs[0].name,
            "persisted_tool"
        );
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
                let normalized = prompt.to_lowercase();
                assert!(
                    normalized.contains("peer_response_terminal:analyst-rt:req-123"),
                    "run_started prompt should expose runtime system-context source: {prompt}"
                );
                assert!(
                    normalized.contains("birch seventeen"),
                    "run_started prompt should expose authoritative terminal peer payload: {prompt}"
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
    async fn test_apply_runtime_context_appends_reinjects_generated_create_only_mob_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = CapturingBuildBuilder::new();
        let captured_builds = Arc::clone(&builder.captured_builds);
        let service = PersistentSessionService::new(
            builder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut session = Session::new();
        let id = session.id().clone();
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling {
                    builtins: meerkat_core::ToolCategoryOverride::Enable,
                    shell: meerkat_core::ToolCategoryOverride::Disable,
                    comms: meerkat_core::ToolCategoryOverride::Disable,
                    mob: meerkat_core::ToolCategoryOverride::Enable,
                    memory: meerkat_core::ToolCategoryOverride::Enable,
                    active_skills: None,
                },
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

        service
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
            .expect("stored-only runtime append should recover the live session");

        let captured_builds = captured_builds.lock().await;
        let build = captured_builds
            .last()
            .expect("recovery builder should capture a build");
        assert_eq!(
            build.override_mob,
            meerkat_core::ToolCategoryOverride::Enable
        );
        let authority = build
            .mob_tool_authority_context
            .as_ref()
            .expect("explicit mob enable should rehydrate generated create-only authority");
        assert!(
            authority.can_create_mobs(),
            "recovered runtime build should keep create authority"
        );
        assert!(
            authority.managed_mob_scope().is_empty(),
            "stored-only recovery must not widen exact scope from bookkeeping state"
        );
    }

    #[tokio::test]
    async fn test_apply_runtime_context_appends_rehydrates_persisted_typed_mob_authority() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let builder = CapturingBuildBuilder::new();
        let captured_builds = Arc::clone(&builder.captured_builds);
        let service = PersistentSessionService::new(
            builder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );

        let mut session = Session::new();
        let id = session.id().clone();
        session
            .set_session_metadata(meerkat_core::SessionMetadata {
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling {
                    builtins: meerkat_core::ToolCategoryOverride::Enable,
                    shell: meerkat_core::ToolCategoryOverride::Disable,
                    comms: meerkat_core::ToolCategoryOverride::Disable,
                    mob: meerkat_core::ToolCategoryOverride::Inherit,
                    memory: meerkat_core::ToolCategoryOverride::Enable,
                    active_skills: None,
                },
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
        let expected_authority = meerkat_core::service::MobToolAuthorityContext::new(
            meerkat_core::service::OpaquePrincipalToken::new("persisted-typed"),
            false,
        )
        .with_managed_mob_scope(["mob-a", "mob-b"])
        .with_caller_provenance(
            meerkat_core::service::MobToolCallerProvenance::new()
                .with_session_id(SessionId::new())
                .with_member_id("lead-1"),
        )
        .with_audit_invocation_id("audit-persisted");
        session
            .set_mob_tool_authority_context(Some(expected_authority.clone()))
            .expect("typed mob authority should serialize");
        store
            .save(&session)
            .await
            .expect("persisted session should save");

        service
            .apply_runtime_context_appends(
                &id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "recover scoped authority".to_string(),
                    source: Some("system-generated:test".to_string()),
                    idempotency_key: None,
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("stored-only runtime append should recover the live session");

        let captured_builds = captured_builds.lock().await;
        let build = captured_builds
            .last()
            .expect("recovery builder should capture a build");
        assert_eq!(
            build.override_mob,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            build.mob_tool_authority_context,
            Some(expected_authority),
            "stored-only recovery must preserve exact managed scope and provenance"
        );
    }

    #[tokio::test]
    async fn test_update_session_mob_authority_context_persists_for_stored_only_recovery() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let seed_service = PersistentSessionService::new(
            DummyBuilder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        let run = seed_service
            .create_session(create_request_with_metadata(
                "hello",
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        let id = run.session_id;

        let expected_authority = meerkat_core::service::MobToolAuthorityContext::new(
            meerkat_core::service::OpaquePrincipalToken::new("granted-scope"),
            true,
        )
        .grant_manage_mob("mob-created");
        seed_service
            .update_session_mob_authority_context(&id, Some(expected_authority.clone()))
            .await
            .expect("persist exact mob authority");

        let persisted = store
            .load(&id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        assert_eq!(
            persisted.mob_tool_authority_context(),
            Some(expected_authority.clone()),
            "store snapshot should carry the canonical authority update"
        );

        let recovering_builder = CapturingBuildBuilder::new();
        let captured_builds = Arc::clone(&recovering_builder.captured_builds);
        let recovering_service = PersistentSessionService::new(
            recovering_builder,
            4,
            Arc::clone(&store),
            None,
            memory_blob_store(),
        );
        recovering_service
            .apply_runtime_context_appends(
                &id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "recover exact scope".to_string(),
                    source: Some("system-generated:test".to_string()),
                    idempotency_key: None,
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![InputId::new()],
            )
            .await
            .expect("stored-only runtime append should recover the live session");

        let captured_builds = captured_builds.lock().await;
        let build = captured_builds
            .last()
            .expect("recovery builder should capture a build");
        assert_eq!(
            build.override_mob,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(build.mob_tool_authority_context, Some(expected_authority));
    }

    /// Create a session request that seeds initial SessionMetadata so
    /// update_keep_alive has something to mutate (mirrors what the real factory does).
    fn create_request_with_metadata(
        prompt: &str,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        let mut session = Session::new();
        let metadata = meerkat_core::SessionMetadata {
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
            .update_session_keep_alive(&id, true)
            .await
            .expect("update_session_keep_alive should succeed");

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
        let result = service.update_session_keep_alive(&id, true).await;
        assert!(result.is_err(), "false→true should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            !exported.session_metadata().unwrap().keep_alive,
            "should roll back to false after failed false→true"
        );

        // --- Bring live state to true successfully ---
        service.update_session_keep_alive(&id, true).await.unwrap();
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(exported.session_metadata().unwrap().keep_alive);

        // --- Transition: true → true with store failure (idempotent retry) ---
        fail_store.set_fail_save(true);
        let result = service.update_session_keep_alive(&id, true).await;
        assert!(result.is_err(), "true→true should fail when store fails");
        fail_store.set_fail_save(false);
        let exported = service.export_session_with_labels(&id).await.unwrap();
        assert!(
            exported.session_metadata().unwrap().keep_alive,
            "should stay true after failed true→true (not flip to false)"
        );

        // --- Transition: true → false with store failure ---
        fail_store.set_fail_save(true);
        let result = service.update_session_keep_alive(&id, false).await;
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
            .expect("live session should carry visibility state after staging");
        assert_eq!(exported_state.staged_filter, filter);
        assert_eq!(exported_state.active_filter, meerkat_core::ToolFilter::All);
        assert_eq!(exported_state.staged_revision, 1);
        assert_eq!(exported_state.active_revision, 0);

        let persisted = store.load(&id).await.unwrap().unwrap();
        let persisted_state = persisted
            .tool_visibility_state()
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
            baseline.tool_visibility_state().is_none(),
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
        assert_eq!(
            exported.tool_visibility_state(),
            baseline.tool_visibility_state(),
            "live session should roll back to the pre-mutation visibility state"
        );

        let persisted = fail_store.inner.load(&id).await.unwrap().unwrap();
        assert_eq!(
            persisted.tool_visibility_state(),
            baseline.tool_visibility_state(),
            "store should retain the pre-mutation visibility state after rollback"
        );
    }

    #[test]
    fn rollback_snapshot_recovers_legacy_filter_metadata_when_canonical_state_is_absent() {
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
            .expect("legacy-only metadata should still produce a rollback snapshot");

        assert_eq!(
            snapshot.active_filter,
            meerkat_core::ToolFilter::Deny(["secret".to_string()].into_iter().collect())
        );
        assert_eq!(
            snapshot.staged_filter,
            meerkat_core::ToolFilter::Deny(["secret".to_string()].into_iter().collect())
        );
        assert_eq!(
            snapshot.inherited_base_filter,
            meerkat_core::ToolFilter::Allow(["visible".to_string()].into_iter().collect())
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
                    "tool_catalog_load".to_string(),
                    serde_json::json!({}),
                ),
            )
            .await
            .expect("dispatch external tool call");

        assert_eq!(outcome.result.text_content(), "handled tool_catalog_load");

        let persisted = service
            .load_persisted(&id)
            .await
            .expect("load persisted session")
            .expect("persisted session should exist");
        let visibility_state = persisted
            .tool_visibility_state()
            .expect("persistent dispatch should save session mutations");
        assert!(
            visibility_state
                .staged_requested_deferred_names
                .contains("requested:tool_catalog_load"),
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
                    "tool_catalog_load".to_string(),
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

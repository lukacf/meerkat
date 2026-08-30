//! EphemeralSessionService — in-memory session lifecycle with no persistence.
//!
//! Each session gets a dedicated tokio task that exclusively owns the `Agent`.
//! Communication happens through channels, generalized from `SessionRuntime` in
//! `meerkat-rpc`.

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::error::AgentError;
use meerkat_core::event::{AgentEvent, EventEnvelope, EventSourceIdentity};
use meerkat_core::image_content::{MissingBlobBehavior, hydrate_deferred_turn_state};
use meerkat_core::lifecycle::core_executor::{
    BoundSessionCommit, CoreApplyOutput, CoreApplyTerminal,
};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, TurnRequestContext};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    DeferredPromptPolicy, MobToolAuthorityContext, SessionControlError, SessionError,
    SessionHistoryPage, SessionHistoryQuery, SessionInfo, SessionQuery, SessionService,
    SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary,
    SessionUsage, SessionView, StageToolResultsDisposition, StageToolResultsRequest,
    StageToolResultsResult, StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::session_document::{
    SessionArchiveDisposition, SessionArchiveRuntimeObservation, SessionDocumentEffect,
    SessionDocumentKey, SessionDocumentLifecycle, SessionDocumentMachineAuthority,
};
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::{ContentInput, RunResult, SessionId, ToolResult, Usage};
use meerkat_core::{
    CancelAfterBoundaryCommand, CancelAfterBoundarySender, ConsumedDeferredTurnInputs,
    DeferredFirstTurnPhase, InputId, RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent,
    RealtimeTranscriptMaterializedMessage, RunId, SessionDeferredTurnState, SessionLlmIdentity,
    SnapshotProjectionError, TurnStateHandle,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

// Tokio re-exports: on wasm32, use the crate-level alias (tokio_with_wasm).
#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, mpsc, oneshot, watch};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, mpsc, oneshot, watch};

#[cfg(test)]
use crate::staged_registry::MaterializationStatus;
use crate::staged_registry::{PromotionTicket, StagedSessionRegistry};
pub use crate::turn_admission::ObservedSessionTailKind;
use crate::turn_admission::{
    BeginOutcome, ClaimOutcome, RuntimeKeepAliveOutcome, RuntimeKeepAliveRequest,
    StartTurnDispatchAuthorization, StartTurnDisposition, StartTurnDispositionOutcome,
    StartTurnPublicTerminal, TurnAdmissionPhase, TurnAdmissionProjection, TurnAdmissionSlot,
};

/// Capacity for the internal agent event channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
const LOSSLESS_EVENT_PROJECTION_WARN_DEPTH: usize = 1_024;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
const LOSSLESS_EVENT_PROJECTION_WARN_INTERVAL_MS: u64 = 60_000;

/// Capacity for session command channel.
const COMMAND_CHANNEL_CAPACITY: usize = 8;

/// Store authority against which one actor-local HeadCanonical successor is
/// prepared.
#[derive(Debug, Clone)]
pub enum HeadCanonicalRuntimeBoundaryAuthority {
    Root,
    Successor {
        store_revision: u64,
        boundary_head: meerkat_core::session_store::SessionHead,
        committed_head_token: String,
    },
}

impl PartialEq for HeadCanonicalRuntimeBoundaryAuthority {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Root, Self::Root) => true,
            (
                Self::Successor {
                    store_revision: left_revision,
                    committed_head_token: left_token,
                    ..
                },
                Self::Successor {
                    store_revision: right_revision,
                    committed_head_token: right_token,
                    ..
                },
            ) => left_revision == right_revision && left_token == right_token,
            _ => false,
        }
    }
}

impl Eq for HeadCanonicalRuntimeBoundaryAuthority {}

impl HeadCanonicalRuntimeBoundaryAuthority {
    #[must_use]
    pub const fn root() -> Self {
        Self::Root
    }

    pub fn successor(
        store_revision: u64,
        boundary_head: meerkat_core::session_store::SessionHead,
        committed_head_token: String,
    ) -> Result<Self, meerkat_core::SessionStoreError> {
        if store_revision == 0 || committed_head_token.is_empty() {
            return Err(meerkat_core::SessionStoreError::Internal(
                "head-canonical successor authority requires a non-zero revision and token"
                    .to_string(),
            ));
        }
        let derived = meerkat_core::session_head_cas_token(&boundary_head)?;
        if derived != committed_head_token {
            return Err(
                meerkat_core::SessionStoreError::TranscriptRevisionConflict {
                    id: boundary_head.id,
                    expected: derived,
                    actual: committed_head_token,
                },
            );
        }
        Ok(Self::Successor {
            store_revision,
            boundary_head,
            committed_head_token: derived,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadCanonicalDeferredProjectionSource {
    LiveActorState,
    ExplicitOverride,
}

/// Exact bounded request sent to the actor that owns the live Session.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct HeadCanonicalRuntimeBoundaryPrepareRequest {
    authority: HeadCanonicalRuntimeBoundaryAuthority,
    observed_head: Option<meerkat_core::session_store::SessionHead>,
    deferred_turn_state: SessionDeferredTurnState,
    request_projection_token: String,
    blob_store: Arc<dyn meerkat_core::BlobStore>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl HeadCanonicalRuntimeBoundaryPrepareRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        authority: HeadCanonicalRuntimeBoundaryAuthority,
        observed_head: Option<meerkat_core::session_store::SessionHead>,
        live_deferred_turn_state: SessionDeferredTurnState,
        projection_source: HeadCanonicalDeferredProjectionSource,
        deferred_turn_state_override: Option<SessionDeferredTurnState>,
        role: &str,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Result<Self, AgentError> {
        let deferred_turn_state = match (projection_source, deferred_turn_state_override) {
            (HeadCanonicalDeferredProjectionSource::LiveActorState, None) => {
                live_deferred_turn_state
            }
            (HeadCanonicalDeferredProjectionSource::ExplicitOverride, Some(state)) => state,
            (HeadCanonicalDeferredProjectionSource::LiveActorState, Some(_)) => {
                return Err(AgentError::InternalError(
                    "live actor HeadCanonical projection cannot carry an explicit override"
                        .to_string(),
                ));
            }
            (HeadCanonicalDeferredProjectionSource::ExplicitOverride, None) => {
                return Err(AgentError::InternalError(
                    "explicit HeadCanonical projection is missing its deferred-turn override"
                        .to_string(),
                ));
            }
        };
        let projection = serde_json::to_vec(&(
            format!("{authority:?}"),
            observed_head.as_ref(),
            &deferred_turn_state,
            projection_source as u8,
            role,
        ))
        .map_err(|error| {
            AgentError::InternalError(format!(
                "failed to bind HeadCanonical request projection: {error}"
            ))
        })?;
        let request_projection_token = format!("{:x}", Sha256::digest(projection));
        Ok(Self {
            authority,
            observed_head,
            deferred_turn_state,
            request_projection_token,
            blob_store,
        })
    }

    pub fn into_parts(
        self,
    ) -> (
        HeadCanonicalRuntimeBoundaryAuthority,
        Option<meerkat_core::session_store::SessionHead>,
        SessionDeferredTurnState,
        String,
        Arc<dyn meerkat_core::BlobStore>,
    ) {
        (
            self.authority,
            self.observed_head,
            self.deferred_turn_state,
            self.request_projection_token,
            self.blob_store,
        )
    }
}

#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalRuntimeBoundary {
    committed: BoundSessionCommit,
    successor_head_token: String,
    conversation_digest: String,
    message_count: usize,
}

impl PreparedHeadCanonicalRuntimeBoundary {
    pub fn new(committed: BoundSessionCommit) -> Result<Self, meerkat_core::SessionStoreError> {
        let boundary = committed.head_canonical().ok_or_else(|| {
            meerkat_core::SessionStoreError::Internal(
                "actor-prepared boundary is not HeadCanonical".to_string(),
            )
        })?;
        let successor = boundary.mutation().successor_head();
        Ok(Self {
            successor_head_token: boundary.mutation().successor_head_token().to_string(),
            conversation_digest: successor.head_revision.clone(),
            message_count: usize::try_from(successor.message_count).map_err(|_| {
                meerkat_core::SessionStoreError::Internal(
                    "HeadCanonical successor message count exceeds host index range".to_string(),
                )
            })?,
            committed,
        })
    }

    #[must_use]
    pub fn committed(&self) -> &BoundSessionCommit {
        &self.committed
    }

    #[must_use]
    pub fn successor_head_token(&self) -> &str {
        &self.successor_head_token
    }

    #[must_use]
    pub fn conversation_digest(&self) -> &str {
        &self.conversation_digest
    }

    #[must_use]
    pub const fn message_count(&self) -> usize {
        self.message_count
    }

    #[must_use]
    pub fn into_committed(self) -> BoundSessionCommit {
        self.committed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadCanonicalRuntimeBoundaryAcknowledgeOutcome {
    Applied,
    AlreadyAcknowledgedExact,
}

/// Drive the canonical session-document archive authority for the standalone
/// profile and require its exact mechanical action vector before the service
/// mutates either live or archived registries.
fn authorize_standalone_archive(id: &SessionId) -> Result<(), SessionError> {
    let mut authority = SessionDocumentMachineAuthority::new();
    let document_key = SessionDocumentKey::new(id.to_string());
    authority
        .recover_session_lifecycle_terminal(document_key.clone(), SessionDocumentLifecycle::Active)
        .map_err(|error| {
            SessionError::Agent(AgentError::InternalError(format!(
                "generated session document authority rejected standalone lifecycle recovery for \
                 session {id}: {error}"
            )))
        })?;
    let effects = authority
        .archive_session_document(
            document_key,
            false,
            false,
            SessionArchiveRuntimeObservation::Absent,
        )
        .map_err(|error| {
            SessionError::Agent(AgentError::InternalError(format!(
                "generated session document authority rejected standalone archive for session \
                 {id}: {error}"
            )))
        })?;
    require_standalone_archive_verdict(id, effects)
}

fn require_standalone_archive_verdict(
    id: &SessionId,
    effects: Vec<SessionDocumentEffect>,
) -> Result<(), SessionError> {
    match effects.as_slice() {
        [
            SessionDocumentEffect::SessionArchiveResolved {
                disposition: SessionArchiveDisposition::Archive,
                write_document: false,
                retire_runtime: false,
            },
        ] => Ok(()),
        [
            SessionDocumentEffect::SessionArchiveResolved {
                disposition,
                write_document,
                retire_runtime,
            },
        ] => Err(SessionError::Agent(AgentError::InternalError(format!(
            "generated session document authority returned invalid standalone archive verdict \
                 for session {id}: disposition={disposition:?}, \
                 write_document={write_document}, retire_runtime={retire_runtime}"
        )))),
        [] => Err(SessionError::Agent(AgentError::InternalError(format!(
            "generated session document authority returned no standalone archive verdict for \
             session {id}"
        )))),
        _ => Err(SessionError::Agent(AgentError::InternalError(format!(
            "generated session document authority returned an ambiguous or unexpected standalone \
             archive effect vector for session {id}"
        )))),
    }
}

fn lag_aware_session_event_stream(
    session_id: SessionId,
    rx: tokio::sync::broadcast::Receiver<Arc<EventEnvelope<AgentEvent>>>,
) -> meerkat_core::comms::EventStream {
    Box::pin(futures::stream::unfold(
        (rx, session_id),
        |(mut rx, stream_session_id)| async move {
            match rx.recv().await {
                Ok(event) => Some((Arc::unwrap_or_clone(event), (rx, stream_session_id))),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
                    let marker = EventEnvelope::new_session(
                        stream_session_id.clone(),
                        0,
                        None,
                        AgentEvent::StreamTruncated {
                            reason: meerkat_core::event::StreamTruncationReason::StreamLagged {
                                dropped,
                            },
                        },
                    );
                    Some((marker, (rx, stream_session_id)))
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
            }
        },
    ))
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
struct LosslessEventProjectionSink {
    tx: mpsc::UnboundedSender<QueuedLosslessEvent>,
    queued_events: Arc<AtomicUsize>,
    high_water_events: AtomicUsize,
    last_warning_at_ms: AtomicU64,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl LosslessEventProjectionSink {
    fn new(
        tx: mpsc::UnboundedSender<QueuedLosslessEvent>,
        queued_events: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            tx,
            queued_events,
            high_water_events: AtomicUsize::new(0),
            last_warning_at_ms: AtomicU64::new(0),
        }
    }

    fn observe_backlog(&self, session_id: Option<&SessionId>) {
        let queued_events = self.queued_events.load(Ordering::Relaxed);
        let prior_high_water = self
            .high_water_events
            .fetch_max(queued_events, Ordering::Relaxed);
        let high_water_events = prior_high_water.max(queued_events);
        if queued_events < LOSSLESS_EVENT_PROJECTION_WARN_DEPTH {
            return;
        }

        let now_ms = SystemTime::now()
            .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        let mut last_warning_at_ms = self.last_warning_at_ms.load(Ordering::Relaxed);
        loop {
            if now_ms.saturating_sub(last_warning_at_ms)
                < LOSSLESS_EVENT_PROJECTION_WARN_INTERVAL_MS
            {
                return;
            }
            match self.last_warning_at_ms.compare_exchange_weak(
                last_warning_at_ms,
                now_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => last_warning_at_ms = observed,
            }
        }

        tracing::warn!(
            session_id = ?session_id,
            queued_events,
            high_water_events,
            warning_threshold_events = LOSSLESS_EVENT_PROJECTION_WARN_DEPTH,
            degraded_event_projection = true,
            "lossless durable event projection backlog is growing"
        );
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub(crate) struct QueuedLosslessEvent {
    envelope: Arc<EventEnvelope<AgentEvent>>,
    queued_events: Arc<AtomicUsize>,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl QueuedLosslessEvent {
    pub(crate) fn new(
        envelope: Arc<EventEnvelope<AgentEvent>>,
        queued_events: Arc<AtomicUsize>,
    ) -> Self {
        queued_events.fetch_add(1, Ordering::Relaxed);
        Self {
            envelope,
            queued_events,
        }
    }

    pub(crate) fn envelope(&self) -> &EventEnvelope<AgentEvent> {
        self.envelope.as_ref()
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl Drop for QueuedLosslessEvent {
    fn drop(&mut self) {
        self.queued_events.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub(crate) type LosslessEventProjectionStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = QueuedLosslessEvent> + Send>>;

fn publish_session_event_to_broadcasts(
    session_event_tx: &tokio::sync::broadcast::Sender<Arc<EventEnvelope<AgentEvent>>>,
    raw_session_event_tx: &tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
    envelope: Arc<EventEnvelope<AgentEvent>>,
) {
    let raw_envelope =
        (raw_session_event_tx.receiver_count() > 0).then(|| envelope.as_ref().clone());
    let _ = session_event_tx.send(envelope);
    if let Some(raw_envelope) = raw_envelope {
        let _ = raw_session_event_tx.send(raw_envelope);
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
async fn publish_session_event_to_channels(
    lossless_event_projection_tx: &Arc<
        tokio::sync::Mutex<Option<Arc<LosslessEventProjectionSink>>>,
    >,
    session_event_tx: &tokio::sync::broadcast::Sender<Arc<EventEnvelope<AgentEvent>>>,
    raw_session_event_tx: &tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
    envelope: EventEnvelope<AgentEvent>,
) {
    let envelope = Arc::new(envelope);
    let lossless_sink = lossless_event_projection_tx.lock().await.clone();
    if let Some(lossless_sink) = lossless_sink.as_ref() {
        let queued = QueuedLosslessEvent::new(
            Arc::clone(&envelope),
            Arc::clone(&lossless_sink.queued_events),
        );
        if lossless_sink.tx.send(queued).is_err() {
            tracing::error!(
                session_id = ?envelope.source_session_id(),
                "lossless durable event projection sink closed"
            );
            let mut slot = lossless_event_projection_tx.lock().await;
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, lossless_sink))
            {
                *slot = None;
            }
        } else {
            lossless_sink.observe_backlog(envelope.source_session_id());
        }
    }
    publish_session_event_to_broadcasts(session_event_tx, raw_session_event_tx, envelope);
}

#[cfg(test)]
mod session_event_stream_tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn lagged_session_subscription_yields_typed_gap_before_retained_events()
    -> Result<(), String> {
        let session_id = SessionId::new();
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        let mut stream = lag_aware_session_event_stream(session_id.clone(), rx);

        for seq in 1..=3 {
            tx.send(Arc::new(EventEnvelope::new_session(
                session_id.clone(),
                seq,
                None,
                AgentEvent::StreamTruncated {
                    reason: meerkat_core::event::StreamTruncationReason::ChannelFull,
                },
            )))
            .map_err(|_| "test receiver unexpectedly closed".to_string())?;
        }

        let gap = stream
            .next()
            .await
            .ok_or_else(|| "expected lag marker".to_string())?;
        assert_eq!(gap.source_session_id(), Some(&session_id));
        assert_eq!(
            gap.seq, 0,
            "synthetic gap marker is not a canonical sequence"
        );
        assert!(matches!(
            gap.payload,
            AgentEvent::StreamTruncated {
                reason: meerkat_core::event::StreamTruncationReason::StreamLagged { dropped: 1 }
            }
        ));

        let retained = stream
            .next()
            .await
            .ok_or_else(|| "expected first retained event".to_string())?;
        assert_eq!(retained.seq, 2);
        Ok(())
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn lossless_projection_sink_receives_every_event_when_broadcast_lags()
    -> Result<(), String> {
        const EVENT_COUNT: u64 = 1_100;
        let session_id = SessionId::new();
        let (broadcast_tx, broadcast_rx) = tokio::sync::broadcast::channel(2);
        let (raw_broadcast_tx, raw_broadcast_rx) = tokio::sync::broadcast::channel(2);
        drop(raw_broadcast_rx);
        let mut broadcast_stream = lag_aware_session_event_stream(session_id.clone(), broadcast_rx);
        let (lossless_tx, mut lossless_rx) = mpsc::unbounded_channel::<QueuedLosslessEvent>();
        let queued_events = Arc::new(AtomicUsize::new(0));
        let lossless_sink = Arc::new(LosslessEventProjectionSink::new(lossless_tx, queued_events));
        let lossless_slot = Arc::new(tokio::sync::Mutex::new(Some(Arc::clone(&lossless_sink))));

        for seq in 1..=EVENT_COUNT {
            publish_session_event_to_channels(
                &lossless_slot,
                &broadcast_tx,
                &raw_broadcast_tx,
                EventEnvelope::new_session(
                    session_id.clone(),
                    seq,
                    None,
                    AgentEvent::StreamTruncated {
                        reason: meerkat_core::event::StreamTruncationReason::ChannelFull,
                    },
                ),
            )
            .await;
        }
        *lossless_slot.lock().await = None;
        assert_eq!(
            lossless_sink.high_water_events.load(Ordering::Relaxed),
            EVENT_COUNT as usize
        );
        assert_ne!(
            lossless_sink.last_warning_at_ms.load(Ordering::Relaxed),
            0,
            "crossing the queue-depth threshold must arm a rate-limited warning"
        );
        drop(lossless_sink);

        let mut lossless_seqs = Vec::new();
        while let Some(event) = lossless_rx.recv().await {
            lossless_seqs.push(event.envelope().seq);
        }
        assert_eq!(lossless_seqs, (1..=EVENT_COUNT).collect::<Vec<_>>());

        let gap = broadcast_stream
            .next()
            .await
            .ok_or_else(|| "expected broadcast lag marker".to_string())?;
        assert!(matches!(
            gap.payload,
            AgentEvent::StreamTruncated {
                reason: meerkat_core::event::StreamTruncationReason::StreamLagged {
                    dropped: 1_098
                }
            }
        ));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Observable state of a session, projected from generated turn-admission
/// authority.
type SessionState = TurnAdmissionProjection;

/// Snapshot of session metadata for read/list operations.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub total_tokens: u64,
    pub usage: Usage,
    pub last_assistant_text: Option<String>,
}

/// Bounded actor-owned transcript authority used to compare a live SessionTask
/// with the exact committed store boundary.
///
/// This carrier deliberately excludes the transcript body. Implementations
/// must derive it from their canonical live `Session`; runtime-backed wrappers
/// cannot silently omit the capability and force ordinary authority checks
/// back through an O(document) export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTranscriptAuthoritySnapshot {
    session_id: SessionId,
    transcript_revision: String,
    message_count: usize,
    mutation_generation: u64,
}

impl SessionTranscriptAuthoritySnapshot {
    pub fn from_session(session: &meerkat_core::Session) -> Result<Self, AgentError> {
        Ok(Self {
            session_id: session.id().clone(),
            transcript_revision: session.transcript_content_digest().map_err(|error| {
                AgentError::InternalError(format!(
                    "failed to derive live transcript authority for session {}: {error}",
                    session.id()
                ))
            })?,
            message_count: session.messages().len(),
            // SessionTask replaces this placeholder with its actor-owned
            // monotonic generation before the snapshot crosses the service
            // boundary.
            mutation_generation: 0,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn transcript_revision(&self) -> &str {
        &self.transcript_revision
    }

    #[must_use]
    pub const fn message_count(&self) -> usize {
        self.message_count
    }

    #[must_use]
    pub const fn mutation_generation(&self) -> u64 {
        self.mutation_generation
    }

    fn bind_actor_generation(mut self, generation: u64) -> Self {
        self.mutation_generation = generation;
        self
    }
}

/// Transcript authority bound to one exact live actor incarnation.
///
/// The inner snapshot fences transcript ABA within the actor. The witness
/// fences remove-and-recreate ABA for the same logical `SessionId`.
#[derive(Clone)]
pub struct LiveSessionTranscriptAuthoritySnapshot {
    actor_witness: LiveSessionActorWitness,
    authority: SessionTranscriptAuthoritySnapshot,
}

impl LiveSessionTranscriptAuthoritySnapshot {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.authority.session_id()
    }

    #[must_use]
    pub fn transcript_revision(&self) -> &str {
        self.authority.transcript_revision()
    }

    #[must_use]
    pub const fn message_count(&self) -> usize {
        self.authority.message_count()
    }

    #[must_use]
    pub const fn mutation_generation(&self) -> u64 {
        self.authority.mutation_generation()
    }
}

impl PartialEq for LiveSessionTranscriptAuthoritySnapshot {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(
            &self.actor_witness.incarnation,
            &other.actor_witness.incarnation,
        ) && self.authority == other.authority
    }
}

impl Eq for LiveSessionTranscriptAuthoritySnapshot {}

/// Opaque identity for one exact in-process session actor.
///
/// A [`SessionId`] names the logical session and is intentionally reusable
/// across actor recovery.  This witness instead names the concrete handle
/// stored in the session service registry.  Callers may clone and compare it,
/// but cannot manufacture authority over a replacement actor.
#[derive(Clone)]
pub struct LiveSessionActorWitness {
    session_id: SessionId,
    incarnation: Arc<LiveSessionActorIncarnation>,
}

struct LiveSessionActorIncarnation {
    live: AtomicBool,
    transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
}

/// One-shot publication cell for the exact actor inserted by a service create.
///
/// The slot lives at the ephemeral registry boundary so both native and wasm
/// persistent facades can publish the witness synchronously after insertion,
/// before an eager first turn reaches its first suspension point.
#[derive(Clone, Default)]
pub struct LiveSessionActorWitnessSlot {
    witness: Arc<OnceLock<LiveSessionActorWitness>>,
}

impl LiveSessionActorWitnessSlot {
    /// Capture the exact actor witness once it has been published by the
    /// session service. An empty slot proves only that actor construction did
    /// not reach its atomic registry-publication boundary.
    pub fn witness(&self) -> Option<LiveSessionActorWitness> {
        self.witness.get().cloned()
    }

    /// Publish the service-minted actor identity at registry insertion.
    #[doc(hidden)]
    pub fn publish(&self, witness: LiveSessionActorWitness) -> Result<(), SessionError> {
        self.witness.set(witness).map_err(|duplicate| {
            SessionError::Agent(AgentError::InternalError(format!(
                "live actor witness for session {} was published more than once",
                duplicate.session_id()
            )))
        })
    }
}

impl LiveSessionActorWitness {
    fn new(
        session_id: SessionId,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    ) -> Self {
        Self {
            session_id,
            incarnation: Arc::new(LiveSessionActorIncarnation {
                live: AtomicBool::new(true),
                transient_turn_context_state,
            }),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn is_handle(&self, handle: &SessionHandle) -> bool {
        self.session_id == handle.actor_witness.session_id
            && Arc::ptr_eq(&self.incarnation, &handle.actor_witness.incarnation)
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn same_incarnation(&self, other: &Self) -> bool {
        self.session_id == other.session_id && Arc::ptr_eq(&self.incarnation, &other.incarnation)
    }

    /// Whether this exact actor incarnation has not been revoked.
    ///
    /// Session registries revoke a witness before removing or replacing its
    /// actor. Callers that sample this while holding the registry's stable
    /// lifecycle boundary can therefore use it as an exact-incarnation
    /// liveness check, never as logical-session authority by itself.
    pub fn is_live(&self) -> bool {
        self.incarnation.live.load(Ordering::Acquire)
    }

    fn revoke(&self) {
        self.incarnation
            .transient_turn_context_state
            .revoke_boundary_actor();
        self.incarnation.live.store(false, Ordering::Release);
    }
}

impl PartialEq for LiveSessionActorWitness {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id && Arc::ptr_eq(&self.incarnation, &other.incarnation)
    }
}

impl Eq for LiveSessionActorWitness {}

impl std::fmt::Debug for LiveSessionActorWitness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveSessionActorWitness")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Service-owned registry for exact in-process session actor incarnations.
///
/// A logical [`SessionId`] may be reused after recovery, so removal is fenced
/// by [`LiveSessionActorWitness`] identity. Removing actor A can therefore
/// never revoke or remove a later actor B for the same logical session.
#[derive(Default)]
pub struct LiveSessionActorRegistry {
    current: std::sync::Mutex<IndexMap<SessionId, LiveSessionActorWitness>>,
}

impl LiveSessionActorRegistry {
    fn lock_current(
        &self,
    ) -> std::sync::MutexGuard<'_, IndexMap<SessionId, LiveSessionActorWitness>> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Mint, store, and synchronously publish one exact actor incarnation.
    ///
    /// Existing current actors are never replaced implicitly. If slot
    /// publication fails, the newly stored incarnation is removed and
    /// revoked before this method returns.
    pub fn insert_and_publish(
        &self,
        slot: &LiveSessionActorWitnessSlot,
        session_id: SessionId,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    ) -> Result<LiveSessionActorWitness, SessionError> {
        let mut current = self.lock_current();
        if current.contains_key(&session_id) {
            return Err(SessionError::Agent(AgentError::InternalError(format!(
                "live session actor is already registered: {session_id}"
            ))));
        }

        let witness =
            LiveSessionActorWitness::new(session_id.clone(), transient_turn_context_state);
        current.insert(session_id.clone(), witness.clone());
        if let Err(error) = slot.publish(witness.clone()) {
            let removed = current.swap_remove(&session_id);
            drop(current);
            if let Some(removed) = removed {
                removed.revoke();
            }
            return Err(error);
        }

        Ok(witness)
    }

    /// Return the exact actor currently registered for a logical session.
    pub fn current(&self, session_id: &SessionId) -> Option<LiveSessionActorWitness> {
        self.lock_current().get(session_id).cloned()
    }

    /// Return whether a logical session currently has a registered actor.
    pub fn contains(&self, session_id: &SessionId) -> bool {
        self.lock_current().contains_key(session_id)
    }

    /// Remove and synchronously revoke `witness` only if it is still current.
    pub fn compare_remove(&self, witness: &LiveSessionActorWitness) -> bool {
        let removed = {
            let mut current = self.lock_current();
            match current.get(witness.session_id()) {
                Some(candidate) if candidate == witness => {
                    current.swap_remove(witness.session_id())
                }
                Some(_) | None => None,
            }
        };
        let Some(removed) = removed else {
            return false;
        };
        removed.revoke();
        true
    }

    /// Remove and synchronously revoke whichever actor is currently registered.
    ///
    /// This session-scoped operation is only for callers that own the service's
    /// archive/discard boundary and therefore exclude replacement. Delayed or
    /// incarnation-scoped cleanup must use [`Self::compare_remove`] instead.
    pub fn remove_current(&self, session_id: &SessionId) -> bool {
        let removed = self.lock_current().swap_remove(session_id);
        let Some(removed) = removed else {
            return false;
        };
        removed.revoke();
        true
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod live_session_actor_registry_tests {
    use super::*;

    fn transient_turn_context_state() -> meerkat_core::TransientTurnContextStateHandle {
        meerkat_core::TransientTurnContextStateHandle::new()
    }

    #[test]
    fn duplicate_current_actor_is_rejected_without_replacement() {
        let registry = LiveSessionActorRegistry::default();
        let session_id = SessionId::new();
        let first_slot = LiveSessionActorWitnessSlot::default();
        let first = registry
            .insert_and_publish(
                &first_slot,
                session_id.clone(),
                transient_turn_context_state(),
            )
            .expect("first actor should register");
        let duplicate_slot = LiveSessionActorWitnessSlot::default();

        let error = registry
            .insert_and_publish(
                &duplicate_slot,
                session_id.clone(),
                transient_turn_context_state(),
            )
            .expect_err("a current actor must not be replaced implicitly");

        assert!(error.to_string().contains("already registered"));
        assert_eq!(registry.current(&session_id), Some(first.clone()));
        assert_eq!(first_slot.witness(), Some(first.clone()));
        assert!(duplicate_slot.witness().is_none());
        assert!(first.is_live());
    }

    #[test]
    fn stale_compare_remove_cannot_remove_or_revoke_replacement() {
        let registry = LiveSessionActorRegistry::default();
        let session_id = SessionId::new();
        let actor_a = registry
            .insert_and_publish(
                &LiveSessionActorWitnessSlot::default(),
                session_id.clone(),
                transient_turn_context_state(),
            )
            .expect("actor A should register");
        assert!(registry.remove_current(&session_id));
        assert!(!actor_a.is_live());

        let actor_b = registry
            .insert_and_publish(
                &LiveSessionActorWitnessSlot::default(),
                session_id.clone(),
                transient_turn_context_state(),
            )
            .expect("actor B should register");

        assert!(!registry.compare_remove(&actor_a));
        assert_eq!(registry.current(&session_id), Some(actor_b.clone()));
        assert!(actor_b.is_live());
    }

    #[test]
    fn exact_current_actor_remove_revokes_and_clears_registration() {
        let registry = LiveSessionActorRegistry::default();
        let session_id = SessionId::new();
        let actor_b = registry
            .insert_and_publish(
                &LiveSessionActorWitnessSlot::default(),
                session_id.clone(),
                transient_turn_context_state(),
            )
            .expect("actor B should register");

        assert!(registry.compare_remove(&actor_b));
        assert!(!actor_b.is_live());
        assert!(!registry.contains(&session_id));
        assert!(registry.current(&session_id).is_none());
    }
}

/// Internal result of one admitted session turn.
///
/// The public turn result intentionally preserves the original `AgentError`.
/// Runtime-backed callers additionally consume the exact machine-terminal
/// witness produced by the concrete agent after its current run was admitted.
#[derive(Debug)]
pub(crate) struct SessionTurnExecutionOutcome {
    pub(crate) result: Result<RunResult, meerkat_core::error::AgentError>,
    pub(crate) machine_terminal_failure:
        Result<Option<meerkat_core::TurnErrorMetadata>, meerkat_core::error::AgentError>,
}

impl SessionTurnExecutionOutcome {
    fn without_machine_terminal(
        result: Result<RunResult, meerkat_core::error::AgentError>,
    ) -> Self {
        Self {
            result,
            machine_terminal_failure: Ok(None),
        }
    }

    pub(crate) fn into_runtime_parts(
        self,
    ) -> Result<
        (
            Result<RunResult, meerkat_core::error::AgentError>,
            Option<meerkat_core::TurnErrorMetadata>,
        ),
        meerkat_core::error::AgentError,
    > {
        let machine_terminal_failure = self.machine_terminal_failure?;
        if self.result.is_ok() && machine_terminal_failure.is_some() {
            return Err(meerkat_core::error::AgentError::InternalError(
                "runtime turn returned success with a machine-terminal failure witness".to_string(),
            ));
        }
        Ok((self.result, machine_terminal_failure))
    }

    fn into_public_result(self) -> Result<RunResult, meerkat_core::error::AgentError> {
        let (result, _machine_terminal_failure) = self.into_runtime_parts()?;
        result
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Commands sent from the service to a session task.
enum SessionCommand {
    /// Execute through the already-materialized agent without committing the
    /// bridge request or result to its canonical Session document.
    StartLiveBridgeOperation {
        request: LiveBridgeSessionOperationRequest,
        cancellation: tokio::sync::watch::Receiver<bool>,
        accepted_tx: oneshot::Sender<
            Result<LiveBridgeSessionOperationTerminalReceiver, meerkat_core::error::AgentError>,
        >,
    },
    /// Side-effect-free eligibility preflight for the exact live member.
    ValidateLiveBridgeMemberEligibility {
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    StartTurn {
        prompt: meerkat_core::types::ContentInput,
        /// Ordinary ordered System messages appended at this admitted turn
        /// boundary, immediately before the turn-authored transcript rows.
        system_messages: Vec<String>,
        /// Host-attached injected context materialized as typed
        /// injected-context user messages before the turn's user message.
        injected_context: Vec<meerkat_core::types::ContentInput>,
        runtime: Box<meerkat_core::service::StartTurnRuntimeSemantics>,
        event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
        result_tx: oneshot::Sender<SessionTurnExecutionOutcome>,
        active_admission: Option<RuntimeContextAdmissionGuard>,
    },
    ReplaceClient {
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    HotSwapLlmIdentity {
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        // Boxed: identity + request policy are large inline values (typed
        // provider-params carrier); keep command variants size-balanced.
        identity: Box<SessionLlmIdentity>,
        request_policy: Box<meerkat_core::SessionLlmRequestPolicy>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    StageToolFilter {
        filter: meerkat_core::ToolFilter,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    SetToolVisibilityState {
        // Boxed: the visibility state carries full ToolName-keyed catalogs.
        state: Option<Box<meerkat_core::SessionToolVisibilityState>>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    SyncSessionFromDurableSnapshot {
        // Boxed: a full Session is by far the largest payload on this channel.
        session: Box<meerkat_core::Session>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    /// Export the full session (messages + metadata) for persistence.
    ExportSession {
        reply_tx: oneshot::Sender<Result<meerkat_core::Session, AgentError>>,
    },
    /// Observe only exact transcript authority; never export the full body.
    ObserveSessionTranscriptAuthority {
        reply_tx: oneshot::Sender<Result<SessionTranscriptAuthoritySnapshot, AgentError>>,
    },
    /// Export only if the actor still has the exact observed transcript
    /// generation and authority.
    ExportSessionIfTranscriptAuthority {
        expected: SessionTranscriptAuthoritySnapshot,
        reply_tx: oneshot::Sender<Result<Option<meerkat_core::Session>, AgentError>>,
    },
    /// Classify a callback-result batch against actor-owned canonical
    /// transcript state without exporting that state across the command seam.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    ClassifyCallbackResultIngress {
        results: Vec<ToolResult>,
        reply_tx: oneshot::Sender<
            Result<meerkat_core::session::CallbackResultIngress, AgentError>,
        >,
    },
    ReconcileRuntimeCompactionProjections {
        intents: Vec<meerkat_core::CompactionProjectionIntent>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    AbortUncommittedCompactionProjections {
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    ExecutionSnapshot {
        reply_tx: oneshot::Sender<
            Result<Option<meerkat_core::AgentExecutionSnapshot>, SnapshotProjectionError>,
        >,
    },
    ToolScopeSnapshot {
        reply_tx: oneshot::Sender<Option<meerkat_core::ToolScopeSnapshot>>,
    },
    VisibleToolDefs {
        reply_tx: oneshot::Sender<Vec<meerkat_core::ToolDef>>,
    },
    ExternalToolSurfaceSnapshot {
        reply_tx: oneshot::Sender<Option<meerkat_core::ExternalToolSurfaceSnapshot>>,
    },
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    PublishInteractionTerminalsExactBatch {
        expected_actor: Option<LiveSessionActorWitness>,
        events: Vec<AgentEvent>,
        event_store: Arc<dyn crate::event_store::EventStore>,
        reply_tx: oneshot::Sender<
            Result<
                Vec<
                    meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt,
                >,
                SessionError,
            >,
        >,
    },
    RecordLiveTerminalError {
        cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
        reply_tx: oneshot::Sender<()>,
    },
    RecordLiveOutputAudioDegraded {
        dropped: u64,
        reply_tx: oneshot::Sender<()>,
    },
    AppendExternalUserContent {
        content: ContentInput,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    AppendExternalAssistantOutput {
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: Usage,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    AppendRealtimeTranscriptEvent {
        event: RealtimeTranscriptEvent,
        reply_tx: oneshot::Sender<
            Result<RealtimeTranscriptApplyOutcome, meerkat_core::error::AgentError>,
        >,
    },
    AdmitLiveAssistantPlaybackTarget {
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        reply_tx: oneshot::Sender<
            Result<meerkat_core::LiveAssistantPlaybackTarget, meerkat_core::error::AgentError>,
        >,
    },
    ResolveLiveAssistantPlaybackTarget {
        channel_id: meerkat_core::LiveChannelId,
        item_id: String,
        content_index: u32,
        reply_tx: oneshot::Sender<Option<meerkat_core::LiveAssistantPlaybackTarget>>,
    },
    ResolveLiveAssistantPlaybackOnChannelClose {
        channel_id: meerkat_core::LiveChannelId,
        reply_tx: oneshot::Sender<
            Result<
                Option<meerkat_core::LiveAssistantPlaybackTruncationEvidence>,
                meerkat_core::error::AgentError,
            >,
        >,
    },
    CommitLiveUserTranscriptFinal {
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: Option<RealtimeTranscriptEvent>,
        reply_tx: oneshot::Sender<
            Result<
                meerkat_core::FinalLiveUserTranscriptCommitEvidence,
                meerkat_core::error::AgentError,
            >,
        >,
    },
    CommitLiveAssistantPlaybackTruncation {
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        evidence: meerkat_core::LiveAssistantPlaybackEvidence,
        reply_tx: oneshot::Sender<
            Result<
                meerkat_core::LiveAssistantPlaybackTruncationEvidence,
                meerkat_core::error::AgentError,
            >,
        >,
    },
    CommitLiveAssistantPlaybackComplete {
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        stop_reason: meerkat_core::StopReason,
        usage: meerkat_core::TurnUsage,
        reply_tx: oneshot::Sender<
            Result<
                meerkat_core::LiveAssistantPlaybackTruncationEvidence,
                meerkat_core::error::AgentError,
            >,
        >,
    },
    ObserveLiveAssistantPlaybackTerminal {
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        evidence: meerkat_core::LiveAssistantPlaybackEvidence,
        stop_reason: meerkat_core::StopReason,
        usage: meerkat_core::TurnUsage,
        reply_tx: oneshot::Sender<
            Result<
                crate::live_transcript_authority::LiveAssistantPlaybackObservationResult,
                meerkat_core::error::AgentError,
            >,
        >,
    },
    ObserveLiveAssistantPlaybackFinal {
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        reply_tx: oneshot::Sender<
            Result<
                Option<meerkat_core::LiveAssistantPlaybackTruncationEvidence>,
                meerkat_core::error::AgentError,
            >,
        >,
    },
    DispatchExternalToolCall {
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
        reply_tx: oneshot::Sender<
            Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError>,
        >,
    },
    UpdateMobToolAuthority {
        authority_context: Option<MobToolAuthorityContext>,
        reply_tx: oneshot::Sender<Result<(), meerkat_core::error::AgentError>>,
    },
    AppendSystemMessageControl {
        req: AppendSystemContextRequest,
        reply_tx: oneshot::Sender<
            Result<meerkat_core::service::AppendSystemContextStatus, AgentError>,
        >,
    },
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    ActivateInstructionControl {
        request: meerkat_core::InstructionActivationRequest,
        reply_tx: oneshot::Sender<
            Result<
                Result<
                    meerkat_core::InstructionActivationMutation,
                    meerkat_core::InstructionActivationError,
                >,
                AgentError,
            >,
        >,
    },
    PrepareHeadCanonicalRuntimeBoundary {
        request: HeadCanonicalRuntimeBoundaryPrepareRequest,
        reply_tx: oneshot::Sender<Result<PreparedHeadCanonicalRuntimeBoundary, AgentError>>,
    },
    AcknowledgeHeadCanonicalRuntimeBoundary {
        successor_head_token: String,
        reply_tx:
            oneshot::Sender<Result<HeadCanonicalRuntimeBoundaryAcknowledgeOutcome, AgentError>>,
    },
    Shutdown,
}

impl SessionCommand {
    /// Conservatively advance actor-owned transcript generation for every
    /// command except the three body/authority observations themselves.
    ///
    /// Over-advancing on a read-only diagnostic is safe (a later conditional
    /// export retries). The default-mutating shape is intentional: a newly
    /// added command cannot silently create an ABA hole by forgetting to opt
    /// into generation advancement.
    fn advances_transcript_authority_generation(&self) -> bool {
        !matches!(
            self,
            Self::StartLiveBridgeOperation { .. }
                | Self::ValidateLiveBridgeMemberEligibility { .. }
                | Self::ExportSession { .. }
                | Self::ObserveSessionTranscriptAuthority { .. }
                | Self::ExportSessionIfTranscriptAuthority { .. }
        )
    }
}

/// Lightweight summary updated after each turn, readable without querying the task.
#[derive(Clone)]
struct SessionSummaryCache {
    updated_at: SystemTime,
    message_count: usize,
    total_tokens: u64,
    usage: Usage,
    last_assistant_text: Option<String>,
}

/// Handle stored in the sessions map.
struct SessionHandle {
    /// Exact incarnation identity for this registry entry.  Logical-session
    /// recovery creates a new witness even when it reuses the same SessionId.
    actor_witness: LiveSessionActorWitness,
    command_tx: mpsc::Sender<SessionCommand>,
    state_tx: watch::Sender<SessionState>,
    state_rx: watch::Receiver<SessionState>,
    summary_rx: watch::Receiver<SessionSummaryCache>,
    llm_identity_rx: watch::Receiver<SessionLlmIdentity>,
    /// Mutexed shell around generated session turn-admission authority.
    turn_admission: Arc<std::sync::Mutex<TurnAdmissionSlot>>,
    created_at: SystemTime,
    /// Key-value labels attached at session creation.
    labels: BTreeMap<String, String>,
    /// Public event injector for pushing external events.
    /// Extracted from the agent before it moves into its task.
    event_injector: Option<Arc<dyn meerkat_core::EventInjector>>,
    /// Internal runtime injector for interaction-scoped streaming.
    interaction_event_injector: Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>>,
    /// Optional comms runtime for keep-alive commands and stream attachment.
    comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    /// Session-attributed peer-send surface with post-commit observation.
    observed_comms_sender: Option<Arc<meerkat_core::ObservedCommsSender>>,
    /// Shared runtime control state for system-context appends.
    transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    /// Mechanical gate closed when an archive snapshot is taken.
    archive_snapshot_gate: Arc<ArchiveSnapshotGate>,
    /// Runtime-owned turn phase handle for active-boundary probes.
    turn_state_handle: Option<Arc<dyn TurnStateHandle>>,
    /// Shared control state for deferred first-turn prompt and staged tool results.
    deferred_turn_state: Arc<std::sync::Mutex<SessionDeferredTurnState>>,
    /// Capacity currently held by live work for this session. Additional
    /// runtime-routed inputs for the same running session join this lease
    /// instead of consuming another global slot.
    active_capacity_lease: Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
    /// Wakes the running turn loop when an interrupt is requested.
    interrupt_notify: Arc<tokio::sync::Notify>,
    /// Typed command sender for cancel-after-boundary requests; the agent
    /// task owns the receiver and drains it at the next boundary.
    cancel_after_boundary_handle: Option<CancelAfterBoundarySender>,
    /// Broadcast channel for session-wide event subscription.
    session_event_tx: tokio::sync::broadcast::Sender<Arc<EventEnvelope<AgentEvent>>>,
    /// Owned-event broadcast reserved for the public synchronous raw receiver.
    /// Normal streams use the shared-envelope lane above, so large payloads
    /// are cloned only while a raw receiver is actually attached.
    raw_session_event_tx: tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
    /// Singular unbounded queue reserved for an installed persistent event
    /// projector. UI subscribers stay on the lossy broadcast lane, so UI lag
    /// cannot drop projector input.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    lossless_event_projection_tx: Arc<tokio::sync::Mutex<Option<Arc<LosslessEventProjectionSink>>>>,
}

/// Cancellation-safe custody of one generated turn-admission claim.
///
/// Public turns claim the canonical slot before awaiting the outer runtime
/// finalization boundary. That preserves the public `SESSION_BUSY` contract
/// instead of turning concurrent turns into a serialized queue. If the caller
/// is cancelled, loses the actor incarnation, or fails before command
/// delivery, dropping this guard returns `Admitted -> Idle` through the same
/// generated authority. Successful command delivery transfers finalization to
/// the session task.
struct StartTurnAdmissionClaim {
    actor_witness: LiveSessionActorWitness,
    validated_identity: Option<SessionLlmIdentity>,
    turn_admission: Arc<std::sync::Mutex<TurnAdmissionSlot>>,
    state_tx: watch::Sender<SessionState>,
    transferred_to_session_task: bool,
}

impl StartTurnAdmissionClaim {
    fn claim(
        id: &SessionId,
        handle: &SessionHandle,
        validated_identity: Option<SessionLlmIdentity>,
    ) -> Result<Self, SessionError> {
        let projection = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            match slot
                .claim()
                .map_err(|_| SessionError::Busy { id: id.clone() })?
            {
                ClaimOutcome::Admitted => slot.projection(),
                // ShuttingDown means the session was archived or discarded;
                // the public contract for callers is NotFound (archived).
                ClaimOutcome::ShutdownTerminal(_) => {
                    return Err(SessionError::NotFound { id: id.clone() });
                }
            }
        };
        handle.state_tx.send_replace(projection);
        Ok(Self {
            actor_witness: handle.actor_witness.clone(),
            validated_identity,
            turn_admission: Arc::clone(&handle.turn_admission),
            state_tx: handle.state_tx.clone(),
            transferred_to_session_task: false,
        })
    }

    fn belongs_to(&self, handle: &SessionHandle) -> bool {
        self.actor_witness.is_handle(handle)
    }

    fn requires_prompt_validation(&self, identity: &SessionLlmIdentity) -> bool {
        self.validated_identity.as_ref() != Some(identity)
    }

    fn transfer_to_session_task(mut self) {
        self.transferred_to_session_task = true;
    }
}

impl Drop for StartTurnAdmissionClaim {
    fn drop(&mut self) {
        if self.transferred_to_session_task {
            return;
        }
        let projection = {
            let mut slot = lock_turn_admission(&self.turn_admission);
            slot.abort_claim().ok().map(|_| slot.projection())
        };
        if let Some(projection) = projection {
            self.state_tx.send_replace(projection);
        }
    }
}

struct ArchiveSnapshotGate {
    closed: AtomicBool,
    apply_lock: std::sync::Mutex<()>,
}

impl ArchiveSnapshotGate {
    fn open() -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false),
            apply_lock: std::sync::Mutex::new(()),
        })
    }

    fn enter_apply(&self) -> Result<std::sync::MutexGuard<'_, ()>, SessionError> {
        let guard = self
            .apply_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) {
            Err(session_archive_snapshot_taken_error())
        } else {
            Ok(guard)
        }
    }

    fn close_for_snapshot(&self) {
        let _guard = self
            .apply_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.closed.store(true, Ordering::Release);
    }
}

fn session_archive_snapshot_taken_error() -> SessionError {
    SessionError::FailedWithData {
        message: "session archive snapshot already taken; runtime system context rejected"
            .to_string(),
        data: serde_json::json!({
            "reason": "archive_snapshot_taken",
        }),
    }
}

pub struct RuntimeContextAdmissionGuard {
    active_capacity_lease: Option<Arc<std::sync::Mutex<SessionActiveCapacityLease>>>,
    active_permit: Option<OwnedSemaphorePermit>,
}

#[derive(Default)]
struct SessionActiveCapacityLease {
    permit: Option<OwnedSemaphorePermit>,
    leases: usize,
    /// In-flight staged→active promotion bound to this lease. Settled at
    /// final release through the registry-owned materialization status: an
    /// uncommitted promotion restores the staged reservation, a committed one
    /// lets the permit drop back to the gate. There are no shell-side
    /// restore flags — the registry status *is* the verdict.
    promotion: Option<PromotionTicket>,
}

#[derive(Default)]
struct ActiveCapacityLeaseRelease {
    permit: Option<OwnedSemaphorePermit>,
    promotion: Option<PromotionTicket>,
}

impl ActiveCapacityLeaseRelease {
    /// Settle a final lease release: an in-flight promotion is resolved
    /// through the registry (restore staged vs release capacity); otherwise
    /// the permit drops and capacity returns to the gate.
    fn settle(self) {
        match self.promotion {
            Some(ticket) => ticket.settle(self.permit),
            None => drop(self.permit),
        }
    }
}

impl Drop for RuntimeContextAdmissionGuard {
    fn drop(&mut self) {
        if let Some(active_capacity_lease) = self.active_capacity_lease.take() {
            release_active_capacity_lease(&active_capacity_lease).settle();
        }
        // A capacity-only admission (`active_permit`) drops with the guard,
        // returning the permit to the gate.
    }
}

impl RuntimeContextAdmissionGuard {
    /// Commit an in-flight staged→active promotion: the run is genuinely
    /// beginning, so the registry-owned status flips `Promoting -> Active`
    /// and the final lease release returns capacity to the gate instead of
    /// restoring the staged reservation. No-op for admissions that did not
    /// originate from a staged reservation.
    fn commit_promotion(&self) {
        let Some(active_capacity_lease) = self.active_capacity_lease.as_ref() else {
            return;
        };
        let ticket = lock_active_capacity_lease(active_capacity_lease)
            .promotion
            .take();
        if let Some(ticket) = ticket {
            ticket.commit();
        }
    }

    pub(crate) fn into_create_session_permit(mut self) -> Option<OwnedSemaphorePermit> {
        if let Some(active_capacity_lease) = self.active_capacity_lease.take() {
            let released = release_active_capacity_lease(&active_capacity_lease);
            // The capacity unit is being repurposed for a new session, so an
            // in-flight promotion can no longer be restored: commit it through
            // the registry before handing the permit over.
            if let Some(ticket) = released.promotion {
                ticket.commit();
            }
            return released.permit;
        }
        self.active_permit.take()
    }
}

struct SessionTaskControl {
    // Browser sessions retain the exact witness in the shared task-control
    // shape, but exact durable terminal publication is native-only.
    #[cfg_attr(
        any(target_arch = "wasm32", not(feature = "session-store")),
        allow(dead_code)
    )]
    actor_witness: LiveSessionActorWitness,
    state_tx: watch::Sender<SessionState>,
    summary_tx: watch::Sender<SessionSummaryCache>,
    llm_identity_tx: watch::Sender<SessionLlmIdentity>,
    turn_admission: Arc<std::sync::Mutex<TurnAdmissionSlot>>,
    interrupt_notify: Arc<tokio::sync::Notify>,
    session_event_tx: tokio::sync::broadcast::Sender<Arc<EventEnvelope<AgentEvent>>>,
    raw_session_event_tx: tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>,
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    lossless_event_projection_tx: Arc<tokio::sync::Mutex<Option<Arc<LosslessEventProjectionSink>>>>,
    archive_snapshot_gate: Arc<ArchiveSnapshotGate>,
    /// Session-context DSL handle (W2-E / issue #264). `None` on standalone
    /// / ephemeral builds that have no runtime-backed DSL authority. The
    /// session task's `publish_summary` helper fires
    /// `AdvanceSessionContext` through this handle after every canonical
    /// session-truth mutation so the realtime projection consumer observes
    /// a typed `SessionContextAdvanced` effect.
    session_context: Option<Arc<dyn meerkat_core::handles::SessionContextHandle>>,
}

impl SessionTaskControl {
    /// Publish first to the singular installed-projector lane, then fan out to
    /// best-effort broadcast subscribers. The dedicated queue absorbs bursts
    /// without letting a slow projector perturb live session ordering or lose
    /// its input through a UI-oriented broadcast ring. EventStore append
    /// remains asynchronous derived state rather than session authority.
    async fn publish_session_event(&self, envelope: EventEnvelope<AgentEvent>) {
        #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
        publish_session_event_to_channels(
            &self.lossless_event_projection_tx,
            &self.session_event_tx,
            &self.raw_session_event_tx,
            envelope,
        )
        .await;
        #[cfg(not(all(feature = "session-store", not(target_arch = "wasm32"))))]
        publish_session_event_to_broadcasts(
            &self.session_event_tx,
            &self.raw_session_event_tx,
            Arc::new(envelope),
        );
    }

    fn advance_session_context_at(&self, observed_at: SystemTime, reason: &'static str) {
        let Some(handle) = self.session_context.as_ref() else {
            return;
        };
        let observed_ms = summary_updated_at_ms(observed_at);
        let current_ms = handle.current_watermark_ms();
        let updated_at_ms = observed_ms.max(current_ms.saturating_add(1));
        if let Err(err) = handle.context_advanced(updated_at_ms) {
            tracing::debug!(
                error = %err,
                reason,
                "AdvanceSessionContext rejected by DSL; projection refresh will rely on later ticks"
            );
        }
    }

    /// Update the summary watch channel and fire the DSL
    /// `AdvanceSessionContext` input so the realtime projection consumer
    /// receives a typed `SessionContextAdvanced` effect (W2-E).
    ///
    /// Monotonic: the DSL guard filters out non-advancing ticks, so a
    /// single call site at every mutation is correct even if two sites
    /// fire back-to-back without an intervening advance.
    fn publish_summary(&self, snapshot: SessionSummaryCache) {
        let updated_at = snapshot.updated_at;
        self.summary_tx.send_replace(snapshot);
        self.advance_session_context_at(updated_at, "summary");
    }
}

/// Convert a `SystemTime` summary timestamp into monotonic milliseconds
/// for the DSL's `AdvanceSessionContext` input. `UNIX_EPOCH`-relative.
/// Saturates to `u64::MAX` on dates far in the future (unreachable under
/// any realistic clock).
///
/// Uses [`meerkat_core::time_compat::UNIX_EPOCH`] so the same `SystemTime`
/// alias applies on both native and wasm32 (which uses `web_time`).
fn summary_updated_at_ms(updated_at: SystemTime) -> u64 {
    updated_at
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Agent abstraction
// ---------------------------------------------------------------------------

/// Trait for building agents from session creation requests.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionAgentBuilder: Send + Sync {
    /// The concrete agent type.
    #[cfg(not(target_arch = "wasm32"))]
    type Agent: SessionAgent + Send + 'static;
    #[cfg(target_arch = "wasm32")]
    type Agent: SessionAgent + 'static;

    /// Resolve whether the selected model accepts inline video user content.
    ///
    /// Implementations with a richer configured model registry can override
    /// this; the default uses the canonical model capability catalog.
    async fn model_supports_inline_video(&self, identity: &SessionLlmIdentity) -> Option<bool> {
        meerkat_models::inline_video_support_for(identity.provider, &identity.model)
    }

    /// Build an agent for a new session.
    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError>;

    /// Reconcile durable compaction stages for an exact session before its
    /// live [`SessionAgent`] has been materialized.
    ///
    /// The runtime calls this seam only when its authoritative compaction
    /// outbox is empty. An empty outbox is still semantic authority: every
    /// durable stage for `session_id` is an uncommitted crash/cancellation
    /// orphan and must be aborted. The builder owns this pre-materialization
    /// boundary because it owns construction-time access to the canonical
    /// memory backend. Generic builders fail closed until they explicitly
    /// provide that backend-owned reconciliation.
    async fn abort_absent_session_compaction_stages(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(format!(
            "session builder cannot reconcile durable compaction stages before session {session_id} is materialized"
        )))
    }
}

/// Trait abstracting over the agent's run/cancel interface.
pub struct SessionAgentTurnInput {
    pub prompt: meerkat_core::types::ContentInput,
    /// Host-attached injected context for this turn. Each entry materializes
    /// as a separate typed injected-context user message immediately before
    /// the turn's user message. Must be empty when `typed_turn_appends` is
    /// non-empty (the runtime authored the whole transcript for the turn).
    pub injected_context: Vec<meerkat_core::types::ContentInput>,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub render_metadata: Option<meerkat_core::types::RenderMetadata>,
    pub typed_turn_appends: Vec<meerkat_core::lifecycle::run_primitive::ConversationAppend>,
    pub transcript_identity: Option<meerkat_core::types::TranscriptMessageIdentity>,
    pub execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
}

/// Exact noncommitting input for one already-bound durable session agent.
///
/// The snapshot is the same clone whose revision was sealed before machine
/// admission. `dispatch_admission` is process-local and reaches the actual
/// member dispatcher through [`meerkat_core::ToolDispatchContext`].
#[derive(Clone)]
pub struct LiveBridgeSessionOperationRequest {
    pub operation_id: Arc<str>,
    pub snapshot: meerkat_core::Session,
    pub semantic_request: meerkat_core::types::ContentInput,
    pub dispatch_admission: meerkat_core::LiveBridgeToolDispatchAdmission,
    pub run_permit: meerkat_core::LiveBridgeNoncommittingRunPermit,
}

impl std::fmt::Debug for LiveBridgeSessionOperationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveBridgeSessionOperationRequest")
            .field("operation_id", &"[REDACTED]")
            .field("session_id", &"[REDACTED]")
            .field("semantic_request", &"[REDACTED]")
            .field("dispatch_admission", &self.dispatch_admission)
            .field("run_permit", &self.run_permit)
            .finish()
    }
}

pub type LiveBridgeSessionOperationTerminalReceiver =
    oneshot::Receiver<Result<RunResult, meerkat_core::error::AgentError>>;

pub type LiveBridgePreparedSessionOperation = meerkat_core::LiveBridgePreparedOperation;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionAgent: Send {
    /// Validate that this exact live member supports the experimental bridge
    /// before any live channel or provider transport is opened.
    fn validate_live_bridge_member_eligibility(
        &self,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "live bridge execution is unsupported by this session agent".to_string(),
        ))
    }

    /// Validate support and exact live-session identity before the session
    /// actor transfers custody to the accepted operation.
    fn validate_live_bridge_operation(
        &self,
        _request: &LiveBridgeSessionOperationRequest,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "live bridge execution is unsupported by this session agent".to_string(),
        ))
    }

    /// Mint operation-local execution material from this exact actor-owned
    /// member before acceptance. The returned future must not borrow or mutate
    /// this SessionAgent, allowing ordinary actor commands to proceed while it
    /// runs.
    fn prepare_live_bridge_operation(
        &self,
        _request: LiveBridgeSessionOperationRequest,
        _cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> Result<LiveBridgePreparedSessionOperation, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "live bridge execution is unsupported by this session agent".to_string(),
        ))
    }

    /// Run the agent with the given prompt, streaming events.
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError>;

    /// Reconcile/finalize compaction memory stages authorized by the exact
    /// RuntimeStore atomic outbox. Standalone agents have no such authority.
    async fn reconcile_runtime_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), meerkat_core::error::AgentError> {
        if intents.is_empty() {
            Ok(())
        } else {
            Err(meerkat_core::error::AgentError::ConfigError(
                "runtime compaction projection reconciliation is unsupported by this session agent"
                    .to_string(),
            ))
        }
    }

    /// Await a supervised sticky-fallback control transaction whose original
    /// run future may have been dropped by a hard interrupt. Runtime-backed
    /// agents publish or compensate the retained saga before the session task
    /// reports the turn result; agents without sticky fallback state are a
    /// no-op.
    async fn settle_inflight_sticky_model_fallback(
        &mut self,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    /// Abort any invisible compaction-memory stage owned by a run future that
    /// was dropped by a hard interrupt before the runtime could atomically
    /// commit its session snapshot and outbox intent.
    ///
    /// SessionTask invokes this only for the hard-interrupt path. Successful
    /// runs must retain their stage until RuntimeStore::atomic_apply commits the
    /// paired transcript rewrite.
    async fn abort_uncommitted_compaction_projections(
        &mut self,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    /// Consume the exact current-run machine-terminal failure witness, if the
    /// concrete agent produced one. Implementations must not infer this from a
    /// post-only snapshot; `Ok(None)` is the safe default for agents without
    /// generated turn-machine authority.
    fn take_runtime_terminal_failure_witness(
        &mut self,
    ) -> Result<Option<meerkat_core::TurnErrorMetadata>, meerkat_core::error::AgentError> {
        Ok(None)
    }

    /// Run the next turn.
    ///
    /// `handling_mode` and `render_metadata` are runtime-owned semantics:
    /// the runtime routes Queue/Steer BEFORE calling the executor, so by
    /// the time this method runs the routing decision is already made.
    /// These parameters are present on the trait because the session task
    /// forwards them from `StartTurnRequest`, but implementations should
    /// not act on them — the runtime is the canonical owner.
    ///
    /// The default rejects non-Queue handling_mode and non-None render_metadata
    /// to prevent silent flattening on paths that can't honor them (§5).
    async fn run_turn_with_events(
        &mut self,
        input: SessionAgentTurnInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        if input.handling_mode != meerkat_core::types::HandlingMode::Queue {
            return Err(meerkat_core::error::AgentError::ConfigError(format!(
                "handling_mode {:?} requires a runtime-backed surface",
                input.handling_mode,
            )));
        }
        if input.render_metadata.is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "render_metadata requires a runtime-backed surface".to_string(),
            ));
        }
        if !input.typed_turn_appends.is_empty() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "typed turn appends require a runtime-backed surface".to_string(),
            ));
        }
        if input.transcript_identity.is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "transcript identity requires a runtime-backed surface".to_string(),
            ));
        }
        if !input.injected_context.is_empty() {
            // The plain run entry cannot materialize typed injected-context
            // messages; silently folding them into the prompt would launder
            // the typed role away (§5). Fail closed.
            return Err(meerkat_core::error::AgentError::ConfigError(
                "injected context is not supported by this session agent".to_string(),
            ));
        }
        self.run_with_events(input.prompt, event_tx).await
    }

    /// Continue from the existing session transcript without pushing a new user
    /// message. Used for deferred first-turn prompts and staged callback
    /// tool-result continuations.
    async fn run_pending_with_events(
        &mut self,
        transcript_identity: Option<meerkat_core::types::TranscriptMessageIdentity>,
        _execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        if transcript_identity.is_some() {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "transcript identity requires a runtime-backed surface".to_string(),
            ));
        }
        Err(meerkat_core::error::AgentError::ConfigError(
            "run_pending_with_events is not supported by this session agent".to_string(),
        ))
    }

    /// Stage skill references to resolve and inject on the next turn.
    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>);

    /// Apply or clear a per-turn tool overlay.
    fn set_turn_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError>;

    /// Apply staged callback tool results before the next continuation turn.
    fn apply_pending_tool_results(
        &mut self,
        results: Vec<meerkat_core::ToolResult>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        if results.is_empty() {
            return Ok(());
        }
        Err(meerkat_core::error::AgentError::ConfigError(
            "staged tool-result continuations are not supported by this session agent".to_string(),
        ))
    }

    /// Replace the LLM client for subsequent turns after proving that its
    /// concrete wire can represent the existing canonical transcript.
    fn replace_client(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "live client replacement is not supported by this session agent".to_string(),
        ))
    }

    /// Atomically update the live client and the session's durable LLM identity.
    fn hot_swap_llm_identity(
        &mut self,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError>;

    /// Stage an external tool visibility filter for subsequent turns.
    fn stage_external_tool_filter(
        &mut self,
        _filter: meerkat_core::ToolFilter,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    /// Replace the canonical tool visibility state carried by the live session.
    fn set_tool_visibility_state(
        &mut self,
        _state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "tool visibility updates are not supported by this session agent".to_string(),
        ))
    }

    /// Dispatch an external tool call through the session's canonical dispatcher.
    async fn dispatch_external_tool_call(
        &mut self,
        _call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "external live tool dispatch is not supported by this session agent".to_string(),
        ))
    }

    /// Dispatch an external tool call with a caller-specific timeout policy.
    async fn dispatch_external_tool_call_with_timeout_policy(
        &mut self,
        call: meerkat_core::ToolCall,
        _timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::AgentError> {
        self.dispatch_external_tool_call(call).await
    }

    /// Cancel the currently running turn.
    fn cancel(&mut self);

    /// Typed command sender for cancel-after-boundary requests.
    ///
    /// Implementations expose a supported exact-cancellation capability only
    /// when this sender is paired with [`Self::turn_state_handle`]. A sender
    /// alone cannot prove which run owns a queued command.
    fn cancel_after_boundary_handle(&self) -> Option<CancelAfterBoundarySender> {
        None
    }

    /// Runtime-owned turn-state handle, when the agent was built through a
    /// machine-backed runtime binding.
    fn turn_state_handle(&self) -> Option<Arc<dyn TurnStateHandle>> {
        None
    }

    /// Session-context advancement handle (W2-E / issue #264).
    ///
    /// Runtime-backed agents expose the session's DSL handle here. The
    /// session task fires `context_advanced(updated_at_ms)` on every
    /// `summary_tx.send_replace` so the realtime projection consumer
    /// observes canonical session-truth advancement as a typed effect
    /// instead of polling a watch channel.
    ///
    /// Standalone agents (WASM, ephemeral tests) return `None`; the task
    /// then skips the emit and the typed effect simply never fires —
    /// which is correct, there is nothing to refresh on those paths.
    fn session_context_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::SessionContextHandle>> {
        None
    }

    /// Get the session ID.
    fn session_id(&self) -> SessionId;

    /// Take a snapshot of the current session state.
    fn snapshot(&self) -> SessionSnapshot;

    /// Take a diagnostic snapshot of the live execution state, if supported.
    fn execution_snapshot(
        &self,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SnapshotProjectionError> {
        Ok(None)
    }

    /// Take a diagnostic snapshot of the live tool-scope state, if supported.
    fn tool_scope_snapshot(&self) -> Option<meerkat_core::ToolScopeSnapshot> {
        None
    }

    /// Snapshot the canonical visible tool definitions for the live session.
    fn visible_tool_defs(&self) -> Vec<meerkat_core::ToolDef> {
        Vec::new()
    }

    /// Take a diagnostic snapshot of the live external tool-surface state, if supported.
    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        None
    }

    /// Clone the full session (messages + metadata) for persistence.
    ///
    /// This is more expensive than `snapshot()` because it includes the
    /// full message history. Only called by `PersistentSessionService`
    /// after each turn.
    fn session_clone(&self) -> Result<meerkat_core::Session, AgentError>;

    /// Observe the live transcript's exact bounded authority without exporting
    /// or cloning its accumulated document.
    fn session_transcript_authority(
        &self,
    ) -> Result<SessionTranscriptAuthoritySnapshot, AgentError>;

    /// Classify callback-result ingress against the actor-owned canonical
    /// Session. Implementations with direct session access should override
    /// this; the default remains exact for lightweight/test agents.
    fn classify_callback_result_ingress(
        &self,
        incoming: &[ToolResult],
    ) -> Result<meerkat_core::session::CallbackResultIngress, AgentError> {
        self.session_clone()?
            .classify_callback_result_ingress(incoming)
    }

    /// Return the durable LLM identity authored by the concrete agent builder.
    ///
    /// Factory-backed agents resolve this through the same model registry path
    /// that constructs the runtime client and persisted session metadata. Mock
    /// or legacy agents without durable metadata may return `None`.
    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        None
    }

    /// Observe the session transcript tail.
    ///
    /// This is deliberately a typed observation, not a pending-boundary
    /// decision. Generated turn-admission authority owns which tail kinds can
    /// admit pending continuation.
    fn observed_session_tail(&self) -> ObservedSessionTailKind;

    /// Update the `keep_alive` flag in the session's durable metadata.
    ///
    /// Called by the session task when the runtime overrides keep-alive on a
    /// live session. This ensures subsequent inheriting calls observe the
    /// updated value.
    fn update_keep_alive(&mut self, _keep_alive: bool) {}

    /// Update the canonical mob operator authority persisted on the session.
    fn update_mob_tool_authority_context(
        &mut self,
        _authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "mob tool authority updates are not supported by this session agent".to_string(),
        ))
    }

    /// Append ordinary ordered System messages at the current transcript tail.
    ///
    fn append_system_messages(&mut self, _contents: Vec<String>) -> Result<(), AgentError> {
        Err(AgentError::ConfigError(
            "ordinary System-message append is not supported by this session agent".to_string(),
        ))
    }

    /// Append one ordinary ordered System message with explicit control identity.
    fn append_system_message_control(
        &mut self,
        _req: AppendSystemContextRequest,
    ) -> Result<meerkat_core::service::AppendSystemContextStatus, AgentError> {
        Err(AgentError::ConfigError(
            "ordinary System-message control append is not supported by this session agent"
                .to_string(),
        ))
    }

    /// Append one typed chronological instruction activation to the actor-owned
    /// canonical Session document.
    fn activate_instruction_control(
        &mut self,
        _request: meerkat_core::InstructionActivationRequest,
    ) -> Result<meerkat_core::InstructionActivationMutation, meerkat_core::InstructionActivationError>
    {
        Err(meerkat_core::InstructionActivationError::Unsupported(
            "this session agent does not expose the canonical Session document".to_string(),
        ))
    }

    async fn prepare_head_canonical_runtime_boundary(
        &mut self,
        _request: HeadCanonicalRuntimeBoundaryPrepareRequest,
    ) -> Result<PreparedHeadCanonicalRuntimeBoundary, AgentError> {
        Err(AgentError::ConfigError(
            "HeadCanonical boundary preparation is not supported by this session agent".to_string(),
        ))
    }

    fn acknowledge_head_canonical_runtime_boundary(
        &mut self,
        _successor_head_token: &str,
    ) -> Result<HeadCanonicalRuntimeBoundaryAcknowledgeOutcome, AgentError> {
        Err(AgentError::ConfigError(
            "HeadCanonical boundary acknowledgement is not supported by this session agent"
                .to_string(),
        ))
    }

    /// Append externally-produced user content into the canonical transcript.
    fn append_external_user_content(
        &mut self,
        _content: ContentInput,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "external user content append is not supported by this session agent".to_string(),
        ))
    }

    /// Append externally-produced assistant output into the canonical transcript.
    fn append_external_assistant_output(
        &mut self,
        _blocks: Vec<meerkat_core::types::AssistantBlock>,
        _stop_reason: meerkat_core::types::StopReason,
        _usage: Usage,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "external assistant output append is not supported by this session agent".to_string(),
        ))
    }

    /// Apply an identity-bearing provider realtime transcript event.
    fn append_realtime_transcript_event(
        &mut self,
        _event: RealtimeTranscriptEvent,
    ) -> Result<RealtimeTranscriptApplyOutcome, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "realtime transcript append is not supported by this session agent".to_string(),
        ))
    }

    /// Observe one exact unmaterialized spoken segment before generated
    /// playback-prefix authority decides whether canonical truncation is legal.
    fn staged_realtime_assistant_segment_text(
        &self,
        _response_id: &str,
        _item_id: &str,
        _content_index: u32,
    ) -> Option<String> {
        None
    }

    fn staged_realtime_assistant_segment_is_final(
        &self,
        _response_id: &str,
        _item_id: &str,
        _content_index: u32,
    ) -> bool {
        false
    }

    /// Admit the exact foreground assistant playback target once.
    fn admit_live_assistant_playback_target(
        &mut self,
        _channel_id: &meerkat_core::LiveChannelId,
        _interaction_id: meerkat_core::InteractionId,
        _response_id: &str,
        _item_id: &str,
        _content_index: u32,
    ) -> Result<meerkat_core::LiveAssistantPlaybackTarget, meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "live assistant playback target admission is not supported by this session agent"
                .to_string(),
        ))
    }

    /// Resolve the exact durable target for a terminal playback report.
    fn live_assistant_playback_target(
        &self,
        _channel_id: &meerkat_core::LiveChannelId,
        _item_id: &str,
        _content_index: u32,
    ) -> Option<meerkat_core::LiveAssistantPlaybackTarget> {
        None
    }

    fn live_assistant_playback_target_for_channel(
        &self,
        _channel_id: &meerkat_core::LiveChannelId,
    ) -> Option<meerkat_core::LiveAssistantPlaybackTarget> {
        None
    }

    fn resolve_live_assistant_playback_target(
        &mut self,
        _channel_id: &meerkat_core::LiveChannelId,
        _interaction_id: meerkat_core::InteractionId,
        _response_id: &str,
        _item_id: &str,
        _content_index: u32,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::ConfigError(
            "live assistant playback target resolution is not supported by this session agent"
                .to_string(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_live_assistant_playback_terminal(
        &mut self,
        channel_id: &meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: &str,
        item_id: &str,
        content_index: u32,
        evidence: meerkat_core::LiveAssistantPlaybackEvidence,
        stop_reason: meerkat_core::StopReason,
        usage: meerkat_core::TurnUsage,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantPlaybackTerminalObserved {
                channel_id: channel_id.to_string(),
                interaction_id,
                response_id: response_id.to_string(),
                item_id: item_id.to_string(),
                content_index,
                evidence,
                stop_reason,
                usage,
            },
        )
        .map(|_| ())
    }

    /// Get request-only state for exact active-turn transient context.
    fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle;

    /// Replace live semantic session state from durable authority while preserving live mechanics.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn sync_session_from_durable_snapshot(
        &mut self,
        _session: meerkat_core::Session,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Err(meerkat_core::error::AgentError::DurableSnapshotSyncUnsupported)
    }

    /// Get an event injector for pushing external events.
    ///
    /// Called once before the agent moves into its dedicated task. The returned
    /// injector is stored in the session handle for surfaces to access.
    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        None
    }

    /// Internal runtime injector for interaction-scoped streaming.
    #[doc(hidden)]
    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        None
    }

    /// Get the comms runtime used by this agent, if any.
    ///
    /// Called once before the agent moves into its dedicated task. The returned
    /// runtime is stored in the session handle for surfaces that need comms
    /// command execution and stream attachment.
    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        None
    }

    fn observed_comms_sender(&self) -> Option<Arc<meerkat_core::ObservedCommsSender>> {
        None
    }
}

#[cfg(test)]
fn validate_prompt_video_input_against_capability(
    prompt: &ContentInput,
    identity: &SessionLlmIdentity,
    supports_inline_video: bool,
) -> Result<(), SessionError> {
    let blocks = match prompt {
        ContentInput::Text(_) => return Ok(()),
        ContentInput::Blocks(blocks) => blocks,
    };

    meerkat_core::validate_inline_video_blocks(blocks)
        .map_err(|err| SessionError::Agent(AgentError::ConfigError(err)))?;

    if meerkat_core::has_video(blocks) && !supports_inline_video {
        return Err(SessionError::Agent(AgentError::ConfigError(format!(
            "inline video input is not supported by model '{}' on provider '{}'",
            identity.model,
            identity.provider.as_str()
        ))));
    }

    Ok(())
}

fn wake_interrupt_notify(notify: &tokio::sync::Notify) {
    notify.notify_waiters();
    notify.notify_one();
}

// ---------------------------------------------------------------------------
// EphemeralSessionService
// ---------------------------------------------------------------------------

/// In-memory session service with no persistence.
///
/// Sessions are kept alive as tokio tasks. All state is lost on process exit.
pub struct EphemeralSessionService<B: SessionAgentBuilder> {
    sessions: RwLock<IndexMap<SessionId, SessionHandle>>,
    archived_views: RwLock<IndexMap<SessionId, SessionView>>,
    /// Stable outer boundary for overlapping turns and live identity/tool
    /// mutations of one logical session. Weak entries keep the same mutex
    /// across every current holder and waiter without retaining rejected or
    /// retired session IDs forever.
    turn_finalization_gates: Mutex<HashMap<SessionId, std::sync::Weak<Mutex<()>>>>,
    builder: B,
    /// Single typed owner of session materialization status, staged-capacity
    /// custody, and the global active-capacity admission seam.
    ///
    /// The registry holds the canonical materialization status per session
    /// (`Staged`/`Promoting`/`Active`), CUSTODY of each staged session's
    /// reserved capacity permit, and the global concurrency gate behind one
    /// lock, so staged-ness is never re-derived from permit-existence in a
    /// handle-side slot. The staged→active promotion verdict
    /// (`begin_promotion`) is single-winner and owned here. The embedded
    /// semaphore remains a pure RESOURCE GATE: it bounds how many sessions
    /// hold live work at once and never decides whether a session is a
    /// valid/admitted authority — that decision is owned by runtime/machine
    /// admission.
    staged_registry: Arc<StagedSessionRegistry>,
    /// Notified when a new session handle is stored. Used by CLI --stdin
    /// to avoid polling for the session to appear.
    session_registered: tokio::sync::Notify,
}

impl<B: SessionAgentBuilder + 'static> EphemeralSessionService<B> {
    /// Deliver a hard interrupt to one exact live run.
    ///
    /// The turn-admission slot serializes this comparison with turn
    /// completion and successor admission. A stale request therefore resolves
    /// as `false` and can never set the ambient successor's interrupt bit.
    pub async fn interrupt_run_if_current(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
    ) -> Result<bool, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let woke = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            let active_run_id = handle
                .turn_state_handle
                .as_deref()
                .and_then(|turn_state| turn_state.snapshot().active_run_id);
            if active_run_id.as_ref() != Some(expected_run_id) {
                return Ok(false);
            }
            slot.request_interrupt()
                .map_err(|_| SessionError::NotRunning { id: id.clone() })?
        };
        if woke {
            wake_interrupt_notify(&handle.interrupt_notify);
        }
        Ok(true)
    }

    /// Deliver cooperative cancellation to one exact live run.
    ///
    /// Machine-owned boundary handles must use this run-scoped seam. The
    /// command itself retains the witness, so a turn transition between this
    /// validation and agent observation cannot redirect the request to a
    /// successor run.
    pub async fn cancel_after_boundary_for_run(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let Some(cancel_after_boundary_handle) = handle.cancel_after_boundary_handle.as_ref()
        else {
            return Err(SessionError::Unsupported(
                "cancel_after_boundary".to_string(),
            ));
        };
        let Some(turn_state_handle) = handle.turn_state_handle.as_deref() else {
            return Err(SessionError::Unsupported(
                "cancel_after_boundary_exact_run_authority".to_string(),
            ));
        };
        let current_run_id = turn_state_handle.snapshot().active_run_id;
        if current_run_id.as_ref() != Some(expected_run_id) {
            return Err(SessionError::NotRunning { id: id.clone() });
        }

        {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.authorize_cancel_after_boundary()
                .map_err(|_| SessionError::NotRunning { id: id.clone() })?;
            // The agent task owns the receiver. If the exact run already ended,
            // send failure is benign; a delivered stale command is rejected by
            // the run-id check in Agent::observe_cancel_after_boundary_request.
            let _ = cancel_after_boundary_handle
                .send(CancelAfterBoundaryCommand::for_run(expected_run_id.clone()));
        }
        wake_interrupt_notify(&handle.interrupt_notify);
        Ok(())
    }

    fn build_runtime_receipt(
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        session: &meerkat_core::Session,
    ) -> Result<RunBoundaryReceiptDraft, SessionError> {
        // Served from the session's retained SHA-256 midstate, so an ordinary
        // append costs O(delta). Re-serializing every message and hashing the
        // result cost a full pass over the document on every turn, which is
        // what made a one-word reply on a large transcript scale with the
        // transcript instead of with the reply.
        //
        // This digest is minted and checked inside one commit — the boundary
        // validator only ever sees a receipt produced moments earlier against
        // the snapshot it was produced from — so the format is an internal
        // agreement, not a persisted wire contract.
        let digest = session.transcript_content_digest().map_err(|err| {
            SessionError::Agent(AgentError::InternalError(format!(
                "failed to digest session transcript for runtime receipt: {err}"
            )))
        })?;

        // Dogma K10: the boundary sequence is machine-owned; the session
        // service returns an UNSEQUENCED draft and the runtime driver mints
        // the final receipt from the generated per-run boundary counter.
        Ok(RunBoundaryReceiptDraft {
            run_id,
            boundary,
            contributing_input_ids,
            conversation_digest: Some(digest),
            message_count: session.messages().len(),
        })
    }

    fn callback_pending_terminal(error: &SessionError) -> Option<CoreApplyTerminal> {
        match error {
            SessionError::Agent(AgentError::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            }) => Some(CoreApplyTerminal::CallbackPending {
                tool_use_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            }),
            SessionError::Agent(AgentError::CallbackBatchPending { pending_tool_calls }) => {
                Some(CoreApplyTerminal::CallbackBatchPending {
                    pending_tool_calls: pending_tool_calls.clone(),
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
        let session = self.export_session(id).await?;
        let receipt =
            Self::build_runtime_receipt(run_id, boundary, contributing_input_ids, &session)?;

        // Hand the boundary the session we already have, so nothing
        // downstream deserializes these bytes back into it. `with_session`
        // performs the single serialization and seals the typed session to
        // the exact bytes it produced.
        CoreApplyOutput::new(receipt, terminal)
            .with_session(std::sync::Arc::new(session))
            .map_err(|err| {
                SessionError::Agent(AgentError::InternalError(format!(
                    "failed to serialize session snapshot for runtime commit: {err}"
                )))
            })
    }

    async fn require_inline_video_support(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<(), meerkat_core::UnsupportedModelCapabilityEvidence> {
        match self.builder.model_supports_inline_video(identity).await {
            Some(true) => Ok(()),
            Some(false) => Err(
                meerkat_core::UnsupportedModelCapabilityEvidence::inline_video(
                    identity.provider,
                    identity.model.clone(),
                    meerkat_core::UnsupportedModelCapabilityReason::CapabilityDisabled,
                ),
            ),
            None => Err(
                meerkat_core::UnsupportedModelCapabilityEvidence::inline_video(
                    identity.provider,
                    identity.model.clone(),
                    meerkat_core::UnsupportedModelCapabilityReason::ProviderModelProfileMissing,
                ),
            ),
        }
    }

    fn missing_durable_llm_identity_error(context: &str) -> SessionError {
        SessionError::Agent(AgentError::ConfigError(format!(
            "{context} requires durable LLM identity from the session agent"
        )))
    }

    async fn validate_prompt_video_input(
        &self,
        prompt: &ContentInput,
        identity: &SessionLlmIdentity,
    ) -> Result<(), SessionError> {
        let blocks = match prompt {
            ContentInput::Text(_) => return Ok(()),
            ContentInput::Blocks(blocks) => blocks,
        };

        meerkat_core::validate_inline_video_blocks(blocks)
            .map_err(|err| SessionError::Agent(AgentError::ConfigError(err)))?;

        if meerkat_core::has_video(blocks)
            && let Err(evidence) = self.require_inline_video_support(identity).await
        {
            return Err(SessionError::Agent(AgentError::ConfigError(
                evidence.to_string(),
            )));
        }

        Ok(())
    }

    fn validate_tool_result_video(results: &[ToolResult]) -> Result<(), SessionError> {
        if results.iter().any(ToolResult::has_video) {
            return Err(SessionError::Agent(AgentError::ConfigError(
                "video blocks are not supported in tool results".to_string(),
            )));
        }
        Ok(())
    }

    /// Create a new ephemeral session service.
    pub fn new(builder: B, max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(IndexMap::new()),
            archived_views: RwLock::new(IndexMap::new()),
            turn_finalization_gates: Mutex::new(HashMap::new()),
            builder,
            staged_registry: Arc::new(StagedSessionRegistry::bounded(max_sessions)),
            session_registered: tokio::sync::Notify::new(),
        }
    }

    async fn turn_finalization_gate_for_session(&self, id: &SessionId) -> Arc<Mutex<()>> {
        let mut gates = self.turn_finalization_gates.lock().await;
        gates.retain(|_, gate| gate.strong_count() != 0);
        if let Some(gate) = gates.get(id).and_then(std::sync::Weak::upgrade) {
            return gate;
        }

        let gate = Arc::new(Mutex::new(()));
        gates.insert(id.clone(), Arc::downgrade(&gate));
        gate
    }

    /// Acquire the exact per-session outer boundary shared by direct turns,
    /// runtime turns, and the multi-command LLM reconfigure transaction.
    pub async fn acquire_runtime_turn_finalization_guard(
        &self,
        id: &SessionId,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        self.turn_finalization_gate_for_session(id)
            .await
            .lock_owned()
            .await
    }

    /// Acquire a global active-capacity permit through the single typed
    /// admission seam.
    ///
    /// This is a resource gate (concurrency limit), not lifecycle authority:
    /// success means there is spare capacity, failure means the global active
    /// ceiling is reached. It never decides whether a session is a valid
    /// admitted authority — that decision is owned by runtime/machine
    /// admission. The permit is reserved inside the
    /// [`StagedSessionRegistry`] lock so capacity accounting can never
    /// interleave with a concurrent status transition.
    fn try_acquire_active_permit(&self) -> Result<Option<OwnedSemaphorePermit>, SessionError> {
        self.staged_registry.reserve_capacity()
    }

    fn acquire_runtime_context_admission_for_handle(
        &self,
        id: &SessionId,
        handle: &SessionHandle,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        // The staged branch is gated by the registry-owned single-winner
        // promotion verdict, never by permit presence: `begin_promotion`
        // flips `Staged -> Promoting` and hands over custody of the staged
        // capacity permit. A loser (already promoting/active) falls through
        // to the non-staged admission paths.
        if let Some(promotion) = self.staged_registry.begin_promotion(id) {
            let pending_first_turn = {
                let state = lock_deferred_turn_state(&handle.deferred_turn_state);
                matches!(state.first_turn_phase(), DeferredFirstTurnPhase::Pending)
            };
            let ticket = if pending_first_turn {
                // The deferred first turn is still pending: keep the
                // promotion in flight so a pre-run failure settles back to
                // `Staged` (reservation restored) while a genuine run-begin
                // commits it to `Active`.
                Some(PromotionTicket::new(
                    Arc::clone(&self.staged_registry),
                    id.clone(),
                ))
            } else {
                // No deferred first turn left to protect: commit immediately
                // so a pre-run failure releases capacity instead of
                // re-staging it for a reservation that no longer exists.
                self.staged_registry.complete_promotion(id);
                None
            };
            return Ok(acquire_active_capacity_lease(
                Arc::clone(&handle.active_capacity_lease),
                promotion.permit,
                ticket,
            ));
        }
        if let Some(admission) =
            try_join_active_capacity_lease(Arc::clone(&handle.active_capacity_lease))
        {
            return Ok(admission);
        }
        match self.staged_registry.reserve(id) {
            Ok(outcome) => Ok(acquire_active_capacity_lease(
                Arc::clone(&handle.active_capacity_lease),
                outcome.permit,
                None,
            )),
            Err(err) => {
                if let Some(admission) =
                    try_join_active_capacity_lease(Arc::clone(&handle.active_capacity_lease))
                {
                    Ok(admission)
                } else {
                    Err(err)
                }
            }
        }
    }

    pub fn ensure_active_capacity_available(&self) -> Result<(), SessionError> {
        self.staged_registry.ensure_capacity_available()
    }

    /// Current typed materialization status for a session as owned by the
    /// admission registry. Exposed for tests that assert the singular status
    /// fact is recorded/cleared with the session lifecycle.
    #[cfg(test)]
    fn materialization_status(&self, id: &SessionId) -> Option<MaterializationStatus> {
        self.staged_registry.status(id)
    }

    fn archived_view_from_handle(id: &SessionId, handle: &SessionHandle) -> SessionView {
        let cache = handle.summary_rx.borrow();
        let llm_identity = handle.llm_identity_rx.borrow().clone();
        SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: handle.created_at,
                updated_at: cache.updated_at,
                message_count: cache.message_count,
                is_active: false,
                model: llm_identity.model,
                provider: llm_identity.provider,
                last_assistant_text: cache.last_assistant_text.clone(),
                labels: handle.labels.clone(),
            },
            billing: SessionUsage {
                total_tokens: cache.total_tokens,
                usage: cache.usage.clone(),
            },
        }
    }

    /// Mechanical registry witness for the live session actor.
    ///
    /// Unlike `SessionService::has_live_session`, this deliberately performs
    /// no actor RPC or durable-authority arbitration, so an in-flight turn
    /// cannot stall host-health observation.
    pub async fn live_session_actor_registered(&self, id: &SessionId) -> bool {
        self.sessions
            .read()
            .await
            .get(id)
            .is_some_and(|handle| !handle.command_tx.is_closed())
    }

    /// Capture the exact currently registered live actor incarnation.
    ///
    /// This is a registry observation only.  Persistent/runtime-backed
    /// callers that need the observation to remain stable must use the
    /// persistent service's turn-boundary lease rather than retaining this
    /// clone by itself.
    pub async fn live_session_actor_witness(
        &self,
        id: &SessionId,
    ) -> Option<LiveSessionActorWitness> {
        self.sessions.read().await.get(id).and_then(|handle| {
            (!handle.command_tx.is_closed()).then(|| handle.actor_witness.clone())
        })
    }

    /// Validate the exact current member bridge policy and isolated-client
    /// capability without consuming operation authority or performing I/O.
    pub async fn validate_live_bridge_member_eligibility(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(id)
                .filter(|handle| !handle.command_tx.is_closed())
                .map(|handle| handle.command_tx.clone())
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ValidateLiveBridgeMemberEligibility { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task exited before live bridge eligibility preflight".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped live bridge eligibility preflight".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Transfer one noncommitting bridge operation to the exact live session
    /// actor. A successful return is the acceptance boundary; all later
    /// outcomes arrive through the terminal receiver and are not retryable as
    /// a fresh invocation.
    pub async fn start_live_bridge_operation(
        &self,
        id: &SessionId,
        request: LiveBridgeSessionOperationRequest,
        cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> Result<LiveBridgeSessionOperationTerminalReceiver, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(id)
                .filter(|handle| !handle.command_tx.is_closed())
                .map(|handle| handle.command_tx.clone())
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
        };
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::StartLiveBridgeOperation {
                request,
                cancellation,
                accepted_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task exited before live bridge custody transfer".to_string(),
                ))
            })?;
        accepted_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped live bridge acceptance".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Reserve runtime capacity for one exact actor incarnation.
    ///
    /// Unlike the persisted-session compatibility path, this never falls
    /// back to a capacity-only permit.  A removed, exited, or replaced actor
    /// is a typed `NotFound`, so the caller cannot carry resource capacity as
    /// if it were actor authority.
    pub async fn acquire_runtime_context_admission_for_actor(
        &self,
        witness: &LiveSessionActorWitness,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(witness.session_id())
            .filter(|handle| witness.is_handle(handle) && !handle.command_tx.is_closed())
            .ok_or_else(|| SessionError::NotFound {
                id: witness.session_id().clone(),
            })?;
        self.acquire_runtime_context_admission_for_handle(witness.session_id(), handle)
    }

    /// Export the full session (messages + metadata) for persistence.
    ///
    /// Returns the complete `Session` including message history. Used by
    /// `PersistentSessionService` to save full snapshots after each turn.
    pub async fn export_session(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::Session, SessionError> {
        let (command_tx, deferred_turn_state) = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            (
                handle.command_tx.clone(),
                Arc::clone(&handle.deferred_turn_state),
            )
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExportSession { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        let mut session = reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)?;

        let state = lock_deferred_turn_state(&deferred_turn_state).clone();
        session.set_deferred_turn_state(state).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize deferred-turn state: {err}"
            )))
        })?;

        Ok(session)
    }

    /// Observe exact actor-owned transcript authority without cloning the
    /// accumulated Session document.
    pub async fn observe_session_transcript_authority(
        &self,
        id: &SessionId,
    ) -> Result<LiveSessionTranscriptAuthoritySnapshot, SessionError> {
        let (actor_witness, command_tx) = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            (handle.actor_witness.clone(), handle.command_tx.clone())
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ObserveSessionTranscriptAuthority { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        let authority = reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the transcript-authority reply".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)?;
        if !actor_witness.is_live() {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(LiveSessionTranscriptAuthoritySnapshot {
            actor_witness,
            authority,
        })
    }

    /// Export the live actor only when its transcript authority and mutation
    /// generation still equal a prior bounded observation. The comparison and
    /// clone execute in one SessionTask command, closing the observation/export
    /// race without parking or hashing another full document.
    pub async fn export_session_if_transcript_authority(
        &self,
        id: &SessionId,
        expected: LiveSessionTranscriptAuthoritySnapshot,
    ) -> Result<Option<meerkat_core::Session>, SessionError> {
        if expected.session_id() != id {
            return Err(SessionError::Agent(AgentError::InternalError(format!(
                "transcript-authority export for {id} received snapshot for {}",
                expected.session_id()
            ))));
        }
        let (command_tx, deferred_turn_state) = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .filter(|handle| expected.actor_witness.is_handle(handle));
            let Some(handle) = handle else {
                return Ok(None);
            };
            (
                handle.command_tx.clone(),
                Arc::clone(&handle.deferred_turn_state),
            )
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExportSessionIfTranscriptAuthority {
                expected: expected.authority.clone(),
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        let Some(mut session) = reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the generation-bound export reply".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)?
        else {
            return Ok(None);
        };
        let still_current = self.sessions.read().await.get(id).is_some_and(|handle| {
            expected.actor_witness.is_handle(handle) && expected.actor_witness.is_live()
        });
        if !still_current {
            return Ok(None);
        }
        let state = lock_deferred_turn_state(&deferred_turn_state).clone();
        session.set_deferred_turn_state(state).map_err(|error| {
            SessionError::Agent(AgentError::InternalError(format!(
                "failed to serialize generation-bound deferred-turn state: {error}"
            )))
        })?;
        Ok(Some(session))
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn classify_callback_result_ingress(
        &self,
        id: &SessionId,
        results: Vec<ToolResult>,
    ) -> Result<meerkat_core::session::CallbackResultIngress, SessionError> {
        let command_tx = self
            .sessions
            .read()
            .await
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
            .command_tx
            .clone();
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ClassifyCallbackResultIngress { results, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Reconcile/finalize invisible compaction memory stages from the exact
    /// runtime atomic outbox.
    pub async fn reconcile_runtime_compaction_projections(
        &self,
        id: &SessionId,
        intents: Vec<meerkat_core::CompactionProjectionIntent>,
    ) -> Result<(), SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            match sessions.get(id) {
                Some(session) => session.command_tx.clone(),
                None if intents.is_empty() => {
                    // Runtime attachment may precede SessionTask
                    // materialization. The empty outbox must still reach the
                    // durable memory owner so crash-before-atomic-commit stages
                    // are aborted; declaring success here would orphan them.
                    return self
                        .builder
                        .abort_absent_session_compaction_stages(id)
                        .await;
                }
                None => return Err(SessionError::NotFound { id: id.clone() }),
            }
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ReconcileRuntimeCompactionProjections { intents, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task exited before compaction projection reconciliation".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped compaction projection reconciliation reply".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Abort the live compaction transaction after RuntimeStore rejected the
    /// boundary and its authoritative outbox was observed empty.
    pub async fn abort_uncommitted_compaction_projections(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
                .command_tx
                .clone()
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::AbortUncommittedCompactionProjections { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task exited before uncommitted compaction abort".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped uncommitted compaction abort reply".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::SetToolVisibilityState {
                state: state.map(Box::new),
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Apply one already-authorized live identity update while the caller
    /// holds this session's turn-finalization boundary.
    pub async fn apply_runtime_session_llm_identity_under_runtime_turn_boundary(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::HotSwapLlmIdentity {
                client,
                identity: Box::new(identity),
                request_policy: Box::new(request_policy),
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Apply already-authorized visibility state while the caller holds the
    /// same turn-finalization boundary as the identity update.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub async fn apply_runtime_session_tool_visibility_state_under_runtime_turn_boundary(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        self.set_session_tool_visibility_state(id, state).await
    }

    /// Get a diagnostic snapshot of the live execution state for a session.
    pub async fn execution_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExecutionSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(|e: SnapshotProjectionError| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    e.to_string(),
                ))
            })
    }

    /// Get a diagnostic snapshot of the live tool-scope state for a session.
    pub async fn tool_scope_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ToolScopeSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Get the canonical visible tool definitions for a live session.
    pub async fn live_visible_tool_defs(
        &self,
        id: &SessionId,
    ) -> Result<Vec<meerkat_core::ToolDef>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::VisibleToolDefs { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Get a diagnostic snapshot of the live external tool-surface state for a session.
    pub async fn external_tool_surface_snapshot(
        &self,
        id: &SessionId,
    ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ExternalToolSurfaceSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Dispatch an external tool call through the live session task.
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

    /// Dispatch an external tool call through the live session task with a
    /// caller-specific timeout policy.
    pub async fn dispatch_external_tool_call_with_timeout_policy(
        &self,
        id: &SessionId,
        call: meerkat_core::ToolCall,
        timeout_policy: meerkat_core::ToolDispatchTimeoutPolicy,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        let command_tx = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            handle.command_tx.clone()
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::DispatchExternalToolCall {
                call,
                timeout_policy,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Get shared deferred-turn control state for a session, if available.
    pub async fn deferred_turn_state(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<std::sync::Mutex<SessionDeferredTurnState>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|h| Arc::clone(&h.deferred_turn_state))
    }

    /// Drop a live session handle without archiving it.
    ///
    /// This is used when a runtime-backed turn mutates the in-memory session
    /// but fails to durably commit its boundary. Discarding the live handle
    /// forces subsequent access to recover from the last persisted snapshot.
    ///
    /// Idempotent: "the live handle is not present" is the desired end state,
    /// so discarding an already-discarded (or never-materialized) session is a
    /// success, not a `NotFound`. The runtime loop's `StopRuntimeExecutor`
    /// clean exit discards the live handle as it quiesces, so a teardown drain
    /// (`unregister_session`) that awaits that quiescence can race an explicit
    /// caller-side discard; both must converge on `Ok(())`. Every production
    /// caller already encodes this (`Ok(()) | Err(NotFound) => Ok(())`); this
    /// makes the contract itself idempotent.
    pub async fn discard_live_session(&self, id: &SessionId) -> Result<(), SessionError> {
        let handle = self.sessions.write().await.swap_remove(id);
        let Some(handle) = handle else {
            return Ok(());
        };
        self.shutdown_removed_live_session_handle(id, handle).await;
        Ok(())
    }

    /// Drop only the actor incarnation named by `witness`.
    ///
    /// Returns `false` when the actor is already absent or a replacement with
    /// the same logical SessionId is current.  In particular, stale cleanup
    /// can never remove the replacement.
    pub async fn discard_live_session_actor(
        &self,
        witness: &LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        let handle = {
            let mut sessions = self.sessions.write().await;
            if !sessions
                .get(witness.session_id())
                .is_some_and(|handle| witness.is_handle(handle))
            {
                return Ok(false);
            }
            sessions.swap_remove(witness.session_id())
        };
        let Some(handle) = handle else {
            return Ok(false);
        };
        self.shutdown_removed_live_session_handle(witness.session_id(), handle)
            .await;
        Ok(true)
    }

    async fn shutdown_removed_live_session_handle(&self, id: &SessionId, handle: SessionHandle) {
        // Revoke the exact incarnation synchronously before any shutdown await.
        // Prepared boundary authorities can now fail closed even while the
        // actor is still processing the shutdown command.
        handle.actor_witness.revoke();
        // Clear the singular typed materialization record. For a staged
        // session the registry-held capacity permit drops with it, so the
        // registry never retains a phantom record (or orphaned reservation)
        // for a session whose handle no longer exists.
        self.staged_registry.forget(id);
        handle.archive_snapshot_gate.close_for_snapshot();
        let projection = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.request_shutdown().ok().map(|_| slot.projection())
        };
        if let Some(projection) = projection {
            handle.state_tx.send_replace(projection);
        }
        let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
    }

    /// Prepare one exact active-turn model boundary and wait until its actor is
    /// parked immediately before consumption. The returned non-clone authority
    /// is tied to the exact registry actor allocation captured here; replacement
    /// or removal revokes it before any shutdown await.
    pub async fn prepare_transient_turn_context_for_active_turn(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
        contexts: Vec<TurnRequestContext>,
    ) -> Result<
        meerkat_core::PreparedTransientTurnContextBoundary,
        meerkat_core::CoreBoundaryStageError,
    > {
        let (actor_witness, state) = {
            let sessions = self.sessions.read().await;
            let handle = sessions.get(id).ok_or_else(|| {
                meerkat_core::CoreBoundaryStageError::stale(format!(
                    "session {id} has no live actor"
                ))
            })?;
            if handle.command_tx.is_closed() || !handle.actor_witness.is_live() {
                return Err(meerkat_core::CoreBoundaryStageError::stale(format!(
                    "session {id} actor is closed"
                )));
            }
            (
                handle.actor_witness.clone(),
                handle.transient_turn_context_state.clone(),
            )
        };

        let prepared = state
            .prepare_active_turn_boundary(expected_run_id, contexts)
            .await?;

        let still_exact = self.sessions.read().await.get(id).is_some_and(|handle| {
            actor_witness.is_handle(handle)
                && actor_witness.is_live()
                && !handle.command_tx.is_closed()
        });
        if !still_exact {
            drop(prepared);
            return Err(meerkat_core::CoreBoundaryStageError::stale(format!(
                "session {id} actor was replaced while preparing boundary"
            )));
        }
        Ok(prepared)
    }

    /// Publish an exact interaction-terminal batch only to the actor named by
    /// `witness`.
    ///
    /// The registry read lock is held only long enough to compare the sealed
    /// incarnation and clone that actor's command sender. An arbitrarily slow
    /// actor callback therefore cannot block removal or replacement, while a
    /// late predecessor callback can never resolve the logical SessionId to a
    /// successor actor.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn publish_interaction_terminals_exact_batch_for_actor(
        &self,
        witness: &LiveSessionActorWitness,
        events: Vec<AgentEvent>,
        event_store: Arc<dyn crate::event_store::EventStore>,
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        let command_tx = {
            let sessions = self.sessions.read().await;
            sessions
                .get(witness.session_id())
                .filter(|handle| {
                    witness.is_handle(handle) && witness.is_live() && !handle.command_tx.is_closed()
                })
                .map(|handle| handle.command_tx.clone())
                .ok_or_else(|| SessionError::NotFound {
                    id: witness.session_id().clone(),
                })?
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::PublishInteractionTerminalsExactBatch {
                expected_actor: Some(witness.clone()),
                events,
                event_store,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Exact session actor has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Exact session actor dropped interaction-terminal reply channel".to_string(),
            ))
        })?
    }

    /// Record a typed live-adapter terminal error onto the session's owned
    /// event stream.
    ///
    /// Routes the typed [`LiveAdapterErrorCode`] through the session task so it
    /// is stamped and broadcast as a terminal `RunFailed` event to session
    /// subscribers, instead of being laundered into a warning log + `Ok(())`.
    ///
    /// [`LiveAdapterErrorCode`]: meerkat_core::live_adapter::LiveAdapterErrorCode
    pub async fn record_live_terminal_error(
        &self,
        id: &SessionId,
        cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::RecordLiveTerminalError { cause, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Record a live transport output-audio delivery degradation (K16).
    ///
    /// Routes the dropped-packet count through the session task so it is
    /// stamped and broadcast as a typed `StreamTruncated` event
    /// (`OutputAudioDegraded`) to session subscribers, instead of remaining a
    /// transport-local counter with no live reader.
    pub async fn record_live_output_audio_degraded(
        &self,
        id: &SessionId,
        dropped: u64,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::RecordLiveOutputAudioDegraded { dropped, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped reply".to_string(),
            ))
        })
    }

    /// Append externally-produced user content into the canonical transcript.
    pub async fn append_external_user_content(
        &self,
        id: &SessionId,
        content: ContentInput,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AppendExternalUserContent { content, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Append externally-produced assistant output into the canonical transcript.
    pub async fn append_external_assistant_output(
        &self,
        id: &SessionId,
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: Usage,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AppendExternalAssistantOutput {
                blocks,
                stop_reason,
                usage,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Apply an identity-bearing provider realtime transcript event.
    pub async fn append_realtime_transcript_event(
        &self,
        id: &SessionId,
        event: RealtimeTranscriptEvent,
    ) -> Result<RealtimeTranscriptApplyOutcome, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AppendRealtimeTranscriptEvent { event, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Admit the exact foreground assistant target in the owning session
    /// actor. Replay returns the original interaction; no terminal caller can
    /// mint or replace it.
    pub async fn admit_live_assistant_playback_target(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
    ) -> Result<meerkat_core::LiveAssistantPlaybackTarget, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::AdmitLiveAssistantPlaybackTarget {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Resolve a previously admitted target. This read never creates an
    /// interaction identity.
    pub async fn live_assistant_playback_target(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
        item_id: String,
        content_index: u32,
    ) -> Result<Option<meerkat_core::LiveAssistantPlaybackTarget>, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ResolveLiveAssistantPlaybackTarget {
                channel_id,
                item_id,
                content_index,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Abandon the exact pending assistant output on channel close through
    /// SessionDocument's close-specific Unmeasured terminal authority.
    pub async fn resolve_live_assistant_playback_on_channel_close(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
    ) -> Result<Option<meerkat_core::LiveAssistantPlaybackTruncationEvidence>, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ResolveLiveAssistantPlaybackOnChannelClose {
                channel_id,
                reply_tx,
            })
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?;
        reply_rx
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?
            .map_err(SessionError::Agent)
    }

    /// Commit provider-final live user input through canonical transcript and
    /// SessionDocument authority, returning non-forgeable reconciliation
    /// evidence only after both steps succeed in the session actor.
    ///
    /// `None` is the explicit terminal missing-input observation. A present
    /// event must be `UserTranscriptFinal` for the exact provider user-turn
    /// correlation carried by `provisional`.
    pub async fn commit_live_user_transcript_final(
        &self,
        id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: Option<RealtimeTranscriptEvent>,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::CommitLiveUserTranscriptFinal {
                provisional,
                final_event,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    /// Apply an assistant playback-prefix observation only after generated
    /// SessionDocument authority classifies the exact interaction evidence.
    #[allow(clippy::too_many_arguments)]
    pub async fn commit_live_assistant_playback_truncation(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        evidence: meerkat_core::LiveAssistantPlaybackEvidence,
    ) -> Result<meerkat_core::LiveAssistantPlaybackTruncationEvidence, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::CommitLiveAssistantPlaybackTruncation {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                evidence,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit_live_assistant_playback_complete(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        stop_reason: meerkat_core::StopReason,
        usage: meerkat_core::TurnUsage,
    ) -> Result<meerkat_core::LiveAssistantPlaybackTruncationEvidence, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::CommitLiveAssistantPlaybackComplete {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                stop_reason,
                usage,
                reply_tx,
            })
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?;
        reply_rx
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?
            .map_err(SessionError::Agent)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn observe_live_assistant_playback_terminal(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
        evidence: meerkat_core::LiveAssistantPlaybackEvidence,
        stop_reason: meerkat_core::StopReason,
        usage: meerkat_core::TurnUsage,
    ) -> Result<
        crate::live_transcript_authority::LiveAssistantPlaybackObservationResult,
        SessionError,
    > {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ObserveLiveAssistantPlaybackTerminal {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                evidence,
                stop_reason,
                usage,
                reply_tx,
            })
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?;
        reply_rx
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?
            .map_err(SessionError::Agent)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn observe_live_assistant_playback_final(
        &self,
        id: &SessionId,
        channel_id: meerkat_core::LiveChannelId,
        interaction_id: meerkat_core::InteractionId,
        response_id: String,
        item_id: String,
        content_index: u32,
    ) -> Result<Option<meerkat_core::LiveAssistantPlaybackTruncationEvidence>, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ObserveLiveAssistantPlaybackFinal {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                reply_tx,
            })
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?;
        reply_rx
            .await
            .map_err(|_| SessionError::Agent(meerkat_core::error::AgentError::Cancelled))?
            .map_err(SessionError::Agent)
    }

    pub(crate) async fn append_system_message_control(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<meerkat_core::service::AppendSystemContextStatus, SessionError> {
        let command_tx = self
            .sessions
            .read()
            .await
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
            .command_tx
            .clone();
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::AppendSystemMessageControl { req, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn activate_instruction_control(
        &self,
        id: &SessionId,
        request: meerkat_core::InstructionActivationRequest,
    ) -> Result<meerkat_core::InstructionActivationMutation, SessionError> {
        let command_tx = self
            .sessions
            .read()
            .await
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
            .command_tx
            .clone();
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::ActivateInstructionControl { request, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)?
            .map_err(|error| SessionError::FailedWithData {
                message: error.to_string(),
                data: serde_json::json!({
                    "instruction_activation_code": error.code(),
                }),
            })
    }

    pub async fn prepare_head_canonical_runtime_boundary(
        &self,
        id: &SessionId,
        authority: HeadCanonicalRuntimeBoundaryAuthority,
        observed_head: Option<meerkat_core::session_store::SessionHead>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
        role: &str,
        deferred_turn_state_override: Option<SessionDeferredTurnState>,
    ) -> Result<PreparedHeadCanonicalRuntimeBoundary, SessionError> {
        let (command_tx, live_deferred_turn_state) = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            (
                handle.command_tx.clone(),
                lock_deferred_turn_state(&handle.deferred_turn_state).clone(),
            )
        };
        let projection_source = if deferred_turn_state_override.is_some() {
            HeadCanonicalDeferredProjectionSource::ExplicitOverride
        } else {
            HeadCanonicalDeferredProjectionSource::LiveActorState
        };
        let request = HeadCanonicalRuntimeBoundaryPrepareRequest::try_new(
            authority,
            observed_head,
            live_deferred_turn_state,
            projection_source,
            deferred_turn_state_override,
            role,
            blob_store,
        )
        .map_err(SessionError::Agent)?;
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::PrepareHeadCanonicalRuntimeBoundary { request, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    pub async fn acknowledge_head_canonical_runtime_boundary(
        &self,
        id: &SessionId,
        successor_head_token: String,
    ) -> Result<HeadCanonicalRuntimeBoundaryAcknowledgeOutcome, SessionError> {
        let command_tx = self
            .sessions
            .read()
            .await
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
            .command_tx
            .clone();
        let (reply_tx, reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::AcknowledgeHeadCanonicalRuntimeBoundary {
                successor_head_token,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(AgentError::InternalError(
                    "Session task dropped the reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn sync_session_from_durable_snapshot(
        &self,
        id: &SessionId,
        session: meerkat_core::Session,
    ) -> Result<(), SessionError> {
        if session.id() != id {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "durable snapshot session id {} does not match live session {id}",
                    session.id()
                )),
            ));
        }
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::SyncSessionFromDurableSnapshot {
                session: Box::new(session),
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    pub async fn apply_runtime_turn(
        &self,
        id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Self::require_runtime_execution_kind_stamp(&req)?;
        let execution = self.start_runtime_turn_execution(id, req).await?;
        let (result, machine_terminal_failure) =
            execution.into_runtime_parts().map_err(|error| {
                SessionError::runtime_executor_stopped(format!(
                    "runtime terminal-witness projection failed after live mutation: {error}"
                ))
            })?;
        match result.map_err(SessionError::Agent) {
            Ok(run_result) => {
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(CoreApplyTerminal::RunResult(Box::new(run_result))),
                )
                .await
            }
            Err(SessionError::Agent(meerkat_core::error::AgentError::NoPendingBoundary)) => {
                let terminal = self.resolve_no_pending_boundary_terminal(id).await?;
                self.build_runtime_output(
                    id,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    Some(terminal),
                )
                .await
            }
            Err(error) => {
                if let Some(error) = machine_terminal_failure {
                    self.build_runtime_output(
                        id,
                        run_id,
                        boundary,
                        contributing_input_ids,
                        Some(CoreApplyTerminal::MachineTerminalFailure { error }),
                    )
                    .await
                } else if let Some(terminal) = Self::callback_pending_terminal(&error) {
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

    pub(crate) async fn resolve_no_pending_boundary_terminal(
        &self,
        id: &SessionId,
    ) -> Result<CoreApplyTerminal, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let terminal = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.resolve_last_start_turn_public_terminal()
        }
        .map_err(|error| {
            SessionError::Agent(AgentError::InternalError(format!(
                "generated turn authority did not confirm NoPendingBoundary terminal: {error}"
            )))
        })?;
        match terminal {
            StartTurnPublicTerminal::NoPendingBoundary => Ok(CoreApplyTerminal::NoPendingBoundary),
        }
    }

    fn require_runtime_execution_kind_stamp(req: &StartTurnRequest) -> Result<(), SessionError> {
        if req
            .runtime
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

    pub async fn acquire_runtime_context_admission(
        &self,
        id: &SessionId,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        self.acquire_runtime_context_admission_for_handle(id, handle)
    }

    pub async fn join_active_runtime_context_admission(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeContextAdmissionGuard>, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        Ok(try_join_active_capacity_lease(Arc::clone(
            &handle.active_capacity_lease,
        )))
    }

    #[cfg(feature = "session-store")]
    pub(crate) async fn acquire_runtime_capacity_admission(
        &self,
    ) -> Result<RuntimeContextAdmissionGuard, SessionError> {
        let active_permit = self.try_acquire_active_permit()?;
        Ok(RuntimeContextAdmissionGuard {
            active_capacity_lease: None,
            active_permit,
        })
    }

    pub(crate) async fn start_runtime_turn_execution(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<SessionTurnExecutionOutcome, SessionError> {
        self.start_turn_execution_with_admission_recovering_not_found(id, req, None, None)
            .await
            .map_err(|(error, _admission)| error)
    }

    #[cfg(feature = "session-store")]
    pub(crate) async fn start_runtime_turn_execution_with_admission_recovering_not_found(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        admission: RuntimeContextAdmissionGuard,
    ) -> Result<SessionTurnExecutionOutcome, (SessionError, Option<RuntimeContextAdmissionGuard>)>
    {
        self.start_turn_execution_with_admission_recovering_not_found(
            id,
            req,
            Some(admission),
            None,
        )
        .await
    }

    async fn start_turn_with_admission(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        reserved_admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_with_admission_recovering_not_found(id, req, reserved_admission)
            .await
            .map_err(|(error, _admission)| error)
    }

    /// Execute a turn while the caller owns this session's stable outer
    /// turn-finalization boundary.
    pub async fn start_turn_under_runtime_turn_finalization_boundary(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_with_admission(id, req, None).await
    }

    async fn start_turn_with_admission_recovering_not_found(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        mut reserved_admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, (SessionError, Option<RuntimeContextAdmissionGuard>)> {
        let outcome = self
            .start_turn_execution_with_admission_recovering_not_found(
                id,
                req,
                reserved_admission.take(),
                None,
            )
            .await?;
        outcome
            .into_public_result()
            .map_err(|error| (SessionError::Agent(error), None))
    }

    async fn start_turn_execution_with_admission_recovering_not_found(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
        mut reserved_admission: Option<RuntimeContextAdmissionGuard>,
        mut preclaimed_turn: Option<StartTurnAdmissionClaim>,
    ) -> Result<SessionTurnExecutionOutcome, (SessionError, Option<RuntimeContextAdmissionGuard>)>
    {
        let (result_tx, result_rx) = oneshot::channel();

        let prompt: meerkat_core::types::ContentInput = req.prompt.clone();

        {
            let sessions = self.sessions.read().await;
            let handle = match sessions.get(id) {
                Some(handle) => handle,
                None => {
                    return Err((
                        SessionError::NotFound { id: id.clone() },
                        reserved_admission.take(),
                    ));
                }
            };
            if preclaimed_turn
                .as_ref()
                .is_some_and(|claim| !claim.belongs_to(handle))
            {
                return Err((
                    SessionError::NotFound { id: id.clone() },
                    reserved_admission.take(),
                ));
            }
            let identity = handle.llm_identity_rx.borrow().clone();
            if preclaimed_turn
                .as_ref()
                .is_none_or(|claim| claim.requires_prompt_validation(&identity))
            {
                self.validate_prompt_video_input(&prompt, &identity)
                    .await
                    .map_err(|error| (error, None))?;
            }

            // Atomic busy check via compare-and-swap. This is the single
            // point of admission — if two callers race, exactly one wins.
            let turn_claim = match preclaimed_turn.take() {
                Some(claim) => claim,
                None => Self::claim_start_turn(id, handle, Some(identity))
                    .map_err(|error| (error, None))?,
            };

            let mut system_messages = req
                .runtime
                .turn_metadata
                .as_ref()
                .map(|metadata| metadata.system_prompts.clone())
                .unwrap_or_default();
            // Runtime metadata is assembled by upstream ordered contributors;
            // the explicit StartTurn field is the final System append at this
            // same boundary. Preserve duplicates and exact content.
            system_messages.extend(req.system_prompt);

            let active_admission = if let Some(admission) = reserved_admission.take() {
                admission
            } else {
                match self.acquire_runtime_context_admission_for_handle(id, handle) {
                    Ok(admission) => admission,
                    Err(err) => {
                        return Err((err, None));
                    }
                }
            };

            let command = SessionCommand::StartTurn {
                prompt,
                system_messages,
                injected_context: req.injected_context,
                runtime: Box::new(req.runtime),
                event_tx: req.event_tx,
                result_tx,
                active_admission: Some(active_admission),
            };
            if handle.command_tx.send(command).await.is_err() {
                // Dropping the rejected command drops its admission guard; the
                // final lease release settles staged-restore vs capacity
                // release through the registry-owned promotion status. The
                // turn-claim guard separately restores Admitted -> Idle.
                return Err((
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        "Session task has exited".to_string(),
                    )),
                    None,
                ));
            }
            turn_claim.transfer_to_session_task();
        }

        let result = result_rx.await.map_err(|_| {
            (
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped the result channel".to_string(),
                )),
                None,
            )
        })?;

        Ok(result)
    }

    /// Get the event injector for a session, if available.
    ///
    /// Returns `None` if the session doesn't exist, has no comms runtime,
    /// or the comms runtime doesn't support event injection.
    pub async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.event_injector.clone())
    }

    #[doc(hidden)]
    pub async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.interaction_event_injector.clone())
    }

    /// Get shared system-context control state for a session, if available.
    pub async fn transient_turn_context_state(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_core::TransientTurnContextStateHandle> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|h| h.transient_turn_context_state.clone())
    }

    /// Get the current live durable LLM identity for a session.
    pub async fn live_session_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.clone(),
            })?;
        Ok(handle.llm_identity_rx.borrow().clone())
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.comms_runtime.clone())
    }

    pub async fn send_comms(
        &self,
        session_id: &SessionId,
        command: meerkat_core::CommsCommand,
    ) -> Option<Result<meerkat_core::SendReceipt, meerkat_core::SendError>> {
        let sender = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .and_then(|handle| handle.observed_comms_sender.clone())
        }?;
        Some(sender.send(command).await)
    }

    /// Wait for a session to be registered.
    ///
    /// Returns when the next session handle is stored. Used by CLI `--stdin`
    /// to wait for the session to become available before starting the stdin reader.
    pub async fn wait_session_registered(&self) {
        self.session_registered.notified().await;
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        let mut sessions = self.sessions.write().await;
        for (id, handle) in sessions.drain(..) {
            handle.actor_witness.revoke();
            // Clear the singular typed materialization record with the handle;
            // a staged session's registry-held permit drops with it.
            self.staged_registry.forget(&id);
            let projection = {
                let mut slot = lock_turn_admission(&handle.turn_admission);
                slot.request_shutdown().ok().map(|_| slot.projection())
            };
            if let Some(projection) = projection {
                handle.state_tx.send_replace(projection);
            }
            let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        }
    }

    /// Subscribe to session-wide events.
    ///
    /// This stream is available as soon as the session is registered and emits
    /// all agent events produced by the session task, regardless of which
    /// interaction triggered them. If this bounded subscriber falls behind,
    /// it emits an explicit `StreamTruncated(StreamLagged)` marker before
    /// continuing with retained events; it never silently skips the gap.
    pub async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| meerkat_core::comms::StreamError::NotFound(format!("session {id}")))?;
        let rx = handle.session_event_tx.subscribe();
        Ok(lag_aware_session_event_stream(id.clone(), rx))
    }

    /// Install the singular lossless event stream consumed by the persistent
    /// service's durable projector.
    ///
    /// This is deliberately not a general subscription API. Ordinary readers
    /// use the bounded broadcast stream above and receive typed truncation
    /// markers when they lag. The durable projector gets one dedicated MPSC
    /// queue so persistence never depends on broadcast retention and projector
    /// latency does not become part of live session execution ordering.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub(crate) async fn install_lossless_event_projection_stream(
        &self,
        id: &SessionId,
    ) -> Result<LosslessEventProjectionStream, meerkat_core::comms::StreamError> {
        let slot = {
            let sessions = self.sessions.read().await;
            let handle = sessions.get(id).ok_or_else(|| {
                meerkat_core::comms::StreamError::NotFound(format!("session {id}"))
            })?;
            Arc::clone(&handle.lossless_event_projection_tx)
        };

        let (tx, rx) = mpsc::unbounded_channel();
        let queued_events = Arc::new(AtomicUsize::new(0));
        let mut current = slot.lock().await;
        if current.as_ref().is_some_and(|sink| !sink.tx.is_closed()) {
            return Err(meerkat_core::comms::StreamError::Internal(format!(
                "lossless event projection stream already installed for session {id}"
            )));
        }
        *current = Some(Arc::new(LosslessEventProjectionSink::new(
            tx,
            queued_events,
        )));
        drop(current);

        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })))
    }

    /// Wait until the session summary reflects a mutation newer than `after`.
    ///
    /// This is intentionally summary-backed rather than agent-event-backed:
    /// runtime-owned context-only appends mutate canonical session state but do
    /// not necessarily produce user-visible agent events. Callers that need an
    /// authoritative "session changed" witness, such as realtime projection
    /// refresh, should follow this mutation boundary instead of the event lane.
    pub async fn wait_for_session_mutation_after(
        &self,
        id: &SessionId,
        after: SystemTime,
    ) -> Result<SystemTime, meerkat_core::comms::StreamError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| meerkat_core::comms::StreamError::NotFound(format!("session {id}")))?;
        let mut rx = handle.summary_rx.clone();
        drop(sessions);

        loop {
            let current = rx.borrow().updated_at;
            if current > after {
                return Ok(current);
            }
            rx.changed()
                .await
                .map_err(|_| meerkat_core::comms::StreamError::Closed)?;
        }
    }

    /// Get a raw broadcast receiver for a session's events.
    ///
    /// Unlike [`Self::subscribe_session_events`] which returns an `EventStream`,
    /// this returns the raw `broadcast::Receiver` which supports synchronous
    /// `try_recv()` — useful for WASM where async polling with noop wakers
    /// doesn't work reliably.
    pub async fn subscribe_session_events_raw(
        &self,
        id: &SessionId,
    ) -> Result<
        tokio::sync::broadcast::Receiver<EventEnvelope<AgentEvent>>,
        meerkat_core::comms::StreamError,
    > {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| meerkat_core::comms::StreamError::NotFound(format!("session {id}")))?;
        Ok(handle.raw_session_event_tx.subscribe())
    }

    fn is_session_state_active(state: SessionState) -> bool {
        state.is_active
    }

    fn claim_start_turn(
        id: &SessionId,
        handle: &SessionHandle,
        validated_identity: Option<SessionLlmIdentity>,
    ) -> Result<StartTurnAdmissionClaim, SessionError> {
        StartTurnAdmissionClaim::claim(id, handle, validated_identity)
    }

    fn request_start_turn(id: &SessionId, handle: &SessionHandle) -> Result<(), SessionError> {
        Self::claim_start_turn(id, handle, None)
            .map(StartTurnAdmissionClaim::transfer_to_session_task)
    }

    fn try_abort_admitted_turn(handle: &SessionHandle) {
        let projection = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            let phase = slot.abort_claim().ok();
            phase.map(|_| slot.projection())
        };
        if let Some(projection) = projection {
            handle.state_tx.send_replace(projection);
        }
    }
}

impl<B: SessionAgentBuilder + 'static> EphemeralSessionService<B> {
    pub(crate) async fn create_session_with_admission(
        &self,
        req: CreateSessionRequest,
        reserved_create_admission: Option<RuntimeContextAdmissionGuard>,
    ) -> Result<RunResult, SessionError> {
        self.create_session_with_admission_and_witness(req, reserved_create_admission, None)
            .await
            .map(|(result, _witness)| result)
    }

    /// Create an actor and return its exact registry witness in the same
    /// future completion as the public result.  The witness is minted before
    /// registry insertion and no suspension occurs between successful
    /// insertion and the deferred-create return, allowing an outer lifecycle
    /// transaction to publish exact cleanup authority without a sampling gap.
    #[doc(hidden)]
    pub async fn create_session_with_admission_and_witness(
        &self,
        req: CreateSessionRequest,
        reserved_create_admission: Option<RuntimeContextAdmissionGuard>,
        actor_witness_slot: Option<&LiveSessionActorWitnessSlot>,
    ) -> Result<(RunResult, LiveSessionActorWitness), SessionError> {
        let prompt = req.prompt.clone();
        let injected_context = req.injected_context.clone();
        let caller_event_tx = req.event_tx.clone();
        let defer_initial_turn =
            req.initial_turn == meerkat_core::service::InitialTurnPolicy::Defer;
        // Injected context is a first-turn submit-work fact; the deferred
        // create path has no first turn to attach it to and the deferred-turn
        // staging slot does not carry it. Fail closed rather than silently
        // dropping host-provided context.
        if defer_initial_turn && !injected_context.is_empty() {
            return Err(SessionError::Unsupported(
                "injected_context is not supported on a deferred session create; deliver it \
                 with the first turn's StartTurnRequest"
                    .to_string(),
            ));
        }
        let labels = req.labels.clone().unwrap_or_default();
        let resumed_session = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref());
        let resumed_session_id = resumed_session.map(|session| session.id().clone());
        let runtime_binding_session_id =
            req.build
                .as_ref()
                .and_then(|build| match &build.runtime_build_mode {
                    meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => {
                        Some(bindings.session_id().clone())
                    }
                    meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                });
        if let (Some(resumed_session_id), Some(runtime_binding_session_id)) =
            (&resumed_session_id, &runtime_binding_session_id)
            && resumed_session_id != runtime_binding_session_id
        {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "runtime binding session {runtime_binding_session_id} does not match resumed session {resumed_session_id}"
                )),
            ));
        }
        let expected_session_id = runtime_binding_session_id.or(resumed_session_id);
        let resumed_deferred_turn_state = resumed_session
            .map(meerkat_core::Session::try_deferred_turn_state)
            .transpose()
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "generated deferred-turn authority rejected session creation restore: {err}"
                )))
            })?
            .flatten();
        let mut deferred_turn_state = resumed_deferred_turn_state.clone().unwrap_or_default();
        let resumed_session_is_deferred_template = resumed_session.is_some_and(|session| {
            session.messages().is_empty() && resumed_deferred_turn_state.is_none()
        });
        if let Some(blob_store) = req
            .build
            .as_ref()
            .and_then(|build| build.blob_store_override.clone())
        {
            hydrate_deferred_turn_state(
                blob_store.as_ref(),
                &mut deferred_turn_state,
                // Session creation hydrates blobs that were just externalized;
                // a missing blob here is a hard fault, not evicted history.
                MissingBlobBehavior::Error,
            )
            .await
            .map_err(|err| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "failed to hydrate deferred-turn state during session creation: {err}"
                )))
            })?;
        }
        if defer_initial_turn && (resumed_session.is_none() || resumed_session_is_deferred_template)
        {
            deferred_turn_state.mark_initial_turn_pending();
        }
        if defer_initial_turn && req.deferred_prompt_policy == DeferredPromptPolicy::Stage {
            deferred_turn_state.stage_initial_prompt(prompt.clone(), SystemTime::now());
        }
        let deferred_turn_state = Arc::new(std::sync::Mutex::new(deferred_turn_state));

        let create_capacity_permit = match reserved_create_admission {
            Some(admission) => admission.into_create_session_permit(),
            None => self.try_acquire_active_permit()?,
        };

        // Create the permanent event channel for this session.
        let (agent_event_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

        // Build the agent
        let agent = self
            .builder
            .build_agent(&req, agent_event_tx.clone())
            .await?;
        let llm_identity = agent
            .durable_llm_identity()
            .ok_or_else(|| Self::missing_durable_llm_identity_error("session creation"))?;
        self.validate_prompt_video_input(&prompt, &llm_identity)
            .await?;
        let session_id = agent.session_id();
        if let Some(expected_session_id) = expected_session_id.as_ref()
            && expected_session_id != &session_id
        {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "built agent session {session_id} does not match requested session {expected_session_id}"
                )),
            ));
        }
        let created_at = SystemTime::now();
        let transient_turn_context_state = agent.transient_turn_context_state();
        let actor_witness =
            LiveSessionActorWitness::new(session_id.clone(), transient_turn_context_state.clone());
        let turn_admission_slot = TurnAdmissionSlot::new();
        let initial_session_state = turn_admission_slot.projection();
        let turn_admission = Arc::new(std::sync::Mutex::new(turn_admission_slot));
        let active_capacity_lease =
            Arc::new(std::sync::Mutex::new(SessionActiveCapacityLease::default()));
        // Deferred sessions hand the reserved capacity permit to the registry
        // (recorded below, once the handle is stored): the registry owns
        // staged-permit custody, there is no handle-side permit slot. Eager
        // sessions thread the permit into the active-capacity lease.
        let (eager_active_admission, staged_create_permit) = if defer_initial_turn {
            (None, Some(create_capacity_permit))
        } else {
            (
                Some(acquire_active_capacity_lease(
                    Arc::clone(&active_capacity_lease),
                    create_capacity_permit,
                    None,
                )),
                None,
            )
        };

        // Extract the event injector before the agent moves into its task.
        let event_injector = agent.event_injector();
        let interaction_event_injector = agent.interaction_event_injector();
        let comms_runtime = agent.comms_runtime();
        let observed_comms_sender = agent.observed_comms_sender();
        let cancel_after_boundary_handle = agent.cancel_after_boundary_handle();
        let turn_state_handle = agent.turn_state_handle();
        // W2-E: capture the session-context DSL handle so the session task
        // can fire `AdvanceSessionContext` on every summary-publish site.
        let session_context = agent.session_context_handle();
        // Create session task channels
        let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(COMMAND_CHANNEL_CAPACITY);
        let archive_snapshot_gate = ArchiveSnapshotGate::open();
        let (state_tx, state_rx) = watch::channel(initial_session_state);
        let state_tx_handle = state_tx.clone();
        let (summary_tx, summary_rx) = watch::channel(SessionSummaryCache {
            updated_at: created_at,
            message_count: 0,
            total_tokens: 0,
            usage: Usage::default(),
            last_assistant_text: None,
        });
        let (llm_identity_tx, llm_identity_rx) = watch::channel(llm_identity);
        let (session_event_tx, session_event_rx) = tokio::sync::broadcast::channel::<
            Arc<EventEnvelope<AgentEvent>>,
        >(EVENT_CHANNEL_CAPACITY);
        drop(session_event_rx);
        let (raw_session_event_tx, raw_session_event_rx) =
            tokio::sync::broadcast::channel::<EventEnvelope<AgentEvent>>(EVENT_CHANNEL_CAPACITY);
        drop(raw_session_event_rx);
        #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
        let lossless_event_projection_tx = Arc::new(tokio::sync::Mutex::new(None));
        let interrupt_notify = Arc::new(tokio::sync::Notify::new());

        // Spawn the session task using the platform-appropriate task API.
        #[cfg(not(target_arch = "wasm32"))]
        tokio::spawn(session_task(
            agent,
            session_id.clone(),
            agent_event_tx,
            agent_event_rx,
            command_rx,
            Arc::clone(&deferred_turn_state),
            SessionTaskControl {
                actor_witness: actor_witness.clone(),
                state_tx,
                summary_tx,
                llm_identity_tx,
                turn_admission: Arc::clone(&turn_admission),
                interrupt_notify: interrupt_notify.clone(),
                session_event_tx: session_event_tx.clone(),
                raw_session_event_tx: raw_session_event_tx.clone(),
                #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
                lossless_event_projection_tx: Arc::clone(&lossless_event_projection_tx),
                session_context: session_context.clone(),
                archive_snapshot_gate: Arc::clone(&archive_snapshot_gate),
            },
        ));
        #[cfg(target_arch = "wasm32")]
        tokio_with_wasm::alias::task::spawn(session_task(
            agent,
            session_id.clone(),
            agent_event_tx,
            agent_event_rx,
            command_rx,
            Arc::clone(&deferred_turn_state),
            SessionTaskControl {
                actor_witness: actor_witness.clone(),
                state_tx,
                summary_tx,
                llm_identity_tx,
                turn_admission: Arc::clone(&turn_admission),
                interrupt_notify: interrupt_notify.clone(),
                session_event_tx: session_event_tx.clone(),
                raw_session_event_tx: raw_session_event_tx.clone(),
                #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
                lossless_event_projection_tx: Arc::clone(&lossless_event_projection_tx),
                session_context: session_context.clone(),
                archive_snapshot_gate: Arc::clone(&archive_snapshot_gate),
            },
        ));

        // Store the handle
        let handle = SessionHandle {
            actor_witness: actor_witness.clone(),
            command_tx: command_tx.clone(),
            state_tx: state_tx_handle,
            state_rx,
            summary_rx,
            llm_identity_rx,
            turn_admission: Arc::clone(&turn_admission),
            created_at,
            labels,
            event_injector,
            interaction_event_injector,
            comms_runtime,
            observed_comms_sender,
            transient_turn_context_state,
            archive_snapshot_gate,
            turn_state_handle,
            deferred_turn_state,
            active_capacity_lease,
            interrupt_notify,
            cancel_after_boundary_handle,
            session_event_tx,
            raw_session_event_tx,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            lossless_event_projection_tx,
        };

        let inserted = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(&session_id) {
                false
            } else {
                sessions.insert(session_id.clone(), handle);
                // Record the singular typed materialization fact keyed by
                // session id. For a deferred session the registry also takes
                // CUSTODY of the reserved capacity permit — staged-ness is
                // never re-derived from permit-presence elsewhere.
                match staged_create_permit {
                    Some(permit) => self.staged_registry.record_staged(&session_id, permit),
                    None => self.staged_registry.record_active(&session_id),
                }
                // Notify waiters (e.g., CLI --stdin) that a session is available.
                self.session_registered.notify_waiters();
                true
            }
        };
        if !inserted {
            // Duplicate IDs are unexpected but can happen if the builder returns a reused ID.
            // Stop the task so it does not leak in the background.
            actor_witness.revoke();
            let _ = command_tx.send(SessionCommand::Shutdown).await;
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "Duplicate session ID generated: {session_id}"
                )),
            ));
        }

        // Exact cleanup authority is published at the actor registry
        // linearization point. In particular, eager create must not wait for
        // its first turn before making the witness observable: that future can
        // be cancelled while the actor remains registered.
        if let Some(actor_witness_slot) = actor_witness_slot
            && let Err(error) = actor_witness_slot.publish(actor_witness.clone())
        {
            // Slot reuse is a malformed transaction, but the just-minted
            // actor is already visible. Compare-and-remove that exact
            // incarnation before returning so the older witness retained
            // by the slot cannot strand an unaddressable replacement.
            let _ = self.discard_live_session_actor(&actor_witness).await;
            return Err(error);
        }

        if defer_initial_turn {
            return Ok((
                RunResult {
                    text: String::new(),
                    session_id,
                    turns: 0,
                    tool_calls: 0,
                    usage: Usage::default(),
                    terminal_cause_kind: None,
                    structured_output: None,
                    extraction_error: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                },
                actor_witness,
            ));
        }

        // Claim the canonical turn slot for the eager first turn.
        {
            let sessions = self.sessions.read().await;
            let handle = sessions.get(&session_id).ok_or_else(|| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                    "fresh session handle missing for eager first turn: {session_id}"
                )))
            })?;
            if let Err(error) = Self::request_start_turn(&session_id, handle) {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "fresh session failed to admit eager first turn: {error}"
                    )),
                ));
            }
        }

        // Dogma K10: `RuntimeTurnMetadata` (via `build.initial_turn_metadata`)
        // is the ONLY carrier of initial-turn render/skill facts — there is no
        // request-level duplicate left to merge.
        let initial_turn_metadata = req
            .build
            .as_ref()
            .and_then(|build| build.initial_turn_metadata.as_ref())
            .cloned();
        let initial_handling_mode = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.handling_mode)
            .unwrap_or(meerkat_core::types::HandlingMode::Queue);
        let initial_turn_tool_overlay = initial_turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.turn_tool_overlay.clone());
        let initial_system_messages = initial_turn_metadata
            .as_ref()
            .map(|metadata| metadata.system_prompts.clone())
            .unwrap_or_default();
        let initial_runtime = meerkat_core::service::StartTurnRuntimeSemantics::new(
            initial_handling_mode,
            initial_turn_tool_overlay,
            initial_turn_metadata,
        );

        // Run the first turn
        let (result_tx, result_rx) = oneshot::channel();
        if command_tx
            .send(SessionCommand::StartTurn {
                prompt,
                system_messages: initial_system_messages,
                injected_context,
                runtime: Box::new(initial_runtime),
                event_tx: caller_event_tx,
                result_tx,
                active_admission: eager_active_admission,
            })
            .await
            .is_err()
        {
            let sessions = self.sessions.read().await;
            if let Some(handle) = sessions.get(&session_id) {
                Self::try_abort_admitted_turn(handle);
            }
            drop(sessions);
            let mut sessions = self.sessions.write().await;
            if let Some(handle) = sessions.swap_remove(&session_id) {
                handle.actor_witness.revoke();
            }
            self.staged_registry.forget(&session_id);
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(
                    "Session task exited before first turn".to_string(),
                ),
            ));
        }

        let result = match result_rx.await {
            Ok(result) => result,
            Err(_) => {
                let mut sessions = self.sessions.write().await;
                if let Some(handle) = sessions.swap_remove(&session_id) {
                    handle.actor_witness.revoke();
                }
                self.staged_registry.forget(&session_id);
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "Session task dropped the result channel".to_string(),
                    ),
                ));
            }
        };

        result
            .into_public_result()
            .map(|result| (result, actor_witness))
            .map_err(SessionError::Agent)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionService for EphemeralSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.create_session_with_admission(req, None).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        // Claim through the generated turn-admission authority before waiting
        // on the runtime-finalization mutex. The claim is the canonical Busy
        // decision; the outer mutex protects identity + turn finalization but
        // must never turn overlapping public turns into a queue.
        let preclaimed_turn = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            let identity = handle.llm_identity_rx.borrow().clone();
            self.validate_prompt_video_input(&req.prompt, &identity)
                .await?;
            Self::claim_start_turn(id, handle, Some(identity))?
        };
        let _turn_finalization_guard = self.acquire_runtime_turn_finalization_guard(id).await;
        self.start_turn_execution_with_admission_recovering_not_found(
            id,
            req,
            None,
            Some(preclaimed_turn),
        )
        .await
        .map_err(|(error, _admission)| error)?
        .into_public_result()
        .map_err(SessionError::Agent)
    }

    async fn reconcile_runtime_compaction_projections(
        &self,
        id: &SessionId,
        intents: Vec<meerkat_core::CompactionProjectionIntent>,
    ) -> Result<(), SessionError> {
        EphemeralSessionService::reconcile_runtime_compaction_projections(self, id, intents).await
    }

    async fn abort_uncommitted_compaction_projections(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        EphemeralSessionService::abort_uncommitted_compaction_projections(self, id).await
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        let _turn_finalization_guard = self.acquire_runtime_turn_finalization_guard(id).await;
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ReplaceClient { client, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    async fn hot_swap_session_llm_identity(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        let _turn_finalization_guard = self.acquire_runtime_turn_finalization_guard(id).await;
        self.apply_runtime_session_llm_identity_under_runtime_turn_boundary(
            id,
            client,
            identity,
            request_policy,
        )
        .await
    }

    async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
        {
            let _turn_finalization_guard = self.acquire_runtime_turn_finalization_guard(id).await;
            self.apply_runtime_session_tool_visibility_state_under_runtime_turn_boundary(id, state)
                .await
        }
        #[cfg(not(all(feature = "session-store", not(target_arch = "wasm32"))))]
        {
            let _ = (id, state);
            Err(SessionError::Unsupported(
                "set_session_tool_visibility_state".to_string(),
            ))
        }
    }

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        let _turn_finalization_guard = self.acquire_runtime_turn_finalization_guard(id).await;
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::StageToolFilter { filter, reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let woke = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.request_interrupt()
                .map_err(|_| SessionError::NotRunning { id: id.clone() })?
        };
        if woke {
            wake_interrupt_notify(&handle.interrupt_notify);
        }
        Ok(())
    }

    async fn interrupt_run_if_current(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
    ) -> Result<bool, SessionError> {
        EphemeralSessionService::interrupt_run_if_current(self, id, expected_run_id).await
    }

    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        // Preserve the ordinary SessionService API's "current active run"
        // semantics by resolving the current witness once, then delegating to
        // the exact run-scoped command path.
        let expected_run_id = {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            if handle.cancel_after_boundary_handle.is_none() {
                return Err(SessionError::Unsupported(
                    "cancel_after_boundary".to_string(),
                ));
            }
            let turn_state_handle = handle.turn_state_handle.as_deref().ok_or_else(|| {
                SessionError::Unsupported("cancel_after_boundary_exact_run_authority".to_string())
            })?;
            turn_state_handle
                .snapshot()
                .active_run_id
                .ok_or_else(|| SessionError::NotRunning { id: id.clone() })?
        };
        self.cancel_after_boundary_for_run(id, &expected_run_id)
            .await
    }

    async fn cancel_after_boundary_for_run(
        &self,
        id: &SessionId,
        expected_run_id: &RunId,
    ) -> Result<(), SessionError> {
        EphemeralSessionService::cancel_after_boundary_for_run(self, id, expected_run_id).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = match sessions.get(id) {
            Some(handle) => handle,
            None => {
                drop(sessions);
                return self
                    .archived_views
                    .read()
                    .await
                    .get(id)
                    .cloned()
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() });
            }
        };

        // Serve live reads from the service-owned summary/watch state instead
        // of round-tripping through the session task. This keeps read-side
        // snapshots responsive on sync surfaces such as wasm exports.
        let state = *handle.state_rx.borrow();
        let summary = handle.summary_rx.borrow().clone();
        let live_identity = handle.llm_identity_rx.borrow().clone();
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: handle.created_at,
                updated_at: summary.updated_at,
                message_count: summary.message_count,
                is_active: Self::is_session_state_active(state),
                model: live_identity.model,
                provider: live_identity.provider,
                last_assistant_text: summary.last_assistant_text,
                labels: handle.labels.clone(),
            },
            billing: SessionUsage {
                total_tokens: summary.total_tokens,
                usage: summary.usage,
            },
        })
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        let mut summaries: Vec<SessionSummary> = sessions
            .iter()
            .map(|(session_id, h)| {
                let state = *h.state_rx.borrow();
                let cache = h.summary_rx.borrow();
                SessionSummary {
                    session_id: session_id.clone(),
                    created_at: h.created_at,
                    updated_at: cache.updated_at,
                    message_count: cache.message_count,
                    total_tokens: cache.total_tokens,
                    is_active: Self::is_session_state_active(state),
                    labels: h.labels.clone(),
                }
            })
            .collect();

        // Filter by labels if specified (all k/v pairs must match).
        if let Some(ref filter_labels) = query.labels {
            summaries.retain(|s| {
                filter_labels
                    .iter()
                    .all(|(k, v)| s.labels.get(k) == Some(v))
            });
        }

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
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        // Reserve shutdown capacity without retaining the global live-session
        // registry. A saturated session must not block reads, interrupts, or
        // unrelated session work while archive waits for its own actor.
        let (mut sessions, shutdown_permit) = loop {
            let expected_sender = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(id)
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() })?
                    .command_tx
                    .clone()
            };
            let reserved = expected_sender.clone().reserve_owned().await;
            let sessions = self.sessions.write().await;
            let Some(current) = sessions.get(id) else {
                return Err(SessionError::NotFound { id: id.clone() });
            };
            if !current.command_tx.same_channel(&expected_sender) {
                // The logical session was rematerialized while capacity was
                // pending. Drop any old-channel permit and retry against the
                // exact current actor; stale capacity is never authority.
                drop(sessions);
                drop(reserved);
                continue;
            }
            let shutdown_permit = reserved.map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task exited before archive shutdown could be reserved".to_string(),
                ))
            })?;
            break (sessions, shutdown_permit);
        };
        // Every remaining awaitable realization resource is acquired before
        // asking the document machine to authorize the lifecycle change.
        // Cancellation while acquiring the archived-view guard therefore
        // leaves both registries and every live-handle carrier untouched.
        // Global lock order for the only operation that needs both registries:
        // live sessions, then archived views. Readers never retain an archived
        // view guard while acquiring the live registry.
        let mut archived_views = self.archived_views.write().await;
        // Standalone archive still changes the canonical session-document
        // lifecycle fact. Obtain the generated Active -> Archived verdict
        // only after effect realization can complete without another await.
        authorize_standalone_archive(id)?;
        let handle = sessions
            .swap_remove(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let archived_view = Self::archived_view_from_handle(id, &handle);
        archived_views.insert(id.clone(), archived_view);

        handle.actor_witness.revoke();
        // Clear the singular typed materialization record with the handle; a
        // staged session's registry-held permit drops with it, freeing the
        // reserved capacity.
        self.staged_registry.forget(id);
        handle.archive_snapshot_gate.close_for_snapshot();

        let projection = {
            let mut slot = lock_turn_admission(&handle.turn_admission);
            slot.request_shutdown().ok().map(|_| slot.projection())
        };
        if let Some(projection) = projection {
            handle.state_tx.send_replace(projection);
        }
        drop(archived_views);
        drop(sessions);
        // The reserved permit turns shutdown delivery into the final
        // synchronous realization step: there is no post-verdict cancellation
        // point that can strand the two registries in different lifecycles.
        drop(shutdown_permit.send(SessionCommand::Shutdown));
        Ok(())
    }

    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::UpdateMobToolAuthority {
                authority_context,
                reply_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;
        reply_rx
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task dropped reply channel".to_string(),
                ))
            })?
            .map_err(SessionError::Agent)
    }

    async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        EphemeralSessionService::<B>::subscribe_session_events(self, id).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionServiceControlExt for EphemeralSessionService<B> {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        let status = self
            .append_system_message_control(id, req)
            .await
            .map_err(SessionControlError::Session)?;
        Ok(AppendSystemContextResult { status })
    }

    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        Self::validate_tool_result_video(&req.results)?;
        let session = self.export_session(id).await?;
        if matches!(
            session.classify_callback_result_ingress(&req.results)?,
            meerkat_core::session::CallbackResultIngress::AlreadyApplied
        ) {
            return Ok(StageToolResultsResult {
                accepted_result_count: 0,
                disposition: StageToolResultsDisposition::AlreadyApplied,
            });
        }
        let state = self
            .deferred_turn_state(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let accepted = {
            let mut guard = lock_deferred_turn_state(&state);
            guard
                .try_stage_tool_results(req.results, SystemTime::now())
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::ConfigError(
                        error.to_string(),
                    ))
                })?
        };
        Ok(StageToolResultsResult {
            accepted_result_count: accepted,
            disposition: if accepted == 0 {
                StageToolResultsDisposition::AlreadyStaged
            } else {
                StageToolResultsDisposition::Staged
            },
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionServiceCommsExt for EphemeralSessionService<B> {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        EphemeralSessionService::<B>::comms_runtime(self, session_id).await
    }

    async fn send_comms(
        &self,
        session_id: &SessionId,
        command: meerkat_core::CommsCommand,
    ) -> Option<Result<meerkat_core::SendReceipt, meerkat_core::SendError>> {
        EphemeralSessionService::<B>::send_comms(self, session_id, command).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        EphemeralSessionService::<B>::event_injector(self, session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        EphemeralSessionService::<B>::interaction_event_injector(self, session_id).await
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B: SessionAgentBuilder + 'static> SessionServiceHistoryExt for EphemeralSessionService<B> {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        match self.export_session(id).await {
            Ok(session) => Ok(SessionHistoryPage::from_messages(
                session.id().clone(),
                session.messages(),
                query,
            )),
            Err(SessionError::NotFound { .. }) => {
                if self.archived_views.read().await.contains_key(id) {
                    Err(SessionError::PersistenceDisabled)
                } else {
                    Err(SessionError::NotFound { id: id.clone() })
                }
            }
            Err(err) => Err(err),
        }
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

/// Long-lived task that exclusively owns a session agent and processes commands.
fn stamp_event_envelope(
    next_seq: &mut u64,
    source: &EventSourceIdentity,
    event: AgentEvent,
) -> EventEnvelope<AgentEvent> {
    *next_seq += 1;
    // mob_id is optional and only set when a surface/runtime has mob context.
    EventEnvelope::new_with_source(source.clone(), *next_seq, None, event)
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
async fn publish_interaction_terminal_batch(
    session_id: &SessionId,
    next_seq: &mut u64,
    control: &SessionTaskControl,
    event_store: &dyn crate::event_store::EventStore,
    expected_actor: Option<&LiveSessionActorWitness>,
    events: Vec<AgentEvent>,
) -> Result<
    Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
    SessionError,
> {
    if let Some(expected_actor) = expected_actor
        && (!expected_actor.same_incarnation(&control.actor_witness) || !expected_actor.is_live())
    {
        return Err(SessionError::NotFound {
            id: expected_actor.session_id().clone(),
        });
    }
    if events.len() > crate::event_store::MAX_EXACT_INTERACTION_TERMINAL_BATCH {
        return Err(SessionError::Store(Box::new(
            crate::event_store::EventStoreError::InvalidExactInteractionTerminalBatch {
                reason: format!(
                    "batch contains {} terminals, exceeding the maximum of {}",
                    events.len(),
                    crate::event_store::MAX_EXACT_INTERACTION_TERMINAL_BATCH
                ),
            },
        )));
    }

    let mut terminals = Vec::with_capacity(events.len());
    for event in events {
        let interaction_id = match &event {
            AgentEvent::InteractionComplete { interaction_id, .. }
            | AgentEvent::InteractionCallbackPending { interaction_id, .. }
            | AgentEvent::InteractionFailed { interaction_id, .. } => *interaction_id,
            _ => {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "runtime terminal publication received a non-interaction event".to_string(),
                    ),
                ));
            }
        };
        terminals.push((
            interaction_id,
            EventEnvelope::new_with_source(
                EventSourceIdentity::interaction(interaction_id),
                0,
                None,
                event,
            ),
        ));
    }
    crate::event_store::validate_exact_interaction_terminal_batch(&terminals)
        .map_err(|error| SessionError::Store(Box::new(error)))?;
    if terminals.is_empty() {
        return Ok(Vec::new());
    }

    let stream_seq_floor = *next_seq;
    let appends = event_store
        .append_interaction_terminals_exact_batch(session_id, stream_seq_floor, &terminals)
        .await
        .map_err(|error| SessionError::Store(Box::new(error)))?;
    if appends.len() != terminals.len() {
        return Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(format!(
                "exact interaction batch returned {} receipts for {} terminals",
                appends.len(),
                terminals.len()
            )),
        ));
    }

    // Validate the complete store result before mutating the live sequencer or
    // broadcasting any row. A malformed custom EventStore therefore cannot
    // create a half-published in-memory batch.
    let mut receipts = Vec::with_capacity(terminals.len());
    let mut inserted = Vec::new();
    let mut saw_inserted = false;
    let mut replay_tail: Option<u64> = None;
    let mut expected_insert_stream_seq: Option<u64> = None;
    let mut canonical_tail = stream_seq_floor;
    for ((interaction_id, requested), append) in terminals.iter().zip(appends) {
        let (was_inserted, stored) = match append {
            crate::event_store::ExactInteractionAppend::Inserted(stored) => (true, stored),
            crate::event_store::ExactInteractionAppend::Replayed(stored) => (false, stored),
        };
        let stored_envelope = stored.to_envelope();
        crate::event_store::validate_exact_interaction_terminal(*interaction_id, &stored_envelope)
            .map_err(|error| SessionError::Store(Box::new(error)))?;
        if stored.mob_id != requested.mob_id
            || !crate::event_store::interaction_terminal_events_semantically_equal(
                &stored.event,
                &requested.payload,
            )
        {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "exact interaction batch returned a non-canonical row for {interaction_id}"
                )),
            ));
        }

        if was_inserted {
            saw_inserted = true;
            let expected = match expected_insert_stream_seq {
                Some(previous) => previous.checked_add(1),
                None => stream_seq_floor
                    .max(replay_tail.unwrap_or(0))
                    .checked_add(1),
            }
            .ok_or_else(|| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "session event stream sequence overflow in exact terminal batch".to_string(),
                ))
            })?;
            if stored.stream_seq != expected {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "exact interaction batch inserted stream seq {}, expected {expected}",
                        stored.stream_seq
                    )),
                ));
            }
            expected_insert_stream_seq = Some(stored.stream_seq);
            inserted.push((stored.stream_seq, stored.to_envelope()));
        } else {
            if saw_inserted {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "exact interaction batch returned a replay after an inserted suffix row"
                            .to_string(),
                    ),
                ));
            }
            if stored.stream_seq == 0 {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "exact interaction replay returned zero stream sequence".to_string(),
                    ),
                ));
            }
            if let Some(previous) = replay_tail {
                let expected = previous.checked_add(1).ok_or_else(|| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        "exact interaction replay prefix sequence overflow".to_string(),
                    ))
                })?;
                if stored.stream_seq != expected {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "exact interaction replay prefix stream seq {}, expected {expected}",
                            stored.stream_seq
                        )),
                    ));
                }
            }
            replay_tail = Some(stored.stream_seq);
        }
        canonical_tail = canonical_tail.max(stored.stream_seq);
        receipts.push(
            meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt::try_new(
                &stored.event,
                stored.seq,
            )
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    error.to_string(),
                ))
            })?,
        );
    }

    inserted.sort_unstable_by_key(|(stream_seq, _)| *stream_seq);
    let actor_still_exact = expected_actor.is_none_or(|expected_actor| {
        expected_actor.same_incarnation(&control.actor_witness) && expected_actor.is_live()
    });
    if actor_still_exact {
        *next_seq = canonical_tail;
        for (_, envelope) in inserted {
            control.publish_session_event(envelope).await;
        }
    }
    Ok(receipts)
}

/// Render a typed [`LiveAdapterErrorCode`] into the human-readable display
/// projection carried by the `RunFailed` event's `error`/`message` fields.
///
/// The typed cause remains authoritative on the wire (it is the source of this
/// projection); this is purely the display mirror, never the routing fact.
fn render_live_terminal_error_message(
    cause: &meerkat_core::live_adapter::LiveAdapterErrorCode,
) -> String {
    use meerkat_core::live_adapter::LiveAdapterErrorCode as Code;
    match cause {
        Code::ConnectionFailed => "live channel connection failed".to_string(),
        Code::ConnectionLost => "live channel connection lost".to_string(),
        Code::ConfigRejected { reason } => {
            format!("live channel configuration rejected: {reason}")
        }
        Code::ProviderError => "live channel provider error".to_string(),
        Code::AuthenticationFailed => "live channel authentication failed".to_string(),
        Code::InternalError => "live channel internal error".to_string(),
        Code::Other { raw } => format!("live channel error: {raw}"),
        _ => "live channel terminal error".to_string(),
    }
}

fn lock_deferred_turn_state(
    state: &Arc<std::sync::Mutex<SessionDeferredTurnState>>,
) -> std::sync::MutexGuard<'_, SessionDeferredTurnState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("deferred-turn state lock poisoned; continuing with inner state");
            poisoned.into_inner()
        }
    }
}

fn lock_active_capacity_lease(
    lease: &Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
) -> std::sync::MutexGuard<'_, SessionActiveCapacityLease> {
    lease
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn acquire_active_capacity_lease(
    active_capacity_lease: Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
    permit: Option<OwnedSemaphorePermit>,
    promotion: Option<PromotionTicket>,
) -> RuntimeContextAdmissionGuard {
    let mut lease = lock_active_capacity_lease(&active_capacity_lease);
    if lease.leases == 0 {
        lease.permit = permit;
        lease.promotion = promotion;
    } else {
        // An existing lease already holds this session's capacity unit, so
        // the incoming permit folds back into the gate (capacity tokens are
        // fungible). An in-flight promotion binds to the shared lease so the
        // FINAL release settles it through the registry.
        drop(permit);
        if lease.promotion.is_none() {
            lease.promotion = promotion;
        }
    }
    lease.leases = lease.leases.saturating_add(1);
    drop(lease);
    RuntimeContextAdmissionGuard {
        active_capacity_lease: Some(active_capacity_lease),
        active_permit: None,
    }
}

fn try_join_active_capacity_lease(
    active_capacity_lease: Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
) -> Option<RuntimeContextAdmissionGuard> {
    let mut lease = lock_active_capacity_lease(&active_capacity_lease);
    if lease.leases == 0 {
        return None;
    }
    lease.leases = lease.leases.saturating_add(1);
    drop(lease);
    Some(RuntimeContextAdmissionGuard {
        active_capacity_lease: Some(active_capacity_lease),
        active_permit: None,
    })
}

fn release_active_capacity_lease(
    active_capacity_lease: &Arc<std::sync::Mutex<SessionActiveCapacityLease>>,
) -> ActiveCapacityLeaseRelease {
    let mut lease = lock_active_capacity_lease(active_capacity_lease);
    if lease.leases == 0 {
        return ActiveCapacityLeaseRelease::default();
    }
    lease.leases -= 1;
    if lease.leases == 0 {
        ActiveCapacityLeaseRelease {
            permit: lease.permit.take(),
            promotion: lease.promotion.take(),
        }
    } else {
        ActiveCapacityLeaseRelease::default()
    }
}

fn lock_turn_admission(
    slot: &Arc<std::sync::Mutex<TurnAdmissionSlot>>,
) -> std::sync::MutexGuard<'_, TurnAdmissionSlot> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Roll an admitted turn back to `Idle` from within the session task when a
/// preflight check fails before the run starts.
fn abort_admitted_turn(control: &SessionTaskControl) {
    let projection = {
        let mut slot = lock_turn_admission(&control.turn_admission);
        let phase = slot.abort_claim().ok();
        phase.map(|_| slot.projection())
    };
    if let Some(projection) = projection {
        control.state_tx.send_replace(projection);
    }
}

fn merge_content_inputs(
    deferred: meerkat_core::types::ContentInput,
    turn: meerkat_core::types::ContentInput,
) -> meerkat_core::types::ContentInput {
    match (&deferred, &turn) {
        (
            meerkat_core::types::ContentInput::Text(deferred_text),
            meerkat_core::types::ContentInput::Text(turn_text),
        ) => meerkat_core::types::ContentInput::Text(format!("{deferred_text}\n\n{turn_text}")),
        _ => {
            let mut blocks = deferred.into_blocks();
            blocks.extend(turn.into_blocks());
            meerkat_core::types::ContentInput::Blocks(blocks)
        }
    }
}

fn restore_deferred_turn_inputs(
    deferred_turn_state: &Arc<std::sync::Mutex<SessionDeferredTurnState>>,
    consumed: ConsumedDeferredTurnInputs,
) {
    let mut guard = lock_deferred_turn_state(deferred_turn_state);
    guard.restore_consumed_turn_inputs(consumed);
}

/// D1 drain obligation: called from both `ShuttingDown` entry points (the
/// `Shutdown` command handler and the `StartTurn` finalize-to-shutdown path).
/// Closes the command channel so senders receive an error, drains any
/// already-buffered commands resolving each waiter with its typed
/// benign/terminal outcome, then closes the machine-owned drain obligation
/// and authorizes session teardown.
async fn drain_session_task_commands<A: SessionAgent>(
    commands: &mut mpsc::Receiver<SessionCommand>,
    agent: &mut A,
    session_id: &SessionId,
    control: &SessionTaskControl,
    next_seq: &mut u64,
    source: &EventSourceIdentity,
    transcript_authority_generation: &mut u64,
) {
    // Prevent any new commands from entering the buffer.
    commands.close();

    // Drain buffered commands and resolve their waiters.
    while let Ok(cmd) = commands.try_recv() {
        if cmd.advances_transcript_authority_generation() {
            *transcript_authority_generation = (*transcript_authority_generation).saturating_add(1);
        }
        match cmd {
            SessionCommand::StartLiveBridgeOperation { accepted_tx, .. } => {
                let _ = accepted_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::ValidateLiveBridgeMemberEligibility { reply_tx } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::StartTurn { result_tx, .. } => {
                // Machine is in ShuttingDown; dispatch authorization resolves
                // `Cancelled` for this phase. Reply directly without
                // attempting a claim (the slot is already ShuttingDown).
                let _ = result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(Err(
                    meerkat_core::error::AgentError::Cancelled,
                )));
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::PublishInteractionTerminalsExactBatch { reply_tx, .. } => {
                let _ = reply_tx.send(Err(SessionError::Agent(
                    meerkat_core::error::AgentError::Cancelled,
                )));
            }
            SessionCommand::ExportSession { reply_tx } => {
                let _ = reply_tx.send(agent.session_clone());
            }
            SessionCommand::ObserveSessionTranscriptAuthority { reply_tx } => {
                let result = agent.session_transcript_authority().map(|snapshot| {
                    snapshot.bind_actor_generation(*transcript_authority_generation)
                });
                let _ = reply_tx.send(result);
            }
            SessionCommand::ExportSessionIfTranscriptAuthority { expected, reply_tx } => {
                let result = agent
                    .session_transcript_authority()
                    .map(|snapshot| {
                        snapshot.bind_actor_generation(*transcript_authority_generation)
                    })
                    .and_then(|current| {
                        if current == expected {
                            agent.session_clone().map(Some)
                        } else {
                            Ok(None)
                        }
                    });
                let _ = reply_tx.send(result);
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::ClassifyCallbackResultIngress { results, reply_tx } => {
                let _ = reply_tx.send(agent.classify_callback_result_ingress(&results));
            }
            SessionCommand::ReconcileRuntimeCompactionProjections { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::AbortUncommittedCompactionProjections { reply_tx } => {
                // Cleanup is still mandatory while shutting down: dropping
                // this command would strand an invisible stage after the
                // runtime already proved its atomic boundary did not commit.
                let result = agent.abort_uncommitted_compaction_projections().await;
                let _ = reply_tx.send(result);
            }
            SessionCommand::ExecutionSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.execution_snapshot());
            }
            SessionCommand::ToolScopeSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.tool_scope_snapshot());
            }
            SessionCommand::VisibleToolDefs { reply_tx } => {
                let _ = reply_tx.send(agent.visible_tool_defs());
            }
            SessionCommand::ExternalToolSurfaceSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.external_tool_surface_snapshot());
            }
            SessionCommand::RecordLiveTerminalError { cause, reply_tx } => {
                let message = render_live_terminal_error_message(&cause);
                let failed = stamp_event_envelope(
                    next_seq,
                    source,
                    AgentEvent::RunFailed {
                        session_id: session_id.clone(),
                        terminal_cause_kind: None,
                        error_report: meerkat_core::event::AgentErrorReport {
                            class: meerkat_core::event::AgentErrorClass::Terminal,
                            reason: None,
                            message,
                        },
                    },
                );
                control.publish_session_event(failed).await;
                let _ = reply_tx.send(());
            }
            SessionCommand::RecordLiveOutputAudioDegraded { dropped, reply_tx } => {
                let truncated = stamp_event_envelope(
                    next_seq,
                    source,
                    AgentEvent::StreamTruncated {
                        reason: meerkat_core::event::StreamTruncationReason::OutputAudioDegraded {
                            dropped,
                        },
                    },
                );
                control.publish_session_event(truncated).await;
                let _ = reply_tx.send(());
            }
            // Mutations during drain — resolve with Cancelled; the session
            // is exiting and the change cannot be committed durably.
            SessionCommand::AppendExternalUserContent { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::AppendExternalAssistantOutput { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::AppendRealtimeTranscriptEvent { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::AdmitLiveAssistantPlaybackTarget { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::ResolveLiveAssistantPlaybackTarget { reply_tx, .. } => {
                let _ = reply_tx.send(None);
            }
            SessionCommand::ResolveLiveAssistantPlaybackOnChannelClose { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::CommitLiveUserTranscriptFinal { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::CommitLiveAssistantPlaybackTruncation { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::CommitLiveAssistantPlaybackComplete { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::ObserveLiveAssistantPlaybackTerminal { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::ObserveLiveAssistantPlaybackFinal { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::DispatchExternalToolCall { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::UpdateMobToolAuthority { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::AppendSystemMessageControl { reply_tx, .. } => {
                let _ = reply_tx.send(Err(AgentError::Cancelled));
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::ActivateInstructionControl { reply_tx, .. } => {
                let _ = reply_tx.send(Err(AgentError::Cancelled));
            }
            SessionCommand::PrepareHeadCanonicalRuntimeBoundary { reply_tx, .. } => {
                let _ = reply_tx.send(Err(AgentError::Cancelled));
            }
            SessionCommand::AcknowledgeHeadCanonicalRuntimeBoundary { reply_tx, .. } => {
                let _ = reply_tx.send(Err(AgentError::Cancelled));
            }
            SessionCommand::ReplaceClient { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::HotSwapLlmIdentity { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::StageToolFilter { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::SetToolVisibilityState { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::SyncSessionFromDurableSnapshot { reply_tx, .. } => {
                let _ = reply_tx.send(Err(meerkat_core::error::AgentError::Cancelled));
            }
            SessionCommand::Shutdown => {
                // Already in ShuttingDown; redundant Shutdown is a no-op.
            }
        }
    }

    // Close the machine-owned drain obligation and authorize teardown.
    {
        let mut slot = lock_turn_admission(&control.turn_admission);
        if let Err(err) = slot.resolve_pending_admission_drained() {
            tracing::warn!(
                error = %err,
                "failed to close pending-admission drain obligation; \
                 teardown will not be authorized by the machine"
            );
            return;
        }
        if let Err(err) = slot.authorize_session_teardown() {
            tracing::warn!(
                error = %err,
                "failed to authorize session teardown after drain obligation closed"
            );
        }
    }
}

async fn session_task<A: SessionAgent>(
    mut agent: A,
    session_id: SessionId,
    agent_event_tx: mpsc::Sender<AgentEvent>,
    mut agent_event_rx: mpsc::Receiver<AgentEvent>,
    mut commands: mpsc::Receiver<SessionCommand>,
    deferred_turn_state: Arc<std::sync::Mutex<SessionDeferredTurnState>>,
    control: SessionTaskControl,
) {
    let mut next_seq: u64 = 0;
    // Lives on the SessionTask incarnation, not inside replaceable Session
    // state. Durable sync and compaction rollback therefore cannot recreate a
    // previously observed generation under the same actor witness.
    let mut transcript_authority_generation: u64 = 0;
    // The service captures this canonical identity immediately after agent
    // construction and uses it as the registry key. Never re-read a custom
    // SessionAgent's potentially mutable identity inside the task.
    let source = EventSourceIdentity::session(session_id.clone());

    loop {
        let Some(cmd) = commands.recv().await else {
            break;
        };
        if cmd.advances_transcript_authority_generation() {
            transcript_authority_generation = transcript_authority_generation.saturating_add(1);
        }

        match cmd {
            SessionCommand::ValidateLiveBridgeMemberEligibility { reply_tx } => {
                let _ = reply_tx.send(agent.validate_live_bridge_member_eligibility());
                continue;
            }
            SessionCommand::StartLiveBridgeOperation {
                request,
                cancellation,
                accepted_tx,
            } => {
                if let Err(error) = agent.validate_live_bridge_operation(&request) {
                    let _ = accepted_tx.send(Err(error));
                    continue;
                }
                let execution = match agent.prepare_live_bridge_operation(request, cancellation) {
                    Ok(execution) => execution,
                    Err(error) => {
                        let _ = accepted_tx.send(Err(error));
                        continue;
                    }
                };
                let (terminal_tx, terminal_rx) = oneshot::channel();
                if accepted_tx.send(Ok(terminal_rx)).is_err() {
                    continue;
                }
                tokio::spawn(async move {
                    let result = execution.await;
                    let _ = terminal_tx.send(result);
                });
                continue;
            }
            SessionCommand::ReplaceClient { client, reply_tx } => {
                let _ = reply_tx.send(agent.replace_client(client));
                continue;
            }
            SessionCommand::HotSwapLlmIdentity {
                client,
                identity,
                request_policy,
                reply_tx,
            } => {
                let result =
                    agent.hot_swap_llm_identity(client, (*identity).clone(), *request_policy);
                if result.is_ok() {
                    control.llm_identity_tx.send_replace(*identity);
                }
                let _ = reply_tx.send(result);
                continue;
            }
            SessionCommand::StageToolFilter { filter, reply_tx } => {
                let _ = reply_tx.send(agent.stage_external_tool_filter(filter));
                continue;
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::SetToolVisibilityState { state, reply_tx } => {
                let _ = reply_tx.send(agent.set_tool_visibility_state(state.map(|state| *state)));
                continue;
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::SyncSessionFromDurableSnapshot { session, reply_tx } => {
                let durable_deferred_turn_state = session
                    .try_deferred_turn_state()
                    .map_err(|err| {
                        meerkat_core::error::AgentError::InternalError(format!(
                            "failed to restore durable deferred-turn state during live session sync: {err}"
                        ))
                    });
                let result = match durable_deferred_turn_state {
                    Ok(durable_deferred_turn_state) => {
                        let result = agent.sync_session_from_durable_snapshot(*session);
                        if result.is_ok()
                            && let Some(durable_deferred_turn_state) = durable_deferred_turn_state
                        {
                            *lock_deferred_turn_state(&deferred_turn_state) =
                                durable_deferred_turn_state;
                        }
                        result
                    }
                    Err(err) => Err(err),
                };
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
                continue;
            }
            SessionCommand::StartTurn {
                prompt,
                system_messages,
                injected_context,
                runtime,
                event_tx,
                result_tx,
                active_admission,
            } => {
                let runtime = *runtime;
                let metadata = runtime.turn_metadata;
                let render_metadata = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.render_metadata.clone());
                let handling_mode = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.handling_mode)
                    .unwrap_or(runtime.handling_mode);
                let skill_references = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.skill_references.clone());
                let turn_tool_overlay = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.turn_tool_overlay.clone())
                    .or(runtime.turn_tool_overlay);
                // #345 / dogma K13: forward the full typed keep-alive
                // tri-state to the machine. The metadata carrier is
                // `Option<KeepAliveDirective>`: Enable(policy) -> Enable,
                // Disable -> Disable, absent -> Preserve. No collapse.
                let keep_alive_request =
                    match metadata.as_ref().and_then(|metadata| metadata.keep_alive) {
                        Some(
                            meerkat_core::lifecycle::run_primitive::KeepAliveDirective::Enable(_),
                        ) => RuntimeKeepAliveRequest::Enable,
                        Some(
                            meerkat_core::lifecycle::run_primitive::KeepAliveDirective::Disable,
                        ) => RuntimeKeepAliveRequest::Disable,
                        None => RuntimeKeepAliveRequest::Preserve,
                    };
                let typed_turn_appends = runtime.typed_turn_appends;
                let prompt = if typed_turn_appends.is_empty() {
                    prompt
                } else {
                    meerkat_core::lifecycle::run_primitive::model_projection_content_input_from_conversation_appends(
                        &typed_turn_appends,
                    )
                };
                let execution_kind = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.execution_kind);
                let transcript_identity = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.transcript_message_identity());
                // `active_admission` is held for the whole turn. On any
                // pre-run failure path below it simply drops at the end of
                // this arm: the final lease release settles staged-restore
                // vs capacity-release through the registry-owned promotion
                // status, so there is no shell-side restore flag plumbing.
                let dispatch_authorization = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.authorize_start_turn_dispatch()
                };
                match dispatch_authorization {
                    Ok(StartTurnDispatchAuthorization::Authorized) => {}
                    Ok(StartTurnDispatchAuthorization::Cancelled) => {
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::Cancelled),
                            ));
                        continue;
                    }
                    Err(error) => {
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::InternalError(format!(
                                    "generated turn authority rejected dispatch: {error}"
                                ))),
                            ));
                        continue;
                    }
                }

                let consumed_deferred_inputs = {
                    let mut guard = lock_deferred_turn_state(&deferred_turn_state);
                    guard.consume_for_started_turn()
                };
                let prompt = match consumed_deferred_inputs.pending_initial_prompt() {
                    Some(staged_prompt) => {
                        merge_content_inputs(staged_prompt.prompt.clone(), prompt)
                    }
                    None => prompt,
                };
                let flattened_tool_results = consumed_deferred_inputs
                    .pending_tool_results()
                    .iter()
                    .flat_map(|pending| pending.results.clone())
                    .collect::<Vec<_>>();
                // Consumed tool-results count; the SAME value feeds the staging
                // disposition and the apply authorization so the two agree.
                let pending_tool_results_count =
                    u64::try_from(flattened_tool_results.len()).unwrap_or(u64::MAX);

                let resolution = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.resolve_start_turn_disposition(
                        execution_kind,
                        &prompt,
                        agent.observed_session_tail(),
                        pending_tool_results_count,
                    )
                };
                let resolution = match resolution {
                    Ok(StartTurnDispositionOutcome::Resolved(resolution)) => resolution,
                    Ok(StartTurnDispositionOutcome::ShutdownTerminal(_)) => {
                        // Machine entered ShuttingDown between claim and
                        // disposition: archive raced the admitted window.
                        // Restore deferred inputs and cancel the waiter.
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            consumed_deferred_inputs,
                        );
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::Cancelled),
                            ));
                        continue;
                    }
                    Err(error) => {
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            consumed_deferred_inputs,
                        );
                        abort_admitted_turn(&control);
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::InternalError(format!(
                                    "illegal start-turn disposition transition: {error}"
                                ))),
                            ));
                        continue;
                    }
                };
                let disposition = resolution.disposition;
                if matches!(disposition, StartTurnDisposition::NoPendingBoundary) {
                    let terminal = resolution.public_terminal;
                    restore_deferred_turn_inputs(&deferred_turn_state, consumed_deferred_inputs);
                    abort_admitted_turn(&control);
                    let result = if terminal == Some(StartTurnPublicTerminal::NoPendingBoundary) {
                        Err(meerkat_core::error::AgentError::NoPendingBoundary)
                    } else {
                        Err(meerkat_core::error::AgentError::InternalError(
                            "generated turn authority omitted NoPendingBoundary terminal witness"
                                .to_string(),
                        ))
                    };
                    let _ = result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                        result,
                    ));
                    continue;
                }
                if let Some(terminal) = resolution.public_terminal {
                    restore_deferred_turn_inputs(&deferred_turn_state, consumed_deferred_inputs);
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(
                        SessionTurnExecutionOutcome::without_machine_terminal(Err(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "generated turn authority emitted terminal {terminal:?} for runnable disposition {disposition:?}"
                            )),
                        )),
                    );
                    continue;
                }
                if matches!(disposition, StartTurnDisposition::RunPending)
                    && !injected_context.is_empty()
                {
                    // A pending continuation replays the existing boundary;
                    // there is no user message to attach injected context to.
                    // Fail closed rather than silently dropping host-provided
                    // context.
                    restore_deferred_turn_inputs(&deferred_turn_state, consumed_deferred_inputs);
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                        Err(meerkat_core::error::AgentError::ConfigError(
                            "injected_context is not supported on a pending continuation turn"
                                .to_string(),
                        )),
                    ));
                    continue;
                }
                let persist_runtime_keep_alive = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.resolve_runtime_keep_alive(keep_alive_request)
                };
                let persist_runtime_keep_alive = match persist_runtime_keep_alive {
                    Ok(RuntimeKeepAliveOutcome::Decided(decision)) => decision,
                    Ok(RuntimeKeepAliveOutcome::ShutdownTerminal(_)) => {
                        // Machine entered ShuttingDown between disposition and
                        // keep-alive: archive raced in the admitted window.
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            consumed_deferred_inputs,
                        );
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::Cancelled),
                            ));
                        continue;
                    }
                    Err(error) => {
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            consumed_deferred_inputs,
                        );
                        abort_admitted_turn(&control);
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::InternalError(format!(
                                    "generated turn authority rejected keep_alive intent: {error}"
                                ))),
                            ));
                        continue;
                    }
                };

                agent.set_skill_references(skill_references);
                if let Err(error) = agent.set_turn_tool_overlay(turn_tool_overlay) {
                    restore_deferred_turn_inputs(&deferred_turn_state, consumed_deferred_inputs);
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                        Err(error),
                    ));
                    continue;
                }
                // The canonical SessionDocumentMachine owns the authorized
                // apply count; the agent's apply becomes the gated effect
                // handler. Fail closed: a machine drive error, an absent verdict,
                // or a count that disagrees with the staged disposition takes the
                // same pre-run failure path as a rejected apply (and never
                // commits the pending results).
                let apply_authorization = {
                    let mut authority = SessionDocumentMachineAuthority::new();
                    authority
                        .apply_pending_tool_results(
                            SessionDocumentKey::new(session_id.to_string()),
                            pending_tool_results_count,
                        )
                        .map_err(|err| {
                            meerkat_core::error::AgentError::InternalError(format!(
                                "generated session document authority rejected pending \
                                 tool-results apply: {err}"
                            ))
                        })
                        .and_then(|effects| {
                            effects
                                .iter()
                                .find_map(|effect| match effect {
                                    SessionDocumentEffect::SessionToolResultsApplied {
                                        applied_count,
                                        ..
                                    } => Some(*applied_count),
                                    _ => None,
                                })
                                .ok_or_else(|| {
                                    meerkat_core::error::AgentError::InternalError(
                                        "generated session document authority returned no \
                                         pending tool-results apply verdict"
                                            .to_string(),
                                    )
                                })
                        })
                        .and_then(|applied_count| {
                            if applied_count == pending_tool_results_count {
                                Ok(())
                            } else {
                                Err(meerkat_core::error::AgentError::InternalError(format!(
                                    "generated session document authority authorized \
                                     {applied_count} pending tool-results but the staged \
                                     disposition consumed {pending_tool_results_count}"
                                )))
                            }
                        })
                };
                let apply_result = match apply_authorization {
                    Ok(()) => agent.apply_pending_tool_results(flattened_tool_results),
                    Err(error) => Err(error),
                };
                if let Err(error) = apply_result {
                    let _ = agent.set_turn_tool_overlay(None);
                    restore_deferred_turn_inputs(&deferred_turn_state, consumed_deferred_inputs);
                    abort_admitted_turn(&control);
                    let _ = result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                        Err(error),
                    ));
                    continue;
                }
                match persist_runtime_keep_alive {
                    crate::turn_admission::RuntimeKeepAlivePersistenceDecision::PersistEnabled => {
                        agent.update_keep_alive(true);
                    }
                    crate::turn_admission::RuntimeKeepAlivePersistenceDecision::PersistDisabled => {
                        agent.update_keep_alive(false);
                    }
                    crate::turn_admission::RuntimeKeepAlivePersistenceDecision::PreserveExisting => {}
                }
                let begin_outcome = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.begin().map(|outcome| (outcome, slot.projection()))
                };
                match begin_outcome {
                    Ok((BeginOutcome::Running, projection)) => {
                        control.state_tx.send_replace(projection);
                        let system_append = if system_messages.is_empty() {
                            Ok(())
                        } else {
                            agent.append_system_messages(system_messages)
                        };
                        if let Err(error) = system_append {
                            let _ = agent.set_turn_tool_overlay(None);
                            let resolve_projection = {
                                let mut slot = lock_turn_admission(&control.turn_admission);
                                slot.resolve().ok().map(|_| slot.projection())
                            };
                            if let Some(projection) = resolve_projection {
                                control.state_tx.send_replace(projection);
                            }
                            let finalize = {
                                let mut slot = lock_turn_admission(&control.turn_admission);
                                slot.finalize().map(|outcome| (outcome, slot.projection()))
                            };
                            let shutting_down = match finalize {
                                Ok((outcome, projection)) => {
                                    control.state_tx.send_replace(projection);
                                    outcome.next_phase == TurnAdmissionPhase::ShuttingDown
                                }
                                Err(finalize_error) => {
                                    tracing::error!(
                                        error = %finalize_error,
                                        "failed to finalize turn after System-message append refusal"
                                    );
                                    false
                                }
                            };
                            drop(active_admission);
                            let _ = result_tx.send(
                                SessionTurnExecutionOutcome::without_machine_terminal(Err(error)),
                            );
                            if shutting_down {
                                drain_session_task_commands(
                                    &mut commands,
                                    &mut agent,
                                    &session_id,
                                    &control,
                                    &mut next_seq,
                                    &source,
                                    &mut transcript_authority_generation,
                                )
                                .await;
                                break;
                            }
                            continue;
                        }
                        // The run is genuinely beginning: commit any in-flight
                        // staged->active promotion so the registry-owned
                        // status flips `Promoting -> Active` and the final
                        // lease release returns capacity to the gate instead
                        // of restoring the staged reservation.
                        if let Some(admission) = active_admission.as_ref() {
                            admission.commit_promotion();
                        }
                    }
                    Ok((BeginOutcome::ShutdownTerminal(_), _projection)) => {
                        // Machine entered ShuttingDown between keep-alive and
                        // begin: archive yanked the admitted window just before
                        // the run would have started.
                        let _ = agent.set_turn_tool_overlay(None);
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            consumed_deferred_inputs,
                        );
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::Cancelled),
                            ));
                        continue;
                    }
                    Err(error) => {
                        let _ = agent.set_turn_tool_overlay(None);
                        restore_deferred_turn_inputs(
                            &deferred_turn_state,
                            consumed_deferred_inputs,
                        );
                        let _ =
                            result_tx.send(SessionTurnExecutionOutcome::without_machine_terminal(
                                Err(meerkat_core::error::AgentError::InternalError(format!(
                                    "illegal begin-run transition: {error}"
                                ))),
                            ));
                        continue;
                    }
                }
                let mut event_stream_open = true;

                // Scope the pinned future so its mutable borrow of `agent` is
                // released before we call `agent.snapshot()`.
                let (result, resolved_projection, interrupted) = {
                    #[cfg(not(target_arch = "wasm32"))]
                    type RunFut<'a> = std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<RunResult, meerkat_core::error::AgentError>,
                                > + Send
                                + 'a,
                        >,
                    >;
                    #[cfg(target_arch = "wasm32")]
                    type RunFut<'a> = std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<RunResult, meerkat_core::error::AgentError>,
                                > + 'a,
                        >,
                    >;
                    // Dispatch based on the canonical disposition computed above.
                    // NoPendingBoundary was already handled before Running state.
                    let run_fut: RunFut<'_> = match disposition {
                        StartTurnDisposition::RunContentTurn => {
                            Box::pin(agent.run_turn_with_events(
                                SessionAgentTurnInput {
                                    prompt,
                                    injected_context,
                                    handling_mode,
                                    render_metadata,
                                    typed_turn_appends,
                                    transcript_identity,
                                    execution_kind,
                                },
                                agent_event_tx.clone(),
                            ))
                        }
                        StartTurnDisposition::RunPending => {
                            Box::pin(agent.run_pending_with_events(
                                transcript_identity,
                                execution_kind,
                                agent_event_tx.clone(),
                            ))
                        }
                        StartTurnDisposition::NoPendingBoundary => {
                            // Already handled above — unreachable here.
                            unreachable!("NoPendingBoundary handled before Running state")
                        }
                    };
                    // run_fut is already Pin<Box<...>>, no tokio::pin! needed.
                    let mut run_fut = run_fut;
                    let mut interrupted = false;
                    let mut resolved_projection = None;

                    let r = loop {
                        if lock_turn_admission(&control.turn_admission).interrupt_pending() {
                            interrupted = true;
                            break Err(meerkat_core::error::AgentError::Cancelled);
                        }
                        let interrupt_wait = control.interrupt_notify.notified();
                        tokio::pin!(interrupt_wait);
                        if lock_turn_admission(&control.turn_admission).interrupt_pending() {
                            interrupted = true;
                            break Err(meerkat_core::error::AgentError::Cancelled);
                        }
                        tokio::select! {
                            result = &mut run_fut => {
                                let mut slot = lock_turn_admission(&control.turn_admission);
                                if slot.interrupt_pending() {
                                    interrupted = true;
                                    break Err(meerkat_core::error::AgentError::Cancelled);
                                }
                                resolved_projection = slot.resolve().ok().map(|_| slot.projection());
                                break result;
                            }
                            () = &mut interrupt_wait => {
                                let interrupt_pending =
                                    lock_turn_admission(&control.turn_admission).interrupt_pending();
                                if interrupt_pending {
                                    interrupted = true;
                                    break Err(meerkat_core::error::AgentError::Cancelled);
                                }
                            }
                            Some(event) = agent_event_rx.recv() => {
                                let envelope = stamp_event_envelope(
                                    &mut next_seq,
                                    &source,
                                    event,
                                );
                                control.publish_session_event(envelope.clone()).await;
                                if event_stream_open
                                    && let Some(ref tx) = event_tx
                                    && tx.send(envelope).await.is_err()
                                {
                                    event_stream_open = false;
                                    tracing::warn!("session event stream receiver dropped; continuing without streaming events");
                                }
                            }
                        }
                    };
                    drop(run_fut);
                    if interrupted {
                        agent.cancel();
                    }

                    // Drain any remaining events
                    while let Ok(event) = agent_event_rx.try_recv() {
                        let envelope = stamp_event_envelope(&mut next_seq, &source, event);
                        control.publish_session_event(envelope.clone()).await;
                        if event_stream_open
                            && let Some(ref tx) = event_tx
                            && tx.send(envelope).await.is_err()
                        {
                            event_stream_open = false;
                            tracing::warn!(
                                "session event stream receiver dropped while draining events"
                            );
                        }
                    }

                    (r, resolved_projection, interrupted)
                }; // run_fut dropped here

                let (result, mut post_run_cleanup_failed) =
                    match agent.settle_inflight_sticky_model_fallback().await {
                        Ok(()) => (result, false),
                        Err(error) => (Err(error), true),
                    };
                let result = if interrupted {
                    match agent.abort_uncommitted_compaction_projections().await {
                        Ok(()) => result,
                        Err(abort_error) => {
                            post_run_cleanup_failed = true;
                            match result {
                                Err(primary) => Err(primary.with_ancillary_failure(
                                    "failed to abort hard-interrupted compaction projection",
                                    abort_error,
                                )),
                                Ok(_) => Err(abort_error),
                            }
                        }
                    }
                } else {
                    result
                };
                if let Some(identity) = agent.durable_llm_identity() {
                    control.llm_identity_tx.send_replace(identity);
                }

                let (result, result_identity_failed) = match result {
                    Ok(run_result) if run_result.session_id != session_id => (
                        Err(meerkat_core::error::AgentError::InternalError(format!(
                            "agent turn returned session {}, expected {session_id}",
                            run_result.session_id
                        ))),
                        true,
                    ),
                    result => (result, false),
                };
                let machine_terminal_failure = agent.take_runtime_terminal_failure_witness();
                let resolve_projection = resolved_projection.or_else(|| {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    slot.resolve().ok().map(|_| slot.projection())
                });
                if let Some(projection) = resolve_projection {
                    control.state_tx.send_replace(projection);
                }

                // Update cached summary
                let snap = agent.snapshot();
                control.publish_summary(SessionSummaryCache {
                    updated_at: snap.updated_at,
                    message_count: snap.message_count,
                    total_tokens: snap.total_tokens,
                    usage: snap.usage,
                    last_assistant_text: snap.last_assistant_text,
                });

                // Release the turn lock AFTER setting state to Idle and
                // updating the summary, so the next caller sees consistent state.
                let (result, cleanup_failed) = if let Err(error) = agent.set_turn_tool_overlay(None)
                {
                    tracing::error!(
                        error = %error,
                        "failed to clear turn tool overlay; failing turn to avoid stale scope"
                    );
                    let result = match result {
                        Err(primary) if primary.requires_session_teardown() => Err(primary
                            .with_ancillary_failure("failed to clear turn tool overlay", &error)),
                        _ => Err(error),
                    };
                    (result, true)
                } else {
                    (result, result_identity_failed || post_run_cleanup_failed)
                };
                let finalize = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    let finalize = slot.finalize();
                    finalize.map(|outcome| (outcome, slot.projection()))
                };
                let shutting_down = match finalize {
                    Ok((outcome, projection)) => {
                        control.state_tx.send_replace(projection);
                        outcome.next_phase == TurnAdmissionPhase::ShuttingDown
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            "failed to finalize session turn admission state"
                        );
                        false
                    }
                };
                drop(active_admission);
                let _ = result_tx.send(SessionTurnExecutionOutcome {
                    result,
                    machine_terminal_failure: if cleanup_failed {
                        Ok(None)
                    } else {
                        machine_terminal_failure
                    },
                });
                if shutting_down {
                    // D1: drain queued admission work before exiting.
                    drain_session_task_commands(
                        &mut commands,
                        &mut agent,
                        &session_id,
                        &control,
                        &mut next_seq,
                        &source,
                        &mut transcript_authority_generation,
                    )
                    .await;
                    break;
                }
            }
            SessionCommand::ExportSession { reply_tx } => {
                let _ = reply_tx.send(agent.session_clone());
            }
            SessionCommand::ObserveSessionTranscriptAuthority { reply_tx } => {
                let result = agent.session_transcript_authority().map(|snapshot| {
                    snapshot.bind_actor_generation(transcript_authority_generation)
                });
                let _ = reply_tx.send(result);
            }
            SessionCommand::ExportSessionIfTranscriptAuthority { expected, reply_tx } => {
                let result = agent
                    .session_transcript_authority()
                    .map(|snapshot| snapshot.bind_actor_generation(transcript_authority_generation))
                    .and_then(|current| {
                        if current == expected {
                            agent.session_clone().map(Some)
                        } else {
                            Ok(None)
                        }
                    });
                let _ = reply_tx.send(result);
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::ClassifyCallbackResultIngress { results, reply_tx } => {
                let _ = reply_tx.send(agent.classify_callback_result_ingress(&results));
            }
            SessionCommand::ReconcileRuntimeCompactionProjections { intents, reply_tx } => {
                let result = agent
                    .reconcile_runtime_compaction_projections(&intents)
                    .await;
                let _ = reply_tx.send(result);
            }
            SessionCommand::AbortUncommittedCompactionProjections { reply_tx } => {
                let result = agent.abort_uncommitted_compaction_projections().await;
                let _ = reply_tx.send(result);
            }
            SessionCommand::ExecutionSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.execution_snapshot());
            }
            SessionCommand::ToolScopeSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.tool_scope_snapshot());
            }
            SessionCommand::VisibleToolDefs { reply_tx } => {
                let _ = reply_tx.send(agent.visible_tool_defs());
            }
            SessionCommand::ExternalToolSurfaceSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.external_tool_surface_snapshot());
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::PublishInteractionTerminalsExactBatch {
                expected_actor,
                events,
                event_store,
                reply_tx,
            } => {
                let result = publish_interaction_terminal_batch(
                    &session_id,
                    &mut next_seq,
                    &control,
                    event_store.as_ref(),
                    expected_actor.as_ref(),
                    events,
                )
                .await;
                let _ = reply_tx.send(result);
            }
            SessionCommand::RecordLiveTerminalError { cause, reply_tx } => {
                let message = render_live_terminal_error_message(&cause);
                let failed = stamp_event_envelope(
                    &mut next_seq,
                    &source,
                    AgentEvent::RunFailed {
                        session_id: session_id.clone(),
                        terminal_cause_kind: None,
                        error_report: meerkat_core::event::AgentErrorReport {
                            class: meerkat_core::event::AgentErrorClass::Terminal,
                            reason: None,
                            message,
                        },
                    },
                );
                control.publish_session_event(failed).await;
                let _ = reply_tx.send(());
            }
            SessionCommand::RecordLiveOutputAudioDegraded { dropped, reply_tx } => {
                let truncated = stamp_event_envelope(
                    &mut next_seq,
                    &source,
                    AgentEvent::StreamTruncated {
                        reason: meerkat_core::event::StreamTruncationReason::OutputAudioDegraded {
                            dropped,
                        },
                    },
                );
                control.publish_session_event(truncated).await;
                let _ = reply_tx.send(());
            }
            SessionCommand::AppendExternalUserContent { content, reply_tx } => {
                let result = agent.append_external_user_content(content);
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::AppendExternalAssistantOutput {
                blocks,
                stop_reason,
                usage,
                reply_tx,
            } => {
                // Project both `Text` (display) and `Transcript` (spoken)
                // lanes to the rendered text stream — they share the same
                // surface for end-user display.
                let text_content = blocks
                    .iter()
                    .filter_map(|block| match block {
                        meerkat_core::types::AssistantBlock::Text { text, .. }
                        | meerkat_core::types::AssistantBlock::Transcript { text, .. } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<String>();
                let usage_for_event = meerkat_core::TurnUsage::try_from_usage(usage.clone()).ok();
                let result = agent.append_external_assistant_output(blocks, stop_reason, usage);
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                    if !text_content.is_empty() {
                        let envelope = stamp_event_envelope(
                            &mut next_seq,
                            &source,
                            AgentEvent::TextComplete {
                                content: text_content,
                            },
                        );
                        control.publish_session_event(envelope).await;
                    }
                    // The completion is published whether or not the host
                    // supplied normalized accounting. Withholding it when the
                    // number is missing would let an accounting gap erase a
                    // semantic fact the caller has already observed.
                    let envelope = stamp_event_envelope(
                        &mut next_seq,
                        &source,
                        AgentEvent::TurnCompleted {
                            stop_reason,
                            usage: usage_for_event,
                        },
                    );
                    control.publish_session_event(envelope).await;
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::AppendRealtimeTranscriptEvent { event, reply_tx } => {
                let result = agent.append_realtime_transcript_event(event);
                if let Ok(outcome) = &result {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                    for materialized in &outcome.materialized_messages {
                        if let RealtimeTranscriptMaterializedMessage::Assistant {
                            text,
                            stop_reason,
                            usage,
                            ..
                        } = materialized
                        {
                            if !text.is_empty() {
                                let envelope = stamp_event_envelope(
                                    &mut next_seq,
                                    &source,
                                    AgentEvent::TextComplete {
                                        content: text.clone(),
                                    },
                                );
                                control.publish_session_event(envelope).await;
                            }
                            let envelope = stamp_event_envelope(
                                &mut next_seq,
                                &source,
                                AgentEvent::TurnCompleted {
                                    stop_reason: *stop_reason,
                                    usage: usage.clone(),
                                },
                            );
                            control.publish_session_event(envelope).await;
                        }
                    }
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::AdmitLiveAssistantPlaybackTarget {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                reply_tx,
            } => {
                let result = crate::live_transcript_authority::admit_live_assistant_playback_target(
                    &mut agent,
                    &session_id,
                    channel_id,
                    interaction_id,
                    response_id,
                    item_id,
                    content_index,
                );
                let _ = reply_tx.send(result);
            }
            SessionCommand::ResolveLiveAssistantPlaybackTarget {
                channel_id,
                item_id,
                content_index,
                reply_tx,
            } => {
                let target =
                    agent.live_assistant_playback_target(&channel_id, &item_id, content_index);
                let _ = reply_tx.send(target);
            }
            SessionCommand::ResolveLiveAssistantPlaybackOnChannelClose {
                channel_id,
                reply_tx,
            } => {
                let result = crate::live_transcript_authority::resolve_live_assistant_playback_on_channel_close(
                    &mut agent,
                    &session_id,
                    channel_id,
                );
                let _ = reply_tx.send(result);
            }
            SessionCommand::CommitLiveUserTranscriptFinal {
                provisional,
                final_event,
                reply_tx,
            } => {
                let result = crate::live_transcript_authority::commit_final_live_user_transcript(
                    &mut agent,
                    &session_id,
                    provisional,
                    final_event,
                );
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::CommitLiveAssistantPlaybackTruncation {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                evidence,
                reply_tx,
            } => {
                let result =
                    crate::live_transcript_authority::commit_live_assistant_playback_truncation(
                        &mut agent,
                        &session_id,
                        channel_id,
                        interaction_id,
                        response_id,
                        item_id,
                        content_index,
                        evidence,
                    );
                if matches!(
                    result.as_ref().map(|receipt| receipt.disposition()),
                    Ok(meerkat_core::LiveAssistantPlaybackTruncationDisposition::CommittedReportedPrefix)
                ) {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::CommitLiveAssistantPlaybackComplete {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                stop_reason,
                usage,
                reply_tx,
            } => {
                let result =
                    crate::live_transcript_authority::commit_live_assistant_playback_complete(
                        &mut agent,
                        &session_id,
                        channel_id,
                        interaction_id,
                        response_id,
                        item_id,
                        content_index,
                        stop_reason,
                        usage,
                    );
                let _ = reply_tx.send(result);
            }
            SessionCommand::ObserveLiveAssistantPlaybackTerminal {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                evidence,
                stop_reason,
                usage,
                reply_tx,
            } => {
                let result = crate::live_transcript_authority::observe_live_assistant_playback_terminal_with_completion(
                    &mut agent,
                    &session_id,
                    channel_id,
                    interaction_id,
                    response_id,
                    item_id,
                    content_index,
                    evidence,
                    stop_reason,
                    usage,
                );
                let _ = reply_tx.send(result);
            }
            SessionCommand::ObserveLiveAssistantPlaybackFinal {
                channel_id,
                interaction_id,
                response_id,
                item_id,
                content_index,
                reply_tx,
            } => {
                let result =
                    crate::live_transcript_authority::observe_live_assistant_playback_final(
                        &mut agent,
                        &session_id,
                        channel_id,
                        interaction_id,
                        response_id,
                        item_id,
                        content_index,
                    );
                let _ = reply_tx.send(result);
            }
            SessionCommand::DispatchExternalToolCall {
                call,
                timeout_policy,
                reply_tx,
            } => {
                let result = agent
                    .dispatch_external_tool_call_with_timeout_policy(call, timeout_policy)
                    .await;
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::UpdateMobToolAuthority {
                authority_context,
                reply_tx,
            } => {
                let _ = reply_tx.send(agent.update_mob_tool_authority_context(authority_context));
            }
            SessionCommand::AppendSystemMessageControl { req, reply_tx } => {
                let result = match control.archive_snapshot_gate.enter_apply() {
                    Ok(_gate) => agent.append_system_message_control(req),
                    Err(error) => Err(AgentError::InternalError(error.to_string())),
                };
                if result.is_ok() {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            SessionCommand::ActivateInstructionControl { request, reply_tx } => {
                let result = match control.archive_snapshot_gate.enter_apply() {
                    Ok(_gate) => Ok(agent.activate_instruction_control(request)),
                    Err(error) => Err(AgentError::InternalError(error.to_string())),
                };
                if matches!(result, Ok(Ok(_))) {
                    let snap = agent.snapshot();
                    control.publish_summary(SessionSummaryCache {
                        updated_at: snap.updated_at,
                        message_count: snap.message_count,
                        total_tokens: snap.total_tokens,
                        usage: snap.usage,
                        last_assistant_text: snap.last_assistant_text,
                    });
                }
                let _ = reply_tx.send(result);
            }
            SessionCommand::PrepareHeadCanonicalRuntimeBoundary { request, reply_tx } => {
                let result = agent.prepare_head_canonical_runtime_boundary(request).await;
                let _ = reply_tx.send(result);
            }
            SessionCommand::AcknowledgeHeadCanonicalRuntimeBoundary {
                successor_head_token,
                reply_tx,
            } => {
                let result =
                    agent.acknowledge_head_canonical_runtime_boundary(&successor_head_token);
                let _ = reply_tx.send(result);
            }
            SessionCommand::Shutdown => {
                let next_projection = {
                    let mut slot = lock_turn_admission(&control.turn_admission);
                    let next_phase = slot.request_shutdown().ok();
                    next_phase.map(|_| slot.projection())
                };
                if let Some(projection) = next_projection {
                    control.state_tx.send_replace(projection);
                }
                // D1: drain queued admission work before exiting.
                drain_session_task_commands(
                    &mut commands,
                    &mut agent,
                    &session_id,
                    &control,
                    &mut next_seq,
                    &source,
                    &mut transcript_authority_generation,
                )
                .await;
                break;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod runtime_turn_metadata_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::handles::TurnStateHandle;
    use meerkat_core::lifecycle::RuntimeExecutionKind;
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
    };
    use meerkat_core::skills::{SkillKey, SkillName};
    use meerkat_runtime::handles::RuntimeTurnStateHandle;
    use std::sync::{Arc, Mutex};

    #[test]
    fn success_with_machine_terminal_witness_is_rejected_for_every_session_backend() {
        let outcome = SessionTurnExecutionOutcome {
            result: Ok(RunResult {
                text: "impossible success".to_string(),
                session_id: SessionId::new(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            }),
            machine_terminal_failure: Ok(Some(meerkat_core::TurnErrorMetadata::terminal(
                meerkat_core::TurnTerminalCauseKind::ToolFailure,
                meerkat_core::TurnTerminalOutcome::Failed,
                "forged witness",
            ))),
        };

        let error = outcome
            .into_runtime_parts()
            .expect_err("success plus a failure witness must fail closed");
        assert!(
            error
                .to_string()
                .contains("success with a machine-terminal failure witness")
        );
    }

    fn test_llm_identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    #[derive(Clone)]
    struct MetadataProbeBuilder {
        observed_skill_references: Arc<Mutex<Vec<Option<Vec<SkillKey>>>>>,
        observed_context_texts: Arc<Mutex<Vec<String>>>,
        run_context_counts: Arc<Mutex<Vec<usize>>>,
        fail_flow_overlay_set: bool,
        session_context_handle: Option<Arc<RecordingSessionContextHandle>>,
    }

    struct MetadataProbeAgent {
        session_id: SessionId,
        session: meerkat_core::Session,
        identity: SessionLlmIdentity,
        observed_skill_references: Arc<Mutex<Vec<Option<Vec<SkillKey>>>>>,
        observed_context_texts: Arc<Mutex<Vec<String>>>,
        run_context_counts: Arc<Mutex<Vec<usize>>>,
        fail_flow_overlay_set: bool,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
        session_context_handle: Option<Arc<RecordingSessionContextHandle>>,
    }

    #[derive(Debug, Default)]
    struct RecordingSessionContextHandle {
        ticks: Mutex<Vec<u64>>,
    }

    impl meerkat_core::handles::SessionContextHandle for RecordingSessionContextHandle {
        fn context_advanced(
            &self,
            updated_at_ms: u64,
        ) -> Result<bool, meerkat_core::handles::DslTransitionError> {
            let mut ticks = self
                .ticks
                .lock()
                .expect("session context ticks lock poisoned");
            if ticks
                .last()
                .is_some_and(|current| updated_at_ms <= *current)
            {
                return Ok(false);
            }
            ticks.push(updated_at_ms);
            Ok(true)
        }

        fn current_watermark_ms(&self) -> u64 {
            self.ticks
                .lock()
                .expect("session context ticks lock poisoned")
                .last()
                .copied()
                .unwrap_or(0)
        }

        fn install_observer(
            &self,
            _observer: Arc<dyn meerkat_core::handles::SessionContextAdvancedObserver>,
        ) {
        }

        fn install_observer_with_baseline(
            &self,
            _observer: Arc<dyn meerkat_core::handles::SessionContextAdvancedObserver>,
        ) -> u64 {
            // Recording handle never dispatches observer callbacks; the
            // watermark read is the entire critical section.
            self.current_watermark_ms()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for MetadataProbeBuilder {
        type Agent = MetadataProbeAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let session_id = session.id().clone();
            Ok(MetadataProbeAgent {
                session_id,
                session,
                identity: test_llm_identity(&req.model),
                observed_skill_references: Arc::clone(&self.observed_skill_references),
                observed_context_texts: Arc::clone(&self.observed_context_texts),
                run_context_counts: Arc::clone(&self.run_context_counts),
                fail_flow_overlay_set: self.fail_flow_overlay_set,
                transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
                session_context_handle: self.session_context_handle.clone(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for MetadataProbeAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            self.run_context_counts
                .lock()
                .expect("run context counts lock poisoned")
                .push(
                    self.observed_context_texts
                        .lock()
                        .expect("observed context texts lock poisoned")
                        .len(),
                );
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, refs: Option<Vec<SkillKey>>) {
            self.observed_skill_references
                .lock()
                .expect("observed skill references lock poisoned")
                .push(refs);
        }

        fn set_turn_tool_overlay(
            &mut self,
            overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            if self.fail_flow_overlay_set && overlay.is_some() {
                return Err(AgentError::ConfigError(
                    "synthetic flow overlay failure".to_string(),
                ));
            }
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            Ok(self.session.clone())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            SessionTranscriptAuthoritySnapshot::from_session(&self.session)
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient_turn_context_state.clone()
        }

        #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
        fn sync_session_from_durable_snapshot(
            &mut self,
            session: meerkat_core::Session,
        ) -> Result<(), AgentError> {
            if session.id() != &self.session_id {
                return Err(AgentError::InternalError(format!(
                    "snapshot session id {} did not match live session {}",
                    session.id(),
                    self.session_id
                )));
            }
            self.session = session;
            Ok(())
        }

        fn session_context_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::handles::SessionContextHandle>> {
            self.session_context_handle.as_ref().map(|handle| {
                Arc::clone(handle) as Arc<dyn meerkat_core::handles::SessionContextHandle>
            })
        }
    }

    #[tokio::test]
    async fn absent_session_compaction_reconciliation_requires_builder_owned_empty_authority_seam()
    {
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::new(Mutex::new(Vec::new())),
                observed_context_texts: Arc::new(Mutex::new(Vec::new())),
                run_context_counts: Arc::new(Mutex::new(Vec::new())),
                fail_flow_overlay_set: false,
                session_context_handle: None,
            },
            1,
        );
        let missing_session_id = SessionId::new();

        let empty_error = service
            .reconcile_runtime_compaction_projections(&missing_session_id, Vec::new())
            .await
            .expect_err("generic builders must not silently ignore empty runtime authority");
        assert!(
            matches!(empty_error, SessionError::Unsupported(_)),
            "an absent session needs an explicit builder-owned durable-memory seam: {empty_error}"
        );

        let intent = meerkat_core::CompactionProjectionIntent {
            projection: serde_json::from_value(serde_json::json!({
                "session_id": &missing_session_id,
                "parent_revision": "missing-parent",
                "revision": "missing-revision",
                "commit_fingerprint": "sha256:absent-session-regression-fixture",
            }))
            .expect("valid projection fixture"),
            summary_tokens: 1,
            messages_before: 2,
            messages_after: 1,
        };
        let error = service
            .reconcile_runtime_compaction_projections(&missing_session_id, vec![intent])
            .await
            .expect_err("non-empty runtime outbox requires its live session agent");
        assert!(
            matches!(error, SessionError::NotFound { ref id } if id == &missing_session_id),
            "non-empty reconciliation must fail closed for an absent session: {error}"
        );
    }

    #[derive(Clone)]
    struct BoundaryPhaseProbeBuilder {
        turn_state: Arc<RuntimeTurnStateHandle>,
    }

    struct BoundaryPhaseProbeAgent {
        session_id: SessionId,
        identity: SessionLlmIdentity,
        turn_state: Arc<RuntimeTurnStateHandle>,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for BoundaryPhaseProbeBuilder {
        type Agent = BoundaryPhaseProbeAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(BoundaryPhaseProbeAgent {
                session_id: SessionId::new(),
                identity: test_llm_identity(&req.model),
                turn_state: Arc::clone(&self.turn_state),
                transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for BoundaryPhaseProbeAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn cancel(&mut self) {}

        fn set_skill_references(&mut self, _refs: Option<Vec<SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            Ok(meerkat_core::Session::with_id(self.session_id.clone()))
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            let session = self.session_clone()?;
            SessionTranscriptAuthoritySnapshot::from_session(&session)
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn turn_state_handle(&self) -> Option<Arc<dyn TurnStateHandle>> {
            let handle: Arc<dyn TurnStateHandle> = self.turn_state.clone();
            Some(handle)
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient_turn_context_state.clone()
        }
    }

    #[tokio::test]
    async fn materialization_status_is_owned_by_registry_across_lifecycle() {
        let turn_state = Arc::new(RuntimeTurnStateHandle::ephemeral());
        let service = EphemeralSessionService::new(
            BoundaryPhaseProbeBuilder {
                turn_state: Arc::clone(&turn_state),
            },
            1,
        );

        // A deferred-first-turn session reserves capacity and is recorded as
        // `Staged` by the single registry owner — not re-derived from
        // permit-existence plus handle-existence.
        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "phase-probe".to_string(),
                prompt: ContentInput::Text("hello".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create deferred session");

        assert_eq!(
            service.materialization_status(&created.session_id),
            Some(MaterializationStatus::Staged),
            "deferred session is recorded Staged by the registry"
        );

        // The single global permit is held by the staged reservation, so a
        // second session must fail closed rather than be admitted silently.
        let second = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "phase-probe".to_string(),
                prompt: ContentInput::Text("again".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await;
        let err = second.expect_err("capacity gate must reject the second session");
        assert!(
            format!("{err}").contains("Max sessions reached"),
            "expected typed capacity-exhaustion error, got: {err}"
        );

        // Archiving the session clears the typed status through the same owner
        // and frees the capacity permit.
        service
            .archive(&created.session_id)
            .await
            .expect("archive deferred session");
        assert_eq!(
            service.materialization_status(&created.session_id),
            None,
            "archive clears the registry status with the handle"
        );
        service
            .ensure_active_capacity_available()
            .expect("capacity freed after archive");
    }

    #[tokio::test]
    async fn eager_initial_turn_forwards_full_runtime_metadata_carrier() {
        let observed_skill_references = Arc::new(Mutex::new(Vec::new()));
        let observed_context_texts = Arc::new(Mutex::new(Vec::new()));
        let run_context_counts = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::clone(&observed_skill_references),
                observed_context_texts,
                run_context_counts,
                fail_flow_overlay_set: false,
                session_context_handle: None,
            },
            1,
        );
        let skill = SkillKey::builtin(SkillName::parse("metadata-probe").expect("valid skill"));

        service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("hello".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    initial_turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        skill_references: Some(vec![skill.clone()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("eager first turn should run");

        assert_eq!(
            *observed_skill_references
                .lock()
                .expect("observed skill references lock poisoned"),
            vec![Some(vec![skill])],
            "eager first turn must forward the full runtime metadata carrier"
        );
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn durable_snapshot_sync_preserves_live_deferred_turn_handle_without_authority_state() {
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::new(Mutex::new(Vec::new())),
                observed_context_texts: Arc::new(Mutex::new(Vec::new())),
                run_context_counts: Arc::new(Mutex::new(Vec::new())),
                fail_flow_overlay_set: false,
                session_context_handle: None,
            },
            1,
        );
        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("staged".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Stage,
                build: None,
                labels: None,
            })
            .await
            .expect("deferred session should create");
        let deferred_state = service
            .deferred_turn_state(&created.session_id)
            .await
            .expect("deferred-turn state handle should exist");
        assert_eq!(
            lock_deferred_turn_state(&deferred_state).first_turn_phase(),
            DeferredFirstTurnPhase::Pending
        );

        let before_sync = service
            .observe_session_transcript_authority(&created.session_id)
            .await
            .expect("pre-sync transcript authority");
        let durable = meerkat_core::Session::with_id(created.session_id.clone());
        service
            .sync_session_from_durable_snapshot(&created.session_id, durable)
            .await
            .expect("durable sync should succeed");
        let after_sync = service
            .observe_session_transcript_authority(&created.session_id)
            .await
            .expect("post-sync transcript authority");

        assert_eq!(
            before_sync.transcript_revision(),
            after_sync.transcript_revision(),
            "fixture must exercise equal-revision whole-Session replacement"
        );
        assert_eq!(before_sync.message_count(), after_sync.message_count());
        assert_ne!(
            before_sync.mutation_generation(),
            after_sync.mutation_generation(),
            "SessionTask generation must survive equal-body Session replacement"
        );
        assert!(
            service
                .export_session_if_transcript_authority(&created.session_id, before_sync)
                .await
                .expect("stale conditional export")
                .is_none(),
            "pre-replacement authority must not export the replacement Session"
        );
        assert!(
            service
                .export_session_if_transcript_authority(&created.session_id, after_sync)
                .await
                .expect("current conditional export")
                .is_some(),
            "current actor generation should export atomically"
        );

        let guard = lock_deferred_turn_state(&deferred_state);
        assert_eq!(guard.first_turn_phase(), DeferredFirstTurnPhase::Pending);
        assert!(
            guard.pending_initial_prompt().is_some(),
            "missing durable authority state must not replace staged live deferred prompt"
        );
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn durable_snapshot_sync_updates_live_deferred_turn_handle_from_authority_state() {
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::new(Mutex::new(Vec::new())),
                observed_context_texts: Arc::new(Mutex::new(Vec::new())),
                run_context_counts: Arc::new(Mutex::new(Vec::new())),
                fail_flow_overlay_set: false,
                session_context_handle: None,
            },
            1,
        );
        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("staged".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Stage,
                build: None,
                labels: None,
            })
            .await
            .expect("deferred session should create");
        let deferred_state = service
            .deferred_turn_state(&created.session_id)
            .await
            .expect("deferred-turn state handle should exist");
        assert_eq!(
            lock_deferred_turn_state(&deferred_state).first_turn_phase(),
            DeferredFirstTurnPhase::Pending
        );

        let mut durable = meerkat_core::Session::with_id(created.session_id.clone());
        durable
            .set_deferred_turn_state(SessionDeferredTurnState::default())
            .expect("generated deferred-turn authority should authorize default state");
        service
            .sync_session_from_durable_snapshot(&created.session_id, durable)
            .await
            .expect("durable sync should succeed");

        let guard = lock_deferred_turn_state(&deferred_state);
        assert_eq!(guard.first_turn_phase(), DeferredFirstTurnPhase::Inactive);
        assert!(
            guard.pending_initial_prompt().is_none(),
            "explicit generated durable state should replace staged live deferred prompt"
        );
    }

    #[tokio::test]
    async fn start_turn_runtime_metadata_is_sole_skill_carrier() {
        let observed_skill_references = Arc::new(Mutex::new(Vec::new()));
        let observed_context_texts = Arc::new(Mutex::new(Vec::new()));
        let run_context_counts = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            MetadataProbeBuilder {
                observed_skill_references: Arc::clone(&observed_skill_references),
                observed_context_texts,
                run_context_counts,
                fail_flow_overlay_set: false,
                session_context_handle: None,
            },
            1,
        );
        let canonical =
            SkillKey::builtin(SkillName::parse("runtime-canonical-skill").expect("valid skill"));

        let result = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "metadata-probe-model".to_string(),
                prompt: ContentInput::Text("defer".to_string()),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions::default()),
                labels: None,
            })
            .await
            .expect("deferred session should create");

        service
            .start_turn(
                &result.session_id,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: ContentInput::Text("go".to_string()),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        meerkat_core::types::HandlingMode::Queue,
                        None,
                        Some(RuntimeTurnMetadata {
                            execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                            skill_references: Some(vec![canonical.clone()]),
                            ..Default::default()
                        }),
                    ),
                },
            )
            .await
            .expect("turn should run with canonical runtime metadata");

        assert_eq!(
            *observed_skill_references
                .lock()
                .expect("observed skill references lock poisoned"),
            vec![Some(vec![canonical])],
            "canonical RuntimeTurnMetadata must be the only skill carrier once present"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod injected_context_turn_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
    };
    use std::sync::{Arc, Mutex};

    fn probe_llm_identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    /// Builder for a probe agent that overrides `run_turn_with_events` and
    /// records each turn's `(prompt, injected_context)` pair, so tests can
    /// assert the service threads the typed injected-context carrier through
    /// the session task in delivery order.
    #[derive(Clone)]
    struct InjectedContextProbeBuilder {
        observed_turns: Arc<Mutex<Vec<(String, Vec<String>)>>>,
    }

    struct InjectedContextProbeAgent {
        session_id: SessionId,
        session: meerkat_core::Session,
        identity: SessionLlmIdentity,
        observed_turns: Arc<Mutex<Vec<(String, Vec<String>)>>>,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for InjectedContextProbeBuilder {
        type Agent = InjectedContextProbeAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            let session_id = session.id().clone();
            Ok(InjectedContextProbeAgent {
                session_id,
                session,
                identity: probe_llm_identity(&req.model),
                observed_turns: Arc::clone(&self.observed_turns),
                transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
            })
        }
    }

    impl InjectedContextProbeAgent {
        fn ok_result(&self) -> RunResult {
            RunResult {
                text: "ok".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for InjectedContextProbeAgent {
        async fn run_with_events(
            &mut self,
            prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            self.session.push(meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::text(prompt.text_content()),
            ));
            self.observed_turns
                .lock()
                .expect("observed turns lock poisoned")
                .push((prompt.text_content(), Vec::new()));
            Ok(self.ok_result())
        }

        async fn run_turn_with_events(
            &mut self,
            input: SessionAgentTurnInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            self.session.push(meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::text(input.prompt.text_content()),
            ));
            self.observed_turns
                .lock()
                .expect("observed turns lock poisoned")
                .push((
                    input.prompt.text_content(),
                    input
                        .injected_context
                        .iter()
                        .map(ContentInput::text_content)
                        .collect(),
                ));
            Ok(self.ok_result())
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: self.session.messages().len(),
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            Ok(self.session.clone())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            SessionTranscriptAuthoritySnapshot::from_session(&self.session)
        }

        fn append_system_messages(&mut self, contents: Vec<String>) -> Result<(), AgentError> {
            for content in contents {
                self.session.append_system_message(content);
            }
            Ok(())
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient_turn_context_state.clone()
        }
    }

    fn create_request(
        prompt: &str,
        injected_context: Vec<ContentInput>,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "injected-context-probe".to_string(),
            prompt: ContentInput::Text(prompt.to_string()),
            injected_context,
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    /// Eager create threads `CreateSessionRequest.injected_context` through
    /// the session task into the first turn's `SessionAgentTurnInput`, in
    /// delivery order (prompt-bearing create is submit-work).
    #[tokio::test]
    async fn eager_create_threads_injected_context_to_first_turn() {
        let observed_turns = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            InjectedContextProbeBuilder {
                observed_turns: Arc::clone(&observed_turns),
            },
            1,
        );

        service
            .create_session(create_request(
                "first prompt",
                vec![
                    ContentInput::Text("ambient one".to_string()),
                    ContentInput::Text("ambient two".to_string()),
                ],
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect("eager create with injected context should run");

        assert_eq!(
            *observed_turns.lock().expect("observed turns lock poisoned"),
            vec![(
                "first prompt".to_string(),
                vec!["ambient one".to_string(), "ambient two".to_string()],
            )],
            "create-path injected context must reach the agent turn input in order"
        );
    }

    /// `StartTurnRequest.injected_context` threads through the session task
    /// into `SessionAgentTurnInput` for an ordinary follow-up turn.
    #[tokio::test]
    async fn start_turn_threads_injected_context_to_agent() {
        let observed_turns = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            InjectedContextProbeBuilder {
                observed_turns: Arc::clone(&observed_turns),
            },
            1,
        );

        let created = service
            .create_session(create_request(
                "defer",
                Vec::new(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("deferred session should create");

        service
            .start_turn(
                &created.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("turn prompt".to_string()),
                    injected_context: vec![ContentInput::Text("turn ambient".to_string())],
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                },
            )
            .await
            .expect("turn with injected context should run");

        assert_eq!(
            *observed_turns.lock().expect("observed turns lock poisoned"),
            vec![("turn prompt".to_string(), vec!["turn ambient".to_string()],)],
            "turn-path injected context must reach the agent turn input"
        );
    }

    #[tokio::test]
    async fn start_turn_appends_all_system_carriers_in_exact_order_between_users() {
        let observed_turns = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            InjectedContextProbeBuilder {
                observed_turns: Arc::clone(&observed_turns),
            },
            1,
        );
        let created = service
            .create_session(create_request(
                "defer",
                Vec::new(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("deferred session should create");

        service
            .start_turn(
                &created.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("first user".to_string()),
                    injected_context: Vec::new(),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                },
            )
            .await
            .expect("first turn should run");

        let metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            system_prompts: vec![
                String::new(),
                " repeated ".to_string(),
                " repeated ".to_string(),
            ],
            ..Default::default()
        };
        service
            .start_turn(
                &created.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("second user".to_string()),
                    injected_context: Vec::new(),
                    system_prompt: Some(" explicit ".to_string()),
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        meerkat_core::types::HandlingMode::Queue,
                        None,
                        Some(metadata),
                    ),
                },
            )
            .await
            .expect("second turn should append Systems then run");

        service
            .start_turn(
                &created.session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("third user".to_string()),
                    injected_context: Vec::new(),
                    system_prompt: Some(String::new()),
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                },
            )
            .await
            .expect("third turn should preserve an empty System append");

        let session = service
            .export_session(&created.session_id)
            .await
            .expect("export exact transcript");
        let rows = session
            .messages()
            .iter()
            .map(|message| match message {
                meerkat_core::types::Message::User(user) => ("user", user.text_content()),
                meerkat_core::types::Message::System(system) => ("system", system.content.clone()),
                other => panic!("unexpected probe transcript row: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                ("user", "first user".to_string()),
                ("system", String::new()),
                ("system", " repeated ".to_string()),
                ("system", " repeated ".to_string()),
                ("system", " explicit ".to_string()),
                ("user", "second user".to_string()),
                ("system", String::new()),
                ("user", "third user".to_string()),
            ],
            "System rows are ordinary ordered transcript data; carrier order, duplicates, and exact bytes must survive"
        );
    }

    /// Deferred create has no first turn to attach injected context to; the
    /// service fails closed instead of silently dropping host context.
    #[tokio::test]
    async fn deferred_create_rejects_injected_context() {
        let observed_turns = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            InjectedContextProbeBuilder {
                observed_turns: Arc::clone(&observed_turns),
            },
            1,
        );

        let err = service
            .create_session(create_request(
                "defer",
                vec![ContentInput::Text("dropped ambient".to_string())],
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect_err("deferred create with injected context must fail closed");
        assert!(
            matches!(err, SessionError::Unsupported(_)),
            "expected typed Unsupported rejection, got {err:?}"
        );
        assert!(
            observed_turns
                .lock()
                .expect("observed turns lock poisoned")
                .is_empty(),
            "no turn may run after the fail-closed rejection"
        );
    }

    /// Wrapper that keeps the trait-default `run_turn_with_events`, proving
    /// the default entry fails closed on injected context instead of folding
    /// it into the prompt (which would launder the typed role away).
    struct DefaultGuardAgent(InjectedContextProbeAgent);

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for DefaultGuardAgent {
        async fn run_with_events(
            &mut self,
            prompt: ContentInput,
            event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            self.0.run_with_events(prompt, event_tx).await
        }

        fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
            self.0.set_skill_references(refs);
        }

        fn set_turn_tool_overlay(
            &mut self,
            overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            self.0.set_turn_tool_overlay(overlay)
        }

        fn hot_swap_llm_identity(
            &mut self,
            client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: SessionLlmIdentity,
            request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            self.0
                .hot_swap_llm_identity(client, identity, request_policy)
        }

        fn cancel(&mut self) {
            self.0.cancel();
        }

        fn session_id(&self) -> SessionId {
            self.0.session_id()
        }

        fn snapshot(&self) -> SessionSnapshot {
            self.0.snapshot()
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            self.0.session_clone()
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            self.0.session_transcript_authority()
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            self.0.observed_session_tail()
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.0.transient_turn_context_state()
        }
    }

    #[tokio::test]
    async fn default_turn_entry_rejects_injected_context() {
        let observed_turns = Arc::new(Mutex::new(Vec::new()));
        let mut agent = DefaultGuardAgent(InjectedContextProbeAgent {
            session_id: SessionId::new(),
            session: meerkat_core::Session::new(),
            identity: probe_llm_identity("default-guard"),
            observed_turns: Arc::clone(&observed_turns),
            transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
        });

        let (event_tx, _event_rx) = mpsc::channel(4);
        let err = agent
            .run_turn_with_events(
                SessionAgentTurnInput {
                    prompt: ContentInput::Text("prompt".to_string()),
                    injected_context: vec![ContentInput::Text("ambient".to_string())],
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    render_metadata: None,
                    typed_turn_appends: Vec::new(),
                    transcript_identity: None,
                    execution_kind: None,
                },
                event_tx,
            )
            .await
            .expect_err("default turn entry must fail closed on injected context");
        assert!(
            matches!(err, AgentError::ConfigError(_)),
            "expected typed config error, got {err:?}"
        );
        assert!(
            observed_turns
                .lock()
                .expect("observed turns lock poisoned")
                .is_empty(),
            "the run must not start after the fail-closed rejection"
        );
    }
}

#[cfg(test)]
mod archive_snapshot_gate_tests {
    use super::*;

    #[test]
    fn standalone_archive_verdict_fails_closed_on_missing_ambiguous_or_wrong_effects() {
        let session_id = SessionId::new();
        assert!(require_standalone_archive_verdict(&session_id, Vec::new()).is_err());
        assert!(
            require_standalone_archive_verdict(
                &session_id,
                vec![SessionDocumentEffect::SessionArchiveResolved {
                    disposition: SessionArchiveDisposition::AlreadyArchived,
                    write_document: false,
                    retire_runtime: false,
                }],
            )
            .is_err()
        );
        assert!(
            require_standalone_archive_verdict(
                &session_id,
                vec![SessionDocumentEffect::SessionArchiveResolved {
                    disposition: SessionArchiveDisposition::Archive,
                    write_document: true,
                    retire_runtime: false,
                }],
            )
            .is_err()
        );
        assert!(
            require_standalone_archive_verdict(
                &session_id,
                vec![
                    SessionDocumentEffect::SessionArchiveResolved {
                        disposition: SessionArchiveDisposition::Archive,
                        write_document: false,
                        retire_runtime: false,
                    },
                    SessionDocumentEffect::SessionArchiveResolved {
                        disposition: SessionArchiveDisposition::Archive,
                        write_document: false,
                        retire_runtime: false,
                    },
                ],
            )
            .is_err()
        );
        assert!(
            require_standalone_archive_verdict(
                &session_id,
                vec![SessionDocumentEffect::SessionArchiveResolved {
                    disposition: SessionArchiveDisposition::Archive,
                    write_document: false,
                    retire_runtime: false,
                }],
            )
            .is_ok(),
            "the exact standalone action vector is authorized"
        );
    }

    #[test]
    fn archive_snapshot_gate_rejects_context_after_snapshot_close() {
        let gate = ArchiveSnapshotGate::open();
        let open_guard = gate.enter_apply();
        assert!(open_guard.is_ok(), "open gate admits context apply");
        drop(open_guard);

        gate.close_for_snapshot();

        let closed_guard = gate.enter_apply();
        assert!(
            closed_guard.is_err(),
            "closed archive gate rejects queued context apply"
        );
        let err = match closed_guard {
            Ok(_) => return,
            Err(err) => err,
        };
        assert_eq!(
            err.structured_data()
                .and_then(|data| data.get("reason").cloned()),
            Some(serde_json::Value::String(
                "archive_snapshot_taken".to_string()
            ))
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod admission_window_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::service::{
        InitialTurnPolicy, SessionBuildOptions, SessionService, StartTurnRequest,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn test_llm_identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    #[derive(Clone)]
    struct AdmissionProbeBuilder {
        run_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        compaction_abort_calls: Arc<AtomicUsize>,
        cancel_after_boundary_tx: CancelAfterBoundarySender,
        turn_admission_for_run: Arc<Mutex<Option<Arc<Mutex<TurnAdmissionSlot>>>>>,
        interrupt_before_success: bool,
    }

    struct AdmissionProbeAgent {
        session_id: SessionId,
        identity: SessionLlmIdentity,
        run_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        compaction_abort_calls: Arc<AtomicUsize>,
        cancel_after_boundary_tx: CancelAfterBoundarySender,
        turn_admission_for_run: Arc<Mutex<Option<Arc<Mutex<TurnAdmissionSlot>>>>>,
        interrupt_before_success: bool,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for AdmissionProbeBuilder {
        type Agent = AdmissionProbeAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(AdmissionProbeAgent {
                session_id: SessionId::new(),
                identity: test_llm_identity(&req.model),
                run_calls: Arc::clone(&self.run_calls),
                cancel_calls: Arc::clone(&self.cancel_calls),
                compaction_abort_calls: Arc::clone(&self.compaction_abort_calls),
                cancel_after_boundary_tx: self.cancel_after_boundary_tx.clone(),
                turn_admission_for_run: Arc::clone(&self.turn_admission_for_run),
                interrupt_before_success: self.interrupt_before_success,
                transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for AdmissionProbeAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            if self.interrupt_before_success {
                let turn_admission = self
                    .turn_admission_for_run
                    .lock()
                    .expect("turn admission probe lock poisoned")
                    .clone()
                    .expect("turn admission probe installed");
                let mut slot = lock_turn_admission(&turn_admission);
                slot.request_interrupt()
                    .expect("running turn should accept interrupt probe");
            }
            self.run_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RunResult {
                text: "ran".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        }

        async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), AgentError> {
            self.compaction_abort_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn cancel_after_boundary_handle(&self) -> Option<CancelAfterBoundarySender> {
            Some(self.cancel_after_boundary_tx.clone())
        }

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            Ok(meerkat_core::Session::new())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            let session = self.session_clone()?;
            SessionTranscriptAuthoritySnapshot::from_session(&session)
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient_turn_context_state.clone()
        }
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "admission-window-test".to_string(),
            prompt: ContentInput::Text("defer".to_string()),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    fn start_turn_request() -> StartTurnRequest {
        StartTurnRequest {
            injected_context: Vec::new(),
            prompt: ContentInput::Text("go".to_string()),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        }
    }

    fn probe_builder(
        run_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        compaction_abort_calls: Arc<AtomicUsize>,
        cancel_after_boundary_tx: CancelAfterBoundarySender,
    ) -> AdmissionProbeBuilder {
        AdmissionProbeBuilder {
            run_calls,
            cancel_calls,
            compaction_abort_calls,
            cancel_after_boundary_tx,
            turn_admission_for_run: Arc::new(Mutex::new(None)),
            interrupt_before_success: false,
        }
    }

    async fn create_admitted_session(
        service: &EphemeralSessionService<AdmissionProbeBuilder>,
    ) -> (SessionId, mpsc::Sender<SessionCommand>) {
        let result = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let command_tx = {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&result.session_id).expect("session handle");
            EphemeralSessionService::<AdmissionProbeBuilder>::request_start_turn(
                &result.session_id,
                handle,
            )
            .expect("admit turn before command delivery");
            handle.command_tx.clone()
        };
        (result.session_id, command_tx)
    }

    async fn deliver_start_turn(
        command_tx: mpsc::Sender<SessionCommand>,
    ) -> Result<RunResult, AgentError> {
        let (result_tx, result_rx) = oneshot::channel();
        let request = start_turn_request();
        command_tx
            .send(SessionCommand::StartTurn {
                prompt: request.prompt,
                system_messages: request.system_prompt.into_iter().collect(),
                injected_context: request.injected_context,
                runtime: Box::new(request.runtime),
                event_tx: request.event_tx,
                result_tx,
                active_admission: None,
            })
            .await
            .expect("send start turn");
        let outcome = result_rx.await.expect("receive start turn result");
        outcome
            .machine_terminal_failure
            .expect("test turn terminal witness should project");
        outcome.result
    }

    #[tokio::test]
    async fn hard_interrupt_during_admission_cancels_before_agent_poll() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let compaction_abort_calls = Arc::new(AtomicUsize::new(0));
        let (cancel_after_boundary_tx, _cancel_after_boundary_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let service = EphemeralSessionService::new(
            probe_builder(
                Arc::clone(&run_calls),
                Arc::clone(&cancel_calls),
                Arc::clone(&compaction_abort_calls),
                cancel_after_boundary_tx,
            ),
            1,
        );
        let (session_id, command_tx) = create_admitted_session(&service).await;

        service
            .interrupt(&session_id)
            .await
            .expect("admitted turn accepts hard interrupt");
        let result = deliver_start_turn(command_tx).await;

        assert!(matches!(result, Err(AgentError::Cancelled)));
        assert_eq!(run_calls.load(Ordering::SeqCst), 0);
        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
        assert_eq!(compaction_abort_calls.load(Ordering::SeqCst), 1);

        let next = service
            .start_turn(&session_id, start_turn_request())
            .await
            .expect("next turn should run after interrupted compaction cleanup");
        assert_eq!(next.text, "ran");
        assert_eq!(compaction_abort_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn boundary_cancel_during_admission_requires_exact_run_authority() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let (cancel_after_boundary_tx, mut cancel_after_boundary_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let service = EphemeralSessionService::new(
            probe_builder(
                Arc::clone(&run_calls),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                cancel_after_boundary_tx,
            ),
            1,
        );
        let (session_id, command_tx) = create_admitted_session(&service).await;

        let error = service
            .cancel_after_boundary(&session_id)
            .await
            .expect_err("a sender without exact run authority must be unsupported");
        assert!(matches!(
            error,
            SessionError::Unsupported(operation)
                if operation == "cancel_after_boundary_exact_run_authority"
        ));
        assert!(matches!(
            cancel_after_boundary_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        let result = deliver_start_turn(command_tx)
            .await
            .expect("start turn should run cooperatively");
        assert_eq!(result.text, "ran");
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);
        // No further command is queued after the single delivery.
        assert!(matches!(
            cancel_after_boundary_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn hard_interrupt_pending_when_run_result_is_ready_wins_over_success() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let compaction_abort_calls = Arc::new(AtomicUsize::new(0));
        let turn_admission_for_run = Arc::new(Mutex::new(None));
        let (cancel_after_boundary_tx, _cancel_after_boundary_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let mut builder = probe_builder(
            Arc::clone(&run_calls),
            Arc::clone(&cancel_calls),
            Arc::clone(&compaction_abort_calls),
            cancel_after_boundary_tx,
        );
        builder.turn_admission_for_run = Arc::clone(&turn_admission_for_run);
        builder.interrupt_before_success = true;
        let service = EphemeralSessionService::new(builder, 1);
        let result = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&result.session_id).expect("session handle");
            *turn_admission_for_run
                .lock()
                .expect("turn admission probe lock poisoned") =
                Some(Arc::clone(&handle.turn_admission));
        }

        let result = service
            .start_turn(&result.session_id, start_turn_request())
            .await;

        assert!(matches!(
            result,
            Err(SessionError::Agent(AgentError::Cancelled))
        ));
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
        assert_eq!(compaction_abort_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn boundary_cancel_on_aborted_admission_requires_exact_run_authority() {
        let run_calls = Arc::new(AtomicUsize::new(0));
        let (cancel_after_boundary_tx, mut cancel_after_boundary_rx) =
            tokio::sync::mpsc::unbounded_channel();
        let service = EphemeralSessionService::new(
            probe_builder(
                Arc::clone(&run_calls),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                cancel_after_boundary_tx,
            ),
            1,
        );
        let (session_id, _command_tx) = create_admitted_session(&service).await;

        let error = service
            .cancel_after_boundary(&session_id)
            .await
            .expect_err("a sender without exact run authority must be unsupported");
        assert!(matches!(
            error,
            SessionError::Unsupported(operation)
                if operation == "cancel_after_boundary_exact_run_authority"
        ));
        assert!(matches!(
            cancel_after_boundary_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&session_id).expect("session handle");
            EphemeralSessionService::<AdmissionProbeBuilder>::try_abort_admitted_turn(handle);
        }
        // Aborting the admission does not synthesize another command; staleness
        // reset is now core-owned via the run-start flush, so no clear helper
        // re-touches the carrier here.
        assert!(matches!(
            cancel_after_boundary_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        let result = service
            .start_turn(&session_id, start_turn_request())
            .await
            .expect("next turn should run");
        assert_eq!(result.text, "ran");
        assert_eq!(run_calls.load(Ordering::SeqCst), 1);
    }
}

/// Shell-level interleaving tests for the 0.7.2 disciplined-shell-inputs
/// archive/teardown seam (undisciplined-shell-inputs-072.json entries 15-20).
///
/// These drive the racy orders deterministically: teardown commits between an
/// input's decision point and its delivery, and every pending waiter must
/// resolve with a typed benign/terminal outcome — never a dropped reply
/// channel, never an internal error for a legitimate interleaving.
///
/// Expected colors after Stage A codegen (DSL landed, shell not yet rewired):
/// - `pending_start_turn_resolves_cancelled_when_archive_wins`: RED
/// - `queued_context_application_resolves_after_archive_mid_turn`: RED
/// - `begin_window_shutdown_yank_resolves_cancelled`: RED
/// - `archive_closes_drain_obligation_before_task_exit`: RED
/// - `post_archive_context_application_returns_typed_not_found`: GREEN (pin)
/// - `start_turn_processed_after_archive_resolves_cancelled`: GREEN (pin)
#[cfg(test)]
#[allow(clippy::expect_used)]
mod archive_shutdown_drain_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::service::{
        InitialTurnPolicy, SessionBuildOptions, SessionService, StartTurnRequest,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    const WAITER_TIMEOUT: Duration = Duration::from_secs(10);

    fn test_llm_identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    /// Probe hooks shared between the test body and the agent running inside
    /// the session task.
    #[derive(Clone)]
    struct DrainProbeHooks {
        entered_run: Arc<tokio::sync::Notify>,
        release_run: Arc<tokio::sync::Semaphore>,
        entered_control: Arc<tokio::sync::Notify>,
        release_control: Arc<tokio::sync::Semaphore>,
        actor_dropped: Arc<tokio::sync::Notify>,
        /// When set, the next pending-tool-results effect handler fires
        /// `request_shutdown` on the installed turn-admission slot. That
        /// handler is the last synchronous agent seam before `begin`, so this
        /// deterministically simulates archive committing its teardown
        /// transition between admitted preflight and begin.
        yank_shutdown_before_begin: Arc<AtomicBool>,
        turn_admission: Arc<std::sync::Mutex<Option<Arc<std::sync::Mutex<TurnAdmissionSlot>>>>>,
    }

    impl DrainProbeHooks {
        fn new() -> Self {
            Self {
                entered_run: Arc::new(tokio::sync::Notify::new()),
                release_run: Arc::new(tokio::sync::Semaphore::new(0)),
                entered_control: Arc::new(tokio::sync::Notify::new()),
                release_control: Arc::new(tokio::sync::Semaphore::new(0)),
                actor_dropped: Arc::new(tokio::sync::Notify::new()),
                yank_shutdown_before_begin: Arc::new(AtomicBool::new(false)),
                turn_admission: Arc::new(std::sync::Mutex::new(None)),
            }
        }

        async fn install_turn_admission<B: SessionAgentBuilder + 'static>(
            &self,
            service: &EphemeralSessionService<B>,
            session_id: &SessionId,
        ) {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(session_id).expect("session handle");
            *self
                .turn_admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::clone(&handle.turn_admission));
        }
    }

    #[derive(Clone)]
    struct DrainProbeBuilder {
        hooks: DrainProbeHooks,
    }

    struct DrainProbeAgent {
        session_id: SessionId,
        identity: SessionLlmIdentity,
        hooks: DrainProbeHooks,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    }

    impl Drop for DrainProbeAgent {
        fn drop(&mut self) {
            self.hooks.actor_dropped.notify_one();
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for DrainProbeBuilder {
        type Agent = DrainProbeAgent;

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(DrainProbeAgent {
                session_id: SessionId::new(),
                identity: test_llm_identity(&req.model),
                hooks: self.hooks.clone(),
                transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for DrainProbeAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            self.hooks.entered_run.notify_one();
            self.hooks
                .release_run
                .acquire()
                .await
                .expect("drain probe run release semaphore should stay open")
                .forget();
            Ok(RunResult {
                text: "ran".to_string(),
                session_id: self.session_id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn apply_pending_tool_results(
            &mut self,
            results: Vec<meerkat_core::ToolResult>,
        ) -> Result<(), AgentError> {
            if !results.is_empty() {
                return Err(AgentError::ConfigError(
                    "drain probe does not support pending tool results".to_string(),
                ));
            }
            if self
                .hooks
                .yank_shutdown_before_begin
                .swap(false, Ordering::SeqCst)
            {
                let turn_admission = self
                    .hooks
                    .turn_admission
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .ok_or_else(|| {
                        AgentError::InternalError(
                            "drain probe has no installed turn-admission slot".to_string(),
                        )
                    })?;
                lock_turn_admission(&turn_admission)
                    .request_shutdown()
                    .map_err(|error| {
                        AgentError::InternalError(format!(
                            "drain probe failed to yank shutdown before begin: {error}"
                        ))
                    })?;
            }
            Ok(())
        }

        async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), AgentError> {
            self.hooks.entered_control.notify_one();
            self.hooks
                .release_control
                .acquire()
                .await
                .expect("drain probe control release semaphore should stay open")
                .forget();
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            Ok(meerkat_core::Session::new())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            let session = self.session_clone()?;
            SessionTranscriptAuthoritySnapshot::from_session(&session)
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(self.identity.clone())
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient_turn_context_state.clone()
        }
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "archive-drain-test".to_string(),
            prompt: ContentInput::Text("defer".to_string()),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    fn start_turn_request() -> StartTurnRequest {
        StartTurnRequest {
            injected_context: Vec::new(),
            prompt: ContentInput::Text("go".to_string()),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        }
    }

    async fn command_tx_for<B: SessionAgentBuilder + 'static>(
        service: &EphemeralSessionService<B>,
        session_id: &SessionId,
    ) -> mpsc::Sender<SessionCommand> {
        let sessions = service.sessions.read().await;
        sessions
            .get(session_id)
            .expect("session handle")
            .command_tx
            .clone()
    }

    async fn turn_admission_for<B: SessionAgentBuilder + 'static>(
        service: &EphemeralSessionService<B>,
        session_id: &SessionId,
    ) -> Arc<std::sync::Mutex<TurnAdmissionSlot>> {
        let sessions = service.sessions.read().await;
        Arc::clone(
            &sessions
                .get(session_id)
                .expect("session handle")
                .turn_admission,
        )
    }

    /// Entry 16/18 ("Archive-Unregister-Command-Leapfrog"): a runtime context
    /// application queued behind archive's `Shutdown` command must resolve
    /// its waiter with a typed benign outcome — the appends are dropped with
    /// the archived session — instead of the session task exiting and
    /// dropping the reply channel.
    #[tokio::test]
    async fn pending_start_turn_resolves_cancelled_when_archive_wins() {
        let hooks = DrainProbeHooks::new();
        let service = Arc::new(EphemeralSessionService::new(
            DrainProbeBuilder {
                hooks: hooks.clone(),
            },
            1,
        ));
        let created = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let session_id = created.session_id.clone();
        let command_tx = command_tx_for(service.as_ref(), &session_id).await;

        // Occupy the session task with a live run.
        let turn_service = Arc::clone(&service);
        let turn_session = session_id.clone();
        let turn = tokio::spawn(async move {
            turn_service
                .start_turn(&turn_session, start_turn_request())
                .await
        });
        tokio::time::timeout(WAITER_TIMEOUT, hooks.entered_run.notified())
            .await
            .expect("turn should enter the probe run");

        // Archive (queues Shutdown), THEN deliver the racing start-turn via
        // the pre-archive handle: it queues behind Shutdown.
        service.archive(&session_id).await.expect("archive");
        let (result_tx, result_rx) = oneshot::channel();
        let request = start_turn_request();
        command_tx
            .send(SessionCommand::StartTurn {
                prompt: request.prompt,
                system_messages: request.system_prompt.into_iter().collect(),
                injected_context: request.injected_context,
                runtime: Box::new(request.runtime),
                event_tx: request.event_tx,
                result_tx,
                active_admission: None,
            })
            .await
            .expect("pre-archive handle accepts the queued start-turn");

        hooks.release_run.add_permits(1);
        let run_result = tokio::time::timeout(WAITER_TIMEOUT, turn)
            .await
            .expect("turn task should finish")
            .expect("turn task should not panic")
            .expect("committed turn must succeed");
        assert_eq!(run_result.text, "ran");

        let pending = tokio::time::timeout(WAITER_TIMEOUT, result_rx)
            .await
            .expect("pending start-turn waiter should settle")
            .expect(
                "start-turn racing archive must resolve with the typed cancellation, \
                 not a dropped result channel",
            );
        assert!(
            matches!(pending.result, Err(AgentError::Cancelled)),
            "pending start-turn must resolve with the typed cancellation, got {pending:?}"
        );
    }

    /// Pin (GREEN): a start-turn the task DOES dequeue after archive's
    /// shutdown transition already resolves with the typed cancellation via
    /// `AuthorizeStartTurnDispatchShuttingDown` -> `Cancelled`.
    #[tokio::test]
    async fn start_turn_processed_after_archive_resolves_cancelled() {
        let hooks = DrainProbeHooks::new();
        let service = EphemeralSessionService::new(
            DrainProbeBuilder {
                hooks: hooks.clone(),
            },
            1,
        );
        let created = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let session_id = created.session_id.clone();
        let command_tx = command_tx_for(&service, &session_id).await;
        let turn_admission = turn_admission_for(&service, &session_id).await;

        // Claim the admission (the start-turn decision point), then let the
        // archive teardown transition commit BEFORE the command is delivered.
        {
            let sessions = service.sessions.read().await;
            let handle = sessions.get(&session_id).expect("session handle");
            EphemeralSessionService::<DrainProbeBuilder>::request_start_turn(&session_id, handle)
                .expect("idle session admits the turn");
        }
        {
            let mut slot = lock_turn_admission(&turn_admission);
            slot.request_shutdown()
                .expect("admitted slot accepts shutdown");
        }

        let (result_tx, result_rx) = oneshot::channel();
        let request = start_turn_request();
        command_tx
            .send(SessionCommand::StartTurn {
                prompt: request.prompt,
                system_messages: request.system_prompt.into_iter().collect(),
                injected_context: request.injected_context,
                runtime: Box::new(request.runtime),
                event_tx: request.event_tx,
                result_tx,
                active_admission: None,
            })
            .await
            .expect("live handle accepts the start-turn command");

        let pending = tokio::time::timeout(WAITER_TIMEOUT, result_rx)
            .await
            .expect("start-turn waiter should settle")
            .expect("start-turn waiter must resolve");
        assert!(
            matches!(pending.result, Err(AgentError::Cancelled)),
            "start-turn dispatched into ShuttingDown must cancel cleanly, got {pending:?}"
        );
    }

    /// Entries 19/20 (begin-window flavor of the mid-arm yank): archive's
    /// shutdown transition commits between the admitted preflight and the
    /// begin step. The waiter must see the typed cancellation, not an
    /// "illegal begin-run transition" internal error.
    #[tokio::test]
    async fn begin_window_shutdown_yank_resolves_cancelled() {
        let hooks = DrainProbeHooks::new();
        let service = EphemeralSessionService::new(
            DrainProbeBuilder {
                hooks: hooks.clone(),
            },
            1,
        );
        let created = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let session_id = created.session_id.clone();
        hooks.install_turn_admission(&service, &session_id).await;
        hooks
            .yank_shutdown_before_begin
            .store(true, Ordering::SeqCst);

        let result = tokio::time::timeout(
            WAITER_TIMEOUT,
            service.start_turn(&session_id, start_turn_request()),
        )
        .await
        .expect("begin-window shutdown yank must settle the start-turn waiter");

        assert!(
            matches!(result, Err(SessionError::Agent(AgentError::Cancelled))),
            "a start-turn yanked into ShuttingDown before begin must cancel cleanly, \
             got {result:?}"
        );
    }

    #[tokio::test]
    async fn archive_does_not_wait_for_saturated_command_queue() {
        let hooks = DrainProbeHooks::new();
        let service = Arc::new(EphemeralSessionService::new(
            DrainProbeBuilder {
                hooks: hooks.clone(),
            },
            2,
        ));
        let created = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let session_id = created.session_id.clone();
        let other_session_id = service
            .create_session(create_request())
            .await
            .expect("create unrelated deferred session")
            .session_id;
        let command_tx = command_tx_for(service.as_ref(), &session_id).await;

        // Hold the actor inside a run so its command receiver cannot drain.
        let turn_service = Arc::clone(&service);
        let turn_session = session_id.clone();
        let turn = tokio::spawn(async move {
            turn_service
                .start_turn(&turn_session, start_turn_request())
                .await
        });
        tokio::time::timeout(WAITER_TIMEOUT, hooks.entered_run.notified())
            .await
            .expect("turn should enter the probe run");

        // Fill every command slot. Archive must use the out-of-band shutdown
        // wake rather than competing for ordinary command capacity.
        let mut queued_probe_replies = Vec::with_capacity(COMMAND_CHANNEL_CAPACITY);
        for _ in 0..COMMAND_CHANNEL_CAPACITY {
            let (reply_tx, reply_rx) = oneshot::channel();
            command_tx
                .send(SessionCommand::VisibleToolDefs { reply_tx })
                .await
                .expect("blocked actor keeps its command receiver alive");
            queued_probe_replies.push(reply_rx);
        }

        tokio::time::timeout(Duration::from_secs(1), service.archive(&session_id))
            .await
            .expect("archive must not wait for shutdown command capacity")
            .expect("archive must succeed");

        assert!(
            !service
                .has_live_session(&session_id)
                .await
                .expect("archived session live query"),
            "archive must remove the exact live actor"
        );
        assert!(
            service
                .archived_views
                .read()
                .await
                .contains_key(&session_id),
            "archive must publish the archived view"
        );

        // Unrelated session B stays promptly readable through both the public
        // live-query and the read path used by interrupt dispatch.
        let other_is_live = tokio::time::timeout(
            Duration::from_secs(1),
            service.has_live_session(&other_session_id),
        )
        .await
        .expect("archive A must not block live-session reads for B")
        .expect("B live-session query should succeed");
        assert!(other_is_live);
        let other_interrupt =
            tokio::time::timeout(Duration::from_secs(1), service.interrupt(&other_session_id))
                .await
                .expect("archive A must not block interrupt-relevant registry access for B");
        assert!(
            matches!(other_interrupt, Err(SessionError::NotRunning { .. })),
            "idle B should remain promptly readable and report NotRunning, got {other_interrupt:?}"
        );
        // The in-flight turn finishes, observes ShuttingDown, and drains every
        // command that was buffered before archive closed the actor channel.
        hooks.release_run.add_permits(1);
        let run_result = tokio::time::timeout(WAITER_TIMEOUT, turn)
            .await
            .expect("turn task should finish")
            .expect("turn task should not panic")
            .expect("in-flight turn must complete");
        assert_eq!(run_result.text, "ran");
        for reply in queued_probe_replies {
            tokio::time::timeout(WAITER_TIMEOUT, reply)
                .await
                .expect("queued command must drain")
                .expect("queued command reply sender must remain live");
        }
        service
            .archive(&other_session_id)
            .await
            .expect("cleanup unrelated session");
    }

    #[tokio::test]
    async fn discard_live_session_actor_does_not_wait_for_saturated_command_queue() {
        let hooks = DrainProbeHooks::new();
        let service = Arc::new(EphemeralSessionService::new(
            DrainProbeBuilder {
                hooks: hooks.clone(),
            },
            1,
        ));
        let created = service
            .create_session(create_request())
            .await
            .expect("create session");
        let session_id = created.session_id;
        let command_tx = command_tx_for(service.as_ref(), &session_id).await;
        let witness = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("live actor witness");

        let (control_reply_tx, control_reply_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::AbortUncommittedCompactionProjections {
                reply_tx: control_reply_tx,
            })
            .await
            .expect("send actor-blocking control command");
        tokio::time::timeout(WAITER_TIMEOUT, hooks.entered_control.notified())
            .await
            .expect("actor should enter the blocking control command");

        let mut queued_probe_replies = Vec::with_capacity(COMMAND_CHANNEL_CAPACITY);
        for _ in 0..COMMAND_CHANNEL_CAPACITY {
            let (reply_tx, reply_rx) = oneshot::channel();
            command_tx
                .send(SessionCommand::VisibleToolDefs { reply_tx })
                .await
                .expect("blocked actor keeps its command receiver alive");
            queued_probe_replies.push(reply_rx);
        }

        tokio::time::timeout(
            Duration::from_secs(1),
            service.discard_live_session_actor(&witness),
        )
        .await
        .expect("discard must not wait for shutdown command capacity")
        .expect("discard should succeed");

        hooks.release_control.add_permits(1);
        tokio::time::timeout(WAITER_TIMEOUT, control_reply_rx)
            .await
            .expect("in-flight control command must resolve")
            .expect("in-flight control reply sender must remain live")
            .expect("in-flight control command must complete");
        for reply in queued_probe_replies {
            tokio::time::timeout(WAITER_TIMEOUT, reply)
                .await
                .expect("queued command must drain")
                .expect("queued command reply sender must remain live");
        }
        tokio::time::timeout(WAITER_TIMEOUT, command_tx.closed())
            .await
            .expect("Notify-driven shutdown must close the actor command channel");
    }

    /// Regression (0.7.2): `discard_live_session` must be idempotent.
    ///
    /// The runtime-loop drain in `unregister_session` (D1) quiesces the loop,
    /// whose `StopRuntimeExecutor` clean exit discards the live handle. A caller
    /// that then explicitly discards the same session (e.g. same-process mob
    /// restart in `smoke_mob_resume`) raced that teardown and got `NotFound`,
    /// which `.expect()` turned into a panic. "Not live" is the desired end
    /// state, so a redundant discard must be `Ok(())`. This is a regular
    /// (non-live, CI-gated) guard for the behavior the smoke lane caught.
    #[tokio::test]
    async fn discard_live_session_is_idempotent() {
        let hooks = DrainProbeHooks::new();
        let service = EphemeralSessionService::new(DrainProbeBuilder { hooks }, 1);
        let created = service
            .create_session(create_request())
            .await
            .expect("create session");
        let session_id = created.session_id.clone();

        // First discard removes the live handle.
        service
            .discard_live_session(&session_id)
            .await
            .expect("first discard removes the live handle");

        // Second discard of the now-absent handle must be a benign no-op, NOT
        // `NotFound` — this is the exact interleaving the teardown drain
        // produces against an explicit caller discard.
        service
            .discard_live_session(&session_id)
            .await
            .expect("second discard is idempotent, not NotFound");

        // A never-materialized session is likewise `Ok(())`.
        let never_live = SessionId::new();
        service
            .discard_live_session(&never_live)
            .await
            .expect("discarding a never-materialized session is idempotent");
    }

    /// D1: archive teardown is only complete once the machine-owned drain
    /// obligation has closed — the session task must flush pending admission
    /// work and fire `ResolvePendingAdmissionDrained` before it exits.
    #[tokio::test]
    async fn archive_closes_drain_obligation_before_task_exit() {
        let hooks = DrainProbeHooks::new();
        let service = EphemeralSessionService::new(DrainProbeBuilder { hooks }, 1);
        let created = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let session_id = created.session_id.clone();
        let command_tx = command_tx_for(&service, &session_id).await;
        let turn_admission = turn_admission_for(&service, &session_id).await;

        service.archive(&session_id).await.expect("archive");

        // The session task exits once it has observed the out-of-band shutdown
        // wake; the command channel closes with it.
        tokio::time::timeout(WAITER_TIMEOUT, command_tx.closed())
            .await
            .expect("session task should exit after archive");

        let slot = lock_turn_admission(&turn_admission);
        assert_eq!(slot.phase(), TurnAdmissionPhase::ShuttingDown);
        assert!(
            !slot.admission_drain_pending(),
            "the session task must close the machine-owned drain obligation \
             (ResolvePendingAdmissionDrained) before it exits"
        );
    }

    #[tokio::test]
    async fn closed_command_channel_authorizes_teardown_before_task_exit() {
        let hooks = DrainProbeHooks::new();
        let service = EphemeralSessionService::new(
            DrainProbeBuilder {
                hooks: hooks.clone(),
            },
            1,
        );
        let created = service
            .create_session(create_request())
            .await
            .expect("create deferred session");
        let turn_admission = turn_admission_for(&service, &created.session_id).await;

        drop(service);

        tokio::time::timeout(WAITER_TIMEOUT, hooks.actor_dropped.notified())
            .await
            .expect("closed command channel must let the actor exit");
        let slot = lock_turn_admission(&turn_admission);
        assert_eq!(slot.phase(), TurnAdmissionPhase::ShuttingDown);
        assert!(
            !slot.admission_drain_pending(),
            "channel-close teardown must close the generated drain obligation before actor exit"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod inline_video_admission_tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::Provider;
    use meerkat_core::service::{
        DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService,
        StartTurnRequest,
    };
    use meerkat_core::types::{ContentBlock, VideoData};
    use std::sync::{Arc, Mutex};

    fn identity(provider: Provider, model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn inline_video_prompt() -> ContentInput {
        ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: "describe this".to_string(),
            },
            ContentBlock::Video {
                media_type: "video/mp4".to_string(),
                duration_ms: 1_000,
                data: VideoData::Inline {
                    data: "AAAA".to_string(),
                },
            },
        ])
    }

    struct BuilderIdentityProbe {
        identity: Option<SessionLlmIdentity>,
        committed_identity_before_turn_failure: Option<SessionLlmIdentity>,
        validated_identities: Arc<Mutex<Vec<SessionLlmIdentity>>>,
        run_result_session_id: Option<SessionId>,
    }

    struct BuilderIdentityAgent {
        session_id: SessionId,
        identity: Option<SessionLlmIdentity>,
        committed_identity_before_turn_failure: Option<SessionLlmIdentity>,
        run_result_session_id: Option<SessionId>,
        transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    }

    struct NoopAgentLlmClient {
        model: String,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl meerkat_core::AgentLlmClient for NoopAgentLlmClient {
        async fn stream_response(
            &self,
            _messages: &[meerkat_core::Message],
            _tools: &[Arc<meerkat_core::ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<meerkat_core::LlmStreamResult, AgentError> {
            Err(AgentError::ConfigError(
                "noop test client should not be called".to_string(),
            ))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        fn model(&self) -> &str {
            &self.model
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgentBuilder for BuilderIdentityProbe {
        type Agent = BuilderIdentityAgent;

        async fn model_supports_inline_video(&self, identity: &SessionLlmIdentity) -> Option<bool> {
            self.validated_identities
                .lock()
                .expect("validated identities lock poisoned")
                .push(identity.clone());
            Some(
                self.identity
                    .as_ref()
                    .is_some_and(|expected| identity == expected),
            )
        }

        async fn build_agent(
            &self,
            _req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(BuilderIdentityAgent {
                session_id: SessionId::new(),
                identity: self.identity.clone(),
                committed_identity_before_turn_failure: self
                    .committed_identity_before_turn_failure
                    .clone(),
                run_result_session_id: self.run_result_session_id.clone(),
                transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
            })
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionAgent for BuilderIdentityAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            if let Some(target) = self.committed_identity_before_turn_failure.take() {
                self.identity = Some(target.clone());
                return Err(AgentError::llm(
                    target.provider.as_str(),
                    meerkat_core::error::LlmFailureReason::InvalidModel(target.model.clone()),
                    "backup terminal failure",
                ));
            }

            // Deliberately mutate the identity reported by the agent whenever
            // an alternate result identity is configured. The session task
            // must compare against its pre-run canonical ID, not ask the agent
            // for a potentially changed ID after the turn.
            if let Some(run_result_session_id) = &self.run_result_session_id {
                self.session_id = run_result_session_id.clone();
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: self
                    .run_result_session_id
                    .clone()
                    .unwrap_or_else(|| self.session_id.clone()),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn meerkat_core::AgentLlmClient>,
            identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            self.identity = Some(identity);
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::error::AgentError> {
            Ok(meerkat_core::Session::new())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<SessionTranscriptAuthoritySnapshot, meerkat_core::error::AgentError> {
            let session = self.session_clone()?;
            SessionTranscriptAuthoritySnapshot::from_session(&session)
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            self.identity.clone()
        }

        fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
            self.transient_turn_context_state.clone()
        }
    }

    fn create_request(
        prompt: ContentInput,
        initial_turn: InitialTurnPolicy,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "providerless-video-alias".to_string(),
            prompt,
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    fn start_turn_request(prompt: ContentInput) -> StartTurnRequest {
        StartTurnRequest {
            injected_context: Vec::new(),
            prompt,
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
        }
    }

    #[test]
    fn provider_gemini_capability_false_rejects_inline_video() {
        let result = validate_prompt_video_input_against_capability(
            &inline_video_prompt(),
            &identity(Provider::Gemini, "gemini-3.5-flash"),
            false,
        );

        let message = match result {
            Err(SessionError::Agent(AgentError::ConfigError(message))) => Some(message),
            _ => None,
        };

        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("not supported by model"))
        );
    }

    #[test]
    fn provider_not_gemini_capability_true_accepts_inline_video() {
        let result = validate_prompt_video_input_against_capability(
            &inline_video_prompt(),
            &identity(Provider::OpenAI, "openai-video-capable-test-model"),
            true,
        );

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_session_rejects_builder_session_identity_mismatch_before_spawn() {
        let durable_identity = identity(Provider::Other, "identity-mismatch-test");
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(durable_identity),
                committed_identity_before_turn_failure: None,
                validated_identities: Arc::new(Mutex::new(Vec::new())),
                run_result_session_id: None,
            },
            1,
        );
        let expected_session = meerkat_core::Session::new();
        let expected_session_id = expected_session.id().clone();
        let mut request = create_request(
            ContentInput::Text("defer".to_string()),
            InitialTurnPolicy::Defer,
        );
        request
            .build
            .get_or_insert_with(Default::default)
            .resume_session = Some(expected_session);

        let error = service
            .create_session(request)
            .await
            .expect_err("builder must not replace the requested session identity");
        assert!(
            error
                .to_string()
                .contains("does not match requested session"),
            "unexpected identity validation error: {error}"
        );
        assert!(
            !service
                .has_live_session(&expected_session_id)
                .await
                .expect("live-session lookup"),
            "identity mismatch must fail before actor registration"
        );
    }

    #[tokio::test]
    async fn session_turn_rejects_agent_result_identity_mismatch_before_reply() {
        let durable_identity = identity(Provider::Other, "turn-identity-mismatch-test");
        let returned_session_id = SessionId::new();
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(durable_identity),
                committed_identity_before_turn_failure: None,
                validated_identities: Arc::new(Mutex::new(Vec::new())),
                run_result_session_id: Some(returned_session_id.clone()),
            },
            1,
        );

        let error = service
            .create_session(create_request(
                ContentInput::Text("run".to_string()),
                InitialTurnPolicy::RunImmediately,
            ))
            .await
            .expect_err("turn result must not replace the actor session identity");
        assert!(
            error.to_string().contains("agent turn returned session"),
            "unexpected turn identity validation error: {error}"
        );
        assert!(
            error.to_string().contains(&returned_session_id.to_string()),
            "identity error must identify the rejected result ID"
        );
    }

    #[tokio::test]
    async fn create_session_validates_initial_video_against_builder_identity() {
        let durable_identity = identity(Provider::Gemini, "providerless-video-alias");
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(durable_identity.clone()),
                committed_identity_before_turn_failure: None,
                validated_identities: Arc::clone(&validated_identities),
                run_result_session_id: None,
            },
            1,
        );

        let result = service
            .create_session(create_request(
                inline_video_prompt(),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("builder-owned Gemini identity should allow inline video");

        let live_identity = service
            .live_session_llm_identity(&result.session_id)
            .await
            .expect("live identity");
        assert_eq!(live_identity, durable_identity);
        assert_eq!(
            *validated_identities
                .lock()
                .expect("validated identities lock poisoned"),
            vec![durable_identity]
        );
    }

    #[tokio::test]
    async fn create_session_rejects_without_builder_identity() {
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: None,
                committed_identity_before_turn_failure: None,
                validated_identities: Arc::clone(&validated_identities),
                run_result_session_id: None,
            },
            1,
        );

        let result = service
            .create_session(create_request(
                ContentInput::Text("defer".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await;

        let message = match result {
            Err(SessionError::Agent(AgentError::ConfigError(message))) => Some(message),
            _ => None,
        };
        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("durable LLM identity"))
        );
        assert!(
            validated_identities
                .lock()
                .expect("validated identities lock poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn start_turn_validates_video_against_builder_seeded_live_identity() {
        let durable_identity = identity(Provider::Gemini, "providerless-video-alias");
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(durable_identity.clone()),
                committed_identity_before_turn_failure: None,
                validated_identities: Arc::clone(&validated_identities),
                run_result_session_id: None,
            },
            1,
        );

        let result = service
            .create_session(create_request(
                ContentInput::Text("defer".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        validated_identities
            .lock()
            .expect("validated identities lock poisoned")
            .clear();

        service
            .start_turn(
                &result.session_id,
                start_turn_request(inline_video_prompt()),
            )
            .await
            .expect("builder-seeded live identity should allow inline video turn");

        assert_eq!(
            *validated_identities
                .lock()
                .expect("validated identities lock poisoned"),
            vec![durable_identity]
        );
    }

    #[tokio::test]
    async fn failed_turn_republishes_settled_sticky_fallback_identity() {
        let primary_identity = identity(Provider::Anthropic, "primary-model");
        let backup_identity = identity(Provider::OpenAI, "backup-model");
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(primary_identity.clone()),
                committed_identity_before_turn_failure: Some(backup_identity.clone()),
                validated_identities: Arc::new(Mutex::new(Vec::new())),
                run_result_session_id: None,
            },
            1,
        );
        let result = service
            .create_session(create_request(
                ContentInput::Text("defer".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");
        assert_eq!(
            service
                .live_session_llm_identity(&result.session_id)
                .await
                .expect("initial live identity"),
            primary_identity
        );

        let turn_error = service
            .start_turn(
                &result.session_id,
                start_turn_request(ContentInput::Text("run".to_string())),
            )
            .await
            .expect_err("backup terminal failure must remain the turn result");
        assert!(matches!(
            turn_error,
            SessionError::Agent(AgentError::Llm { .. })
        ));
        assert_eq!(
            service
                .live_session_llm_identity(&result.session_id)
                .await
                .expect("settled fallback live identity"),
            backup_identity,
            "the live identity watch must mirror the committed fallback even when its retry fails"
        );
    }

    #[tokio::test]
    async fn hot_swap_replaces_builder_seeded_live_identity() {
        let durable_identity = identity(Provider::Gemini, "providerless-video-alias");
        let validated_identities = Arc::new(Mutex::new(Vec::new()));
        let service = EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(durable_identity),
                committed_identity_before_turn_failure: None,
                validated_identities,
                run_result_session_id: None,
            },
            1,
        );
        let result = service
            .create_session(create_request(
                ContentInput::Text("defer".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create session");

        let target_identity = identity(Provider::OpenAI, "gpt-5.4");
        service
            .hot_swap_session_llm_identity(
                &result.session_id,
                Arc::new(NoopAgentLlmClient {
                    model: target_identity.model.clone(),
                }),
                target_identity.clone(),
                meerkat_core::SessionLlmRequestPolicy {
                    model: target_identity.model.clone(),
                    credential_identity: None,
                    provider_params: None,
                    provider_tool_defaults: None,
                    provider_native_tools: meerkat_core::ProviderNativeToolPolicy::Inherit,
                },
            )
            .await
            .expect("hot-swap should update the live identity watch");

        let live_identity = service
            .live_session_llm_identity(&result.session_id)
            .await
            .expect("live identity");
        assert_eq!(live_identity, target_identity);
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn public_turn_identity_and_visibility_paths_share_the_runtime_finalization_gate() {
        let service = Arc::new(EphemeralSessionService::new(
            BuilderIdentityProbe {
                identity: Some(identity(Provider::OpenAI, "gate-probe")),
                committed_identity_before_turn_failure: None,
                validated_identities: Arc::new(Mutex::new(Vec::new())),
                run_result_session_id: None,
            },
            1,
        ));
        let created = service
            .create_session(create_request(
                ContentInput::Text("deferred gate probe".to_string()),
                InitialTurnPolicy::Defer,
            ))
            .await
            .expect("create deferred gate-probe session");
        let session_id = created.session_id;
        let held_guard = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;

        let mut start_turn_task = {
            let service = Arc::clone(&service);
            let session_id = session_id.clone();
            tokio::spawn(async move {
                SessionService::start_turn(
                    service.as_ref(),
                    &session_id,
                    start_turn_request(ContentInput::Text("blocked turn".to_string())),
                )
                .await
            })
        };
        let mut identity_task = {
            let service = Arc::clone(&service);
            let session_id = session_id.clone();
            tokio::spawn(async move {
                let target = identity(Provider::OpenAI, "gate-probe-target");
                SessionService::hot_swap_session_llm_identity(
                    service.as_ref(),
                    &session_id,
                    Arc::new(NoopAgentLlmClient {
                        model: target.model.clone(),
                    }),
                    target.clone(),
                    meerkat_core::SessionLlmRequestPolicy {
                        model: target.model,
                        credential_identity: None,
                        provider_params: None,
                        provider_tool_defaults: None,
                        provider_native_tools: meerkat_core::ProviderNativeToolPolicy::Inherit,
                    },
                )
                .await
            })
        };
        let mut visibility_task = {
            let service = Arc::clone(&service);
            let session_id = session_id.clone();
            tokio::spawn(async move {
                SessionService::set_session_tool_visibility_state(
                    service.as_ref(),
                    &session_id,
                    None,
                )
                .await
            })
        };

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if service
                    .read(&session_id)
                    .await
                    .expect("read preclaimed turn projection")
                    .state
                    .is_active
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("blocked public turn should publish its admission claim");
        let overlapping_turn = SessionService::start_turn(
            service.as_ref(),
            &session_id,
            start_turn_request(ContentInput::Text("overlapping turn".to_string())),
        )
        .await
        .expect_err("a second public turn must not queue behind the finalization gate");
        assert!(matches!(
            overlapping_turn,
            SessionError::Busy { ref id } if id == &session_id
        ));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut start_turn_task,)
                .await
                .is_err(),
            "public start_turn bypassed the held runtime-finalization gate"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut identity_task,)
                .await
                .is_err(),
            "public hot_swap_session_llm_identity bypassed the held runtime-finalization gate"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut visibility_task,)
                .await
                .is_err(),
            "public set_session_tool_visibility_state bypassed the held runtime-finalization gate"
        );
        {
            let gates = service.turn_finalization_gates.lock().await;
            assert_eq!(gates.len(), 1, "overlapping paths must share one gate");
            assert!(gates.contains_key(&session_id));
        }

        drop(held_guard);

        let start_turn_result =
            tokio::time::timeout(std::time::Duration::from_secs(1), start_turn_task)
                .await
                .expect("start_turn should proceed after releasing the gate")
                .expect("start_turn task should not panic");
        let identity_result =
            tokio::time::timeout(std::time::Duration::from_secs(1), identity_task)
                .await
                .expect("identity mutation should proceed after releasing the gate")
                .expect("identity mutation task should not panic");
        let visibility_result =
            tokio::time::timeout(std::time::Duration::from_secs(1), visibility_task)
                .await
                .expect("visibility mutation should proceed after releasing the gate")
                .expect("visibility mutation task should not panic");

        start_turn_result.expect("turn should proceed under the released boundary");
        identity_result.expect("identity mutation should proceed under the released boundary");
        assert!(matches!(
            visibility_result,
            Err(SessionError::Agent(AgentError::ConfigError(ref message)))
                if message == "tool visibility updates are not supported by this session agent"
        ));

        let replacement_id = SessionId::new();
        let replacement_guard = service
            .acquire_runtime_turn_finalization_guard(&replacement_id)
            .await;
        {
            let gates = service.turn_finalization_gates.lock().await;
            assert_eq!(gates.len(), 1, "dead weak gate entries must be reaped");
            assert!(!gates.contains_key(&session_id));
            assert!(gates.contains_key(&replacement_id));
        }
        drop(replacement_guard);
    }
}

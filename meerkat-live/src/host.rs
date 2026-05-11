//! Live adapter host — transport-side orchestrator for live provider sessions.
//!
//! E26: relocated from `meerkat-runtime` into `meerkat-live` so the dependency
//! direction matches the architectural intent — transport (`meerkat-live`)
//! owns the live-adapter seam directly and surfaces (`meerkat-rpc`) compose
//! the host with a `SessionService`-backed `LiveProjectionSink`. Previously
//! `meerkat-live` depended on `meerkat-runtime` to import the host, which
//! was the wrong direction (the runtime crate is heavyweight and shouldn't
//! be a transitive dep of a thin transport crate).
//!
//! Sits outside MeerkatMachine (not machine state, not a second machine).
//! Uses Meerkat services to build projections and gate observations.
//! Provider transport mechanics stay inside adapter implementations.
//!
//! ## Projection contract (wave-2 MVP)
//!
//! The host owns the seam between adapter observations and canonical Meerkat
//! semantic facts. `apply_observation` dispatches by [`ObservationRouting`]:
//!
//! - `AppendTranscript`  → writes to the injected [`LiveProjectionSink`]
//!   (user transcripts, assistant deltas/finals, turn-completed projection).
//! - `DispatchToolCall`  → routes through the injected
//!   [`LiveToolDispatcher`] and submits the result back via
//!   [`LiveAdapterCommand::SubmitToolResult`] / `SubmitToolError`.
//! - `SignalInterrupt`   → calls the sink's `signal_turn_interrupt` (the same
//!   path the user-facing interrupt RPC uses).
//! - `UpdateStatus`      → updates host-tracked status; pump task observed it.
//! - `TerminalError`     → terminalizes the channel (status `Closed`) and
//!   surfaces the error to the sink for session-level signalling.
//! - `Noop`              → no-op (e.g. bare audio chunks in the projection).
//!
//! `LiveProjectionSink` is the runtime-side abstraction over `SessionService`.
//! Wiring an implementation through `SessionService` is the surface side's
//! responsibility — the host trait is intentionally minimal so the same host
//! can be exercised in deterministic unit tests with a recorder fake.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::RealtimeTranscriptEvent;
use meerkat_core::SessionLlmIdentity;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::ToolError;
use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode,
    LiveAdapterObservation, LiveAdapterStatus, LiveInputChunk, LiveToolResult,
};
use meerkat_core::types::{SessionId, StopReason, ToolCall, ToolResult, Usage};
use serde_json::value::RawValue;
use tokio::sync::Mutex;

/// Opaque channel identifier for a live adapter session.
///
/// G41: contents are a v4 UUID minted at `LiveAdapterHost::open_channel` time
/// (via [`Self::random_uuid`]). The previous `live_{N}` `AtomicU64` shape
/// collided across `rkat-rpc` restarts and across instances sharing a host.
/// The newtype wraps `String` (not `Uuid`) so external callers — handlers
/// that round-trip the value through wire types and CLI surfaces that already
/// store opaque channel ids as strings — do not need a dependency on `uuid`.
/// `Self::new` is preserved for callers that already hold a serialized id
/// (test setup, deserialized wire frames).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiveChannelId(String);

impl LiveChannelId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Mint a fresh, globally-unique channel id (v4 UUID, hyphenated).
    ///
    /// G41: replaces the prior process-monotonic `live_{N}` shape so that
    /// channel ids are durable identifiers rather than process-local
    /// infrastructure handles.
    #[must_use]
    pub fn random_uuid() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LiveChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Projection sink — the host's escape hatch into canonical Meerkat semantics
// ---------------------------------------------------------------------------

/// Routed outcome of [`LiveAdapterHost::apply_observation`].
///
/// Returned to callers (e.g. the WS pump in `meerkat-rpc`) so they can both
/// observe what semantic action was taken and short-circuit follow-up work
/// (for instance, drop the channel after a `Terminal` outcome).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ObservationOutcome {
    /// Observation was a no-op at the projection level (audio chunk, idle event).
    Noop,
    /// Status was updated; new value is reported.
    StatusUpdated(LiveAdapterStatus),
    /// Transcript fragment was appended to canonical session history.
    TranscriptAppended,
    /// Assistant transcript was truncated (barge-in projection).
    TranscriptTruncated,
    /// A tool call was dispatched and its result was submitted back to the adapter.
    ToolCallDispatched {
        provider_call_id: String,
        tool_name: String,
    },
    /// A tool call was observed but no dispatcher was wired; nothing was sent back.
    ToolCallSkipped {
        provider_call_id: String,
        tool_name: String,
        reason: ToolDispatchSkipReason,
    },
    /// A tool call was dispatched but did not produce a result within the
    /// configured timeout. The adapter has been notified via
    /// `LiveAdapterCommand::SubmitToolError` so the provider can unblock its
    /// turn; no `ToolDispatched` outcome is projected (dogma sin #3 — the
    /// runtime decides the semantic consequence of a stuck tool).
    ToolCallTimedOut {
        provider_call_id: String,
        tool_name: String,
        timeout: std::time::Duration,
    },
    /// Turn-interrupt signal was forwarded to the projection sink.
    InterruptSignalled,
    /// Channel was terminalized (terminal error or `Closed`).
    Terminal { code: LiveAdapterErrorCode },
    /// R5-9: a per-command failure scoped to the offending command —
    /// e.g. an unsupported `LiveInputChunk::Image` rejected by the
    /// provider's local guard. The channel survives; the WS pump
    /// forwards the typed JSON observation to the client and continues
    /// the loop. Sibling of [`Self::Terminal`] for the typed
    /// `LiveAdapterObservation::CommandRejected` variant.
    CommandRejected {
        code: LiveAdapterErrorCode,
        message: String,
    },
}

/// Why a tool call observation was not dispatched. Distinguishes "no
/// dispatcher injected" (config gap) from "dispatcher rejected the call"
/// (downstream failure projected as a tool-error result).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolDispatchSkipReason {
    NoDispatcher,
    InvalidArguments,
}

/// Runtime-side projection sink consumed by [`LiveAdapterHost`].
///
/// This is the seam through which adapter observations become canonical
/// Meerkat semantic facts. Surfaces (RPC/REST/WASM) provide an implementation
/// that bridges into [`meerkat_core::SessionService`] / `EventInjector` /
/// observer streams. Tests use [`RecordingProjectionSink`] (in this module's
/// `#[cfg(test)]` block) to assert routing decisions deterministically.
/// Provider-side identity carried alongside a transcript fragment.
///
/// All fields are `Option<&str>` / `Option<u32>` because the underlying
/// `LiveAdapterObservation` variants emit them as `#[serde(default,
/// skip_serializing_if = "Option::is_none")]` per A11. Bundling them in one
/// struct keeps sink trait signatures readable and lets sink implementations
/// pattern-match the fields they care about without the call sites passing
/// six separate arguments.
///
/// Variants:
/// - `UserTranscriptFinal` carries `provider_item_id`, `previous_item_id`,
///   `content_index` (no `response_id` / `delta_id` — those are
///   assistant-only).
/// - `AssistantTextDelta` carries the full set including `response_id` and
///   `delta_id`.
/// - `AssistantTranscriptFinal` carries `provider_item_id` (always present),
///   `previous_item_id`, `content_index`, `response_id` (no `delta_id` — a
///   final is not a delta).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveTranscriptIdentity<'a> {
    pub provider_item_id: Option<&'a str>,
    pub previous_item_id: Option<&'a str>,
    pub content_index: Option<u32>,
    pub response_id: Option<&'a str>,
    pub delta_id: Option<&'a str>,
}

impl<'a> LiveTranscriptIdentity<'a> {
    /// Build identity for a user-side observation (response/delta IDs are
    /// always `None` on the user side).
    pub fn user(
        provider_item_id: Option<&'a str>,
        previous_item_id: Option<&'a str>,
        content_index: Option<u32>,
    ) -> Self {
        Self {
            provider_item_id,
            previous_item_id,
            content_index,
            response_id: None,
            delta_id: None,
        }
    }

    /// Build identity for a streaming assistant delta (full set carried).
    pub fn assistant_delta(
        provider_item_id: Option<&'a str>,
        previous_item_id: Option<&'a str>,
        content_index: Option<u32>,
        response_id: Option<&'a str>,
        delta_id: Option<&'a str>,
    ) -> Self {
        Self {
            provider_item_id,
            previous_item_id,
            content_index,
            response_id,
            delta_id,
        }
    }

    /// Build identity for an assistant final (no `delta_id`, but everything
    /// else is meaningful and `provider_item_id` is required upstream).
    pub fn assistant_final(
        provider_item_id: &'a str,
        previous_item_id: Option<&'a str>,
        content_index: Option<u32>,
        response_id: Option<&'a str>,
    ) -> Self {
        Self {
            provider_item_id: Some(provider_item_id),
            previous_item_id,
            content_index,
            response_id,
            delta_id: None,
        }
    }
}

#[async_trait::async_trait]
pub trait LiveProjectionSink: Send + Sync {
    /// Append a finalized user transcript fragment to canonical session history.
    async fn append_user_transcript(
        &self,
        session_id: &SessionId,
        text: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError>;

    /// Append a streaming **display-text** delta (authored output) to
    /// canonical session history. Flushed as `AssistantBlock::Text` at
    /// turn boundary.
    ///
    /// T6: distinct from [`Self::append_assistant_transcript_delta`] —
    /// display text is preserved across barge-in (the user is not
    /// "speaking over" written output).
    async fn append_assistant_text_delta(
        &self,
        session_id: &SessionId,
        delta: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError>;

    /// Append a streaming **spoken-transcript** delta (audio-derived
    /// output) to canonical session history. Flushed as
    /// `AssistantBlock::Transcript { source: Spoken, .. }` at turn
    /// boundary.
    ///
    /// T6: distinct from [`Self::append_assistant_text_delta`] — the
    /// transcript lane is what the model *said*, not what it *wrote*, and
    /// is dropped on barge-in (T7).
    async fn append_assistant_transcript_delta(
        &self,
        session_id: &SessionId,
        delta: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError>;

    /// Append a finalized **display-text** assistant block (with stop
    /// reason + usage). Flushed as `AssistantBlock::Text` when
    /// [`Self::signal_turn_completed`] drains the per-response buffer.
    ///
    /// R6: the trait passes `response_id` through so the sink can key its
    /// per-turn buffer on `(SessionId, response_id)` — a stale or
    /// overlapping `response.done` cannot flush the wrong buffered final.
    async fn append_assistant_text_final(
        &self,
        session_id: &SessionId,
        text: &str,
        identity: LiveTranscriptIdentity<'_>,
        stop_reason: StopReason,
        usage: Usage,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError>;

    /// Append a finalized **spoken-transcript** assistant block (with stop
    /// reason + usage). Flushed as
    /// `AssistantBlock::Transcript { source: Spoken, .. }` when
    /// [`Self::signal_turn_completed`] drains the per-response buffer.
    ///
    /// R6: same `(SessionId, response_id)` keying as the text-final
    /// variant. T7: barge-in drains buffered transcript blocks but leaves
    /// buffered text blocks untouched.
    async fn append_assistant_transcript_final(
        &self,
        session_id: &SessionId,
        text: &str,
        identity: LiveTranscriptIdentity<'_>,
        stop_reason: StopReason,
        usage: Usage,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError>;

    /// Project an assistant transcript truncation (barge-in side effect).
    ///
    /// `response_id` and `content_index` are the provider-supplied identity
    /// fields the runtime's realtime-transcript layer needs to fold the
    /// truncation into the staged response. When `response_id` is `None`,
    /// implementors must surface a typed [`LiveProjectionError::Rejected`]
    /// rather than silently committing an empty value (P1#3).
    async fn truncate_assistant_transcript(
        &self,
        session_id: &SessionId,
        provider_item_id: Option<&str>,
        previous_item_id: Option<&str>,
        content_index: Option<u32>,
        response_id: Option<&str>,
        text: Option<&str>,
    ) -> Result<(), LiveProjectionError>;

    /// Project a turn-interrupt fact through the same path the user-facing
    /// interrupt RPC uses.
    ///
    /// G4 (P1): `response_id` carries the in-flight provider response id from
    /// [`LiveAdapterObservation::TurnInterrupted`]. When the barge-in arrives
    /// before any transcript delta has been staged, the sink would otherwise
    /// have nothing to bind the truncation to; with the response id plumbed
    /// through, the sink can scope truncation to the right response. Optional
    /// because not every adapter or interrupt source surfaces a response id
    /// (synthetic interrupts, transcript-only providers, the user-facing
    /// `live/interrupt` RPC).
    async fn signal_turn_interrupt(
        &self,
        session_id: &SessionId,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError>;

    /// Mark the live turn complete in canonical session state.
    ///
    /// R6: `response_id` carries the provider's response identifier from
    /// [`LiveAdapterObservation::TurnCompleted`] so the sink can drain the
    /// matching `(SessionId, response_id)` buffer slot — not just by
    /// session id, which lets a stale or overlapping `response.done` flush
    /// the wrong buffered transcript.
    async fn signal_turn_completed(
        &self,
        session_id: &SessionId,
        stop_reason: StopReason,
        usage: Usage,
        response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError>;

    /// Surface a terminal adapter error at the session level.
    async fn signal_terminal_error(
        &self,
        session_id: &SessionId,
        code: LiveAdapterErrorCode,
        message: &str,
    ) -> Result<(), LiveProjectionError>;

    /// Append a structured realtime transcript event to canonical session state.
    ///
    /// P1#2: provider adapters now emit `LiveAdapterObservation::RealtimeTranscript`
    /// for events the host previously dropped on the floor (`ItemObserved`,
    /// `ItemSkipped`, `AssistantTurnCompleted`, `AssistantTurnInterrupted`).
    /// Routing them through this seam wires those events into the session
    /// runtime's idempotent ordering / staging machinery — the same path that
    /// already handles streaming assistant deltas.
    ///
    /// R6-6 (P3 dogma): no default body. Every sink — production, no-op, and
    /// test fakes alike — must explicitly handle realtime transcript events.
    /// A default `Ok(())` would let a forgetful "mandatory" sink silently
    /// drop `ItemObserved` / `AssistantTurnCompleted` / etc., which is the
    /// same fail-open shape R5-1 closed at construction time.
    async fn append_realtime_transcript(
        &self,
        session_id: &SessionId,
        event: &RealtimeTranscriptEvent,
    ) -> Result<(), LiveProjectionError>;
}

/// Errors returned by [`LiveProjectionSink`] implementations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LiveProjectionError {
    #[error("session {0} not found")]
    SessionNotFound(SessionId),
    #[error("projection rejected: {0}")]
    Rejected(String),
    #[error("projection sink internal error: {0}")]
    Internal(String),
}

/// No-op projection sink for tests and degraded configurations that
/// intentionally opt out of routing live observations to a canonical
/// session owner.
///
/// R5-1 (P2 dogma): every [`LiveAdapterHost`] now carries a mandatory
/// projection sink so a successful "transcript projected" outcome
/// implies a real semantic owner. Callers that genuinely want
/// projection to drop on the floor (host smoke tests, channel-id
/// lifecycle tests, transport plumbing tests) opt in **explicitly**
/// by constructing this sink — the lack of a sink is no longer a
/// silent fail-open.
///
/// Marked `#[doc(hidden)]` to discourage non-test use; production
/// surfaces (`rkat-rpc`) wire `SessionServiceProjectionSink` instead.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct NoOpProjectionSink;

#[async_trait::async_trait]
impl LiveProjectionSink for NoOpProjectionSink {
    async fn append_user_transcript(
        &self,
        _session_id: &SessionId,
        _text: &str,
        _identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn append_assistant_text_delta(
        &self,
        _session_id: &SessionId,
        _delta: &str,
        _identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn append_assistant_transcript_delta(
        &self,
        _session_id: &SessionId,
        _delta: &str,
        _identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn append_assistant_text_final(
        &self,
        _session_id: &SessionId,
        _text: &str,
        _identity: LiveTranscriptIdentity<'_>,
        _stop_reason: StopReason,
        _usage: Usage,
        _response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn append_assistant_transcript_final(
        &self,
        _session_id: &SessionId,
        _text: &str,
        _identity: LiveTranscriptIdentity<'_>,
        _stop_reason: StopReason,
        _usage: Usage,
        _response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn truncate_assistant_transcript(
        &self,
        _session_id: &SessionId,
        _provider_item_id: Option<&str>,
        _previous_item_id: Option<&str>,
        _content_index: Option<u32>,
        _response_id: Option<&str>,
        _text: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn signal_turn_interrupt(
        &self,
        _session_id: &SessionId,
        _response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn signal_turn_completed(
        &self,
        _session_id: &SessionId,
        _stop_reason: StopReason,
        _usage: Usage,
        _response_id: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    async fn signal_terminal_error(
        &self,
        _session_id: &SessionId,
        _code: LiveAdapterErrorCode,
        _message: &str,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }

    /// R6-6 (P3 dogma): explicit no-op. The whole point of [`NoOpProjectionSink`]
    /// is that no canonical projection happens — making this an explicit body
    /// (rather than relying on a trait default) means the dogma is visible at
    /// the implementation site, not hidden in a default that future fakes
    /// could silently inherit.
    async fn append_realtime_transcript(
        &self,
        _session_id: &SessionId,
        _event: &RealtimeTranscriptEvent,
    ) -> Result<(), LiveProjectionError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Per-channel state
// ---------------------------------------------------------------------------

/// Closed channels are retained for [`CLOSED_CHANNEL_TTL`] after
/// `close_channel` so post-close `live/status` can report `Closed { reason }`
/// instead of `ChannelNotFound` (G42).
const CLOSED_CHANNEL_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Per-channel state tracked by the host.
struct ChannelState {
    session_id: SessionId,
    /// Source-of-truth status. Driven by adapter observations after attach,
    /// not by the host asserting `Ready` at attach time (F32, F33).
    status: LiveAdapterStatus,
    snapshot_version: u64,
    adapter: Option<Arc<dyn LiveAdapter>>,
    /// When `Some`, the channel was closed and is retained until this instant
    /// (G42). Reads are still serviced; commands are rejected.
    retire_at: Option<std::time::Instant>,
    /// R11: LLM identity the channel was opened with.
    ///
    /// Recorded by the surface (`live/open` handler) immediately after
    /// `attach_adapter` so `propagate_config_to_live_channels` can detect a
    /// model/provider swap on `config/patch`. The OpenAI realtime adapter
    /// rejects model/provider drift via its R1 guard (the realtime
    /// `session.update` event cannot change model). Without a recorded
    /// identity here the runtime would unconditionally enqueue Refresh and
    /// the channel would land in `ConfigRejected` error state instead of
    /// closing cleanly so the SDK can reopen.
    ///
    /// `None` for channels opened before identity recording was wired
    /// (degraded test configs, factory-less paths). On `None` the runtime
    /// falls back to legacy Refresh routing.
    bound_llm_identity: Option<SessionLlmIdentity>,
    /// CC1 (R11 wire-signal): one-shot synthetic observation to deliver to
    /// the next [`LiveAdapterHost::next_observation_raw`] caller before any
    /// adapter poll happens.
    ///
    /// Populated by [`LiveAdapterHost::signal_terminal_error`] so a typed
    /// terminal error (e.g. `ConfigRejected { reason: "model_swap: ..." }`)
    /// flows through the same WS pump path as a provider-emitted Error
    /// observation. Without this, runtime-side terminations only show up as
    /// a TCP close from `close_channel`, which clients cannot distinguish
    /// from a network drop.
    ///
    /// Single-slot by design: there's exactly one terminal moment per
    /// channel; subsequent `signal_terminal_error` calls overwrite (the
    /// channel is being torn down either way). Read priority — see
    /// `next_observation_raw` — must come before `adapter_for` so the
    /// synthetic obs survives a concurrent `close_channel`.
    pending_synthetic_obs: Option<LiveAdapterObservation>,
}

/// Errors from the live adapter host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LiveAdapterHostError {
    #[error("channel {0} not found")]
    ChannelNotFound(LiveChannelId),
    #[error("session {0} not found")]
    SessionNotFound(SessionId),
    #[error("channel {0} is not ready (status: {1:?})")]
    ChannelNotReady(LiveChannelId, LiveAdapterStatus),
    #[error("session {0} already has an active channel")]
    SessionAlreadyBound(SessionId),
    #[error("no adapter attached to channel {0}")]
    NoAdapter(LiveChannelId),
    /// E29: typed adapter error preserved structurally (not flattened to String).
    #[error(transparent)]
    AdapterError(#[from] LiveAdapterError),
    #[error("projection sink error: {0}")]
    ProjectionError(#[from] LiveProjectionError),
}

/// Observation routing decision — what the host does with an adapter
/// observation.
///
/// Marked `#[non_exhaustive]` (H49) because new variants will be added as
/// the projection grows (e.g. provider-native barge-in audio cursor).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ObservationRouting {
    AppendTranscript,
    /// Pass-through of a structured [`RealtimeTranscriptEvent`] from the
    /// provider adapter. Routed to [`LiveProjectionSink::append_realtime_transcript`]
    /// so the session layer's idempotent ordering / staging machinery owns
    /// materialization (P1#2). Replaces the prior `Noop` fallthrough that
    /// silently dropped these structured events.
    AppendRealtimeTranscript {
        event: RealtimeTranscriptEvent,
    },
    DispatchToolCall {
        provider_call_id: String,
        tool_name: String,
    },
    SignalInterrupt,
    UpdateStatus(LiveAdapterStatus),
    TerminalError,
    /// R5-9: a non-terminal scoped command rejection. Distinct from
    /// [`Self::TerminalError`] so the host's projection refuses to
    /// close the channel for typed-input failures the client can retry.
    CommandRejection,
    Noop,
}

/// Session-scoped tool executor used by [`LiveAdapterHost`].
///
/// The provider only reports a tool call and a live channel id. The host
/// resolves that channel back to the owning [`SessionId`] and hands the call to
/// this trait. Production surfaces should implement it by delegating to the
/// canonical session service / runtime tool dispatcher; tests and compatibility
/// adapters may wrap a plain [`AgentToolDispatcher`].
#[async_trait::async_trait]
pub trait LiveToolDispatcher: Send + Sync {
    async fn dispatch_live_tool_call(
        &self,
        session_id: &SessionId,
        call: ToolCall,
    ) -> Result<ToolDispatchOutcome, LiveToolDispatchError>;
}

/// Error returned by a [`LiveToolDispatcher`].
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum LiveToolDispatchError {
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("live tool dispatch rejected: {0}")]
    Rejected(String),
    #[error("live tool dispatch internal error: {0}")]
    Internal(String),
}

struct AgentLiveToolDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
}

impl AgentLiveToolDispatcher {
    fn new(inner: Arc<dyn AgentToolDispatcher>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl LiveToolDispatcher for AgentLiveToolDispatcher {
    async fn dispatch_live_tool_call(
        &self,
        _session_id: &SessionId,
        call: ToolCall,
    ) -> Result<ToolDispatchOutcome, LiveToolDispatchError> {
        let args_string = serde_json::to_string(&call.args).map_err(|err| {
            LiveToolDispatchError::Internal(format!(
                "failed to serialize live tool-call arguments: {err}"
            ))
        })?;
        let raw = RawValue::from_string(args_string).map_err(|err| {
            LiveToolDispatchError::Internal(format!(
                "failed to create live tool-call argument payload: {err}"
            ))
        })?;
        let view = meerkat_core::types::ToolCallView {
            id: &call.id,
            name: &call.name,
            args: raw.as_ref(),
        };
        self.inner.dispatch(view).await.map_err(Into::into)
    }
}

/// Runtime-owned host for live provider adapter sessions.
///
/// This is NOT MeerkatMachine state (dogma: adapter must not become a
/// second machine). It's a runtime orchestrator that:
/// - Owns the map of active adapter channels and their adapters
/// - Builds projection snapshots from canonical session state
/// - Routes adapter observations to the right Meerkat API
/// - Exposes transport bootstrap info for the surface API
pub struct LiveAdapterHost {
    inner: Mutex<HostInner>,
    // G41: removed `next_channel_id: AtomicU64` — channel ids are now v4 UUIDs
    // minted by `LiveChannelId::random_uuid()` so the per-process counter is
    // dead weight (and would imply a process-monotonic guarantee the new ids
    // intentionally do not provide).
    /// Mandatory projection sink (R5-1 P2 dogma).
    ///
    /// A successful [`ObservationOutcome::TranscriptAppended`] /
    /// [`ObservationOutcome::InterruptSignalled`] / [`ObservationOutcome::Terminal`]
    /// outcome implies a real semantic owner received the projection.
    /// Surfaces that genuinely want projections to drop on the floor
    /// (host smoke tests, channel-id lifecycle tests) opt in explicitly
    /// by constructing the host with [`NoOpProjectionSink`] — the
    /// lack of a sink is no longer a silent fail-open hidden in an
    /// `Option`.
    projection_sink: Arc<dyn LiveProjectionSink>,
    /// Late-bindable session-scoped live tool dispatcher.
    ///
    /// `Mutex<Option<...>>` rather than the prior plain `Option<...>` because
    /// the dispatcher may be constructed after the host has already been
    /// wrapped by a transport state. A builder-only `with_*` API can't reach
    /// that point, so we expose [`set_live_tool_dispatcher`] as a late setter.
    /// Reads are short and fully sync — never held across an `.await`.
    tool_dispatcher: std::sync::Mutex<Option<Arc<dyn LiveToolDispatcher>>>,
    /// Optional per-tool-call dispatch timeout. When set, `dispatch_tool_call`
    /// races the dispatcher future against
    /// [`tokio::time::timeout`](tokio::time::timeout); on elapse, the host
    /// submits a typed `LiveAdapterCommand::SubmitToolError` to the adapter
    /// and returns [`ObservationOutcome::ToolCallTimedOut`] instead of
    /// `ToolCallDispatched`. The runtime is therefore not deadlockable by a
    /// dispatcher that holds a tool call forever (dogma: the adapter cannot
    /// stall canonical session lifecycle).
    ///
    /// `None` (the default) preserves legacy unbounded-await behavior. Surfaces
    /// that want a deadline budget explicitly opt in via
    /// [`Self::with_tool_timeout`].
    tool_timeout: Option<Duration>,
}

/// Default tool-call dispatch timeout used by surfaces that opt into a
/// timeout without specifying a value. Picked to be long enough to
/// accommodate reasonable tool work (HTTP fetches, database calls, model
/// calls invoked from a tool) but short enough that a stuck dispatcher
/// cannot indefinitely hold a live provider's turn.
pub const DEFAULT_LIVE_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

struct HostInner {
    /// `IndexMap` keeps insertion order stable for `active_channels` (which a
    /// few tests rely on as a deterministic projection).
    channels: IndexMap<LiveChannelId, ChannelState>,
    /// N80: O(1) reverse lookup so `open_channel` does not linear-scan the
    /// `channels` map under the host mutex when binding by `SessionId`.
    by_session: HashMap<SessionId, LiveChannelId>,
}

impl LiveAdapterHost {
    /// Construct a new host with the given projection sink.
    ///
    /// R5-1 (P2 dogma): the sink is mandatory at construction so
    /// "projection appended" success outcomes always imply a real
    /// semantic owner. Tests / smoke configs that intentionally
    /// drop projections opt in explicitly via
    /// `LiveAdapterHost::new(Arc::new(NoOpProjectionSink))`.
    #[must_use]
    pub fn new(projection_sink: Arc<dyn LiveProjectionSink>) -> Self {
        Self {
            inner: Mutex::new(HostInner {
                channels: IndexMap::new(),
                by_session: HashMap::new(),
            }),
            projection_sink,
            tool_dispatcher: std::sync::Mutex::new(None),
            tool_timeout: None,
        }
    }

    /// Builder: install a per-tool-call dispatch timeout.
    ///
    /// When set, every `ToolCallRequested` observation triggers a dispatcher
    /// call wrapped in [`tokio::time::timeout`]; if the dispatcher does not
    /// produce a result before the deadline, the host:
    ///
    /// 1. Sends a typed `LiveAdapterCommand::SubmitToolError` to the adapter
    ///    so the provider can unblock the live turn.
    /// 2. Returns [`ObservationOutcome::ToolCallTimedOut`] (not
    ///    `ToolCallDispatched`) so the projection layer can audit the miss.
    ///
    /// Surfaces that omit this builder retain the prior unbounded-await
    /// behavior. [`DEFAULT_LIVE_TOOL_TIMEOUT`] is the recommended default.
    #[must_use]
    pub fn with_tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = Some(timeout);
        self
    }

    /// Read the configured per-tool-call dispatch timeout.
    ///
    /// `None` indicates the host runs with unbounded await behavior. Surfaces
    /// (notably `rkat-rpc`) MUST install [`DEFAULT_LIVE_TOOL_TIMEOUT`] at
    /// startup; this accessor is the test seam that pins that contract.
    #[must_use]
    pub fn tool_timeout(&self) -> Option<Duration> {
        self.tool_timeout
    }

    /// Builder: install a legacy agent tool dispatcher.
    ///
    /// Without a dispatcher, observed tool calls return
    /// `ObservationOutcome::ToolCallSkipped { reason: NoDispatcher, .. }`.
    /// This compatibility helper wraps the dispatcher in a
    /// [`LiveToolDispatcher`] that ignores the session id. Production surfaces
    /// should prefer [`Self::with_live_tool_dispatcher`] so tool execution can
    /// flow through the session-owned dispatcher.
    #[must_use]
    pub fn with_tool_dispatcher(self, dispatcher: Arc<dyn AgentToolDispatcher>) -> Self {
        self.set_tool_dispatcher(dispatcher);
        self
    }

    /// Builder: install the session-scoped live tool dispatcher.
    #[must_use]
    pub fn with_live_tool_dispatcher(self, dispatcher: Arc<dyn LiveToolDispatcher>) -> Self {
        self.set_live_tool_dispatcher(dispatcher);
        self
    }

    /// Late setter for a legacy agent tool dispatcher.
    ///
    /// Kept for tests and compatibility. Runtime surfaces should install a
    /// session-scoped dispatcher with [`Self::set_live_tool_dispatcher`].
    pub fn set_tool_dispatcher(&self, dispatcher: Arc<dyn AgentToolDispatcher>) {
        self.set_live_tool_dispatcher(Arc::new(AgentLiveToolDispatcher::new(dispatcher)));
    }

    /// Late setter for the session-scoped live tool dispatcher.
    ///
    /// Surfaces (rkat-rpc) cannot construct the dispatcher until after the
    /// host has been wrapped inside `LiveWsState`, so a builder-only
    /// `with_live_tool_dispatcher` is unreachable for them. This setter
    /// accepts `&self` (no `mut`) and replaces whatever was previously installed.
    /// Subsequent `ToolCallRequested` observations dispatch through the
    /// new value; in-flight dispatches that already cloned an `Arc` to the
    /// previous dispatcher continue running with that one.
    pub fn set_live_tool_dispatcher(&self, dispatcher: Arc<dyn LiveToolDispatcher>) {
        // Lock is brief and never held across an .await — tool dispatch reads
        // load_dispatcher() which clones the Arc and drops the guard before
        // calling dispatch().
        if let Ok(mut slot) = self.tool_dispatcher.lock() {
            *slot = Some(dispatcher);
        }
        // Lock poisoning here would mean a previous panic while holding the
        // dispatcher lock — exceedingly unlikely (the only operations are
        // `Some(_)` writes and clone-on-read). Falling through silently
        // preserves the prior dispatcher rather than dropping the new wiring
        // on the floor; the late-set contract is "best effort install."
    }

    /// Snapshot the currently-installed dispatcher (clones the Arc).
    ///
    /// Returns `None` if no dispatcher has been installed yet, in which case
    /// `dispatch_tool_call` projects `ObservationOutcome::ToolCallSkipped {
    /// reason: NoDispatcher }`.
    fn load_dispatcher(&self) -> Option<Arc<dyn LiveToolDispatcher>> {
        self.tool_dispatcher
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone))
    }

    pub async fn open_channel(
        &self,
        session_id: SessionId,
    ) -> Result<LiveChannelId, LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);

        // N80: O(1) reverse-map lookup instead of linear scan over the
        // channels map. Only counts channels not in their post-close grace
        // window — a closed channel must not block the next open.
        if let Some(existing) = inner.by_session.get(&session_id).cloned()
            && let Some(channel) = inner.channels.get(&existing)
            && channel.retire_at.is_none()
        {
            return Err(LiveAdapterHostError::SessionAlreadyBound(session_id));
        }

        // G41: v4 UUID — globally-unique across `rkat-rpc` restarts and
        // co-tenant host instances. Replaced the prior `live_{N}` shape
        // (process-monotonic `AtomicU64`).
        let channel_id = LiveChannelId::random_uuid();

        inner.channels.insert(
            channel_id.clone(),
            ChannelState {
                session_id: session_id.clone(),
                status: LiveAdapterStatus::Opening,
                snapshot_version: 0,
                adapter: None,
                retire_at: None,
                bound_llm_identity: None,
                pending_synthetic_obs: None,
            },
        );
        inner.by_session.insert(session_id, channel_id.clone());

        Ok(channel_id)
    }

    /// Attach a live adapter to an open channel.
    ///
    /// F32: the channel does NOT become `Ready` here. Status remains
    /// `Opening` until the adapter emits `LiveAdapterObservation::Ready`,
    /// which is observed via `apply_observation`/`next_observation`.
    pub async fn attach_adapter(
        &self,
        channel_id: &LiveChannelId,
        adapter: Arc<dyn LiveAdapter>,
    ) -> Result<(), LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        if channel.retire_at.is_some() {
            return Err(LiveAdapterHostError::ChannelNotFound(channel_id.clone()));
        }
        channel.adapter = Some(adapter);
        // Intentionally NOT setting status to Ready (F32). Driven by
        // adapter observations.
        Ok(())
    }

    /// R11: record the LLM identity the channel was opened with.
    ///
    /// Called by `live/open` after `attach_adapter` succeeds so
    /// `propagate_config_to_live_channels` can detect a model / provider
    /// swap on a later `config/patch` and route to a clean close (so the
    /// SDK can reopen against the new identity) instead of issuing a
    /// `Refresh` the OpenAI realtime adapter is forced to reject.
    ///
    /// Idempotent — calling twice replaces the recorded identity. A future
    /// hot-swap path that mutates identity in place without closing would
    /// rewrite via this same setter.
    pub async fn set_channel_llm_identity(
        &self,
        channel_id: &LiveChannelId,
        identity: SessionLlmIdentity,
    ) -> Result<(), LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.bound_llm_identity = Some(identity);
        Ok(())
    }

    /// R11: read the LLM identity the channel was opened with.
    ///
    /// Returns `Ok(None)` when the channel was opened by a path that did
    /// not record identity (degraded factory-less config in tests, or a
    /// pre-R11 caller). The runtime treats `None` as "do not assume a
    /// swap" and falls through to the legacy Refresh path.
    pub async fn channel_llm_identity(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<Option<SessionLlmIdentity>, LiveAdapterHostError> {
        let inner = self.inner.lock().await;
        inner
            .channels
            .get(channel_id)
            .map(|ch| ch.bound_llm_identity.clone())
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    /// Send a command to the adapter on a channel.
    pub async fn send_command(
        &self,
        channel_id: &LiveChannelId,
        command: LiveAdapterCommand,
    ) -> Result<(), LiveAdapterHostError> {
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ false)
            .await?;
        adapter.send_command(command).await?;
        Ok(())
    }

    /// Send an input chunk to the adapter on a channel.
    ///
    /// F31: rejects when the host-tracked status does not `accepts_commands()`
    /// (i.e. channel is not in `Ready`). Returns `ChannelNotReady` so callers
    /// can map to a typed wire error.
    pub async fn send_input(
        &self,
        channel_id: &LiveChannelId,
        chunk: LiveInputChunk,
    ) -> Result<(), LiveAdapterHostError> {
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ true)
            .await?;
        adapter
            .send_command(LiveAdapterCommand::SendInput { chunk })
            .await?;
        Ok(())
    }

    /// Submit a tool result back to the adapter on a channel.
    ///
    /// Used by the projection contract after [`AgentToolDispatcher::dispatch`]
    /// produces a result for a `ToolCallRequested` observation (A5).
    pub async fn submit_tool_result(
        &self,
        channel_id: &LiveChannelId,
        result: LiveToolResult,
    ) -> Result<(), LiveAdapterHostError> {
        self.send_command(channel_id, LiveAdapterCommand::SubmitToolResult { result })
            .await
    }

    /// Submit a tool error back to the adapter on a channel.
    pub async fn submit_tool_error(
        &self,
        channel_id: &LiveChannelId,
        call_id: String,
        error: String,
    ) -> Result<(), LiveAdapterHostError> {
        self.send_command(
            channel_id,
            LiveAdapterCommand::SubmitToolError { call_id, error },
        )
        .await
    }

    /// Poll the next observation from the adapter on a channel and project it.
    ///
    /// Convenience wrapper around `next_observation_raw` + `apply_observation`.
    /// Surfaces that need to react to the typed [`ObservationOutcome`] (e.g.
    /// to drop a WS connection on `Terminal`) should call the two halves
    /// directly.
    pub async fn next_observation(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<Option<LiveAdapterObservation>, LiveAdapterHostError> {
        let obs = self.next_observation_raw(channel_id).await?;
        if let Some(ref obs) = obs {
            // Best-effort projection. Errors are surfaced by `apply_observation`
            // for callers that want to react; here we discard so the read API
            // stays compatible with existing handlers.
            let _ = self.apply_observation(channel_id, obs).await?;
        }
        Ok(obs)
    }

    /// Read the next adapter observation without applying it to canonical state.
    ///
    /// F34: when the adapter pump errors with a transport/closed/provider
    /// error, the host marks the channel terminal so subsequent
    /// `channel_status` reads do not return a stale `Ready`.
    pub async fn next_observation_raw(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<Option<LiveAdapterObservation>, LiveAdapterHostError> {
        // CC1: check for a one-shot synthetic observation pushed by
        // `signal_terminal_error` before polling the adapter. This makes
        // typed terminal errors (e.g. R11 model_swap → `ConfigRejected`)
        // observable to the WS pump even if the underlying adapter has
        // already been torn down by `close_channel` — the pending slot
        // sits in `ChannelState`, not on the adapter. R5-3 also relies
        // on this slot as a fallback for adapter implementations whose
        // `inject_observation` is a no-op (e.g. test stubs).
        {
            let mut inner = self.inner.lock().await;
            if let Some(channel) = inner.channels.get_mut(channel_id)
                && let Some(obs) = channel.pending_synthetic_obs.take()
            {
                return Ok(Some(obs));
            }
        }
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ false)
            .await?;
        match adapter.next_observation().await {
            Ok(Some(obs)) => Ok(Some(obs)),
            Ok(None) => {
                // R5-3 fallback: if the adapter closed before the
                // synthetic observation was injected (or the adapter
                // implementation is a no-op stub), one final pending
                // check delivers the typed event before the consumer
                // sees end-of-stream.
                let mut inner = self.inner.lock().await;
                if let Some(channel) = inner.channels.get_mut(channel_id)
                    && let Some(obs) = channel.pending_synthetic_obs.take()
                {
                    return Ok(Some(obs));
                }
                Ok(None)
            }
            Err(err) => {
                // R5-8: on adapter pump / transport failure, fully
                // retire the channel — set `status = Closed`, schedule
                // `retire_at`, drop the adapter Arc, and reuse the
                // R5-3 synthetic-error seam to surface a typed `Error`
                // observation to the consumer instead of a generic
                // disconnect. Without this the channel stays half-bound
                // (status=Closed, retire_at=None, adapter still held)
                // and `open_channel` rejects future bindings for the
                // same session id with `SessionAlreadyBound` — the
                // broken-session symptom.
                let synthetic = LiveAdapterObservation::Error {
                    code: LiveAdapterErrorCode::ProviderError,
                    message: format!("adapter read failure: {err}"),
                };
                {
                    let mut inner = self.inner.lock().await;
                    if let Some(channel) = inner.channels.get_mut(channel_id) {
                        channel.status = LiveAdapterStatus::Closed;
                        channel.retire_at = Some(std::time::Instant::now() + CLOSED_CHANNEL_TTL);
                        // Drop the adapter Arc so transport resources
                        // are released even though we keep the channel
                        // entry around for `live/status` reads until
                        // the TTL elapses (G42).
                        channel.adapter = None;
                    }
                }
                // Surface the synthetic Error first; the WS pump will
                // forward it to the client and close with a typed
                // `terminal:provider_error` reason via
                // `apply_observation`'s `Terminal` arm.
                Ok(Some(synthetic))
            }
        }
    }

    /// Project an adapter observation into canonical Meerkat semantic state.
    ///
    /// This is the heart of the projection contract (A1–A6, A10, A14):
    /// classify → dispatch → return a typed [`ObservationOutcome`] describing
    /// what was applied. Status updates are always written to host-tracked
    /// channel state. Transcript / tool / interrupt routing requires the
    /// host to be configured with the relevant injected seams.
    pub async fn apply_observation(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveAdapterObservation,
    ) -> Result<ObservationOutcome, LiveAdapterHostError> {
        let routing = Self::classify_observation(observation);

        // Always reflect status updates first so other readers see fresh
        // status even if the projection sink is not wired yet.
        if let ObservationRouting::UpdateStatus(ref status) = routing {
            self.apply_status_update(channel_id, status.clone()).await?;
        }

        let session_id = self.channel_session(channel_id).await?;

        match (routing, observation) {
            (ObservationRouting::Noop, _) => Ok(ObservationOutcome::Noop),

            (ObservationRouting::UpdateStatus(status), _) => {
                Ok(ObservationOutcome::StatusUpdated(status))
            }

            (
                ObservationRouting::AppendTranscript,
                LiveAdapterObservation::UserTranscriptFinal {
                    provider_item_id,
                    previous_item_id,
                    content_index,
                    text,
                    ..
                },
            ) => {
                let identity = LiveTranscriptIdentity::user(
                    provider_item_id.as_deref(),
                    previous_item_id.as_deref(),
                    *content_index,
                );
                self.projection_sink
                    .append_user_transcript(&session_id, text, identity)
                    .await?;
                Ok(ObservationOutcome::TranscriptAppended)
            }

            (
                ObservationRouting::AppendTranscript,
                LiveAdapterObservation::AssistantTextDelta {
                    provider_item_id,
                    previous_item_id,
                    content_index,
                    response_id,
                    delta_id,
                    delta,
                    ..
                },
            ) => {
                let identity = LiveTranscriptIdentity::assistant_delta(
                    provider_item_id.as_deref(),
                    previous_item_id.as_deref(),
                    *content_index,
                    response_id.as_deref(),
                    delta_id.as_deref(),
                );
                // T6: display text routes to the text lane; flushed as
                // AssistantBlock::Text.
                self.projection_sink
                    .append_assistant_text_delta(&session_id, delta, identity)
                    .await?;
                Ok(ObservationOutcome::TranscriptAppended)
            }

            (
                ObservationRouting::AppendTranscript,
                LiveAdapterObservation::AssistantTranscriptDelta {
                    provider_item_id,
                    previous_item_id,
                    content_index,
                    response_id,
                    delta_id,
                    delta,
                    ..
                },
            ) => {
                let identity = LiveTranscriptIdentity::assistant_delta(
                    provider_item_id.as_deref(),
                    previous_item_id.as_deref(),
                    *content_index,
                    response_id.as_deref(),
                    delta_id.as_deref(),
                );
                // T6: spoken transcript routes to the transcript lane;
                // flushed as AssistantBlock::Transcript { source: Spoken }.
                self.projection_sink
                    .append_assistant_transcript_delta(&session_id, delta, identity)
                    .await?;
                Ok(ObservationOutcome::TranscriptAppended)
            }

            (
                ObservationRouting::AppendTranscript,
                LiveAdapterObservation::AssistantTranscriptFinal {
                    provider_item_id,
                    previous_item_id,
                    content_index,
                    response_id,
                    text,
                    stop_reason,
                    usage,
                    ..
                },
            ) => {
                let identity = LiveTranscriptIdentity::assistant_final(
                    provider_item_id,
                    previous_item_id.as_deref(),
                    *content_index,
                    response_id.as_deref(),
                );
                // R6: forward the response_id so the sink keys its
                // per-turn buffer on (SessionId, response_id). T6:
                // spoken-transcript final routes to the transcript lane.
                self.projection_sink
                    .append_assistant_transcript_final(
                        &session_id,
                        text,
                        identity,
                        *stop_reason,
                        usage.clone(),
                        response_id.as_deref(),
                    )
                    .await?;
                Ok(ObservationOutcome::TranscriptAppended)
            }

            (
                ObservationRouting::AppendTranscript,
                LiveAdapterObservation::AssistantTranscriptTruncated {
                    provider_item_id,
                    previous_item_id,
                    content_index,
                    response_id,
                    text,
                },
            ) => {
                self.projection_sink
                    .truncate_assistant_transcript(
                        &session_id,
                        provider_item_id.as_deref(),
                        previous_item_id.as_deref(),
                        *content_index,
                        response_id.as_deref(),
                        text.as_deref(),
                    )
                    .await?;
                Ok(ObservationOutcome::TranscriptTruncated)
            }

            (
                ObservationRouting::AppendTranscript,
                LiveAdapterObservation::TurnCompleted {
                    response_id,
                    stop_reason,
                    usage,
                },
            ) => {
                // R6: pass the response_id through so the sink can
                // drain the matching (SessionId, response_id) buffer
                // slot, not just by session_id.
                self.projection_sink
                    .signal_turn_completed(
                        &session_id,
                        *stop_reason,
                        usage.clone(),
                        response_id.as_deref(),
                    )
                    .await?;
                Ok(ObservationOutcome::TranscriptAppended)
            }

            // P1#2: structured realtime transcript events flow through the
            // typed sink seam so the session runtime's idempotent ordering /
            // staging machinery owns materialization. Mirrors the seam wave-3
            // wired up for assistant deltas.
            (ObservationRouting::AppendRealtimeTranscript { event }, _) => {
                self.projection_sink
                    .append_realtime_transcript(&session_id, &event)
                    .await?;
                Ok(ObservationOutcome::TranscriptAppended)
            }

            (
                ObservationRouting::DispatchToolCall { .. },
                LiveAdapterObservation::ToolCallRequested {
                    provider_call_id,
                    tool_name,
                    arguments,
                },
            ) => {
                self.dispatch_tool_call(channel_id, provider_call_id, tool_name, arguments.clone())
                    .await
            }

            (
                ObservationRouting::SignalInterrupt,
                LiveAdapterObservation::TurnInterrupted { response_id },
            ) => {
                self.projection_sink
                    .signal_turn_interrupt(&session_id, response_id.as_deref())
                    .await?;
                Ok(ObservationOutcome::InterruptSignalled)
            }

            (
                ObservationRouting::TerminalError,
                LiveAdapterObservation::Error { code, message },
            ) => {
                // A10: terminalize host channel state alongside the
                // session-level signal so subsequent `live/status` reflects
                // truth and the channel is reapable.
                {
                    let mut inner = self.inner.lock().await;
                    if let Some(channel) = inner.channels.get_mut(channel_id) {
                        channel.status = LiveAdapterStatus::Closed;
                        channel.retire_at = Some(std::time::Instant::now() + CLOSED_CHANNEL_TTL);
                    }
                }
                self.projection_sink
                    .signal_terminal_error(&session_id, code.clone(), message)
                    .await?;
                Ok(ObservationOutcome::Terminal { code: code.clone() })
            }

            // R5-9: scoped per-command rejection. The channel must
            // survive — the client sent a typed-input variant the
            // provider local-guard rejected (e.g.
            // `LiveInputChunk::Image` against an audio-only model),
            // and the next valid command should land on the same
            // channel. We deliberately do NOT touch host channel
            // state (status, retire_at, adapter) and do NOT call
            // `signal_terminal_error` on the projection sink — those
            // are the terminal-only obligations.
            (
                ObservationRouting::CommandRejection,
                LiveAdapterObservation::CommandRejected { code, message },
            ) => Ok(ObservationOutcome::CommandRejected {
                code: code.clone(),
                message: message.clone(),
            }),

            // Routing said AppendTranscript but the observation kind didn't
            // match any of the variants we handle above. Fall through as a
            // no-op so adding a new transcript-shaped variant doesn't panic
            // here before the projection knows how to handle it.
            (ObservationRouting::AppendTranscript, _) => Ok(ObservationOutcome::Noop),

            // Routing/observation mismatches that should not occur — return
            // a no-op outcome so the adapter pump keeps going. (The
            // `classify_observation` function is the single source of truth;
            // any mismatch here is a bug in classification, not a runtime
            // condition we should panic on.)
            _ => Ok(ObservationOutcome::Noop),
        }
    }

    async fn dispatch_tool_call(
        &self,
        channel_id: &LiveChannelId,
        provider_call_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ObservationOutcome, LiveAdapterHostError> {
        let dispatcher = match self.load_dispatcher() {
            Some(d) => d,
            None => {
                // P2#2: without a dispatcher, the provider would otherwise
                // wait forever for a tool result that will never arrive,
                // deadlocking the live session until the provider's own
                // timeout (or never). Send a typed `SubmitToolError` so the
                // provider can complete the response with an error and
                // unstick the live turn. The typed
                // `ObservationOutcome::ToolCallSkipped { NoDispatcher }`
                // return is preserved so the host's audit trail still shows
                // the miswiring. Best-effort: if the adapter is not attached
                // (no channel adapter yet), swallow the error rather than
                // poisoning the projection — the original miswiring is the
                // root cause and is already audited via the outcome.
                let _ = self
                    .submit_tool_error(
                        channel_id,
                        provider_call_id.to_string(),
                        "live tool dispatcher not configured".to_string(),
                    )
                    .await;
                return Ok(ObservationOutcome::ToolCallSkipped {
                    provider_call_id: provider_call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    reason: ToolDispatchSkipReason::NoDispatcher,
                });
            }
        };

        let session_id = self.channel_session(channel_id).await?;
        let call = ToolCall::new(
            provider_call_id.to_string(),
            tool_name.to_string(),
            arguments,
        );

        // Race the dispatcher future against the configured timeout (if any).
        // Without a timeout, fall through to the legacy unbounded await so
        // existing surfaces (and tests that don't pause the clock) keep their
        // behavior.
        let dispatch_call = dispatcher.dispatch_live_tool_call(&session_id, call);
        let dispatch_result = match self.tool_timeout {
            Some(timeout) => match tokio::time::timeout(timeout, dispatch_call).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    // Notify the adapter so the provider can unblock its turn.
                    // The error string is a stable, parseable shape: "tool
                    // dispatch timeout after Ns" so downstream surfaces can
                    // route on it without parsing the dispatcher's error.
                    let error_text = format!("tool dispatch timeout after {timeout:?}");
                    self.submit_tool_error(channel_id, provider_call_id.to_string(), error_text)
                        .await?;
                    return Ok(ObservationOutcome::ToolCallTimedOut {
                        provider_call_id: provider_call_id.to_string(),
                        tool_name: tool_name.to_string(),
                        timeout,
                    });
                }
            },
            None => dispatch_call.await,
        };
        match dispatch_result {
            Ok(outcome) => {
                let live_result =
                    tool_result_from_dispatch(provider_call_id.to_string(), outcome.result);
                self.submit_tool_result(channel_id, live_result).await?;
            }
            Err(err) => {
                self.submit_tool_error(channel_id, provider_call_id.to_string(), err.to_string())
                    .await?;
            }
        }

        Ok(ObservationOutcome::ToolCallDispatched {
            provider_call_id: provider_call_id.to_string(),
            tool_name: tool_name.to_string(),
        })
    }

    /// Helper: fetch the live adapter for a channel, optionally enforcing
    /// `LiveAdapterStatus::accepts_commands()` (F31).
    async fn adapter_for(
        &self,
        channel_id: &LiveChannelId,
        require_ready: bool,
    ) -> Result<Arc<dyn LiveAdapter>, LiveAdapterHostError> {
        let inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        if channel.retire_at.is_some() {
            // Channel is in post-close grace: status reads still work, but
            // commands/observations target a removed adapter.
            return Err(LiveAdapterHostError::ChannelNotReady(
                channel_id.clone(),
                channel.status.clone(),
            ));
        }
        if require_ready && !channel.status.accepts_commands() {
            return Err(LiveAdapterHostError::ChannelNotReady(
                channel_id.clone(),
                channel.status.clone(),
            ));
        }
        let adapter = channel
            .adapter
            .as_ref()
            .ok_or_else(|| LiveAdapterHostError::NoAdapter(channel_id.clone()))?;
        Ok(Arc::clone(adapter))
    }

    /// CC1 / R11 wire-signal: enqueue a synthetic terminal `Error`
    /// observation on the channel and close the underlying adapter.
    ///
    /// The synthetic observation is queued on a per-channel one-shot slot
    /// that [`Self::next_observation_raw`] inspects before polling the
    /// adapter. The next read by the WS pump returns the synthetic Error,
    /// which the pump forwards to the client and routes through
    /// [`Self::apply_observation`] — yielding
    /// [`ObservationOutcome::Terminal`] and a typed `terminal:<slug>` close
    /// frame (slug derived from the serde tag, e.g. `config_rejected`).
    ///
    /// Use this when the runtime decides a channel must die for a typed
    /// reason that the provider adapter never produced (e.g. model swap on
    /// `config/patch`, where the OpenAI realtime adapter cannot rebind the
    /// model in-place). Without this seam clients only see a TCP close
    /// indistinguishable from a network drop.
    ///
    /// `message` defaults from the typed code: `ConfigRejected { reason }`
    /// uses `reason` directly, other variants fall back to the serde tag.
    /// The pending obs is enqueued FIRST, then `close_channel` is called —
    /// this is safe because `next_observation_raw` checks the pending slot
    /// before honoring `retire_at`, so the WS pump still sees the typed
    /// signal even after the adapter has been released.
    pub async fn signal_terminal_error(
        &self,
        channel_id: &LiveChannelId,
        code: LiveAdapterErrorCode,
    ) -> Result<(), LiveAdapterHostError> {
        let message = match &code {
            // R5-2: `reason` is a typed `LiveConfigRejectionReason`; render
            // via `Display` (which preserves the human-readable swap/text
            // contract callers previously read out of the `String` reason).
            LiveAdapterErrorCode::ConfigRejected { reason } => reason.to_string(),
            other => format!("{other:?}"),
        };
        let synthetic = LiveAdapterObservation::Error {
            code: code.clone(),
            message: message.clone(),
        };

        // R5-3: deliver the synthetic Error THROUGH the adapter's own
        // observation channel so an in-flight `next_observation()`
        // future on the WS pump returns the typed event before the
        // close-driven `None`. We do this in two complementary phases:
        //
        // 1. Stage the synthetic in `pending_synthetic_obs` so a
        //    subsequent `next_observation_raw` call (after channel
        //    close) still returns the typed event — adapters whose
        //    `inject_observation` is a no-op (test stubs) rely on this
        //    fallback path.
        // 2. Call the adapter's `inject_observation` to push the
        //    synthetic onto the live observation stream — this is what
        //    actually unsticks an awaiting WS pump on a real adapter.
        //
        // Order matters: stage FIRST, inject SECOND, close THIRD. If
        // injection fails (adapter already torn down), the fallback in
        // `next_observation_raw`'s pending check still surfaces the
        // typed event.
        let adapter = {
            let mut inner = self.inner.lock().await;
            let channel = inner
                .channels
                .get_mut(channel_id)
                .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
            channel.pending_synthetic_obs = Some(synthetic.clone());
            channel.adapter.clone()
        };
        if let Some(adapter) = adapter {
            // Best-effort: a half-closed adapter cannot accept the
            // injection, but the `pending_synthetic_obs` fallback above
            // covers that case.
            let _ = adapter.inject_observation(synthetic).await;
        }
        // Close the adapter so any in-flight provider activity is
        // released. The synthetic observation has already been pushed
        // onto the control channel (or staged in pending_synthetic_obs)
        // so it surfaces ahead of the close-driven end-of-stream.
        self.close_channel(channel_id).await
    }

    pub async fn close_channel(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<(), LiveAdapterHostError> {
        // G42: keep the channel reachable for `live/status` until the TTL
        // elapses. We unbind the adapter (releasing transport resources) and
        // mark the channel as `Closed`, but leave the entry in `channels`
        // so post-close reads can report the terminal status.
        let adapter = {
            let mut inner = self.inner.lock().await;
            Self::reap_retired_locked(&mut inner);
            let channel = inner
                .channels
                .get_mut(channel_id)
                .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
            let adapter = channel.adapter.take();
            channel.status = LiveAdapterStatus::Closed;
            channel.retire_at = Some(std::time::Instant::now() + CLOSED_CHANNEL_TTL);
            adapter
        };
        if let Some(adapter) = adapter {
            let _ = adapter.close().await;
        }
        Ok(())
    }

    pub async fn channel_status(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<LiveAdapterStatus, LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);
        inner
            .channels
            .get(channel_id)
            .map(|ch| ch.status.clone())
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    pub async fn channel_session(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<SessionId, LiveAdapterHostError> {
        let inner = self.inner.lock().await;
        inner
            .channels
            .get(channel_id)
            .map(|ch| ch.session_id.clone())
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    pub fn classify_observation(observation: &LiveAdapterObservation) -> ObservationRouting {
        match observation {
            LiveAdapterObservation::Ready => {
                ObservationRouting::UpdateStatus(LiveAdapterStatus::Ready)
            }
            LiveAdapterObservation::UserTranscriptFinal { .. } => {
                ObservationRouting::AppendTranscript
            }
            LiveAdapterObservation::AssistantTextDelta { .. } => {
                ObservationRouting::AppendTranscript
            }
            // T5/T6: spoken-transcript deltas route to the transcript lane,
            // distinct from display-text deltas above.
            LiveAdapterObservation::AssistantTranscriptDelta { .. } => {
                ObservationRouting::AppendTranscript
            }
            LiveAdapterObservation::AssistantAudioChunk { .. } => ObservationRouting::Noop,
            LiveAdapterObservation::AssistantTranscriptFinal { .. } => {
                ObservationRouting::AppendTranscript
            }
            LiveAdapterObservation::AssistantTranscriptTruncated { .. } => {
                ObservationRouting::AppendTranscript
            }
            // P1#2: structured realtime events flow through the typed
            // realtime-transcript seam so the session layer owns idempotent
            // ordering. Without this route, ItemObserved / ItemSkipped /
            // AssistantTurnCompleted / AssistantTurnInterrupted dropped to
            // `Noop` and bypassed canonical staging.
            LiveAdapterObservation::RealtimeTranscript { event } => {
                ObservationRouting::AppendRealtimeTranscript {
                    event: event.clone(),
                }
            }
            LiveAdapterObservation::ToolCallRequested {
                provider_call_id,
                tool_name,
                ..
            } => ObservationRouting::DispatchToolCall {
                provider_call_id: provider_call_id.clone(),
                tool_name: tool_name.clone(),
            },
            LiveAdapterObservation::TurnInterrupted { .. } => ObservationRouting::SignalInterrupt,
            LiveAdapterObservation::TurnCompleted { .. } => ObservationRouting::AppendTranscript,
            LiveAdapterObservation::StatusChanged { status } => {
                ObservationRouting::UpdateStatus(status.clone())
            }
            LiveAdapterObservation::Error { .. } => ObservationRouting::TerminalError,
            // R5-9: scoped per-command rejection — channel survives, WS
            // pump forwards JSON and continues. Distinct routing so the
            // typed taxonomy at the wire boundary
            // (`Error` → terminal, `CommandRejected` → scoped) maps
            // 1:1 onto host outcomes.
            LiveAdapterObservation::CommandRejected { .. } => ObservationRouting::CommandRejection,
            _ => ObservationRouting::Noop,
        }
    }

    pub async fn apply_status_update(
        &self,
        channel_id: &LiveChannelId,
        status: LiveAdapterStatus,
    ) -> Result<(), LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.status = status;
        Ok(())
    }

    pub async fn next_snapshot_version(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<u64, LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.snapshot_version += 1;
        Ok(channel.snapshot_version)
    }

    pub async fn active_channels(&self) -> Vec<LiveChannelId> {
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);
        // G7 (P2): exclude channels in their post-close TTL retention
        // window. Retained-closed entries remain in `channels` so the
        // reap path (G1) and post-close status reads (G42) keep working,
        // but they no longer accept input — callers like
        // `propagate_config_to_live_channels` and UI listings must not
        // see them as active. `retire_at == Some(_)` is the canonical
        // discriminator (already gated on in `open_channel` rebind and
        // `attach_adapter`).
        inner
            .channels
            .iter()
            .filter(|(_, ch)| ch.retire_at.is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Reap channels whose post-close TTL has elapsed.
    fn reap_retired_locked(inner: &mut HostInner) {
        let now = std::time::Instant::now();
        let to_drop: Vec<LiveChannelId> = inner
            .channels
            .iter()
            .filter_map(|(id, ch)| match ch.retire_at {
                Some(deadline) if deadline <= now => Some(id.clone()),
                _ => None,
            })
            .collect();
        for id in to_drop {
            if let Some(ch) = inner.channels.shift_remove(&id) {
                // G1 (P1): only clear the reverse mapping if it still
                // points at the channel being reaped. After close + rebind,
                // `by_session[S]` already names the new channel, and an
                // unconditional remove would strand the rebound channel
                // (subsequent opens for S would then create duplicates).
                if inner
                    .by_session
                    .get(&ch.session_id)
                    .is_some_and(|current| current == &id)
                {
                    inner.by_session.remove(&ch.session_id);
                }
            }
        }
    }
}

/// Helper: project a typed [`ToolResult`] from the agent dispatcher into
/// the seam-owned [`LiveToolResult`]. The two carry the same structured
/// content shape (`Vec<ContentBlock>`) so tool-result fidelity (text, image,
/// video) is preserved across the seam (closes E30 at this layer).
fn tool_result_from_dispatch(call_id: String, result: ToolResult) -> LiveToolResult {
    LiveToolResult {
        call_id,
        content: result.content,
        is_error: result.is_error,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::live_adapter::{
        LiveAdapterError, LiveAdapterErrorCode, LiveAdapterObservation, LiveDegradationReason,
    };
    use meerkat_core::ops::ToolDispatchOutcome;
    use meerkat_core::types::{StopReason, ToolDef, Usage};
    use meerkat_core::{DispatcherCapabilities, ToolCatalogCapabilities, ToolCatalogEntry};
    use std::sync::Mutex as StdMutex;

    fn test_session_id() -> SessionId {
        SessionId::new()
    }

    // -- Tool timeout accessor (G6 regression) --

    /// G6 (P2): production rkat-rpc startup MUST install
    /// [`DEFAULT_LIVE_TOOL_TIMEOUT`] on the live host. The binary's
    /// build pattern is not directly testable, so the host exposes
    /// [`LiveAdapterHost::tool_timeout`] as a stable accessor; this test
    /// pins both the default-`None` behavior and the builder wiring so
    /// the production callsite (meerkat-rpc/src/main.rs) can be
    /// asserted to produce a host with the timeout populated.
    #[test]
    fn tool_timeout_defaults_to_none_and_builder_sets_it() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        assert_eq!(host.tool_timeout(), None);

        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink))
            .with_tool_timeout(DEFAULT_LIVE_TOOL_TIMEOUT);
        assert_eq!(host.tool_timeout(), Some(DEFAULT_LIVE_TOOL_TIMEOUT));
        // Pin the canonical default value so accidental constant drift
        // is caught here rather than at integration time.
        assert_eq!(DEFAULT_LIVE_TOOL_TIMEOUT, Duration::from_secs(30));
    }

    // -- R5-1 (P2 dogma): projection sink mandatory at construction --

    /// R5-1 (P2 dogma): the projection sink is mandatory at host
    /// construction. A successful `ObservationOutcome::TranscriptAppended`
    /// must imply a real semantic owner received the projection — the
    /// previous `Option<Arc<dyn LiveProjectionSink>>` permitted
    /// "successful append with nobody to project to," which violates the
    /// dogma that success outcomes are truthful completion.
    ///
    /// This test pins:
    ///   1. A host built with the production-shape `RecordingProjectionSink`
    ///      routes a `UserTranscriptFinal` observation to the sink and
    ///      returns `TranscriptAppended` — the sink received exactly one
    ///      append, confirming the success outcome corresponds to a real
    ///      projection.
    ///   2. A host built with the explicit-opt-out `NoOpProjectionSink`
    ///      still classifies the same observation and returns
    ///      `TranscriptAppended` (the no-op sink swallows the append by
    ///      design). The intent of "dropping projections on the floor" is
    ///      now visible in the call site (`Arc::new(NoOpProjectionSink)`)
    ///      rather than hidden in `projection_sink: None`.
    #[tokio::test]
    async fn projection_sink_is_mandatory_at_construction() {
        // Production shape: real sink routes the observation.
        let recording = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&recording) as _);
        let session = test_session_id();
        let ch = host.open_channel(session.clone()).await.unwrap();
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item-1".into()),
            previous_item_id: None,
            content_index: Some(0),
            text: "hello".into(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(matches!(outcome, ObservationOutcome::TranscriptAppended));
        assert_eq!(
            recording.user_transcripts.lock().unwrap().len(),
            1,
            "production-shape host must route user transcripts to the sink"
        );

        // Test opt-out shape: NoOpProjectionSink is the *explicit* way to
        // request "drop projections on the floor". The success outcome is
        // still TranscriptAppended (the routing happened; the sink chose
        // to swallow it), but the lack of a semantic owner is now visible
        // at the call site rather than hidden in `Option<_>`.
        let noop_host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session2 = test_session_id();
        let ch2 = noop_host.open_channel(session2).await.unwrap();
        let outcome2 = noop_host.apply_observation(&ch2, &obs).await.unwrap();
        assert!(matches!(outcome2, ObservationOutcome::TranscriptAppended));
    }

    // -- Channel lifecycle --

    #[tokio::test]
    async fn open_channel_returns_unique_ids() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let s1 = test_session_id();
        let s2 = test_session_id();
        let ch1 = host.open_channel(s1).await.unwrap();
        let ch2 = host.open_channel(s2).await.unwrap();
        assert_ne!(ch1, ch2);
    }

    #[tokio::test]
    async fn open_channel_ids_are_uuid_shape_not_live_n() {
        // G41 regression: the legacy `live_{N}` shape was process-monotonic
        // and collided across `rkat-rpc` restarts and across co-tenant host
        // instances. Channel ids must now be v4 UUIDs.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch1 = host.open_channel(test_session_id()).await.unwrap();
        let ch2 = host.open_channel(test_session_id()).await.unwrap();

        for ch in [&ch1, &ch2] {
            let s = ch.as_str();
            assert!(
                !s.starts_with("live_"),
                "channel id retained legacy `live_N` shape: {s}"
            );
            // Parse strictly as a v4 UUID — `random_uuid` is the only
            // documented constructor; anything else is a regression.
            let parsed =
                uuid::Uuid::parse_str(s).expect("channel id should be a valid UUID string");
            assert_eq!(
                parsed.get_version(),
                Some(uuid::Version::Random),
                "channel id should be a v4 UUID"
            );
        }
        assert_ne!(ch1, ch2);
    }

    #[tokio::test]
    async fn open_channel_starts_in_opening_status() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Opening);
    }

    #[tokio::test]
    async fn duplicate_session_binding_rejected() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();
        let _ch = host.open_channel(session_id.clone()).await.unwrap();
        let err = host.open_channel(session_id.clone()).await.unwrap_err();
        assert!(matches!(err, LiveAdapterHostError::SessionAlreadyBound(id) if id == session_id));
    }

    #[tokio::test]
    async fn close_channel_marks_closed_and_retains_for_status_reads() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.close_channel(&ch).await.unwrap();
        // G42: post-close status is `Closed`, not `ChannelNotFound`.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Closed);
    }

    #[tokio::test]
    async fn close_channel_allows_rebinding_same_session() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        host.close_channel(&ch).await.unwrap();
        let ch2 = host.open_channel(session_id).await.unwrap();
        assert_ne!(ch, ch2);
    }

    /// G1 (P1) regression: after close+rebind, the reap of the retired
    /// channel must NOT clear `by_session[S]` — that mapping now points at
    /// the rebound channel B. Previously the reap unconditionally executed
    /// `by_session.remove(S)`, stranding B and allowing a third open to
    /// create a duplicate active channel for the same session.
    #[tokio::test]
    async fn reap_of_retired_channel_preserves_rebound_session_mapping() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();

        let ch_a = host.open_channel(session_id.clone()).await.unwrap();
        host.close_channel(&ch_a).await.unwrap();
        let ch_b = host.open_channel(session_id.clone()).await.unwrap();
        assert_ne!(ch_a, ch_b);

        // Force-expire A's retire window so the reaper drops A on the next
        // sweep. B remains active (no `retire_at` set).
        {
            let mut inner = host.inner.lock().await;
            if let Some(channel) = inner.channels.get_mut(&ch_a) {
                channel.retire_at =
                    Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
            }
        }

        // `active_channels` invokes the reaper. Post-reap: B is still
        // active and `by_session[S]` still names B.
        let active = host.active_channels().await;
        assert_eq!(active, vec![ch_b.clone()]);
        {
            let inner = host.inner.lock().await;
            assert_eq!(
                inner.by_session.get(&session_id),
                Some(&ch_b),
                "reap of retired A must not clear B's reverse mapping"
            );
            assert!(
                !inner.channels.contains_key(&ch_a),
                "retired channel A must be dropped"
            );
            assert!(
                inner.channels.contains_key(&ch_b),
                "rebound channel B must remain"
            );
        }

        // A subsequent open for the same session must be rejected (B is
        // still bound) — the pre-fix bug allowed it to succeed and create
        // a duplicate active channel.
        let err = host.open_channel(session_id.clone()).await.unwrap_err();
        assert!(
            matches!(err, LiveAdapterHostError::SessionAlreadyBound(id) if id == session_id),
            "after reap, third open for session must still see B as bound"
        );
        assert_eq!(host.active_channels().await.len(), 1);
    }

    /// G7 (P2) regression: a channel inside its post-close TTL retention
    /// window must not appear in `active_channels()`. Closed-but-retained
    /// entries stay in the underlying map (so the reaper and post-close
    /// status reads keep working) but the public `active_channels()`
    /// accessor must not advertise them — callers like
    /// `propagate_config_to_live_channels` would otherwise fan config
    /// swaps out at retired channels. Post-reap, the channel is gone
    /// from the map entirely, so it must still not appear.
    #[tokio::test]
    async fn active_channels_excludes_retained_closed_channels() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let s1 = test_session_id();
        let s2 = test_session_id();

        let live = host.open_channel(s1).await.unwrap();
        let closing = host.open_channel(s2).await.unwrap();

        // Both freshly opened — both should be active.
        let active_pre = host.active_channels().await;
        assert!(active_pre.contains(&live));
        assert!(active_pre.contains(&closing));
        assert_eq!(active_pre.len(), 2);

        // Close one. It enters the TTL retention window
        // (`retire_at = Some(now + CLOSED_CHANNEL_TTL)`); the reaper
        // does NOT drop it yet.
        host.close_channel(&closing).await.unwrap();

        // G7: the retained-closed channel must NOT appear in
        // `active_channels()`, even though it's still in the underlying
        // map. The live channel must still appear.
        let active_during_ttl = host.active_channels().await;
        assert_eq!(
            active_during_ttl,
            vec![live.clone()],
            "retained-closed channel must not appear in active_channels()"
        );
        // Post-close status reads still succeed (channel is in the map).
        assert_eq!(
            host.channel_status(&closing).await.unwrap(),
            LiveAdapterStatus::Closed,
        );

        // Force-expire the TTL and drive the reap; the closed channel
        // is dropped from the map. Still must not appear in
        // `active_channels()`.
        {
            let mut inner = host.inner.lock().await;
            if let Some(channel) = inner.channels.get_mut(&closing) {
                channel.retire_at =
                    Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
            }
        }
        let active_post_reap = host.active_channels().await;
        assert_eq!(active_post_reap, vec![live.clone()]);
        {
            let inner = host.inner.lock().await;
            assert!(
                !inner.channels.contains_key(&closing),
                "post-reap, retired channel must be dropped from the map"
            );
        }
    }

    #[tokio::test]
    async fn channel_session_returns_bound_session() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        assert_eq!(host.channel_session(&ch).await.unwrap(), session_id);
    }

    /// CC1: `signal_terminal_error` must enqueue a synthetic `Error`
    /// observation and close the underlying adapter. The pending obs must
    /// be readable through `next_observation_raw` even after the channel
    /// has transitioned to `Closed` (the slot lives on `ChannelState`, not
    /// on the adapter), so the WS pump can forward it to the client and
    /// derive a typed `terminal:config_rejected` close-frame reason.
    #[tokio::test]
    async fn signal_terminal_error_enqueues_synthetic_error_obs_and_closes_channel() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();

        let code = LiveAdapterErrorCode::ConfigRejected {
            reason: meerkat_core::live_adapter::LiveConfigRejectionReason::RefreshModelSwap {
                from_model: "gpt-realtime".to_string(),
                to_model: "gpt-realtime-1.5".to_string(),
            },
        };
        host.signal_terminal_error(&ch, code).await.unwrap();

        // Channel transitions to Closed as part of the seam.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Closed);

        // The pending synthetic obs is still readable post-close — the
        // pending-slot check in `next_observation_raw` happens BEFORE the
        // `adapter_for` retire-at gate.
        let obs = host
            .next_observation_raw(&ch)
            .await
            .expect("next_observation_raw should return synthetic obs even post-close")
            .expect("synthetic obs must be Some");
        match obs {
            LiveAdapterObservation::Error { code, message } => match code {
                LiveAdapterErrorCode::ConfigRejected { reason } => {
                    assert!(matches!(
                        reason,
                        meerkat_core::live_adapter::LiveConfigRejectionReason::RefreshModelSwap {
                            ref to_model,
                            ..
                        } if to_model == "gpt-realtime-1.5"
                    ));
                    // R5-2: synthetic Error.message mirrors the reason's
                    // Display projection — the human-readable swap text
                    // still appears in the rendered message.
                    assert!(message.contains("close + reopen"));
                }
                other => panic!("expected ConfigRejected, got {other:?}"),
            },
            other => panic!("expected Error observation, got {other:?}"),
        }
    }

    /// CC1: applying the synthetic `Error` observation through
    /// `apply_observation` must produce `ObservationOutcome::Terminal` with
    /// the same `ConfigRejected` code, so the WS pump's `terminal:<slug>`
    /// close-frame derivation in `transport.rs` picks up the typed reason
    /// (slug `config_rejected`).
    #[tokio::test]
    async fn synthetic_terminal_error_routes_through_apply_observation_to_terminal_outcome() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();

        let code = LiveAdapterErrorCode::ConfigRejected {
            reason: meerkat_core::live_adapter::LiveConfigRejectionReason::ChannelIdentitySwap {
                from_model: "a".to_string(),
                from_provider: meerkat_core::Provider::OpenAI,
                to_model: "b".to_string(),
                to_provider: meerkat_core::Provider::OpenAI,
            },
        };
        host.signal_terminal_error(&ch, code).await.unwrap();

        let obs = host.next_observation_raw(&ch).await.unwrap().unwrap();
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        match outcome {
            ObservationOutcome::Terminal { code } => match code {
                LiveAdapterErrorCode::ConfigRejected { reason } => {
                    assert!(matches!(
                        reason,
                        meerkat_core::live_adapter::LiveConfigRejectionReason::ChannelIdentitySwap {
                            ref from_model, ref to_model, ..
                        } if from_model == "a" && to_model == "b"
                    ));
                }
                other => panic!("expected ConfigRejected, got {other:?}"),
            },
            other => panic!("expected Terminal outcome, got {other:?}"),
        }
    }

    /// R5-3: `signal_terminal_error` must deliver the synthetic
    /// `LiveAdapterObservation::Error` to the consumer *before* the
    /// channel-closed end-of-stream signal. Verified end-to-end via
    /// the `pending_synthetic_obs` fallback: the consumer reads the
    /// typed Error, then a subsequent read returns `None` (the
    /// channel-closed signal). The legacy race — where the in-flight
    /// `next_observation()` returned `None` first and the typed
    /// signal was lost — must not recur.
    #[tokio::test]
    async fn signal_terminal_error_delivers_synthetic_error_before_close_signal() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();

        let code = LiveAdapterErrorCode::ConfigRejected {
            reason: meerkat_core::live_adapter::LiveConfigRejectionReason::Other {
                detail: "model_swap_test".to_string(),
            },
        };
        host.signal_terminal_error(&ch, code).await.unwrap();

        // First read: synthetic Error (typed, with reason).
        let first = host
            .next_observation_raw(&ch)
            .await
            .expect("first read should succeed")
            .expect("synthetic Error must surface before end-of-stream");
        match first {
            LiveAdapterObservation::Error { code, message } => match code {
                LiveAdapterErrorCode::ConfigRejected { reason } => {
                    // R5-2: `Other.detail` is the diagnostic catch-all and
                    // its `Display` projection equals the detail text, so
                    // the synthetic `Error.message` matches verbatim.
                    assert!(matches!(
                        &reason,
                        meerkat_core::live_adapter::LiveConfigRejectionReason::Other { detail }
                            if detail == "model_swap_test"
                    ));
                    assert_eq!(message, "model_swap_test");
                }
                other => unreachable!("expected ConfigRejected, got {other:?}"),
            },
            other => unreachable!("expected Error obs first, got {other:?}"),
        }
    }

    /// CC1: `signal_terminal_error` on an unknown channel must surface
    /// `ChannelNotFound`, mirroring the rest of the host's per-channel
    /// surface. Without this, the runtime swap path could silently drop
    /// the typed signal on a channel that was already reaped.
    #[tokio::test]
    async fn signal_terminal_error_on_missing_channel_returns_channel_not_found() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let bogus = LiveChannelId::random_uuid();
        let result = host
            .signal_terminal_error(
                &bogus,
                LiveAdapterErrorCode::ConfigRejected {
                    reason: meerkat_core::live_adapter::LiveConfigRejectionReason::Other {
                        detail: "no channel".into(),
                    },
                },
            )
            .await;
        assert!(
            matches!(&result, Err(LiveAdapterHostError::ChannelNotFound(id)) if id == &bogus),
            "expected ChannelNotFound for unknown channel, got {result:?}"
        );
    }

    // -- Observation classification --

    #[test]
    fn ready_observation_routes_to_status_update() {
        let routing = LiveAdapterHost::classify_observation(&LiveAdapterObservation::Ready);
        assert_eq!(
            routing,
            ObservationRouting::UpdateStatus(LiveAdapterStatus::Ready)
        );
    }

    #[test]
    fn tool_call_observation_routes_to_dispatch() {
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_1".into(),
            tool_name: "calculator".into(),
            arguments: serde_json::json!({"x": 1}),
        };
        let routing = LiveAdapterHost::classify_observation(&obs);
        assert_eq!(
            routing,
            ObservationRouting::DispatchToolCall {
                provider_call_id: "call_1".into(),
                tool_name: "calculator".into(),
            }
        );
    }

    #[test]
    fn barge_in_observation_routes_to_interrupt() {
        let routing =
            LiveAdapterHost::classify_observation(&LiveAdapterObservation::TurnInterrupted {
                response_id: None,
            });
        assert_eq!(routing, ObservationRouting::SignalInterrupt);
        let routing_with_id =
            LiveAdapterHost::classify_observation(&LiveAdapterObservation::TurnInterrupted {
                response_id: Some("resp_42".into()),
            });
        assert_eq!(routing_with_id, ObservationRouting::SignalInterrupt);
    }

    #[test]
    fn user_transcript_routes_to_append() {
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item_1".into()),
            previous_item_id: None,
            content_index: None,
            text: "hello".into(),
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::AppendTranscript
        );
    }

    #[test]
    fn assistant_text_delta_routes_to_append() {
        let obs = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: Some("item_2".into()),
            previous_item_id: None,
            content_index: None,
            response_id: None,
            delta_id: None,
            delta: "world".into(),
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::AppendTranscript
        );
    }

    #[test]
    fn turn_completed_routes_to_append() {
        let obs = LiveAdapterObservation::TurnCompleted {
            response_id: None,
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::AppendTranscript
        );
    }

    #[test]
    fn error_observation_routes_to_terminal() {
        let obs = LiveAdapterObservation::Error {
            code: LiveAdapterErrorCode::ConnectionLost,
            message: "ws closed".into(),
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::TerminalError
        );
    }

    #[test]
    fn audio_chunk_routes_to_noop() {
        let obs = LiveAdapterObservation::AssistantAudioChunk {
            data: vec![0; 100],
            sample_rate_hz: 24000,
            channels: 1,
            response_id: None,
            item_id: None,
            content_index: None,
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::Noop
        );
    }

    #[test]
    fn status_changed_routes_to_status_update() {
        let obs = LiveAdapterObservation::StatusChanged {
            status: LiveAdapterStatus::Degraded {
                reason: LiveDegradationReason::ProviderThrottled,
            },
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::UpdateStatus(LiveAdapterStatus::Degraded {
                reason: LiveDegradationReason::ProviderThrottled,
            })
        );
    }

    // -- Status tracking --

    #[tokio::test]
    async fn apply_status_update_changes_channel_status() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Ready
        );
    }

    // -- Snapshot versioning --

    #[tokio::test]
    async fn snapshot_version_increments_monotonically() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let v1 = host.next_snapshot_version(&ch).await.unwrap();
        let v2 = host.next_snapshot_version(&ch).await.unwrap();
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
    }

    // -- Active channels --

    #[tokio::test]
    async fn active_channels_lists_open_channels() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch1 = host.open_channel(test_session_id()).await.unwrap();
        let ch2 = host.open_channel(test_session_id()).await.unwrap();
        let active = host.active_channels().await;
        assert_eq!(active.len(), 2);
        assert!(active.contains(&ch1));
        assert!(active.contains(&ch2));
    }

    // -- Adapter attachment --

    #[tokio::test]
    async fn send_input_without_adapter_returns_error() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let err = host
            .send_input(&ch, LiveInputChunk::Text { text: "hi".into() })
            .await
            .unwrap_err();
        // F31: with no adapter, status is still `Opening`; rejection is the
        // not-ready guard. (If status were `Ready`, we'd hit `NoAdapter`.)
        assert!(matches!(err, LiveAdapterHostError::ChannelNotReady(_, _)));
    }

    #[tokio::test]
    async fn attach_adapter_does_not_assert_ready() {
        // F32: attach leaves status Opening; only `Ready` observation
        // promotes the channel.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Opening
        );
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Opening,
            "attach_adapter must NOT mark channel Ready (F32)"
        );
    }

    // -- F31: send_input gate --

    #[tokio::test]
    async fn send_input_rejected_when_not_ready() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        // Status is still `Opening` — `accepts_commands()` is false.
        let err = host
            .send_input(&ch, LiveInputChunk::Text { text: "hi".into() })
            .await
            .unwrap_err();
        match err {
            LiveAdapterHostError::ChannelNotReady(_, status) => {
                assert_eq!(status, LiveAdapterStatus::Opening);
            }
            other => panic!("expected ChannelNotReady, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_input_accepts_when_ready() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        host.send_input(&ch, LiveInputChunk::Text { text: "hi".into() })
            .await
            .unwrap();
    }

    // -- E29: typed adapter error --

    #[tokio::test]
    async fn adapter_pump_error_terminalizes_channel_status() {
        // R5-8 (was F34): when the pump errors, the host fully retires
        // the channel — sets status=Closed, schedules retire_at, drops
        // the adapter Arc — and surfaces a synthetic typed
        // `LiveAdapterObservation::Error` to the consumer instead of an
        // `AdapterError` propagation. This lets the WS pump close with
        // a typed reason and reuses R5-3's synthetic-error seam.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(ErroringAdapter))
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        let obs = host
            .next_observation_raw(&ch)
            .await
            .unwrap()
            .expect("synthetic Error obs surfaces on adapter Err");
        match obs {
            LiveAdapterObservation::Error { code, message } => {
                assert_eq!(code, LiveAdapterErrorCode::ProviderError);
                assert!(
                    message.contains("adapter read failure"),
                    "synthetic message must explain origin; got `{message}`"
                );
            }
            other => unreachable!("expected synthetic Error, got {other:?}"),
        }
        // Channel status now reflects the terminal pump failure AND
        // retire_at is set so a future open_channel for the same
        // session can succeed once the TTL elapses.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Closed);
        {
            let inner = host.inner.lock().await;
            let channel = inner
                .channels
                .get(&ch)
                .expect("channel preserved for live/status until TTL elapses");
            assert!(
                channel.retire_at.is_some(),
                "R5-8: adapter Err must set retire_at"
            );
            assert!(
                channel.adapter.is_none(),
                "R5-8: adapter Err must drop the adapter Arc"
            );
        }
    }

    /// R5-9: a `LiveAdapterObservation::CommandRejected` flowing
    /// through `apply_observation` produces a typed
    /// `ObservationOutcome::CommandRejected` — NOT
    /// `ObservationOutcome::Terminal`. The channel state must remain
    /// untouched (status, retire_at, adapter all preserved) so a
    /// follow-up command can be sent on the same channel.
    #[tokio::test]
    async fn command_rejected_routes_non_terminally_and_preserves_channel() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        let obs = LiveAdapterObservation::CommandRejected {
            code: LiveAdapterErrorCode::ConfigRejected {
                reason:
                    meerkat_core::live_adapter::LiveConfigRejectionReason::ImageInputNotImplemented,
            },
            message: "image_input_not_implemented".into(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        match outcome {
            ObservationOutcome::CommandRejected { code, message } => {
                assert!(matches!(
                    code,
                    LiveAdapterErrorCode::ConfigRejected {
                        reason: meerkat_core::live_adapter::LiveConfigRejectionReason::ImageInputNotImplemented,
                    }
                ));
                assert_eq!(message, "image_input_not_implemented");
            }
            other => {
                unreachable!("CommandRejected must produce CommandRejected outcome, got {other:?}")
            }
        }

        // Channel remains live — status untouched, retire_at not set,
        // adapter still attached.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Ready);
        {
            let inner = host.inner.lock().await;
            let channel = inner.channels.get(&ch).expect("channel present");
            assert!(
                channel.retire_at.is_none(),
                "CommandRejected must not retire the channel"
            );
            assert!(
                channel.adapter.is_some(),
                "CommandRejected must not drop the adapter"
            );
        }
    }

    /// R5-8: after the adapter pump errors, the channel is fully
    /// retired and `open_channel` for the same session id succeeds
    /// after the previous binding's retire reaper sweeps. Without
    /// `retire_at` being set on adapter Err, the legacy code path
    /// stranded the session — `open_channel` rejected the rebind with
    /// `SessionAlreadyBound` indefinitely.
    #[tokio::test]
    async fn adapter_err_releases_session_for_rebind() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();
        let ch1 = host.open_channel(session_id.clone()).await.unwrap();
        host.attach_adapter(&ch1, Arc::new(ErroringAdapter))
            .await
            .unwrap();
        host.apply_status_update(&ch1, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        // Trigger the adapter error (and the host's R5-8 cleanup).
        let _ = host.next_observation_raw(&ch1).await.unwrap();

        // Force-expire the retire window so the reaper accepts the
        // rebind without sleeping for `CLOSED_CHANNEL_TTL` seconds.
        {
            let mut inner = host.inner.lock().await;
            if let Some(channel) = inner.channels.get_mut(&ch1) {
                channel.retire_at =
                    Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
            }
        }

        let ch2 = host
            .open_channel(session_id.clone())
            .await
            .expect("rebind for same session must succeed once previous channel is retired");
        assert_ne!(ch1, ch2);
    }

    // -- A2/A3: transcript projection --

    #[tokio::test]
    async fn user_transcript_observation_appends_to_sink() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item_1".into()),
            previous_item_id: None,
            content_index: None,
            text: "hello world".into(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(matches!(outcome, ObservationOutcome::TranscriptAppended));
        let user = sink.user_transcripts.lock().unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].0, session_id);
        assert_eq!(user[0].1, "hello world");
        assert_eq!(user[0].2.provider_item_id.as_deref(), Some("item_1"));
    }

    #[tokio::test]
    async fn user_transcript_full_identity_propagates_end_to_end() {
        // A11 contract: every identity field carried by
        // `LiveAdapterObservation::UserTranscriptFinal` must reach the sink.
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item_1".into()),
            previous_item_id: Some("item_0".into()),
            content_index: Some(2),
            text: "hello".into(),
        };
        host.apply_observation(&ch, &obs).await.unwrap();
        let user = sink.user_transcripts.lock().unwrap();
        let identity = &user[0].2;
        assert_eq!(identity.provider_item_id.as_deref(), Some("item_1"));
        assert_eq!(identity.previous_item_id.as_deref(), Some("item_0"));
        assert_eq!(identity.content_index, Some(2));
        // Response/delta IDs are assistant-only; ensure they do not leak
        // through on the user side.
        assert_eq!(identity.response_id, None);
        assert_eq!(identity.delta_id, None);
    }

    #[tokio::test]
    async fn assistant_text_delta_observation_appends_to_sink() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let obs = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: None,
            previous_item_id: None,
            content_index: None,
            response_id: None,
            delta_id: None,
            delta: "Hello".into(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(matches!(outcome, ObservationOutcome::TranscriptAppended));
        // T6: AssistantTextDelta routes to the text lane.
        let deltas = sink.text_deltas.lock().unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].0, session_id);
        assert_eq!(deltas[0].1, "Hello");
        // The transcript lane stays empty for a display-text observation.
        assert!(sink.transcript_deltas.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn assistant_text_delta_full_identity_propagates_end_to_end() {
        // A11 contract: every identity field on `AssistantTextDelta` —
        // including `response_id` and `delta_id` — must reach the sink.
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let obs = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: Some("item_42".into()),
            previous_item_id: Some("item_41".into()),
            content_index: Some(1),
            response_id: Some("resp_xyz".into()),
            delta_id: Some("d_7".into()),
            delta: "world".into(),
        };
        host.apply_observation(&ch, &obs).await.unwrap();
        let deltas = sink.text_deltas.lock().unwrap();
        let identity = &deltas[0].2;
        assert_eq!(identity.provider_item_id.as_deref(), Some("item_42"));
        assert_eq!(identity.previous_item_id.as_deref(), Some("item_41"));
        assert_eq!(identity.content_index, Some(1));
        assert_eq!(identity.response_id.as_deref(), Some("resp_xyz"));
        assert_eq!(identity.delta_id.as_deref(), Some("d_7"));
    }

    #[tokio::test]
    async fn assistant_transcript_final_observation_calls_sink() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let obs = LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: "resp_1".into(),
            previous_item_id: None,
            content_index: None,
            response_id: None,
            text: "All done.".into(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        host.apply_observation(&ch, &obs).await.unwrap();
        // T6: AssistantTranscriptFinal routes to the transcript lane.
        let finals = sink.transcript_finals.lock().unwrap();
        assert_eq!(finals.len(), 1);
        assert_eq!(finals[0].0, session_id);
        assert_eq!(finals[0].1, "All done.");
        assert!(sink.text_finals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn assistant_transcript_final_full_identity_propagates_end_to_end() {
        // A11: AssistantTranscriptFinal carries provider_item_id (required),
        // previous_item_id, content_index, response_id. delta_id is not
        // applicable to a final.
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let obs = LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: "item_final".into(),
            previous_item_id: Some("item_prev".into()),
            content_index: Some(0),
            response_id: Some("resp_final".into()),
            text: "done".into(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        host.apply_observation(&ch, &obs).await.unwrap();
        let finals = sink.transcript_finals.lock().unwrap();
        let identity = &finals[0].2;
        assert_eq!(identity.provider_item_id.as_deref(), Some("item_final"));
        assert_eq!(identity.previous_item_id.as_deref(), Some("item_prev"));
        assert_eq!(identity.content_index, Some(0));
        assert_eq!(identity.response_id.as_deref(), Some("resp_final"));
        assert_eq!(identity.delta_id, None);
    }

    #[tokio::test]
    async fn turn_completed_observation_signals_sink() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let obs = LiveAdapterObservation::TurnCompleted {
            response_id: None,
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        host.apply_observation(&ch, &obs).await.unwrap();
        let turns = sink.turn_completed.lock().unwrap();
        assert_eq!(turns.len(), 1);
    }

    // -- T6: assistant_transcript_delta routes to transcript lane only --

    #[tokio::test]
    async fn assistant_transcript_delta_routes_to_transcript_lane() {
        // T6: AssistantTranscriptDelta must reach the transcript-lane sink
        // method, NOT the text-lane method. The host classification +
        // apply_observation pair owns the dispatch; this test pins the
        // routing so a future refactor cannot collapse the lanes back.
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let obs = LiveAdapterObservation::AssistantTranscriptDelta {
            provider_item_id: Some("item_t".into()),
            previous_item_id: None,
            content_index: Some(0),
            response_id: Some("resp_t".into()),
            delta_id: Some("d_t".into()),
            delta: "spoken word".into(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(matches!(outcome, ObservationOutcome::TranscriptAppended));
        let transcript_deltas = sink.transcript_deltas.lock().unwrap();
        assert_eq!(transcript_deltas.len(), 1);
        assert_eq!(transcript_deltas[0].0, session_id);
        assert_eq!(transcript_deltas[0].1, "spoken word");
        // The text lane MUST stay empty for a transcript observation.
        assert!(
            sink.text_deltas.lock().unwrap().is_empty(),
            "AssistantTranscriptDelta must not reach the text-lane sink (T6)"
        );
    }

    // -- P1#2: structured realtime transcript pass-through --
    //
    // Regression: provider adapters emit
    // `LiveAdapterObservation::RealtimeTranscript { event }` for events the
    // host previously fell through to `Noop` on (`ItemObserved`,
    // `ItemSkipped`, `AssistantTurnCompleted`, `AssistantTurnInterrupted`).
    // The host must route those through the typed sink seam so the session
    // runtime's idempotent ordering / staging machinery owns materialization
    // — the same path that already handles streaming assistant deltas.

    #[tokio::test]
    async fn realtime_transcript_observation_routes_to_append_realtime_transcript() {
        use meerkat_core::RealtimeTranscriptRole;
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();

        let event = RealtimeTranscriptEvent::ItemObserved {
            item_id: "item_realtime_1".into(),
            previous_item_id: Some("item_realtime_0".into()),
            role: RealtimeTranscriptRole::Assistant,
            response_id: Some("resp_realtime_1".into()),
        };
        let obs = LiveAdapterObservation::RealtimeTranscript {
            event: event.clone(),
        };

        // Routing first: must NOT be Noop — it must be the structured pass-
        // through variant with the event preserved.
        let routing = LiveAdapterHost::classify_observation(&obs);
        match routing {
            ObservationRouting::AppendRealtimeTranscript { event: routed } => {
                assert_eq!(routed, event);
            }
            other => panic!("expected AppendRealtimeTranscript, got {other:?}"),
        }

        // End-to-end: apply_observation routes through the sink exactly once.
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(
            matches!(outcome, ObservationOutcome::TranscriptAppended),
            "expected TranscriptAppended, got {outcome:?}"
        );

        let recorded = sink.realtime_events.lock().unwrap();
        assert_eq!(recorded.len(), 1, "sink must see exactly one append");
        assert_eq!(recorded[0].0, session_id);
        assert_eq!(recorded[0].1, event);

        // No legacy fallthrough: `Noop` would mean nothing else in the sink
        // moved either. Pin that by checking adjacent recorders are empty.
        assert!(sink.text_deltas.lock().unwrap().is_empty());
        assert!(sink.transcript_deltas.lock().unwrap().is_empty());
        assert!(sink.text_finals.lock().unwrap().is_empty());
        assert!(sink.transcript_finals.lock().unwrap().is_empty());
        assert!(sink.user_transcripts.lock().unwrap().is_empty());
        assert!(sink.turn_completed.lock().unwrap().is_empty());
        assert!(sink.interrupts.lock().unwrap().is_empty());
    }

    // R6-6 (P3 dogma): `LiveProjectionSink::append_realtime_transcript` is a
    // *required* trait method — there is no default body. This test pins the
    // path through `NoOpProjectionSink` (the explicit-opt-out sink) so that
    // a future refactor cannot reintroduce a silent default that drops
    // realtime events on the floor for "mandatory" sinks that forgot to
    // override the method.
    #[tokio::test]
    async fn noop_projection_sink_explicitly_accepts_realtime_transcript() {
        use meerkat_core::RealtimeTranscriptRole;
        let sink: Arc<dyn LiveProjectionSink> = Arc::new(NoOpProjectionSink);
        let host = LiveAdapterHost::new(Arc::clone(&sink));
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();

        let event = RealtimeTranscriptEvent::ItemObserved {
            item_id: "item_noop".into(),
            previous_item_id: None,
            role: RealtimeTranscriptRole::Assistant,
            response_id: Some("resp_noop".into()),
        };
        let obs = LiveAdapterObservation::RealtimeTranscript {
            event: event.clone(),
        };

        // The host-side route must treat NoOpProjectionSink as a real owner
        // (TranscriptAppended), not a silent fallthrough — that is exactly
        // what the explicit `Ok(())` body on the impl guarantees once the
        // trait default is removed.
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(
            matches!(outcome, ObservationOutcome::TranscriptAppended),
            "NoOpProjectionSink must accept RealtimeTranscript explicitly, got {outcome:?}"
        );

        // And the direct trait call also has to compile + succeed — proving
        // the impl exists at the impl site, not via a trait default.
        sink.append_realtime_transcript(&session_id, &event)
            .await
            .expect("NoOpProjectionSink::append_realtime_transcript must be explicit Ok");
    }

    #[tokio::test]
    async fn realtime_transcript_assistant_turn_completed_routes_through_sink() {
        // Pin that AssistantTurnCompleted — historically lost by the fall-
        // through to `Noop` — also reaches the sink. (Different inner variant
        // shape: stop_reason + usage, not item identity.)
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let event = RealtimeTranscriptEvent::AssistantTurnCompleted {
            response_id: "resp_complete".into(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let obs = LiveAdapterObservation::RealtimeTranscript {
            event: event.clone(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert!(matches!(outcome, ObservationOutcome::TranscriptAppended));
        let recorded = sink.realtime_events.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1, event);
    }

    // -- A4/A5: tool call dispatch + submit --

    #[tokio::test]
    async fn tool_call_observation_dispatches_through_tool_authority() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _)
            .with_tool_dispatcher(Arc::clone(&dispatcher) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_42".into(),
            tool_name: "calculator".into(),
            arguments: serde_json::json!({"a": 2, "b": 3}),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        match outcome {
            ObservationOutcome::ToolCallDispatched {
                provider_call_id,
                tool_name,
            } => {
                assert_eq!(provider_call_id, "call_42");
                assert_eq!(tool_name, "calculator");
            }
            other => panic!("expected ToolCallDispatched, got {other:?}"),
        }
        // Dispatcher saw the call.
        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_42");
        assert_eq!(calls[0].1, "calculator");
        // Adapter saw the SubmitToolResult command.
        let submitted = adapter.submitted_results.lock().unwrap();
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].call_id, "call_42");
    }

    #[tokio::test]
    async fn tool_call_skipped_when_no_dispatcher_wired() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_99".into(),
            tool_name: "calculator".into(),
            arguments: serde_json::json!({}),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        match outcome {
            ObservationOutcome::ToolCallSkipped {
                reason: ToolDispatchSkipReason::NoDispatcher,
                ..
            } => {}
            other => panic!("expected ToolCallSkipped/NoDispatcher, got {other:?}"),
        }
    }

    // -- P2#2: missing-dispatcher does not deadlock the live session --
    //
    // Regression: when no tool dispatcher is wired and a `ToolCallRequested`
    // observation arrives, the host previously returned `ToolCallSkipped {
    // NoDispatcher }` and silently dropped the call on the floor. The
    // provider then waited forever for a tool result that would never come,
    // deadlocking the live session until the provider's own timeout (or
    // never). The fix is to ALSO send a typed `SubmitToolError` back to the
    // adapter so the provider can complete the response with an error and
    // unstick its turn — while still surfacing the typed
    // `ToolCallSkipped/NoDispatcher` outcome for the host audit trail.
    #[tokio::test]
    async fn tool_call_no_dispatcher_submits_error_to_adapter() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let adapter = Arc::new(RecordingAdapter::default());
        // No dispatcher installed.
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        // Status must accept commands so the adapter command path is exercised
        // (otherwise `send_command` rejects with ChannelNotReady — but the
        // helper used for SubmitToolError uses `require_ready=false` since the
        // command is allowed when not Ready; verifying via apply_status_update
        // anyway to mirror the production attach sequence).
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_unwired".into(),
            tool_name: "calculator".into(),
            arguments: serde_json::json!({}),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();

        // 1. The typed audit-trail outcome is preserved.
        match outcome {
            ObservationOutcome::ToolCallSkipped {
                provider_call_id,
                tool_name,
                reason: ToolDispatchSkipReason::NoDispatcher,
            } => {
                assert_eq!(provider_call_id, "call_unwired");
                assert_eq!(tool_name, "calculator");
            }
            other => panic!("expected ToolCallSkipped/NoDispatcher, got {other:?}"),
        }

        // 2. The adapter received a SubmitToolError keyed on the same call_id
        //    so the provider can complete its response and unblock the live
        //    turn. This is the new behavior P2#2 introduces.
        let errors = adapter.submitted_errors.lock().unwrap();
        assert_eq!(
            errors.len(),
            1,
            "adapter must receive exactly one SubmitToolError when dispatcher is missing"
        );
        assert_eq!(errors[0].0, "call_unwired");
        assert!(
            errors[0].1.contains("dispatcher"),
            "error message should mention the missing dispatcher; got {:?}",
            errors[0].1
        );

        // 3. No phantom SubmitToolResult was sent.
        assert!(adapter.submitted_results.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_tool_dispatcher_late_binds_after_construction() {
        // Wave-3 follow-up: rkat-rpc constructs the host before the
        // callback channel that backs the dispatcher exists. Pre-set, a
        // ToolCallRequested must skip; post-`set_tool_dispatcher`, the same
        // observation must dispatch through the newly-installed dispatcher.
        let sink = Arc::new(RecordingProjectionSink::default());
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_pre".into(),
            tool_name: "calc".into(),
            arguments: serde_json::json!({}),
        };
        // Phase 1: no dispatcher → skipped.
        match host.apply_observation(&ch, &obs).await.unwrap() {
            ObservationOutcome::ToolCallSkipped {
                reason: ToolDispatchSkipReason::NoDispatcher,
                ..
            } => {}
            other => panic!("expected pre-set skip, got {other:?}"),
        }
        assert_eq!(dispatcher.calls.lock().unwrap().len(), 0);

        // Phase 2: late install → dispatch flows through.
        host.set_tool_dispatcher(Arc::clone(&dispatcher) as _);
        let obs2 = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_post".into(),
            tool_name: "calc".into(),
            arguments: serde_json::json!({"x": 1}),
        };
        match host.apply_observation(&ch, &obs2).await.unwrap() {
            ObservationOutcome::ToolCallDispatched {
                provider_call_id, ..
            } => {
                assert_eq!(provider_call_id, "call_post");
            }
            other => panic!("expected post-set dispatch, got {other:?}"),
        }
        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_post");
    }

    #[tokio::test]
    async fn set_tool_dispatcher_replaces_previously_installed_dispatcher() {
        // The setter is idempotent on repeated installs and the most recent
        // install wins. (In-flight dispatches that already cloned an Arc to
        // the prior value are unaffected — that's the documented contract.)
        let sink = Arc::new(RecordingProjectionSink::default());
        let first = Arc::new(RecordingDispatcher::default());
        let second = Arc::new(RecordingDispatcher::default());
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _)
            .with_tool_dispatcher(Arc::clone(&first) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        host.set_tool_dispatcher(Arc::clone(&second) as _);
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_swap".into(),
            tool_name: "calc".into(),
            arguments: serde_json::json!({}),
        };
        host.apply_observation(&ch, &obs).await.unwrap();

        assert_eq!(first.calls.lock().unwrap().len(), 0);
        assert_eq!(second.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tool_call_dispatch_error_submits_tool_error_to_adapter() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let dispatcher = Arc::new(FailingDispatcher);
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _)
            .with_tool_dispatcher(Arc::clone(&dispatcher) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_err".into(),
            tool_name: "failing".into(),
            arguments: serde_json::json!({}),
        };
        host.apply_observation(&ch, &obs).await.unwrap();
        let errors = adapter.submitted_errors.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "call_err");
    }

    // -- K61: tool-call dispatch timeout --
    //
    // Original deleted test: `realtime_tool_timeout`. Pins the contract that a
    // dispatcher which holds a tool call past the configured deadline does NOT
    // hang the runtime. The host must:
    //   1. Submit a typed `LiveAdapterCommand::SubmitToolError` to the adapter
    //      so the provider can unblock its turn.
    //   2. Return `ObservationOutcome::ToolCallTimedOut` (NOT
    //      `ToolCallDispatched`) so the projection layer sees the miss.
    //   3. Skip the projection sink — no phantom `ToolDispatched` outcome
    //      surfaces to canonical session state.
    //
    // The deterministic-clock harness (`tokio::time::pause` + `advance`) makes
    // the test exercise the real `tokio::time::timeout` boundary without a
    // wall-clock dependency, closing the original "needs harness" gap.

    /// Tool dispatcher whose `dispatch` future awaits a long sleep before
    /// returning. Under `tokio::time::pause()` the sleep never elapses on its
    /// own; the test drives the clock past the host's `tool_timeout` to force
    /// the timeout branch.
    struct SlowDispatcher {
        sleep_for: Duration,
        calls: StdMutex<u32>,
    }

    impl SlowDispatcher {
        fn new(sleep_for: Duration) -> Self {
            Self {
                sleep_for,
                calls: StdMutex::new(0),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for SlowDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities::default()
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::from([])
        }

        fn pending_catalog_sources(&self) -> Arc<[String]> {
            Arc::from([])
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, meerkat_core::error::ToolError> {
            *self.calls.lock().unwrap() += 1;
            tokio::time::sleep(self.sleep_for).await;
            // Echo result so a non-timed-out dispatch still produces a sane outcome.
            let tool_result =
                meerkat_core::types::ToolResult::new(call.id.to_string(), "ok".into(), false);
            Ok(ToolDispatchOutcome::from(tool_result))
        }

        fn capabilities(&self) -> DispatcherCapabilities {
            DispatcherCapabilities::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn realtime_tool_timeout() {
        // K61: a dispatcher that takes longer than the host's `tool_timeout`
        // must produce a typed `ToolCallTimedOut` outcome and a
        // `SubmitToolError` to the adapter — no phantom dispatch result.
        let timeout = Duration::from_millis(500);
        let dispatcher_sleep = Duration::from_secs(60);
        let sink = Arc::new(RecordingProjectionSink::default());
        let dispatcher = Arc::new(SlowDispatcher::new(dispatcher_sleep));
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _)
            .with_tool_dispatcher(Arc::clone(&dispatcher) as _)
            .with_tool_timeout(timeout);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_slow".into(),
            tool_name: "slow_tool".into(),
            arguments: serde_json::json!({"q": 1}),
        };

        // Race the host's apply_observation against a clock-advance. Both
        // futures are driven by the same tokio runtime under `start_paused`,
        // so advancing the clock past the timeout is what unblocks the
        // host's `tokio::time::timeout`.
        let host_call = async { host.apply_observation(&ch, &obs).await.unwrap() };
        let drive_clock = async {
            // Yield once so apply_observation gets to the timeout boundary
            // before we advance the clock.
            tokio::task::yield_now().await;
            tokio::time::advance(timeout + Duration::from_millis(1)).await;
        };
        let (outcome, _) = tokio::join!(host_call, drive_clock);

        match outcome {
            ObservationOutcome::ToolCallTimedOut {
                provider_call_id,
                tool_name,
                timeout: t,
            } => {
                assert_eq!(provider_call_id, "call_slow");
                assert_eq!(tool_name, "slow_tool");
                assert_eq!(t, timeout);
            }
            other => panic!("expected ToolCallTimedOut, got {other:?}"),
        }

        // Dispatcher was invoked exactly once.
        assert_eq!(*dispatcher.calls.lock().unwrap(), 1);

        // Adapter saw a SubmitToolError (NOT a SubmitToolResult).
        let errors = adapter.submitted_errors.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "call_slow");
        assert!(
            errors[0].1.contains("timeout"),
            "tool error message should mention timeout: {}",
            errors[0].1
        );
        let results = adapter.submitted_results.lock().unwrap();
        assert!(
            results.is_empty(),
            "no SubmitToolResult should reach the adapter on timeout: {results:?}"
        );

        // Projection sink saw no spurious tool-related projection.
        assert_eq!(sink.text_finals.lock().unwrap().len(), 0);
        assert_eq!(sink.transcript_finals.lock().unwrap().len(), 0);
        assert_eq!(sink.terminal_errors.lock().unwrap().len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tool_call_dispatch_succeeds_when_within_deadline() {
        // K61 companion: with a deadline configured, a dispatcher that
        // returns before the deadline must still produce
        // `ToolCallDispatched` and submit the result. Pins that the timeout
        // wrapper does not change the success path.
        let timeout = Duration::from_secs(5);
        let dispatcher_sleep = Duration::from_millis(100);
        let sink = Arc::new(RecordingProjectionSink::default());
        let dispatcher = Arc::new(SlowDispatcher::new(dispatcher_sleep));
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _)
            .with_tool_dispatcher(Arc::clone(&dispatcher) as _)
            .with_tool_timeout(timeout);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_fast".into(),
            tool_name: "fast_tool".into(),
            arguments: serde_json::json!({}),
        };
        let host_call = async { host.apply_observation(&ch, &obs).await.unwrap() };
        let drive_clock = async {
            tokio::task::yield_now().await;
            // Advance past dispatcher_sleep but well before timeout.
            tokio::time::advance(dispatcher_sleep + Duration::from_millis(10)).await;
        };
        let (outcome, _) = tokio::join!(host_call, drive_clock);

        match outcome {
            ObservationOutcome::ToolCallDispatched {
                provider_call_id, ..
            } => assert_eq!(provider_call_id, "call_fast"),
            other => panic!("expected ToolCallDispatched, got {other:?}"),
        }
        let results = adapter.submitted_results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_id, "call_fast");
        assert!(adapter.submitted_errors.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tool_call_dispatch_without_timeout_preserves_unbounded_await() {
        // K61 companion: a host without `with_tool_timeout` must still
        // produce `ToolCallDispatched` for a fast dispatcher (the legacy
        // unbounded-await branch). Smoke-tests the `None` arm of
        // `tool_timeout`.
        let sink = Arc::new(RecordingProjectionSink::default());
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let adapter = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _)
            .with_tool_dispatcher(Arc::clone(&dispatcher) as _);
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.apply_status_update(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_legacy".into(),
            tool_name: "calc".into(),
            arguments: serde_json::json!({}),
        };
        match host.apply_observation(&ch, &obs).await.unwrap() {
            ObservationOutcome::ToolCallDispatched { .. } => {}
            other => panic!("expected ToolCallDispatched on no-timeout host, got {other:?}"),
        }
        assert_eq!(adapter.submitted_results.lock().unwrap().len(), 1);
    }

    // -- A6: barge-in projection --

    #[tokio::test]
    async fn barge_in_observation_calls_signal_interrupt() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let outcome = host
            .apply_observation(
                &ch,
                &LiveAdapterObservation::TurnInterrupted {
                    response_id: Some("resp_42".into()),
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ObservationOutcome::InterruptSignalled));
        let interrupts = sink.interrupts.lock().unwrap();
        assert_eq!(interrupts.len(), 1);
        assert_eq!(interrupts[0].0, session_id);
        // G4 (P1): the in-flight response id is plumbed through to the sink
        // so the truncation can be scoped to the specific response even when
        // the barge-in lands before any transcript delta has been staged.
        assert_eq!(interrupts[0].1.as_deref(), Some("resp_42"));
    }

    // -- A10: terminal error projection --

    #[tokio::test]
    async fn terminal_error_observation_marks_channel_closed_and_signals_sink() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        let obs = LiveAdapterObservation::Error {
            code: LiveAdapterErrorCode::ConnectionLost,
            message: "ws closed unexpectedly".into(),
        };
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        match outcome {
            ObservationOutcome::Terminal {
                code: LiveAdapterErrorCode::ConnectionLost,
            } => {}
            other => panic!("expected Terminal/ConnectionLost, got {other:?}"),
        }
        // Channel reflects terminal status.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Closed);
        // Sink was signalled.
        let terminals = sink.terminal_errors.lock().unwrap();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].0, session_id);
        assert!(matches!(
            terminals[0].1,
            LiveAdapterErrorCode::ConnectionLost
        ));
    }

    // -- N80: O(1) session lookup correctness check --

    #[tokio::test]
    async fn duplicate_session_check_uses_reverse_map() {
        // Verifies that the reverse map honors the same invariant as the
        // prior linear scan: a closed-but-not-yet-reaped channel does not
        // block re-binding the same session.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let s = test_session_id();
        let ch = host.open_channel(s.clone()).await.unwrap();
        host.close_channel(&ch).await.unwrap();
        // Closed channel still in retention map; rebind must succeed.
        host.open_channel(s).await.unwrap();
    }

    // ---------------------------------------------------------------------
    // Test fakes
    // ---------------------------------------------------------------------

    struct StubAdapter;

    impl StubAdapter {
        fn new() -> Self {
            Self
        }
    }

    #[async_trait]
    impl LiveAdapter for StubAdapter {
        async fn send_command(&self, _command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
            Ok(None)
        }

        fn status(&self) -> LiveAdapterStatus {
            LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), LiveAdapterError> {
            Ok(())
        }
    }

    /// Adapter whose pump always errors.
    struct ErroringAdapter;

    #[async_trait]
    impl LiveAdapter for ErroringAdapter {
        async fn send_command(&self, _command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
            Err(LiveAdapterError::TransportError {
                message: "pump dead".into(),
            })
        }

        fn status(&self) -> LiveAdapterStatus {
            LiveAdapterStatus::Closed
        }

        async fn close(&self) -> Result<(), LiveAdapterError> {
            Ok(())
        }
    }

    /// Adapter that records `SubmitToolResult` / `SubmitToolError` so tests
    /// can verify the round-trip from observation → dispatch → submit.
    #[derive(Default)]
    struct RecordingAdapter {
        submitted_results: StdMutex<Vec<LiveToolResult>>,
        submitted_errors: StdMutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl LiveAdapter for RecordingAdapter {
        async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
            match command {
                LiveAdapterCommand::SubmitToolResult { result } => {
                    self.submitted_results.lock().unwrap().push(result);
                }
                LiveAdapterCommand::SubmitToolError { call_id, error } => {
                    self.submitted_errors.lock().unwrap().push((call_id, error));
                }
                _ => {}
            }
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
            Ok(None)
        }

        fn status(&self) -> LiveAdapterStatus {
            LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), LiveAdapterError> {
            Ok(())
        }
    }

    /// Tool dispatcher fake that records the calls it sees and returns an
    /// echo result so the host can submit it back to the adapter.
    #[derive(Default)]
    struct RecordingDispatcher {
        /// (call_id, tool_name, args_json)
        calls: StdMutex<Vec<(String, String, String)>>,
    }

    #[async_trait]
    impl AgentToolDispatcher for RecordingDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities::default()
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::from([])
        }

        fn pending_catalog_sources(&self) -> Arc<[String]> {
            Arc::from([])
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, meerkat_core::error::ToolError> {
            self.calls.lock().unwrap().push((
                call.id.to_string(),
                call.name.to_string(),
                call.args.get().to_string(),
            ));
            let tool_result =
                meerkat_core::types::ToolResult::new(call.id.to_string(), "ok".into(), false);
            Ok(ToolDispatchOutcome::from(tool_result))
        }

        fn capabilities(&self) -> DispatcherCapabilities {
            DispatcherCapabilities::default()
        }
    }

    /// Tool dispatcher that always errors.
    struct FailingDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for FailingDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities::default()
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            Arc::from([])
        }

        fn pending_catalog_sources(&self) -> Arc<[String]> {
            Arc::from([])
        }

        async fn dispatch(
            &self,
            _call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, meerkat_core::error::ToolError> {
            Err(meerkat_core::error::ToolError::ExecutionFailed {
                message: "bang".into(),
            })
        }

        fn capabilities(&self) -> DispatcherCapabilities {
            DispatcherCapabilities::default()
        }
    }

    /// Owned mirror of [`LiveTranscriptIdentity`] used by the recording sink
    /// so tests can keep captured rows past the synchronous trait callsite.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct OwnedIdentity {
        provider_item_id: Option<String>,
        previous_item_id: Option<String>,
        content_index: Option<u32>,
        response_id: Option<String>,
        delta_id: Option<String>,
    }

    impl OwnedIdentity {
        fn from_borrowed(identity: LiveTranscriptIdentity<'_>) -> Self {
            Self {
                provider_item_id: identity.provider_item_id.map(|s| s.to_string()),
                previous_item_id: identity.previous_item_id.map(|s| s.to_string()),
                content_index: identity.content_index,
                response_id: identity.response_id.map(|s| s.to_string()),
                delta_id: identity.delta_id.map(|s| s.to_string()),
            }
        }
    }

    /// Records every projection sink call so tests can assert routing
    /// decisions deterministically. Identity is captured in full so tests
    /// can pin A11's end-to-end identity preservation.
    #[derive(Default)]
    #[allow(clippy::type_complexity)]
    struct RecordingProjectionSink {
        user_transcripts: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        // T6: split text-lane and transcript-lane recording so tests can
        // assert the host routed the right observation to the right method.
        text_deltas: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        transcript_deltas: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        text_finals: StdMutex<
            Vec<(
                SessionId,
                String,
                OwnedIdentity,
                StopReason,
                Usage,
                Option<String>,
            )>,
        >,
        transcript_finals: StdMutex<
            Vec<(
                SessionId,
                String,
                OwnedIdentity,
                StopReason,
                Usage,
                Option<String>,
            )>,
        >,
        truncations: StdMutex<
            Vec<(
                SessionId,
                Option<String>,
                Option<String>,
                Option<u32>,
                Option<String>,
                Option<String>,
            )>,
        >,
        interrupts: StdMutex<Vec<(SessionId, Option<String>)>>,
        turn_completed: StdMutex<Vec<(SessionId, StopReason, Usage, Option<String>)>>,
        terminal_errors: StdMutex<Vec<(SessionId, LiveAdapterErrorCode, String)>>,
        realtime_events: StdMutex<Vec<(SessionId, RealtimeTranscriptEvent)>>,
    }

    #[async_trait]
    impl LiveProjectionSink for RecordingProjectionSink {
        async fn append_user_transcript(
            &self,
            session_id: &SessionId,
            text: &str,
            identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            self.user_transcripts.lock().unwrap().push((
                session_id.clone(),
                text.to_string(),
                OwnedIdentity::from_borrowed(identity),
            ));
            Ok(())
        }

        async fn append_assistant_text_delta(
            &self,
            session_id: &SessionId,
            delta: &str,
            identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            self.text_deltas.lock().unwrap().push((
                session_id.clone(),
                delta.to_string(),
                OwnedIdentity::from_borrowed(identity),
            ));
            Ok(())
        }

        async fn append_assistant_transcript_delta(
            &self,
            session_id: &SessionId,
            delta: &str,
            identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            self.transcript_deltas.lock().unwrap().push((
                session_id.clone(),
                delta.to_string(),
                OwnedIdentity::from_borrowed(identity),
            ));
            Ok(())
        }

        async fn append_assistant_text_final(
            &self,
            session_id: &SessionId,
            text: &str,
            identity: LiveTranscriptIdentity<'_>,
            stop_reason: StopReason,
            usage: Usage,
            response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.text_finals.lock().unwrap().push((
                session_id.clone(),
                text.to_string(),
                OwnedIdentity::from_borrowed(identity),
                stop_reason,
                usage,
                response_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn append_assistant_transcript_final(
            &self,
            session_id: &SessionId,
            text: &str,
            identity: LiveTranscriptIdentity<'_>,
            stop_reason: StopReason,
            usage: Usage,
            response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.transcript_finals.lock().unwrap().push((
                session_id.clone(),
                text.to_string(),
                OwnedIdentity::from_borrowed(identity),
                stop_reason,
                usage,
                response_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn truncate_assistant_transcript(
            &self,
            session_id: &SessionId,
            provider_item_id: Option<&str>,
            previous_item_id: Option<&str>,
            content_index: Option<u32>,
            response_id: Option<&str>,
            text: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.truncations.lock().unwrap().push((
                session_id.clone(),
                provider_item_id.map(|s| s.to_string()),
                previous_item_id.map(|s| s.to_string()),
                content_index,
                response_id.map(|s| s.to_string()),
                text.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn signal_turn_interrupt(
            &self,
            session_id: &SessionId,
            response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.interrupts
                .lock()
                .unwrap()
                .push((session_id.clone(), response_id.map(|s| s.to_string())));
            Ok(())
        }

        async fn signal_turn_completed(
            &self,
            session_id: &SessionId,
            stop_reason: StopReason,
            usage: Usage,
            response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.turn_completed.lock().unwrap().push((
                session_id.clone(),
                stop_reason,
                usage,
                response_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn signal_terminal_error(
            &self,
            session_id: &SessionId,
            code: LiveAdapterErrorCode,
            message: &str,
        ) -> Result<(), LiveProjectionError> {
            self.terminal_errors.lock().unwrap().push((
                session_id.clone(),
                code,
                message.to_string(),
            ));
            Ok(())
        }

        async fn append_realtime_transcript(
            &self,
            session_id: &SessionId,
            event: &RealtimeTranscriptEvent,
        ) -> Result<(), LiveProjectionError> {
            self.realtime_events
                .lock()
                .unwrap()
                .push((session_id.clone(), event.clone()));
            Ok(())
        }
    }
}

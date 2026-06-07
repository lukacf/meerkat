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
//! - `UpdateStatus`      → reports status already committed through generated
//!   MeerkatMachine status authority.
//! - `TerminalError`     → surfaces the error to the sink only after
//!   generated close authority has committed channel terminality.
//! - `Noop`              → no-op (e.g. bare audio chunks in the projection).
//!
//! `LiveProjectionSink` is the runtime-side abstraction over `SessionService`.
//! Wiring an implementation through `SessionService` is the surface side's
//! responsibility — the host trait is intentionally minimal so the same host
//! can be exercised in deterministic unit tests with a recorder fake.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indexmap::IndexMap;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::RealtimeTranscriptEvent;
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

/// Typed evidence that a live refresh command was accepted onto the adapter
/// command queue.
///
/// This is host-minted observation evidence, not the public refresh result.
/// External crates can read it and submit it to MeerkatMachine, but cannot
/// forge it because the constructor is private to `meerkat-live`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRefreshQueueAcceptance {
    channel_id: String,
    acceptance_sequence: u64,
}

impl LiveRefreshQueueAcceptance {
    #[must_use]
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    #[must_use]
    pub fn acceptance_sequence(&self) -> u64 {
        self.acceptance_sequence
    }

    fn from_host_queue_acceptance(
        channel_id: impl Into<String>,
        acceptance_sequence: u64,
    ) -> Option<Self> {
        let channel_id = channel_id.into();
        if channel_id.is_empty() || acceptance_sequence == 0 {
            return None;
        }
        Some(Self {
            channel_id,
            acceptance_sequence,
        })
    }
}

/// Closed classifier for live adapter commands whose queue acceptance backs a
/// public RPC result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveCommandAcceptanceKind {
    SendInput,
    CommitInput,
    Interrupt,
    TruncateAssistantOutput,
}

/// Typed evidence that a live adapter command was accepted onto the adapter
/// command queue.
///
/// This is host-minted observation evidence, not the public RPC result.
/// External crates can read it and submit it to MeerkatMachine, but cannot
/// forge it because the constructor is private to `meerkat-live`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCommandQueueAcceptance {
    channel_id: String,
    kind: LiveCommandAcceptanceKind,
    acceptance_sequence: u64,
}

impl LiveCommandQueueAcceptance {
    #[must_use]
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    #[must_use]
    pub fn kind(&self) -> LiveCommandAcceptanceKind {
        self.kind
    }

    #[must_use]
    pub fn acceptance_sequence(&self) -> u64 {
        self.acceptance_sequence
    }

    fn from_host_queue_acceptance(
        channel_id: impl Into<String>,
        kind: LiveCommandAcceptanceKind,
        acceptance_sequence: u64,
    ) -> Option<Self> {
        let channel_id = channel_id.into();
        if channel_id.is_empty() || acceptance_sequence == 0 {
            return None;
        }
        Some(Self {
            channel_id,
            kind,
            acceptance_sequence,
        })
    }
}

/// Typed evidence that a live channel close handoff was accepted by the host.
///
/// This is host-minted observation evidence, not the public close result.
/// External crates can read it and submit it to MeerkatMachine, but cannot
/// forge it because the constructor is private to `meerkat-live`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveChannelCloseObservation {
    channel_id: String,
    close_sequence: u64,
}

impl LiveChannelCloseObservation {
    #[must_use]
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    #[must_use]
    pub fn close_sequence(&self) -> u64 {
        self.close_sequence
    }

    fn from_host_close_observation(
        channel_id: impl Into<String>,
        close_sequence: u64,
    ) -> Option<Self> {
        let channel_id = channel_id.into();
        if channel_id.is_empty() || close_sequence == 0 {
            return None;
        }
        Some(Self {
            channel_id,
            close_sequence,
        })
    }
}

/// Non-forgeable generated-authority handoff for committing host close cleanup.
#[derive(Debug, Clone)]
pub struct LiveChannelCloseCommitAuthority {
    channel_id: String,
    close_sequence: u64,
    consumed: Arc<AtomicBool>,
}

impl LiveChannelCloseCommitAuthority {
    #[must_use]
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    #[must_use]
    pub fn close_sequence(&self) -> u64 {
        self.close_sequence
    }

    fn consume_once(&self) -> Result<(), LiveAdapterHostError> {
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LiveAdapterHostError::CloseAuthorityAlreadyConsumed)
    }

    #[cfg_attr(not(meerkat_internal_generated_authority_bridge), allow(dead_code))]
    fn from_generated_parts(channel_id: String, close_sequence: u64) -> Self {
        Self {
            channel_id,
            close_sequence,
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    fn from_generated_test_machine(
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        close_sequence: u64,
    ) -> Self {
        let mut authority =
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineAuthority::new();
        authority
            .apply_signal(
                meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineSignal::Initialize,
            )
            .expect("initialize generated MeerkatMachine authority");
        let channel_id_string = channel_id.as_str().to_owned();
        meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineMutator::apply(
            &mut authority,
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineInput::ResolveLiveOpenAdmission {
                session_id: session_id.to_string(),
                channel_id: channel_id_string.clone(),
                llm_identity: generated_test_llm_identity(),
            },
        )
        .expect("generated MeerkatMachine live-open admission");
        let transition = meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineMutator::apply(
            &mut authority,
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineInput::RecordLiveCloseClosed {
                session_id: session_id.to_string(),
                channel_id: channel_id_string.clone(),
                close_observation_sequence: close_sequence,
            },
        )
        .expect("generated MeerkatMachine live-close result");
        assert!(
            transition.effects().iter().any(|effect| matches!(
                effect,
                meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineEffect::LiveCloseResultResolved {
                    channel_id: effect_channel_id,
                    closed: true,
                    close_observation_sequence,
                    ..
                } if *effect_channel_id == channel_id_string && *close_observation_sequence == close_sequence
            )),
            "generated live-close result effect"
        );
        Self::from_generated_parts(channel_id_string, close_sequence)
    }
}

/// Host-minted observation of the adapter transport status.
///
/// The host owns transport mechanics and observed adapter health; generated
/// MeerkatMachine authority owns the public `live/status` result projected to
/// SDK/RPC callers. `observation_sequence` is a provenance nonce for
/// correlating a shell observation with the generated effect that accepted it;
/// skipped or rejected nonce values are not semantic status truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveChannelStatusObservation {
    channel_id: String,
    status: LiveAdapterStatus,
    observation_sequence: u64,
}

impl LiveChannelStatusObservation {
    #[must_use]
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    #[must_use]
    pub fn status(&self) -> &LiveAdapterStatus {
        &self.status
    }

    #[must_use]
    pub fn observation_sequence(&self) -> u64 {
        self.observation_sequence
    }

    fn from_host_status_observation(
        channel_id: impl Into<String>,
        status: LiveAdapterStatus,
        observation_sequence: u64,
    ) -> Option<Self> {
        let channel_id = channel_id.into();
        if channel_id.is_empty() || observation_sequence == 0 {
            return None;
        }
        Some(Self {
            channel_id,
            status,
            observation_sequence,
        })
    }
}

/// Non-forgeable generated-authority handoff for committing host status cache.
#[derive(Debug, Clone)]
pub struct LiveChannelStatusCommitAuthority {
    channel_id: String,
    status_observation_sequence: u64,
    consumed: Arc<AtomicBool>,
}

impl LiveChannelStatusCommitAuthority {
    #[must_use]
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    #[must_use]
    pub fn status_observation_sequence(&self) -> u64 {
        self.status_observation_sequence
    }

    fn consume_once(&self) -> Result<(), LiveAdapterHostError> {
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LiveAdapterHostError::StatusAuthorityAlreadyConsumed)
    }

    #[cfg_attr(not(meerkat_internal_generated_authority_bridge), allow(dead_code))]
    fn from_generated_parts(channel_id: String, status_observation_sequence: u64) -> Self {
        Self {
            channel_id,
            status_observation_sequence,
            consumed: Arc::new(AtomicBool::new(false)),
        }
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

/// Typed terminal fact for a live tool dispatch that exceeded the configured
/// per-call timeout (D281).
///
/// The host already returns the typed [`ObservationOutcome::ToolCallTimedOut`]
/// to its own callers; this newtype is the *adapter-facing* counterpart. The
/// previous code fabricated an ad-hoc parseable string
/// (`format!("tool dispatch timeout after {timeout:?}")`) and submitted it as
/// the [`LiveAdapterCommand::SubmitToolError`] payload, asking the adapter /
/// downstream to recover the fact by parsing prose. Instead the host now mints
/// this typed fact; the only point a string is produced is its [`Display`]
/// projection at the meerkat-core `SubmitToolError { error: String }` seam
/// edge (that wire field is owned by `meerkat-core::live_adapter`, so the
/// adapter command itself cannot be made fully typed from this crate). The
/// `Display` rendering is a deterministic projection of the typed fields, not
/// a source of truth: callers in this crate route on the typed value, never on
/// the rendered string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveToolDispatchTimeout {
    timeout: Duration,
}

impl LiveToolDispatchTimeout {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl std::fmt::Display for LiveToolDispatchTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tool dispatch timeout after {:?}", self.timeout)
    }
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

    /// Resolve the required identity triple for an assistant **delta** event
    /// (D199): `response_id`, `delta_id`, and `item_id` (provider item id).
    ///
    /// The sink previously coalesced each missing id to an empty string via
    /// `unwrap_or_default()`, emitting an [`RealtimeTranscriptEvent`] whose
    /// identity was empty-string truth — a delta that cannot be bound back to
    /// any response/item. This accessor fails closed: a missing required id
    /// yields a typed [`LiveTranscriptIdentityError`] naming the absent field,
    /// so the projection rejects the malformed delta rather than fabricating
    /// empty identity.
    pub fn require_delta_identity(&self) -> Result<DeltaIdentity<'a>, LiveTranscriptIdentityError> {
        let response_id = self
            .response_id
            .ok_or(LiveTranscriptIdentityError::MissingResponseId)?;
        let delta_id = self
            .delta_id
            .ok_or(LiveTranscriptIdentityError::MissingDeltaId)?;
        let item_id = self
            .provider_item_id
            .ok_or(LiveTranscriptIdentityError::MissingItemId)?;
        Ok(DeltaIdentity {
            response_id,
            delta_id,
            item_id,
            previous_item_id: self.previous_item_id,
            content_index: self.content_index,
        })
    }
}

/// The required identity triple resolved for an assistant **delta** event
/// (D199). Produced by [`LiveTranscriptIdentity::require_delta_identity`];
/// guarantees `response_id` / `delta_id` / `item_id` are present (non-empty
/// fail-closed), so the realtime-transcript layer can key per-response staging
/// without empty-string identity truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeltaIdentity<'a> {
    pub response_id: &'a str,
    pub delta_id: &'a str,
    pub item_id: &'a str,
    pub previous_item_id: Option<&'a str>,
    pub content_index: Option<u32>,
}

/// Typed failure when a transcript identity is missing a required id (D199).
///
/// Replaces the prior `unwrap_or_default()` empty-string coalescing in the
/// projection sink: a delta lacking `response_id` / `delta_id` / `item_id`
/// fails closed with a named field rather than emitting empty-string identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LiveTranscriptIdentityError {
    #[error("transcript delta is missing a required response_id")]
    MissingResponseId,
    #[error("transcript delta is missing a required delta_id")]
    MissingDeltaId,
    #[error("transcript delta is missing a required item_id")]
    MissingItemId,
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
///
/// D301: failure semantics are typed, not prose-parsed. Each underlying
/// [`meerkat_core::SessionError`] that a sink encounters lands in a *distinct*
/// variant via [`LiveProjectionError::from_session_error`], carrying the
/// session error's stable `error_code` so downstream callers route on a typed
/// class rather than reparsing `SessionError::to_string()`. The previous sink
/// mapper collapsed every non-`NotFound`/`Unsupported` variant into
/// `Internal(other.to_string())`, erasing the typed cause.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LiveProjectionError {
    #[error("session {0} not found")]
    SessionNotFound(SessionId),
    /// The session service refused the projection (typed
    /// `SessionError::Unsupported`).
    #[error("projection rejected: {0}")]
    Rejected(String),
    /// A turn is already in progress on the target session
    /// (`SessionError::Busy`).
    #[error("session busy: {0}")]
    SessionBusy(SessionId),
    /// No turn is currently running on the target session
    /// (`SessionError::NotRunning`).
    #[error("session not running: {0}")]
    SessionNotRunning(SessionId),
    /// A required session capability (persistence / compaction) is disabled.
    /// Carries the disabled capability's stable session error code.
    #[error("session capability disabled ({code}): {message}")]
    CapabilityDisabled { code: &'static str, message: String },
    /// A session-level failure whose typed cause is identified by the stable
    /// `SessionError::code` (store error, agent error, structured failure).
    /// Carries the code so callers route on the typed class, not the message.
    #[error("session error [{code}]: {message}")]
    Session { code: &'static str, message: String },
    #[error("projection sink internal error: {0}")]
    Internal(String),
}

impl LiveProjectionError {
    /// Classify a [`meerkat_core::SessionError`] into a typed
    /// [`LiveProjectionError`] variant (D301).
    ///
    /// `session_id` is the projection target, used to populate identity-bearing
    /// variants. Every `SessionError` lands in a distinct typed variant
    /// carrying the session error's stable `code()` — no variant is collapsed
    /// into a prose-only `Internal(to_string())`. This is the single
    /// classification owner; surfaces route through it instead of re-deriving
    /// the mapping per call site.
    #[must_use]
    pub fn from_session_error(session_id: &SessionId, err: meerkat_core::SessionError) -> Self {
        use meerkat_core::SessionError;
        // Compute the stable code + human-readable message before the match
        // consumes `err`. `code()` returns a `&'static str` and `to_string()`
        // borrows `err` via `Display`, so both are available before the move.
        let code = err.code();
        let message = err.to_string();
        match err {
            SessionError::NotFound { .. } => Self::SessionNotFound(session_id.clone()),
            SessionError::Unsupported(reason) => Self::Rejected(reason),
            SessionError::Busy { id } => Self::SessionBusy(id),
            SessionError::NotRunning { id } => Self::SessionNotRunning(id),
            SessionError::PersistenceDisabled | SessionError::CompactionDisabled => {
                Self::CapabilityDisabled { code, message }
            }
            _ => Self::Session { code, message },
        }
    }
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

/// Per-channel transport state tracked by the host.
struct ChannelState {
    session_id: SessionId,
    /// Generated-committed transport status cache. The host can reserve typed
    /// status observations, but this cache changes only after generated
    /// MeerkatMachine status authority returns a commit handoff.
    status: LiveAdapterStatus,
    /// Host observation nonce. This can advance while collecting evidence, but
    /// generated MeerkatMachine authority owns the accepted status sequence in
    /// `live_channel_status_observation_sequence_by_channel`.
    status_observation_sequence: u64,
    snapshot_version: u64,
    refresh_acceptance_sequence: u64,
    command_acceptance_sequence: u64,
    close_observation_sequence: u64,
    adapter: Option<Arc<dyn LiveAdapter>>,
    /// Transport-retention deadline for a channel the generated owner has
    /// already closed/terminalized. This is a resource-cache timer: public
    /// lifecycle/admission surfaces must route through generated authority
    /// before they interpret the channel as open, closed, or reusable.
    /// Reads are still serviced during the grace window; adapter handoffs are
    /// rejected because the transport handle has been released.
    retire_at: Option<std::time::Instant>,
    /// CC1 (R11 wire-signal): one-shot synthetic observation to deliver to
    /// the next [`LiveAdapterHost::next_observation_raw`] caller before any
    /// adapter poll happens.
    ///
    /// Populated by [`LiveAdapterHost::signal_terminal_error`] so a typed
    /// terminal error (e.g. `ConfigRejected { reason: "model_swap: ..." }`)
    /// flows through the same generated close-authority path as a
    /// provider-emitted Error observation. Without this, runtime-side
    /// terminations can tear down transport resources without recording the
    /// machine-owned close fact first.
    ///
    /// Single-slot by design: there's exactly one terminal moment per
    /// channel; subsequent `signal_terminal_error` calls overwrite (the
    /// channel is being torn down either way). Read priority — see
    /// `next_observation_raw` — must come before `adapter_for` so the
    /// synthetic obs survives a concurrent `close_channel`.
    pending_synthetic_obs: Option<LiveAdapterObservation>,
}

/// Non-forgeable generated-authority handoff for materializing a live channel.
///
/// `LiveAdapterHost` owns transport resources only. Callers that expose
/// public lifecycle/admission behavior must first route `live/open` through
/// generated machine authority and pass the resulting admitted handoff here.
#[derive(Debug, Clone)]
pub struct LiveChannelOpenAuthority {
    session_id: SessionId,
    channel_id: LiveChannelId,
    sequence: u64,
    consumed: Arc<AtomicBool>,
}

#[cfg(test)]
fn generated_test_llm_identity()
-> meerkat_machine_schema::catalog::dsl::meerkat_machine::SessionLlmIdentity {
    meerkat_machine_schema::catalog::dsl::meerkat_machine::SessionLlmIdentity {
        model: "gpt-realtime-2".to_string(),
        provider: meerkat_machine_schema::catalog::dsl::meerkat_machine::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params_repr: None,
        auth_binding: None,
    }
}

#[cfg(test)]
fn generated_test_live_channel_status(
    status: &LiveAdapterStatus,
) -> (
    meerkat_machine_schema::catalog::dsl::meerkat_machine::LiveChannelPublicStatus,
    Option<meerkat_machine_schema::catalog::dsl::meerkat_machine::LiveChannelDegradationReason>,
    Option<String>,
) {
    use meerkat_core::live_adapter::LiveDegradationReason;
    use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
        LiveChannelDegradationReason as DslReason, LiveChannelPublicStatus as DslStatus,
    };

    match status {
        LiveAdapterStatus::Idle => (DslStatus::Idle, None, None),
        LiveAdapterStatus::Opening => (DslStatus::Opening, None, None),
        LiveAdapterStatus::Ready => (DslStatus::Ready, None, None),
        LiveAdapterStatus::Closing => (DslStatus::Closing, None, None),
        LiveAdapterStatus::Closed => (DslStatus::Closed, None, None),
        LiveAdapterStatus::Degraded { reason } => match reason {
            LiveDegradationReason::RateLimited => {
                (DslStatus::Degraded, Some(DslReason::RateLimited), None)
            }
            LiveDegradationReason::ProviderThrottled => (
                DslStatus::Degraded,
                Some(DslReason::ProviderThrottled),
                None,
            ),
            LiveDegradationReason::NetworkUnstable => {
                (DslStatus::Degraded, Some(DslReason::NetworkUnstable), None)
            }
            LiveDegradationReason::Other { detail } => (
                DslStatus::Degraded,
                Some(DslReason::Other),
                Some(detail.clone().into_owned()),
            ),
            other => (
                DslStatus::Degraded,
                Some(DslReason::Unknown),
                Some(format!("{other:?}")),
            ),
        },
        other => (
            DslStatus::Degraded,
            Some(DslReason::Unknown),
            Some(format!("{other:?}")),
        ),
    }
}

impl LiveChannelOpenAuthority {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    fn consume_once(&self) -> Result<(), LiveAdapterHostError> {
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LiveAdapterHostError::OpenAuthorityAlreadyConsumed)
    }

    #[cfg_attr(not(meerkat_internal_generated_authority_bridge), allow(dead_code))]
    fn from_generated_parts(
        session_id: SessionId,
        channel_id: LiveChannelId,
        sequence: u64,
    ) -> Self {
        Self {
            session_id,
            channel_id,
            sequence,
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    fn from_generated_test_machine(session_id: SessionId, channel_id: LiveChannelId) -> Self {
        let mut authority =
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineAuthority::new();
        authority
            .apply_signal(
                meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineSignal::Initialize,
            )
            .expect("initialize generated MeerkatMachine authority");
        let channel_id_string = channel_id.as_str().to_owned();
        let transition = meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineMutator::apply(
            &mut authority,
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineInput::ResolveLiveOpenAdmission {
                session_id: session_id.to_string(),
                channel_id: channel_id_string.clone(),
                llm_identity: generated_test_llm_identity(),
            },
        )
        .expect("generated MeerkatMachine live-open admission");
        let sequence = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineEffect::LiveOpenAdmissionResolved {
                    channel_id: effect_channel_id,
                    admitted: true,
                    sequence,
                    ..
                } if *effect_channel_id == channel_id_string => Some(*sequence),
                _ => None,
            })
            .expect("generated live-open admission effect");
        Self::from_generated_parts(session_id, channel_id, sequence)
    }
}

#[cfg(meerkat_internal_generated_authority_bridge)]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_open_admission_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_live_open_admission_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;

    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_close_result_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_live_close_result_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;

    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_channel_status_result_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_live_channel_status_result_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;
}

#[cfg(meerkat_internal_generated_authority_bridge)]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_live_runtime_generated_live_channel_open_authority_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_channel_open_authority_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    session_id: SessionId,
    channel_id: LiveChannelId,
    sequence: u64,
) -> Result<LiveChannelOpenAuthority, String> {
    #[allow(unsafe_code)]
    let valid =
        unsafe { runtime_live_open_admission_generated_authority_bridge_token_is_valid(token) };
    if !valid {
        return Err(
            "live channel open authority requires the generated runtime admission bridge token"
                .into(),
        );
    }
    Ok(LiveChannelOpenAuthority::from_generated_parts(
        session_id, channel_id, sequence,
    ))
}

#[cfg(meerkat_internal_generated_authority_bridge)]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_live_runtime_generated_live_channel_close_commit_authority_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_channel_close_commit_authority_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    channel_id: String,
    close_sequence: u64,
) -> Result<LiveChannelCloseCommitAuthority, String> {
    #[allow(unsafe_code)]
    let valid =
        unsafe { runtime_live_close_result_generated_authority_bridge_token_is_valid(token) };
    if !valid {
        return Err(
            "live channel close commit authority requires the generated runtime close bridge token"
                .into(),
        );
    }
    Ok(LiveChannelCloseCommitAuthority::from_generated_parts(
        channel_id,
        close_sequence,
    ))
}

#[cfg(meerkat_internal_generated_authority_bridge)]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_live_runtime_generated_live_channel_status_commit_authority_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_channel_status_commit_authority_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    channel_id: String,
    status_observation_sequence: u64,
) -> Result<LiveChannelStatusCommitAuthority, String> {
    #[allow(unsafe_code)]
    let valid = unsafe {
        runtime_live_channel_status_result_generated_authority_bridge_token_is_valid(token)
    };
    if !valid {
        return Err(
            "live channel status commit authority requires the generated runtime status bridge token"
                .into(),
        );
    }
    Ok(LiveChannelStatusCommitAuthority::from_generated_parts(
        channel_id,
        status_observation_sequence,
    ))
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
    #[error("live channel open lacks generated admission authority")]
    OpenNotAuthorized,
    #[error("live channel open authority was already consumed")]
    OpenAuthorityAlreadyConsumed,
    #[error("live channel close lacks generated commit authority")]
    CloseNotAuthorized,
    #[error("live channel close authority was already consumed")]
    CloseAuthorityAlreadyConsumed,
    #[error("live channel status lacks generated commit authority")]
    StatusNotAuthorized,
    #[error("live channel status authority was already consumed")]
    StatusAuthorityAlreadyConsumed,
    #[error("no adapter attached to channel {0}")]
    NoAdapter(LiveChannelId),
    #[error("unsupported host command: {0}")]
    UnsupportedCommand(&'static str),
    /// E29: typed adapter error preserved structurally (not flattened to String).
    #[error(transparent)]
    AdapterError(#[from] LiveAdapterError),
    #[error("projection sink error: {0}")]
    ProjectionError(#[from] LiveProjectionError),
}

impl LiveAdapterHostError {
    /// Stable typed reason code for this host error (D153).
    ///
    /// Transports surface this as the `WsErrorFrame.reason` so clients route
    /// on a typed class rather than reparsing the human-readable `error`
    /// message. The codes are stable wire strings; adding a variant must add
    /// its code here (the match is exhaustive over the `#[non_exhaustive]`
    /// enum from inside the crate).
    #[must_use]
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ChannelNotFound(_) => "channel_not_found",
            Self::SessionNotFound(_) => "session_not_found",
            Self::ChannelNotReady(..) => "channel_not_ready",
            Self::SessionAlreadyBound(_) => "session_already_bound",
            Self::OpenNotAuthorized => "open_not_authorized",
            Self::OpenAuthorityAlreadyConsumed => "open_authority_already_consumed",
            Self::CloseNotAuthorized => "close_not_authorized",
            Self::CloseAuthorityAlreadyConsumed => "close_authority_already_consumed",
            Self::StatusNotAuthorized => "status_not_authorized",
            Self::StatusAuthorityAlreadyConsumed => "status_authority_already_consumed",
            Self::NoAdapter(_) => "no_adapter",
            Self::UnsupportedCommand(_) => "unsupported_command",
            Self::AdapterError(_) => "adapter_error",
            Self::ProjectionError(_) => "projection_error",
        }
    }
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

    pub async fn open_channel_with_authority(
        &self,
        authority: &LiveChannelOpenAuthority,
    ) -> Result<LiveChannelId, LiveAdapterHostError> {
        authority.consume_once()?;
        self.open_channel_after_generated_authority(
            authority.session_id().clone(),
            authority.channel_id().clone(),
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn open_channel_with_generated_test_machine_authority(
        &self,
        session_id: SessionId,
    ) -> Result<LiveChannelId, LiveAdapterHostError> {
        let channel_id = LiveChannelId::random_uuid();
        let authority =
            LiveChannelOpenAuthority::from_generated_test_machine(session_id, channel_id);
        self.open_channel_with_authority(&authority).await
    }

    async fn open_channel_after_generated_authority(
        &self,
        session_id: SessionId,
        channel_id: LiveChannelId,
    ) -> Result<LiveChannelId, LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);

        // Resource-cache consistency check after generated admission. Public
        // duplicate-session admission is owned by MeerkatMachine; this guard
        // only fails closed if the host still has a live transport binding
        // that contradicts the generated handoff.
        if let Some(existing) = inner.by_session.get(&session_id).cloned()
            && let Some(channel) = inner.channels.get(&existing)
            && channel.retire_at.is_none()
        {
            return Err(LiveAdapterHostError::SessionAlreadyBound(session_id));
        }

        inner.channels.insert(
            channel_id.clone(),
            ChannelState {
                session_id: session_id.clone(),
                status: LiveAdapterStatus::Opening,
                status_observation_sequence: 0,
                snapshot_version: 0,
                refresh_acceptance_sequence: 0,
                command_acceptance_sequence: 0,
                close_observation_sequence: 0,
                adapter: None,
                retire_at: None,
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

    fn command_acceptance_kind(
        command: &LiveAdapterCommand,
    ) -> Result<LiveCommandAcceptanceKind, LiveAdapterHostError> {
        match command {
            LiveAdapterCommand::SendInput { .. } => Ok(LiveCommandAcceptanceKind::SendInput),
            LiveAdapterCommand::CommitInput { .. } => Ok(LiveCommandAcceptanceKind::CommitInput),
            LiveAdapterCommand::Interrupt => Ok(LiveCommandAcceptanceKind::Interrupt),
            LiveAdapterCommand::TruncateAssistantOutput { .. } => {
                Ok(LiveCommandAcceptanceKind::TruncateAssistantOutput)
            }
            LiveAdapterCommand::Refresh { .. } => Err(LiveAdapterHostError::UnsupportedCommand(
                "refresh commands must use LiveAdapterHost::enqueue_refresh",
            )),
            _ => Err(LiveAdapterHostError::UnsupportedCommand(
                "live command has no public queue-acceptance authority",
            )),
        }
    }

    async fn record_command_queue_acceptance(
        &self,
        channel_id: &LiveChannelId,
        kind: LiveCommandAcceptanceKind,
    ) -> Result<LiveCommandQueueAcceptance, LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.command_acceptance_sequence = channel.command_acceptance_sequence.saturating_add(1);
        LiveCommandQueueAcceptance::from_host_queue_acceptance(
            channel_id.as_str().to_owned(),
            kind,
            channel.command_acceptance_sequence,
        )
        .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    /// Send a command to the adapter on a channel.
    pub async fn send_command(
        &self,
        channel_id: &LiveChannelId,
        command: LiveAdapterCommand,
    ) -> Result<(), LiveAdapterHostError> {
        if matches!(&command, LiveAdapterCommand::Refresh { .. }) {
            return Err(LiveAdapterHostError::UnsupportedCommand(
                "refresh commands must use LiveAdapterHost::enqueue_refresh",
            ));
        }
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ false)
            .await?;
        adapter.send_command(command).await?;
        Ok(())
    }

    /// Send a command to the adapter and return typed queue-acceptance
    /// evidence for public-result authority.
    pub async fn send_command_observed(
        &self,
        channel_id: &LiveChannelId,
        command: LiveAdapterCommand,
    ) -> Result<LiveCommandQueueAcceptance, LiveAdapterHostError> {
        if matches!(&command, LiveAdapterCommand::Refresh { .. }) {
            return Err(LiveAdapterHostError::UnsupportedCommand(
                "refresh commands must use LiveAdapterHost::enqueue_refresh",
            ));
        }
        let acceptance_kind = Self::command_acceptance_kind(&command)?;
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ false)
            .await?;
        adapter.send_command(command).await?;
        self.record_command_queue_acceptance(channel_id, acceptance_kind)
            .await
    }

    /// Enqueue a refresh command and return typed queue-acceptance evidence.
    ///
    /// The acceptance receipt is minted only after the adapter command queue
    /// accepts the refresh. MeerkatMachine consumes the receipt to decide the
    /// public `live/refresh` result; the host does not construct that result.
    pub async fn enqueue_refresh(
        &self,
        channel_id: &LiveChannelId,
        snapshot: meerkat_core::live_adapter::LiveProjectionSnapshot,
    ) -> Result<LiveRefreshQueueAcceptance, LiveAdapterHostError> {
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ false)
            .await?;
        adapter
            .send_command(LiveAdapterCommand::Refresh { snapshot })
            .await?;

        let mut inner = self.inner.lock().await;
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.refresh_acceptance_sequence = channel.refresh_acceptance_sequence.saturating_add(1);
        LiveRefreshQueueAcceptance::from_host_queue_acceptance(
            channel_id.as_str().to_owned(),
            channel.refresh_acceptance_sequence,
        )
        .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    /// Send an input chunk to the adapter on a channel.
    ///
    /// F31: adapter-mechanical guard using the observed transport status.
    /// Public result/status authority still lives in MeerkatMachine; this
    /// check prevents writes to a transport that is not ready to accept bytes.
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

    /// Send an input chunk and return typed queue-acceptance evidence for
    /// public-result authority. Public transports that do not return a
    /// per-frame success result use [`Self::send_input`] instead so they do
    /// not mint authority evidence that cannot be consumed by MeerkatMachine.
    pub async fn send_input_observed(
        &self,
        channel_id: &LiveChannelId,
        chunk: LiveInputChunk,
    ) -> Result<LiveCommandQueueAcceptance, LiveAdapterHostError> {
        let adapter = self
            .adapter_for(channel_id, /* require_ready = */ true)
            .await?;
        adapter
            .send_command(LiveAdapterCommand::SendInput { chunk })
            .await?;
        self.record_command_queue_acceptance(channel_id, LiveCommandAcceptanceKind::SendInput)
            .await
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

    /// Submit a typed tool-dispatch-timeout fact back to the adapter (D281).
    ///
    /// The caller passes the typed [`LiveToolDispatchTimeout`] fact; the only
    /// stringification happens here, at the meerkat-core
    /// [`LiveAdapterCommand::SubmitToolError`] seam edge, via the fact's
    /// [`Display`] projection. No call site fabricates a parseable prose
    /// string; the typed value is the truth.
    pub async fn submit_tool_dispatch_timeout(
        &self,
        channel_id: &LiveChannelId,
        call_id: String,
        timeout: LiveToolDispatchTimeout,
    ) -> Result<(), LiveAdapterHostError> {
        self.submit_tool_error(channel_id, call_id, timeout.to_string())
            .await
    }

    /// Poll the next observation from the adapter on a channel and project it.
    ///
    /// Convenience wrapper around `next_observation_raw` + `apply_observation`.
    /// This wrapper fails closed on non-terminal status observations because
    /// the host has no generated status feedback handle here. Surfaces that
    /// need status, terminality, or typed [`ObservationOutcome`] values must
    /// call `next_observation_raw`, route status/close evidence through
    /// generated authority, then call `apply_observation`.
    pub async fn next_observation(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<Option<LiveAdapterObservation>, LiveAdapterHostError> {
        let obs = self.next_observation_raw(channel_id).await?;
        if let Some(ref obs) = obs {
            if let ObservationRouting::UpdateStatus(status) = Self::classify_observation(obs)
                && !status.is_terminal()
            {
                return Err(LiveAdapterHostError::StatusNotAuthorized);
            }
            if Self::observation_requires_generated_close(obs)
                && !self.generated_close_has_committed(channel_id).await?
            {
                return Err(LiveAdapterHostError::CloseNotAuthorized);
            }
            // Best-effort projection. Errors are surfaced by `apply_observation`
            // for callers that want to react; here we discard so the read API
            // stays compatible with existing handlers.
            let _ = self.apply_observation(channel_id, obs).await?;
        }
        Ok(obs)
    }

    /// Read the next adapter observation without applying it to canonical state.
    ///
    /// Adapter read failures are surfaced as typed terminal observations.
    /// The host does not commit close/retire state here; transports must route
    /// terminal observations through generated close feedback before cleanup.
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
                let synthetic = LiveAdapterObservation::Error {
                    code: LiveAdapterErrorCode::ProviderError,
                    message: format!("adapter read failure: {err}"),
                };
                Ok(Some(synthetic))
            }
        }
    }

    /// Project an adapter observation into canonical Meerkat semantic state.
    ///
    /// This is the heart of the projection contract (A1–A6, A10, A14):
    /// classify → dispatch → return a typed [`ObservationOutcome`] describing
    /// what was applied. Non-terminal status updates must be committed through
    /// generated status authority before this method is called; terminal
    /// status/error observations require the generated close authority path to
    /// commit first. Transcript / tool / interrupt routing requires the host
    /// to be configured with the relevant injected seams.
    pub async fn apply_observation(
        &self,
        channel_id: &LiveChannelId,
        observation: &LiveAdapterObservation,
    ) -> Result<ObservationOutcome, LiveAdapterHostError> {
        if Self::observation_requires_generated_close(observation)
            && !self.generated_close_has_committed(channel_id).await?
        {
            return Err(LiveAdapterHostError::CloseNotAuthorized);
        }

        let routing = Self::classify_observation(observation);

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
                    // D281: notify the adapter so the provider can unblock its
                    // turn — but carry the typed `LiveToolDispatchTimeout` fact
                    // rather than fabricating a parseable prose string. The
                    // typed `ObservationOutcome::ToolCallTimedOut` is returned
                    // to the host's own callers; the adapter-facing submission
                    // renders the typed fact only at the meerkat-core
                    // `SubmitToolError { error: String }` seam edge (that wire
                    // field is owned by meerkat-core and cannot be made fully
                    // typed from this crate).
                    self.submit_tool_dispatch_timeout(
                        channel_id,
                        provider_call_id.to_string(),
                        LiveToolDispatchTimeout::new(timeout),
                    )
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
    /// which the pump routes through generated close authority before
    /// [`Self::apply_observation`] can yield [`ObservationOutcome::Terminal`].
    ///
    /// Use this when the runtime decides a channel must die for a typed
    /// reason that the provider adapter never produced (e.g. model swap on
    /// `config/patch`, where the OpenAI realtime adapter cannot rebind the
    /// model in-place). Without this seam the host can lose the typed terminal
    /// cause before generated close authority records terminality.
    ///
    /// `message` defaults from the typed code: `ConfigRejected { reason }`
    /// uses `reason` directly, other variants fall back to the serde tag.
    /// The pending obs is enqueued FIRST, then a close observation is reserved
    /// for generated close authority. Callers commit host close cleanup only
    /// after that authority accepts the observation.
    #[cfg(test)]
    pub(crate) async fn signal_terminal_error(
        &self,
        channel_id: &LiveChannelId,
        code: LiveAdapterErrorCode,
    ) -> Result<(), LiveAdapterHostError> {
        let observation = self
            .signal_terminal_error_observed(channel_id, code)
            .await?;
        let authority = self
            .close_commit_authority_from_generated_test_machine(&observation)
            .await?;
        self.commit_channel_close_observation(&observation, &authority)
            .await
    }

    pub async fn signal_terminal_error_observed(
        &self,
        channel_id: &LiveChannelId,
        code: LiveAdapterErrorCode,
    ) -> Result<LiveChannelCloseObservation, LiveAdapterHostError> {
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
        // Order matters: stage FIRST, inject SECOND, reserve close evidence
        // THIRD. If injection fails (adapter already torn down), the fallback in
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
        self.reserve_channel_close_observation(channel_id).await
    }

    pub async fn reserve_channel_close_observation(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<LiveChannelCloseObservation, LiveAdapterHostError> {
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.close_observation_sequence = channel.close_observation_sequence.saturating_add(1);
        LiveChannelCloseObservation::from_host_close_observation(
            channel_id.as_str().to_owned(),
            channel.close_observation_sequence,
        )
        .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    pub async fn commit_channel_close_observation(
        &self,
        observation: &LiveChannelCloseObservation,
        authority: &LiveChannelCloseCommitAuthority,
    ) -> Result<(), LiveAdapterHostError> {
        authority.consume_once()?;
        if authority.channel_id() != observation.channel_id()
            || authority.close_sequence() != observation.close_sequence()
        {
            return Err(LiveAdapterHostError::CloseNotAuthorized);
        }
        // G42: keep the channel reachable for `live/status` until the TTL
        // elapses. We unbind the adapter (releasing transport resources) and
        // mark the channel as `Closed`, but leave the entry in `channels`
        // so post-close reads can report the terminal status. This commit is
        // called only after generated close authority accepts the typed close
        // observation.
        let channel_id = LiveChannelId::new(observation.channel_id().to_owned());
        let adapter = {
            let mut inner = self.inner.lock().await;
            Self::reap_retired_locked(&mut inner);
            let channel = inner
                .channels
                .get_mut(&channel_id)
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

    #[cfg(test)]
    pub(crate) async fn close_commit_authority_from_generated_test_machine(
        &self,
        observation: &LiveChannelCloseObservation,
    ) -> Result<LiveChannelCloseCommitAuthority, LiveAdapterHostError> {
        let channel_id = LiveChannelId::new(observation.channel_id().to_owned());
        let session_id = self.channel_session(&channel_id).await?;
        Ok(
            LiveChannelCloseCommitAuthority::from_generated_test_machine(
                &session_id,
                &channel_id,
                observation.close_sequence(),
            ),
        )
    }

    #[cfg(test)]
    pub(crate) async fn close_channel_observed_with_generated_test_machine_authority(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<LiveChannelCloseObservation, LiveAdapterHostError> {
        let observation = self.reserve_channel_close_observation(channel_id).await?;
        let authority = self
            .close_commit_authority_from_generated_test_machine(&observation)
            .await?;
        self.commit_channel_close_observation(&observation, &authority)
            .await?;
        Ok(observation)
    }

    #[cfg(test)]
    pub(crate) async fn close_channel_with_generated_test_machine_authority(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<(), LiveAdapterHostError> {
        self.close_channel_observed_with_generated_test_machine_authority(channel_id)
            .await
            .map(|_| ())
    }

    #[cfg(test)]
    pub(crate) async fn status_commit_authority_from_generated_test_machine(
        &self,
        observation: &LiveChannelStatusObservation,
    ) -> Result<LiveChannelStatusCommitAuthority, LiveAdapterHostError> {
        let channel_id = LiveChannelId::new(observation.channel_id().to_owned());
        let session_id = self.channel_session(&channel_id).await?;
        let channel_id_string = channel_id.as_str().to_owned();
        let (status, degradation_reason, degradation_detail) =
            generated_test_live_channel_status(observation.status());
        let mut authority =
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineAuthority::new();
        authority
            .apply_signal(
                meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineSignal::Initialize,
            )
            .expect("initialize generated MeerkatMachine authority");
        meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineMutator::apply(
            &mut authority,
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineInput::ResolveLiveOpenAdmission {
                session_id: session_id.to_string(),
                channel_id: channel_id_string.clone(),
                llm_identity: generated_test_llm_identity(),
            },
        )
        .expect("generated MeerkatMachine live-open admission");
        let transition = meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineMutator::apply(
            &mut authority,
            meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineInput::RecordLiveChannelStatus {
                channel_id: channel_id_string.clone(),
                status,
                status_observation_sequence: observation.observation_sequence(),
                degradation_reason,
                degradation_detail: degradation_detail.clone(),
            },
        )
        .expect("generated MeerkatMachine live-status result");
        assert!(
            transition.effects().iter().any(|effect| matches!(
                effect,
                meerkat_machine_schema::catalog::dsl::meerkat_machine::MeerkatMachineEffect::LiveChannelStatusResolved {
                    channel_id: effect_channel_id,
                    status: effect_status,
                    status_observation_sequence,
                    ..
                } if *effect_channel_id == channel_id_string
                    && *effect_status == status
                    && *status_observation_sequence == observation.observation_sequence()
            )),
            "generated live-status result effect"
        );
        Ok(LiveChannelStatusCommitAuthority::from_generated_parts(
            channel_id_string,
            observation.observation_sequence(),
        ))
    }

    #[cfg(test)]
    pub(crate) async fn commit_status_with_generated_test_machine_authority(
        &self,
        channel_id: &LiveChannelId,
        status: LiveAdapterStatus,
    ) -> Result<LiveChannelStatusObservation, LiveAdapterHostError> {
        let observation = self
            .reserve_channel_status_observation(channel_id, status)
            .await?;
        let authority = self
            .status_commit_authority_from_generated_test_machine(&observation)
            .await?;
        self.commit_channel_status_observation(&observation, &authority)
            .await?;
        Ok(observation)
    }

    /// Return the host's generated-committed adapter-status cache.
    ///
    /// Crate-internal transport cleanup and unit tests may inspect this cache
    /// directly. Public surfaces must use [`Self::channel_status_observation`]
    /// and submit the observation to generated MeerkatMachine authority before
    /// projecting a result to callers.
    pub(crate) async fn channel_status(
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

    pub async fn channel_status_observation(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<LiveChannelStatusObservation, LiveAdapterHostError> {
        // This sequence is host-minted observation evidence only. Gaps from
        // failed or abandoned generated submissions are non-semantic; the
        // generated machine owns the accepted sequence it stores in
        // `live_channel_status_observation_sequence_by_channel`.
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.status_observation_sequence = channel.status_observation_sequence.saturating_add(1);
        LiveChannelStatusObservation::from_host_status_observation(
            channel_id.as_str().to_owned(),
            channel.status.clone(),
            channel.status_observation_sequence,
        )
        .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    pub async fn reserve_channel_status_observation(
        &self,
        channel_id: &LiveChannelId,
        status: LiveAdapterStatus,
    ) -> Result<LiveChannelStatusObservation, LiveAdapterHostError> {
        if status.is_terminal() {
            return Err(LiveAdapterHostError::StatusNotAuthorized);
        }
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);
        let channel = inner
            .channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        if channel.retire_at.is_some() {
            return Err(LiveAdapterHostError::ChannelNotReady(
                channel_id.clone(),
                channel.status.clone(),
            ));
        }
        // This sequence is host-minted observation evidence only. It does not
        // become status truth unless generated MeerkatMachine authority accepts
        // it and returns a commit handoff.
        channel.status_observation_sequence = channel.status_observation_sequence.saturating_add(1);
        LiveChannelStatusObservation::from_host_status_observation(
            channel_id.as_str().to_owned(),
            status,
            channel.status_observation_sequence,
        )
        .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    pub async fn commit_channel_status_observation(
        &self,
        observation: &LiveChannelStatusObservation,
        authority: &LiveChannelStatusCommitAuthority,
    ) -> Result<(), LiveAdapterHostError> {
        authority.consume_once()?;
        if authority.channel_id() != observation.channel_id()
            || authority.status_observation_sequence() != observation.observation_sequence()
        {
            return Err(LiveAdapterHostError::StatusNotAuthorized);
        }
        if observation.status().is_terminal() {
            return Err(LiveAdapterHostError::StatusNotAuthorized);
        }
        let channel_id = LiveChannelId::new(observation.channel_id().to_owned());
        let mut inner = self.inner.lock().await;
        Self::reap_retired_locked(&mut inner);
        let channel = inner
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        if channel.retire_at.is_some() {
            return Err(LiveAdapterHostError::ChannelNotReady(
                channel_id,
                channel.status.clone(),
            ));
        }
        channel.status = observation.status().clone();
        Ok(())
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

    /// Lower a transport-local barge-in (e.g. local VAD detecting user speech
    /// while output audio is still queued) into the canonical interrupt seam
    /// (D223).
    ///
    /// This routes through the *same* [`LiveProjectionSink::signal_turn_interrupt`]
    /// path that adapter-observed [`LiveAdapterObservation::TurnInterrupted`]
    /// barge-ins use, so a transport-discarded output-audio queue is a typed
    /// interrupt fact the session observes — not a silent
    /// `tracing`-and-discard. The transport has no provider response id for a
    /// locally-detected barge-in, so `response_id` is `None` (the same shape
    /// the user-facing `live/interrupt` RPC uses).
    pub async fn signal_transport_barge_in(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<(), LiveAdapterHostError> {
        let session_id = self.channel_session(channel_id).await?;
        self.projection_sink
            .signal_turn_interrupt(&session_id, None)
            .await?;
        Ok(())
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

    fn observation_requires_generated_close(observation: &LiveAdapterObservation) -> bool {
        match observation {
            LiveAdapterObservation::Error { .. } => true,
            LiveAdapterObservation::StatusChanged { status } => status.is_terminal(),
            _ => false,
        }
    }

    /// Read-only transport projection of generated close cleanup.
    ///
    /// `Closed` can be written only by [`Self::commit_channel_close_observation`],
    /// which consumes a generated close commit handoff. Transports use this to
    /// avoid requesting a second close decision when a runtime path has already
    /// committed terminality before the staged terminal observation is drained.
    pub(crate) async fn generated_close_has_committed(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<bool, LiveAdapterHostError> {
        self.channel_status(channel_id)
            .await
            .map(|status| status.is_terminal())
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
        // Transport-resource view: exclude channels in their post-close TTL
        // retention window. Retained-closed entries remain in `channels` so
        // the reap path (G1) and post-close status reads (G42) keep working,
        // but they no longer have an adapter handoff target. Public
        // lifecycle/admission callers must still require generated live-open
        // authority; this list is only the host's currently handoff-capable
        // transport cache.
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session.clone())
            .await
            .unwrap();
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
        let ch2 = noop_host
            .open_channel_with_generated_test_machine_authority(session2)
            .await
            .unwrap();
        let outcome2 = noop_host.apply_observation(&ch2, &obs).await.unwrap();
        assert!(matches!(outcome2, ObservationOutcome::TranscriptAppended));
    }

    // -- Channel lifecycle --

    #[tokio::test]
    async fn open_channel_returns_unique_ids() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let s1 = test_session_id();
        let s2 = test_session_id();
        let ch1 = host
            .open_channel_with_generated_test_machine_authority(s1)
            .await
            .unwrap();
        let ch2 = host
            .open_channel_with_generated_test_machine_authority(s2)
            .await
            .unwrap();
        assert_ne!(ch1, ch2);
    }

    #[tokio::test]
    async fn open_channel_ids_are_uuid_shape_not_live_n() {
        // G41 regression: the legacy `live_{N}` shape was process-monotonic
        // and collided across `rkat-rpc` restarts and across co-tenant host
        // instances. Channel ids must now be v4 UUIDs.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch1 = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let ch2 = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();

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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Opening);
    }

    #[tokio::test]
    async fn channel_status_observation_advances_per_channel() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();

        let first = host.channel_status_observation(&ch).await.unwrap();
        let second = host.channel_status_observation(&ch).await.unwrap();

        assert_eq!(first.channel_id(), ch.as_str());
        assert_eq!(first.status(), &LiveAdapterStatus::Opening);
        assert_eq!(first.observation_sequence(), 1);
        assert_eq!(second.observation_sequence(), 2);
    }

    #[tokio::test]
    async fn duplicate_session_binding_rejected() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();
        let _ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        let err = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, LiveAdapterHostError::SessionAlreadyBound(id) if id == session_id));
    }

    #[tokio::test]
    async fn close_channel_marks_closed_and_retains_for_status_reads() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.close_channel_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        // G42: post-close status is `Closed`, not `ChannelNotFound`.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Closed);
    }

    #[tokio::test]
    async fn close_channel_observation_advances_per_channel() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();

        let first = host
            .close_channel_observed_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        let second = host
            .close_channel_observed_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();

        assert_eq!(first.channel_id(), ch.as_str());
        assert_eq!(first.close_sequence(), 1);
        assert_eq!(second.close_sequence(), 2);
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Closed
        );
    }

    #[tokio::test]
    async fn close_channel_allows_rebinding_same_session() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let session_id = test_session_id();
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.close_channel_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        let ch2 = host
            .open_channel_with_generated_test_machine_authority(session_id)
            .await
            .unwrap();
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

        let ch_a = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.close_channel_with_generated_test_machine_authority(&ch_a)
            .await
            .unwrap();
        let ch_b = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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
        let err = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap_err();
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

        let live = host
            .open_channel_with_generated_test_machine_authority(s1)
            .await
            .unwrap();
        let closing = host
            .open_channel_with_generated_test_machine_authority(s2)
            .await
            .unwrap();

        // Both freshly opened — both should be active.
        let active_pre = host.active_channels().await;
        assert!(active_pre.contains(&live));
        assert!(active_pre.contains(&closing));
        assert_eq!(active_pre.len(), 2);

        // Close one. It enters the TTL retention window
        // (`retire_at = Some(now + CLOSED_CHANNEL_TTL)`); the reaper
        // does NOT drop it yet.
        host.close_channel_with_generated_test_machine_authority(&closing)
            .await
            .unwrap();

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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        assert_eq!(host.channel_session(&ch).await.unwrap(), session_id);
    }

    /// CC1: `signal_terminal_error` must enqueue a synthetic `Error`
    /// observation and close the underlying adapter. The pending obs must
    /// be readable through `next_observation_raw` even after the channel
    /// has transitioned to `Closed` (the slot lives on `ChannelState`, not
    /// on the adapter), so transports can observe it after generated close
    /// authority accepts the channel close.
    #[tokio::test]
    async fn signal_terminal_error_enqueues_synthetic_error_obs_and_closes_channel() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
    /// the same `ConfigRejected` code after generated close authority has
    /// accepted the channel close.
    #[tokio::test]
    async fn synthetic_terminal_error_routes_through_apply_observation_to_terminal_outcome() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
    async fn commit_status_with_generated_authority_changes_channel_status() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Ready
        );
    }

    #[tokio::test]
    async fn terminal_status_update_requires_generated_close_authority() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let err = host
            .commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Closed)
            .await
            .expect_err("terminal status update must not bypass generated close authority");
        assert!(matches!(err, LiveAdapterHostError::StatusNotAuthorized));

        let obs = LiveAdapterObservation::StatusChanged {
            status: LiveAdapterStatus::Closed,
        };
        let err = host
            .apply_observation(&ch, &obs)
            .await
            .expect_err("closed observation must not bypass generated close authority");
        assert!(matches!(err, LiveAdapterHostError::CloseNotAuthorized));

        host.close_channel_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert_eq!(
            outcome,
            ObservationOutcome::StatusUpdated(LiveAdapterStatus::Closed)
        );
    }

    // -- Snapshot versioning --

    #[tokio::test]
    async fn snapshot_version_increments_monotonically() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let v1 = host.next_snapshot_version(&ch).await.unwrap();
        let v2 = host.next_snapshot_version(&ch).await.unwrap();
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
    }

    // -- Active channels --

    #[tokio::test]
    async fn active_channels_lists_open_channels() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch1 = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let ch2 = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let active = host.active_channels().await;
        assert_eq!(active.len(), 2);
        assert!(active.contains(&ch1));
        assert!(active.contains(&ch2));
    }

    // -- Adapter attachment --

    #[tokio::test]
    async fn send_input_without_adapter_returns_error() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        let err = host
            .send_input(&ch, LiveInputChunk::Text { text: "hi".into() })
            .await
            .unwrap_err();
        // F31: with no adapter, status is still `Opening`; rejection is the
        // not-ready guard. (If status were `Ready`, we'd hit `NoAdapter`.)
        assert!(matches!(err, LiveAdapterHostError::ChannelNotReady(_, _)));
    }

    #[tokio::test]
    async fn send_command_rejects_refresh_without_typed_acceptance_path() {
        let session_id = test_session_id();
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        let snapshot = meerkat_core::live_adapter::LiveProjectionSnapshot {
            session_id,
            snapshot_version: 1,
            seed_messages: vec![],
            visible_tools: vec![],
            system_prompt: None,
            model_id: "model-a".into(),
            provider_id: meerkat_core::Provider::Other,
            audio_config: None,
            runtime_system_context: vec![],
        };

        let err = host
            .send_command(&ch, LiveAdapterCommand::Refresh { snapshot })
            .await
            .unwrap_err();

        assert!(matches!(err, LiveAdapterHostError::UnsupportedCommand(_)));
    }

    #[tokio::test]
    async fn attach_adapter_does_not_assert_ready() {
        // F32: attach leaves status Opening; only `Ready` observation
        // promotes the channel.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        host.send_input(&ch, LiveInputChunk::Text { text: "hi".into() })
            .await
            .unwrap();
    }

    // -- E29: typed adapter error --

    #[tokio::test]
    async fn adapter_pump_error_routes_close_through_generated_authority() {
        // Adapter pump failures surface a synthetic terminal observation.
        // The host does not retire the channel until the transport routes
        // that terminal observation through generated close authority.
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::new(ErroringAdapter))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();
        let obs = host
            .next_observation_raw(&ch)
            .await
            .unwrap()
            .expect("synthetic Error obs surfaces on adapter Err");
        match &obs {
            LiveAdapterObservation::Error { code, message } => {
                assert_eq!(*code, LiveAdapterErrorCode::ProviderError);
                assert!(
                    message.contains("adapter read failure"),
                    "synthetic message must explain origin; got `{message}`"
                );
            }
            other => unreachable!("expected synthetic Error, got {other:?}"),
        }
        let err = host
            .apply_observation(&ch, &obs)
            .await
            .expect_err("terminal observation must wait for generated close authority");
        assert!(matches!(err, LiveAdapterHostError::CloseNotAuthorized));

        // Terminal observation routing waits for generated close authority.
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Ready);
        {
            let inner = host.inner.lock().await;
            let channel = inner
                .channels
                .get(&ch)
                .expect("channel remains present before close authority");
            assert!(
                channel.retire_at.is_none(),
                "adapter Err must not set retire_at before generated close authority"
            );
            assert!(
                channel.adapter.is_some(),
                "adapter Err must not drop the adapter before generated close authority"
            );
        }

        host.close_channel_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        assert_eq!(
            outcome,
            ObservationOutcome::Terminal {
                code: LiveAdapterErrorCode::ProviderError
            }
        );
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
                "generated close authority must set retire_at"
            );
            assert!(
                channel.adapter.is_none(),
                "generated close authority must drop the adapter Arc"
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch1 = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        host.attach_adapter(&ch1, Arc::new(ErroringAdapter))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch1, LiveAdapterStatus::Ready)
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
            .open_channel_with_generated_test_machine_authority(session_id.clone())
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();

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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();

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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        // Status must accept commands so the adapter command path is exercised
        // (otherwise `send_command` rejects with ChannelNotReady — but the
        // helper used for SubmitToolError uses `require_ready=false` since the
        // command is allowed when not Ready; verifying via the generated status
        // commit helper anyway to mirror the production attach sequence).
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        // D281 gate: the adapter-facing payload is exactly the typed
        // `LiveToolDispatchTimeout` fact's `Display` projection — not an
        // ad-hoc fabricated string. This pins that the typed fact owns the
        // truth and the only stringification is the deterministic seam-edge
        // render.
        assert_eq!(
            errors[0].1,
            LiveToolDispatchTimeout::new(timeout).to_string(),
            "tool dispatch timeout must submit the typed fact's Display projection, \
             not a fabricated string: {}",
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::clone(&adapter) as _)
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
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

    // -- D223: transport-local barge-in lowers into the interrupt seam --

    #[tokio::test]
    async fn transport_barge_in_emits_typed_interrupt_via_sink() {
        // A transport-discarded output-audio queue (local VAD barge-in) must
        // become a typed interrupt fact the session observes — routed through
        // the same `signal_turn_interrupt` seam adapter-observed barge-ins use,
        // not a silent discard. (Fails-old: there was no host seam for a
        // transport-local barge-in, so the discard was log-only.)
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();

        host.signal_transport_barge_in(&ch).await.unwrap();

        let interrupts = sink.interrupts.lock().unwrap();
        assert_eq!(
            interrupts.len(),
            1,
            "barge-in must signal exactly one interrupt"
        );
        assert_eq!(interrupts[0].0, session_id);
        // Transport-local barge-in carries no provider response id (same shape
        // as the user-facing `live/interrupt` RPC).
        assert_eq!(interrupts[0].1, None);
    }

    // -- A10: terminal error projection --

    #[tokio::test]
    async fn terminal_error_observation_signals_sink_without_closing_host_directly() {
        let sink = Arc::new(RecordingProjectionSink::default());
        let host = LiveAdapterHost::new(Arc::clone(&sink) as _);
        let session_id = test_session_id();
        let ch = host
            .open_channel_with_generated_test_machine_authority(session_id.clone())
            .await
            .unwrap();
        let obs = LiveAdapterObservation::Error {
            code: LiveAdapterErrorCode::ConnectionLost,
            message: "ws closed unexpectedly".into(),
        };
        let err = host
            .apply_observation(&ch, &obs)
            .await
            .expect_err("terminal error must wait for generated close authority");
        assert!(matches!(err, LiveAdapterHostError::CloseNotAuthorized));
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Opening
        );
        assert_eq!(sink.terminal_errors.lock().unwrap().len(), 0);

        let close = host
            .close_channel_observed_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        assert_eq!(close.channel_id(), ch.as_str());

        let outcome = host.apply_observation(&ch, &obs).await.unwrap();
        match outcome {
            ObservationOutcome::Terminal {
                code: LiveAdapterErrorCode::ConnectionLost,
            } => {}
            other => panic!("expected Terminal/ConnectionLost, got {other:?}"),
        }
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
        let ch = host
            .open_channel_with_generated_test_machine_authority(s.clone())
            .await
            .unwrap();
        host.close_channel_with_generated_test_machine_authority(&ch)
            .await
            .unwrap();
        // Closed channel still in retention map; rebind must succeed.
        host.open_channel_with_generated_test_machine_authority(s)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn transport_send_input_does_not_mint_command_acceptance_evidence() {
        let host = LiveAdapterHost::new(Arc::new(NoOpProjectionSink));
        let ch = host
            .open_channel_with_generated_test_machine_authority(test_session_id())
            .await
            .unwrap();
        host.attach_adapter(&ch, Arc::new(StubAdapter::new()))
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(&ch, LiveAdapterStatus::Ready)
            .await
            .unwrap();

        host.send_input(
            &ch,
            LiveInputChunk::Text {
                text: "hello".into(),
            },
        )
        .await
        .unwrap();
        {
            let inner = host.inner.lock().await;
            let channel = inner.channels.get(&ch).unwrap();
            assert_eq!(channel.command_acceptance_sequence, 0);
        }

        let acceptance = host
            .send_input_observed(
                &ch,
                LiveInputChunk::Text {
                    text: "hello".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(acceptance.kind(), LiveCommandAcceptanceKind::SendInput);
        assert_eq!(acceptance.acceptance_sequence(), 1);
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

    // ---------------------------------------------------------------------
    // D301: typed SessionError -> LiveProjectionError classification
    // ---------------------------------------------------------------------

    #[test]
    fn session_error_classification_lands_in_distinct_typed_variants() {
        use meerkat_core::SessionError;

        let sid = test_session_id();

        // NotFound -> SessionNotFound (identity-bearing).
        assert!(matches!(
            LiveProjectionError::from_session_error(
                &sid,
                SessionError::NotFound { id: sid.clone() }
            ),
            LiveProjectionError::SessionNotFound(_)
        ));

        // Unsupported -> Rejected, preserving the typed reason payload.
        match LiveProjectionError::from_session_error(
            &sid,
            SessionError::Unsupported("nope".into()),
        ) {
            LiveProjectionError::Rejected(reason) => assert_eq!(reason, "nope"),
            other => panic!("expected Rejected, got {other:?}"),
        }

        // Busy -> SessionBusy (distinct from Internal/Session).
        assert!(matches!(
            LiveProjectionError::from_session_error(&sid, SessionError::Busy { id: sid.clone() }),
            LiveProjectionError::SessionBusy(_)
        ));

        // NotRunning -> SessionNotRunning.
        assert!(matches!(
            LiveProjectionError::from_session_error(
                &sid,
                SessionError::NotRunning { id: sid.clone() }
            ),
            LiveProjectionError::SessionNotRunning(_)
        ));

        // PersistenceDisabled / CompactionDisabled -> CapabilityDisabled,
        // carrying the stable code (no prose-only Internal collapse).
        match LiveProjectionError::from_session_error(&sid, SessionError::PersistenceDisabled) {
            LiveProjectionError::CapabilityDisabled { code, .. } => {
                assert_eq!(code, "SESSION_PERSISTENCE_DISABLED");
            }
            other => panic!("expected CapabilityDisabled, got {other:?}"),
        }
        match LiveProjectionError::from_session_error(&sid, SessionError::CompactionDisabled) {
            LiveProjectionError::CapabilityDisabled { code, .. } => {
                assert_eq!(code, "SESSION_COMPACTION_DISABLED");
            }
            other => panic!("expected CapabilityDisabled, got {other:?}"),
        }

        // Store error -> typed Session { code } carrying the stable code,
        // NOT a prose-only Internal(to_string()).
        let store_err: Box<dyn std::error::Error + Send + Sync> = "disk gone".into();
        match LiveProjectionError::from_session_error(&sid, SessionError::Store(store_err)) {
            LiveProjectionError::Session { code, .. } => {
                assert_eq!(code, "SESSION_STORE_ERROR");
            }
            other => panic!("expected typed Session, got {other:?}"),
        }

        // FailedWithData -> typed Session { code } (still carries the stable
        // code; the prose lives only in `message`).
        match LiveProjectionError::from_session_error(
            &sid,
            SessionError::FailedWithData {
                message: "boom".into(),
                data: serde_json::json!({"k": "v"}),
            },
        ) {
            LiveProjectionError::Session { code, message } => {
                assert_eq!(code, "SESSION_ERROR");
                assert_eq!(message, "boom");
            }
            other => panic!("expected typed Session, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------
    // D199: transcript delta identity fails closed on missing required ids
    // ---------------------------------------------------------------------

    #[test]
    fn delta_identity_requires_response_delta_and_item_ids() {
        // Full identity resolves to a typed triple.
        let full = LiveTranscriptIdentity::assistant_delta(
            Some("item-1"),
            Some("prev-0"),
            Some(2),
            Some("resp-9"),
            Some("delta-3"),
        );
        let resolved = full
            .require_delta_identity()
            .expect("full identity resolves");
        assert_eq!(resolved.response_id, "resp-9");
        assert_eq!(resolved.delta_id, "delta-3");
        assert_eq!(resolved.item_id, "item-1");
        assert_eq!(resolved.previous_item_id, Some("prev-0"));
        assert_eq!(resolved.content_index, Some(2));

        // Missing response_id fails closed with a typed error (NOT an
        // empty-string-coalesced identity).
        let no_resp = LiveTranscriptIdentity::assistant_delta(
            Some("item-1"),
            None,
            Some(0),
            None,
            Some("delta-3"),
        );
        assert_eq!(
            no_resp.require_delta_identity(),
            Err(LiveTranscriptIdentityError::MissingResponseId)
        );

        // Missing delta_id fails closed.
        let no_delta = LiveTranscriptIdentity::assistant_delta(
            Some("item-1"),
            None,
            Some(0),
            Some("resp-9"),
            None,
        );
        assert_eq!(
            no_delta.require_delta_identity(),
            Err(LiveTranscriptIdentityError::MissingDeltaId)
        );

        // Missing item_id fails closed.
        let no_item = LiveTranscriptIdentity::assistant_delta(
            None,
            None,
            Some(0),
            Some("resp-9"),
            Some("delta-3"),
        );
        assert_eq!(
            no_item.require_delta_identity(),
            Err(LiveTranscriptIdentityError::MissingItemId)
        );
    }
}

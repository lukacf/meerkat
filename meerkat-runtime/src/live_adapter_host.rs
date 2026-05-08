//! Live adapter host — runtime-owned orchestrator for live provider sessions.
//!
//! Sits outside MeerkatMachine (not machine state, not a second machine).
//! Uses Meerkat services to build projections and gate observations.
//! Provider transport mechanics stay inside adapter implementations.

use std::collections::HashMap;

use meerkat_core::live_adapter::{LiveAdapterObservation, LiveAdapterStatus};
use meerkat_core::types::SessionId;
use tokio::sync::Mutex;

/// Opaque channel identifier for a live adapter session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiveChannelId(String);

impl LiveChannelId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
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

/// Per-channel state tracked by the host.
#[derive(Debug)]
struct ChannelState {
    session_id: SessionId,
    status: LiveAdapterStatus,
    snapshot_version: u64,
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
    #[error("adapter error: {0}")]
    AdapterError(String),
}

/// Observation routing decision — what the host does with an adapter observation.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationRouting {
    /// Append to canonical session transcript.
    AppendTranscript,
    /// Dispatch a tool call through Meerkat's tool dispatcher.
    DispatchToolCall {
        provider_call_id: String,
        tool_name: String,
    },
    /// Signal an interrupt to the session's current turn.
    SignalInterrupt,
    /// Update channel status tracking.
    UpdateStatus(LiveAdapterStatus),
    /// Terminal error — channel should be closed.
    TerminalError,
    /// No routing action needed (status update only, informational).
    Noop,
}

/// Runtime-owned host for live provider adapter sessions.
///
/// This is NOT MeerkatMachine state (dogma: adapter must not become a
/// second machine). It's a runtime orchestrator that:
/// - Owns the map of active adapter channels
/// - Builds projection snapshots from canonical session state
/// - Routes adapter observations to the right Meerkat API
/// - Exposes transport bootstrap info for the surface API
pub struct LiveAdapterHost {
    channels: Mutex<HashMap<LiveChannelId, ChannelState>>,
    next_channel_id: std::sync::atomic::AtomicU64,
}

impl LiveAdapterHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
            next_channel_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Open a new live channel for a session.
    ///
    /// Returns the channel ID and initial snapshot version. The caller
    /// must separately build the actual provider adapter session using
    /// the snapshot.
    pub async fn open_channel(
        &self,
        session_id: SessionId,
    ) -> Result<LiveChannelId, LiveAdapterHostError> {
        let mut channels = self.channels.lock().await;

        if channels.values().any(|ch| ch.session_id == session_id) {
            return Err(LiveAdapterHostError::SessionAlreadyBound(session_id));
        }

        let id = self
            .next_channel_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let channel_id = LiveChannelId::new(format!("live_{id}"));

        channels.insert(
            channel_id.clone(),
            ChannelState {
                session_id,
                status: LiveAdapterStatus::Opening,
                snapshot_version: 0,
            },
        );

        Ok(channel_id)
    }

    /// Close a live channel, removing it from the host.
    pub async fn close_channel(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<(), LiveAdapterHostError> {
        let mut channels = self.channels.lock().await;
        if channels.remove(channel_id).is_none() {
            return Err(LiveAdapterHostError::ChannelNotFound(channel_id.clone()));
        }
        Ok(())
    }

    /// Get the current status of a channel.
    pub async fn channel_status(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<LiveAdapterStatus, LiveAdapterHostError> {
        let channels = self.channels.lock().await;
        channels
            .get(channel_id)
            .map(|ch| ch.status.clone())
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    /// Get the session ID bound to a channel.
    pub async fn channel_session(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<SessionId, LiveAdapterHostError> {
        let channels = self.channels.lock().await;
        channels
            .get(channel_id)
            .map(|ch| ch.session_id.clone())
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))
    }

    /// Classify an adapter observation into a routing decision.
    ///
    /// The host does NOT execute the routing — it classifies what
    /// should happen and returns the decision. The caller (surface
    /// layer) executes it through the appropriate Meerkat API.
    /// This keeps the host as a classifier, not an authority.
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
            LiveAdapterObservation::AssistantAudioChunk { .. } => ObservationRouting::Noop,
            LiveAdapterObservation::AssistantTranscriptFinal { .. } => {
                ObservationRouting::AppendTranscript
            }
            LiveAdapterObservation::AssistantTranscriptTruncated { .. } => {
                ObservationRouting::AppendTranscript
            }
            LiveAdapterObservation::ToolCallRequested {
                provider_call_id,
                tool_name,
                ..
            } => ObservationRouting::DispatchToolCall {
                provider_call_id: provider_call_id.clone(),
                tool_name: tool_name.clone(),
            },
            LiveAdapterObservation::TurnInterrupted => ObservationRouting::SignalInterrupt,
            LiveAdapterObservation::TurnCompleted { .. } => ObservationRouting::AppendTranscript,
            LiveAdapterObservation::StatusChanged { status } => {
                ObservationRouting::UpdateStatus(status.clone())
            }
            LiveAdapterObservation::Error { .. } => ObservationRouting::TerminalError,
            _ => ObservationRouting::Noop,
        }
    }

    /// Apply a status update from an observation to the channel's tracked state.
    pub async fn apply_status_update(
        &self,
        channel_id: &LiveChannelId,
        status: LiveAdapterStatus,
    ) -> Result<(), LiveAdapterHostError> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.status = status;
        Ok(())
    }

    /// Increment and return the next snapshot version for a channel.
    /// Used when rebuilding the provider session after model switch or reconnect.
    pub async fn next_snapshot_version(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<u64, LiveAdapterHostError> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.snapshot_version += 1;
        Ok(channel.snapshot_version)
    }

    /// List all active channel IDs.
    pub async fn active_channels(&self) -> Vec<LiveChannelId> {
        let channels = self.channels.lock().await;
        channels.keys().cloned().collect()
    }
}

impl Default for LiveAdapterHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::live_adapter::{LiveAdapterErrorCode, LiveAdapterObservation};
    use meerkat_core::types::{StopReason, Usage};

    fn test_session_id() -> SessionId {
        SessionId::new()
    }

    // -- Channel lifecycle --

    #[tokio::test]
    async fn open_channel_returns_unique_ids() {
        let host = LiveAdapterHost::new();
        let s1 = test_session_id();
        let s2 = test_session_id();
        let ch1 = host.open_channel(s1).await.unwrap();
        let ch2 = host.open_channel(s2).await.unwrap();
        assert_ne!(ch1, ch2);
    }

    #[tokio::test]
    async fn open_channel_starts_in_opening_status() {
        let host = LiveAdapterHost::new();
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let status = host.channel_status(&ch).await.unwrap();
        assert_eq!(status, LiveAdapterStatus::Opening);
    }

    #[tokio::test]
    async fn duplicate_session_binding_rejected() {
        let host = LiveAdapterHost::new();
        let session_id = test_session_id();
        let _ch = host.open_channel(session_id.clone()).await.unwrap();
        let err = host.open_channel(session_id.clone()).await.unwrap_err();
        assert!(matches!(err, LiveAdapterHostError::SessionAlreadyBound(id) if id == session_id));
    }

    #[tokio::test]
    async fn close_channel_removes_it() {
        let host = LiveAdapterHost::new();
        let ch = host.open_channel(test_session_id()).await.unwrap();
        host.close_channel(&ch).await.unwrap();
        let err = host.channel_status(&ch).await.unwrap_err();
        assert!(matches!(err, LiveAdapterHostError::ChannelNotFound(_)));
    }

    #[tokio::test]
    async fn close_channel_allows_rebinding_same_session() {
        let host = LiveAdapterHost::new();
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        host.close_channel(&ch).await.unwrap();
        let ch2 = host.open_channel(session_id).await.unwrap();
        assert_ne!(ch, ch2);
    }

    #[tokio::test]
    async fn channel_session_returns_bound_session() {
        let host = LiveAdapterHost::new();
        let session_id = test_session_id();
        let ch = host.open_channel(session_id.clone()).await.unwrap();
        assert_eq!(host.channel_session(&ch).await.unwrap(), session_id);
    }

    // -- Observation classification (dogma: host classifies, doesn't execute) --

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
            LiveAdapterHost::classify_observation(&LiveAdapterObservation::TurnInterrupted);
        assert_eq!(routing, ObservationRouting::SignalInterrupt);
    }

    #[test]
    fn user_transcript_routes_to_append() {
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: "item_1".into(),
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
            provider_item_id: "item_2".into(),
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
                reason: "throttled".into(),
            },
        };
        assert_eq!(
            LiveAdapterHost::classify_observation(&obs),
            ObservationRouting::UpdateStatus(LiveAdapterStatus::Degraded {
                reason: "throttled".into(),
            })
        );
    }

    // -- Status tracking --

    #[tokio::test]
    async fn apply_status_update_changes_channel_status() {
        let host = LiveAdapterHost::new();
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
        let host = LiveAdapterHost::new();
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let v1 = host.next_snapshot_version(&ch).await.unwrap();
        let v2 = host.next_snapshot_version(&ch).await.unwrap();
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
    }

    // -- Active channels --

    #[tokio::test]
    async fn active_channels_lists_open_channels() {
        let host = LiveAdapterHost::new();
        let ch1 = host.open_channel(test_session_id()).await.unwrap();
        let ch2 = host.open_channel(test_session_id()).await.unwrap();
        let active = host.active_channels().await;
        assert_eq!(active.len(), 2);
        assert!(active.contains(&ch1));
        assert!(active.contains(&ch2));
    }
}

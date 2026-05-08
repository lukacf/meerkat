//! Live adapter host — runtime-owned orchestrator for live provider sessions.
//!
//! Sits outside MeerkatMachine (not machine state, not a second machine).
//! Uses Meerkat services to build projections and gate observations.
//! Provider transport mechanics stay inside adapter implementations.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterObservation, LiveAdapterStatus, LiveInputChunk,
};
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
struct ChannelState {
    session_id: SessionId,
    status: LiveAdapterStatus,
    snapshot_version: u64,
    adapter: Option<Arc<Mutex<Box<dyn LiveAdapter>>>>,
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
    #[error("adapter error: {0}")]
    AdapterError(String),
}

/// Observation routing decision — what the host does with an adapter observation.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationRouting {
    AppendTranscript,
    DispatchToolCall {
        provider_call_id: String,
        tool_name: String,
    },
    SignalInterrupt,
    UpdateStatus(LiveAdapterStatus),
    TerminalError,
    Noop,
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
                adapter: None,
            },
        );

        Ok(channel_id)
    }

    /// Attach a live adapter to an open channel.
    pub async fn attach_adapter(
        &self,
        channel_id: &LiveChannelId,
        adapter: Box<dyn LiveAdapter>,
    ) -> Result<(), LiveAdapterHostError> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(channel_id)
            .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
        channel.adapter = Some(Arc::new(Mutex::new(adapter)));
        channel.status = LiveAdapterStatus::Ready;
        Ok(())
    }

    /// Send a command to the adapter on a channel.
    pub async fn send_command(
        &self,
        channel_id: &LiveChannelId,
        command: LiveAdapterCommand,
    ) -> Result<(), LiveAdapterHostError> {
        let adapter = {
            let channels = self.channels.lock().await;
            let channel = channels
                .get(channel_id)
                .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
            Arc::clone(
                channel
                    .adapter
                    .as_ref()
                    .ok_or_else(|| LiveAdapterHostError::NoAdapter(channel_id.clone()))?,
            )
        };
        let mut adapter = adapter.lock().await;
        adapter
            .send_command(command)
            .await
            .map_err(|e| LiveAdapterHostError::AdapterError(e.to_string()))
    }

    /// Send an input chunk to the adapter on a channel.
    pub async fn send_input(
        &self,
        channel_id: &LiveChannelId,
        chunk: LiveInputChunk,
    ) -> Result<(), LiveAdapterHostError> {
        self.send_command(channel_id, LiveAdapterCommand::SendInput { chunk })
            .await
    }

    /// Poll the next observation from the adapter on a channel.
    pub async fn next_observation(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<Option<LiveAdapterObservation>, LiveAdapterHostError> {
        let adapter = {
            let channels = self.channels.lock().await;
            let channel = channels
                .get(channel_id)
                .ok_or_else(|| LiveAdapterHostError::ChannelNotFound(channel_id.clone()))?;
            Arc::clone(
                channel
                    .adapter
                    .as_ref()
                    .ok_or_else(|| LiveAdapterHostError::NoAdapter(channel_id.clone()))?,
            )
        };
        let obs = {
            let mut adapter = adapter.lock().await;
            adapter
                .next_observation()
                .await
                .map_err(|e| LiveAdapterHostError::AdapterError(e.to_string()))?
        };

        if let Some(ref obs) = obs {
            let routing = Self::classify_observation(obs);
            if let ObservationRouting::UpdateStatus(ref status) = routing {
                let mut channels = self.channels.lock().await;
                if let Some(channel) = channels.get_mut(channel_id) {
                    channel.status = status.clone();
                }
            }
        }

        Ok(obs)
    }

    pub async fn close_channel(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<(), LiveAdapterHostError> {
        let adapter = {
            let mut channels = self.channels.lock().await;
            if let Some(channel) = channels.remove(channel_id) {
                channel.adapter
            } else {
                return Err(LiveAdapterHostError::ChannelNotFound(channel_id.clone()));
            }
        };
        if let Some(adapter) = adapter {
            let mut adapter = adapter.lock().await;
            let _ = adapter.close().await;
        }
        Ok(())
    }

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
    use meerkat_core::live_adapter::{
        LiveAdapterError, LiveAdapterErrorCode, LiveAdapterObservation,
    };
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

    // -- Adapter attachment --

    #[tokio::test]
    async fn send_input_without_adapter_returns_error() {
        let host = LiveAdapterHost::new();
        let ch = host.open_channel(test_session_id()).await.unwrap();
        let err = host
            .send_input(&ch, LiveInputChunk::Text { text: "hi".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, LiveAdapterHostError::NoAdapter(_)));
    }

    #[tokio::test]
    async fn attach_adapter_sets_status_to_ready() {
        let host = LiveAdapterHost::new();
        let ch = host.open_channel(test_session_id()).await.unwrap();
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Opening
        );
        host.attach_adapter(&ch, Box::new(StubAdapter::new()))
            .await
            .unwrap();
        assert_eq!(
            host.channel_status(&ch).await.unwrap(),
            LiveAdapterStatus::Ready
        );
    }

    struct StubAdapter {
        status: LiveAdapterStatus,
    }

    impl StubAdapter {
        fn new() -> Self {
            Self {
                status: LiveAdapterStatus::Ready,
            }
        }
    }

    #[async_trait::async_trait]
    impl LiveAdapter for StubAdapter {
        async fn send_command(
            &mut self,
            _command: LiveAdapterCommand,
        ) -> Result<(), LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &mut self,
        ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
            Ok(None)
        }

        fn status(&self) -> &LiveAdapterStatus {
            &self.status
        }

        async fn close(&mut self) -> Result<(), LiveAdapterError> {
            self.status = LiveAdapterStatus::Closed;
            Ok(())
        }
    }
}

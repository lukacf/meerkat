//! Surface implementation of [`LiveProjectionSink`].
//!
//! Bridges the runtime-side projection contract (A1-A6, A14) into canonical
//! Meerkat semantic state via [`SessionRuntime`]. The runtime crate defines
//! the trait shape; this module holds the only production implementation.
//!
//! Each sink method maps to an existing `SessionRuntime` seam:
//! - User transcripts -> `append_external_user_content`
//! - Assistant deltas -> `append_realtime_transcript_event` (preserves identity)
//! - Assistant finals -> `append_external_assistant_output`
//! - Truncation -> `append_realtime_transcript_event` (AssistantTranscriptTruncated)
//! - Turn-interrupt -> `interrupt_live_with_machine_authority`
//! - Turn-completed -> no-op (the assistant final already committed history;
//!   `RealtimeTranscriptEvent::AssistantTurnCompleted` is folded into the final
//!   path by the host already)
//! - Terminal error -> tracing log + best-effort channel termination
//!
//! Identity-fidelity note: the host trait now passes `LiveTranscriptIdentity`
//! end-to-end (closes A11 at the trait layer). Assistant deltas can populate
//! `RealtimeTranscriptEvent::AssistantTextDelta` with the real
//! `response_id` / `delta_id` / `previous_item_id` / `content_index` rather
//! than synthesizing empty defaults.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::RealtimeTranscriptEvent;
use meerkat_core::live_adapter::LiveAdapterErrorCode;
use meerkat_core::types::{AssistantBlock, ContentInput, SessionId, StopReason, Usage};
use meerkat_runtime::live_adapter_host::{
    LiveProjectionError, LiveProjectionSink, LiveTranscriptIdentity,
};

use crate::session_runtime::SessionRuntime;

/// Bridges [`LiveProjectionSink`] callbacks into [`SessionRuntime`].
pub struct SessionServiceProjectionSink {
    runtime: Arc<SessionRuntime>,
}

impl SessionServiceProjectionSink {
    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        Self { runtime }
    }
}

fn session_error_to_projection(
    err: meerkat_core::SessionError,
    id: &SessionId,
) -> LiveProjectionError {
    match err {
        meerkat_core::SessionError::NotFound { .. } => {
            LiveProjectionError::SessionNotFound(id.clone())
        }
        meerkat_core::SessionError::Unsupported(reason) => LiveProjectionError::Rejected(reason),
        other => LiveProjectionError::Internal(other.to_string()),
    }
}

#[async_trait]
impl LiveProjectionSink for SessionServiceProjectionSink {
    async fn append_user_transcript(
        &self,
        session_id: &SessionId,
        text: &str,
        _identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        // A2: user transcripts land as canonical user input via the existing
        // external-append seam. Identity fields are not consumed here because
        // the user-content append path commits directly into canonical
        // history; ordering/staging fidelity matters for assistant deltas
        // (where the runtime's realtime-transcript layer reorders by item id),
        // not for finalized user input.
        self.runtime
            .append_external_user_content(session_id, ContentInput::Text(text.to_string()))
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn append_assistant_delta(
        &self,
        session_id: &SessionId,
        delta: &str,
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        // A3 (deltas): route through the typed realtime-transcript seam so the
        // session's idempotent ordering / staging logic owns delta application.
        // A11: response_id / delta_id / previous_item_id / content_index are
        // now plumbed end-to-end via `LiveTranscriptIdentity`.
        let event = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: identity.response_id.unwrap_or_default().to_string(),
            delta_id: identity.delta_id.unwrap_or_default().to_string(),
            item_id: identity.provider_item_id.unwrap_or_default().to_string(),
            previous_item_id: identity.previous_item_id.map(|s| s.to_string()),
            content_index: identity.content_index.unwrap_or(0),
            delta: delta.to_string(),
        };
        self.runtime
            .append_realtime_transcript_event(session_id, event)
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn append_assistant_final(
        &self,
        session_id: &SessionId,
        text: &str,
        _identity: LiveTranscriptIdentity<'_>,
        stop_reason: StopReason,
        usage: Usage,
    ) -> Result<(), LiveProjectionError> {
        // A3 (finals): commit a single assistant text block as canonical output.
        // Identity is informational here — the canonical assistant-output
        // append already represents the final block; provider item identity
        // is recorded upstream in the realtime-transcript seam from the
        // matching delta/final stream.
        let blocks = vec![AssistantBlock::Text {
            text: text.to_string(),
            meta: None,
        }];
        self.runtime
            .append_external_assistant_output(session_id, blocks, stop_reason, usage)
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn truncate_assistant_transcript(
        &self,
        session_id: &SessionId,
        provider_item_id: Option<&str>,
        text: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // Barge-in projection: tell the session's realtime transcript layer to
        // truncate its staged assistant content for this item.
        let event = RealtimeTranscriptEvent::AssistantTranscriptTruncated {
            response_id: String::new(),
            item_id: provider_item_id.unwrap_or_default().to_string(),
            content_index: 0,
            text: text.unwrap_or_default().to_string(),
        };
        self.runtime
            .append_realtime_transcript_event(session_id, event)
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
    }

    async fn signal_turn_interrupt(
        &self,
        session_id: &SessionId,
    ) -> Result<(), LiveProjectionError> {
        // A6: barge-in projects through the same machine-authority interrupt
        // path the user-facing `live/interrupt` RPC uses. Tolerate
        // `NotRunning` (typical when no turn is currently in flight) so that a
        // late provider-side interrupt does not poison the channel.
        match self
            .runtime
            .interrupt_live_with_machine_authority(session_id)
            .await
        {
            Ok(()) => Ok(()),
            Err(meerkat_core::SessionError::NotRunning { .. }) => Ok(()),
            Err(err) => Err(session_error_to_projection(err, session_id)),
        }
    }

    async fn signal_turn_completed(
        &self,
        _session_id: &SessionId,
        _stop_reason: StopReason,
        _usage: Usage,
    ) -> Result<(), LiveProjectionError> {
        // The host invokes this after `TurnCompleted`. The assistant final
        // path is already responsible for committing canonical blocks; we
        // intentionally treat the turn-completed signal as a marker the
        // session's transcript layer already absorbs via `AssistantTurnCompleted`
        // events emitted alongside the final. No further surface action is
        // required; making this a sink no-op keeps the seam idempotent.
        Ok(())
    }

    async fn signal_terminal_error(
        &self,
        session_id: &SessionId,
        code: LiveAdapterErrorCode,
        message: &str,
    ) -> Result<(), LiveProjectionError> {
        // Surface a tracing event so operators can correlate. The host has
        // already terminalized the channel state (status=Closed + reap timer);
        // session-level lifecycle remains in caller hands. A future enhancement
        // could route this to the runtime's session-event stream once a
        // canonical terminal-error seam exists on `SessionService`.
        tracing::warn!(
            target: "meerkat_rpc::live_projection",
            session_id = %session_id,
            ?code,
            message,
            "live adapter terminal error",
        );
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use meerkat_core::live_adapter::{
        LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterObservation,
        LiveAdapterStatus,
    };
    use meerkat_core::ops::ToolDispatchOutcome;
    use meerkat_core::types::{ContentBlock, ToolCallView, ToolResult};
    use meerkat_core::{AgentToolDispatcher, ToolDef};
    use meerkat_runtime::live_adapter_host::LiveAdapterHost;
    use std::sync::Mutex as StdMutex;

    // ------------------------------------------------------------------
    // Fakes that record sink/dispatcher activity without spinning a real
    // SessionRuntime. The tests below call host.apply_observation(...)
    // directly and assert the projection sink/dispatcher wiring.
    // ------------------------------------------------------------------

    /// Owned mirror of [`LiveTranscriptIdentity`] for capturing past the
    /// trait callsite (the borrowed form does not survive the await).
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

    #[derive(Default)]
    struct RecordingSink {
        user_transcripts: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        assistant_deltas: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        assistant_finals: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        truncations: StdMutex<Vec<SessionId>>,
        interrupts: StdMutex<Vec<SessionId>>,
        completed: StdMutex<Vec<SessionId>>,
        terminals: StdMutex<Vec<(SessionId, LiveAdapterErrorCode)>>,
    }

    #[async_trait]
    impl LiveProjectionSink for RecordingSink {
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
        async fn append_assistant_delta(
            &self,
            session_id: &SessionId,
            delta: &str,
            identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            self.assistant_deltas.lock().unwrap().push((
                session_id.clone(),
                delta.to_string(),
                OwnedIdentity::from_borrowed(identity),
            ));
            Ok(())
        }
        async fn append_assistant_final(
            &self,
            session_id: &SessionId,
            text: &str,
            identity: LiveTranscriptIdentity<'_>,
            _stop_reason: StopReason,
            _usage: Usage,
        ) -> Result<(), LiveProjectionError> {
            self.assistant_finals.lock().unwrap().push((
                session_id.clone(),
                text.to_string(),
                OwnedIdentity::from_borrowed(identity),
            ));
            Ok(())
        }
        async fn truncate_assistant_transcript(
            &self,
            session_id: &SessionId,
            _provider_item_id: Option<&str>,
            _text: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.truncations.lock().unwrap().push(session_id.clone());
            Ok(())
        }
        async fn signal_turn_interrupt(
            &self,
            session_id: &SessionId,
        ) -> Result<(), LiveProjectionError> {
            self.interrupts.lock().unwrap().push(session_id.clone());
            Ok(())
        }
        async fn signal_turn_completed(
            &self,
            session_id: &SessionId,
            _stop_reason: StopReason,
            _usage: Usage,
        ) -> Result<(), LiveProjectionError> {
            self.completed.lock().unwrap().push(session_id.clone());
            Ok(())
        }
        async fn signal_terminal_error(
            &self,
            session_id: &SessionId,
            code: LiveAdapterErrorCode,
            _message: &str,
        ) -> Result<(), LiveProjectionError> {
            self.terminals
                .lock()
                .unwrap()
                .push((session_id.clone(), code));
            Ok(())
        }
    }

    /// Adapter that records SubmitToolResult/SubmitToolError commands and
    /// otherwise is a no-op.
    #[derive(Default)]
    struct RecordingAdapter {
        commands: StdMutex<Vec<LiveAdapterCommand>>,
    }

    #[async_trait]
    impl LiveAdapter for RecordingAdapter {
        fn status(&self) -> LiveAdapterStatus {
            LiveAdapterStatus::Ready
        }
        async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
            self.commands.lock().unwrap().push(command);
            Ok(())
        }
        async fn next_observation(
            &self,
        ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
            Ok(None)
        }
        async fn close(&self) -> Result<(), LiveAdapterError> {
            Ok(())
        }
    }

    /// Tool dispatcher that records calls and returns a synthetic text result.
    struct RecordingDispatcher {
        calls: StdMutex<Vec<(String, String)>>,
    }

    impl Default for RecordingDispatcher {
        fn default() -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for RecordingDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }
        async fn dispatch(
            &self,
            view: ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, meerkat_core::ToolError> {
            self.calls
                .lock()
                .unwrap()
                .push((view.id.to_string(), view.name.to_string()));
            Ok(ToolDispatchOutcome::sync_result(ToolResult {
                tool_use_id: view.id.to_string(),
                is_error: false,
                content: ContentBlock::text_vec("ok".to_string()),
            }))
        }
    }

    fn test_session_id() -> SessionId {
        SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap()
    }

    #[tokio::test]
    async fn user_transcript_final_records_user_message() {
        let sink: Arc<RecordingSink> = Arc::new(RecordingSink::default());
        let host = LiveAdapterHost::new()
            .with_projection_sink(Arc::clone(&sink) as Arc<dyn LiveProjectionSink>);
        let session_id = test_session_id();
        let channel = host.open_channel(session_id.clone()).await.unwrap();

        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item_1".to_string()),
            previous_item_id: Some("item_0".to_string()),
            content_index: Some(2),
            text: "hello".to_string(),
        };

        host.apply_observation(&channel, &obs).await.unwrap();

        let recorded = sink.user_transcripts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, session_id);
        assert_eq!(recorded[0].1, "hello");
        // A11: the user-side identity tuple flows end-to-end through
        // `LiveTranscriptIdentity` and is no longer dropped at the trait layer.
        let identity = &recorded[0].2;
        assert_eq!(identity.provider_item_id.as_deref(), Some("item_1"));
        assert_eq!(identity.previous_item_id.as_deref(), Some("item_0"));
        assert_eq!(identity.content_index, Some(2));
    }

    #[tokio::test]
    async fn assistant_delta_records_delta_append_with_full_identity() {
        let sink: Arc<RecordingSink> = Arc::new(RecordingSink::default());
        let host = LiveAdapterHost::new()
            .with_projection_sink(Arc::clone(&sink) as Arc<dyn LiveProjectionSink>);
        let session_id = test_session_id();
        let channel = host.open_channel(session_id.clone()).await.unwrap();

        let obs = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: Some("item_2".to_string()),
            previous_item_id: Some("item_1".to_string()),
            content_index: Some(0),
            response_id: Some("resp_7".to_string()),
            delta_id: Some("delta_3".to_string()),
            delta: "partial".to_string(),
        };

        host.apply_observation(&channel, &obs).await.unwrap();

        let recorded = sink.assistant_deltas.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1, "partial");
        // A11: the assistant-delta identity tuple — including response_id and
        // delta_id — must flow end-to-end through `LiveTranscriptIdentity`
        // rather than being synthesized as empty strings.
        let identity = &recorded[0].2;
        assert_eq!(identity.provider_item_id.as_deref(), Some("item_2"));
        assert_eq!(identity.previous_item_id.as_deref(), Some("item_1"));
        assert_eq!(identity.content_index, Some(0));
        assert_eq!(identity.response_id.as_deref(), Some("resp_7"));
        assert_eq!(identity.delta_id.as_deref(), Some("delta_3"));
    }

    #[tokio::test]
    async fn tool_call_dispatches_through_dispatcher() {
        let sink: Arc<RecordingSink> = Arc::new(RecordingSink::default());
        let dispatcher: Arc<RecordingDispatcher> = Arc::new(RecordingDispatcher::default());
        let adapter: Arc<RecordingAdapter> = Arc::new(RecordingAdapter::default());
        let host = LiveAdapterHost::new()
            .with_projection_sink(Arc::clone(&sink) as Arc<dyn LiveProjectionSink>)
            .with_tool_dispatcher(Arc::clone(&dispatcher) as Arc<dyn AgentToolDispatcher>);
        let session_id = test_session_id();
        let channel = host.open_channel(session_id.clone()).await.unwrap();
        host.attach_adapter(&channel, Arc::clone(&adapter) as Arc<dyn LiveAdapter>)
            .await
            .unwrap();

        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_42".to_string(),
            tool_name: "echo".to_string(),
            arguments: serde_json::json!({"value": 1}),
        };

        host.apply_observation(&channel, &obs).await.unwrap();

        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_42");
        assert_eq!(calls[0].1, "echo");

        // A5: result must be submitted back to the adapter as
        // `SubmitToolResult` so the provider gets the answer.
        let cmds = adapter.commands.lock().unwrap();
        assert!(matches!(
            cmds.first(),
            Some(LiveAdapterCommand::SubmitToolResult { .. })
        ));
    }

    #[tokio::test]
    async fn turn_interrupted_signals_interrupt() {
        let sink: Arc<RecordingSink> = Arc::new(RecordingSink::default());
        let host = LiveAdapterHost::new()
            .with_projection_sink(Arc::clone(&sink) as Arc<dyn LiveProjectionSink>);
        let session_id = test_session_id();
        let channel = host.open_channel(session_id.clone()).await.unwrap();

        host.apply_observation(&channel, &LiveAdapterObservation::TurnInterrupted)
            .await
            .unwrap();

        let recorded = sink.interrupts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0], session_id);
    }
}

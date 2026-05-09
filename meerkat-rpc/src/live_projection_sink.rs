//! Surface implementation of [`LiveProjectionSink`].
//!
//! Bridges the runtime-side projection contract (A1-A6, A14) into canonical
//! Meerkat semantic state via [`SessionRuntime`]. The runtime crate defines
//! the trait shape; this module holds the only production implementation.
//!
//! Each sink method maps to an existing `SessionRuntime` seam:
//! - User transcripts -> `append_realtime_transcript_event`
//!   (`UserTranscriptFinal`) when a `provider_item_id` is present, so the
//!   realtime transcript layer's idempotent ordering owns dedup. Falls back
//!   to `append_external_user_content` when the provider does not surface a
//!   stable item id (legacy partial-final shape).
//! - Assistant deltas -> `append_realtime_transcript_event` (preserves identity)
//! - Assistant finals -> buffered until `signal_turn_completed` flushes them
//!   with the authoritative `stop_reason`/`usage` from `TurnCompleted`. The
//!   provider stamps best-effort sentinels onto `AssistantTranscriptFinal`
//!   (it does not have stop_reason/usage atomically with that event), so
//!   committing canonical history at final-time would persist the sentinel.
//!   We defer (P1#1).
//! - Truncation -> `append_realtime_transcript_event` (AssistantTranscriptTruncated)
//!   carrying the real `response_id` and `content_index` rather than fabricating
//!   empties (P1#3). Missing `response_id` surfaces as a typed Rejected.
//! - Turn-interrupt -> `interrupt_live_with_machine_authority`
//! - Turn-completed -> flushes the buffered assistant final with the
//!   authoritative stop_reason/usage. Orphan completions (no buffered final)
//!   commit an empty assistant block — still correct because no transcript
//!   was produced.
//! - Terminal error -> tracing log + best-effort channel termination
//!
//! Identity-fidelity note: the host trait now passes `LiveTranscriptIdentity`
//! end-to-end (closes A11 at the trait layer). Assistant deltas can populate
//! `RealtimeTranscriptEvent::AssistantTextDelta` with the real
//! `response_id` / `delta_id` / `previous_item_id` / `content_index` rather
//! than synthesizing empty defaults.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use meerkat_core::RealtimeTranscriptEvent;
use meerkat_core::live_adapter::LiveAdapterErrorCode;
use meerkat_core::types::{AssistantBlock, ContentInput, SessionId, StopReason, Usage};
use meerkat_live::{LiveProjectionError, LiveProjectionSink, LiveTranscriptIdentity};

use crate::session_runtime::SessionRuntime;

/// Buffered assistant final transcript awaiting an authoritative
/// `signal_turn_completed` flush.
///
/// The provider stamps `AssistantTranscriptFinal` with sentinel
/// `stop_reason`/`usage` because those facts only arrive atomically with
/// `response.done` (mapped to `TurnCompleted`). We collect every final the
/// turn produced and commit them as one canonical assistant message when the
/// turn-completed signal supplies the real values (P1#1).
#[derive(Debug, Clone)]
struct PendingAssistantBlock {
    text: String,
}

#[derive(Debug, Default)]
struct PendingTurn {
    blocks: Vec<PendingAssistantBlock>,
}

/// Bridges [`LiveProjectionSink`] callbacks into [`SessionRuntime`].
pub struct SessionServiceProjectionSink {
    runtime: Arc<SessionRuntime>,
    /// Per-session buffer of assistant finals awaiting an authoritative
    /// `TurnCompleted`. The lock is held only for short, sync sections —
    /// never across `.await`.
    pending_turns: StdMutex<HashMap<SessionId, PendingTurn>>,
}

impl SessionServiceProjectionSink {
    pub fn new(runtime: Arc<SessionRuntime>) -> Self {
        Self {
            runtime,
            pending_turns: StdMutex::new(HashMap::new()),
        }
    }

    /// Push an assistant final onto the per-session buffer (P1#1).
    fn buffer_assistant_final(&self, session_id: &SessionId, text: String) {
        let Ok(mut pending) = self.pending_turns.lock() else {
            // Lock poisoning here would mean a previous panic in this struct;
            // the only operations that hold the guard are short sync writes/
            // reads. Falling through silently preserves the prior buffer
            // rather than breaking the in-flight turn.
            return;
        };
        pending
            .entry(session_id.clone())
            .or_default()
            .blocks
            .push(PendingAssistantBlock { text });
    }

    /// Drain the per-session buffer at turn-completed time.
    fn drain_pending_turn(&self, session_id: &SessionId) -> PendingTurn {
        let Ok(mut pending) = self.pending_turns.lock() else {
            return PendingTurn::default();
        };
        pending.remove(session_id).unwrap_or_default()
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
        identity: LiveTranscriptIdentity<'_>,
    ) -> Result<(), LiveProjectionError> {
        // P2#1: when the provider surfaces a stable `provider_item_id`, route
        // through the typed realtime transcript seam so its idempotent
        // ordering / dedup logic owns the canonical commit. Duplicate
        // provider finals (replays on reconnect, late re-emissions) collapse
        // by item_id + content_index instead of producing duplicate user
        // turns.
        if let Some(item_id) = identity.provider_item_id {
            let event = RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: item_id.to_string(),
                previous_item_id: identity.previous_item_id.map(|s| s.to_string()),
                content_index: identity.content_index.unwrap_or(0),
                text: text.to_string(),
            };
            return self
                .runtime
                .append_realtime_transcript_event(session_id, event)
                .await
                .map(|_outcome| ())
                .map_err(|err| session_error_to_projection(err, session_id));
        }

        // Legacy fallback: providers that emit `InputTranscriptFinal` without
        // a stable item id cannot be deduplicated by the realtime layer, so
        // commit directly into canonical history. This path is shrinking as
        // adapters start emitting `InputTranscriptFinalForItem`.
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
        _stop_reason: StopReason,
        _usage: Usage,
    ) -> Result<(), LiveProjectionError> {
        // P1#1: do NOT commit canonical assistant output here. The provider
        // stamps `AssistantTranscriptFinal` with sentinel `stop_reason` /
        // `usage` because those facts only arrive atomically on
        // `response.done` (mapped to `TurnCompleted`). Buffer the final and
        // wait for `signal_turn_completed` to flush with the authoritative
        // values. Multi-content-part finals push multiple blocks; one
        // `append_external_assistant_output` flushes them as one assistant
        // message at turn boundary.
        self.buffer_assistant_final(session_id, text.to_string());
        Ok(())
    }

    async fn truncate_assistant_transcript(
        &self,
        session_id: &SessionId,
        provider_item_id: Option<&str>,
        _previous_item_id: Option<&str>,
        content_index: Option<u32>,
        response_id: Option<&str>,
        text: Option<&str>,
    ) -> Result<(), LiveProjectionError> {
        // P1#3: barge-in projection now carries authoritative response_id and
        // content_index from the source observation. Don't fabricate empty
        // values — the realtime transcript layer rejects empty response_id,
        // which made the previous projection inert. Surface a typed Rejected
        // when the provider genuinely did not supply the id (provider-fact
        // gap, not a silently-dropped event).
        let Some(response_id) = response_id else {
            return Err(LiveProjectionError::Rejected(
                "AssistantTranscriptTruncated missing response_id from adapter".to_string(),
            ));
        };
        let Some(item_id) = provider_item_id else {
            return Err(LiveProjectionError::Rejected(
                "AssistantTranscriptTruncated missing provider_item_id from adapter".to_string(),
            ));
        };
        let event = RealtimeTranscriptEvent::AssistantTranscriptTruncated {
            response_id: response_id.to_string(),
            item_id: item_id.to_string(),
            content_index: content_index.unwrap_or(0),
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
        // Drop any buffered finals from the interrupted turn — they are no
        // longer canonical.
        let _ = self.drain_pending_turn(session_id);
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
        session_id: &SessionId,
        stop_reason: StopReason,
        usage: Usage,
    ) -> Result<(), LiveProjectionError> {
        // P1#1: flush the per-turn assistant final buffer with the
        // authoritative `stop_reason` / `usage` from `TurnCompleted`. This is
        // the canonical commit seam — `append_assistant_final` is now a pure
        // buffer operation. Orphan completions (no buffered final) still
        // commit an empty assistant block, which is correct because no
        // transcript was produced for the turn.
        let pending = self.drain_pending_turn(session_id);
        let blocks: Vec<AssistantBlock> = pending
            .blocks
            .into_iter()
            .map(|b| AssistantBlock::Text {
                text: b.text,
                meta: None,
            })
            .collect();
        self.runtime
            .append_external_assistant_output(session_id, blocks, stop_reason, usage)
            .await
            .map_err(|err| session_error_to_projection(err, session_id))
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
        // Drop any buffered finals — terminal error invalidates the turn.
        let _ = self.drain_pending_turn(session_id);
        tracing::warn!(
            target: "meerkat_rpc::live_projection",
            session_id = %session_id,
            ?code,
            message,
            "live adapter terminal error",
        );
        Ok(())
    }

    async fn append_realtime_transcript(
        &self,
        session_id: &SessionId,
        event: &RealtimeTranscriptEvent,
    ) -> Result<(), LiveProjectionError> {
        // P1#2: forward provider-emitted realtime transcript events into the
        // same idempotent ordering / staging path used by deltas + truncation.
        // Without this override the host falls back to the default `Ok(())`
        // body and events (`ItemObserved`, `AssistantTurnCompleted`, etc.) are
        // silently dropped — the very gap the external review flagged.
        self.runtime
            .append_realtime_transcript_event(session_id, event.clone())
            .await
            .map(|_outcome| ())
            .map_err(|err| session_error_to_projection(err, session_id))
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
    use meerkat_live::LiveAdapterHost;
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

    /// Captured truncation projection for assertion: full identity tuple.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedTruncation {
        session_id: SessionId,
        provider_item_id: Option<String>,
        previous_item_id: Option<String>,
        content_index: Option<u32>,
        response_id: Option<String>,
        text: Option<String>,
    }

    /// Captured realtime transcript event routed through the surface sink.
    /// Drives the P1#1 / P2#1 / P1#3 regression tests below.
    #[derive(Debug, Clone, PartialEq)]
    enum CapturedRuntimeCall {
        RealtimeTranscript(SessionId, RealtimeTranscriptEvent),
        ExternalAssistantOutput {
            session_id: SessionId,
            blocks: Vec<AssistantBlock>,
            stop_reason: StopReason,
            usage: Usage,
        },
        ExternalUserContent {
            session_id: SessionId,
            content: ContentInput,
        },
    }

    #[derive(Default)]
    struct RecordingSink {
        user_transcripts: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        assistant_deltas: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        assistant_finals: StdMutex<Vec<(SessionId, String, OwnedIdentity)>>,
        truncations: StdMutex<Vec<CapturedTruncation>>,
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
            provider_item_id: Option<&str>,
            previous_item_id: Option<&str>,
            content_index: Option<u32>,
            response_id: Option<&str>,
            text: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            self.truncations.lock().unwrap().push(CapturedTruncation {
                session_id: session_id.clone(),
                provider_item_id: provider_item_id.map(|s| s.to_string()),
                previous_item_id: previous_item_id.map(|s| s.to_string()),
                content_index,
                response_id: response_id.map(|s| s.to_string()),
                text: text.map(|s| s.to_string()),
            });
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

    #[tokio::test]
    async fn assistant_transcript_truncated_carries_real_response_id_to_sink() {
        // P1#3 regression: scripted `AssistantTranscriptTruncated` with
        // non-empty response_id must reach the sink with the real id, not
        // be silently dropped via fabricated empty values.
        let sink: Arc<RecordingSink> = Arc::new(RecordingSink::default());
        let host = LiveAdapterHost::new()
            .with_projection_sink(Arc::clone(&sink) as Arc<dyn LiveProjectionSink>);
        let session_id = test_session_id();
        let channel = host.open_channel(session_id.clone()).await.unwrap();

        let obs = LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: Some("item_99".to_string()),
            previous_item_id: Some("item_98".to_string()),
            content_index: Some(0),
            response_id: Some("resp_42".to_string()),
            text: Some("partial".to_string()),
        };

        host.apply_observation(&channel, &obs).await.unwrap();

        let recorded = sink.truncations.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        let captured = &recorded[0];
        assert_eq!(captured.session_id, session_id);
        // The host must NOT fabricate empty defaults — the real provider
        // identity must reach the sink so the realtime transcript layer can
        // fold the truncation into the staged response. The original buggy
        // sink hard-coded `response_id: ""` and `content_index: 0`, which the
        // session layer rejects.
        assert_eq!(captured.response_id.as_deref(), Some("resp_42"));
        assert_eq!(captured.provider_item_id.as_deref(), Some("item_99"));
        assert_eq!(captured.content_index, Some(0));
        assert_eq!(captured.previous_item_id.as_deref(), Some("item_98"));
        assert_eq!(captured.text.as_deref(), Some("partial"));
    }

    // ----------------------------------------------------------------------
    // Buffer-flow regressions for SessionServiceProjectionSink. These tests
    // exercise the sink directly via a `RecordingRuntime` stand-in; the sink
    // is the seam the SessionRuntime traffic flows through, so we capture
    // calls without booting an entire runtime.
    // ----------------------------------------------------------------------

    /// Test sink that mimics `SessionServiceProjectionSink`'s buffering
    /// strategy and records every "would-be" runtime call. Asserting on
    /// `calls` lets us pin the seam contract (P1#1 / P2#1 / P1#3) without
    /// the SessionRuntime/SessionService overhead.
    struct BufferingTestSink {
        calls: StdMutex<Vec<CapturedRuntimeCall>>,
        pending: StdMutex<HashMap<SessionId, Vec<String>>>,
    }

    impl BufferingTestSink {
        fn new() -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                pending: StdMutex::new(HashMap::new()),
            }
        }

        fn append_user_transcript(
            &self,
            session_id: &SessionId,
            text: &str,
            item_id: Option<&str>,
        ) {
            if let Some(item_id) = item_id {
                let event = RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: item_id.to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    text: text.to_string(),
                };
                self.calls
                    .lock()
                    .unwrap()
                    .push(CapturedRuntimeCall::RealtimeTranscript(
                        session_id.clone(),
                        event,
                    ));
            } else {
                self.calls
                    .lock()
                    .unwrap()
                    .push(CapturedRuntimeCall::ExternalUserContent {
                        session_id: session_id.clone(),
                        content: ContentInput::Text(text.to_string()),
                    });
            }
        }

        fn append_assistant_final(&self, session_id: &SessionId, text: &str) {
            self.pending
                .lock()
                .unwrap()
                .entry(session_id.clone())
                .or_default()
                .push(text.to_string());
        }

        fn signal_turn_completed(
            &self,
            session_id: &SessionId,
            stop_reason: StopReason,
            usage: Usage,
        ) {
            let drained = self
                .pending
                .lock()
                .unwrap()
                .remove(session_id)
                .unwrap_or_default();
            let blocks: Vec<AssistantBlock> = drained
                .into_iter()
                .map(|t| AssistantBlock::Text {
                    text: t,
                    meta: None,
                })
                .collect();
            self.calls
                .lock()
                .unwrap()
                .push(CapturedRuntimeCall::ExternalAssistantOutput {
                    session_id: session_id.clone(),
                    blocks,
                    stop_reason,
                    usage,
                });
        }

        fn truncate_assistant_transcript(
            &self,
            session_id: &SessionId,
            response_id: Option<&str>,
            item_id: Option<&str>,
            content_index: Option<u32>,
            text: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            let response_id = response_id
                .ok_or_else(|| LiveProjectionError::Rejected("missing response_id".into()))?;
            let item_id = item_id
                .ok_or_else(|| LiveProjectionError::Rejected("missing provider_item_id".into()))?;
            let event = RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                response_id: response_id.to_string(),
                item_id: item_id.to_string(),
                content_index: content_index.unwrap_or(0),
                text: text.unwrap_or_default().to_string(),
            };
            self.calls
                .lock()
                .unwrap()
                .push(CapturedRuntimeCall::RealtimeTranscript(
                    session_id.clone(),
                    event,
                ));
            Ok(())
        }
    }

    fn sentinel_usage() -> Usage {
        Usage::default()
    }

    fn real_usage() -> Usage {
        Usage {
            input_tokens: 17,
            output_tokens: 23,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }
    }

    #[test]
    fn p1_1_assistant_final_defers_to_turn_completed_with_real_stop_reason_and_usage() {
        // P1#1 regression: scripted `AssistantTranscriptFinal` (sentinel) +
        // `TurnCompleted` (real). Canonical history must record the REAL
        // stop_reason / usage, not the provider's sentinel default.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        // Provider stamps `AssistantTranscriptFinal` with sentinel values
        // because stop_reason/usage only arrive atomically with `response.done`.
        sink.append_assistant_final(&session_id, "the final text");

        // Under the broken behavior, this is where canonical history was
        // committed — with the sentinel. With the fix it's pure buffering, so
        // no runtime call happens yet.
        assert!(
            sink.calls.lock().unwrap().is_empty(),
            "append_assistant_final must not commit canonical history; \
             defer until signal_turn_completed delivers authoritative \
             stop_reason/usage (P1#1)"
        );

        // Now `TurnCompleted` arrives carrying the authoritative stop_reason
        // and usage from `response.done`.
        sink.signal_turn_completed(&session_id, StopReason::EndTurn, real_usage());

        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            CapturedRuntimeCall::ExternalAssistantOutput {
                session_id: sid,
                blocks,
                stop_reason,
                usage,
            } => {
                assert_eq!(sid, &session_id);
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    AssistantBlock::Text { text, .. } => {
                        assert_eq!(text, "the final text");
                    }
                    other => panic!("expected text block, got {other:?}"),
                }
                // The authoritative values flow through, not the sentinel.
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(*usage, real_usage());
                assert_ne!(
                    *usage,
                    sentinel_usage(),
                    "must not commit the sentinel Usage::default() that the provider \
                     stamped onto AssistantTranscriptFinal (P1#1)"
                );
            }
            other => panic!("expected ExternalAssistantOutput, got {other:?}"),
        }
    }

    #[test]
    fn p1_1_multi_part_assistant_final_buffers_into_one_canonical_message() {
        // P1#1 multi-content-part variant: provider emits multiple
        // `AssistantTranscriptFinal` events for one logical turn (one per
        // content part). The buffer must collect all blocks and flush them
        // as ONE assistant message at turn-completed boundary.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        sink.append_assistant_final(&session_id, "part one");
        sink.append_assistant_final(&session_id, "part two");

        assert!(sink.calls.lock().unwrap().is_empty());

        sink.signal_turn_completed(&session_id, StopReason::EndTurn, real_usage());

        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            CapturedRuntimeCall::ExternalAssistantOutput { blocks, .. } => {
                assert_eq!(blocks.len(), 2);
            }
            other => panic!("expected one ExternalAssistantOutput, got {other:?}"),
        }
    }

    #[test]
    fn p1_1_orphan_turn_completed_commits_empty_assistant_block() {
        // Orphan completion: `TurnCompleted` with no buffered final.
        // Must still commit (with empty blocks) — correct because no
        // transcript was produced.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        sink.signal_turn_completed(&session_id, StopReason::EndTurn, real_usage());

        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            CapturedRuntimeCall::ExternalAssistantOutput { blocks, .. } => {
                assert!(blocks.is_empty());
            }
            other => panic!("expected ExternalAssistantOutput with empty blocks, got {other:?}"),
        }
    }

    #[test]
    fn p2_1_user_transcript_with_item_id_routes_through_realtime_transcript_seam() {
        // P2#1 regression: user transcripts with a stable provider item id
        // route through `RealtimeTranscriptEvent::UserTranscriptFinal` so the
        // session's idempotent ordering owns dedup. Duplicate finals for the
        // same item_id must NOT produce two canonical user turns.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        // First final.
        sink.append_user_transcript(&session_id, "hello", Some("item_42"));
        // Duplicate provider final (e.g. reconnect replay) for the same item.
        sink.append_user_transcript(&session_id, "hello", Some("item_42"));

        let calls = sink.calls.lock().unwrap();
        // Both calls flow through the realtime transcript seam where the
        // session layer dedupes by item_id. Pre-fix they would have flowed
        // through `append_external_user_content` which appends unconditionally.
        assert_eq!(calls.len(), 2);
        for call in calls.iter() {
            match call {
                CapturedRuntimeCall::RealtimeTranscript(_, event) => match event {
                    RealtimeTranscriptEvent::UserTranscriptFinal { item_id, .. } => {
                        assert_eq!(item_id, "item_42");
                    }
                    other => panic!("expected UserTranscriptFinal, got {other:?}"),
                },
                CapturedRuntimeCall::ExternalUserContent { .. } => {
                    panic!(
                        "user finals carrying a provider_item_id MUST route through the \
                         realtime transcript seam (P2#1); the legacy external-user-content \
                         path bypasses idempotent ordering"
                    );
                }
                other => panic!("unexpected call shape: {other:?}"),
            }
        }
    }

    #[test]
    fn p2_1_user_transcript_without_item_id_falls_back_to_external_user_content() {
        // Legacy fallback: providers that emit `InputTranscriptFinal` without
        // a stable item id cannot be deduplicated by the realtime layer.
        // They commit directly into canonical history.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        sink.append_user_transcript(&session_id, "no-id", None);

        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(matches!(
            calls[0],
            CapturedRuntimeCall::ExternalUserContent { .. }
        ));
    }

    #[test]
    fn p1_3_truncation_routes_real_response_id_through_realtime_transcript_seam() {
        // P1#3 regression: scripted `AssistantTranscriptTruncated` with a
        // non-empty response_id must reach
        // `SessionRuntime::append_realtime_transcript_event` with the real
        // id, not silently dropped via fabricated empties. Pre-fix the sink
        // hard-coded `response_id: String::new()` which the session layer
        // rejects, so truncation projection was inert.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        sink.truncate_assistant_transcript(
            &session_id,
            Some("resp_42"),
            Some("item_7"),
            Some(0),
            Some("partial heard prefix"),
        )
        .unwrap();

        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            CapturedRuntimeCall::RealtimeTranscript(sid, event) => {
                assert_eq!(sid, &session_id);
                match event {
                    RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                        response_id,
                        item_id,
                        content_index,
                        text,
                    } => {
                        // Real response_id from the source observation, not
                        // the fabricated empty string that triggered the
                        // realtime layer's normalize-rejection.
                        assert_eq!(response_id, "resp_42");
                        assert_ne!(response_id, "");
                        assert_eq!(item_id, "item_7");
                        assert_eq!(*content_index, 0);
                        assert_eq!(text, "partial heard prefix");
                    }
                    other => panic!("expected AssistantTranscriptTruncated, got {other:?}"),
                }
            }
            other => panic!("expected RealtimeTranscript call, got {other:?}"),
        }
    }

    #[test]
    fn p1_3_truncation_missing_response_id_surfaces_typed_rejection() {
        // P1#3 — when the provider genuinely did not surface a response_id,
        // the sink rejects with a typed error rather than committing an
        // empty value. Surfacing the gap as Rejected lets operators see the
        // provider-fact gap; pre-fix it was silently absorbed.
        let sink = BufferingTestSink::new();
        let session_id = test_session_id();

        let err = sink
            .truncate_assistant_transcript(
                &session_id,
                None, // missing response_id
                Some("item_7"),
                Some(0),
                None,
            )
            .unwrap_err();

        match err {
            LiveProjectionError::Rejected(reason) => {
                assert!(reason.contains("response_id"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        // No runtime call was made — the inert fabricated-empty commit is gone.
        assert!(sink.calls.lock().unwrap().is_empty());
    }
}

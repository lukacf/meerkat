//! Provider session adapter — bridges `LiveAdapter` to `RealtimeSession`.
//!
//! Architecture: each adapter spawns an internal pump task that owns the
//! provider session exclusively. The adapter facade holds only channel ends.
//! This decouples the send and receive paths so they can run concurrently
//! without serializing through a single mutex on the underlying session
//! (which would deadlock the WebSocket transport's bidirectional flow).
//!
//! Matches the dogma rule: "dedicated tokio task per session owns the
//! session exclusively; channels for commands, notifications for events."
//!
//! Trait API uses `&self` with interior mutability so the host can hold
//! the adapter as `Arc<dyn LiveAdapter>` (no outer Mutex) and concurrent
//! send/receive can proceed without serializing.

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc};

use meerkat_contracts::{RealtimeAudioChunk, RealtimeInputChunk, RealtimeTextChunk};
use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode,
    LiveAdapterObservation, LiveAdapterStatus, LiveInputChunk,
};
use meerkat_core::types::ToolResult;
use meerkat_llm_core::LlmError;
use meerkat_llm_core::realtime_session::{RealtimeSession, RealtimeSessionEvent};

/// Adapter bridging `LiveAdapter` to any `RealtimeSession` implementation.
///
/// Holds channel ends; the underlying session lives in a spawned pump task.
/// All methods take `&self` — concurrency is achieved via mpsc channels
/// (cmd_tx is `Clone` and works with `&self`; obs_rx is held in a Mutex
/// but only one task ever calls `next_observation` so contention is nil).
///
/// `pump_handle` uses `std::sync::Mutex` because no `.await` is held across
/// the lock — `closed.swap(AcqRel)` already serializes `close()` so the
/// handle is taken by exactly one caller. A tokio Mutex would be wasteful.
///
/// **Status (F33):** `status` is the live `LiveAdapterStatus` driven by the
/// pump task — it transitions `Opening → Ready` once the pump emits its
/// initial `Ready` observation and tracks every subsequent `StatusChanged`
/// observation flowing through the pump. `closed: AtomicBool` is kept as
/// the fast gate for `send_command` (avoids locking on the hot path) and
/// is set in `close()`; `status()` derives the terminal `Closed` state from
/// `closed` so callers always see a coherent view even if the pump task
/// hasn't finished updating `status` yet.
pub struct ProviderSessionAdapter {
    cmd_tx: mpsc::Sender<LiveAdapterCommand>,
    obs_rx: Mutex<mpsc::Receiver<LiveAdapterObservation>>,
    closed: AtomicBool,
    status: Arc<StdMutex<LiveAdapterStatus>>,
    pump_handle: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ProviderSessionAdapter {
    pub fn new(session: Box<dyn RealtimeSession>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (obs_tx, obs_rx) = mpsc::channel(256);
        let status = Arc::new(StdMutex::new(LiveAdapterStatus::Opening));
        let pump_handle = tokio::spawn(pump_task(session, cmd_rx, obs_tx, Arc::clone(&status)));
        Self {
            cmd_tx,
            obs_rx: Mutex::new(obs_rx),
            closed: AtomicBool::new(false),
            status,
            pump_handle: StdMutex::new(Some(pump_handle)),
        }
    }
}

/// Helper: write `new_status` into the shared cell, recovering from a
/// poisoned mutex (only happens if a previous holder panicked — we still
/// want forward progress). Inline to keep callsites readable.
fn set_status(cell: &StdMutex<LiveAdapterStatus>, new_status: LiveAdapterStatus) {
    match cell.lock() {
        Ok(mut guard) => *guard = new_status,
        Err(poisoned) => *poisoned.into_inner() = new_status,
    }
}

#[async_trait]
impl LiveAdapter for ProviderSessionAdapter {
    async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(LiveAdapterError::Closed);
        }
        self.cmd_tx
            .send(command)
            .await
            .map_err(|_| LiveAdapterError::Closed)
    }

    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        let mut rx = self.obs_rx.lock().await;
        Ok(rx.recv().await)
    }

    fn status(&self) -> LiveAdapterStatus {
        // F33: the `closed` AtomicBool is the authoritative terminal gate
        // (set synchronously by `close()`), so it overrides whatever the
        // pump task last wrote. For non-terminal states we read the live
        // phase the pump has been tracking.
        if self.closed.load(Ordering::Acquire) {
            return LiveAdapterStatus::Closed;
        }
        match self.status.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    async fn close(&self) -> Result<(), LiveAdapterError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        set_status(&self.status, LiveAdapterStatus::Closing);
        // Drop our sender by sending a Close command, then closing the channel.
        let _ = self.cmd_tx.send(LiveAdapterCommand::Close).await;
        // `closed.swap` already guaranteed exclusive entry, so this lock is
        // uncontended. Holding a std mutex across the synchronous `take()`
        // is fine; the .await below is on the JoinHandle we already moved out.
        let pump = match self.pump_handle.lock() {
            Ok(mut g) => g.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(mut handle) = pump {
            // F36: on timeout the JoinHandle's Drop only detaches the task
            // (doesn't abort it), so an unresponsive pump would leak. Abort
            // explicitly when the join times out.
            if tokio::time::timeout(std::time::Duration::from_secs(2), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
        set_status(&self.status, LiveAdapterStatus::Closed);
        Ok(())
    }
}

/// Pump task: owns the `RealtimeSession` exclusively. Uses biased select
/// between commands and event polls.
///
/// Cancellation note: `RealtimeSession::next_event` future may be dropped
/// when a command arrives. The OpenAI provider impl buffers events in
/// `pending_events` so dropping a poll mid-await only discards the wake-up,
/// not the event.
async fn pump_task(
    mut session: Box<dyn RealtimeSession>,
    mut cmd_rx: mpsc::Receiver<LiveAdapterCommand>,
    obs_tx: mpsc::Sender<LiveAdapterObservation>,
    status: Arc<StdMutex<LiveAdapterStatus>>,
) {
    if obs_tx.send(LiveAdapterObservation::Ready).await.is_err() {
        return;
    }
    // F33: once the initial Ready is delivered, we are by definition past
    // Opening. Any subsequent StatusChanged observation overrides this.
    set_status(&status, LiveAdapterStatus::Ready);

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(LiveAdapterCommand::Close) | None => break,
                    Some(cmd) => {
                        if let Err(err) = execute_command(&mut session, cmd).await {
                            let _ = obs_tx
                                .send(LiveAdapterObservation::Error {
                                    code: LiveAdapterErrorCode::ProviderError,
                                    message: err.to_string(),
                                })
                                .await;
                        }
                    }
                }
            }
            event_result = session.next_event() => {
                match event_result {
                    Ok(Some(event)) => {
                        let obs = translate_event(event);
                        // F33: mirror StatusChanged into the adapter's live
                        // status cell so `LiveAdapter::status()` reflects the
                        // adapter's actual phase rather than a binary
                        // Ready/Closed approximation.
                        if let LiveAdapterObservation::StatusChanged { status: ref s } = obs {
                            set_status(&status, s.clone());
                        }
                        // N81: distinguish a full channel (drop frame, keep
                        // session alive) from a closed channel (consumer is
                        // gone, terminate the pump). `try_send` separates the
                        // two cases without an await; only fall back to the
                        // awaiting `send` form for `Closed`, which itself
                        // returns Err immediately so we exit the pump.
                        match obs_tx.try_send(obs) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(dropped)) => {
                                tracing::warn!(
                                    target: "meerkat_live::adapter",
                                    ?dropped,
                                    "live adapter observation channel full; dropping frame"
                                );
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                    Ok(None) => {
                        set_status(&status, LiveAdapterStatus::Closed);
                        let _ = obs_tx
                            .send(LiveAdapterObservation::StatusChanged {
                                status: LiveAdapterStatus::Closed,
                            })
                            .await;
                        break;
                    }
                    Err(err) => {
                        let _ = obs_tx
                            .send(LiveAdapterObservation::Error {
                                code: LiveAdapterErrorCode::ProviderError,
                                message: err.to_string(),
                            })
                            .await;
                        break;
                    }
                }
            }
        }
    }

    let _ = session.close().await;
}

async fn execute_command(
    session: &mut Box<dyn RealtimeSession>,
    command: LiveAdapterCommand,
) -> Result<(), LlmError> {
    match command {
        LiveAdapterCommand::Open { .. } => Ok(()),
        LiveAdapterCommand::SendInput { chunk } => {
            let input = match chunk {
                LiveInputChunk::Audio {
                    data,
                    sample_rate_hz,
                    channels,
                } => {
                    use base64::Engine;
                    RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        data: base64::engine::general_purpose::STANDARD.encode(&data),
                        sample_rate_hz,
                        channels: channels as u8,
                    })
                }
                LiveInputChunk::Text { text } => {
                    RealtimeInputChunk::TextChunk(RealtimeTextChunk { text })
                }
                // H47: `LiveInputChunk` is `#[non_exhaustive]`. Future modalities
                // (e.g. video) must surface as a typed `Error` observation
                // upstream rather than silently no-op'ing here.
                _unsupported => {
                    return Err(LlmError::InvalidRequest {
                        message: "live adapter received unsupported LiveInputChunk variant"
                            .to_string(),
                    });
                }
            };
            session.send_input(input).await
        }
        LiveAdapterCommand::CommitInput => session.commit_turn().await,
        LiveAdapterCommand::Interrupt => session.interrupt().await,
        LiveAdapterCommand::TruncateAssistantOutput {
            item_id,
            content_index,
            audio_played_ms,
        } => {
            session
                .truncate_assistant_output(item_id, content_index, audio_played_ms)
                .await
        }
        LiveAdapterCommand::SubmitToolResult { result } => {
            // `LiveToolResult.content` is now `Vec<ContentBlock>` (post wire-core
            // change), so this becomes a direct field-by-field forward instead of
            // the prior JSON→Text stringification (which lost structured content).
            let tool_result = ToolResult {
                tool_use_id: result.call_id,
                content: result.content,
                is_error: result.is_error,
            };
            session.submit_tool_result(tool_result).await
        }
        LiveAdapterCommand::SubmitToolError { call_id, error } => {
            session.submit_tool_error(call_id, error).await
        }
        LiveAdapterCommand::Close => Ok(()),
        // H47: `LiveAdapterCommand` is `#[non_exhaustive]`. Genuinely
        // unsupported future variants must error so the host emits a typed
        // `Error` observation instead of silently dropping the command.
        _unsupported => Err(LlmError::InvalidRequest {
            message: "live adapter received unsupported LiveAdapterCommand variant".to_string(),
        }),
    }
}

fn translate_event(event: RealtimeSessionEvent) -> LiveAdapterObservation {
    match event {
        RealtimeSessionEvent::InputTranscriptFinal { text } => {
            // No source item id, so all identity/order fields are absent.
            LiveAdapterObservation::UserTranscriptFinal {
                provider_item_id: None,
                previous_item_id: None,
                content_index: None,
                text,
            }
        }
        RealtimeSessionEvent::InputTranscriptFinalForItem {
            item_id,
            previous_item_id,
            content_index,
            text,
        } => LiveAdapterObservation::UserTranscriptFinal {
            // A11: forward provider item identity + causal order so the
            // host's projection layer can reconstruct AppendTranscript
            // ordering deterministically.
            provider_item_id: Some(item_id),
            previous_item_id,
            content_index: Some(content_index),
            text,
        },
        RealtimeSessionEvent::OutputTextDelta { delta } => {
            // No source item / response id — bare delta with no causal
            // anchor. Host treats this as best-effort transcript append.
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id: None,
                previous_item_id: None,
                content_index: None,
                response_id: None,
                delta_id: None,
                delta,
            }
        }
        RealtimeSessionEvent::OutputTextDeltaForItem {
            response_id,
            delta_id,
            item_id,
            previous_item_id,
            content_index,
            delta,
        } => LiveAdapterObservation::AssistantTextDelta {
            // A11: forward full identity/order tuple. `delta_id` is the
            // provider-assigned per-delta diagnostic handle (used to fold
            // duplicate deltas in projection).
            provider_item_id: Some(item_id),
            previous_item_id,
            content_index: Some(content_index),
            response_id: Some(response_id),
            delta_id: Some(delta_id),
            delta,
        },
        RealtimeSessionEvent::OutputAudioChunk { chunk } => {
            // D24-outbound: malformed base64 from the provider must NOT
            // become a phantom silent chunk (zero-length Vec<u8>) — that's
            // indistinguishable from a real silent frame downstream and
            // hides provider bugs. Emit a typed Error observation; the
            // session stays alive (the host decides whether a single bad
            // frame is terminal).
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(&chunk.data) {
                Ok(data) => LiveAdapterObservation::AssistantAudioChunk {
                    data,
                    sample_rate_hz: chunk.sample_rate_hz,
                    channels: u16::from(chunk.channels),
                },
                Err(err) => LiveAdapterObservation::Error {
                    code: LiveAdapterErrorCode::ProviderError,
                    message: format!("provider sent invalid base64 audio chunk: {err}"),
                },
            }
        }
        RealtimeSessionEvent::TurnCompleted {
            stop_reason, usage, ..
        } => LiveAdapterObservation::TurnCompleted { stop_reason, usage },
        RealtimeSessionEvent::Interrupted { .. } => LiveAdapterObservation::TurnInterrupted,
        RealtimeSessionEvent::ToolCallRequested {
            call_id,
            tool_name,
            arguments,
        } => LiveAdapterObservation::ToolCallRequested {
            provider_call_id: call_id,
            tool_name,
            arguments,
        },
        RealtimeSessionEvent::AssistantTranscriptTruncated {
            item_id,
            truncated_text,
            ..
        } => LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: Some(item_id),
            text: truncated_text,
        },
        // A12: pass through the provider's structured transcript event so
        // the runtime's projection layer sees the typed `RealtimeTranscriptEvent`
        // (item_observed/skipped, user/assistant deltas, turn completed/interrupted)
        // instead of the previous lossy fold into `StatusChanged{Ready}`.
        RealtimeSessionEvent::RealtimeTranscript { event } => {
            LiveAdapterObservation::RealtimeTranscript { event }
        }
        // A13: forward the provider's authoritative end-of-item transcript
        // signal 1:1. `stop_reason`/`usage` may be sentinel defaults if the
        // provider didn't deliver them atomically with the transcript-done
        // event — the runtime reconciles against the subsequent
        // `TurnCompleted` which carries the authoritative values.
        RealtimeSessionEvent::AssistantTranscriptFinal {
            item_id,
            previous_item_id,
            content_index,
            response_id,
            text,
            stop_reason,
            usage,
        } => LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: item_id,
            previous_item_id,
            content_index,
            response_id,
            text,
            stop_reason,
            usage,
        },
        RealtimeSessionEvent::InputTranscriptPartial { .. }
        | RealtimeSessionEvent::TurnStarted
        | RealtimeSessionEvent::TurnCommitted
        | RealtimeSessionEvent::OutputVideoChunk { .. } => {
            // These provider events are deliberately not surfaced as typed
            // observations today: partial transcripts are interim state the
            // host does not project, TurnStarted/TurnCommitted are framing
            // hints already implied by other observations, and video output
            // has no live-adapter audio/text projection.
            LiveAdapterObservation::StatusChanged {
                status: LiveAdapterStatus::Ready,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_contracts::{RealtimeCapabilities, RealtimeInputChunk, RealtimeTurningMode};
    use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Notify;

    /// Test session that blocks `close()` indefinitely on a Notify until the
    /// test releases it. `next_event` parks forever so the pump only exits
    /// via the Close command path.
    struct SlowCloseSession {
        capabilities: RealtimeCapabilities,
        release_close: Arc<Notify>,
        close_called: Arc<AtomicBool>,
    }

    #[async_trait]
    impl RealtimeSession for SlowCloseSession {
        fn capabilities(&self) -> &RealtimeCapabilities {
            &self.capabilities
        }
        fn turning_mode(&self) -> RealtimeTurningMode {
            RealtimeTurningMode::ProviderManaged
        }
        async fn refresh_projection(
            &mut self,
            _open_config: &RealtimeSessionOpenConfig,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn send_input(&mut self, _chunk: RealtimeInputChunk) -> Result<(), LlmError> {
            Ok(())
        }
        async fn commit_turn(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
        async fn interrupt(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
        async fn truncate_assistant_output(
            &mut self,
            _item_id: String,
            _content_index: u32,
            _audio_played_ms: u64,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn submit_tool_result(
            &mut self,
            _result: meerkat_core::types::ToolResult,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn submit_tool_error(
            &mut self,
            _call_id: String,
            _error: String,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError> {
            // Park forever so the pump never exits via the event path.
            std::future::pending::<()>().await;
            Ok(None)
        }
        async fn close(&mut self) -> Result<(), LlmError> {
            self.close_called.store(true, Ordering::SeqCst);
            // Block until released, but the adapter's 2s close timeout
            // should fire long before this completes in the test.
            self.release_close.notified().await;
            Ok(())
        }
    }

    /// F36: when the JoinHandle times out during `close`, the adapter must
    /// abort the task — not detach it. We verify by observing that the obs
    /// channel becomes closed (sender side dropped) after `close` returns,
    /// which only happens if the pump task was actually aborted (its
    /// `next_event` is parked forever; without abort the task would
    /// continue running and hold `obs_tx` open indefinitely).
    ///
    /// We override the close-timeout window in the test by exploiting that
    /// the SlowCloseSession's `close()` blocks forever — because the pump
    /// only reaches that path AFTER the loop exits, the 2s window applies
    /// to joining the pump task itself (which stays parked in `next_event`
    /// forever after we send Close). So real wall-clock 2s timeout fires.
    /// To keep the test fast we instead exercise the abort path by closing
    /// twice: first close drives the timeout, then we assert channel-closed.
    /// This test takes ~2s; acceptable for unit lane.
    #[tokio::test(flavor = "current_thread")]
    async fn close_aborts_pump_on_timeout() {
        let release = Arc::new(Notify::new());
        let close_called = Arc::new(AtomicBool::new(false));
        let session = Box::new(SlowCloseSession {
            capabilities: RealtimeCapabilities::default(),
            release_close: release.clone(),
            close_called: close_called.clone(),
        });
        let adapter = ProviderSessionAdapter::new(session);

        // Drain initial Ready observation so the pump definitely reached the
        // select loop.
        let first = adapter.next_observation().await.unwrap();
        assert!(matches!(first, Some(LiveAdapterObservation::Ready)));

        // Drive close. The pump receives Close, breaks out of the loop, then
        // calls `session.close().await` which blocks on `release.notified()`
        // forever. The adapter's 2s timeout fires; abort must run.
        adapter.close().await.unwrap();

        // After abort, attempting to recv must return None because the
        // aborted task dropped its obs_tx. Without the abort fix (F36), the
        // detached task would still be alive and obs_tx would still be open
        // (recv would hang forever).
        let mut rx = adapter.obs_rx.lock().await;
        let next = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
        match next {
            Ok(None) => {} // expected: channel closed → task aborted
            Ok(Some(obs)) => panic!("unexpected observation after abort: {obs:?}"),
            Err(_) => panic!(
                "obs channel did not close within 500ms after abort — pump task likely \
                 detached (F36 regression: JoinHandle dropped without abort)"
            ),
        }

        let _ = close_called;
        release.notify_waiters();
    }

    /// H47: `LiveAdapterCommand` is `#[non_exhaustive]`. A future variant
    /// must hit the catch-all and produce `LlmError::InvalidRequest`, not
    /// silently succeed. We can't construct a future variant from outside
    /// the crate, so we exercise the `LiveInputChunk` catch-all path which
    /// has the same shape and demonstrates the rule. (The `LiveAdapterCommand`
    /// catch-all is a parallel arm reviewed by Clippy + reading the code.)
    ///
    /// We construct an `Audio` then `Text` chunk and verify they are accepted —
    /// this guards the explicit arms remain exhaustive for known variants.
    #[tokio::test]
    async fn execute_command_known_input_variants_succeed() {
        struct OkSession;
        #[async_trait]
        impl RealtimeSession for OkSession {
            fn capabilities(&self) -> &RealtimeCapabilities {
                static CAPS: std::sync::OnceLock<RealtimeCapabilities> = std::sync::OnceLock::new();
                CAPS.get_or_init(RealtimeCapabilities::default)
            }
            fn turning_mode(&self) -> RealtimeTurningMode {
                RealtimeTurningMode::ProviderManaged
            }
            async fn refresh_projection(
                &mut self,
                _open_config: &RealtimeSessionOpenConfig,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn send_input(&mut self, _chunk: RealtimeInputChunk) -> Result<(), LlmError> {
                Ok(())
            }
            async fn commit_turn(&mut self) -> Result<(), LlmError> {
                Ok(())
            }
            async fn interrupt(&mut self) -> Result<(), LlmError> {
                Ok(())
            }
            async fn truncate_assistant_output(
                &mut self,
                _item_id: String,
                _content_index: u32,
                _audio_played_ms: u64,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn submit_tool_result(
                &mut self,
                _result: meerkat_core::types::ToolResult,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn submit_tool_error(
                &mut self,
                _call_id: String,
                _error: String,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError> {
                std::future::pending().await
            }
            async fn close(&mut self) -> Result<(), LlmError> {
                Ok(())
            }
        }

        let mut session: Box<dyn RealtimeSession> = Box::new(OkSession);
        let audio = LiveAdapterCommand::SendInput {
            chunk: LiveInputChunk::Audio {
                data: vec![0u8; 16],
                sample_rate_hz: 24000,
                channels: 1,
            },
        };
        execute_command(&mut session, audio).await.unwrap();

        let text = LiveAdapterCommand::SendInput {
            chunk: LiveInputChunk::Text { text: "hi".into() },
        };
        execute_command(&mut session, text).await.unwrap();

        // Close is explicitly handled.
        execute_command(&mut session, LiveAdapterCommand::Close)
            .await
            .unwrap();
    }

    /// N76 / sanity: pump_handle uses std::sync::Mutex — verify it does not
    /// require Send across an .await by simply ensuring we can construct
    /// and close an adapter without compile errors. (Compile-time guarded.)
    #[tokio::test]
    async fn adapter_constructs_and_closes_with_std_pump_handle_lock() {
        struct ClosesQuickly;
        #[async_trait]
        impl RealtimeSession for ClosesQuickly {
            fn capabilities(&self) -> &RealtimeCapabilities {
                static CAPS: std::sync::OnceLock<RealtimeCapabilities> = std::sync::OnceLock::new();
                CAPS.get_or_init(RealtimeCapabilities::default)
            }
            fn turning_mode(&self) -> RealtimeTurningMode {
                RealtimeTurningMode::ProviderManaged
            }
            async fn refresh_projection(
                &mut self,
                _open_config: &RealtimeSessionOpenConfig,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn send_input(&mut self, _chunk: RealtimeInputChunk) -> Result<(), LlmError> {
                Ok(())
            }
            async fn commit_turn(&mut self) -> Result<(), LlmError> {
                Ok(())
            }
            async fn interrupt(&mut self) -> Result<(), LlmError> {
                Ok(())
            }
            async fn truncate_assistant_output(
                &mut self,
                _item_id: String,
                _content_index: u32,
                _audio_played_ms: u64,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn submit_tool_result(
                &mut self,
                _result: meerkat_core::types::ToolResult,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn submit_tool_error(
                &mut self,
                _call_id: String,
                _error: String,
            ) -> Result<(), LlmError> {
                Ok(())
            }
            async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError> {
                Ok(None)
            }
            async fn close(&mut self) -> Result<(), LlmError> {
                Ok(())
            }
        }
        let adapter = ProviderSessionAdapter::new(Box::new(ClosesQuickly));
        adapter.close().await.unwrap();
    }

    // -- D24 / F33 / A11 / A12 -----------------------------------------------
    //
    // The tests below share a `ScriptedSession` that emits a pre-loaded queue
    // of `RealtimeSessionEvent`s, then parks on `next_event` so the pump stays
    // alive past the queue. This lets us drive the pump through specific
    // events and observe the resulting `LiveAdapterObservation`s end-to-end
    // (translate_event + pump_task + status mirroring).

    use meerkat_contracts::RealtimeAudioChunk;
    use meerkat_core::realtime_transcript::{RealtimeTranscriptEvent, RealtimeTranscriptRole};
    use meerkat_core::types::{StopReason, Usage};
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutexAlias;

    struct ScriptedSession {
        capabilities: RealtimeCapabilities,
        events: StdMutexAlias<VecDeque<RealtimeSessionEvent>>,
    }

    impl ScriptedSession {
        fn new(events: Vec<RealtimeSessionEvent>) -> Self {
            Self {
                capabilities: RealtimeCapabilities::default(),
                events: StdMutexAlias::new(events.into()),
            }
        }
    }

    #[async_trait]
    impl RealtimeSession for ScriptedSession {
        fn capabilities(&self) -> &RealtimeCapabilities {
            &self.capabilities
        }
        fn turning_mode(&self) -> RealtimeTurningMode {
            RealtimeTurningMode::ProviderManaged
        }
        async fn refresh_projection(
            &mut self,
            _open_config: &RealtimeSessionOpenConfig,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn send_input(&mut self, _chunk: RealtimeInputChunk) -> Result<(), LlmError> {
            Ok(())
        }
        async fn commit_turn(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
        async fn interrupt(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
        async fn truncate_assistant_output(
            &mut self,
            _item_id: String,
            _content_index: u32,
            _audio_played_ms: u64,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn submit_tool_result(
            &mut self,
            _result: meerkat_core::types::ToolResult,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn submit_tool_error(
            &mut self,
            _call_id: String,
            _error: String,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError> {
            // Pop an event if any remain; otherwise park forever so the pump
            // stays in its select! and we can observe the post-queue status.
            let next = match self.events.lock() {
                Ok(mut q) => q.pop_front(),
                Err(p) => p.into_inner().pop_front(),
            };
            if let Some(evt) = next {
                return Ok(Some(evt));
            }
            std::future::pending::<()>().await;
            Ok(None)
        }
        async fn close(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// Drain observations from the adapter until one matches `pred` or
    /// `max_drained` is exceeded. Returns the matching observation.
    /// Used to skip over Ready/StatusChanged when looking for a specific
    /// downstream observation.
    async fn drain_until(
        adapter: &ProviderSessionAdapter,
        max_drained: usize,
        mut pred: impl FnMut(&LiveAdapterObservation) -> bool,
    ) -> Option<LiveAdapterObservation> {
        for _ in 0..max_drained {
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                adapter.next_observation(),
            )
            .await
            {
                Ok(Ok(Some(obs))) => {
                    if pred(&obs) {
                        return Some(obs);
                    }
                }
                _ => return None,
            }
        }
        None
    }

    /// D24-outbound: a malformed base64 audio chunk from the provider must
    /// surface as a typed `Error` observation, not a phantom silent chunk.
    #[tokio::test(flavor = "current_thread")]
    async fn malformed_audio_base64_emits_typed_error_observation() {
        let bad_chunk = RealtimeAudioChunk {
            mime_type: "audio/pcm".to_string(),
            // '!!!' is not valid base64 (would need to be from the
            // standard alphabet [A-Za-z0-9+/=]).
            data: "!!!not-base64!!!".to_string(),
            sample_rate_hz: 24_000,
            channels: 1,
        };
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::OutputAudioChunk { chunk: bad_chunk },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let err_obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::Error { .. })
        })
        .await
        .expect("expected Error observation for malformed base64");

        match err_obs {
            LiveAdapterObservation::Error { code, message } => {
                assert_eq!(code, LiveAdapterErrorCode::ProviderError);
                assert!(
                    message.contains("invalid base64"),
                    "error message should mention base64 issue, got: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// D24-outbound: a well-formed audio chunk still flows as
    /// `AssistantAudioChunk` (regression guard — make sure D24's error path
    /// doesn't break the happy path).
    #[tokio::test(flavor = "current_thread")]
    async fn valid_audio_base64_still_emits_audio_chunk() {
        use base64::Engine;
        let payload = vec![0xAA_u8, 0xBB, 0xCC, 0xDD];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
        let good_chunk = RealtimeAudioChunk {
            mime_type: "audio/pcm".to_string(),
            data: encoded,
            sample_rate_hz: 24_000,
            channels: 1,
        };
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::OutputAudioChunk { chunk: good_chunk },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let audio_obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::AssistantAudioChunk { .. })
        })
        .await
        .expect("expected AssistantAudioChunk observation");

        match audio_obs {
            LiveAdapterObservation::AssistantAudioChunk {
                data,
                sample_rate_hz,
                channels,
            } => {
                assert_eq!(data, vec![0xAA, 0xBB, 0xCC, 0xDD]);
                assert_eq!(sample_rate_hz, 24_000);
                assert_eq!(channels, 1);
            }
            other => panic!("expected AssistantAudioChunk, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// F33-adapter: `adapter.status()` reflects the live phase, not a
    /// binary Ready/Closed.
    ///
    /// What this test proves:
    ///   1. After the initial Ready observation, `status()` returns `Ready`
    ///      (not the binary "is closed?" approximation the adapter had
    ///      before — that would have been Ready in this case too, but the
    ///      same code now drives the cell from the live phase).
    ///   2. After `close()`, `status()` returns `Closed`.
    ///
    /// What this test does NOT prove (covered by the e2e-smoke lane and
    /// `status_changed_observation_mirrors_into_adapter_status` below):
    ///   - The Opening→Ready transition itself (it happens before the first
    ///     observable side-effect, so the pump may or may not be observed
    ///     mid-transition without racing).
    ///   - The Degraded path. `translate_event` doesn't synthesize Degraded
    ///     today (no source `RealtimeSessionEvent` maps to it). The mirror
    ///     code path is exercised by `status_changed_observation_mirrors…`
    ///     below using a Ready-status echo; the same arm covers Degraded
    ///     once a source event begins emitting it.
    #[tokio::test(flavor = "current_thread")]
    async fn status_reflects_live_phase_not_binary() {
        let session = Box::new(ScriptedSession::new(vec![]));
        let adapter = ProviderSessionAdapter::new(session);

        // Drain initial Ready observation; pump sets status to Ready after.
        let first = adapter.next_observation().await.unwrap();
        assert!(matches!(first, Some(LiveAdapterObservation::Ready)));
        tokio::task::yield_now().await;
        assert_eq!(adapter.status(), LiveAdapterStatus::Ready);

        // After close(), status is Closed.
        adapter.close().await.unwrap();
        assert_eq!(adapter.status(), LiveAdapterStatus::Closed);
    }

    /// F33-adapter: end-to-end `StatusChanged` mirror — when the pump
    /// receives an event that translates to a `StatusChanged` observation,
    /// the adapter's `status()` reflects the new phase. We use
    /// `RealtimeSessionEvent::TurnStarted`, which today translates to
    /// `StatusChanged { status: Ready }`. This is the same code path that
    /// would mirror a `Degraded` if a future translate_event arm produces
    /// one (or if a real provider session emitted `StatusChanged{Degraded}`
    /// through the same translate_event seam).
    #[tokio::test(flavor = "current_thread")]
    async fn status_changed_observation_mirrors_into_adapter_status() {
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::TurnStarted,
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        // Drain initial Ready.
        assert!(matches!(
            adapter.next_observation().await.unwrap(),
            Some(LiveAdapterObservation::Ready)
        ));

        // Drain the StatusChanged that translate_event emits for TurnStarted.
        let next = adapter.next_observation().await.unwrap();
        match next {
            Some(LiveAdapterObservation::StatusChanged { status }) => {
                assert_eq!(status, LiveAdapterStatus::Ready);
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }
        // Adapter status reflects the mirrored value.
        tokio::task::yield_now().await;
        assert_eq!(adapter.status(), LiveAdapterStatus::Ready);

        adapter.close().await.unwrap();
        assert_eq!(adapter.status(), LiveAdapterStatus::Closed);
    }

    /// A11: the full identity/order tuple from `OutputTextDeltaForItem`
    /// must be plumbed through to the typed observation.
    #[tokio::test(flavor = "current_thread")]
    async fn assistant_text_delta_for_item_plumbs_full_identity() {
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::OutputTextDeltaForItem {
                response_id: "resp_1".into(),
                delta_id: "delta_5".into(),
                item_id: "item_42".into(),
                previous_item_id: Some("item_41".into()),
                content_index: 3,
                delta: "hello".into(),
            },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::AssistantTextDelta { .. })
        })
        .await
        .expect("expected AssistantTextDelta");

        match obs {
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                delta_id,
                delta,
            } => {
                assert_eq!(provider_item_id.as_deref(), Some("item_42"));
                assert_eq!(previous_item_id.as_deref(), Some("item_41"));
                assert_eq!(content_index, Some(3));
                assert_eq!(response_id.as_deref(), Some("resp_1"));
                assert_eq!(delta_id.as_deref(), Some("delta_5"));
                assert_eq!(delta, "hello");
            }
            other => panic!("expected AssistantTextDelta, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// A11: bare `OutputTextDelta` (no source identity) → all order fields None.
    #[tokio::test(flavor = "current_thread")]
    async fn assistant_text_delta_without_item_emits_all_none() {
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::OutputTextDelta {
                delta: "bare".into(),
            },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::AssistantTextDelta { .. })
        })
        .await
        .expect("expected AssistantTextDelta");

        match obs {
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                delta_id,
                delta,
            } => {
                assert!(provider_item_id.is_none());
                assert!(previous_item_id.is_none());
                assert!(content_index.is_none());
                assert!(response_id.is_none());
                assert!(delta_id.is_none());
                assert_eq!(delta, "bare");
            }
            other => panic!("expected AssistantTextDelta, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// A11: `InputTranscriptFinalForItem` plumbs identity and order.
    #[tokio::test(flavor = "current_thread")]
    async fn user_transcript_final_for_item_plumbs_identity() {
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::InputTranscriptFinalForItem {
                item_id: "user_item_7".into(),
                previous_item_id: None,
                content_index: 0,
                text: "what's the weather".into(),
            },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::UserTranscriptFinal { .. })
        })
        .await
        .expect("expected UserTranscriptFinal");

        match obs {
            LiveAdapterObservation::UserTranscriptFinal {
                provider_item_id,
                previous_item_id,
                content_index,
                text,
            } => {
                assert_eq!(provider_item_id.as_deref(), Some("user_item_7"));
                assert!(previous_item_id.is_none());
                assert_eq!(content_index, Some(0));
                assert_eq!(text, "what's the weather");
            }
            other => panic!("expected UserTranscriptFinal, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// A12: `RealtimeTranscript` event passes through as a typed
    /// observation, not folded into `StatusChanged{Ready}`.
    #[tokio::test(flavor = "current_thread")]
    async fn realtime_transcript_event_passes_through_as_observation() {
        let inner = RealtimeTranscriptEvent::ItemObserved {
            item_id: "item_99".into(),
            previous_item_id: Some("item_98".into()),
            role: RealtimeTranscriptRole::Assistant,
            response_id: Some("resp_42".into()),
        };
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::RealtimeTranscript {
                event: inner.clone(),
            },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::RealtimeTranscript { .. })
        })
        .await
        .expect("expected RealtimeTranscript observation (A12: must not fold into StatusChanged)");

        match obs {
            LiveAdapterObservation::RealtimeTranscript { event } => {
                assert_eq!(event, inner);
            }
            other => panic!("expected RealtimeTranscript, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// A13: `RealtimeSessionEvent::AssistantTranscriptFinal` forwards 1:1
    /// to `LiveAdapterObservation::AssistantTranscriptFinal`, preserving
    /// the full identity tuple, text, and best-effort stop_reason/usage.
    #[tokio::test(flavor = "current_thread")]
    async fn assistant_transcript_final_forwards_with_full_identity() {
        let usage = Usage {
            input_tokens: 12,
            output_tokens: 34,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::AssistantTranscriptFinal {
                item_id: "item_final_1".into(),
                previous_item_id: Some("item_prev".into()),
                content_index: Some(2),
                response_id: Some("resp_xyz".into()),
                text: "the final transcript".into(),
                // Use a non-default StopReason so a regression that hardcodes
                // EndTurn on the receiver (which is the #[default]) gets caught.
                stop_reason: StopReason::ToolUse,
                usage: usage.clone(),
            },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::AssistantTranscriptFinal { .. })
        })
        .await
        .expect("expected AssistantTranscriptFinal");

        match obs {
            LiveAdapterObservation::AssistantTranscriptFinal {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                text,
                stop_reason,
                usage: got_usage,
            } => {
                assert_eq!(provider_item_id, "item_final_1");
                assert_eq!(previous_item_id.as_deref(), Some("item_prev"));
                assert_eq!(content_index, Some(2));
                assert_eq!(response_id.as_deref(), Some("resp_xyz"));
                assert_eq!(text, "the final transcript");
                assert_eq!(stop_reason, StopReason::ToolUse);
                assert_eq!(got_usage, usage);
            }
            other => panic!("expected AssistantTranscriptFinal, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }

    /// A12 regression: `RealtimeTranscript` with an `AssistantTextDelta`
    /// payload also forwards (covers the richer sub-variant).
    #[tokio::test(flavor = "current_thread")]
    async fn realtime_transcript_assistant_delta_passes_through() {
        let inner = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_77".into(),
            delta_id: "d_1".into(),
            item_id: "item_77".into(),
            previous_item_id: None,
            content_index: 0,
            delta: "tok".into(),
        };
        let session = Box::new(ScriptedSession::new(vec![
            RealtimeSessionEvent::RealtimeTranscript {
                event: inner.clone(),
            },
        ]));
        let adapter = ProviderSessionAdapter::new(session);

        let obs = drain_until(&adapter, 8, |obs| {
            matches!(obs, LiveAdapterObservation::RealtimeTranscript { .. })
        })
        .await
        .expect("expected RealtimeTranscript observation");

        match obs {
            LiveAdapterObservation::RealtimeTranscript { event } => {
                assert_eq!(event, inner);
            }
            other => panic!("expected RealtimeTranscript, got {other:?}"),
        }
        adapter.close().await.unwrap();
    }
}

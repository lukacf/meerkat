//! `RealtimeMockServer` — scripted mock that impersonates an OpenAI
//! Realtime session without requiring live credentials or a network.
//!
//! ## Why
//!
//! Tests that exercise the realtime attachment lifecycle (turn-driven
//! realtime, peer-response-triggered turns, subscriber streams over a
//! realtime session) today run only against live OpenAI credentials in
//! `e2e-live`. The s71 turn-8 regression proved that this is not enough:
//! cross-axis bugs need a deterministic harness.
//!
//! ## Shape
//!
//! The mock implements `meerkat_llm_core::realtime_session::{RealtimeSession,
//! RealtimeSessionFactory}` directly. Input frames are recorded so the test
//! can assert on them; output events are scripted per-scenario and emitted
//! in response to `next_event` calls.
//!
//! A `RealtimeMockServer` is a container for a registered set of scenarios
//! and the factory that the production code consumes. It is NOT a real WS
//! server — the `realtime` side of Meerkat's provider contract lives at the
//! `RealtimeSession` trait, not at the socket, so the "server" here is the
//! authoritative scripting authority that the factory opens sessions from.
//!
//! If a future test needs raw WS framing (e.g. to exercise `oai-rt-rs`
//! parsing), the mock can be extended with a TCP listener that serializes
//! the scripted events as `response.output_text.delta` / `response.done`
//! JSON frames; for now, every consumer talks through the trait and we
//! keep the harness zero-overhead.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_contracts::{
    RealtimeAudioChunk, RealtimeCapabilities, RealtimeInputChunk, RealtimeInputKind,
    RealtimeOutputKind, RealtimeTurningMode,
};
use meerkat_core::ToolResult;
use meerkat_llm_core::LlmError;
use meerkat_llm_core::realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionEvent, RealtimeSessionFactory,
    RealtimeSessionOpenConfig,
};
use tokio::sync::Mutex;

/// One scripted event the mock emits when the test harness asks for the
/// next event from an open session.
#[derive(Debug, Clone)]
pub enum ScriptedEvent {
    OutputTextDelta(String),
    TurnStarted,
    TurnCommitted,
    TurnCompleted {
        stop_reason: meerkat_core::StopReason,
    },
    Interrupted,
    /// Simulate the provider end-of-stream: `next_event` will return
    /// `Ok(None)` for all subsequent polls.
    End,
}

impl ScriptedEvent {
    fn into_session_event(self) -> Option<Result<Option<RealtimeSessionEvent>, LlmError>> {
        match self {
            ScriptedEvent::OutputTextDelta(delta) => {
                Some(Ok(Some(RealtimeSessionEvent::OutputTextDelta { delta })))
            }
            ScriptedEvent::TurnStarted => Some(Ok(Some(RealtimeSessionEvent::TurnStarted))),
            ScriptedEvent::TurnCommitted => Some(Ok(Some(RealtimeSessionEvent::TurnCommitted))),
            ScriptedEvent::TurnCompleted { stop_reason } => {
                Some(Ok(Some(RealtimeSessionEvent::TurnCompleted {
                    response_id: "mock_response".to_string(),
                    stop_reason,
                    usage: Default::default(),
                })))
            }
            ScriptedEvent::Interrupted => Some(Ok(Some(RealtimeSessionEvent::Interrupted {
                response_id: Some("mock_response".to_string()),
            }))),
            ScriptedEvent::End => Some(Ok(None)),
        }
    }
}

/// A pre-scripted provider conversation. The scenario is consumed in order
/// by the session. Additional events can be pushed onto the scenario while
/// the session is live (via the server handle) so tests can simulate
/// provider-side pushes that arrive after a specific input frame has been
/// observed.
#[derive(Debug, Clone, Default)]
pub struct ScriptedScenario {
    pub events: Vec<ScriptedEvent>,
    /// Optional capability override. If `None`, the mock advertises a
    /// conservative baseline (text in/out, provider-managed turns, no
    /// video, no audio format announcement).
    pub capabilities: Option<RealtimeCapabilities>,
}

impl ScriptedScenario {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event(mut self, event: ScriptedEvent) -> Self {
        self.events.push(event);
        self
    }

    pub fn with_events(mut self, events: impl IntoIterator<Item = ScriptedEvent>) -> Self {
        self.events.extend(events);
        self
    }

    pub fn with_capabilities(mut self, capabilities: RealtimeCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }
}

fn baseline_capabilities() -> RealtimeCapabilities {
    RealtimeCapabilities {
        input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
        output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
        turning_modes: vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ],
        interrupt_supported: true,
        transcript_supported: true,
        tool_lifecycle_events_supported: false,
        video_supported: false,
        audio_input_format: None,
        audio_output_format: None,
    }
}

/// Recording bag every session shares with the server so tests can inspect
/// what was sent provider-wards.
#[derive(Debug, Default)]
pub struct SessionRecording {
    pub inputs: Vec<RealtimeInputChunk>,
    pub tool_results: Vec<ToolResult>,
    pub tool_errors: Vec<(String, String)>,
    pub commits: u32,
    pub interrupts: u32,
    pub truncations: u32,
    pub refresh_projections: u32,
    pub closed: bool,
}

/// A scripted mock session. Consumers normally receive this as
/// `Box<dyn RealtimeSession>` from the factory.
pub struct RealtimeMockSession {
    capabilities: RealtimeCapabilities,
    turning_mode: RealtimeTurningMode,
    queued: VecDeque<ScriptedEvent>,
    recording: Arc<Mutex<SessionRecording>>,
}

impl RealtimeMockSession {
    pub fn new(
        scenario: ScriptedScenario,
        turning_mode: RealtimeTurningMode,
        recording: Arc<Mutex<SessionRecording>>,
    ) -> Self {
        let capabilities = scenario.capabilities.unwrap_or_else(baseline_capabilities);
        let queued: VecDeque<ScriptedEvent> = scenario.events.into_iter().collect();
        Self {
            capabilities,
            turning_mode,
            queued,
            recording,
        }
    }
}

#[async_trait]
impl RealtimeSession for RealtimeMockSession {
    fn capabilities(&self) -> &RealtimeCapabilities {
        &self.capabilities
    }

    fn turning_mode(&self) -> RealtimeTurningMode {
        self.turning_mode
    }

    async fn refresh_projection(
        &mut self,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<(), LlmError> {
        self.recording.lock().await.refresh_projections += 1;
        Ok(())
    }

    async fn send_input(&mut self, chunk: RealtimeInputChunk) -> Result<(), LlmError> {
        self.recording.lock().await.inputs.push(chunk);
        Ok(())
    }

    async fn commit_turn(&mut self) -> Result<(), LlmError> {
        self.recording.lock().await.commits += 1;
        Ok(())
    }

    async fn interrupt(&mut self) -> Result<(), LlmError> {
        self.recording.lock().await.interrupts += 1;
        Ok(())
    }

    async fn truncate_assistant_output(
        &mut self,
        _item_id: String,
        _content_index: u32,
        _audio_played_ms: u64,
    ) -> Result<(), LlmError> {
        self.recording.lock().await.truncations += 1;
        Ok(())
    }

    async fn submit_tool_result(&mut self, result: ToolResult) -> Result<(), LlmError> {
        self.recording.lock().await.tool_results.push(result);
        Ok(())
    }

    async fn submit_tool_error(&mut self, call_id: String, error: String) -> Result<(), LlmError> {
        self.recording
            .lock()
            .await
            .tool_errors
            .push((call_id, error));
        Ok(())
    }

    async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError> {
        match self.queued.pop_front() {
            Some(event) => event.into_session_event().unwrap_or(Ok(None)),
            None => Ok(None),
        }
    }

    async fn close(&mut self) -> Result<(), LlmError> {
        self.recording.lock().await.closed = true;
        Ok(())
    }
}

/// Factory that hands out pre-scripted mock sessions in FIFO order.
///
/// Callers register scenarios via the `RealtimeMockServer` before the code
/// under test opens a session. Every `open_session` call consumes one
/// scenario. If there are no more scenarios queued, the factory returns
/// `LlmError::InvalidRequest` so the test fails loudly rather than hanging.
pub struct RealtimeMockSessionFactory {
    capabilities: RealtimeCapabilities,
    queued: Mutex<VecDeque<ScriptedScenario>>,
    recordings: Mutex<Vec<Arc<Mutex<SessionRecording>>>>,
}

impl RealtimeMockSessionFactory {
    pub fn new(capabilities: RealtimeCapabilities) -> Self {
        Self {
            capabilities,
            queued: Mutex::new(VecDeque::new()),
            recordings: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RealtimeSessionFactory for RealtimeMockSessionFactory {
    fn capabilities(&self) -> RealtimeCapabilities {
        self.capabilities.clone()
    }

    async fn open_session(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        let scenario = match self.queued.lock().await.pop_front() {
            Some(scenario) => scenario,
            None => {
                return Err(LlmError::InvalidRequest {
                    message: "RealtimeMockSessionFactory: no scenario queued".to_string(),
                });
            }
        };
        let recording = Arc::new(Mutex::new(SessionRecording::default()));
        self.recordings.lock().await.push(Arc::clone(&recording));
        Ok(Box::new(RealtimeMockSession::new(
            scenario,
            open_config.turning_mode,
            recording,
        )))
    }

    async fn attach_external_session(
        &self,
        _target: &RealtimeExternalSessionTarget,
        _turning_mode: RealtimeTurningMode,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        Err(LlmError::InvalidRequest {
            message: "RealtimeMockSessionFactory does not support external attach".to_string(),
        })
    }
}

/// Test-facing server. Owns the factory and lets tests register scenarios
/// and inspect per-session recordings.
pub struct RealtimeMockServer {
    factory: Arc<RealtimeMockSessionFactory>,
}

impl RealtimeMockServer {
    pub fn new() -> Self {
        Self::with_capabilities(baseline_capabilities())
    }

    pub fn with_capabilities(capabilities: RealtimeCapabilities) -> Self {
        Self {
            factory: Arc::new(RealtimeMockSessionFactory::new(capabilities)),
        }
    }

    pub fn factory(&self) -> Arc<dyn RealtimeSessionFactory> {
        Arc::clone(&self.factory) as Arc<dyn RealtimeSessionFactory>
    }

    /// Queue another scripted scenario. Scenarios are handed out FIFO;
    /// call this once for every `open_session` the code under test will
    /// perform.
    pub async fn enqueue(&self, scenario: ScriptedScenario) {
        self.factory.queued.lock().await.push_back(scenario);
    }

    /// Snapshot of the nth session's recording (0-indexed in open-order).
    pub async fn recording_for(&self, idx: usize) -> Option<Arc<Mutex<SessionRecording>>> {
        self.factory
            .recordings
            .lock()
            .await
            .get(idx)
            .map(Arc::clone)
    }

    /// Number of sessions the factory has opened so far.
    pub async fn opened_sessions(&self) -> usize {
        self.factory.recordings.lock().await.len()
    }
}

impl Default for RealtimeMockServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_contracts::RealtimeTextChunk;
    use meerkat_core::StopReason;

    fn text_chunk(text: &str) -> RealtimeInputChunk {
        RealtimeInputChunk::TextChunk(RealtimeTextChunk {
            text: text.to_string(),
        })
    }

    #[tokio::test]
    async fn scripted_scenario_replays_in_order() {
        let server = RealtimeMockServer::new();
        server
            .enqueue(
                ScriptedScenario::new()
                    .with_event(ScriptedEvent::TurnStarted)
                    .with_event(ScriptedEvent::OutputTextDelta("hello".into()))
                    .with_event(ScriptedEvent::TurnCompleted {
                        stop_reason: StopReason::EndTurn,
                    })
                    .with_event(ScriptedEvent::End),
            )
            .await;

        let factory = server.factory();
        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            meerkat_core::SessionLlmIdentity {
                model: "mock-realtime".into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            Vec::new(),
            Vec::new(),
        );

        let mut session = factory.open_session(&open_config).await.unwrap();

        assert!(matches!(
            session.next_event().await.unwrap(),
            Some(RealtimeSessionEvent::TurnStarted)
        ));
        assert!(matches!(
            session.next_event().await.unwrap(),
            Some(RealtimeSessionEvent::OutputTextDelta { .. })
        ));
        assert!(matches!(
            session.next_event().await.unwrap(),
            Some(RealtimeSessionEvent::TurnCompleted { .. })
        ));
        assert!(session.next_event().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn inputs_are_recorded() {
        let server = RealtimeMockServer::new();
        server
            .enqueue(ScriptedScenario::new().with_event(ScriptedEvent::End))
            .await;

        let factory = server.factory();
        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            meerkat_core::SessionLlmIdentity {
                model: "mock-realtime".into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            Vec::new(),
            Vec::new(),
        );

        let mut session = factory.open_session(&open_config).await.unwrap();
        session
            .send_input(text_chunk("user says hi"))
            .await
            .unwrap();
        session.commit_turn().await.unwrap();
        session.close().await.unwrap();

        let recording = server.recording_for(0).await.expect("one session");
        let recording = recording.lock().await;
        assert_eq!(recording.inputs.len(), 1);
        assert_eq!(recording.commits, 1);
        assert!(recording.closed);
    }

    #[tokio::test]
    async fn empty_queue_fails_loudly() {
        let server = RealtimeMockServer::new();
        let factory = server.factory();
        let open_config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            meerkat_core::SessionLlmIdentity {
                model: "mock-realtime".into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            Vec::new(),
            Vec::new(),
        );
        match factory.open_session(&open_config).await {
            Ok(_) => panic!("expected InvalidRequest, got a session"),
            Err(err) => assert!(matches!(err, LlmError::InvalidRequest { .. })),
        }
    }

    #[tokio::test]
    async fn external_attach_is_unsupported() {
        let server = RealtimeMockServer::new();
        let factory = server.factory();
        let target = RealtimeExternalSessionTarget::new("sess_1").unwrap();
        match factory
            .attach_external_session(&target, RealtimeTurningMode::ProviderManaged)
            .await
        {
            Ok(_) => panic!("expected InvalidRequest, got a session"),
            Err(err) => assert!(matches!(err, LlmError::InvalidRequest { .. })),
        }
    }

    // Suppress unused-warning for import that only the wider tests need.
    #[test]
    fn audio_chunk_type_is_reachable() {
        // RealtimeAudioChunk is re-exported via meerkat_contracts; assert
        // the import wiring stays alive so cell tests can build audio fixtures.
        let _sizeof = std::mem::size_of::<RealtimeAudioChunk>();
    }
}

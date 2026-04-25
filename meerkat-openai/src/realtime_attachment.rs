//! OpenAI live attachment companion for Meerkat.
//!
//! Provider transport mechanics live in `meerkat-client::openai_live`. This
//! module owns provider-specific orchestration that coordinates those
//! mechanics with runtime-owned live attachment authority.

#![allow(clippy::large_futures)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::live::{
    OpenAiLiveCallTarget, OpenAiLiveServerEvent, OpenAiLiveSession, OpenAiLiveSessionFactory,
    openai_live_function_call_error_event, openai_live_function_call_success_events,
};
use async_trait::async_trait;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::types::{SessionId, ToolCall};
use meerkat_llm_core::LlmError;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use meerkat_runtime::{
    Input, MeerkatMachine, PromptInput, RealtimeAttachmentSignalAuthority,
    RealtimeAttachmentStatus, RuntimeDriverError, SessionServiceRuntimeExt,
};

enum OpenAiLiveTaskState {
    Connecting,
    Attached(JoinHandle<()>),
}

/// Runtime-to-session seam for provider-originated live tool calls.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait RealtimeAttachmentToolDispatchHost: Send + Sync {
    async fn dispatch_external_tool_call(
        &self,
        session_id: &SessionId,
        call: ToolCall,
    ) -> Result<ToolDispatchOutcome, RuntimeDriverError>;
}

/// Client-owned mechanical orchestrator for OpenAI live attachments.
///
/// Semantic truth remains in `MeerkatMachine`: this helper only manages the
/// provider session handle and its background event pump.
pub struct OpenAiRealtimeAttachmentOrchestrator {
    runtime: Arc<MeerkatMachine>,
    factory: Arc<dyn OpenAiLiveSessionFactory>,
    tool_dispatch_host: Arc<dyn RealtimeAttachmentToolDispatchHost>,
    tasks: Arc<Mutex<HashMap<SessionId, OpenAiLiveTaskState>>>,
}

impl OpenAiRealtimeAttachmentOrchestrator {
    /// Create a new orchestrator using the canonical runtime adapter and a
    /// provider-specific OpenAI live session factory.
    pub fn new(
        runtime: Arc<MeerkatMachine>,
        factory: Arc<dyn OpenAiLiveSessionFactory>,
        tool_dispatch_host: Arc<dyn RealtimeAttachmentToolDispatchHost>,
    ) -> Self {
        Self {
            runtime,
            factory,
            tool_dispatch_host,
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Ensure the runtime-owned capability-driven realtime binding has an
    /// OpenAI provider session and mark it ready once connected.
    pub async fn ensure_attached_for_capable_session(
        &self,
        session_id: &SessionId,
        target: &OpenAiLiveCallTarget,
    ) -> Result<(), RuntimeDriverError> {
        {
            let mut tasks = self.tasks.lock().await;
            if tasks.contains_key(session_id) {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "openai live attachment already orchestrated for session {session_id}"
                    ),
                });
            }
            tasks.insert(session_id.clone(), OpenAiLiveTaskState::Connecting);
        }

        let authority = match self
            .runtime
            .apply_capability_driven_realtime_transport(session_id)
            .await
        {
            Ok(Some(authority)) => authority,
            Ok(None) => match self
                .runtime
                .current_realtime_attachment_authority(session_id)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    self.tasks.lock().await.remove(session_id);
                    return Err(error);
                }
            },
            Err(error) => {
                self.tasks.lock().await.remove(session_id);
                return Err(error);
            }
        };

        let session = match self.factory.attach_to_call(target).await {
            Ok(session) => session,
            Err(error) => {
                self.tasks.lock().await.remove(session_id);
                let _ = self
                    .runtime
                    .require_realtime_attachment_reattach_for_authority(authority)
                    .await;
                return Err(map_openai_live_error(error));
            }
        };

        if let Err(error) = self
            .runtime
            .publish_realtime_attachment_signal(
                authority.clone(),
                RealtimeAttachmentStatus::BindingReady,
            )
            .await
        {
            self.tasks.lock().await.remove(session_id);
            let _ = self
                .runtime
                .require_realtime_attachment_reattach_for_authority(authority)
                .await;
            return Err(error);
        }

        let runtime = Arc::clone(&self.runtime);
        let tasks = Arc::clone(&self.tasks);
        let tool_dispatch_host = Arc::clone(&self.tool_dispatch_host);
        let session_id_owned = session_id.clone();
        let handle = tokio::spawn(async move {
            let _ = run_openai_live_event_loop(
                runtime.clone(),
                session_id_owned.clone(),
                tool_dispatch_host,
                authority,
                session,
            )
            .await;
            let mut tasks = tasks.lock().await;
            tasks.remove(&session_id_owned);
        });

        let mut tasks = self.tasks.lock().await;
        tasks.insert(session_id.clone(), OpenAiLiveTaskState::Attached(handle));
        Ok(())
    }

    /// Stop the currently orchestrated provider session. Runtime attachment
    /// lifecycle remains capability-driven by `MeerkatMachine`.
    pub async fn stop_provider_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let task = self.tasks.lock().await.remove(session_id);
        if let Some(OpenAiLiveTaskState::Attached(handle)) = task {
            handle.abort();
        }
        Ok(())
    }
}

fn map_openai_live_error(error: LlmError) -> RuntimeDriverError {
    match error {
        LlmError::InvalidRequest { message }
        | LlmError::AuthenticationFailed { message }
        | LlmError::ModelNotFound { model: message }
        | LlmError::ContentFiltered { reason: message } => {
            RuntimeDriverError::ValidationFailed { reason: message }
        }
        other => RuntimeDriverError::Internal(format!("openai live attach failed: {other}")),
    }
}

async fn run_openai_live_event_loop(
    runtime: Arc<MeerkatMachine>,
    session_id: SessionId,
    tool_dispatch_host: Arc<dyn RealtimeAttachmentToolDispatchHost>,
    authority: RealtimeAttachmentSignalAuthority,
    mut session: Box<dyn OpenAiLiveSession>,
) -> Result<(), RuntimeDriverError> {
    loop {
        match session.next_event().await {
            Ok(Some(event)) => {
                handle_openai_live_event(
                    &runtime,
                    tool_dispatch_host.as_ref(),
                    &session_id,
                    session.as_mut(),
                    event,
                )
                .await?;
            }
            Ok(None) => {
                return match runtime
                    .require_realtime_attachment_reattach_for_authority(authority)
                    .await
                {
                    Ok(()) | Err(RuntimeDriverError::ValidationFailed { .. }) => Ok(()),
                    Err(error) => Err(error),
                };
            }
            Err(error) => {
                let _ = runtime
                    .require_realtime_attachment_reattach_for_authority(authority)
                    .await;
                return Err(map_openai_live_error(error));
            }
        }
    }
}

async fn handle_openai_live_event(
    runtime: &Arc<MeerkatMachine>,
    tool_dispatch_host: &dyn RealtimeAttachmentToolDispatchHost,
    session_id: &SessionId,
    session: &mut dyn OpenAiLiveSession,
    event: OpenAiLiveServerEvent,
) -> Result<(), RuntimeDriverError> {
    match event {
        OpenAiLiveServerEvent::InputAudioTranscriptionCompleted { transcript, .. } => {
            runtime
                .accept_input(
                    session_id,
                    Input::Prompt(PromptInput::new(transcript, None)),
                )
                .await?;
            Ok(())
        }
        OpenAiLiveServerEvent::InputAudioBufferSpeechStarted { .. } => {
            match runtime.interrupt_current_run(session_id).await {
                Ok(()) | Err(RuntimeDriverError::NotReady { .. }) => Ok(()),
                Err(error) => Err(error),
            }
        }
        OpenAiLiveServerEvent::ResponseFunctionCallArgumentsDone {
            call_id,
            name,
            arguments,
            ..
        } => {
            let arguments =
                serde_json::from_str(&arguments).unwrap_or(serde_json::Value::String(arguments));
            let call = ToolCall::new(call_id.clone(), name, arguments);
            match tool_dispatch_host
                .dispatch_external_tool_call(session_id, call)
                .await
            {
                Ok(outcome) => {
                    let output = outcome.result.text_content();
                    let events = openai_live_function_call_success_events(call_id, output);
                    for event in events {
                        session
                            .send_raw(event)
                            .await
                            .map_err(map_openai_live_error)?;
                    }
                    Ok(())
                }
                Err(error) => {
                    session
                        .send_raw(openai_live_function_call_error_event(
                            call_id,
                            error.to_string(),
                        ))
                        .await
                        .map_err(map_openai_live_error)?;
                    Ok(())
                }
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::live::{
        OpenAiLiveCallTarget, OpenAiLiveClientEvent, OpenAiLiveServerEvent, OpenAiLiveSession,
        OpenAiLiveSessionFactory,
    };
    use async_trait::async_trait;
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_core::types::{SessionId, ToolCall, ToolResult};
    use meerkat_core::{Provider, SessionLlmIdentity, ToolDispatchOutcome};
    use serde_json::json;
    use tokio::sync::{Mutex, Notify};

    use super::{OpenAiRealtimeAttachmentOrchestrator, RealtimeAttachmentToolDispatchHost};
    use meerkat_llm_core::LlmError;
    use meerkat_runtime::{
        HydratedSessionLlmState, Input, MeerkatMachine, PromptInput, RealtimeAttachmentStatus,
        ResolvedSessionLlmReconfigure, RuntimeDriverError, SessionLlmCapabilitySurface,
        SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost, SessionLlmReconfigureRequest,
        SessionServiceRuntimeExt,
    };

    struct NoopExecutor;

    #[async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::without_terminal(
                RunBoundaryReceipt {
                    run_id: RunId::new(),
                    boundary: RunApplyBoundary::Immediate,
                    contributing_input_ids: Vec::new(),
                    conversation_digest: Some("digest".to_string()),
                    message_count: 0,
                    sequence: 1,
                },
                None,
            ))
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    struct RecordingExecutor {
        applied_prompts: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.applied_prompts
                .lock()
                .await
                .push(primitive.extract_content_input().text_content());
            Ok(CoreApplyOutput::without_terminal(
                RunBoundaryReceipt {
                    run_id: RunId::new(),
                    boundary: RunApplyBoundary::Immediate,
                    contributing_input_ids: Vec::new(),
                    conversation_digest: Some("digest".to_string()),
                    message_count: 0,
                    sequence: 1,
                },
                None,
            ))
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    struct FakeSession {
        events: VecDeque<OpenAiLiveServerEvent>,
        event_gate: Option<Arc<Notify>>,
        close_gate: Option<Arc<Notify>>,
        sent_events: Arc<Mutex<Vec<OpenAiLiveClientEvent>>>,
    }

    #[async_trait]
    impl OpenAiLiveSession for FakeSession {
        async fn send_raw(&mut self, event: OpenAiLiveClientEvent) -> Result<(), LlmError> {
            self.sent_events.lock().await.push(event);
            Ok(())
        }

        async fn next_event(&mut self) -> Result<Option<OpenAiLiveServerEvent>, LlmError> {
            if let Some(gate) = self.event_gate.take() {
                gate.notified().await;
            }
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            if let Some(gate) = self.close_gate.take() {
                gate.notified().await;
            }
            Ok(None)
        }
    }

    struct FakeFactory {
        sessions: Mutex<VecDeque<Result<Box<dyn OpenAiLiveSession>, LlmError>>>,
    }

    #[async_trait]
    impl OpenAiLiveSessionFactory for FakeFactory {
        async fn open_session(
            &self,
            _open_config: &meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<Box<dyn OpenAiLiveSession>, LlmError> {
            Err(LlmError::InvalidRequest {
                message: "attachment tests do not use provider-created sessions".to_string(),
            })
        }

        async fn attach_to_call(
            &self,
            _target: &OpenAiLiveCallTarget,
        ) -> Result<Box<dyn OpenAiLiveSession>, LlmError> {
            self.sessions
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| panic!("fake factory missing queued session"))
        }
    }

    struct FakeToolDispatchHost {
        calls: Arc<Mutex<Vec<(SessionId, ToolCall)>>>,
        outcomes: Mutex<VecDeque<Result<ToolDispatchOutcome, RuntimeDriverError>>>,
    }

    #[async_trait]
    impl RealtimeAttachmentToolDispatchHost for FakeToolDispatchHost {
        async fn dispatch_external_tool_call(
            &self,
            session_id: &SessionId,
            call: ToolCall,
        ) -> Result<ToolDispatchOutcome, RuntimeDriverError> {
            self.calls.lock().await.push((session_id.clone(), call));
            self.outcomes.lock().await.pop_front().unwrap_or_else(|| {
                Err(RuntimeDriverError::Internal(
                    "missing fake tool dispatch outcome".to_string(),
                ))
            })
        }
    }

    struct RealtimeTestReconfigureHost {
        identity: SessionLlmIdentity,
        capability_surface: SessionLlmCapabilitySurface,
    }

    #[async_trait]
    impl SessionLlmReconfigureHost for RealtimeTestReconfigureHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
            Ok(HydratedSessionLlmState {
                current_identity: self.identity.clone(),
                current_visibility_state: Default::default(),
                current_capability_surface: Some(self.capability_surface.clone()),
                capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
                base_tool_names: Default::default(),
            })
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &SessionLlmReconfigureRequest,
            _current_identity: &SessionLlmIdentity,
        ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
            Ok(ResolvedSessionLlmReconfigure {
                target_identity: self.identity.clone(),
                target_capability_surface: self.capability_surface.clone(),
            })
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _identity: &SessionLlmIdentity,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &SessionId,
            _visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn persist_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn discard_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }
    }

    fn realtime_test_capability_surface() -> SessionLlmCapabilitySurface {
        SessionLlmCapabilitySurface {
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            inline_video: false,
            vision: false,
            image_tool_results: false,
            supports_web_search: false,
            realtime: true,
            call_timeout_secs: None,
        }
    }

    fn install_realtime_test_reconfigure_host(runtime: &Arc<MeerkatMachine>) {
        runtime.set_session_llm_reconfigure_host(Arc::new(RealtimeTestReconfigureHost {
            identity: SessionLlmIdentity {
                model: "gpt-realtime".to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                connection_ref: None,
            },
            capability_surface: realtime_test_capability_surface(),
        }));
    }

    async fn runtime_with_live_executor() -> (Arc<MeerkatMachine>, SessionId) {
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        install_realtime_test_reconfigure_host(&runtime);
        let session_id = SessionId::new();
        runtime
            .register_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
            .await;
        runtime
            .project_realtime_attachment_intent(&session_id, true)
            .await
            .expect("intent projection should succeed");
        (runtime, session_id)
    }

    fn noop_tool_dispatch_host() -> Arc<FakeToolDispatchHost> {
        Arc::new(FakeToolDispatchHost {
            calls: Arc::new(Mutex::new(Vec::new())),
            outcomes: Mutex::new(
                VecDeque::<Result<ToolDispatchOutcome, RuntimeDriverError>>::new(),
            ),
        })
    }

    async fn runtime_with_recording_executor()
    -> (Arc<MeerkatMachine>, SessionId, Arc<Mutex<Vec<String>>>) {
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        install_realtime_test_reconfigure_host(&runtime);
        let session_id = SessionId::new();
        let applied_prompts = Arc::new(Mutex::new(Vec::new()));
        runtime
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    applied_prompts: applied_prompts.clone(),
                }),
            )
            .await;
        runtime
            .project_realtime_attachment_intent(&session_id, true)
            .await
            .expect("intent projection should succeed");
        (runtime, session_id, applied_prompts)
    }

    async fn runtime_with_blocking_executor() -> (
        Arc<MeerkatMachine>,
        SessionId,
        Arc<AtomicUsize>,
        Arc<Notify>,
        Arc<Notify>,
        Arc<Notify>,
    ) {
        struct BlockingExecutor {
            cancel_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::CancelCurrentRun { .. }) {
                    self.cancel_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let runtime = Arc::new(MeerkatMachine::ephemeral());
        install_realtime_test_reconfigure_host(&runtime);
        let session_id = SessionId::new();
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());
        runtime
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    cancel_calls: cancel_calls.clone(),
                    apply_started: apply_started.clone(),
                    apply_finished: apply_finished.clone(),
                    allow_finish: allow_finish.clone(),
                }),
            )
            .await;
        runtime
            .project_realtime_attachment_intent(&session_id, true)
            .await
            .expect("intent projection should succeed");
        (
            runtime,
            session_id,
            cancel_calls,
            apply_started,
            apply_finished,
            allow_finish,
        )
    }

    #[tokio::test]
    async fn openai_live_orchestrator_attach_marks_binding_ready() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::new(),
                event_gate: None,
                close_gate: Some(hold_open.clone()),
                sent_events,
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target = OpenAiLiveCallTarget::new("call_attach_ready")
            .expect("non-empty call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::BindingReady);

        hold_open.notify_waiters();
    }

    #[tokio::test]
    async fn openai_live_orchestrator_attach_failure_marks_reattach_required() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Err(LlmError::InvalidRequest {
                message: "call rejected".to_string(),
            })])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target =
            OpenAiLiveCallTarget::new("call_attach_fail").expect("call target should succeed");

        let error = orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect_err("attach should surface provider rejection");
        assert!(matches!(error, RuntimeDriverError::ValidationFailed { .. }));

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::ReattachRequired);
    }

    #[tokio::test]
    async fn openai_live_orchestrator_remote_disconnect_requires_reattach() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::from([OpenAiLiveServerEvent::ResponseCreated {
                    event_id: "evt_1".to_string(),
                    response: serde_json::from_value(json!({
                            "id": "resp_1",
                            "object": "realtime.response",
                            "status": "in_progress",
                            "status_details": null,
                            "output": [],
                            "usage": null
                    }))
                    .expect("response payload should deserialize"),
                }]),
                event_gate: None,
                close_gate: None,
                sent_events,
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target =
            OpenAiLiveCallTarget::new("call_disconnect").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        tokio::time::sleep(Duration::from_millis(25)).await;

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::ReattachRequired);
    }

    #[tokio::test]
    async fn openai_live_orchestrator_stop_provider_session_keeps_runtime_binding() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::new(),
                event_gate: None,
                close_gate: Some(hold_open.clone()),
                sent_events,
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target = OpenAiLiveCallTarget::new("call_detach").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");
        orchestrator
            .stop_provider_session(&session_id)
            .await
            .expect("detach should succeed");

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::BindingReady);

        hold_open.notify_waiters();
    }

    #[tokio::test]
    async fn openai_live_orchestrator_stale_disconnect_does_not_override_replacement() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::new(),
                event_gate: None,
                close_gate: Some(hold_open.clone()),
                sent_events,
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target =
            OpenAiLiveCallTarget::new("call_stale_disconnect").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        runtime
            .replace_realtime_attachment(&session_id)
            .await
            .expect("replacement should mint fresh authority");
        hold_open.notify_waiters();
        tokio::time::sleep(Duration::from_millis(25)).await;

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::ReplacementPending);
    }

    #[tokio::test]
    async fn openai_live_orchestrator_commits_final_transcripts_as_runtime_prompts() {
        let (runtime, session_id, applied_prompts) = runtime_with_recording_executor().await;
        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::from([OpenAiLiveServerEvent::InputAudioTranscriptionCompleted {
                    event_id: "evt_1".to_string(),
                    item_id: "item_1".to_string(),
                    content_index: 0,
                    transcript: "hello from live audio".to_string(),
                    logprobs: None,
                    usage: None,
                }]),
                event_gate: None,
                close_gate: Some(hold_open.clone()),
                sent_events,
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target =
            OpenAiLiveCallTarget::new("call_transcript").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if applied_prompts.lock().await.as_slice() == ["hello from live audio"] {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("transcript should be committed through runtime prompt admission");

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::BindingReady);

        hold_open.notify_waiters();
    }

    #[tokio::test]
    async fn openai_live_orchestrator_maps_speech_started_to_runtime_interrupt() {
        let (runtime, session_id, cancel_calls, apply_started, apply_finished, allow_finish) =
            runtime_with_blocking_executor().await;
        let hold_open = Arc::new(Notify::new());
        let release_event = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::from([OpenAiLiveServerEvent::InputAudioBufferSpeechStarted {
                    event_id: "evt_1".to_string(),
                    audio_start_ms: 10,
                    item_id: "item_1".to_string(),
                }]),
                event_gate: Some(release_event.clone()),
                close_gate: Some(hold_open.clone()),
                sent_events,
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            runtime.clone(),
            factory,
            noop_tool_dispatch_host(),
        );
        let target =
            OpenAiLiveCallTarget::new("call_interrupt").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        let input = Input::Prompt(PromptInput::new(
            "running prompt for live interrupt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let (outcome, completion_handle) = runtime
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached prompt should expose completion");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("executor apply should start");

        release_event.notify_waiters();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(
            cancel_calls.load(Ordering::SeqCst),
            0,
            "runtime interrupt should stay deferred while apply is still running"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("executor apply should finish once released");
        let _ = completion_handle.wait().await;

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if cancel_calls.load(Ordering::SeqCst) == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("speech started should route through runtime interrupt");

        let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &runtime,
            &session_id,
        )
        .await
        .expect("runtime status should resolve");
        assert_eq!(status, RealtimeAttachmentStatus::BindingReady);

        hold_open.notify_waiters();
    }

    #[tokio::test]
    async fn openai_live_orchestrator_routes_function_calls_through_shared_dispatch_seam() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::from([
                    OpenAiLiveServerEvent::ResponseFunctionCallArgumentsDone {
                        event_id: "evt_tool_1".to_string(),
                        response_id: "resp_1".to_string(),
                        item_id: "item_1".to_string(),
                        output_index: 0,
                        call_id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: r#"{"hello":"world"}"#.to_string(),
                    },
                ]),
                event_gate: None,
                close_gate: Some(hold_open.clone()),
                sent_events: sent_events.clone(),
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let recorded_calls = Arc::new(Mutex::new(Vec::new()));
        let host = Arc::new(FakeToolDispatchHost {
            calls: recorded_calls.clone(),
            outcomes: Mutex::new(VecDeque::from([Ok(ToolDispatchOutcome::sync_result(
                ToolResult::new("call_1".to_string(), "tool output".to_string(), false),
            ))])),
        });
        let orchestrator =
            OpenAiRealtimeAttachmentOrchestrator::new(runtime.clone(), factory, host);
        let target =
            OpenAiLiveCallTarget::new("call_tool_success").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if sent_events.lock().await.len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("tool output events should be sent to the provider");

        let calls = recorded_calls.lock().await.clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, session_id);
        assert_eq!(calls[0].1.id, "call_1");
        assert_eq!(calls[0].1.name, "echo");
        assert_eq!(calls[0].1.args, json!({ "hello": "world" }));

        let sent = sent_events.lock().await.clone();
        assert!(matches!(
            sent.as_slice(),
            [
                OpenAiLiveClientEvent::ConversationItemCreate { .. },
                OpenAiLiveClientEvent::ResponseCreate { .. }
            ]
        ));
        if let OpenAiLiveClientEvent::ConversationItemCreate { .. } = &sent[0] {
            let payload =
                serde_json::to_value(&sent[0]).expect("provider event should serialize for checks");
            assert_eq!(payload["item"]["call_id"], "call_1");
            assert!(
                payload["item"]["output"]
                    .as_str()
                    .expect("function-call output should serialize as text")
                    .contains("tool output")
            );
        } else {
            panic!("unexpected first provider event: {:?}", sent[0]);
        }

        hold_open.notify_waiters();
    }

    #[tokio::test]
    async fn openai_live_orchestrator_reports_tool_dispatch_errors_back_to_provider() {
        let (runtime, session_id) = runtime_with_live_executor().await;
        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeSession {
                events: VecDeque::from([
                    OpenAiLiveServerEvent::ResponseFunctionCallArgumentsDone {
                        event_id: "evt_tool_err".to_string(),
                        response_id: "resp_2".to_string(),
                        item_id: "item_2".to_string(),
                        output_index: 0,
                        call_id: "call_2".to_string(),
                        name: "explode".to_string(),
                        arguments: r#"{"boom":true}"#.to_string(),
                    },
                ]),
                event_gate: None,
                close_gate: Some(hold_open.clone()),
                sent_events: sent_events.clone(),
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let host = Arc::new(FakeToolDispatchHost {
            calls: Arc::new(Mutex::new(Vec::new())),
            outcomes: Mutex::new(VecDeque::from([Err(
                RuntimeDriverError::ValidationFailed {
                    reason: "tool denied".to_string(),
                },
            )])),
        });
        let orchestrator =
            OpenAiRealtimeAttachmentOrchestrator::new(runtime.clone(), factory, host);
        let target =
            OpenAiLiveCallTarget::new("call_tool_error").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !sent_events.lock().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("tool error should be reported back to provider");

        let sent = sent_events.lock().await.clone();
        assert_eq!(sent.len(), 1);
        if let OpenAiLiveClientEvent::ConversationItemCreate { .. } = &sent[0] {
            let payload =
                serde_json::to_value(&sent[0]).expect("provider event should serialize for checks");
            assert_eq!(payload["item"]["call_id"], "call_2");
            assert!(
                payload["item"]["output"]
                    .as_str()
                    .expect("function-call output should serialize as text")
                    .contains("tool denied")
            );
        } else {
            panic!("unexpected provider event: {:?}", sent[0]);
        }

        hold_open.notify_waiters();
    }
}

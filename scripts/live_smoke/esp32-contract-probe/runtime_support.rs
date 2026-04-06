use std::sync::Arc;

use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::{SessionError, SessionService, StartTurnRequest, TurnToolOverlay};
use meerkat_core::types::{ContentInput, HandlingMode, RenderMetadata, RunResult, SessionId};
use meerkat_core::{Agent, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use meerkat_session::ephemeral::{SessionAgent, SessionSnapshot};
use tokio::sync::mpsc;

pub type ProbeDynAgent =
    Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

pub struct ProbeNoopStore;

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl AgentSessionStore for ProbeNoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

pub struct ProbeSessionAgent {
    agent: ProbeDynAgent,
}

impl ProbeSessionAgent {
    pub fn new(agent: ProbeDynAgent) -> Self {
        Self { agent }
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl SessionAgent for ProbeSessionAgent {
    async fn run_with_events(
        &mut self,
        prompt: ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.agent.run_with_events(prompt, event_tx).await
    }

    async fn run_turn_with_events(
        &mut self,
        prompt: ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        if handling_mode != HandlingMode::Queue {
            return Err(AgentError::ConfigError(format!(
                "handling_mode {handling_mode:?} requires a runtime-backed surface; direct session-service path supports Queue only",
            )));
        }
        if render_metadata.is_some() {
            return Err(AgentError::ConfigError(
                "render_metadata requires a runtime-backed surface; direct session-service path does not support it".to_string(),
            ));
        }
        self.agent.run_with_events(prompt, event_tx).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.agent.pending_skill_references = refs;
    }

    fn set_flow_tool_overlay(&mut self, overlay: Option<TurnToolOverlay>) -> Result<(), AgentError> {
        self.agent
            .set_flow_tool_overlay(overlay)
            .map_err(|error| AgentError::ConfigError(error.to_string()))
    }

    fn replace_client(&mut self, client: Arc<dyn AgentLlmClient>) {
        self.agent.replace_client(client);
    }

    fn update_keep_alive(&mut self, keep_alive: bool) {
        if let Some(mut metadata) = self.agent.session().session_metadata() {
            metadata.keep_alive = keep_alive;
            let _ = self.agent.session_mut().set_session_metadata(metadata);
        }
    }

    fn update_system_prompt(&mut self, system_prompt: String) -> Result<(), AgentError> {
        self.agent.session_mut().set_system_prompt(system_prompt);
        Ok(())
    }

    fn hot_swap_llm_identity(
        &mut self,
        client: Arc<dyn AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        let Some(mut metadata) = self.agent.session().session_metadata() else {
            return Err(AgentError::InternalError(
                "session metadata missing during llm identity hot-swap".to_string(),
            ));
        };
        metadata.apply_llm_identity(&identity);
        self.agent
            .session_mut()
            .set_session_metadata(metadata)
            .map_err(|err| {
                AgentError::InternalError(format!(
                    "failed to update session metadata during llm identity hot-swap: {err}"
                ))
            })?;
        self.agent.replace_client(client);
        Ok(())
    }

    fn stage_external_tool_filter(
        &mut self,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), AgentError> {
        self.agent
            .stage_external_tool_filter(filter)
            .map(|_| ())
            .map_err(|error| AgentError::ConfigError(error.to_string()))
    }

    fn cancel(&mut self) {
        self.agent.cancel();
    }

    fn session_id(&self) -> SessionId {
        self.agent.session().id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        let session = self.agent.session();
        SessionSnapshot {
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            total_tokens: session.total_tokens(),
            usage: session.total_usage(),
            last_assistant_text: session.last_assistant_text(),
        }
    }

    fn session_clone(&self) -> meerkat_core::Session {
        self.agent.session_with_system_context_state()
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.agent
            .session_mut()
            .append_system_context_blocks(appends);
    }

    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
        self.agent.system_context_state()
    }

    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.agent.comms_arc()?.event_injector()
    }

    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.agent.comms_arc()?.interaction_event_injector()
    }

    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.agent.comms_arc()
    }
}

pub struct ProbeSessionRuntimeExecutor<S: SessionService> {
    service: Arc<S>,
    session_id: SessionId,
}

impl<S: SessionService> ProbeSessionRuntimeExecutor<S> {
    pub fn new(service: Arc<S>, session_id: SessionId) -> Self {
        Self { service, session_id }
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl<S: SessionService + 'static> CoreExecutor for ProbeSessionRuntimeExecutor<S> {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        #[cfg(target_os = "espidf")]
        {
            let prompt = extract_prompt(&primitive);
            let preview = prompt.text_content();
            println!(
                "MKT:PROBE_EXECUTOR:APPLY_BEGIN run_id=\"{}\" prompt_bytes={} prompt_preview={}",
                run_id,
                preview.len(),
                serde_json::to_string(&preview.chars().take(120).collect::<String>())
                    .unwrap_or_else(|_| "\"<serialize-failed>\"".to_string())
            );
        }

        let result = self
            .service
            .start_turn(
                &self.session_id,
                StartTurnRequest {
                    prompt: extract_prompt(&primitive),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: HandlingMode::Queue,
                    event_tx: None,
                    skill_references: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.skill_references.clone()),
                    flow_tool_overlay: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.flow_tool_overlay.clone()),
                    additional_instructions: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.additional_instructions.clone()),
                },
            )
            .await
            .map_err(|error| {
                #[cfg(target_os = "espidf")]
                {
                    println!(
                        "MKT:PROBE_EXECUTOR:APPLY_ERR run_id=\"{}\" error={}",
                        run_id,
                        serde_json::to_string(&error.to_string())
                            .unwrap_or_else(|_| "\"<serialize-failed>\"".to_string())
                    );
                }
                CoreExecutorError::ApplyFailed {
                    reason: error.to_string(),
                }
            })?;

        #[cfg(target_os = "espidf")]
        {
            println!(
                "MKT:PROBE_EXECUTOR:APPLY_OK run_id=\"{}\" turns={} tool_calls={} text_bytes={}",
                run_id,
                result.turns,
                result.tool_calls,
                result.text.len()
            );
        }

        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceipt {
                run_id,
                boundary: match &primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            None,
            result,
        ))
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .service
                .interrupt(&self.session_id)
                .await
                .map_err(|error| CoreExecutorError::ControlFailed {
                    reason: error.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => match self.service.archive(&self.session_id).await {
                Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                Err(error) => Err(CoreExecutorError::ControlFailed {
                    reason: error.to_string(),
                }),
            },
            _ => Ok(()),
        }
    }
}

pub fn extract_prompt(primitive: &RunPrimitive) -> ContentInput {
    match primitive {
        RunPrimitive::StagedInput(staged) => {
            let mut all_blocks = Vec::new();
            for append in &staged.appends {
                match &append.content {
                    CoreRenderable::Text { text } => {
                        all_blocks.push(meerkat_core::types::ContentBlock::Text {
                            text: text.clone(),
                        });
                    }
                    CoreRenderable::Blocks { blocks } => {
                        all_blocks.extend(blocks.iter().cloned());
                    }
                    _ => {}
                }
            }
            if all_blocks.is_empty() {
                ContentInput::Text(String::new())
            } else if all_blocks.len() == 1 {
                if let meerkat_core::types::ContentBlock::Text { text } = &all_blocks[0] {
                    ContentInput::Text(text.clone())
                } else {
                    ContentInput::Blocks(all_blocks)
                }
            } else {
                ContentInput::Blocks(all_blocks)
            }
        }
        RunPrimitive::ImmediateAppend(append) => match &append.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        RunPrimitive::ImmediateContextAppend(ctx) => match &ctx.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        _ => ContentInput::Text(String::new()),
    }
}

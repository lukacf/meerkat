//! SessionRuntimeExecutor — implements `CoreExecutor` by delegating to `SessionRuntime`.
//!
//! This is the bridge between the v9 runtime layer and the existing RPC session
//! management. When the RuntimeLoop processes a queued input, it calls
//! `CoreExecutor::apply()` on this executor, which translates the `RunPrimitive`
//! into a `SessionRuntime::start_turn()` call.

use std::sync::Arc;

use meerkat_core::ContentInput;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunPrimitive};
use meerkat_core::service::SessionError;
use meerkat_core::types::SessionId;
use tokio::sync::mpsc;

use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;
#[cfg(feature = "mob")]
use meerkat_mob::MobSessionService;

/// Implements `CoreExecutor` by delegating to `SessionRuntime::start_turn()`.
///
/// Each session gets its own executor instance, which is owned by the
/// RuntimeLoop task. The executor extracts the prompt from the `RunPrimitive`
/// and calls `start_turn`, forwarding events to the RPC notification sink.
pub struct SessionRuntimeExecutor {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
    notification_sink: NotificationSink,
}

#[cfg(feature = "mob")]
pub struct MobRpcRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
    notification_sink: NotificationSink,
}

#[cfg(feature = "mob")]
impl MobRpcRuntimeExecutor {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        session_id: SessionId,
        notification_sink: NotificationSink,
    ) -> Self {
        Self {
            session_service,
            session_id,
            notification_sink,
        }
    }
}

impl SessionRuntimeExecutor {
    /// Create a new executor for a specific session.
    pub fn new(
        runtime: Arc<SessionRuntime>,
        session_id: SessionId,
        notification_sink: NotificationSink,
    ) -> Self {
        Self {
            runtime,
            session_id,
            notification_sink,
        }
    }
}

/// Extract prompt content from a `RunPrimitive`, preserving multimodal blocks.
fn extract_prompt(primitive: &RunPrimitive) -> ContentInput {
    match primitive {
        RunPrimitive::StagedInput(staged) => {
            let mut all_blocks = Vec::new();
            for append in &staged.appends {
                match &append.content {
                    CoreRenderable::Text { text } => {
                        all_blocks
                            .push(meerkat_core::types::ContentBlock::Text { text: text.clone() });
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

#[async_trait::async_trait]
impl CoreExecutor for SessionRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let prompt = extract_prompt(&primitive);

        // Create an event channel and forward events to the notification sink
        let (event_tx, mut event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(128);
        let sink = self.notification_sink.clone();
        let sid = self.session_id.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                sink.emit_event(&sid, &event).await;
            }
        });

        let result = self
            .runtime
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                &primitive,
                prompt,
                event_tx,
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.skill_references.clone()),
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.flow_tool_overlay.clone()),
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.additional_instructions.clone()),
                Some(crate::handlers::turn::TurnOverrides {
                    host_mode: primitive.turn_metadata().and_then(|meta| meta.host_mode),
                    model: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.model.clone()),
                    provider: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.provider.clone()),
                    provider_params: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.provider_params.clone()),
                    max_tokens: None,
                    system_prompt: None,
                    output_schema: None,
                    structured_output_retries: None,
                }),
            )
            .await;

        // Wait for the event forwarder to finish
        let _ = forwarder.await;

        match result {
            Ok(output) => Ok(output),
            Err(rpc_err) => Err(CoreExecutorError::ApplyFailed {
                reason: rpc_err.message,
            }),
        }
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .runtime
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::ControlFailed { reason: e.message }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self.runtime.discard_live_session(&self.session_id).await;
                self.runtime
                    .runtime_adapter()
                    .unregister_session(&self.session_id)
                    .await;
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutor for MobRpcRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let prompt = extract_prompt(&primitive);
        let (event_tx, mut event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(128);
        let sink = self.notification_sink.clone();
        let sid = self.session_id.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                sink.emit_event(&sid, &event).await;
            }
        });

        let req = meerkat_core::service::StartTurnRequest {
            prompt,
            event_tx: Some(event_tx),
            host_mode: primitive
                .turn_metadata()
                .and_then(|meta| meta.host_mode)
                .unwrap_or(false),
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            additional_instructions: primitive
                .turn_metadata()
                .and_then(|meta| meta.additional_instructions.clone()),
        };

        let result = self
            .session_service
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                req,
                match &primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
                },
                primitive.contributing_input_ids().to_vec(),
            )
            .await;

        let _ = forwarder.await;
        result.map_err(|err| CoreExecutorError::ApplyFailed {
            reason: err.to_string(),
        })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .session_service
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::ControlFailed {
                    reason: e.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self
                    .session_service
                    .discard_live_session(&self.session_id)
                    .await;
                if let Some(adapter) = self.session_service.runtime_adapter() {
                    adapter.unregister_session(&self.session_id).await;
                }
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

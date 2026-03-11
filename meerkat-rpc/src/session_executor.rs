//! SessionRuntimeExecutor — implements `CoreExecutor` by delegating to `SessionRuntime`.
//!
//! This is the bridge between the v9 runtime layer and the existing RPC session
//! management. When the RuntimeLoop processes a queued input, it calls
//! `CoreExecutor::apply()` on this executor, which translates the `RunPrimitive`
//! into a `SessionRuntime::start_turn()` call.

use std::sync::Arc;

use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::types::SessionId;
use tokio::sync::mpsc;

use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;

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

/// Extract prompt text from a `RunPrimitive`.
fn extract_prompt(primitive: &RunPrimitive) -> String {
    match primitive {
        RunPrimitive::StagedInput(staged) => staged
            .appends
            .iter()
            .filter_map(|a| match &a.content {
                CoreRenderable::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        RunPrimitive::ImmediateAppend(append) => match &append.content {
            CoreRenderable::Text { text } => text.clone(),
            _ => String::new(),
        },
        RunPrimitive::ImmediateContextAppend(ctx) => match &ctx.content {
            CoreRenderable::Text { text } => text.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

#[async_trait::async_trait]
impl CoreExecutor for SessionRuntimeExecutor {
    async fn apply(
        &mut self,
        primitive: RunPrimitive,
    ) -> Result<RunBoundaryReceipt, CoreExecutorError> {
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
            .start_turn(
                &self.session_id,
                prompt,
                event_tx,
                None, // skill_references
                None, // flow_tool_overlay
                None, // additional_instructions
                None, // overrides
            )
            .await;

        // Wait for the event forwarder to finish
        let _ = forwarder.await;

        match result {
            Ok(_run_result) => {
                let boundary = match &primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
                };
                Ok(RunBoundaryReceipt {
                    run_id: RunId::new(),
                    boundary,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                })
            }
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
            RunControlCommand::StopRuntimeExecutor { .. } => Ok(()),
            _ => Ok(()),
        }
    }
}

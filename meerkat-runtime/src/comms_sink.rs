//! Concrete RuntimeInputSink implementation.
//!
//! Routes host-mode comms interactions through the RuntimeSessionAdapter
//! instead of calling Agent::run() directly.

use std::sync::Arc;

use meerkat_core::agent::{CommsRuntime, RuntimeInputSink};
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::InboxInteraction;
use meerkat_core::lifecycle::RunControlCommand;
use meerkat_core::types::SessionId;

use crate::comms_bridge::interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{ContinuationInput, Input};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::session_adapter::RuntimeSessionAdapter;
use crate::tokio::sync::mpsc;

/// Routes host-mode comms interactions through the runtime adapter.
///
/// Awaits only admission (durable-before-ack), NOT execution completion —
/// the host loop continues immediately after the input is accepted.
pub struct RuntimeCommsBridge {
    adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    comms_runtime: Option<Arc<dyn CommsRuntime>>,
}

impl RuntimeCommsBridge {
    pub fn new(
        adapter: Arc<RuntimeSessionAdapter>,
        session_id: SessionId,
        comms_runtime: Option<Arc<dyn CommsRuntime>>,
    ) -> Self {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        Self {
            adapter,
            session_id,
            runtime_id,
            comms_runtime,
        }
    }

    fn spawn_completion_bridge(
        comms_runtime: Option<Arc<dyn CommsRuntime>>,
        interaction_id: meerkat_core::interaction::InteractionId,
        subscriber: Option<mpsc::Sender<AgentEvent>>,
        handle: Option<crate::completion::CompletionHandle>,
    ) {
        crate::tokio::spawn(async move {
            let outcome = match handle {
                Some(handle) => handle.wait().await,
                None => CompletionOutcome::CompletedWithoutResult,
            };

            if let Some(tx) = subscriber {
                let event = match outcome {
                    CompletionOutcome::Completed(result) => AgentEvent::InteractionComplete {
                        interaction_id,
                        result: result.text,
                    },
                    CompletionOutcome::CompletedWithoutResult => AgentEvent::InteractionComplete {
                        interaction_id,
                        result: String::new(),
                    },
                    CompletionOutcome::Abandoned(reason)
                    | CompletionOutcome::RuntimeTerminated(reason) => {
                        AgentEvent::InteractionFailed {
                            interaction_id,
                            error: reason,
                        }
                    }
                };

                let _ =
                    crate::tokio::time::timeout(std::time::Duration::from_secs(5), tx.send(event))
                        .await;
            }

            if let Some(runtime) = comms_runtime {
                runtime.mark_interaction_complete(&interaction_id);
            }
        });
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeInputSink for RuntimeCommsBridge {
    async fn accept_peer_input(&self, interaction: InboxInteraction) -> Result<(), String> {
        let interaction_id = interaction.id;
        let subscriber = self
            .comms_runtime
            .as_ref()
            .and_then(|runtime| runtime.interaction_subscriber(&interaction_id));
        let input = interaction_to_runtime_input(&interaction, &self.runtime_id);
        let (_outcome, handle) = self
            .adapter
            .accept_input_with_completion(&self.session_id, input)
            .await
            .map_err(|e| e.to_string())?;

        if subscriber.is_some() || handle.is_some() {
            Self::spawn_completion_bridge(
                self.comms_runtime.clone(),
                interaction_id,
                subscriber,
                handle,
            );
        } else if let Some(runtime) = &self.comms_runtime {
            runtime.mark_interaction_complete(&interaction_id);
        }

        Ok(())
    }

    async fn schedule_terminal_response_continuation(
        &self,
        request_id: Option<String>,
    ) -> Result<(), String> {
        let input = Input::Continuation(ContinuationInput::terminal_peer_response_for_request(
            "terminal peer response injected into session state",
            request_id,
        ));
        self.adapter
            .accept_input(&self.session_id, input)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn stop_runtime_executor(&self, reason: &str) -> Result<(), String> {
        self.adapter
            .stop_runtime_executor(
                &self.session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: reason.to_string(),
                },
            )
            .await
            .map_err(|e| e.to_string())
    }
}

pub type RuntimeCommsInputSink = RuntimeCommsBridge;

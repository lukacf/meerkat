//! CommsAgent and related high-level agent integration.

pub mod dispatcher;
#[cfg(not(target_arch = "wasm32"))]
pub mod listener;
pub mod manager;
pub mod types;

pub use dispatcher::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
#[cfg(unix)]
pub use listener::spawn_uds_listener;
#[cfg(not(target_arch = "wasm32"))]
pub use listener::{ListenerHandle, spawn_tcp_listener};
pub use manager::{CommsManager, CommsManagerConfig};
pub use types::{CommsContent, CommsMessage, CommsStatus, MessageIntent};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::error::AgentError;
use meerkat_core::types::RunResult;
use meerkat_core::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use tokio::sync::watch;

/// Agent wrapper that integrates comms inbox.
pub struct CommsAgent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    agent: Agent<C, T, S>,
    comms_manager: CommsManager,
}

impl<C, T, S> CommsAgent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    pub fn new(agent: Agent<C, T, S>, comms_manager: CommsManager) -> Self {
        Self {
            agent,
            comms_manager,
        }
    }

    pub fn agent(&self) -> &Agent<C, T, S> {
        &self.agent
    }
    pub fn agent_mut(&mut self) -> &mut Agent<C, T, S> {
        &mut self.agent
    }
    pub fn comms_manager(&self) -> &CommsManager {
        &self.comms_manager
    }
    pub fn comms_manager_mut(&mut self) -> &mut CommsManager {
        &mut self.comms_manager
    }

    pub async fn run(&mut self, user_input: String) -> Result<RunResult, AgentError> {
        let inbox_messages = self.comms_manager.drain_messages();
        let combined_input = if inbox_messages.is_empty() {
            user_input
        } else {
            let comms_text = format_inbox_messages(&inbox_messages);
            if user_input.is_empty() {
                comms_text
            } else {
                format!("{comms_text}\n\n---\n\n{user_input}")
            }
        };
        self.agent.run(combined_input.into()).await
    }

    pub async fn run_stay_alive(
        &mut self,
        initial_prompt: String,
        mut cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Result<RunResult, AgentError> {
        let mut last_result = self.run(initial_prompt).await?;
        loop {
            if let Some(ref rx) = cancel_rx
                && *rx.borrow()
            {
                return Err(AgentError::Cancelled);
            }
            let first_msg = if let Some(ref mut rx) = cancel_rx {
                tokio::select! {
                    _ = rx.changed() => {
                        if *rx.borrow() { return Err(AgentError::Cancelled); }
                        continue;
                    }
                    msg = self.comms_manager.recv_message() => {
                        msg.ok_or_else(|| AgentError::ToolError("Inbox closed".to_string()))?
                    }
                }
            } else {
                self.comms_manager
                    .recv_message()
                    .await
                    .ok_or_else(|| AgentError::ToolError("Inbox closed".to_string()))?
            };
            let mut messages = vec![first_msg];
            messages.extend(self.comms_manager.drain_messages());
            if contains_dismiss(&messages) {
                return Ok(last_result);
            }
            let comms_text = format_inbox_messages(&messages);
            last_result = self.agent.run(comms_text.into()).await?;
        }
    }
}

fn contains_dismiss(messages: &[CommsMessage]) -> bool {
    messages.iter().any(|m| match &m.content {
        crate::agent::types::CommsContent::Message { body, .. } => {
            body.trim().eq_ignore_ascii_case("DISMISS")
        }
        _ => false,
    })
}

fn format_inbox_messages(messages: &[CommsMessage]) -> String {
    messages
        .iter()
        .map(types::CommsMessage::to_user_message_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

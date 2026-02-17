//! CommsAgent and related high-level agent integration.

pub mod dispatcher;
pub mod listener;
pub mod manager;
pub mod types;

pub use dispatcher::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
pub use listener::{ListenerHandle, spawn_tcp_listener};
#[cfg(unix)]
pub use listener::spawn_uds_listener;
pub use manager::{CommsManager, CommsManagerConfig};
pub use types::{CommsContent, CommsMessage, CommsStatus, MessageIntent};

use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
use meerkat_core::retry::RetryPolicy;
use meerkat_core::session::Session;
use meerkat_core::types::RunResult;
use meerkat_core::{Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use std::sync::Arc;
use tokio::sync::watch;

/// Agent wrapper that integrates comms inbox.
pub struct CommsAgent<C, T, S>
where
    C: AgentLlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
{
    agent: Agent<C, T, S>,
    comms_manager: CommsManager,
}

impl<C, T, S> CommsAgent<C, T, S>
where
    C: AgentLlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
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
                format!("{}\n\n---\n\n{}", comms_text, user_input)
            }
        };
        self.agent.run(combined_input).await
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
            last_result = self.agent.run(comms_text).await?;
        }
    }
}

fn contains_dismiss(messages: &[CommsMessage]) -> bool {
    messages.iter().any(|m| match &m.content {
        crate::agent::types::CommsContent::Message { body } => {
            body.trim().eq_ignore_ascii_case("DISMISS")
        }
        _ => false,
    })
}

fn format_inbox_messages(messages: &[CommsMessage]) -> String {
    messages
        .iter()
        .map(|m| m.to_user_message_text())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub struct CommsAgentBuilder {
    model: Option<String>,
    system_prompt: Option<String>,
    max_tokens_per_turn: Option<u32>,
    budget_limits: Option<BudgetLimits>,
    retry_policy: Option<RetryPolicy>,
    session: Option<Session>,
}

impl CommsAgentBuilder {
    pub fn new() -> Self {
        Self {
            model: None,
            system_prompt: None,
            max_tokens_per_turn: None,
            budget_limits: None,
            retry_policy: None,
            session: None,
        }
    }
    pub fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }
    pub fn system_prompt(mut self, p: impl Into<String>) -> Self {
        self.system_prompt = Some(p.into());
        self
    }
    pub fn max_tokens_per_turn(mut self, t: u32) -> Self {
        self.max_tokens_per_turn = Some(t);
        self
    }
    pub fn budget(mut self, l: BudgetLimits) -> Self {
        self.budget_limits = Some(l);
        self
    }
    pub fn retry_policy(mut self, p: RetryPolicy) -> Self {
        self.retry_policy = Some(p);
        self
    }
    pub fn resume_session(mut self, s: Session) -> Self {
        self.session = Some(s);
        self
    }

    pub async fn build<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
        comms_manager: CommsManager,
    ) -> CommsAgent<C, T, S>
    where
        C: AgentLlmClient + 'static,
        T: AgentToolDispatcher + 'static,
        S: AgentSessionStore + 'static,
    {
        let mut builder = AgentBuilder::new();
        if let Some(m) = self.model {
            builder = builder.model(m);
        }
        if let Some(p) = self.system_prompt {
            builder = builder.system_prompt(p);
        }
        if let Some(t) = self.max_tokens_per_turn {
            builder = builder.max_tokens_per_turn(t);
        }
        if let Some(l) = self.budget_limits {
            builder = builder.budget(l);
        }
        if let Some(p) = self.retry_policy {
            builder = builder.retry_policy(p);
        }
        if let Some(s) = self.session {
            builder = builder.resume_session(s);
        }
        let agent = builder.build(client, tools, store).await;
        CommsAgent::new(agent, comms_manager)
    }
}

impl Default for CommsAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

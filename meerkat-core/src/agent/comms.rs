//! Agent comms helpers (host mode).

use crate::comms_runtime::CommsRuntime;
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::types::{Message, RunResult, Usage, UserMessage};
use tokio::sync::mpsc;

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Get the comms runtime, if enabled.
    ///
    /// Returns `None` if comms is disabled or if this is a subagent.
    pub fn comms(&self) -> Option<&CommsRuntime> {
        self.comms_runtime.as_ref()
    }

    /// Get mutable access to the comms runtime, if enabled.
    ///
    /// Returns `None` if comms is disabled or if this is a subagent.
    pub fn comms_mut(&mut self) -> Option<&mut CommsRuntime> {
        self.comms_runtime.as_mut()
    }

    /// Drain comms inbox and inject messages into session.
    ///
    /// This is called at turn boundaries to process incoming inter-agent messages.
    /// It is non-blocking on the inbox but awaits the trusted peers read lock.
    pub(super) async fn drain_comms_inbox(&mut self) {
        if let Some(ref mut comms) = self.comms_runtime {
            let messages = comms.drain_messages().await;
            if !messages.is_empty() {
                tracing::debug!("Injecting {} comms messages into session", messages.len());

                // Format all messages into a single user message for the LLM
                // Pre-calculate capacity to avoid reallocations
                let mut combined = String::new();
                let mut first = true;
                for msg in &messages {
                    if !first {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(&msg.to_user_message_text());
                    first = false;
                }

                self.session
                    .push(Message::User(UserMessage { content: combined }));
            }
        }
    }

    /// Run the agent in host mode: process initial prompt, then stay alive for comms messages.
    ///
    /// Host mode:
    /// 1. Runs the initial prompt (if non-empty)
    /// 2. Waits for incoming comms messages
    /// 3. Processes each batch of messages as a new turn
    /// 4. Continues until budget exhaustion, error, or process termination
    ///
    /// Requires comms to be enabled. Returns error if comms is not configured.
    ///
    /// # Arguments
    /// * `initial_prompt` - Initial prompt to run before entering host loop. If empty,
    ///   skips the initial LLM call and immediately waits for comms messages.
    ///
    /// # Returns
    /// The result from the last successful run. In practice, this usually means
    /// the run that exhausted the budget or encountered an error.
    pub async fn run_host_mode(&mut self, initial_prompt: String) -> Result<RunResult, AgentError> {
        use std::time::Duration;

        // Verify comms is enabled
        if self.comms_runtime.is_none() {
            return Err(AgentError::ConfigError(
                "Host mode requires comms to be enabled".to_string(),
            ));
        }

        // Run initial prompt if non-empty, OR if session has pending user message
        // (sub-agents pre-load the prompt into session, so initial_prompt may be empty)
        let has_pending_user_message = self
            .session
            .messages()
            .last()
            .is_some_and(|m| matches!(m, crate::types::Message::User(_)));

        let mut last_result = if !initial_prompt.trim().is_empty() {
            // Non-empty prompt - run normally (will push user message)
            self.run(initial_prompt).await?
        } else if has_pending_user_message {
            // Session already has a user message - run the loop directly without pushing
            self.run_loop(None).await?
        } else {
            // No prompt and no pending messages - create minimal result
            RunResult {
                text: String::new(),
                session_id: self.session.id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: Usage::default(),
                structured_output: None,
                schema_warnings: None,
            }
        };

        // Get inbox notify for waiting
        let inbox_notify = self
            .comms_runtime
            .as_ref()
            .ok_or_else(|| AgentError::InternalError("comms not initialized".to_string()))?
            .inbox_notify();

        // Polling interval for budget checks when no duration limit
        const POLL_INTERVAL: Duration = Duration::from_secs(60);

        loop {
            // Check budget before waiting
            if self.budget.is_exhausted() {
                tracing::info!("Host mode: budget exhausted, exiting");
                return Ok(last_result);
            }

            // Determine timeout: use remaining budget duration if set, otherwise poll interval
            let timeout = self.budget.remaining_duration().unwrap_or(POLL_INTERVAL);

            // IMPORTANT: Register the notified future BEFORE draining to avoid race condition.
            // If we drain first, a message could arrive between drain and await, and we'd
            // miss the notification (Notify is non-latched).
            let notified = inbox_notify.notified();

            // Try to drain any already-queued messages first
            let messages = if let Some(ref mut comms) = self.comms_runtime {
                comms.drain_messages().await
            } else {
                Vec::new()
            };

            // If we have messages, process them immediately (don't wait)
            if !messages.is_empty() {
                let combined_input = messages
                    .iter()
                    .map(|m| m.to_user_message_text())
                    .collect::<Vec<_>>()
                    .join("\n\n");

                tracing::debug!("Host mode: processing {} comms message(s)", messages.len());

                match self.run(combined_input).await {
                    Ok(result) => {
                        last_result = result;
                    }
                    Err(e) => {
                        if e.is_graceful() {
                            tracing::info!("Host mode: graceful exit - {}", e);
                            return Ok(last_result);
                        }
                        return Err(e);
                    }
                }
                continue;
            }

            // No messages queued - wait for notification or timeout
            tokio::select! {
                _ = notified => {
                    // Message arrived, loop back to drain and process
                }
                _ = tokio::time::sleep(timeout) => {
                    // Timeout: check budget again (for duration-based limits)
                    tracing::trace!("Host mode: timeout, checking budget");
                }
            }
        }
    }

    /// Run in host mode with event streaming for verbose output
    ///
    /// Same as `run_host_mode` but emits events for monitoring the agent's progress.
    pub async fn run_host_mode_with_events(
        &mut self,
        initial_prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        use std::time::Duration;

        // Verify comms is enabled
        if self.comms_runtime.is_none() {
            return Err(AgentError::ConfigError(
                "Host mode requires comms to be enabled".to_string(),
            ));
        }

        // Run initial prompt if non-empty, OR if session has pending user message
        // (sub-agents pre-load the prompt into session, so initial_prompt may be empty)
        let has_pending_user_message = self
            .session
            .messages()
            .last()
            .is_some_and(|m| matches!(m, crate::types::Message::User(_)));

        let mut last_result = if !initial_prompt.trim().is_empty() {
            // Non-empty prompt - run normally (will push user message)
            self.run_with_events(initial_prompt, event_tx.clone())
                .await?
        } else if has_pending_user_message {
            // Session already has a user message - run the loop directly without pushing
            let run_prompt = self
                .session
                .messages()
                .last()
                .and_then(|msg| match msg {
                    Message::User(user) => Some(user.content.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let _ = event_tx
                .send(AgentEvent::RunStarted {
                    session_id: self.session.id().clone(),
                    prompt: run_prompt,
                })
                .await;
            self.run_loop(Some(event_tx.clone())).await?
        } else {
            // No prompt and no pending messages - create minimal result
            RunResult {
                text: String::new(),
                session_id: self.session.id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: Usage::default(),
                structured_output: None,
                schema_warnings: None,
            }
        };

        // Get inbox notify for waiting
        let inbox_notify = self
            .comms_runtime
            .as_ref()
            .ok_or_else(|| AgentError::InternalError("comms not initialized".to_string()))?
            .inbox_notify();

        // Polling interval for budget checks when no duration limit
        const POLL_INTERVAL: Duration = Duration::from_secs(60);

        loop {
            // Check budget before waiting
            if self.budget.is_exhausted() {
                tracing::info!("Host mode: budget exhausted, exiting");
                return Ok(last_result);
            }

            // Determine timeout: use remaining budget duration if set, otherwise poll interval
            let timeout = self.budget.remaining_duration().unwrap_or(POLL_INTERVAL);

            // IMPORTANT: Register the notified future BEFORE draining to avoid race condition.
            let notified = inbox_notify.notified();

            // Try to drain any already-queued messages first
            let messages = if let Some(ref mut comms) = self.comms_runtime {
                comms.drain_messages().await
            } else {
                Vec::new()
            };

            // If we have messages, process them immediately (don't wait)
            if !messages.is_empty() {
                let combined_input = messages
                    .iter()
                    .map(|m| m.to_user_message_text())
                    .collect::<Vec<_>>()
                    .join("\n\n");

                tracing::debug!("Host mode: processing {} comms message(s)", messages.len());

                match self.run_with_events(combined_input, event_tx.clone()).await {
                    Ok(result) => {
                        last_result = result;
                    }
                    Err(e) => {
                        if e.is_graceful() {
                            tracing::info!("Host mode: graceful exit - {}", e);
                            return Ok(last_result);
                        }
                        return Err(e);
                    }
                }
                continue;
            }

            // No messages queued - wait for notification or timeout
            tokio::select! {
                _ = notified => {
                    // Message arrived, loop back to drain and process
                }
                _ = tokio::time::sleep(timeout) => {
                    // Timeout: check budget again (for duration-based limits)
                    tracing::trace!("Host mode: timeout, checking budget");
                }
            }
        }
    }
}

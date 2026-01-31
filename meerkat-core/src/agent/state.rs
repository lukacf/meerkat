//! Agent state machine internals.

use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType};
use crate::state::LoopState;
use crate::types::{AssistantMessage, Message, RunResult, ToolDef, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{
    Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CALLBACK_TOOL_PREFIX,
    LlmStreamResult,
};

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Call LLM with retry logic
    async fn call_llm_with_retry(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError> {
        let mut attempt = 0u32;

        loop {
            // Wait for retry delay if not first attempt
            if attempt > 0 {
                let delay = self.retry_policy.delay_for_attempt(attempt);
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .stream_response(
                    messages,
                    tools,
                    max_tokens,
                    self.config.temperature,
                    self.config.provider_params.as_ref(),
                )
                .await
            {
                Ok(result) => return Ok(result),
                Err(e) => {
                    // Check if we should retry
                    if e.is_recoverable() && self.retry_policy.should_retry(attempt) {
                        tracing::warn!(
                            "LLM call failed (attempt {}), retrying: {}",
                            attempt + 1,
                            e
                        );
                        attempt += 1;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// The main agent loop
    pub(super) async fn run_loop(
        &mut self,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let mut turn_count = 0u32;
        let max_turns = self.config.max_turns.unwrap_or(100);
        let mut tool_call_count = 0u32;

        // Helper to conditionally emit events (only when listener exists)
        macro_rules! emit_event {
            ($event:expr) => {
                if let Some(ref tx) = event_tx {
                    let _ = tx.send($event).await;
                }
            };
        }

        loop {
            // Check turn limit
            if turn_count >= max_turns {
                self.state = LoopState::Completed;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            // Check budget
            if self.budget.is_exhausted() {
                emit_event!(AgentEvent::BudgetWarning {
                    budget_type: BudgetType::Tokens,
                    used: self.session.total_tokens(),
                    limit: self.budget.remaining(),
                    percent: 1.0,
                });
                self.state = LoopState::Completed;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            match self.state {
                LoopState::CallingLlm => {
                    // Emit turn start
                    emit_event!(AgentEvent::TurnStarted {
                        turn_number: turn_count,
                    });

                    // Get tool definitions
                    let tool_defs = self.tools.tools();

                    // Call LLM with retry
                    let result = self
                        .call_llm_with_retry(
                            self.session.messages(),
                            &tool_defs,
                            self.config.max_tokens_per_turn,
                        )
                        .await?;

                    // Update budget
                    self.budget.record_usage(&result.usage);

                    if !result.content.is_empty() {
                        emit_event!(AgentEvent::TextComplete {
                            content: result.content.clone(),
                        });
                    }

                    // Check if we have tool calls
                    if !result.tool_calls.is_empty() {
                        // Add assistant message with tool calls
                        self.session.push(Message::Assistant(AssistantMessage {
                            content: result.content,
                            tool_calls: result.tool_calls.clone(),
                            stop_reason: result.stop_reason,
                            usage: result.usage,
                        }));

                        // Emit tool call requests
                        for tc in &result.tool_calls {
                            emit_event!(AgentEvent::ToolCallRequested {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                args: tc.args.clone(),
                            });
                        }

                        // Transition to waiting for ops
                        self.state.transition(LoopState::WaitingForOps)?;

                        // Execute tool calls in parallel
                        let num_tool_calls = result.tool_calls.len();
                        let tools_ref = &self.tools;

                        // Emit all execution start events
                        for tc in &result.tool_calls {
                            emit_event!(AgentEvent::ToolExecutionStarted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                            });
                        }

                        // Execute all tool calls in parallel using join_all
                        let dispatch_futures: Vec<_> = result
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                let id = tc.id.clone();
                                let name = tc.name.clone();
                                let args = tc.args.clone();
                                let thought_signature = tc.thought_signature.clone();
                                async move {
                                    let start = std::time::Instant::now();
                                    let dispatch_result = tools_ref.dispatch(&name, &args).await;
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    (
                                        id,
                                        name,
                                        args,
                                        dispatch_result,
                                        duration_ms,
                                        thought_signature,
                                    )
                                }
                            })
                            .collect();

                        let dispatch_results = futures::future::join_all(dispatch_futures).await;

                        // Process results and emit events
                        let mut tool_results = Vec::with_capacity(num_tool_calls);
                        for (id, name, args, dispatch_result, duration_ms, thought_signature) in
                            dispatch_results
                        {
                            let (content, is_error) = match dispatch_result {
                                Ok(v) => {
                                    // Stringify the Value for the LLM
                                    let s = match &v {
                                        Value::String(s) => s.clone(),
                                        _ => serde_json::to_string(&v).unwrap_or_default(),
                                    };
                                    (s, false)
                                }
                                Err(crate::error::ToolError::Other(msg))
                                    if msg.starts_with(CALLBACK_TOOL_PREFIX) =>
                                {
                                    let payload = msg
                                        .strip_prefix(CALLBACK_TOOL_PREFIX)
                                        .and_then(|suffix| {
                                            serde_json::from_str::<Value>(suffix).ok()
                                        })
                                        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                    let mut payload =
                                        payload.as_object().cloned().unwrap_or_default();
                                    payload
                                        .entry("tool_use_id".to_string())
                                        .or_insert(Value::String(id.clone()));
                                    payload
                                        .entry("tool_name".to_string())
                                        .or_insert(Value::String(name.clone()));
                                    payload.entry("args".to_string()).or_insert(args.clone());
                                    let serialized = serde_json::to_string(&Value::Object(payload))
                                        .unwrap_or_default();
                                    return Err(AgentError::ToolError(format!(
                                        "{}{}",
                                        CALLBACK_TOOL_PREFIX, serialized
                                    )));
                                }
                                Err(e) => {
                                    let payload = e.to_error_payload();
                                    let serialized = serde_json::to_string(&payload)
                                        .unwrap_or_else(|_| {
                                            "{\"error\":\"tool_error\",\"message\":\"tool error\"}"
                                                .to_string()
                                        });
                                    (serialized, true)
                                }
                            };

                            // Emit execution complete
                            emit_event!(AgentEvent::ToolExecutionCompleted {
                                id: id.clone(),
                                name: name.clone(),
                                result: content.clone(),
                                is_error,
                                duration_ms,
                            });

                            // Emit result received
                            emit_event!(AgentEvent::ToolResultReceived {
                                id: id.clone(),
                                name: name.clone(),
                                is_error,
                            });

                            tool_results.push(ToolResult {
                                tool_use_id: id,
                                content,
                                is_error,
                                thought_signature,
                            });

                            // Track tool call in budget
                            self.budget.record_tool_call();
                            tool_call_count += 1;
                        }

                        // Add tool results to session
                        self.session.push(Message::ToolResults {
                            results: tool_results,
                        });

                        // Go through DrainingEvents to CallingLlm (state machine requires this)
                        self.state.transition(LoopState::DrainingEvents)?;

                        // === TURN BOUNDARY: drain comms, collect sub-agent results ===

                        // Drain comms inbox and inject messages into session
                        self.drain_comms_inbox().await;

                        // Collect completed sub-agent results and inject into session
                        let sub_agent_results = self.collect_sub_agent_results().await;
                        if !sub_agent_results.is_empty() {
                            // Inject sub-agent results as tool results
                            let results: Vec<ToolResult> = sub_agent_results
                                .into_iter()
                                .map(|r| ToolResult {
                                    tool_use_id: r.id.to_string(),
                                    content: r.content,
                                    is_error: r.is_error,
                                    thought_signature: None, // Sub-agents don't use thought signatures
                                })
                                .collect();
                            self.session.push(Message::ToolResults { results });
                        }

                        // === END TURN BOUNDARY ===

                        self.state.transition(LoopState::CallingLlm)?;
                        turn_count += 1;
                    } else {
                        // No tool calls - we're done
                        let final_text = result.content.clone();
                        self.session.push(Message::Assistant(AssistantMessage {
                            content: result.content,
                            tool_calls: vec![],
                            stop_reason: result.stop_reason,
                            usage: result.usage.clone(),
                        }));

                        // Emit turn completed
                        emit_event!(AgentEvent::TurnCompleted {
                            stop_reason: result.stop_reason,
                            usage: result.usage,
                        });

                        // Transition to completed
                        self.state.transition(LoopState::DrainingEvents)?;
                        self.state.transition(LoopState::Completed)?;

                        // Save session
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }

                        // Emit run completed
                        emit_event!(AgentEvent::RunCompleted {
                            session_id: self.session.id().clone(),
                            result: final_text.clone(),
                            usage: self.session.total_usage(),
                        });

                        return Ok(RunResult {
                            text: final_text,
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                        });
                    }
                }
                LoopState::WaitingForOps => {
                    // This state is handled inline above
                    unreachable!("WaitingForOps handled inline");
                }
                LoopState::DrainingEvents => {
                    // Wait for any pending events to be processed
                    self.state.transition(LoopState::Completed)?;
                }
                LoopState::Cancelling => {
                    // Handle cancellation
                    self.state.transition(LoopState::Completed)?;
                    return Ok(self.build_result(turn_count, tool_call_count));
                }
                LoopState::ErrorRecovery => {
                    // Attempt recovery
                    self.state.transition(LoopState::CallingLlm)?;
                }
                LoopState::Completed => {
                    return Ok(self.build_result(turn_count, tool_call_count));
                }
            }
        }
    }

    /// Build a RunResult from current state
    fn build_result(&self, turns: u32, tool_calls: u32) -> RunResult {
        RunResult {
            text: self.session.last_assistant_text().unwrap_or("").to_string(),
            session_id: self.session.id().clone(),
            usage: self.session.total_usage(),
            turns,
            tool_calls,
        }
    }
}

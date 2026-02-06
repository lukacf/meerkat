//! Agent state machine internals.

use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType};
use crate::hooks::{
    HookDecision, HookInvocation, HookLlmRequest, HookLlmResponse, HookPatch, HookPoint,
    HookToolCall, HookToolResult,
};
use crate::state::LoopState;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, RunResult, ToolCallView, ToolDef, ToolResult,
};
use serde_json::Value;
use serde_json::value::RawValue;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult};

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
        temperature: Option<f32>,
        provider_params: Option<&Value>,
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
                .stream_response(messages, tools, max_tokens, temperature, provider_params)
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

                    let mut effective_max_tokens = self.config.max_tokens_per_turn;
                    let mut effective_temperature = self.config.temperature;
                    let mut effective_provider_params = self.config.provider_params.clone();

                    // Pre-LLM hooks may rewrite request params or deny the turn.
                    let pre_llm_report = self
                        .execute_hooks(
                            HookInvocation {
                                point: HookPoint::PreLlmRequest,
                                session_id: self.session.id().clone(),
                                turn_number: Some(turn_count),
                                prompt: None,
                                error: None,
                                llm_request: Some(HookLlmRequest {
                                    max_tokens: effective_max_tokens,
                                    temperature: effective_temperature,
                                    provider_params: effective_provider_params.clone(),
                                    message_count: self.session.messages().len(),
                                }),
                                llm_response: None,
                                tool_call: None,
                                tool_result: None,
                            },
                            event_tx.as_ref(),
                        )
                        .await?;

                    if let Some(HookDecision::Deny {
                        reason_code,
                        message,
                        payload,
                        ..
                    }) = pre_llm_report.decision
                    {
                        return Err(AgentError::HookDenied {
                            point: HookPoint::PreLlmRequest,
                            reason_code,
                            message,
                            payload,
                        });
                    }

                    for patch in pre_llm_report.patches {
                        if let HookPatch::LlmRequest {
                            max_tokens,
                            temperature,
                            provider_params,
                        } = patch
                        {
                            emit_event!(AgentEvent::HookRewriteApplied {
                                hook_id: "runtime".to_string(),
                                point: HookPoint::PreLlmRequest,
                                patch: HookPatch::LlmRequest {
                                    max_tokens,
                                    temperature,
                                    provider_params: provider_params.clone(),
                                },
                            });
                            if let Some(value) = max_tokens {
                                effective_max_tokens = value;
                            }
                            if temperature.is_some() {
                                effective_temperature = temperature;
                            }
                            if provider_params.is_some() {
                                effective_provider_params = provider_params;
                            }
                        }
                    }

                    // Call LLM with retry
                    let result = self
                        .call_llm_with_retry(
                            self.session.messages(),
                            &tool_defs,
                            effective_max_tokens,
                            effective_temperature,
                            effective_provider_params.as_ref(),
                        )
                        .await?;

                    // Update budget + session usage
                    self.budget.record_usage(&result.usage);
                    self.session.record_usage(result.usage.clone());

                    let (blocks, stop_reason, usage) = result.into_parts();
                    let mut assistant_msg = BlockAssistantMessage {
                        blocks,
                        stop_reason,
                    };
                    let mut assistant_text = assistant_msg.to_string();
                    if !assistant_text.is_empty() {
                        emit_event!(AgentEvent::TextComplete {
                            content: assistant_text.clone(),
                        });
                    }

                    let post_llm_report = self
                        .execute_hooks(
                            HookInvocation {
                                point: HookPoint::PostLlmResponse,
                                session_id: self.session.id().clone(),
                                turn_number: Some(turn_count),
                                prompt: None,
                                error: None,
                                llm_request: None,
                                llm_response: Some(HookLlmResponse {
                                    assistant_text: assistant_text.clone(),
                                }),
                                tool_call: None,
                                tool_result: None,
                            },
                            event_tx.as_ref(),
                        )
                        .await?;

                    if let Some(HookDecision::Deny {
                        reason_code,
                        message,
                        payload,
                        ..
                    }) = post_llm_report.decision
                    {
                        return Err(AgentError::HookDenied {
                            point: HookPoint::PostLlmResponse,
                            reason_code,
                            message,
                            payload,
                        });
                    }

                    for patch in post_llm_report.patches {
                        if let HookPatch::AssistantText { text } = patch {
                            emit_event!(AgentEvent::HookRewriteApplied {
                                hook_id: "runtime".to_string(),
                                point: HookPoint::PostLlmResponse,
                                patch: HookPatch::AssistantText { text: text.clone() },
                            });
                            rewrite_assistant_text(&mut assistant_msg.blocks, text);
                            assistant_text = assistant_msg.to_string();
                        }
                    }

                    // Check if we have tool calls
                    if assistant_msg.has_tool_calls() {
                        // Add assistant message with ordered blocks
                        self.session
                            .push(Message::BlockAssistant(assistant_msg.clone()));

                        // Emit tool call requests
                        for tc in assistant_msg.tool_calls() {
                            let args_value: Value = serde_json::from_str(tc.args.get())
                                .unwrap_or_else(|_| Value::String(tc.args.get().to_string()));
                            emit_event!(AgentEvent::ToolCallRequested {
                                id: tc.id.to_string(),
                                name: tc.name.to_string(),
                                args: args_value,
                            });
                        }

                        // Transition to waiting for ops
                        self.state.transition(LoopState::WaitingForOps)?;

                        // Execute tool calls in parallel
                        let tool_calls: Vec<ToolCallOwned> = assistant_msg
                            .tool_calls()
                            .map(ToolCallOwned::from_view)
                            .collect();
                        let tools_ref = Arc::clone(&self.tools);
                        let mut executable_tool_calls = Vec::new();
                        let mut tool_results = Vec::with_capacity(tool_calls.len());

                        for mut tc in tool_calls {
                            let args_value: Value = serde_json::from_str(tc.args.get())
                                .unwrap_or_else(|_| Value::String(tc.args.get().to_string()));

                            let pre_tool_report = self
                                .execute_hooks(
                                    HookInvocation {
                                        point: HookPoint::PreToolExecution,
                                        session_id: self.session.id().clone(),
                                        turn_number: Some(turn_count),
                                        prompt: None,
                                        error: None,
                                        llm_request: None,
                                        llm_response: None,
                                        tool_call: Some(HookToolCall {
                                            tool_use_id: tc.id.clone(),
                                            name: tc.name.clone(),
                                            args: args_value,
                                        }),
                                        tool_result: None,
                                    },
                                    event_tx.as_ref(),
                                )
                                .await?;

                            if let Some(HookDecision::Deny {
                                reason_code,
                                message,
                                payload,
                                ..
                            }) = pre_tool_report.decision
                            {
                                let denied_payload = serde_json::json!({
                                    "error": "hook_denied",
                                    "reason_code": serde_json::to_value(reason_code).unwrap_or_else(|_| Value::String("runtime_error".to_string())),
                                    "message": message,
                                    "payload": payload,
                                });
                                let denied_content = serde_json::to_string(&denied_payload)
                                    .unwrap_or_else(|_| {
                                        "{\"error\":\"hook_denied\",\"message\":\"denied by hook\"}"
                                            .to_string()
                                    });
                                tool_results.push(ToolResult {
                                    tool_use_id: tc.id.clone(),
                                    content: denied_content,
                                    is_error: true,
                                    thought_signature: None,
                                });
                                emit_event!(AgentEvent::ToolExecutionCompleted {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    result: tool_results
                                        .last()
                                        .map(|r| r.content.clone())
                                        .unwrap_or_default(),
                                    is_error: true,
                                    duration_ms: 0,
                                });
                                emit_event!(AgentEvent::ToolResultReceived {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    is_error: true,
                                });
                                self.budget.record_tool_call();
                                tool_call_count += 1;
                                continue;
                            }

                            for patch in pre_tool_report.patches {
                                if let HookPatch::ToolArgs { args } = patch {
                                    emit_event!(AgentEvent::HookRewriteApplied {
                                        hook_id: "runtime".to_string(),
                                        point: HookPoint::PreToolExecution,
                                        patch: HookPatch::ToolArgs { args: args.clone() },
                                    });
                                    tc.set_args(args);
                                }
                            }

                            emit_event!(AgentEvent::ToolExecutionStarted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                            });
                            executable_tool_calls.push(tc);
                        }

                        // Execute all allowed tool calls in parallel using join_all
                        let dispatch_futures: Vec<_> = executable_tool_calls
                            .into_iter()
                            .map(|tc| {
                                let tools_ref = Arc::clone(&tools_ref);
                                async move {
                                    let start = std::time::Instant::now();
                                    let dispatch_result = tools_ref.dispatch(tc.as_view()).await;
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    (tc, dispatch_result, duration_ms)
                                }
                            })
                            .collect();

                        let dispatch_results = futures::future::join_all(dispatch_futures).await;

                        // Process results and emit events
                        for (tc, dispatch_result, duration_ms) in dispatch_results {
                            let mut tool_result = match dispatch_result {
                                Ok(result) => result,
                                Err(crate::error::ToolError::CallbackPending {
                                    tool_name: callback_tool,
                                    args: callback_args,
                                }) => {
                                    // Merge tool_use_id into args for external handler
                                    let mut merged_args =
                                        callback_args.as_object().cloned().unwrap_or_default();
                                    merged_args.insert(
                                        "tool_use_id".to_string(),
                                        Value::String(tc.id.clone()),
                                    );
                                    return Err(AgentError::CallbackPending {
                                        tool_name: callback_tool,
                                        args: Value::Object(merged_args),
                                    });
                                }
                                Err(e) => {
                                    let payload = e.to_error_payload();
                                    let serialized = serde_json::to_string(&payload)
                                        .unwrap_or_else(|_| {
                                            "{\"error\":\"tool_error\",\"message\":\"tool error\"}"
                                                .to_string()
                                        });
                                    ToolResult {
                                        tool_use_id: tc.id.clone(),
                                        content: serialized,
                                        is_error: true,
                                        thought_signature: None,
                                    }
                                }
                            };

                            if tool_result.tool_use_id.is_empty() {
                                tool_result.tool_use_id = tc.id.clone();
                            }

                            let post_tool_report = self
                                .execute_hooks(
                                    HookInvocation {
                                        point: HookPoint::PostToolExecution,
                                        session_id: self.session.id().clone(),
                                        turn_number: Some(turn_count),
                                        prompt: None,
                                        error: None,
                                        llm_request: None,
                                        llm_response: None,
                                        tool_call: None,
                                        tool_result: Some(HookToolResult {
                                            tool_use_id: tc.id.clone(),
                                            name: tc.name.clone(),
                                            content: tool_result.content.clone(),
                                            is_error: tool_result.is_error,
                                        }),
                                    },
                                    event_tx.as_ref(),
                                )
                                .await?;

                            if let Some(HookDecision::Deny {
                                reason_code,
                                message,
                                payload,
                                ..
                            }) = post_tool_report.decision
                            {
                                let denied_payload = serde_json::json!({
                                    "error": "hook_denied",
                                    "reason_code": serde_json::to_value(reason_code).unwrap_or_else(|_| Value::String("runtime_error".to_string())),
                                    "message": message,
                                    "payload": payload,
                                });
                                tool_result.content = serde_json::to_string(&denied_payload)
                                    .unwrap_or_else(|_| {
                                        "{\"error\":\"hook_denied\",\"message\":\"denied by hook\"}"
                                            .to_string()
                                    });
                                tool_result.is_error = true;
                            }

                            for patch in post_tool_report.patches {
                                if let HookPatch::ToolResult { content, is_error } = patch {
                                    emit_event!(AgentEvent::HookRewriteApplied {
                                        hook_id: "runtime".to_string(),
                                        point: HookPoint::PostToolExecution,
                                        patch: HookPatch::ToolResult {
                                            content: content.clone(),
                                            is_error,
                                        },
                                    });
                                    tool_result.content = content;
                                    if let Some(value) = is_error {
                                        tool_result.is_error = value;
                                    }
                                }
                            }

                            // Emit execution complete
                            emit_event!(AgentEvent::ToolExecutionCompleted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                result: tool_result.content.clone(),
                                is_error: tool_result.is_error,
                                duration_ms,
                            });

                            // Emit result received
                            emit_event!(AgentEvent::ToolResultReceived {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                is_error: tool_result.is_error,
                            });

                            tool_results.push(tool_result);

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

                        let turn_boundary_report = self
                            .execute_hooks(
                                HookInvocation {
                                    point: HookPoint::TurnBoundary,
                                    session_id: self.session.id().clone(),
                                    turn_number: Some(turn_count),
                                    prompt: None,
                                    error: None,
                                    llm_request: None,
                                    llm_response: None,
                                    tool_call: None,
                                    tool_result: None,
                                },
                                event_tx.as_ref(),
                            )
                            .await?;
                        if let Some(HookDecision::Deny {
                            reason_code,
                            message,
                            payload,
                            ..
                        }) = turn_boundary_report.decision
                        {
                            return Err(AgentError::HookDenied {
                                point: HookPoint::TurnBoundary,
                                reason_code,
                                message,
                                payload,
                            });
                        }

                        // === END TURN BOUNDARY ===

                        self.state.transition(LoopState::CallingLlm)?;
                        turn_count += 1;
                    } else {
                        // No tool calls - we're done with the agentic loop
                        let final_text = assistant_text.clone();
                        self.session.push(Message::BlockAssistant(assistant_msg));

                        // Emit turn completed
                        emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                        // Check if we need to perform extraction turn for structured output
                        if self.config.output_schema.is_some() {
                            // Perform extraction turn to get validated JSON
                            let extraction_result = self
                                .perform_extraction_turn(turn_count, tool_call_count)
                                .await;

                            // Transition to completed regardless of extraction result
                            self.state.transition(LoopState::DrainingEvents)?;
                            self.state.transition(LoopState::Completed)?;

                            // Save session
                            if let Err(e) = self.store.save(&self.session).await {
                                tracing::warn!("Failed to save session: {}", e);
                            }

                            // Emit run completed (use extraction result text if successful)
                            if let Ok(ref result) = extraction_result {
                                emit_event!(AgentEvent::RunCompleted {
                                    session_id: self.session.id().clone(),
                                    result: result.text.clone(),
                                    usage: self.session.total_usage(),
                                });
                            }

                            return extraction_result;
                        }

                        // No extraction needed - complete normally
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
                            structured_output: None,
                            schema_warnings: None,
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
            text: self.session.last_assistant_text().unwrap_or_default(),
            session_id: self.session.id().clone(),
            usage: self.session.total_usage(),
            turns,
            tool_calls,
            structured_output: None,
            schema_warnings: None,
        }
    }
}

fn rewrite_assistant_text(blocks: &mut Vec<AssistantBlock>, replacement: String) {
    for block in blocks.iter_mut() {
        if let AssistantBlock::Text { text, .. } = block {
            *text = replacement;
            return;
        }
    }
    blocks.insert(
        0,
        AssistantBlock::Text {
            text: replacement,
            meta: None,
        },
    );
}

#[derive(Debug, Clone)]
struct ToolCallOwned {
    id: String,
    name: String,
    args: Box<RawValue>,
}

impl ToolCallOwned {
    fn from_view(view: ToolCallView<'_>) -> Self {
        let args = RawValue::from_string(view.args.get().to_string())
            .unwrap_or_else(|_| fallback_raw_value());
        Self {
            id: view.id.to_string(),
            name: view.name.to_string(),
            args,
        }
    }

    fn as_view(&self) -> ToolCallView<'_> {
        ToolCallView {
            id: &self.id,
            name: &self.name,
            args: &self.args,
        }
    }

    fn set_args(&mut self, args: Value) {
        let raw = RawValue::from_string(args.to_string()).unwrap_or_else(|_| fallback_raw_value());
        self.args = raw;
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<RawValue> {
    RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

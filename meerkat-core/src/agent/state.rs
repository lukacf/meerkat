//! Agent state machine internals.

use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType};
use crate::event_tap::tap_try_send;
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
    #[allow(unused_assignments)]
    pub(super) async fn run_loop(
        &mut self,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let mut turn_count = 0u32;
        let max_turns = self.config.max_turns.unwrap_or(100);
        let mut tool_call_count = 0u32;
        let mut event_stream_open = true;

        // Helper to conditionally emit events (only when listener exists)
        macro_rules! emit_event {
            ($event:expr) => {
                if event_stream_open {
                    let event = $event;
                    tap_try_send(&self.event_tap, event.clone());
                    if let Some(ref tx) = event_tx {
                        if tx.send(event).await.is_err() {
                            event_stream_open = false;
                            tracing::warn!(
                                "agent event stream receiver dropped; continuing without streaming events"
                            );
                        }
                    }
                }
            };
        }

        loop {
            if self.state == LoopState::CallingLlm {
                self.drain_comms_inbox().await;
            }

            // Check turn limit
            if turn_count >= max_turns {
                self.state.transition(LoopState::Completed)?;
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
                self.state.transition(LoopState::Completed)?;
                return Ok(self.build_result(turn_count, tool_call_count));
            }

            // Check compaction trigger (before CallingLlm)
            if self.state == LoopState::CallingLlm {
                if let Some(ref compactor) = self.compactor {
                    let ctx = crate::agent::compact::build_compaction_context(
                        self.session.messages(),
                        self.last_input_tokens,
                        self.last_compaction_turn,
                        turn_count,
                    );
                    if compactor.should_compact(&ctx) {
                        let outcome = crate::agent::compact::run_compaction(
                            self.client.as_ref(),
                            compactor,
                            self.session.messages(),
                            self.last_input_tokens,
                            turn_count,
                            &self.event_tap,
                            &event_tx,
                        )
                        .await;

                        if let Ok(outcome) = outcome {
                            // Replace session messages
                            *self.session.messages_mut() = outcome.new_messages;
                            // Record compaction usage
                            self.session.record_usage(outcome.summary_usage.clone());
                            self.budget.record_usage(&outcome.summary_usage);
                            // Update tracking
                            self.last_input_tokens = 0;
                            self.last_compaction_turn = Some(turn_count);

                            // Index discarded messages into memory store (fire-and-forget)
                            if let Some(ref memory_store) = self.memory_store {
                                let store = Arc::clone(memory_store);
                                let session_id = self.session.id().clone();
                                let discarded = outcome.discarded;
                                tokio::spawn(async move {
                                    for message in &discarded {
                                        let content = message.as_indexable_text();
                                        if !content.is_empty() {
                                            let metadata = crate::memory::MemoryMetadata {
                                                session_id: session_id.clone(),
                                                turn: Some(turn_count),
                                                indexed_at: std::time::SystemTime::now(),
                                            };
                                            if let Err(e) = store.index(&content, metadata).await {
                                                tracing::warn!(
                                                    "failed to index compaction discard into memory: {e}"
                                                );
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        // On failure: non-fatal, continue with uncompacted history
                    }
                }
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

                    for outcome in &pre_llm_report.outcomes {
                        for patch in &outcome.patches {
                            if let HookPatch::LlmRequest {
                                max_tokens,
                                temperature,
                                provider_params,
                            } = patch
                            {
                                emit_event!(AgentEvent::HookRewriteApplied {
                                    hook_id: outcome.hook_id.to_string(),
                                    point: HookPoint::PreLlmRequest,
                                    patch: HookPatch::LlmRequest {
                                        max_tokens: *max_tokens,
                                        temperature: *temperature,
                                        provider_params: provider_params.clone(),
                                    },
                                });
                                if let Some(value) = max_tokens {
                                    effective_max_tokens = *value;
                                }
                                if temperature.is_some() {
                                    effective_temperature = *temperature;
                                }
                                if provider_params.is_some() {
                                    effective_provider_params = provider_params.clone();
                                }
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
                    self.last_input_tokens = result.usage.input_tokens;
                    self.session.record_usage(result.usage.clone());

                    let (blocks, stop_reason, usage) = result.into_parts();
                    let mut assistant_msg = BlockAssistantMessage {
                        blocks,
                        stop_reason,
                    };
                    let mut assistant_text = assistant_msg.to_string();

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
                                    tool_call_names: assistant_msg
                                        .tool_calls()
                                        .map(|call| call.name.to_string())
                                        .collect(),
                                    stop_reason: Some(stop_reason),
                                    usage: Some(usage.clone()),
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

                    for outcome in &post_llm_report.outcomes {
                        for patch in &outcome.patches {
                            if let HookPatch::AssistantText { text } = patch {
                                emit_event!(AgentEvent::HookRewriteApplied {
                                    hook_id: outcome.hook_id.to_string(),
                                    point: HookPoint::PostLlmResponse,
                                    patch: HookPatch::AssistantText { text: text.clone() },
                                });
                                rewrite_assistant_text(&mut assistant_msg.blocks, text.clone());
                                assistant_text = assistant_msg.to_string();
                            }
                        }
                    }

                    if !assistant_text.is_empty() {
                        emit_event!(AgentEvent::TextComplete {
                            content: assistant_text.clone(),
                        });
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

                        let pre_tool_reports =
                            futures::future::join_all(tool_calls.iter().map(|tc| {
                                let args_value: Value = serde_json::from_str(tc.args.get())
                                    .unwrap_or_else(|_| Value::String(tc.args.get().to_string()));
                                self.execute_hooks(
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
                            }))
                            .await;

                        for (mut tc, pre_tool_report) in
                            tool_calls.into_iter().zip(pre_tool_reports.into_iter())
                        {
                            let pre_tool_report = pre_tool_report?;

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

                            for outcome in &pre_tool_report.outcomes {
                                for patch in &outcome.patches {
                                    if let HookPatch::ToolArgs { args } = patch {
                                        emit_event!(AgentEvent::HookRewriteApplied {
                                            hook_id: outcome.hook_id.to_string(),
                                            point: HookPoint::PreToolExecution,
                                            patch: HookPatch::ToolArgs { args: args.clone() },
                                        });
                                        tc.set_args(args.clone());
                                    }
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

                            for outcome in &post_tool_report.outcomes {
                                for patch in &outcome.patches {
                                    if let HookPatch::ToolResult { content, is_error } = patch {
                                        emit_event!(AgentEvent::HookRewriteApplied {
                                            hook_id: outcome.hook_id.to_string(),
                                            point: HookPoint::PostToolExecution,
                                            patch: HookPatch::ToolResult {
                                                content: content.clone(),
                                                is_error: *is_error,
                                            },
                                        });
                                        tool_result.content = content.clone();
                                        if let Some(value) = is_error {
                                            tool_result.is_error = *value;
                                        }
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

pub(crate) fn rewrite_assistant_text(blocks: &mut Vec<AssistantBlock>, replacement: String) {
    let first_text_idx = blocks
        .iter()
        .position(|block| matches!(block, AssistantBlock::Text { .. }));

    if let Some(idx) = first_text_idx {
        if let AssistantBlock::Text { text, .. } = &mut blocks[idx] {
            *text = replacement;
        }
        let mut i = idx + 1;
        while i < blocks.len() {
            if matches!(blocks[i], AssistantBlock::Text { .. }) {
                blocks.remove(i);
            } else {
                i += 1;
            }
        }
        return;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::rewrite_assistant_text;
    use crate::AgentBuilder;
    use crate::budget::BudgetLimits;
    use crate::error::{AgentError, ToolError};
    use crate::interaction::{InboxInteraction, InteractionContent, InteractionId};
    use crate::state::LoopState;
    use crate::types::AssistantBlock;
    use crate::types::{Message, StopReason, ToolCallView, ToolDef, ToolResult, Usage};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::{Mutex, Notify};
    use uuid::Uuid;

    #[test]
    fn rewrite_assistant_text_rewrites_all_text_blocks() {
        let mut blocks = vec![
            AssistantBlock::Text {
                text: "first".to_string(),
                meta: None,
            },
            AssistantBlock::ToolUse {
                id: "t1".to_string(),
                name: "tool".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            },
            AssistantBlock::Text {
                text: "second".to_string(),
                meta: None,
            },
        ];

        rewrite_assistant_text(&mut blocks, "redacted".to_string());

        let text_blocks: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                AssistantBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(text_blocks, vec!["redacted"]);
    }

    struct MockClient;

    #[async_trait]
    impl crate::agent::AgentLlmClient for MockClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<crate::LlmStreamResult, AgentError> {
            Ok(crate::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    struct MockTools;

    #[async_trait]
    impl crate::agent::AgentToolDispatcher for MockTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.to_string(),
            })
        }
    }

    struct MockStore;

    #[async_trait]
    impl crate::agent::AgentSessionStore for MockStore {
        async fn save(&self, _session: &crate::Session) -> Result<(), AgentError> {
            Ok(())
        }
        async fn load(&self, _id: &str) -> Result<Option<crate::Session>, AgentError> {
            Ok(None)
        }
    }

    struct MockInteractionRuntime {
        interactions: Mutex<Vec<InboxInteraction>>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl crate::agent::CommsRuntime for MockInteractionRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        async fn drain_interactions(&self) -> Vec<InboxInteraction> {
            let mut guard = self.interactions.lock().await;
            std::mem::take(&mut *guard)
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }
    }

    #[tokio::test]
    async fn run_loop_max_turn_from_calling_llm_completes() {
        let mut agent = AgentBuilder::new()
            .build(
                Arc::new(MockClient),
                Arc::new(MockTools),
                Arc::new(MockStore),
            )
            .await;
        agent.config.max_turns = Some(0);
        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(*agent.state(), LoopState::Completed);
        assert_eq!(result.turns, 0);
    }

    #[tokio::test]
    async fn run_loop_budget_exhausted_from_calling_llm_completes() {
        let mut agent = AgentBuilder::new()
            .budget(BudgetLimits::default().with_max_tokens(0))
            .build(
                Arc::new(MockClient),
                Arc::new(MockTools),
                Arc::new(MockStore),
            )
            .await;
        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(*agent.state(), LoopState::Completed);
        assert_eq!(result.turns, 0);
    }

    #[tokio::test]
    async fn run_loop_invalid_completed_to_completed_is_error() {
        let mut agent = AgentBuilder::new()
            .build(
                Arc::new(MockClient),
                Arc::new(MockTools),
                Arc::new(MockStore),
            )
            .await;
        agent.config.max_turns = Some(0);
        agent.state = LoopState::Completed;
        let result = agent.run_loop(None).await;
        assert!(matches!(
            result,
            Err(AgentError::InvalidStateTransition { .. })
        ));
    }

    #[tokio::test]
    async fn run_loop_error_recovery_to_completed_is_valid() {
        let mut agent = AgentBuilder::new()
            .build(
                Arc::new(MockClient),
                Arc::new(MockTools),
                Arc::new(MockStore),
            )
            .await;
        agent.config.max_turns = Some(0);
        agent.state = LoopState::ErrorRecovery;
        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(*agent.state(), LoopState::Completed);
        assert_eq!(result.turns, 0);
    }

    #[tokio::test]
    async fn error_recovery_to_calling_llm_drains_comms_inbox() {
        let runtime = Arc::new(MockInteractionRuntime {
            interactions: Mutex::new(vec![InboxInteraction {
                id: InteractionId(Uuid::now_v7()),
                from: "ext".to_string(),
                content: InteractionContent::Message {
                    body: "injected during recovery".to_string(),
                },
                rendered_text: "injected during recovery".to_string(),
            }]),
            notify: Arc::new(Notify::new()),
        });

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(runtime as Arc<dyn crate::agent::CommsRuntime>)
            .build(
                Arc::new(MockClient),
                Arc::new(MockTools),
                Arc::new(MockStore),
            )
            .await;
        agent.state = LoopState::ErrorRecovery;
        agent.config.max_turns = Some(1);

        let _ = agent.run_loop(None).await.unwrap();
        let saw_injected = agent.session.messages().iter().any(|m| match m {
            Message::User(u) => u.content.contains("injected during recovery"),
            _ => false,
        });
        assert!(saw_injected);
    }
}

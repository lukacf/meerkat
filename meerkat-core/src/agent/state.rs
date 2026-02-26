//! Agent state machine internals.

use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType};
use crate::hooks::{
    HookDecision, HookInvocation, HookLlmRequest, HookLlmResponse, HookPatch, HookPoint,
    HookToolCall, HookToolResult,
};
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
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

        // Helper to conditionally emit events (only when listener exists).
        // Also forwards to the event tap for interaction-scoped subscribers.
        macro_rules! emit_event {
            ($event:expr) => {
                {
                    let event = $event;
                    crate::event_tap::tap_try_send(&self.event_tap, &event);
                    if event_stream_open {
                        if let Some(ref tx) = event_tx {
                            if tx.send(event).await.is_err() {
                                event_stream_open = false;
                                tracing::warn!(
                                    "agent event stream receiver dropped; continuing without streaming events"
                                );
                            }
                        }
                    }
                }
            };
        }

        loop {
            // Drain comms inbox at top of loop when entering CallingLlm.
            // Catches messages during ErrorRecovery -> CallingLlm transitions
            // and the window between turn-boundary drain and next LLM call.
            if self.state == LoopState::CallingLlm {
                self.drain_comms_inbox().await;
            }

            // Check turn limit
            if turn_count >= max_turns {
                self.state.transition(LoopState::Completed)?;
                return Ok(self.build_result(turn_count, tool_call_count).await);
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
                return Ok(self.build_result(turn_count, tool_call_count).await);
            }

            // Check compaction trigger (before CallingLlm)
            if self.state == LoopState::CallingLlm
                && let Some(ref compactor) = self.compactor
            {
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
                        &event_tx,
                        &self.event_tap,
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
                                            indexed_at: crate::time_compat::SystemTime::now(),
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
                                    let start = crate::time_compat::Instant::now();
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
                            skill_diagnostics: self.collect_skill_diagnostics().await,
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
                    return Ok(self.build_result(turn_count, tool_call_count).await);
                }
                LoopState::ErrorRecovery => {
                    // Attempt recovery
                    self.state.transition(LoopState::CallingLlm)?;
                }
                LoopState::Completed => {
                    return Ok(self.build_result(turn_count, tool_call_count).await);
                }
            }
        }
    }

    /// Build a RunResult from current state
    async fn build_result(&self, turns: u32, tool_calls: u32) -> RunResult {
        RunResult {
            text: self.session.last_assistant_text().unwrap_or_default(),
            session_id: self.session.id().clone(),
            usage: self.session.total_usage(),
            turns,
            tool_calls,
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: self.collect_skill_diagnostics().await,
        }
    }

    async fn collect_skill_diagnostics(&self) -> Option<crate::skills::SkillRuntimeDiagnostics> {
        let runtime = self.skill_engine.as_ref()?;
        let source_health = runtime.health_snapshot().await.ok()?;
        let quarantined = runtime.quarantined_diagnostics().await.unwrap_or_default();
        Some(crate::skills::SkillRuntimeDiagnostics {
            source_health,
            quarantined,
        })
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::manual_async_fn)]
mod tests {
    use super::rewrite_assistant_text;
    use crate::agent::{AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
    use crate::budget::{Budget, BudgetLimits};
    use crate::error::{AgentError, ToolError};
    use crate::skills::{
        ResolvedSkill, SkillCollection, SkillDescriptor, SkillEngine, SkillFilter, SkillId,
        SkillKey, SkillName, SourceUuid,
    };
    use crate::state::LoopState;
    use crate::types::{
        AssistantBlock, Message, StopReason, ToolCallView, ToolDef, ToolResult, Usage,
    };
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

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

    struct StaticLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for StaticLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            Ok(super::LlmStreamResult::new(
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

    struct RecordingLlmClient {
        seen_user_messages: Mutex<Vec<String>>,
    }

    impl RecordingLlmClient {
        fn new() -> Self {
            Self {
                seen_user_messages: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<String> {
            self.seen_user_messages.lock().unwrap().clone()
        }
    }

    struct RecordingSkillEngine {
        seen_ids: Mutex<Vec<SkillId>>,
    }

    impl RecordingSkillEngine {
        fn new() -> Self {
            Self {
                seen_ids: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<SkillId> {
            self.seen_ids.lock().unwrap().clone()
        }
    }

    impl SkillEngine for RecordingSkillEngine {
        fn inventory_section(
            &self,
        ) -> impl Future<Output = Result<String, crate::skills::SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            ids: &[SkillId],
        ) -> impl Future<Output = Result<Vec<ResolvedSkill>, crate::skills::SkillError>> + Send
        {
            let ids = ids.to_vec();
            async move {
                let mut seen = self.seen_ids.lock().unwrap();
                seen.extend_from_slice(&ids);
                drop(seen);

                Ok(vec![ResolvedSkill {
                    id: ids.first().cloned().unwrap_or_else(|| {
                        SkillId("dc256086-0d2f-4f61-a307-320d4148107f/email-extractor".to_string())
                    }),
                    name: "email-extractor".to_string(),
                    rendered_body: "<skill>injected canonical skill</skill>".to_string(),
                    byte_size: 34,
                }])
            }
        }

        fn collections(
            &self,
        ) -> impl Future<Output = Result<Vec<SkillCollection>, crate::skills::SkillError>> + Send
        {
            async move { Ok(vec![]) }
        }

        fn list_skills(
            &self,
            _filter: &SkillFilter,
        ) -> impl Future<Output = Result<Vec<SkillDescriptor>, crate::skills::SkillError>> + Send
        {
            async move { Ok(vec![]) }
        }

        fn quarantined_diagnostics(
            &self,
        ) -> impl Future<
            Output = Result<
                Vec<crate::skills::SkillQuarantineDiagnostic>,
                crate::skills::SkillError,
            >,
        > + Send {
            async move { Ok(Vec::new()) }
        }

        fn health_snapshot(
            &self,
        ) -> impl Future<
            Output = Result<crate::skills::SourceHealthSnapshot, crate::skills::SkillError>,
        > + Send {
            async move { Ok(crate::skills::SourceHealthSnapshot::default()) }
        }

        fn list_artifacts(
            &self,
            id: &SkillId,
        ) -> impl Future<
            Output = Result<Vec<crate::skills::SkillArtifact>, crate::skills::SkillError>,
        > + Send {
            let missing = id.clone();
            async move { Err(crate::skills::SkillError::NotFound { id: missing }) }
        }

        fn read_artifact(
            &self,
            id: &SkillId,
            _artifact_path: &str,
        ) -> impl Future<
            Output = Result<crate::skills::SkillArtifactContent, crate::skills::SkillError>,
        > + Send {
            let missing = id.clone();
            async move { Err(crate::skills::SkillError::NotFound { id: missing }) }
        }

        fn invoke_function(
            &self,
            id: &SkillId,
            _function_name: &str,
            _arguments: Value,
        ) -> impl Future<Output = Result<Value, crate::skills::SkillError>> + Send {
            let missing = id.clone();
            async move { Err(crate::skills::SkillError::NotFound { id: missing }) }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for RecordingLlmClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut seen = self.seen_user_messages.lock().unwrap();
            for msg in messages {
                if let Message::User(user) = msg {
                    seen.push(user.content.clone());
                }
            }
            drop(seen);

            Ok(super::LlmStreamResult::new(
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

    struct NoTools;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for NoTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.to_string(),
            })
        }
    }

    struct NoopStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentSessionStore for NoopStore {
        async fn save(&self, _session: &crate::session::Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<crate::session::Session>, AgentError> {
            Ok(None)
        }
    }

    struct MockDrainCommsRuntime {
        queued: tokio::sync::Mutex<Vec<String>>,
        notify: Arc<Notify>,
    }

    impl MockDrainCommsRuntime {
        fn with_messages(messages: Vec<String>) -> Self {
            Self {
                queued: tokio::sync::Mutex::new(messages),
                notify: Arc::new(Notify::new()),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl crate::agent::CommsRuntime for MockDrainCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            let mut guard = self.queued.lock().await;
            std::mem::take(&mut *guard)
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }
    }

    async fn build_agent<C>(client: Arc<C>) -> crate::agent::Agent<C, NoTools, NoopStore>
    where
        C: AgentLlmClient + ?Sized + 'static,
    {
        AgentBuilder::new()
            .build(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await
    }

    #[tokio::test]
    async fn calling_llm_with_max_turns_zero_completes_with_zero_turns() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(0);
        agent.state = LoopState::CallingLlm;

        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(agent.state, LoopState::Completed);
    }

    #[tokio::test]
    async fn calling_llm_with_budget_exhausted_completes_with_zero_turns() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(10);
        agent.state = LoopState::CallingLlm;
        agent.budget = Budget::new(BudgetLimits {
            max_tokens: Some(0),
            max_duration: None,
            max_tool_calls: None,
        });

        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(agent.state, LoopState::Completed);
    }

    #[tokio::test]
    async fn completed_with_max_turns_zero_returns_invalid_transition() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(0);
        agent.state = LoopState::Completed;

        let err = agent
            .run_loop(None)
            .await
            .expect_err("expected transition error");
        let AgentError::InvalidStateTransition { from, to } = err else {
            unreachable!("expected InvalidStateTransition, got {err:?}");
        };
        assert_eq!(from, "Completed");
        assert_eq!(to, "Completed");
    }

    #[tokio::test]
    async fn error_recovery_with_max_turns_zero_completes() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(0);
        agent.state = LoopState::ErrorRecovery;

        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(agent.state, LoopState::Completed);
    }

    #[tokio::test]
    async fn error_recovery_drains_comms_message_when_transitioning_to_calling_llm() {
        let client = Arc::new(RecordingLlmClient::new());
        let comms = Arc::new(MockDrainCommsRuntime::with_messages(vec![
            "queued during recovery".to_string(),
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms)
            .build(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.config.max_turns = Some(1);
        agent.state = LoopState::ErrorRecovery;

        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(result.turns, 1);

        let seen = client.seen();
        assert!(
            seen.iter().any(|m| m.contains("queued during recovery")),
            "expected queued comms message to be drained into LLM input, saw: {:?}",
            seen
        );
    }

    #[tokio::test]
    async fn pending_skill_keys_are_resolved_and_injected_into_runtime_prompt() {
        let client = Arc::new(RecordingLlmClient::new());
        let skill_engine = Arc::new(RecordingSkillEngine::new());
        let skill_runtime = Arc::new(crate::skills::SkillRuntime::new(skill_engine.clone()));
        let mut agent = AgentBuilder::new()
            .with_skill_engine(skill_runtime)
            .build(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.pending_skill_references = Some(vec![SkillKey {
            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid source uuid"),
            skill_name: SkillName::parse("email-extractor").expect("valid skill name"),
        }]);
        agent.config.max_turns = Some(1);

        let result = agent
            .run("plain user prompt".to_string())
            .await
            .expect("run should succeed");
        assert_eq!(result.turns, 1);

        let seen_ids = skill_engine.seen();
        assert!(
            seen_ids
                .iter()
                .any(|id| id.0 == "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"),
            "expected canonical skill id to be forwarded to skill engine, saw: {:?}",
            seen_ids
        );

        let seen_messages = client.seen();
        assert!(
            seen_messages
                .iter()
                .any(|msg| msg.contains("<skill>injected canonical skill</skill>")),
            "expected runtime prompt to include rendered skill injection, saw: {:?}",
            seen_messages
        );
    }
}

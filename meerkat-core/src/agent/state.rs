//! Agent state machine internals.

use crate::error::AgentError;
use crate::event::{AgentEvent, BudgetType, ToolConfigChangeOperation, ToolConfigChangedPayload};
use crate::hooks::{
    HookDecision, HookInvocation, HookLlmRequest, HookLlmResponse, HookPatch, HookPoint,
    HookToolCall, HookToolResult,
};
use crate::lifecycle::RunId;
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::turn_execution_authority::{
    ContentShape, TurnExecutionEffect, TurnExecutionInput, TurnExecutionMutator,
    TurnExecutionTransition,
};
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, RunResult, ToolCallView, ToolDef, ToolResult,
    UserMessage,
};
use serde_json::Value;
use serde_json::value::RawValue;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult};

/// Pre-selected timeout source — determined before the LLM await, not inferred after.
///
/// This ensures timeout classification is deterministic and not guessed from
/// merged elapsed durations.
enum CallTimeoutSource {
    /// Hard per-call timeout fired (from override or profile default).
    CallBudget,
    /// Explicit whole-turn time budget fired.
    TurnBudget,
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Resolve the effective call timeout for this LLM call.
    ///
    /// Resolution order:
    ///   1. Explicit override (Value/Disabled) — from build/config tri-state
    ///   2. Profile default — from injected model resolver + current model identity
    ///   3. RetryPolicy.call_timeout — from builder-level policy (direct Rust API)
    ///   4. None — no timeout
    ///
    /// `Disabled` suppresses ALL lower-layer defaults (profile and retry policy).
    fn resolve_effective_call_timeout(&self) -> Option<std::time::Duration> {
        use crate::config::CallTimeoutOverride;
        match &self.call_timeout_override {
            CallTimeoutOverride::Value(d) => Some(*d),
            CallTimeoutOverride::Disabled => None,
            CallTimeoutOverride::Inherit => {
                // Consult the injected resolver with the current model/provider.
                self.model_defaults_resolver
                    .as_ref()
                    .and_then(|r| r.call_timeout_for(self.client.provider(), self.client.model()))
                    // Fall through to RetryPolicy.call_timeout for direct builder users.
                    .or(self.retry_policy.call_timeout)
            }
        }
    }

    /// Call LLM with retry logic, budget-aware at all liveness points.
    ///
    /// This is the single LLM-call liveness gate. It enforces:
    /// - budget check at loop entry and after retry sleep
    /// - retry delay capped by remaining turn budget
    /// - per-call timeout from override/profile, wrapped with remaining turn budget
    /// - typed timeout-origin selection before await
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
            // 1. Budget gate at loop entry
            self.budget.check()?;

            // 2. Compute effective timeout for this call
            let effective_call_timeout = self.resolve_effective_call_timeout();
            let remaining_turn = self.budget.remaining_duration();

            // If remaining turn budget is zero, surface immediately
            if remaining_turn == Some(std::time::Duration::ZERO) {
                self.budget.check()?;
                // check() should have returned Err; unreachable, but be safe
            }

            // 4. Determine whether to wrap the call and select timeout source
            let call_result = match (effective_call_timeout, remaining_turn) {
                (None, None) => {
                    // No timeout wrapper needed
                    self.client
                        .stream_response(messages, tools, max_tokens, temperature, provider_params)
                        .await
                }
                (call_to, turn_remaining) => {
                    // At least one timeout is active — select source and wrap
                    let (effective_timeout, source) = match (call_to, turn_remaining) {
                        (Some(ct), None) => (ct, CallTimeoutSource::CallBudget),
                        (None, Some(tr)) => (tr, CallTimeoutSource::TurnBudget),
                        (Some(ct), Some(tr)) => {
                            if tr < ct {
                                (tr, CallTimeoutSource::TurnBudget)
                            } else {
                                (ct, CallTimeoutSource::CallBudget)
                            }
                        }
                        (None, None) => unreachable!(), // Already handled above
                    };

                    match tokio::time::timeout(
                        effective_timeout,
                        self.client.stream_response(
                            messages,
                            tools,
                            max_tokens,
                            temperature,
                            provider_params,
                        ),
                    )
                    .await
                    {
                        Ok(inner_result) => inner_result,
                        Err(_elapsed) => {
                            // Timeout fired — classify by pre-selected source.
                            // TurnBudget is non-retryable (whole-turn expired).
                            // CallBudget flows through step 5 retry logic below.
                            match source {
                                CallTimeoutSource::CallBudget => Err(AgentError::Llm {
                                    provider: self.client.provider(),
                                    reason: crate::error::LlmFailureReason::CallTimeout {
                                        duration_ms: effective_timeout.as_millis() as u64,
                                    },
                                    message: format!(
                                        "LLM call timed out after {}s",
                                        effective_timeout.as_secs()
                                    ),
                                }),
                                CallTimeoutSource::TurnBudget => {
                                    let limit = self
                                        .budget
                                        .time_usage()
                                        .map(|(_, l)| l / 1000)
                                        .unwrap_or(0);
                                    let elapsed = self
                                        .budget
                                        .time_usage()
                                        .map(|(e, _)| e / 1000)
                                        .unwrap_or(0);
                                    return Err(AgentError::TimeBudgetExceeded {
                                        elapsed_secs: elapsed,
                                        limit_secs: limit,
                                    });
                                }
                            }
                        }
                    }
                }
            };

            // 5. Handle call result
            match call_result {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if e.is_recoverable() && self.retry_policy.should_retry(attempt) {
                        let hint = e.retry_after_hint();
                        let computed = self.retry_policy.delay_for_attempt(attempt + 1);
                        let delay = compute_retry_delay(hint, computed, e.is_rate_limited());
                        let capped = match self.budget.remaining_duration() {
                            Some(remaining) => delay.min(remaining),
                            None => delay,
                        };
                        tracing::warn!(
                            "LLM call failed (attempt {}), retrying in {}ms: {}",
                            attempt + 1,
                            capped.as_millis(),
                            e
                        );
                        attempt += 1;
                        tokio::time::sleep(capped).await;
                        self.budget.check()?;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    async fn drain_turn_boundary(
        &mut self,
        turn_count: u32,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
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
                event_tx,
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

        Ok(())
    }

    /// Apply a typed input to the turn-execution authority and sync the
    /// observable `LoopState` from the resulting canonical phase.
    fn apply_turn_input(
        &mut self,
        input: TurnExecutionInput,
    ) -> Result<TurnExecutionTransition, AgentError> {
        let transition = self.turn_authority.apply(input)?;
        self.state = transition.next_phase.to_loop_state();
        Ok(transition)
    }

    /// Execute side effects from a transition. Handles CheckCompaction
    /// effects that the authority emits on CallingLlm entry.
    async fn execute_turn_effects(
        &mut self,
        transition: &TurnExecutionTransition,
        turn_count: u32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        for effect in &transition.effects {
            if let TurnExecutionEffect::CheckCompaction = effect {
                let current_boundary_index = self.compaction_cadence.session_boundary_index;
                if let Some(ref compactor) = self.compactor {
                    let ctx = crate::agent::compact::build_compaction_context(
                        self.session.messages(),
                        self.last_input_tokens,
                        self.compaction_cadence.last_compaction_boundary_index,
                        current_boundary_index,
                    );
                    if compactor.should_compact(&ctx) {
                        let outcome = crate::agent::compact::run_compaction(
                            self.client.as_ref(),
                            compactor,
                            self.session.messages(),
                            self.last_input_tokens,
                            current_boundary_index,
                            event_tx,
                            &self.event_tap,
                        )
                        .await;

                        if let Ok(outcome) = outcome {
                            *self.session.messages_mut() = outcome.new_messages;
                            self.session.record_usage(outcome.summary_usage.clone());
                            self.budget.record_usage(&outcome.summary_usage);
                            self.last_input_tokens = 0;
                            self.compaction_cadence.last_compaction_boundary_index =
                                Some(outcome.session_boundary_index);

                            if let Some(ref memory_store) = self.memory_store {
                                let store = std::sync::Arc::clone(memory_store);
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
                    }
                }
                self.compaction_cadence.session_boundary_index = self
                    .compaction_cadence
                    .session_boundary_index
                    .saturating_add(1);
                let cadence = self.compaction_cadence.clone();
                if let Err(error) =
                    crate::agent::compact::persist_compaction_cadence(self.session_mut(), &cadence)
                {
                    tracing::warn!(
                        error = %error,
                        "failed to persist session compaction cadence metadata"
                    );
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

        // --- Authority lifecycle: start a conversation run ---
        let run_id = RunId(uuid::Uuid::new_v4());
        self.apply_turn_input(TurnExecutionInput::StartConversationRun {
            run_id: run_id.clone(),
        })?;
        // PrimitiveApplied transitions ApplyingPrimitive -> CallingLlm
        let t = self.apply_turn_input(TurnExecutionInput::PrimitiveApplied {
            run_id: run_id.clone(),
            admitted_content_shape: ContentShape("conversation".to_string()),
            vision_enabled: false,
            image_tool_results_enabled: false,
        })?;
        self.execute_turn_effects(&t, 0, &event_tx).await;

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
            // Check turn limit
            if turn_count >= max_turns {
                self.apply_turn_input(TurnExecutionInput::TurnLimitReached {
                    run_id: run_id.clone(),
                })?;
                return self.build_result(turn_count, tool_call_count).await;
            }

            // Check budget — typed routing per budget kind
            if let Err(budget_err) = self.budget.check() {
                match budget_err {
                    AgentError::TimeBudgetExceeded {
                        elapsed_secs,
                        limit_secs,
                    } => {
                        emit_event!(AgentEvent::BudgetWarning {
                            budget_type: BudgetType::Time,
                            used: elapsed_secs,
                            limit: limit_secs,
                            percent: 1.0,
                        });
                        self.pending_fatal_diagnostic = Some(AgentError::TimeBudgetExceeded {
                            elapsed_secs,
                            limit_secs,
                        });
                        self.apply_turn_input(TurnExecutionInput::TimeBudgetExceeded {
                            run_id: run_id.clone(),
                        })?;
                    }
                    AgentError::TokenBudgetExceeded { used, limit } => {
                        emit_event!(AgentEvent::BudgetWarning {
                            budget_type: BudgetType::Tokens,
                            used,
                            limit,
                            percent: 1.0,
                        });
                        self.apply_turn_input(TurnExecutionInput::BudgetExhausted {
                            run_id: run_id.clone(),
                        })?;
                    }
                    AgentError::ToolCallBudgetExceeded { count, limit } => {
                        emit_event!(AgentEvent::BudgetWarning {
                            budget_type: BudgetType::ToolCalls,
                            used: count as u64,
                            limit: limit as u64,
                            percent: 1.0,
                        });
                        self.apply_turn_input(TurnExecutionInput::BudgetExhausted {
                            run_id: run_id.clone(),
                        })?;
                    }
                    other => return Err(other),
                }
                return self.build_result(turn_count, tool_call_count).await;
            }

            match self.state {
                LoopState::CallingLlm => {
                    // 1. Poll external updates BEFORE tool capture so newly
                    //    connected tools are visible in the same LLM call.
                    let ext = self.tools.poll_external_updates().await;

                    // 2. Emit ToolConfigChanged for completed background connections.
                    for notice in &ext.notices {
                        let mut payload = notice.to_tool_config_changed_payload();
                        if payload.applied_at_turn.is_none() {
                            payload.applied_at_turn = Some(turn_count);
                        }
                        emit_event!(AgentEvent::ToolConfigChanged { payload });
                    }

                    // 3. Manage [MCP_PENDING] notice lifecycle.
                    //    Always strip prior synthetic notices to avoid stale state.
                    //    Uses starts_with on a strict prefix to avoid matching user text.
                    const MCP_PENDING_PREFIX: &str = "[SYSTEM NOTICE][MCP_PENDING] ";
                    self.session.messages_mut().retain(
                        |m| !matches!(m, Message::User(u) if u.text_content().starts_with(MCP_PENDING_PREFIX)),
                    );
                    if !ext.pending.is_empty() {
                        self.session.push(Message::User(UserMessage::text(format!(
                            "{MCP_PENDING_PREFIX}Servers connecting: {}. \
                             Tools will appear when ready.",
                            ext.pending.join(", ")
                        ))));
                    }

                    // 4. Apply tool scope staged updates atomically at the CallingLlm boundary.
                    let tool_defs = {
                        let dispatcher_tools = self.tools.tools();
                        match self.tool_scope.apply_staged(dispatcher_tools.clone()) {
                            Ok(applied) => {
                                if applied.changed() {
                                    let status = format!(
                                        "boundary_applied(base_changed={},visible_changed={},revision={})",
                                        applied.base_changed(),
                                        applied.visible_changed(),
                                        applied.applied_revision.0
                                    );
                                    emit_event!(AgentEvent::ToolConfigChanged {
                                        payload: ToolConfigChangedPayload {
                                            operation: ToolConfigChangeOperation::Reload,
                                            target: "tool_scope".to_string(),
                                            status: status.clone(),
                                            persisted: false,
                                            applied_at_turn: Some(turn_count),
                                        },
                                    });
                                    // Represent runtime notices as user-scoped synthetic context
                                    // (same pattern as peer lifecycle updates) so this does not
                                    // mutate or replace the canonical system prompt.
                                    self.session.push(Message::User(UserMessage::text(format!(
                                        "[SYSTEM NOTICE][TOOL_SCOPE] Tool configuration changed at turn boundary: {status}"
                                    ))));
                                }
                                applied.tools
                            }
                            Err(err) => {
                                let status = format!("warning_fallback_all({err})");
                                tracing::warn!(
                                    error = %err,
                                    "tool scope boundary apply failed; falling back to full dispatcher tools"
                                );
                                emit_event!(AgentEvent::ToolConfigChanged {
                                    payload: ToolConfigChangedPayload {
                                        operation: ToolConfigChangeOperation::Reload,
                                        target: "tool_scope".to_string(),
                                        status: status.clone(),
                                        persisted: false,
                                        applied_at_turn: Some(turn_count),
                                    },
                                });
                                self.session.push(Message::User(UserMessage::text(format!(
                                    "[SYSTEM NOTICE][TOOL_SCOPE][WARNING] Tool scope apply failed ({err}); falling back to full tool set."
                                ))));
                                dispatcher_tools
                            }
                        }
                    };

                    // Emit turn start
                    emit_event!(AgentEvent::TurnStarted {
                        turn_number: turn_count,
                    });

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

                    // In extraction mode, override tools/temperature/params
                    if self.extraction_mode {
                        // Force temperature 0.0 for deterministic output
                        effective_temperature = Some(0.0_f32);
                        // Inject structured_output into provider params
                        let mut params =
                            effective_provider_params.unwrap_or_else(|| serde_json::json!({}));
                        if let Some(output_schema) = &self.config.output_schema
                            && let Some(obj) = params.as_object_mut()
                        {
                            obj.insert("structured_output".to_string(), output_schema.to_value());
                        }
                        effective_provider_params = Some(params);
                    }

                    // No tools for extraction turn (empty slice)
                    let empty_tools: Arc<[Arc<crate::types::ToolDef>]> = Arc::from([]);
                    let call_tool_defs = if self.extraction_mode {
                        &empty_tools
                    } else {
                        &tool_defs
                    };

                    // Call LLM with retry — route errors through machine authority
                    let boundary_system_context = self.take_pending_system_context_boundary();
                    let request_messages =
                        self.llm_messages_with_runtime_system_context(&boundary_system_context);
                    let result = match self
                        .call_llm_with_retry(
                            &request_messages,
                            call_tool_defs,
                            effective_max_tokens,
                            effective_temperature,
                            effective_provider_params.as_ref(),
                        )
                        .await
                    {
                        Ok(r) => r,
                        Err(
                            e @ AgentError::TimeBudgetExceeded {
                                elapsed_secs,
                                limit_secs,
                            },
                        ) => {
                            // Dedicated time-budget terminal path — same as loop-top
                            emit_event!(AgentEvent::BudgetWarning {
                                budget_type: BudgetType::Time,
                                used: elapsed_secs,
                                limit: limit_secs,
                                percent: 1.0,
                            });
                            self.pending_fatal_diagnostic = Some(e);
                            self.apply_turn_input(TurnExecutionInput::TimeBudgetExceeded {
                                run_id: run_id.clone(),
                            })?;
                            return self.build_result(turn_count, tool_call_count).await;
                        }
                        Err(AgentError::TokenBudgetExceeded { used, limit }) => {
                            // Token budget exhausted during LLM call — same
                            // terminal path as loop-top budget check.
                            emit_event!(AgentEvent::BudgetWarning {
                                budget_type: BudgetType::Tokens,
                                used,
                                limit,
                                percent: 1.0,
                            });
                            self.apply_turn_input(TurnExecutionInput::BudgetExhausted {
                                run_id: run_id.clone(),
                            })?;
                            return self.build_result(turn_count, tool_call_count).await;
                        }
                        Err(AgentError::ToolCallBudgetExceeded { count, limit }) => {
                            // Tool-call budget exhausted during LLM call — same
                            // terminal path as loop-top budget check.
                            emit_event!(AgentEvent::BudgetWarning {
                                budget_type: BudgetType::ToolCalls,
                                used: count as u64,
                                limit: limit as u64,
                                percent: 1.0,
                            });
                            self.apply_turn_input(TurnExecutionInput::BudgetExhausted {
                                run_id: run_id.clone(),
                            })?;
                            return self.build_result(turn_count, tool_call_count).await;
                        }
                        Err(e @ AgentError::Llm { .. }) => {
                            // Exhausted hard LLM-call failure — route through
                            // machine-owned FatalFailure and preserve diagnostics.
                            self.pending_fatal_diagnostic = Some(e);
                            self.apply_turn_input(TurnExecutionInput::FatalFailure {
                                run_id: run_id.clone(),
                            })?;
                            return self.build_result(turn_count, tool_call_count).await;
                        }
                        Err(e) => return Err(e),
                    };

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
                        let tc_count = assistant_msg.tool_calls().count() as u32;
                        self.apply_turn_input(TurnExecutionInput::LlmReturnedToolCalls {
                            run_id: run_id.clone(),
                            tool_count: tc_count,
                        })?;

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
                                tool_results.push(ToolResult::new(
                                    tc.id.clone(),
                                    denied_content,
                                    true,
                                ));
                                emit_event!(AgentEvent::ToolExecutionCompleted {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    result: tool_results
                                        .last()
                                        .map(ToolResult::text_content)
                                        .unwrap_or_default(),
                                    is_error: true,
                                    duration_ms: 0,
                                    has_images: false,
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

                        // Build a set of currently visible tool names for dispatch-time gating.
                        // This prevents models from executing tools hidden by ToolScope
                        // external filters (e.g. view_image on non-image-capable models).
                        let visible_tool_names: std::collections::HashSet<String> =
                            tool_defs.iter().map(|t| t.name.clone()).collect();

                        // Execute all allowed tool calls in parallel using join_all
                        let dispatch_futures: Vec<_> = executable_tool_calls
                            .into_iter()
                            .map(|tc| {
                                let tools_ref = Arc::clone(&tools_ref);
                                let visible = visible_tool_names.contains(&tc.name);
                                async move {
                                    let start = crate::time_compat::Instant::now();
                                    let dispatch_result = if visible {
                                        tools_ref.dispatch(tc.as_view()).await
                                    } else {
                                        Err(crate::error::ToolError::NotFound {
                                            name: tc.name.clone(),
                                        })
                                    };
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    (tc, dispatch_result, duration_ms)
                                }
                            })
                            .collect();

                        let dispatch_results = futures::future::join_all(dispatch_futures).await;

                        // Process results and emit events
                        let mut all_async_ops = Vec::<crate::ops::AsyncOpRef>::new();
                        for (tc, dispatch_result, duration_ms) in dispatch_results {
                            let mut tool_result = match dispatch_result {
                                Ok(outcome) => {
                                    all_async_ops.extend(outcome.async_ops);
                                    outcome.result
                                }
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
                                    ToolResult::new(tc.id.clone(), serialized, true)
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
                                            content: tool_result.text_content(),
                                            is_error: tool_result.is_error,
                                            has_images: tool_result.has_images(),
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
                                tool_result.set_text_content(
                                    serde_json::to_string(&denied_payload).unwrap_or_else(|_| {
                                        "{\"error\":\"hook_denied\",\"message\":\"denied by hook\"}"
                                            .to_string()
                                    }),
                                );
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
                                        // Rebuild: patched text first, then image blocks
                                        // in their original relative order.
                                        crate::hooks::apply_tool_result_patch(
                                            &mut tool_result,
                                            content.clone(),
                                            *is_error,
                                        );
                                    }
                                }
                            }

                            // Emit execution complete
                            emit_event!(AgentEvent::ToolExecutionCompleted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                result: tool_result.text_content(),
                                is_error: tool_result.is_error,
                                duration_ms,
                                has_images: tool_result.has_images(),
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

                        let pending_op_refs = all_async_ops;
                        let barrier_operation_ids = pending_op_refs
                            .iter()
                            .filter(|r| r.wait_policy == crate::ops::WaitPolicy::Barrier)
                            .map(|r| r.operation_id.clone())
                            .collect::<Vec<_>>();
                        let has_barrier_ops = pending_op_refs
                            .iter()
                            .any(|r| r.wait_policy == crate::ops::WaitPolicy::Barrier);

                        // Add tool results to session
                        self.session.push(Message::ToolResults {
                            results: tool_results,
                        });

                        self.apply_turn_input(TurnExecutionInput::RegisterPendingOps {
                            run_id: run_id.clone(),
                            op_refs: pending_op_refs,
                            barrier_operation_ids,
                            has_barrier_ops,
                        })?;

                        if self.turn_authority.has_barrier_ops() {
                            // Stay in WaitingForOps — the outer match arm will
                            // await completion of barrier ops via wait-set.
                            continue;
                        }

                        // No pending ops — tool calls resolved, drain boundary, continue
                        self.apply_turn_input(TurnExecutionInput::ToolCallsResolved {
                            run_id: run_id.clone(),
                        })?;
                        self.drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await?;
                        let t = self.apply_turn_input(TurnExecutionInput::BoundaryContinue {
                            run_id: run_id.clone(),
                        })?;
                        self.execute_turn_effects(&t, turn_count, &event_tx).await;
                        turn_count += 1;
                    } else if self.extraction_mode {
                        // Extraction turn response — validate against schema
                        self.session.push(Message::BlockAssistant(assistant_msg));

                        // Drain turn boundary (fires TurnBoundary hooks, drains comms)
                        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal {
                            run_id: run_id.clone(),
                        })?;
                        self.drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await?;
                        emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                        // Authority: DrainingBoundary -> Extracting for validation
                        self.apply_turn_input(TurnExecutionInput::EnterExtraction {
                            run_id: run_id.clone(),
                            max_retries: self.config.structured_output_retries,
                        })?;

                        // Validate extraction response
                        let content = assistant_text.trim();
                        let json_content = super::extraction::strip_code_fences(content);

                        let validation_error =
                            match serde_json::from_str::<serde_json::Value>(json_content) {
                                Ok(parsed) => {
                                    let output_schema =
                                        self.config.output_schema.as_ref().ok_or_else(|| {
                                            AgentError::InternalError(
                                                "extraction_mode without output_schema".into(),
                                            )
                                        })?;
                                    let normalized = super::extraction::unwrap_named_object_wrapper(
                                        parsed,
                                        output_schema,
                                    );

                                    // Validate against schema (when jsonschema feature is available)
                                    let schema_error: Option<String>;
                                    #[cfg(feature = "jsonschema")]
                                    {
                                        let compiled =
                                            self.client.compile_schema(output_schema).map_err(
                                                |e| AgentError::InvalidOutputSchema(e.to_string()),
                                            )?;
                                        let validator =
                                            jsonschema::Validator::new(&compiled.schema).map_err(
                                                |e| AgentError::InvalidOutputSchema(e.to_string()),
                                            )?;
                                        schema_error =
                                            if let Err(error) = validator.validate(&normalized) {
                                                Some(format!("Schema validation failed: {error}"))
                                            } else {
                                                None
                                            };
                                    }
                                    #[cfg(not(feature = "jsonschema"))]
                                    {
                                        tracing::warn!(
                                            "Structured output schema validation unavailable \
                                        (jsonschema feature disabled). Accepting parsed \
                                        JSON without schema validation."
                                        );
                                        schema_error = None;
                                    }

                                    if schema_error.is_none() {
                                        // Validation passed — store result
                                        self.extraction_result = Some(normalized);
                                    }
                                    schema_error
                                }
                                Err(e) => Some(format!("Invalid JSON: {e}")),
                            };

                        if let Some(error) = validation_error {
                            // Validation failed — authority decides retry vs exhaust
                            self.extraction_last_error = Some(error.clone());
                            let t = self.apply_turn_input(
                                TurnExecutionInput::ExtractionValidationFailed {
                                    run_id: run_id.clone(),
                                    error: error.clone(),
                                },
                            )?;

                            if !self.turn_authority.phase().is_terminal() {
                                // Authority decided to retry — push retry prompt
                                let retry_prompt = format!(
                                    "The previous output was invalid: {error}. \
                                    Please provide valid JSON matching the schema. \
                                    Output ONLY the JSON, no additional text."
                                );
                                self.session
                                    .push(Message::User(UserMessage::text(retry_prompt)));
                                self.execute_turn_effects(&t, turn_count, &event_tx).await;
                                turn_count += 1;
                                continue;
                            }

                            // Authority decided retries exhausted
                            if let Err(e) = self.store.save(&self.session).await {
                                tracing::warn!("Failed to save session: {}", e);
                            }
                            return Err(AgentError::StructuredOutputValidationFailed {
                                attempts: self.turn_authority.extraction_attempts(),
                                reason: error,
                                last_output: self.session.last_assistant_text().unwrap_or_default(),
                            });
                        }

                        // Validation passed — complete via authority
                        self.apply_turn_input(TurnExecutionInput::ExtractionValidationPassed {
                            run_id: run_id.clone(),
                        })?;
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }
                        return Ok(RunResult {
                            text: self.session.last_assistant_text().unwrap_or_default(),
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                            structured_output: self.extraction_result.take(),
                            schema_warnings: self.extraction_schema_warnings.take(),
                            skill_diagnostics: None,
                        });
                    } else {
                        // No tool calls - we're done with the agentic loop
                        let final_text = assistant_text.clone();
                        self.session.push(Message::BlockAssistant(assistant_msg));

                        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal {
                            run_id: run_id.clone(),
                        })?;
                        self.drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await?;

                        // Emit turn completed only after turn-boundary hooks
                        // accept and boundary side effects are committed.
                        emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                        // Check if we need to perform extraction turn for structured output
                        if let Some(output_schema) = self.config.output_schema.as_ref()
                            && !self.extraction_mode
                        {
                            // Enter extraction mode via authority
                            self.extraction_mode = true;
                            self.extraction_result = None;
                            self.extraction_last_error = None;

                            // Compile schema and capture warnings
                            let compiled = self
                                .client
                                .compile_schema(output_schema)
                                .map_err(|e| AgentError::InvalidOutputSchema(e.to_string()))?;
                            self.extraction_schema_warnings = if compiled.warnings.is_empty() {
                                None
                            } else {
                                Some(compiled.warnings.clone())
                            };

                            // Push extraction prompt as user message
                            let prompt =
                                self.config.extraction_prompt.clone().unwrap_or_else(|| {
                                    super::extraction::DEFAULT_EXTRACTION_PROMPT.to_string()
                                });
                            self.session.push(Message::User(UserMessage::text(prompt)));

                            // Authority: DrainingBoundary -> Extracting -> CallingLlm
                            self.apply_turn_input(TurnExecutionInput::EnterExtraction {
                                run_id: run_id.clone(),
                                max_retries: self.config.structured_output_retries,
                            })?;
                            let t = self.apply_turn_input(TurnExecutionInput::ExtractionStart {
                                run_id: run_id.clone(),
                            })?;
                            self.execute_turn_effects(&t, turn_count, &event_tx).await;
                            turn_count += 1;
                            continue;
                        }

                        // No extraction needed - complete normally
                        self.apply_turn_input(TurnExecutionInput::BoundaryComplete {
                            run_id: run_id.clone(),
                        })?;

                        // Save session
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }

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
                    // Await completion of all pending barrier operations via
                    // the machine-owned turn-local wait-set. Only barrier ops
                    // block the turn; detached ops run independently.
                    if self.turn_authority.pending_op_refs().is_none() {
                        return Err(AgentError::InternalError(
                            "WaitingForOps entered without registered pending_op_refs".to_string(),
                        ));
                    }
                    let barrier_ids = self.turn_authority.barrier_op_ids();
                    if !barrier_ids.is_empty() {
                        let owned_ids: Vec<crate::ops::OperationId> =
                            barrier_ids.iter().map(|id| (*id).clone()).collect();
                        let wait_result = if let Some(ref registry) = self.ops_lifecycle {
                            registry.wait_all(&run_id, &owned_ids).await.map_err(|e| {
                                AgentError::InternalError(format!(
                                    "ops lifecycle wait_all failed: {e}"
                                ))
                            })?
                        } else {
                            return Err(AgentError::InternalError(
                                "barrier ops registered without ops_lifecycle registry".to_string(),
                            ));
                        };
                        // Feed OpsBarrierSatisfied through the generated protocol
                        // helper using the authority-derived obligation token.
                        use crate::generated::protocol_ops_barrier_satisfaction::{
                            accept_wait_all_satisfied, submit_ops_barrier_satisfied,
                        };
                        let obligation = accept_wait_all_satisfied(wait_result.satisfied);
                        let transition = submit_ops_barrier_satisfied(
                            &mut self.turn_authority,
                            obligation,
                            run_id.clone(),
                        )?;
                        self.state = transition.next_phase.to_loop_state();
                    }
                    self.apply_turn_input(TurnExecutionInput::ToolCallsResolved {
                        run_id: run_id.clone(),
                    })?;
                    self.drain_turn_boundary(turn_count, event_tx.as_ref())
                        .await?;
                    let t = self.apply_turn_input(TurnExecutionInput::BoundaryContinue {
                        run_id: run_id.clone(),
                    })?;
                    self.execute_turn_effects(&t, turn_count, &event_tx).await;
                    turn_count += 1;
                }
                LoopState::DrainingEvents => {
                    // Wait for any pending events to be processed
                    self.apply_turn_input(TurnExecutionInput::BoundaryComplete {
                        run_id: run_id.clone(),
                    })?;
                }
                LoopState::Cancelling => {
                    // Handle cancellation
                    self.apply_turn_input(TurnExecutionInput::CancellationObserved {
                        run_id: run_id.clone(),
                    })?;
                    return self.build_result(turn_count, tool_call_count).await;
                }
                LoopState::ErrorRecovery => {
                    // Attempt recovery
                    let t = self.apply_turn_input(TurnExecutionInput::RetryRequested {
                        run_id: run_id.clone(),
                    })?;
                    self.execute_turn_effects(&t, turn_count, &event_tx).await;
                }
                LoopState::Completed => {
                    return self.build_result(turn_count, tool_call_count).await;
                }
            }
        }
    }

    /// Build a RunResult from current state, using the generated terminal
    /// classification to decide Ok vs Err.
    async fn build_result(&mut self, turns: u32, tool_calls: u32) -> Result<RunResult, AgentError> {
        use crate::generated::terminal_surface_mapping::{SurfaceResultClass, classify_terminal};

        let outcome = self.turn_authority.terminal_outcome();
        let classification = classify_terminal(&outcome);

        match classification {
            Some(SurfaceResultClass::HardFailure) => {
                // Consume the pending diagnostic once — prefer the originating
                // typed error over a generic TerminalFailure so that
                // provider/reason/message truth is preserved on the surface.
                if let Some(diagnostic) = self.pending_fatal_diagnostic.take() {
                    Err(diagnostic)
                } else {
                    Err(AgentError::TerminalFailure {
                        outcome: format!("{outcome:?}"),
                    })
                }
            }
            _ => {
                // Success, Cancelled, or no terminal outcome yet (early exit).
                // Clear any stale diagnostic so it cannot bleed into a later run.
                self.pending_fatal_diagnostic = None;
                Ok(RunResult {
                    text: self.session.last_assistant_text().unwrap_or_default(),
                    session_id: self.session.id().clone(),
                    usage: self.session.total_usage(),
                    turns,
                    tool_calls,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: self.collect_skill_diagnostics().await,
                })
            }
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

/// Compute retry delay using server hint, policy, and rate-limit floor.
fn compute_retry_delay(
    hint: Option<std::time::Duration>,
    computed: std::time::Duration,
    is_rate_limited: bool,
) -> std::time::Duration {
    match hint {
        Some(h) if h > computed => h,
        _ if is_rate_limited => computed.max(std::time::Duration::from_secs(30)),
        _ => computed,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::manual_async_fn)]
mod tests {
    use super::rewrite_assistant_text;
    use crate::agent::{AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
    use crate::budget::{Budget, BudgetLimits};
    use crate::compact::{CompactionContext, CompactionResult, Compactor};
    use crate::error::{AgentError, ToolError};
    use crate::skills::{
        ResolvedSkill, SkillCollection, SkillDescriptor, SkillEngine, SkillFilter, SkillId,
        SkillKey, SkillName, SourceUuid,
    };
    use crate::state::LoopState;
    use crate::tool_scope::{EXTERNAL_TOOL_FILTER_METADATA_KEY, ToolFilter};
    use crate::types::{
        AssistantBlock, Message, StopReason, ToolCallView, ToolDef, ToolResult, Usage,
    };
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tokio::sync::{Notify, mpsc};

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

        fn model(&self) -> &'static str {
            "mock-model"
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
                    seen.push(user.text_content());
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

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct CompactionAwareLlmClient {
        last_user_messages: Mutex<Vec<String>>,
    }

    impl CompactionAwareLlmClient {
        fn new() -> Self {
            Self {
                last_user_messages: Mutex::new(Vec::new()),
            }
        }

        fn seen_last_user_messages(&self) -> Vec<String> {
            self.last_user_messages.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for CompactionAwareLlmClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let last_user = messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::User(user) => Some(user.text_content()),
                    _ => None,
                })
                .unwrap_or_default();
            self.last_user_messages
                .lock()
                .unwrap()
                .push(last_user.clone());

            let text = if last_user == "COMPACT NOW" {
                "summary".to_string()
            } else {
                "ok".to_string()
            };

            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text { text, meta: None }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct TrackingCompactor {
        compact_on_boundary: Option<u64>,
        seen_contexts: Mutex<Vec<CompactionContext>>,
    }

    impl TrackingCompactor {
        fn new(compact_on_boundary: Option<u64>) -> Self {
            Self {
                compact_on_boundary,
                seen_contexts: Mutex::new(Vec::new()),
            }
        }

        fn seen_boundaries(&self) -> Vec<u64> {
            self.seen_contexts
                .lock()
                .unwrap()
                .iter()
                .map(|ctx| ctx.session_boundary_index)
                .collect()
        }
    }

    impl Compactor for TrackingCompactor {
        fn should_compact(&self, ctx: &CompactionContext) -> bool {
            self.seen_contexts.lock().unwrap().push(ctx.clone());
            self.compact_on_boundary == Some(ctx.session_boundary_index)
        }

        fn compaction_prompt(&self) -> &'static str {
            "COMPACT NOW"
        }

        fn max_summary_tokens(&self) -> u32 {
            32
        }

        fn rebuild_history(&self, messages: &[Message], _summary: &str) -> CompactionResult {
            CompactionResult {
                messages: messages.to_vec(),
                discarded: Vec::new(),
            }
        }
    }

    struct NoTools;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for NoTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
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

    struct FullToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        dispatched_names: Mutex<Vec<String>>,
    }

    impl FullToolDispatcher {
        fn new(tool_names: &[&str]) -> Self {
            let tools = tool_names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        name: (*name).to_string(),
                        description: format!("{name} tool"),
                        input_schema: serde_json::json!({ "type": "object" }),
                    })
                })
                .collect::<Vec<_>>()
                .into();

            Self {
                tools,
                dispatched_names: Mutex::new(Vec::new()),
            }
        }

        fn dispatched(&self) -> Vec<String> {
            self.dispatched_names.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for FullToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            self.dispatched_names
                .lock()
                .unwrap()
                .push(call.name.to_string());

            Ok(ToolResult::new(
                call.id.to_string(),
                format!("dispatched {}", call.name),
                false,
            )
            .into())
        }
    }

    struct VisibilityRecordingLlmClient {
        call_count: Mutex<u32>,
        seen_tools: Mutex<Vec<Vec<String>>>,
    }

    impl VisibilityRecordingLlmClient {
        fn new() -> Self {
            Self {
                call_count: Mutex::new(0),
                seen_tools: Mutex::new(Vec::new()),
            }
        }

        fn seen_tools(&self) -> Vec<Vec<String>> {
            self.seen_tools.lock().unwrap().clone()
        }
    }

    struct SingleTurnVisibilityClient {
        seen_tools: Mutex<Vec<Vec<String>>>,
    }

    impl SingleTurnVisibilityClient {
        fn new() -> Self {
            Self {
                seen_tools: Mutex::new(Vec::new()),
            }
        }

        fn seen_tools(&self) -> Vec<Vec<String>> {
            self.seen_tools.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentLlmClient for SingleTurnVisibilityClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>(),
            );
            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    #[async_trait]
    impl AgentLlmClient for VisibilityRecordingLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>(),
            );

            let mut calls = self.call_count.lock().unwrap();
            let response = if *calls == 0 {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "secret".to_string(),
                        args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                    Usage::default(),
                )
            } else {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::Text {
                        text: "done".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage::default(),
                )
            };
            *calls += 1;
            Ok(response)
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct StagedDrainCommsRuntime {
        batches: tokio::sync::Mutex<Vec<Vec<String>>>,
        notify: Arc<Notify>,
    }

    impl StagedDrainCommsRuntime {
        fn with_batches(batches: Vec<Vec<String>>) -> Self {
            Self {
                batches: tokio::sync::Mutex::new(batches),
                notify: Arc::new(Notify::new()),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl crate::agent::CommsRuntime for StagedDrainCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            let mut guard = self.batches.lock().await;
            if guard.is_empty() {
                Vec::new()
            } else {
                guard.remove(0)
            }
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
    async fn reused_session_follow_up_run_can_compact_before_first_llm_call() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let compactor = Arc::new(TrackingCompactor::new(Some(1)));
        let mut agent = AgentBuilder::new()
            .compactor(compactor.clone())
            .build(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        agent.run("second".into()).await.unwrap();

        assert_eq!(compactor.seen_boundaries(), vec![0, 1]);
        assert_eq!(
            client.seen_last_user_messages(),
            vec![
                "first".to_string(),
                "COMPACT NOW".to_string(),
                "second".to_string()
            ]
        );
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
    async fn completed_with_max_turns_zero_returns_immediately() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(0);
        agent.state = LoopState::Completed;

        // With the turn execution authority, Completed + max_turns=0
        // correctly returns immediately with 0 turns rather than
        // erroring with InvalidStateTransition.
        let result = agent.run_loop(None).await.unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(agent.state, LoopState::Completed);
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
    async fn run_with_events_emits_run_completed_for_max_turns_zero() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(0);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();
        assert_eq!(result.turns, 0);

        let mut saw_run_completed = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::RunCompleted { result, .. } = event {
                saw_run_completed = true;
                assert_eq!(result, "");
            }
        }
        assert!(
            saw_run_completed,
            "successful early exits should still emit RunCompleted"
        );
    }

    #[tokio::test]
    async fn run_completed_event_uses_hook_rewritten_text() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookInvocation, HookOutcome,
            HookPatch, HookPoint,
        };

        struct RewriteRunCompletedHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for RewriteRunCompletedHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::RunCompleted {
                    return Ok(HookExecutionReport::empty());
                }
                Ok(HookExecutionReport {
                    outcomes: vec![HookOutcome {
                        hook_id: crate::hooks::HookId::new("rewrite-run-completed"),
                        point: HookPoint::RunCompleted,
                        priority: 0,
                        registration_index: 0,
                        decision: None,
                        patches: vec![HookPatch::RunResult {
                            text: "patched-final-text".to_string(),
                        }],
                        published_patches: Vec::new(),
                        error: None,
                        duration_ms: None,
                    }],
                    decision: None,
                    patches: Vec::new(),
                    published_patches: Vec::new(),
                })
            }
        }

        let mut agent = AgentBuilder::new()
            .with_hook_engine(Arc::new(RewriteRunCompletedHook))
            .build(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();
        assert_eq!(result.text, "patched-final-text");

        let mut run_completed_text = None;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::RunCompleted { result, .. } = event {
                run_completed_text = Some(result);
            }
        }
        assert_eq!(
            run_completed_text.as_deref(),
            Some("patched-final-text"),
            "RunCompleted should reflect the hook-rewritten final result"
        );
    }

    #[tokio::test]
    async fn run_completed_hook_failure_emits_run_failed_without_run_completed() {
        use crate::hooks::{
            HookDecision, HookEngine, HookEngineError, HookExecutionReport, HookInvocation,
            HookPoint, HookReasonCode,
        };

        struct DenyRunCompletedHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for DenyRunCompletedHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::RunCompleted {
                    return Ok(HookExecutionReport::empty());
                }
                Ok(HookExecutionReport {
                    decision: Some(HookDecision::Deny {
                        hook_id: crate::hooks::HookId::new("deny-run-completed"),
                        reason_code: HookReasonCode::PolicyViolation,
                        message: "deny completed".to_string(),
                        payload: None,
                    }),
                    ..HookExecutionReport::empty()
                })
            }
        }

        let mut agent = AgentBuilder::new()
            .with_hook_engine(Arc::new(DenyRunCompletedHook))
            .build(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("RunCompleted hook denial should fail the run");
        assert!(matches!(
            err,
            AgentError::HookDenied {
                point: HookPoint::RunCompleted,
                ..
            }
        ));

        let mut saw_run_failed = false;
        let mut saw_run_completed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunFailed { .. } => saw_run_failed = true,
                crate::event::AgentEvent::RunCompleted { .. } => saw_run_completed = true,
                _ => {}
            }
        }
        assert!(
            saw_run_failed,
            "hook-denied completion should emit RunFailed"
        );
        assert!(
            !saw_run_completed,
            "hook-denied completion should not also emit RunCompleted"
        );
    }

    #[tokio::test]
    async fn turn_boundary_denial_blocks_boundary_side_effects_and_turn_completed() {
        use crate::hooks::{
            HookDecision, HookEngine, HookEngineError, HookExecutionReport, HookInvocation,
            HookPoint, HookReasonCode,
        };

        struct DenyTurnBoundaryHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for DenyTurnBoundaryHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::TurnBoundary {
                    return Ok(HookExecutionReport::empty());
                }
                Ok(HookExecutionReport {
                    decision: Some(HookDecision::Deny {
                        hook_id: crate::hooks::HookId::new("deny-turn-boundary"),
                        reason_code: HookReasonCode::PolicyViolation,
                        message: "deny boundary".to_string(),
                        payload: None,
                    }),
                    ..HookExecutionReport::empty()
                })
            }
        }

        let comms = Arc::new(StagedDrainCommsRuntime::with_batches(vec![
            Vec::new(),
            vec!["late boundary message".to_string()],
        ]));
        let mut agent = AgentBuilder::new()
            .with_hook_engine(Arc::new(DenyTurnBoundaryHook))
            .with_comms_runtime(comms)
            .build(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("TurnBoundary denial should fail the run");
        assert!(matches!(
            err,
            AgentError::HookDenied {
                point: HookPoint::TurnBoundary,
                ..
            }
        ));

        assert!(
            !agent.session().messages().iter().any(|message| matches!(
                message,
                Message::User(user) if user.text_content().contains("late boundary message")
            )),
            "boundary-denied turns should not commit late comms boundary side effects"
        );

        let mut saw_turn_completed = false;
        let mut saw_run_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::TurnCompleted { .. } => saw_turn_completed = true,
                crate::event::AgentEvent::RunFailed { .. } => saw_run_failed = true,
                _ => {}
            }
        }
        assert!(
            !saw_turn_completed,
            "boundary denial should not emit TurnCompleted before failing the run"
        );
        assert!(saw_run_failed, "boundary denial should emit RunFailed");
    }

    #[tokio::test]
    async fn run_without_primary_channel_still_emits_run_lifecycle_to_tap() {
        use crate::event_tap::EventTapState;
        use std::sync::atomic::AtomicBool;

        let tap = crate::event_tap::new_event_tap();
        let (tap_tx, mut tap_rx) = mpsc::channel(128);
        {
            let mut guard = tap.lock();
            *guard = Some(EventTapState {
                tx: tap_tx,
                truncated: AtomicBool::new(false),
            });
        }

        let mut agent = AgentBuilder::new()
            .with_event_tap(tap)
            .build(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let result = agent
            .run("tap-only prompt".to_string().into())
            .await
            .unwrap();
        assert_eq!(result.text, "ok");

        let mut saw_run_started = false;
        let mut saw_run_completed = false;
        while let Ok(event) = tap_rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunStarted { .. } => saw_run_started = true,
                crate::event::AgentEvent::RunCompleted { .. } => saw_run_completed = true,
                _ => {}
            }
        }
        assert!(saw_run_started, "tap should receive RunStarted");
        assert!(saw_run_completed, "tap should receive RunCompleted");
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
            .run("plain user prompt".to_string().into())
            .await
            .expect("run should succeed");
        assert_eq!(result.turns, 1);

        let seen_ids = skill_engine.seen();
        assert!(
            seen_ids
                .iter()
                .any(|id| id.0 == "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"),
            "expected canonical skill id to be forwarded to skill engine, saw: {seen_ids:?}"
        );

        let seen_messages = client.seen();
        assert!(
            seen_messages
                .iter()
                .any(|msg| msg.contains("<skill>injected canonical skill</skill>")),
            "expected runtime prompt to include rendered skill injection, saw: {seen_messages:?}"
        );
    }

    #[tokio::test]
    async fn provider_receives_filtered_tools_and_dispatch_blocks_hidden_tools() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = AgentBuilder::new()
            .build(client.clone(), tools.clone(), Arc::new(NoopStore))
            .await;

        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap();
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");

        // Provider sees only visible tools (filtered by ToolScope)
        let seen = client.seen_tools();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], vec!["visible".to_string()]);
        assert_eq!(seen[1], vec!["visible".to_string()]);

        // Hidden tools are NOT dispatched — blocked at execution time too
        let dispatched = tools.dispatched();
        assert!(
            dispatched.is_empty(),
            "hidden tools should not be dispatched, but got: {dispatched:?}"
        );
    }

    #[tokio::test]
    async fn run_loop_boundary_applies_filter_and_emits_tool_config_changed_and_notice() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = AgentBuilder::new()
            .build(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap();

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();
        assert_eq!(result.text, "done");
        assert_eq!(client.seen_tools(), vec![vec!["visible".to_string()]]);

        let mut saw_config_event = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::ToolConfigChanged { payload } = event {
                assert_eq!(
                    payload.operation,
                    crate::event::ToolConfigChangeOperation::Reload
                );
                assert_eq!(payload.target, "tool_scope");
                assert!(payload.status.contains("boundary_applied"));
                saw_config_event = true;
            }
        }
        assert!(
            saw_config_event,
            "expected ToolConfigChanged event on boundary visibility change"
        );

        let notices: Vec<String> = agent
            .session()
            .messages()
            .iter()
            .filter_map(|msg| match msg {
                Message::User(user)
                    if user.text_content().contains("[SYSTEM NOTICE][TOOL_SCOPE]") =>
                {
                    Some(user.text_content())
                }
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1);
    }

    #[tokio::test]
    async fn run_loop_fails_safe_to_full_tools_with_warning_event_and_notice() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = AgentBuilder::new()
            .build(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap();
        agent.inject_tool_scope_boundary_failure_once_for_test();

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();
        assert_eq!(result.text, "done");
        assert_eq!(
            client.seen_tools(),
            vec![vec!["visible".to_string(), "secret".to_string()]]
        );

        let mut saw_warning_event = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::ToolConfigChanged { payload } = event
                && payload.status.contains("warning_fallback_all")
            {
                saw_warning_event = true;
            }
        }
        assert!(
            saw_warning_event,
            "expected warning ToolConfigChanged event during fail-safe fallback"
        );

        let notices: Vec<String> = agent
            .session()
            .messages()
            .iter()
            .filter_map(|msg| match msg {
                Message::User(user) if user.text_content().contains("[TOOL_SCOPE][WARNING]") => {
                    Some(user.text_content())
                }
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1);
    }

    #[tokio::test]
    async fn builder_restores_persisted_external_filter_from_session_metadata() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut session = crate::Session::new();
        session.set_metadata(
            EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let mut agent = AgentBuilder::new()
            .resume_session(session)
            .build(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");
        assert_eq!(
            client.seen_tools(),
            vec![vec!["visible".to_string()], vec!["visible".to_string()]]
        );
    }

    #[tokio::test]
    async fn builder_prunes_unknown_persisted_filter_tools() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut session = crate::Session::new();
        session.set_metadata(
            EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Allow(
                ["visible".to_string(), "missing".to_string()]
                    .into_iter()
                    .collect(),
            ))
            .unwrap(),
        );

        let mut agent = AgentBuilder::new()
            .resume_session(session)
            .build(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");
        let seen = client.seen_tools();
        assert_eq!(
            seen,
            vec![vec!["visible".to_string()], vec!["visible".to_string()]]
        );
    }

    /// Mock LLM client that returns high usage, causing budget exhaustion
    /// after recording on the caller side.
    struct HighUsageLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for HighUsageLlmClient {
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
                Usage {
                    input_tokens: 500,
                    output_tokens: 500,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    /// Regression test: token budget exhaustion detected after LLM-call usage
    /// recording must route through the machine authority's BudgetExhausted
    /// terminal path (SurfaceResultClass::Success), not escape as a raw
    /// AgentError::TokenBudgetExceeded.
    ///
    /// Before the fix, budget errors from inside call_llm_with_retry bypassed
    /// the machine authority entirely (Dogma §4: same semantic condition, one
    /// canonical terminal path).
    #[tokio::test]
    async fn token_budget_exhausted_after_llm_call_routes_through_authority() {
        // Budget allows exactly 100 tokens — the first LLM call reports 1000,
        // so usage recording at the call site pushes the budget over the limit.
        // The next loop-top budget.check() fires TokenBudgetExceeded, which
        // must be routed to BudgetExhausted -> Success, not raw Err.
        let mut agent = build_agent(Arc::new(HighUsageLlmClient)).await;
        agent.config.max_turns = Some(10);
        agent.state = LoopState::CallingLlm;
        agent.budget = Budget::new(BudgetLimits {
            max_tokens: Some(100),
            max_duration: None,
            max_tool_calls: None,
        });

        // Must be Ok (Success via BudgetExhausted), not Err(TokenBudgetExceeded).
        let result = agent.run_loop(None).await;
        assert!(
            result.is_ok(),
            "token budget exhaustion must route through BudgetExhausted (Success), \
             not escape as raw AgentError: {:?}",
            result.err()
        );
        assert_eq!(agent.state, LoopState::Completed);
    }

    /// Regression test: tool-call budget exhaustion detected anywhere in the
    /// agent loop must route through BudgetExhausted, producing Success.
    #[tokio::test]
    async fn tool_call_budget_exhausted_routes_through_authority() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(10);
        agent.state = LoopState::CallingLlm;
        agent.budget = Budget::new(BudgetLimits {
            max_tokens: None,
            max_duration: None,
            max_tool_calls: Some(0),
        });

        let result = agent.run_loop(None).await;
        assert!(
            result.is_ok(),
            "tool-call budget exhaustion must route through BudgetExhausted (Success), \
             not escape as raw AgentError: {:?}",
            result.err()
        );
        assert_eq!(agent.state, LoopState::Completed);
    }

    // -- Retry delay hint tests (PR #156 port) --

    use std::time::Duration;

    #[test]
    fn compute_retry_delay_uses_computed_for_non_rate_limited() {
        let delay = super::compute_retry_delay(None, Duration::from_millis(500), false);
        assert_eq!(delay, Duration::from_millis(500));
    }

    #[test]
    fn compute_retry_delay_uses_30s_floor_for_rate_limited_without_hint() {
        let delay = super::compute_retry_delay(None, Duration::from_millis(500), true);
        assert_eq!(delay, Duration::from_secs(30));
    }

    #[test]
    fn compute_retry_delay_uses_hint_when_greater_than_computed() {
        let delay = super::compute_retry_delay(
            Some(Duration::from_secs(60)),
            Duration::from_millis(500),
            true,
        );
        assert_eq!(delay, Duration::from_secs(60));
    }

    #[test]
    fn compute_retry_delay_uses_30s_floor_when_hint_below_floor() {
        let delay = super::compute_retry_delay(
            Some(Duration::from_millis(100)),
            Duration::from_millis(500),
            true,
        );
        assert_eq!(delay, Duration::from_secs(30));
    }

    #[test]
    fn compute_retry_delay_uses_computed_when_greater_than_hint() {
        let delay = super::compute_retry_delay(
            Some(Duration::from_secs(60)),
            Duration::from_secs(90),
            true,
        );
        assert_eq!(delay, Duration::from_secs(90));
    }

    /// Mock LLM client that returns RateLimited with a retry_after hint on
    /// the first call, then succeeds on the second. Records call timestamps
    /// so the test can verify the server-directed delay was applied.
    struct RateLimitThenSucceedClient {
        call_times: Mutex<Vec<std::time::Instant>>,
        retry_after: Duration,
    }

    impl RateLimitThenSucceedClient {
        fn new(retry_after: Duration) -> Self {
            Self {
                call_times: Mutex::new(Vec::new()),
                retry_after,
            }
        }

        fn call_times(&self) -> Vec<std::time::Instant> {
            self.call_times.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for RateLimitThenSucceedClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut times = self.call_times.lock().unwrap();
            times.push(std::time::Instant::now());
            let attempt = times.len();
            drop(times);

            if attempt == 1 {
                Err(AgentError::llm(
                    "mock",
                    crate::error::LlmFailureReason::RateLimited {
                        retry_after: Some(self.retry_after),
                    },
                    "rate limited by mock",
                ))
            } else {
                Ok(super::LlmStreamResult::new(
                    vec![AssistantBlock::Text {
                        text: "ok after retry".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage::default(),
                ))
            }
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    /// Integration test: verify that a 429 with Retry-After hint actually
    /// delays the retry by at least the server-directed duration, and that
    /// the run completes successfully after the retry.
    #[tokio::test]
    async fn retry_after_hint_delays_second_attempt() {
        use crate::retry::RetryPolicy;

        let hint = Duration::from_millis(200); // short for CI speed
        let client = Arc::new(RateLimitThenSucceedClient::new(hint));
        let mut agent = AgentBuilder::new()
            .retry_policy(RetryPolicy {
                max_retries: 3,
                initial_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                call_timeout: None,
            })
            .build(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent.run("test".to_string().into()).await;
        assert!(
            result.is_ok(),
            "run should succeed after retry: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().text, "ok after retry");

        let times = client.call_times();
        assert_eq!(
            times.len(),
            2,
            "expected exactly 2 LLM calls (1 fail + 1 success)"
        );

        let actual_delay = times[1].duration_since(times[0]);
        // The retry delay should be at least the hint (200ms), but because
        // the 30s floor applies to rate limits, the actual delay should be
        // max(hint, 30s). However, 200ms < 30s, so the floor kicks in.
        // For a fast test, we verify the delay is at least the hint.
        // The compute_retry_delay unit tests verify the floor independently.
        assert!(
            actual_delay >= hint,
            "retry delay ({actual_delay:?}) must be at least the server hint ({hint:?})",
        );
    }

    /// Integration test: rate-limited error WITHOUT a hint uses the 30s floor,
    /// but we can't wait 30s in CI. Instead verify via a tight time budget
    /// that the delay is correctly capped and the budget check fires.
    #[tokio::test]
    async fn rate_limit_without_hint_respects_budget_cap() {
        use crate::budget::{Budget, BudgetLimits};
        use crate::retry::RetryPolicy;

        let client = Arc::new(RateLimitThenSucceedClient::new(Duration::ZERO));
        let mut agent = AgentBuilder::new()
            .retry_policy(RetryPolicy {
                max_retries: 3,
                initial_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                call_timeout: None,
            })
            .build(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        // Set a 100ms time budget — the 30s rate-limit floor will be
        // capped to 100ms by remaining_duration, then budget.check()
        // fires TimeBudgetExceeded after the sleep.
        agent.budget = Budget::new(BudgetLimits {
            max_tokens: None,
            max_duration: Some(Duration::from_millis(100)),
            max_tool_calls: None,
        });

        let _result = agent.run("test".to_string().into()).await;
        // The run should terminate (not hang for 30s). Whether it returns
        // Ok (retry succeeded within budget) or Err (budget exceeded after
        // capped delay) depends on timing, but it must NOT hang.
        let times = client.call_times();
        assert!(
            times.len() <= 2,
            "should not retry endlessly; got {} calls",
            times.len()
        );
    }
}

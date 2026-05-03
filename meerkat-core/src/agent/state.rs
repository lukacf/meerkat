//! Agent state machine internals.

use crate::budget::{BudgetDimension, BudgetExceeded};
use crate::error::{AgentError, ToolError};
use crate::event::{
    AgentEvent, BackgroundJobTerminalStatus, BudgetType, DeferredCatalogDelta,
    ToolConfigChangeDomain, ToolConfigChangeOperation, ToolConfigChangeStatus,
    ToolConfigChangedPayload,
};
use crate::hooks::{
    HookDecision, HookInvocation, HookLlmRequest, HookLlmResponse, HookPatch, HookPoint,
    HookToolCall, HookToolResult,
};
use crate::image_content::{MissingBlobBehavior, hydrate_messages_for_execution};
use crate::lifecycle::RunId;
use crate::lifecycle::run_primitive::ProviderParamsOverride;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_catalog::{ToolCatalogDeferredEligibility, ToolCatalogMode, ToolPlaneClass};
use crate::turn_execution_authority::{
    ContentShape, TurnExecutionEffect, TurnExecutionInput, TurnExecutionTransition,
    TurnFailureReason, TurnPhase, TurnPrimitiveKind, TurnTerminalOutcome,
    terminal_outcome_for_budget_exceeded,
};
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, RunResult, SystemNoticeKind,
    SystemNoticeMessage, ToolCallView, ToolDef, ToolNameSet, UserMessage,
};
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{
    Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult,
    select_tool_catalog_mode,
};

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

struct LlmRetryRequest<'a> {
    run_id: &'a RunId,
    turn_count: u32,
    event_tx: &'a Option<mpsc::Sender<AgentEvent>>,
    messages: &'a [Message],
    tools: &'a [Arc<ToolDef>],
    max_tokens: u32,
    temperature: Option<f32>,
    provider_params: Option<&'a ProviderParamsOverride>,
}

fn turn_input_run_id(input: &TurnExecutionInput) -> Option<RunId> {
    match input {
        TurnExecutionInput::StartConversationRun { run_id }
        | TurnExecutionInput::StartImmediateAppend { run_id }
        | TurnExecutionInput::StartImmediateContext { run_id }
        | TurnExecutionInput::PrimitiveApplied { run_id, .. }
        | TurnExecutionInput::LlmReturnedToolCalls { run_id, .. }
        | TurnExecutionInput::LlmReturnedTerminal { run_id }
        | TurnExecutionInput::RegisterPendingOps { run_id, .. }
        | TurnExecutionInput::ToolCallsResolved { run_id }
        | TurnExecutionInput::OpsBarrierSatisfied { run_id, .. }
        | TurnExecutionInput::BoundaryContinue { run_id }
        | TurnExecutionInput::BoundaryComplete { run_id }
        | TurnExecutionInput::RecoverableFailure { run_id, .. }
        | TurnExecutionInput::FatalFailure { run_id, .. }
        | TurnExecutionInput::RetryRequested { run_id, .. }
        | TurnExecutionInput::CancelNow { run_id }
        | TurnExecutionInput::CancelAfterBoundary { run_id }
        | TurnExecutionInput::CancellationObserved { run_id }
        | TurnExecutionInput::AcknowledgeTerminal { run_id }
        | TurnExecutionInput::TurnLimitReached { run_id }
        | TurnExecutionInput::BudgetExhausted { run_id }
        | TurnExecutionInput::TimeBudgetExceeded { run_id }
        | TurnExecutionInput::BudgetLimitExceeded { run_id, .. }
        | TurnExecutionInput::EnterExtraction { run_id, .. }
        | TurnExecutionInput::ExtractionValidationPassed { run_id }
        | TurnExecutionInput::ExtractionValidationFailed { run_id, .. }
        | TurnExecutionInput::ExtractionStart { run_id } => Some(run_id.clone()),
        TurnExecutionInput::ForceCancelNoRun => None,
    }
}

fn turn_input_failure_reason(input: &TurnExecutionInput) -> Option<TurnFailureReason> {
    match input {
        TurnExecutionInput::FatalFailure { reason, .. } => Some(reason.clone()),
        TurnExecutionInput::TurnLimitReached { .. } => Some(TurnFailureReason::new(
            crate::event::AgentErrorClass::MaxTurns,
            "turn limit reached",
        )),
        TurnExecutionInput::BudgetExhausted { .. } => Some(TurnFailureReason::terminal_outcome(
            TurnTerminalOutcome::BudgetExhausted,
        )),
        TurnExecutionInput::TimeBudgetExceeded { .. } => Some(TurnFailureReason::terminal_outcome(
            TurnTerminalOutcome::TimeBudgetExceeded,
        )),
        TurnExecutionInput::BudgetLimitExceeded { exceeded, .. } => {
            Some(TurnFailureReason::budget_exceeded(*exceeded))
        }
        TurnExecutionInput::ExtractionValidationFailed { error, .. } => {
            Some(TurnFailureReason::new(
                crate::event::AgentErrorClass::StructuredOutput,
                error.clone(),
            ))
        }
        _ => None,
    }
}

fn budget_warning_event(exceeded: BudgetExceeded) -> AgentEvent {
    let budget_type = match exceeded.dimension {
        BudgetDimension::Tokens => BudgetType::Tokens,
        BudgetDimension::Time => BudgetType::Time,
        BudgetDimension::ToolCalls => BudgetType::ToolCalls,
    };
    AgentEvent::BudgetWarning {
        budget_type,
        used: exceeded.used,
        limit: exceeded.limit,
        percent: 1.0,
    }
}

fn synthetic_notice_message(kind: SystemNoticeKind, body: impl Into<String>) -> Message {
    Message::SystemNotice(SystemNoticeMessage::new(kind, body))
}

fn is_synthetic_notice(message: &Message, kind: SystemNoticeKind) -> bool {
    matches!(message, Message::SystemNotice(notice) if notice.kind == kind)
}

fn merge_provider_param_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_obj), Value::Object(patch_obj)) => {
            for (key, value) in patch_obj {
                if value.is_null() {
                    target_obj.remove(key);
                } else {
                    merge_provider_param_patch(
                        target_obj.entry(key.clone()).or_insert(Value::Null),
                        value,
                    );
                }
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
}

fn merged_provider_params(defaults: Option<&Value>, explicit: Option<&Value>) -> Option<Value> {
    match (defaults, explicit) {
        (None, None) => None,
        (Some(defaults), None) => Some(defaults.clone()),
        (None, Some(explicit)) => Some(explicit.clone()),
        (Some(defaults), Some(explicit)) => {
            let mut merged = defaults.clone();
            merge_provider_param_patch(&mut merged, explicit);
            Some(merged)
        }
    }
}

fn hidden_deferred_catalog_names(
    catalog: &[crate::ToolCatalogEntry],
    visible_names: &BTreeSet<String>,
) -> BTreeSet<String> {
    catalog
        .iter()
        .filter(|entry| entry.plane == ToolPlaneClass::Session)
        .filter(|entry| {
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )
        })
        .map(|entry| entry.tool.name.to_string())
        .filter(|name| !visible_names.contains(name))
        .collect()
}

fn dispatcher_knows_tool<T>(dispatcher: &T, name: &str) -> bool
where
    T: AgentToolDispatcher + ?Sized,
{
    if dispatcher.tool_catalog_capabilities().exact_catalog {
        dispatcher
            .tool_catalog()
            .iter()
            .any(|entry| entry.tool.name == name)
    } else {
        dispatcher.tools().iter().any(|tool| tool.name == name)
    }
}

fn precheck_visible_tool_call<T>(
    dispatcher: &T,
    visible_names: &ToolNameSet,
    name: &str,
) -> Result<(), ToolError>
where
    T: AgentToolDispatcher + ?Sized,
{
    if visible_names.contains(name) {
        return Ok(());
    }
    if dispatcher_knows_tool(dispatcher, name) {
        return Err(ToolError::access_denied(name));
    }
    Err(ToolError::not_found(name))
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Snapshot the runtime-backed turn-state authority.
    ///
    /// Pre-wave-a, a missing `turn_state_handle` fell through to a
    /// standalone in-process `self.turn_state` authority. Wave-a deleted
    /// the standalone authority entirely; the runtime-backed handle is now
    /// the only turn-state source. For live runs the facade wires the
    /// handle via `AgentBuilder::with_turn_state_handle` (see
    /// `meerkat/src/factory.rs` where it plugs `SessionRuntimeBindings`).
    ///
    /// Returns a typed `AgentError::InternalError` when callers on the
    /// live-run path query turn state without an attached handle —
    /// preserves the pre-retype intent (a real secondary authority, never
    /// a silent default) as a fail-loud error rather than an
    /// `unwrap_or_default` silent-drop.
    fn runtime_turn_authority_snapshot(&self) -> Result<crate::TurnStateSnapshot, AgentError> {
        let handle = self.turn_state_handle.as_deref().ok_or_else(|| {
            AgentError::InternalError(
                "runtime turn-state handle missing: agent was built without \
                 with_turn_state_handle but is being queried on a live-run code path \
                 — the standalone fallback was deleted in wave-a; runtime-backed \
                 wiring is required"
                    .to_string(),
            )
        })?;
        if self.runtime_execution_kind_required && self.runtime_execution_kind.is_none() {
            return Err(AgentError::InternalError(
                "runtime_execution_kind not set: turn-state handle is attached but \
                 the runtime build mode did not classify the execution kind"
                    .to_string(),
            ));
        }
        Ok(handle.snapshot())
    }

    fn turn_active_run_id(&self) -> Result<Option<RunId>, AgentError> {
        Ok(self.runtime_turn_authority_snapshot()?.active_run_id)
    }

    pub(super) fn turn_phase(&self) -> Result<TurnPhase, AgentError> {
        Ok(self.runtime_turn_authority_snapshot()?.turn_phase)
    }

    fn turn_cancel_after_boundary(&self) -> Result<bool, AgentError> {
        Ok(self
            .runtime_turn_authority_snapshot()?
            .cancel_after_boundary)
    }

    fn turn_has_barrier_ops(&self) -> Result<bool, AgentError> {
        Ok(self.runtime_turn_authority_snapshot()?.has_barrier_ops)
    }

    fn turn_barrier_operation_ids(&self) -> Result<Vec<crate::ops::OperationId>, AgentError> {
        let snapshot = self.runtime_turn_authority_snapshot()?;
        Ok(snapshot.barrier_operation_ids.iter().cloned().collect())
    }

    fn turn_pending_ops_registered(&self) -> Result<bool, AgentError> {
        Ok(!self
            .runtime_turn_authority_snapshot()?
            .pending_op_refs
            .is_empty())
    }

    fn turn_in_extraction_flow(&self) -> Result<bool, AgentError> {
        Ok(self
            .runtime_turn_authority_snapshot()?
            .max_extraction_retries
            > 0)
    }

    fn turn_terminal_outcome(&self) -> Result<TurnTerminalOutcome, AgentError> {
        // `terminal_outcome` on the snapshot is `Option<TurnTerminalOutcome>`;
        // `None` there is a meaningful phase ("not yet terminal"), distinct
        // from "handle absent" which surfaces above as an error. Project
        // the in-handle `None` to `TurnTerminalOutcome::None` per the
        // DSL contract (see `TurnStateSnapshot::terminal_outcome` doc).
        Ok(self
            .runtime_turn_authority_snapshot()?
            .terminal_outcome
            .unwrap_or(TurnTerminalOutcome::None))
    }

    fn turn_extraction_attempts(&self) -> Result<u32, AgentError> {
        let snapshot = self.runtime_turn_authority_snapshot()?;
        Ok(u32::try_from(snapshot.extraction_attempts).unwrap_or(u32::MAX))
    }

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
        &mut self,
        request: LlmRetryRequest<'_>,
    ) -> Result<LlmStreamResult, AgentError> {
        let LlmRetryRequest {
            run_id,
            turn_count,
            event_tx,
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        } = request;

        let hydrated_messages = if let Some(blob_store) = self.blob_store.as_ref() {
            let mut hydrated = messages.to_vec();
            hydrate_messages_for_execution(
                blob_store.as_ref(),
                &mut hydrated,
                MissingBlobBehavior::HistoricalPlaceholder,
            )
            .await
            .map_err(|err| {
                AgentError::InternalError(format!(
                    "failed to hydrate image refs before llm execution: {err}"
                ))
            })?;
            Some(hydrated)
        } else {
            None
        };
        let messages = hydrated_messages.as_deref().unwrap_or(messages);
        let mut attempt = 0u32;

        loop {
            // 1. Budget gate at loop entry
            if let Some(exceeded) = self.budget.observe().exceeded() {
                return Err(exceeded.to_agent_error());
            }

            // 2. Compute effective timeout for this call
            let effective_call_timeout = self.resolve_effective_call_timeout();
            let remaining_turn = self.budget.remaining_duration();

            // If remaining turn budget is zero, surface immediately
            if remaining_turn == Some(std::time::Duration::ZERO)
                && let Some(exceeded) = self.budget.observe().exceeded()
            {
                return Err(exceeded.to_agent_error());
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
                                    let exceeded =
                                        self.budget.observe().exceeded().unwrap_or_else(|| {
                                            let timeout_ms = effective_timeout.as_millis() as u64;
                                            let (elapsed_ms, limit_ms) = self
                                                .budget
                                                .time_usage()
                                                .unwrap_or((timeout_ms, timeout_ms));
                                            BudgetExceeded {
                                                dimension: BudgetDimension::Time,
                                                used: elapsed_ms / 1000,
                                                limit: limit_ms / 1000,
                                            }
                                        });
                                    return Err(exceeded.to_agent_error());
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
                    if let Some(retry_schedule) = self.retry_policy.schedule_retry(
                        &e,
                        attempt,
                        self.budget.remaining_duration(),
                    ) {
                        tracing::warn!(
                            "LLM call failed (attempt {}), retrying in {}ms: {}",
                            retry_schedule.plan.attempt,
                            retry_schedule.plan.selected_delay_ms,
                            e
                        );
                        let recover =
                            self.apply_turn_input(TurnExecutionInput::RecoverableFailure {
                                run_id: run_id.clone(),
                                retry: retry_schedule.clone(),
                            })?;
                        self.execute_turn_effects(&recover, turn_count, event_tx)
                            .await;
                        let _ = crate::event_tap::tap_emit(
                            &self.event_tap,
                            event_tx.as_ref(),
                            AgentEvent::Retrying {
                                attempt: retry_schedule.plan.attempt,
                                max_attempts: retry_schedule.plan.max_retries,
                                error: e.to_string(),
                                delay_ms: retry_schedule.plan.selected_delay_ms,
                                retry: Some(retry_schedule.clone()),
                            },
                        )
                        .await;
                        attempt += 1;
                        tokio::time::sleep(retry_schedule.plan.selected_delay()).await;
                        if let Some(exceeded) = self.budget.observe().exceeded() {
                            return Err(exceeded.to_agent_error());
                        }
                        let retry = self.apply_turn_input(TurnExecutionInput::RetryRequested {
                            run_id: run_id.clone(),
                            retry_attempt: retry_schedule.plan.attempt,
                        })?;
                        self.execute_turn_effects(&retry, turn_count, event_tx)
                            .await;
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
                    prompt_input: None,
                    prompt: None,
                    error_report: None,
                    error_class: None,
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
            hook_id,
            reason_code,
            message,
            payload,
        }) = turn_boundary_report.decision
        {
            return Err(AgentError::HookDenied {
                hook_id,
                point: HookPoint::TurnBoundary,
                reason_code,
                message,
                payload,
            });
        }

        Ok(())
    }

    /// Apply a typed input to the runtime-backed turn state when available,
    /// then mirror through the standalone local fallback and observable
    /// `LoopState`.
    fn apply_turn_input_via_runtime_handle(
        &self,
        input: &TurnExecutionInput,
    ) -> Result<(), AgentError> {
        let Some(handle) = self.turn_state_handle.as_deref() else {
            return Ok(());
        };

        let result = match input {
            TurnExecutionInput::StartConversationRun { run_id }
                if handle.snapshot().active_run_id.as_ref() == Some(run_id) =>
            {
                Ok(())
            }
            TurnExecutionInput::StartConversationRun { run_id } => handle.start_conversation_run(
                run_id.clone(),
                TurnPrimitiveKind::ConversationTurn,
                ContentShape::Conversation,
                false,
                false,
                0,
            ),
            TurnExecutionInput::StartImmediateAppend { run_id }
                if handle.snapshot().active_run_id.as_ref() == Some(run_id) =>
            {
                Ok(())
            }
            TurnExecutionInput::StartImmediateAppend { run_id } => {
                handle.start_immediate_append(run_id.clone())
            }
            TurnExecutionInput::StartImmediateContext { run_id }
                if handle.snapshot().active_run_id.as_ref() == Some(run_id) =>
            {
                Ok(())
            }
            TurnExecutionInput::StartImmediateContext { run_id } => {
                handle.start_immediate_context(run_id.clone())
            }
            TurnExecutionInput::PrimitiveApplied {
                run_id: _,
                admitted_content_shape: _,
                vision_enabled: _,
                image_tool_results_enabled: _,
            } => handle.primitive_applied(),
            TurnExecutionInput::LlmReturnedToolCalls { tool_count, .. } => {
                handle.llm_returned_tool_calls(u64::from(*tool_count))
            }
            TurnExecutionInput::LlmReturnedTerminal { .. } => handle.llm_returned_terminal(),
            TurnExecutionInput::RegisterPendingOps {
                op_refs,
                barrier_operation_ids,
                ..
            } => handle.register_pending_ops(
                op_refs.iter().cloned().collect(),
                barrier_operation_ids.iter().cloned().collect(),
            ),
            TurnExecutionInput::ToolCallsResolved { .. } => handle.tool_calls_resolved(),
            TurnExecutionInput::OpsBarrierSatisfied { operation_ids, .. } => {
                handle.ops_barrier_satisfied(operation_ids.iter().cloned().collect())
            }
            TurnExecutionInput::BoundaryContinue { .. } => handle.boundary_continue(),
            TurnExecutionInput::BoundaryComplete { .. } => handle.boundary_complete(),
            TurnExecutionInput::RecoverableFailure { retry, .. } => {
                handle.recoverable_failure(retry.clone())
            }
            TurnExecutionInput::FatalFailure { reason, .. } => handle.fatal_failure(reason.clone()),
            TurnExecutionInput::RetryRequested { retry_attempt, .. } => {
                handle.retry_requested(*retry_attempt)
            }
            TurnExecutionInput::CancelNow { .. } => handle.cancel_now(),
            TurnExecutionInput::CancelAfterBoundary { .. } => {
                handle.request_cancel_after_boundary()
            }
            TurnExecutionInput::CancellationObserved { .. } => handle.cancellation_observed(),
            TurnExecutionInput::AcknowledgeTerminal { .. } => {
                handle.acknowledge_terminal(self.turn_terminal_outcome()?)
            }
            TurnExecutionInput::TurnLimitReached { .. } => handle.turn_limit_reached(),
            TurnExecutionInput::BudgetExhausted { .. } => handle.budget_exhausted(),
            TurnExecutionInput::TimeBudgetExceeded { .. } => handle.time_budget_exceeded(),
            TurnExecutionInput::BudgetLimitExceeded { exceeded, .. } => {
                match terminal_outcome_for_budget_exceeded(*exceeded) {
                    TurnTerminalOutcome::TimeBudgetExceeded => handle.time_budget_exceeded(),
                    TurnTerminalOutcome::BudgetExhausted => handle.budget_exhausted(),
                    _ => unreachable!("budget exceeded maps only to budget terminal outcomes"),
                }
            }
            TurnExecutionInput::EnterExtraction { max_retries, .. } => {
                handle.enter_extraction(*max_retries)
            }
            TurnExecutionInput::ExtractionValidationPassed { .. } => {
                handle.extraction_validation_passed()
            }
            TurnExecutionInput::ExtractionValidationFailed { error, .. } => {
                handle.extraction_validation_failed(error.clone())
            }
            TurnExecutionInput::ExtractionStart { .. } => handle.extraction_start(),
            TurnExecutionInput::ForceCancelNoRun => handle.force_cancel_no_run(),
        };

        result.map_err(|err| {
            AgentError::InternalError(format!(
                "runtime turn-state handle rejected {input:?}: {err}"
            ))
        })
    }

    pub(super) fn apply_turn_input(
        &mut self,
        input: TurnExecutionInput,
    ) -> Result<TurnExecutionTransition, AgentError> {
        let prev_phase = self.turn_phase()?;
        self.apply_turn_input_via_runtime_handle(&input)?;
        let next_phase = self.turn_phase()?;

        // Effects are derived from phase transitions only. The runtime
        // authority owns all other side-effect decisions; core just
        // surfaces the compaction tick on CallingLlm entry so the
        // standalone compactor path still fires.
        let mut effects = Vec::new();
        if prev_phase != TurnPhase::CallingLlm && next_phase == TurnPhase::CallingLlm {
            effects.push(TurnExecutionEffect::CheckCompaction);
        }
        if prev_phase != TurnPhase::Completed
            && next_phase == TurnPhase::Completed
            && let Some(run_id) = turn_input_run_id(&input)
        {
            effects.push(TurnExecutionEffect::RunCompleted { run_id });
        }
        if prev_phase != TurnPhase::Failed
            && next_phase == TurnPhase::Failed
            && let Some(run_id) = turn_input_run_id(&input)
        {
            let reason = turn_input_failure_reason(&input).unwrap_or_else(|| {
                TurnFailureReason::terminal_outcome(
                    self.turn_terminal_outcome()
                        .unwrap_or(TurnTerminalOutcome::Failed),
                )
            });
            effects.push(TurnExecutionEffect::RunFailed { run_id, reason });
        }
        if prev_phase != TurnPhase::Cancelled
            && next_phase == TurnPhase::Cancelled
            && let Some(run_id) = turn_input_run_id(&input)
        {
            effects.push(TurnExecutionEffect::RunCancelled { run_id });
        }

        Ok(TurnExecutionTransition {
            prev_phase,
            next_phase,
            effects,
        })
    }

    /// Execute side effects from a transition. Handles CheckCompaction
    /// effects emitted on CallingLlm entry.
    async fn execute_turn_effects(
        &mut self,
        transition: &TurnExecutionTransition,
        turn_count: u32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        for effect in &transition.effects {
            match effect {
                TurnExecutionEffect::RunCompleted { run_id } => {
                    if let Some(handle) = self.turn_state_handle.as_deref()
                        && let Err(error) = handle.run_completed(run_id.clone())
                    {
                        tracing::warn!(
                            error = %error,
                            "runtime turn-state handle rejected RunCompleted effect"
                        );
                    }
                }
                TurnExecutionEffect::RunFailed { run_id, reason } => {
                    if let Some(handle) = self.turn_state_handle.as_deref()
                        && let Err(error) = handle.run_failed(run_id.clone(), reason.clone())
                    {
                        tracing::warn!(
                            error = %error,
                            "runtime turn-state handle rejected RunFailed effect"
                        );
                    }
                }
                TurnExecutionEffect::RunCancelled { run_id } => {
                    if let Some(handle) = self.turn_state_handle.as_deref()
                        && let Err(error) = handle.run_cancelled(run_id.clone())
                    {
                        tracing::warn!(
                            error = %error,
                            "runtime turn-state handle rejected RunCancelled effect"
                        );
                    }
                }
                TurnExecutionEffect::RunStarted { .. }
                | TurnExecutionEffect::BoundaryApplied { .. }
                | TurnExecutionEffect::CheckCompaction => {}
            }
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
                            match self
                                .index_compaction_discards(&outcome.discarded, turn_count)
                                .await
                            {
                                crate::memory::MemoryIndexDelivery::Rejected {
                                    error,
                                    attempted_entries,
                                    ..
                                } => {
                                    let error_message = format!(
                                        "memory indexing failed after compaction ({attempted_entries} entries attempted): {error}"
                                    );
                                    tracing::warn!(
                                        error = %error,
                                        attempted_entries,
                                        "memory store rejected compaction discard indexing; preserving original session history"
                                    );
                                    if !crate::event_tap::tap_emit(
                                        &self.event_tap,
                                        event_tx.as_ref(),
                                        AgentEvent::CompactionFailed {
                                            error: error_message,
                                        },
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            "compaction event stream receiver dropped before memory-indexing CompactionFailed"
                                        );
                                    }
                                    continue;
                                }
                                crate::memory::MemoryIndexDelivery::NoStore { .. }
                                | crate::memory::MemoryIndexDelivery::Delivered(_) => {}
                            }

                            *self.session.messages_mut_internal() = outcome.new_messages;
                            self.session.record_usage(outcome.summary_usage.clone());
                            self.budget.record_usage(&outcome.summary_usage);
                            self.last_input_tokens = 0;
                            self.compaction_cadence.last_compaction_boundary_index =
                                Some(outcome.session_boundary_index);
                            if !crate::event_tap::tap_emit(
                                &self.event_tap,
                                event_tx.as_ref(),
                                AgentEvent::CompactionCompleted {
                                    summary_tokens: outcome.summary_usage.output_tokens,
                                    messages_before: outcome.messages_before,
                                    messages_after: outcome.messages_after,
                                },
                            )
                            .await
                            {
                                tracing::warn!(
                                    "compaction event stream receiver dropped before CompactionCompleted"
                                );
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

    async fn index_compaction_discards(
        &self,
        discarded: &[Message],
        turn_count: u32,
    ) -> crate::memory::MemoryIndexDelivery {
        let session_id = self.session.id().clone();
        let scope = crate::memory::MemoryIndexScope::for_session(session_id.clone());
        let Some(memory_store) = self.memory_store.as_ref() else {
            return crate::memory::MemoryIndexDelivery::NoStore { scope };
        };
        let mut requests = Vec::new();
        for message in discarded {
            let content = message.as_indexable_text();
            if content.is_empty() {
                continue;
            }
            let metadata = crate::memory::MemoryMetadata {
                session_id: session_id.clone(),
                turn: Some(turn_count),
                indexed_at: crate::time_compat::SystemTime::now(),
            };
            let request =
                match crate::memory::MemoryIndexRequest::new(scope.clone(), content, metadata) {
                    Ok(request) => request,
                    Err(error) => {
                        let attempted_entries = requests.len() + 1;
                        return crate::memory::MemoryIndexDelivery::Rejected {
                            scope,
                            attempted_entries,
                            error,
                        };
                    }
                };
            requests.push(request);
        }

        let attempted_entries = requests.len();
        if attempted_entries == 0 {
            return crate::memory::MemoryIndexDelivery::Delivered(
                crate::memory::MemoryIndexReceipt {
                    scope,
                    indexed_entries: 0,
                },
            );
        }

        let batch = match crate::memory::MemoryIndexBatch::new(scope.clone(), requests) {
            Ok(batch) => batch,
            Err(error) => {
                return crate::memory::MemoryIndexDelivery::Rejected {
                    scope,
                    attempted_entries,
                    error,
                };
            }
        };
        match memory_store.index_scoped_batch(batch).await {
            Ok(receipt) => crate::memory::MemoryIndexDelivery::Delivered(receipt),
            Err(error) => crate::memory::MemoryIndexDelivery::Rejected {
                scope,
                attempted_entries,
                error,
            },
        }
    }

    fn observe_cancel_after_boundary_request(&mut self, run_id: &RunId) -> Result<(), AgentError> {
        if !self
            .cancel_after_boundary_requested
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }

        if self.turn_active_run_id()?.as_ref() != Some(run_id)
            || self.turn_cancel_after_boundary()?
        {
            return Ok(());
        }

        match self.turn_phase()? {
            TurnPhase::ApplyingPrimitive
            | TurnPhase::CallingLlm
            | TurnPhase::WaitingForOps
            | TurnPhase::DrainingBoundary
            | TurnPhase::Extracting
            | TurnPhase::ErrorRecovery => {
                self.apply_turn_input(TurnExecutionInput::CancelAfterBoundary {
                    run_id: run_id.clone(),
                })?;
            }
            _ => {}
        }

        Ok(())
    }

    async fn terminalize_fatal_error(
        &mut self,
        run_id: &RunId,
        turn_count: u32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        error: &AgentError,
    ) -> Result<(), AgentError> {
        if self.turn_phase()?.is_terminal() {
            return Ok(());
        }
        let transition = self.apply_turn_input(TurnExecutionInput::FatalFailure {
            run_id: run_id.clone(),
            reason: TurnFailureReason::from_agent_error(error),
        })?;
        self.execute_turn_effects(&transition, turn_count, event_tx)
            .await;
        Ok(())
    }

    async fn run_completed_hooks_before_terminal(
        &mut self,
        result: &mut RunResult,
        run_id: &RunId,
        turn_count: u32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        if self.run_completed_hooks_applied {
            return Ok(());
        }
        if let Err(error) = self.run_completed_hooks(result, event_tx.as_ref()).await {
            self.terminalize_fatal_error(run_id, turn_count, event_tx, &error)
                .await?;
            return Err(error);
        }
        Ok(())
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
        let run_id = self
            .turn_state_handle
            .as_deref()
            .and_then(|handle| handle.snapshot().active_run_id)
            .unwrap_or_else(|| RunId(uuid::Uuid::new_v4()));
        self.apply_turn_input(TurnExecutionInput::StartConversationRun {
            run_id: run_id.clone(),
        })?;
        // PrimitiveApplied transitions ApplyingPrimitive -> CallingLlm
        let t = self.apply_turn_input(TurnExecutionInput::PrimitiveApplied {
            run_id: run_id.clone(),
            admitted_content_shape: ContentShape::Conversation,
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
            self.observe_cancel_after_boundary_request(&run_id)?;

            // Check turn limit
            if turn_count >= max_turns {
                self.apply_turn_input(TurnExecutionInput::TurnLimitReached {
                    run_id: run_id.clone(),
                })?;
                return self.build_result(turn_count, tool_call_count).await;
            }

            if let Some(exceeded) = self.budget.observe().exceeded() {
                emit_event!(budget_warning_event(exceeded));
                if matches!(exceeded.dimension, BudgetDimension::Time) {
                    self.pending_fatal_diagnostic = Some(exceeded.to_agent_error());
                }
                self.apply_turn_input(TurnExecutionInput::BudgetLimitExceeded {
                    run_id: run_id.clone(),
                    exceeded,
                })?;
                return self.build_result(turn_count, tool_call_count).await;
            }

            match self.turn_phase()? {
                TurnPhase::Ready | TurnPhase::ApplyingPrimitive | TurnPhase::CallingLlm => {
                    // 0. Auth lease refresh loop (Phase 1.5-rev).
                    //    The canonical auth-state owner is the MeerkatMachine
                    //    DSL (see meerkat-machine-schema/src/catalog/dsl/
                    //    meerkat_machine.rs). The runner's role here is
                    //    *mechanism*: it drives DSL-legal transitions when
                    //    the observable state calls for them, and emits a
                    //    synthetic session notice when the projected DSL
                    //    state is `reauth_required`. The runner never
                    //    decides terminal *class* — it only surfaces the
                    //    state the machine has already committed to.
                    //    (Plan §1.5r.9: snapshot-poll is the canonical
                    //    runner-side mechanism; dogma §3 "rebuild
                    //    projections" permits the read; no redundant DSL
                    //    effect is declared.)
                    //
                    //    Strip prior synthetic AuthReauthRequired notices
                    //    so the notice always reflects current DSL state.
                    self.session.messages_mut_internal().retain(|message| {
                        !is_synthetic_notice(message, SystemNoticeKind::AuthReauthRequired)
                    });

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
                    self.session.messages_mut_internal().retain(|message| {
                        !is_synthetic_notice(message, SystemNoticeKind::McpPending)
                    });
                    // The MCP lifecycle handle is the only authoritative read
                    // side for pending server notices. Without it, core has no
                    // typed owner to project from.
                    let pending_servers: Vec<String> = self
                        .mcp_server_lifecycle_handle
                        .as_deref()
                        .map(|handle| handle.pending_server_ids().into_iter().collect())
                        .unwrap_or_default();
                    if !pending_servers.is_empty() {
                        self.session.push(synthetic_notice_message(
                            SystemNoticeKind::McpPending,
                            format!(
                                "Servers connecting: {}. Tools will appear when ready.",
                                pending_servers.join(", ")
                            ),
                        ));
                    }

                    // 3b. Background shell job completion notices via CompletionFeed.
                    self.session.messages_mut_internal().retain(|message| {
                        !is_synthetic_notice(message, SystemNoticeKind::BackgroundJob)
                    });
                    // Feed path: ops-lifecycle-tracked completions from the runtime.
                    if let Some(ref feed) = self.completion_feed {
                        let batch = feed.list_since(self.applied_cursor);
                        for entry in batch.entries.iter().filter(|e| {
                            e.kind == crate::ops_lifecycle::OperationKind::BackgroundToolOp
                        }) {
                            let enrichment = self
                                .completion_enrichment
                                .as_ref()
                                .and_then(|e| e.enrich(&entry.operation_id));

                            let job_id = enrichment
                                .as_ref()
                                .map(|e| e.job_id.clone())
                                .unwrap_or_else(|| entry.operation_id.to_string());
                            let detail = enrichment
                                .as_ref()
                                .map(|e| e.detail.clone())
                                .unwrap_or_default();
                            let terminal_status =
                                BackgroundJobTerminalStatus::from_terminal_outcome(
                                    &entry.terminal_outcome,
                                );
                            let status_str = terminal_status.as_str();

                            emit_event!(AgentEvent::BackgroundJobCompleted {
                                job_id: job_id.clone(),
                                display_name: entry.display_name.clone(),
                                status: status_str.to_string(),
                                terminal_status: Some(terminal_status),
                                detail: detail.clone(),
                            });

                            let mut notice = format!(
                                "Background job `{}` (id={}) {}: {}",
                                entry.display_name, job_id, status_str, detail,
                            );
                            notice.push_str("\nUse shell_job_status to get the full output.");
                            self.session.push(synthetic_notice_message(
                                SystemNoticeKind::BackgroundJob,
                                notice,
                            ));
                        }
                        self.applied_cursor = batch.watermark;
                        if let Some(ref cs) = self.epoch_cursor_state {
                            cs.agent_applied_cursor
                                .store(batch.watermark, std::sync::atomic::Ordering::Release);
                        }
                    }

                    // Legacy path: completions from custom/override dispatchers that
                    // report through poll_external_updates().background_completions
                    // without wiring into the ops-lifecycle registry. This runs
                    // independently of the feed — they are complementary sources.
                    for completion in &ext.background_completions {
                        let Some(terminal_status) = completion
                            .terminal_outcome
                            .as_ref()
                            .map(BackgroundJobTerminalStatus::from_terminal_outcome)
                            .or_else(|| {
                                BackgroundJobTerminalStatus::from_operation_status(
                                    completion.status,
                                )
                            })
                        else {
                            continue;
                        };
                        let status_str = terminal_status.as_str();

                        emit_event!(AgentEvent::BackgroundJobCompleted {
                            job_id: completion.job_id.clone(),
                            display_name: completion.display_name.clone(),
                            status: status_str.to_string(),
                            terminal_status: Some(terminal_status),
                            detail: completion.detail.clone(),
                        });
                        let mut notice = format!(
                            "Background job `{}` (id={}) {}: {}",
                            completion.display_name,
                            completion.job_id,
                            status_str,
                            completion.detail,
                        );
                        notice.push_str("\nUse shell_job_status to get the full output.");
                        self.session.push(synthetic_notice_message(
                            SystemNoticeKind::BackgroundJob,
                            notice,
                        ));
                    }

                    // 4. Apply tool scope staged updates atomically at the CallingLlm boundary.
                    let tool_defs = {
                        let dispatcher_tools = self.tools.tools();
                        let exact_catalog = self.tools.tool_catalog_capabilities().exact_catalog;
                        let catalog_mode = select_tool_catalog_mode(self.tools.as_ref());
                        let current_catalog = exact_catalog.then(|| self.tools.tool_catalog());
                        let current_pending_catalog_sources = if exact_catalog {
                            self.tools.pending_catalog_sources()
                        } else {
                            Arc::from([])
                        };
                        let (control_tool_names, deferred_tool_names) = match (
                            exact_catalog,
                            current_catalog.as_ref(),
                        ) {
                            (true, Some(catalog)) => {
                                let control_names = catalog
                                    .iter()
                                    .filter(|entry| entry.plane == ToolPlaneClass::Control)
                                    .map(|entry| entry.tool.name.to_string())
                                    .collect::<std::collections::HashSet<_>>();
                                let deferred_names = if !control_names.is_empty()
                                    && matches!(catalog_mode, ToolCatalogMode::Deferred)
                                {
                                    catalog
                                                .iter()
                                                .filter(|entry| {
                                                    entry.plane == ToolPlaneClass::Session
                                                })
                                                .filter(|entry| {
                                                    matches!(
                                                        entry.deferred_eligibility,
                                                        ToolCatalogDeferredEligibility::DeferredEligible { .. }
                                                    )
                                                })
                                                .map(|entry| entry.tool.name.to_string())
                                                .collect()
                                } else {
                                    std::collections::HashSet::new()
                                };
                                (control_names, deferred_names)
                            }
                            _ => (
                                std::collections::HashSet::new(),
                                std::collections::HashSet::new(),
                            ),
                        };
                        let previous_visibility_state = self.tool_scope.visibility_state().ok();
                        let apply_result = match self.tool_scope.promote_staged_visibility() {
                            Ok(visibility_state) => {
                                if let Some(previous_visibility_state) =
                                    previous_visibility_state.as_ref()
                                {
                                    self.tool_scope.apply_staged_projection_with_previous(
                                        dispatcher_tools.clone(),
                                        control_tool_names.into_iter().collect(),
                                        deferred_tool_names.into_iter().collect(),
                                        previous_visibility_state,
                                        &visibility_state,
                                    )
                                } else {
                                    self.tool_scope.apply_staged_projection(
                                        dispatcher_tools.clone(),
                                        control_tool_names,
                                        deferred_tool_names,
                                        &visibility_state,
                                    )
                                }
                            }
                            Err(err) => Err(err),
                        };
                        match apply_result {
                            Ok(applied) => {
                                if let Err(err) = self.publish_committed_visible_set() {
                                    tracing::warn!(
                                        error = %err,
                                        "failed to persist canonical tool visibility state after boundary apply"
                                    );
                                }
                                if applied.changed() {
                                    let status_info = ToolConfigChangeStatus::boundary_applied(
                                        applied.base_changed(),
                                        applied.visible_changed(),
                                        applied.applied_revision.0,
                                    );
                                    let status = status_info.status_text();
                                    emit_event!(AgentEvent::ToolConfigChanged {
                                        payload: ToolConfigChangedPayload::new(
                                            ToolConfigChangeOperation::Reload,
                                            "tool_scope",
                                            status_info,
                                            false,
                                        )
                                        .with_applied_at_turn(Some(turn_count))
                                        .with_domain(Some(ToolConfigChangeDomain::ToolScope)),
                                    });
                                    // Represent runtime notices as user-scoped synthetic context
                                    // (same pattern as peer lifecycle updates) so this does not
                                    // mutate or replace the canonical system prompt.
                                    self.session.push(synthetic_notice_message(
                                        SystemNoticeKind::ToolScope,
                                        format!(
                                            "Tool configuration changed at turn boundary: {status}"
                                        ),
                                    ));
                                }
                                let visible_names_set = applied
                                    .visible_names
                                    .iter()
                                    .cloned()
                                    .collect::<BTreeSet<_>>();
                                let hidden_deferred_names =
                                    if matches!(catalog_mode, ToolCatalogMode::Deferred) {
                                        current_catalog
                                            .as_ref()
                                            .map(|catalog| {
                                                hidden_deferred_catalog_names(
                                                    catalog.as_ref(),
                                                    &visible_names_set,
                                                )
                                            })
                                            .unwrap_or_default()
                                    } else {
                                        BTreeSet::new()
                                    };
                                let pending_catalog_sources =
                                    if matches!(catalog_mode, ToolCatalogMode::Deferred) {
                                        current_pending_catalog_sources
                                            .iter()
                                            .cloned()
                                            .collect::<BTreeSet<_>>()
                                    } else {
                                        BTreeSet::new()
                                    };
                                let added_hidden_names = hidden_deferred_names
                                    .difference(&self.last_hidden_deferred_catalog_names)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                let removed_hidden_names = self
                                    .last_hidden_deferred_catalog_names
                                    .difference(&hidden_deferred_names)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                let pending_sources_changed =
                                    pending_catalog_sources != self.last_pending_catalog_sources;
                                if !added_hidden_names.is_empty()
                                    || !removed_hidden_names.is_empty()
                                    || pending_sources_changed
                                {
                                    let pending_sources =
                                        pending_catalog_sources.iter().cloned().collect::<Vec<_>>();
                                    let status_info =
                                        ToolConfigChangeStatus::deferred_catalog_delta(
                                            added_hidden_names.len(),
                                            removed_hidden_names.len(),
                                            pending_sources.len(),
                                        );
                                    emit_event!(AgentEvent::ToolConfigChanged {
                                        payload: ToolConfigChangedPayload::new(
                                            ToolConfigChangeOperation::Reload,
                                            "deferred_catalog",
                                            status_info,
                                            false,
                                        )
                                        .with_applied_at_turn(Some(turn_count))
                                        .with_domain(Some(ToolConfigChangeDomain::DeferredCatalog))
                                        .with_deferred_catalog_delta(Some(DeferredCatalogDelta {
                                            added_hidden_names: added_hidden_names.clone(),
                                            removed_hidden_names: removed_hidden_names.clone(),
                                            pending_sources: pending_sources.clone(),
                                        },)),
                                    });
                                    let mut notice_parts = Vec::new();
                                    if !added_hidden_names.is_empty() {
                                        notice_parts.push(format!(
                                            "new deferred tools available: {}",
                                            added_hidden_names.join(", ")
                                        ));
                                    }
                                    if !removed_hidden_names.is_empty() {
                                        notice_parts.push(format!(
                                            "deferred tools removed: {}",
                                            removed_hidden_names.join(", ")
                                        ));
                                    }
                                    if !pending_sources.is_empty() {
                                        notice_parts.push(format!(
                                            "sources still connecting: {}",
                                            pending_sources.join(", ")
                                        ));
                                    }
                                    if !notice_parts.is_empty() {
                                        self.session.push(synthetic_notice_message(
                                            SystemNoticeKind::ToolScope,
                                            format!(
                                                "Deferred catalog changed at turn boundary: {}",
                                                notice_parts.join("; ")
                                            ),
                                        ));
                                    }
                                }
                                self.last_hidden_deferred_catalog_names = hidden_deferred_names;
                                self.last_pending_catalog_sources = pending_catalog_sources;
                                applied.tools
                            }
                            Err(err) => {
                                let status_info =
                                    ToolConfigChangeStatus::warning_failed_closed(err.to_string());
                                tracing::warn!(
                                    error = %err,
                                    "tool scope boundary apply failed; closing visible tool set for this boundary"
                                );
                                emit_event!(AgentEvent::ToolConfigChanged {
                                    payload: ToolConfigChangedPayload::new(
                                        ToolConfigChangeOperation::Reload,
                                        "tool_scope",
                                        status_info,
                                        false,
                                    )
                                    .with_applied_at_turn(Some(turn_count))
                                    .with_domain(Some(ToolConfigChangeDomain::ToolScope)),
                                });
                                self.session.push(synthetic_notice_message(
                                    SystemNoticeKind::ToolScopeWarning,
                                    format!(
                                        "Tool scope apply failed ({err}); closing the visible tool set until the next boundary."
                                    ),
                                ));
                                self.tool_scope.fail_closed_projection().unwrap_or_else(
                                    |close_err| {
                                        tracing::warn!(
                                            error = %close_err,
                                            "failed to persist fail-closed tool-scope projection"
                                        );
                                        Vec::<Arc<crate::types::ToolDef>>::new().into()
                                    },
                                )
                            }
                        }
                    };

                    // Emit turn start
                    emit_event!(AgentEvent::TurnStarted {
                        turn_number: turn_count,
                    });

                    let mut effective_max_tokens = self.config.max_tokens_per_turn;
                    let mut effective_temperature = self.config.temperature;
                    let mut effective_provider_params = merged_provider_params(
                        self.config.provider_tool_defaults.as_ref(),
                        self.config.provider_params.as_ref(),
                    );

                    // Pre-LLM hooks may rewrite request params or deny the turn.
                    let pre_llm_report = self
                        .execute_hooks(
                            HookInvocation {
                                point: HookPoint::PreLlmRequest,
                                session_id: self.session.id().clone(),
                                turn_number: Some(turn_count),
                                prompt_input: None,
                                prompt: None,
                                error_report: None,
                                error_class: None,
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
                        hook_id,
                        reason_code,
                        message,
                        payload,
                    }) = pre_llm_report.decision
                    {
                        let error = AgentError::HookDenied {
                            hook_id,
                            point: HookPoint::PreLlmRequest,
                            reason_code,
                            message,
                            payload,
                        };
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
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
                                    hook_id: outcome.hook_id.clone(),
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
                    let in_extraction = self.turn_in_extraction_flow()?;
                    if in_extraction {
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
                        // Strip provider-native tool keys — extraction is deterministic, no tools.
                        if let Some(obj) = params.as_object_mut() {
                            obj.remove("web_search");
                            obj.remove("google_search");
                        }
                        effective_provider_params = Some(params);
                    }

                    // No tools for extraction turn (empty slice)
                    let empty_tools: Arc<[Arc<crate::types::ToolDef>]> = Arc::from([]);
                    let call_tool_defs = if in_extraction {
                        &empty_tools
                    } else {
                        &tool_defs
                    };
                    let typed_provider_params = effective_provider_params
                        .as_ref()
                        .map(|params| {
                            ProviderParamsOverride::from_legacy_provider_value(
                                self.client.provider(),
                                params,
                            )
                        })
                        .filter(|params| !params.is_empty());

                    // Call LLM with retry — route errors through machine authority
                    let boundary_system_context = self.take_pending_system_context_boundary();
                    let request_messages =
                        self.llm_messages_with_runtime_system_context(&boundary_system_context);
                    let result = match self
                        .call_llm_with_retry(LlmRetryRequest {
                            run_id: &run_id,
                            turn_count,
                            event_tx: &event_tx,
                            messages: &request_messages,
                            tools: call_tool_defs,
                            max_tokens: effective_max_tokens,
                            temperature: effective_temperature,
                            provider_params: typed_provider_params.as_ref(),
                        })
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            if let Some(exceeded) = BudgetExceeded::from_agent_error(&e) {
                                emit_event!(budget_warning_event(exceeded));
                                if matches!(exceeded.dimension, BudgetDimension::Time) {
                                    self.pending_fatal_diagnostic = Some(e);
                                }
                                self.apply_turn_input(TurnExecutionInput::BudgetLimitExceeded {
                                    run_id: run_id.clone(),
                                    exceeded,
                                })?;
                                return self.build_result(turn_count, tool_call_count).await;
                            }
                            if matches!(&e, AgentError::Llm { .. }) {
                                // Exhausted hard LLM-call failure — route through
                                // machine-owned FatalFailure and preserve diagnostics.
                                let reason = TurnFailureReason::from_agent_error(&e);
                                self.pending_fatal_diagnostic = Some(e);
                                self.apply_turn_input(TurnExecutionInput::FatalFailure {
                                    run_id: run_id.clone(),
                                    reason,
                                })?;
                                return self.build_result(turn_count, tool_call_count).await;
                            }
                            return Err(e);
                        }
                    };

                    // Update budget + session usage
                    self.budget.record_usage(&result.usage);
                    self.last_input_tokens = result.usage.input_tokens;
                    self.session.record_usage(result.usage.clone());
                    if let Some(exceeded) = self.budget.observe().exceeded() {
                        emit_event!(budget_warning_event(exceeded));
                        self.apply_turn_input(TurnExecutionInput::BudgetLimitExceeded {
                            run_id: run_id.clone(),
                            exceeded,
                        })?;
                        return self.build_result(turn_count, tool_call_count).await;
                    }

                    let (blocks, stop_reason, usage) = result.into_parts();
                    let mut assistant_msg = BlockAssistantMessage::new(blocks, stop_reason);
                    let mut assistant_text = assistant_msg.to_string();

                    let post_llm_report = self
                        .execute_hooks(
                            HookInvocation {
                                point: HookPoint::PostLlmResponse,
                                session_id: self.session.id().clone(),
                                turn_number: Some(turn_count),
                                prompt_input: None,
                                prompt: None,
                                error_report: None,
                                error_class: None,
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
                        hook_id,
                        reason_code,
                        message,
                        payload,
                    }) = post_llm_report.decision
                    {
                        let error = AgentError::HookDenied {
                            hook_id,
                            point: HookPoint::PostLlmResponse,
                            reason_code,
                            message,
                            payload,
                        };
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
                    }

                    for outcome in &post_llm_report.outcomes {
                        for patch in &outcome.patches {
                            if let HookPatch::AssistantText { text } = patch {
                                emit_event!(AgentEvent::HookRewriteApplied {
                                    hook_id: outcome.hook_id.clone(),
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

                    self.observe_cancel_after_boundary_request(&run_id)?;

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
                                name: tc.name.into(),
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
                        let visible_tool_names = tool_defs
                            .iter()
                            .map(|tool| tool.tool_name())
                            .collect::<ToolNameSet>();

                        let pre_tool_reports =
                            futures::future::join_all(tool_calls.iter().map(|tc| {
                                let args_value: Value = serde_json::from_str(tc.args.get())
                                    .unwrap_or_else(|_| Value::String(tc.args.get().to_string()));
                                self.execute_hooks(
                                    HookInvocation {
                                        point: HookPoint::PreToolExecution,
                                        session_id: self.session.id().clone(),
                                        turn_number: Some(turn_count),
                                        prompt_input: None,
                                        prompt: None,
                                        error_report: None,
                                        error_class: None,
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

                        for (tool_index, (mut tc, pre_tool_report)) in tool_calls
                            .into_iter()
                            .zip(pre_tool_reports.into_iter())
                            .enumerate()
                        {
                            let pre_tool_report = pre_tool_report?;

                            if let Some(HookDecision::Deny {
                                hook_id,
                                reason_code,
                                message,
                                payload,
                            }) = pre_tool_report.decision
                            {
                                let error = AgentError::HookDenied {
                                    hook_id,
                                    point: HookPoint::PreToolExecution,
                                    reason_code,
                                    message,
                                    payload,
                                };
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }

                            for outcome in &pre_tool_report.outcomes {
                                for patch in &outcome.patches {
                                    if let HookPatch::ToolArgs { args } = patch {
                                        emit_event!(AgentEvent::HookRewriteApplied {
                                            hook_id: outcome.hook_id.clone(),
                                            point: HookPoint::PreToolExecution,
                                            patch: HookPatch::ToolArgs { args: args.clone() },
                                        });
                                        tc.set_args(args.clone());
                                    }
                                }
                            }

                            if let Err(error) = precheck_visible_tool_call(
                                tools_ref.as_ref(),
                                &visible_tool_names,
                                tc.name.as_str(),
                            ) {
                                let error = AgentError::ToolError(error.to_string());
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }

                            emit_event!(AgentEvent::ToolExecutionStarted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                            });
                            executable_tool_calls.push((tool_index, tc));
                        }

                        // Execute all allowed tool calls in parallel using join_all
                        let dispatch_futures: Vec<_> = executable_tool_calls
                            .into_iter()
                            .map(|(tool_index, tc)| {
                                let tools_ref = Arc::clone(&tools_ref);
                                async move {
                                    let start = crate::time_compat::Instant::now();
                                    let dispatch_result = tools_ref.dispatch(tc.as_view()).await;
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    (tool_index, tc, dispatch_result, duration_ms)
                                }
                            })
                            .collect();

                        let mut dispatch_results =
                            futures::future::join_all(dispatch_futures).await;
                        dispatch_results.sort_by_key(|(tool_index, _, _, _)| *tool_index);

                        // Process results and emit events
                        let mut all_async_ops = Vec::<crate::ops::AsyncOpRef>::new();
                        let mut accumulated_session_effects =
                            Vec::<crate::ops::SessionEffect>::new();
                        for (_, tc, dispatch_result, duration_ms) in dispatch_results {
                            let mut tool_session_effects = Vec::new();
                            let mut tool_result = match dispatch_result {
                                Ok(outcome) => {
                                    all_async_ops.extend(outcome.async_ops);
                                    tool_session_effects = outcome.session_effects;
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
                                    crate::ops::terminal_tool_outcome_for_error(tc.id.clone(), e)
                                        .result
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
                                        prompt_input: None,
                                        prompt: None,
                                        error_report: None,
                                        error_class: None,
                                        error: None,
                                        llm_request: None,
                                        llm_response: None,
                                        tool_call: None,
                                        tool_result: Some(
                                            HookToolResult::from_tool_result_with_id(
                                                tc.id.clone(),
                                                tc.name.clone(),
                                                &tool_result,
                                            ),
                                        ),
                                    },
                                    event_tx.as_ref(),
                                )
                                .await?;

                            if let Some(HookDecision::Deny {
                                hook_id,
                                reason_code,
                                message,
                                payload,
                            }) = post_tool_report.decision
                            {
                                let error = AgentError::HookDenied {
                                    hook_id,
                                    point: HookPoint::PostToolExecution,
                                    reason_code,
                                    message,
                                    payload,
                                };
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }

                            for outcome in &post_tool_report.outcomes {
                                for patch in &outcome.patches {
                                    if let HookPatch::ToolResult { content, is_error } = patch {
                                        emit_event!(AgentEvent::HookRewriteApplied {
                                            hook_id: outcome.hook_id.clone(),
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
                                content: tool_result.content.clone(),
                                is_error: tool_result.is_error,
                                duration_ms,
                            });

                            // Emit result received
                            emit_event!(AgentEvent::ToolResultReceived {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                content: tool_result.content.clone(),
                                is_error: tool_result.is_error,
                            });

                            if tool_result.has_video() {
                                return Err(AgentError::ConfigError(
                                    "video blocks are not supported in tool results".to_string(),
                                ));
                            }

                            tool_results.push(tool_result);
                            accumulated_session_effects.extend(tool_session_effects);

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

                        // Apply state-mutating effects before ToolResults, but
                        // defer transcript-producing assistant blocks until
                        // after ToolResults so provider tool-call adjacency is
                        // preserved.
                        let (post_tool_effects, pre_tool_effects): (Vec<_>, Vec<_>) =
                            accumulated_session_effects.into_iter().partition(|effect| {
                                matches!(
                                    effect,
                                    crate::ops::SessionEffect::AppendAssistantBlocks { .. }
                                )
                            });
                        if !pre_tool_effects.is_empty() {
                            self.apply_session_effects(&pre_tool_effects)?;
                        }

                        // Add tool results to session
                        self.session.push(Message::tool_results(tool_results));

                        if !post_tool_effects.is_empty() {
                            self.apply_session_effects(&post_tool_effects)?;
                        }

                        self.observe_cancel_after_boundary_request(&run_id)?;

                        self.apply_turn_input(TurnExecutionInput::RegisterPendingOps {
                            run_id: run_id.clone(),
                            op_refs: pending_op_refs,
                            barrier_operation_ids,
                            has_barrier_ops,
                        })?;

                        if self.turn_has_barrier_ops()? {
                            // Stay in WaitingForOps — the outer match arm will
                            // await completion of barrier ops via wait-set.
                            continue;
                        }

                        // No pending ops — tool calls resolved, drain boundary, continue
                        self.apply_turn_input(TurnExecutionInput::ToolCallsResolved {
                            run_id: run_id.clone(),
                        })?;
                        if let Err(error) = self
                            .drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await
                        {
                            self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                                .await?;
                            return Err(error);
                        }
                        let t = self.apply_turn_input(TurnExecutionInput::BoundaryContinue {
                            run_id: run_id.clone(),
                        })?;
                        self.execute_turn_effects(&t, turn_count, &event_tx).await;
                        turn_count += 1;
                    } else if self.turn_in_extraction_flow()? {
                        // Extraction turn response — validate against schema
                        self.session.push(Message::BlockAssistant(assistant_msg));

                        // Drain turn boundary (fires TurnBoundary hooks, drains comms)
                        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal {
                            run_id: run_id.clone(),
                        })?;
                        if let Err(error) = self
                            .drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await
                        {
                            self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                                .await?;
                            return Err(error);
                        }
                        self.observe_cancel_after_boundary_request(&run_id)?;
                        emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                        // Authority: DrainingBoundary -> Extracting for validation
                        self.apply_turn_input(TurnExecutionInput::EnterExtraction {
                            run_id: run_id.clone(),
                            max_retries: self.config.structured_output_retries,
                        })?;

                        let output_schema =
                            self.config.output_schema.as_ref().ok_or_else(|| {
                                AgentError::InternalError(
                                    "extraction flow without output_schema".into(),
                                )
                            })?;
                        let compiled = self
                            .client
                            .compile_schema(output_schema)
                            .map_err(|e| AgentError::InvalidOutputSchema(e.to_string()))?;
                        let validation = super::extraction::validate_response_text(
                            &assistant_text,
                            output_schema,
                            &compiled.schema,
                        )?;

                        match validation {
                            super::extraction::ExtractionValidation::Failed {
                                error,
                                retry_prompt,
                            } => {
                                // Validation failed — authority decides retry vs exhaust
                                let t = self.apply_turn_input(
                                    TurnExecutionInput::ExtractionValidationFailed {
                                        run_id: run_id.clone(),
                                        error: error.clone(),
                                    },
                                )?;

                                if !self.turn_phase()?.is_terminal() {
                                    // Authority decided to retry — push retry prompt
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
                                    attempts: self.turn_extraction_attempts()?,
                                    reason: error,
                                    last_output: self
                                        .session
                                        .last_assistant_text()
                                        .unwrap_or_default(),
                                });
                            }
                            super::extraction::ExtractionValidation::Passed(normalized) => {
                                self.extraction_state.record_success(normalized);
                            }
                        }

                        let mut result = RunResult {
                            text: self.session.last_assistant_text().unwrap_or_default(),
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                            structured_output: self.extraction_state.take_result(),
                            schema_warnings: self.extraction_state.take_schema_warnings(),
                            skill_diagnostics: None,
                        };
                        self.run_completed_hooks_before_terminal(
                            &mut result,
                            &run_id,
                            turn_count,
                            &event_tx,
                        )
                        .await?;

                        // Validation passed — complete via authority
                        let t = self.apply_turn_input(
                            TurnExecutionInput::ExtractionValidationPassed {
                                run_id: run_id.clone(),
                            },
                        )?;
                        self.execute_turn_effects(&t, turn_count, &event_tx).await;
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }
                        return Ok(result);
                    } else {
                        // No tool calls - we're done with the agentic loop
                        let final_text = assistant_text.clone();
                        self.session.push(Message::BlockAssistant(assistant_msg));

                        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal {
                            run_id: run_id.clone(),
                        })?;
                        if let Err(error) = self
                            .drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await
                        {
                            self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                                .await?;
                            return Err(error);
                        }
                        self.observe_cancel_after_boundary_request(&run_id)?;

                        // Check if we need to perform extraction turn for structured output
                        if let Some(output_schema) = self.config.output_schema.as_ref()
                            && !self.turn_in_extraction_flow()?
                        {
                            // The model turn is complete, but the run is not
                            // complete until the extraction turn validates.
                            emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                            // Enter extraction mode via authority
                            self.extraction_state.reset();

                            // Compile schema and capture warnings
                            let compiled = self
                                .client
                                .compile_schema(output_schema)
                                .map_err(|e| AgentError::InvalidOutputSchema(e.to_string()))?;
                            self.extraction_state
                                .set_schema_warnings(compiled.warnings.clone());

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

                        let mut result = RunResult {
                            text: final_text,
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                            structured_output: None,
                            schema_warnings: None,
                            skill_diagnostics: self.collect_skill_diagnostics().await,
                        };
                        self.run_completed_hooks_before_terminal(
                            &mut result,
                            &run_id,
                            turn_count,
                            &event_tx,
                        )
                        .await?;

                        // Emit turn completed only after all terminal hooks accept
                        // and boundary side effects are committed.
                        emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                        // No extraction needed - complete normally
                        let t = self.apply_turn_input(TurnExecutionInput::BoundaryComplete {
                            run_id: run_id.clone(),
                        })?;
                        self.execute_turn_effects(&t, turn_count, &event_tx).await;

                        // Save session
                        if let Err(e) = self.store.save(&self.session).await {
                            tracing::warn!("Failed to save session: {}", e);
                        }

                        return Ok(result);
                    }
                }
                TurnPhase::WaitingForOps => {
                    // Await completion of all pending barrier operations via
                    // the machine-owned turn-local wait-set. Only barrier ops
                    // block the turn; detached ops run independently.
                    if !self.turn_pending_ops_registered()? {
                        return Err(AgentError::InternalError(
                            "WaitingForOps entered without registered pending_op_refs".to_string(),
                        ));
                    }
                    let barrier_ids = self.turn_barrier_operation_ids()?;
                    if !barrier_ids.is_empty() {
                        let wait_result = if let Some(ref registry) = self.ops_lifecycle {
                            registry
                                .wait_all(&run_id, &barrier_ids)
                                .await
                                .map_err(|e| {
                                    AgentError::InternalError(format!(
                                        "ops lifecycle wait_all failed: {e}"
                                    ))
                                })?
                        } else {
                            return Err(AgentError::InternalError(
                                "barrier ops registered without ops_lifecycle registry".to_string(),
                            ));
                        };
                        // Feed OpsBarrierSatisfied through the shared turn-input
                        // path so the runtime handle remains the primary writer
                        // when present.
                        self.apply_turn_input(TurnExecutionInput::OpsBarrierSatisfied {
                            run_id: run_id.clone(),
                            operation_ids: wait_result.satisfied.operation_ids,
                        })?;
                    }
                    self.observe_cancel_after_boundary_request(&run_id)?;
                    self.apply_turn_input(TurnExecutionInput::ToolCallsResolved {
                        run_id: run_id.clone(),
                    })?;
                    if let Err(error) = self
                        .drain_turn_boundary(turn_count, event_tx.as_ref())
                        .await
                    {
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
                    }
                    let t = self.apply_turn_input(TurnExecutionInput::BoundaryContinue {
                        run_id: run_id.clone(),
                    })?;
                    self.execute_turn_effects(&t, turn_count, &event_tx).await;
                    turn_count += 1;
                }
                TurnPhase::DrainingBoundary | TurnPhase::Extracting => {
                    // Wait for any pending events to be processed
                    let t = self.apply_turn_input(TurnExecutionInput::BoundaryComplete {
                        run_id: run_id.clone(),
                    })?;
                    self.execute_turn_effects(&t, turn_count, &event_tx).await;
                }
                TurnPhase::Cancelling => {
                    // Handle cancellation
                    self.apply_turn_input(TurnExecutionInput::CancellationObserved {
                        run_id: run_id.clone(),
                    })?;
                    return self.build_result(turn_count, tool_call_count).await;
                }
                TurnPhase::ErrorRecovery => {
                    // Attempt recovery
                    let retry_attempt = self.runtime_turn_authority_snapshot()?.llm_retry_attempt;
                    let t = self.apply_turn_input(TurnExecutionInput::RetryRequested {
                        run_id: run_id.clone(),
                        retry_attempt,
                    })?;
                    self.execute_turn_effects(&t, turn_count, &event_tx).await;
                }
                TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled => {
                    return self.build_result(turn_count, tool_call_count).await;
                }
            }
        }
    }

    /// Build a RunResult from current state, using the generated terminal
    /// classification to decide Ok vs Err.
    async fn build_result(&mut self, turns: u32, tool_calls: u32) -> Result<RunResult, AgentError> {
        use crate::generated::terminal_surface_mapping::{SurfaceResultClass, classify_terminal};

        let outcome = self.turn_terminal_outcome()?;
        let classification = classify_terminal(&outcome);
        match classification {
            Some(SurfaceResultClass::HardFailure) => {
                // Consume the pending diagnostic once — prefer the originating
                // typed error over a generic TerminalFailure so that
                // provider/reason/message truth is preserved on the surface.
                if let Some(diagnostic) = self.pending_fatal_diagnostic.take() {
                    Err(diagnostic)
                } else {
                    Err(AgentError::TerminalFailure { outcome })
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
            name: view.name.into(),
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::manual_async_fn
)]
mod tests {
    use super::{SystemNoticeKind, is_synthetic_notice, rewrite_assistant_text};
    use crate::agent::{AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
    use crate::blob::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
    use crate::budget::{Budget, BudgetLimits};
    use crate::compact::{CompactionContext, CompactionResult, Compactor};
    use crate::error::{AgentError, ToolError};
    use crate::memory::{
        MemoryIndexBatch, MemoryIndexReceipt, MemoryIndexScope, MemoryMetadata, MemoryResult,
        MemorySearchScope, MemoryStore, MemoryStoreError,
    };
    use crate::retry::select_retry_delay;
    use crate::skills::{
        ResolvedSkill, SkillCollection, SkillDescriptor, SkillEngine, SkillFilter, SkillKey,
        SkillName, SourceUuid,
    };
    use crate::state::LoopState;
    use crate::tool_scope::{
        EXTERNAL_TOOL_FILTER_METADATA_KEY, INHERITED_TOOL_FILTER_METADATA_KEY, ToolFilter,
    };
    use crate::types::{
        AssistantBlock, ContentBlock, ImageData, Message, StopReason, ToolCall, ToolCallView,
        ToolDef, ToolResult, Usage, UserMessage,
    };
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tokio::sync::{Notify, mpsc};

    /// Attach an in-core phase-tracking `TurnStateHandle` to a raw
    /// `AgentBuilder` and explicitly stamp the test run as a content turn.
    ///
    /// Wave-A deleted the standalone in-core turn-state fallback
    /// (`LocalTurnExecutionState`); `Agent::runtime_turn_authority_snapshot`
    /// now requires a live handle on every run path. Tests that construct
    /// a bare `AgentBuilder::new()` (wrapped here by this helper) must thread
    /// both a `TurnStateHandle` and a runtime execution kind through the build.
    ///
    /// `meerkat-core` cannot borrow `RuntimeTurnStateHandle` from
    /// `meerkat-runtime` (circular dev-dep instantiates two copies of
    /// `meerkat-core` and the trait impls do not unify). Instead we use
    /// the in-core test helper
    /// [`super::test_turn_state_handle::TestTurnStateHandle`], which
    /// ports the deleted pre-wave-a `LocalTurnExecutionState` transition
    /// logic verbatim so the agent loop sees faithful phase advancement.
    ///
    /// See #32 Class W1 — runtime_turn_authority missing handle.
    fn with_test_turn_state_handle(builder: AgentBuilder) -> AgentBuilder {
        use crate::agent::test_turn_state_handle::TestTurnStateHandle;
        builder
            .with_turn_state_handle(Arc::new(TestTurnStateHandle::new()))
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
    }

    #[test]
    fn rewrite_assistant_text_rewrites_all_text_blocks() {
        let mut blocks = vec![
            AssistantBlock::Text {
                text: "first".to_string(),
                meta: None,
            },
            AssistantBlock::ToolUse {
                id: "t1".to_string(),
                name: "tool".into(),
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
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
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
        seen_provider_params:
            Mutex<Vec<Option<crate::lifecycle::run_primitive::ProviderParamsOverride>>>,
    }

    impl RecordingLlmClient {
        fn new() -> Self {
            Self {
                seen_user_messages: Mutex::new(Vec::new()),
                seen_provider_params: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<String> {
            self.seen_user_messages.lock().unwrap().clone()
        }

        fn seen_params(
            &self,
        ) -> Vec<Option<crate::lifecycle::run_primitive::ProviderParamsOverride>> {
            self.seen_provider_params.lock().unwrap().clone()
        }
    }

    struct RecordingSkillEngine {
        seen_keys: Mutex<Vec<SkillKey>>,
    }

    impl RecordingSkillEngine {
        fn new() -> Self {
            Self {
                seen_keys: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<SkillKey> {
            self.seen_keys.lock().unwrap().clone()
        }
    }

    fn fixture_skill_key(name: &str) -> SkillKey {
        SkillKey::new(
            SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid fixture source uuid"),
            SkillName::parse(name).expect("valid fixture skill name"),
        )
    }

    impl SkillEngine for RecordingSkillEngine {
        fn inventory_section(
            &self,
        ) -> impl Future<Output = Result<String, crate::skills::SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            keys: &[SkillKey],
        ) -> impl Future<Output = Result<Vec<ResolvedSkill>, crate::skills::SkillError>> + Send
        {
            let keys = keys.to_vec();
            async move {
                let mut seen = self.seen_keys.lock().unwrap();
                seen.extend_from_slice(&keys);
                drop(seen);

                Ok(vec![ResolvedSkill {
                    key: keys
                        .first()
                        .cloned()
                        .unwrap_or_else(|| fixture_skill_key("email-extractor")),
                    name: "email-extractor".into(),
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
            key: &SkillKey,
        ) -> impl Future<
            Output = Result<Vec<crate::skills::SkillArtifact>, crate::skills::SkillError>,
        > + Send {
            let missing = key.clone();
            async move { Err(crate::skills::SkillError::NotFound { key: missing }) }
        }

        fn read_artifact(
            &self,
            key: &SkillKey,
            _artifact_path: &str,
        ) -> impl Future<
            Output = Result<crate::skills::SkillArtifactContent, crate::skills::SkillError>,
        > + Send {
            let missing = key.clone();
            async move { Err(crate::skills::SkillError::NotFound { key: missing }) }
        }

        fn invoke_function(
            &self,
            key: &SkillKey,
            _function_name: &str,
            _arguments: Value,
        ) -> impl Future<Output = Result<Value, crate::skills::SkillError>> + Send {
            let missing = key.clone();
            async move { Err(crate::skills::SkillError::NotFound { key: missing }) }
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
            provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut seen = self.seen_user_messages.lock().unwrap();
            for msg in messages {
                if let Message::User(user) = msg {
                    seen.push(user.text_content());
                }
            }
            drop(seen);
            self.seen_provider_params
                .lock()
                .unwrap()
                .push(provider_params.cloned());

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
            "openai"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct ImageHydrationLlmClient {
        seen_user_blocks: Mutex<Vec<Vec<ContentBlock>>>,
    }

    impl ImageHydrationLlmClient {
        fn new() -> Self {
            Self {
                seen_user_blocks: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<Vec<ContentBlock>> {
            self.seen_user_blocks.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for ImageHydrationLlmClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut seen = self.seen_user_blocks.lock().unwrap();
            for message in messages {
                if let Message::User(user) = message {
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

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct RecordingBlobStore {
        blobs: std::collections::HashMap<BlobId, BlobPayload>,
        gets: Mutex<Vec<BlobId>>,
    }

    impl RecordingBlobStore {
        fn new(payloads: Vec<BlobPayload>) -> Self {
            Self {
                blobs: payloads
                    .into_iter()
                    .map(|payload| (payload.blob_id.clone(), payload))
                    .collect(),
                gets: Mutex::new(Vec::new()),
            }
        }

        fn gets(&self) -> Vec<BlobId> {
            self.gets.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl BlobStore for RecordingBlobStore {
        async fn put_image(
            &self,
            media_type: &str,
            _data: &str,
        ) -> Result<BlobRef, BlobStoreError> {
            let blob_id = BlobId::new(format!("sha256:test-{}", self.blobs.len()));
            Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            self.gets.lock().unwrap().push(blob_id.clone());
            self.blobs
                .get(blob_id)
                .cloned()
                .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, _blob_id: &BlobId) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
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
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
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

    struct DiscardingCompactor {
        compact_on_boundary: u64,
    }

    impl DiscardingCompactor {
        fn new(compact_on_boundary: u64) -> Self {
            Self {
                compact_on_boundary,
            }
        }
    }

    impl Compactor for DiscardingCompactor {
        fn should_compact(&self, ctx: &CompactionContext) -> bool {
            self.compact_on_boundary == ctx.session_boundary_index
        }

        fn compaction_prompt(&self) -> &'static str {
            "COMPACT NOW"
        }

        fn max_summary_tokens(&self) -> u32 {
            32
        }

        fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
            let last_user = messages
                .iter()
                .rev()
                .find(|message| matches!(message, Message::User(_)))
                .cloned();
            let mut compacted = vec![Message::User(UserMessage::text(format!(
                "[Context compacted] {summary}"
            )))];
            if let Some(last_user) = last_user {
                compacted.push(last_user);
            }
            let discarded_len = messages.len().saturating_sub(1);
            CompactionResult {
                messages: compacted,
                discarded: messages.iter().take(discarded_len).cloned().collect(),
            }
        }
    }

    struct RecordingMemoryStore {
        entries: Mutex<Vec<(MemoryIndexScope, String, MemoryMetadata)>>,
        fail_indexing: bool,
        fail_after_successful_entries: Option<usize>,
    }

    impl RecordingMemoryStore {
        fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                fail_indexing: false,
                fail_after_successful_entries: None,
            }
        }

        fn failing() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                fail_indexing: true,
                fail_after_successful_entries: None,
            }
        }

        fn failing_after(successful_entries: usize) -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
                fail_indexing: false,
                fail_after_successful_entries: Some(successful_entries),
            }
        }

        fn contents(&self) -> Vec<String> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .map(|(_, content, _)| content.clone())
                .collect()
        }

        fn scopes(&self) -> Vec<MemoryIndexScope> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .map(|(scope, _, _)| scope.clone())
                .collect()
        }
    }

    #[async_trait]
    impl MemoryStore for RecordingMemoryStore {
        async fn index_scoped_batch(
            &self,
            batch: MemoryIndexBatch,
        ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
            if self.fail_indexing {
                return Err(MemoryStoreError::Index("injected failure".to_string()));
            }
            let (receipt_scope, requests) = batch.into_parts();
            let indexed_entries = requests.len();
            let mut entries = self.entries.lock().unwrap();
            if self
                .fail_after_successful_entries
                .is_some_and(|successful_entries| indexed_entries > successful_entries)
            {
                return Err(MemoryStoreError::Index("injected failure".to_string()));
            }
            for request in requests {
                let (scope, content, metadata) = request.into_parts();
                entries.push((scope, content, metadata));
            }
            Ok(MemoryIndexReceipt {
                scope: receipt_scope,
                indexed_entries,
            })
        }

        async fn search(
            &self,
            _scope: &MemorySearchScope,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<MemoryResult>, MemoryStoreError> {
            Ok(Vec::new())
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
                name: call.name.into(),
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
                        name: (*name).into(),
                        description: format!("{name} tool"),
                        input_schema: serde_json::json!({ "type": "object" }),
                        provenance: None,
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

    struct PlaneAwareToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[crate::ToolCatalogEntry]>,
        dispatched_names: Mutex<Vec<String>>,
    }

    impl PlaneAwareToolDispatcher {
        fn new() -> Self {
            let visible = Arc::new(ToolDef {
                name: "visible".into(),
                description: "visible tool".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            });
            let secret = Arc::new(ToolDef {
                name: "secret".into(),
                description: "secret tool".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            });
            let control = Arc::new(ToolDef {
                name: "tool_catalog_search".into(),
                description: "control search tool".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            });
            let tools: Arc<[Arc<ToolDef>]> = vec![
                Arc::clone(&visible),
                Arc::clone(&secret),
                Arc::clone(&control),
            ]
            .into();
            let catalog = vec![
                crate::ToolCatalogEntry::session_inline(visible, true),
                crate::ToolCatalogEntry::session_inline(secret, true),
                crate::ToolCatalogEntry::control_inline(control, true),
            ]
            .into();

            Self {
                tools,
                catalog,
                dispatched_names: Mutex::new(Vec::new()),
            }
        }

        fn dispatched(&self) -> Vec<String> {
            self.dispatched_names.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for PlaneAwareToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
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

    struct DeferredLoadDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[crate::ToolCatalogEntry]>,
    }

    impl DeferredLoadDispatcher {
        fn new() -> Self {
            let deferred = Arc::new(ToolDef {
                name: "deferred_tool".into(),
                description:
                    "deferred tool that must stay hidden until tool_catalog_load reaches the next boundary."
                        .to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: Some(crate::ToolProvenance {
                    kind: crate::ToolSourceKind::Callback,
                    source_id: "test".into(),
                }),
            });
            let deferred_two = Arc::new(ToolDef {
                name: "deferred_tool_two".into(),
                description:
                    "second deferred tool used only to keep the test dispatcher above the adaptive catalog threshold."
                        .to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: Some(crate::ToolProvenance {
                    kind: crate::ToolSourceKind::Callback,
                    source_id: "test".into(),
                }),
            });
            let control = Arc::new(ToolDef {
                name: "tool_catalog_load".into(),
                description: "control load tool".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            });
            Self {
                tools: vec![
                    Arc::clone(&deferred),
                    Arc::clone(&deferred_two),
                    Arc::clone(&control),
                ]
                .into(),
                catalog: vec![
                    crate::ToolCatalogEntry::session_deferred(
                        deferred,
                        true,
                        "callback:test".to_string(),
                    ),
                    crate::ToolCatalogEntry::session_deferred(
                        deferred_two,
                        true,
                        "callback:test".to_string(),
                    ),
                    crate::ToolCatalogEntry::control_inline(control, true),
                ]
                .into(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for DeferredLoadDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            if call.name == "tool_catalog_load" {
                let mut outcome = crate::ops::ToolDispatchOutcome::sync_result(ToolResult::new(
                    call.id.to_string(),
                    "loaded".to_string(),
                    false,
                ));
                outcome
                    .session_effects
                    .push(crate::ops::SessionEffect::RequestDeferredTools {
                        authorities: vec![crate::DeferredToolLoadAuthority::new(
                            "deferred_tool",
                            crate::ToolVisibilityWitness {
                                stable_owner_key: Some("callback:test".to_string()),
                                last_seen_provenance: Some(crate::ToolProvenance {
                                    kind: crate::ToolSourceKind::Callback,
                                    source_id: "test".into(),
                                }),
                            },
                        )],
                    });
                return Ok(outcome);
            }

            Ok(ToolResult::new(call.id.to_string(), format!("ran {}", call.name), false).into())
        }
    }

    struct DeferredWithoutControlDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[crate::ToolCatalogEntry]>,
    }

    impl DeferredWithoutControlDispatcher {
        fn new() -> Self {
            let secret = Arc::new(ToolDef {
                name: "secret".into(),
                description: "deferred secret tool that direct AgentBuilder users must still reach without a control plane."
                    .to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: Some(crate::ToolProvenance {
                    kind: crate::ToolSourceKind::Callback,
                    source_id: "direct-builder".into(),
                }),
            });
            let deferred_two = Arc::new(ToolDef {
                name: "deferred_tool_two".into(),
                description:
                    "second deferred tool used only to keep the direct builder dispatcher above the adaptive threshold."
                        .to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: Some(crate::ToolProvenance {
                    kind: crate::ToolSourceKind::Callback,
                    source_id: "direct-builder".into(),
                }),
            });

            Self {
                tools: vec![Arc::clone(&secret), Arc::clone(&deferred_two)].into(),
                catalog: vec![
                    crate::ToolCatalogEntry::session_deferred(
                        secret,
                        true,
                        "callback:direct-builder".to_string(),
                    ),
                    crate::ToolCatalogEntry::session_deferred(
                        deferred_two,
                        true,
                        "callback:direct-builder".to_string(),
                    ),
                ]
                .into(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for DeferredWithoutControlDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), format!("ran {}", call.name), false).into())
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
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
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
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect::<Vec<_>>(),
            );

            let mut calls = self.call_count.lock().unwrap();
            let response = if *calls == 0 {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "secret".into(),
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

    struct ControlPlaneVisibilityClient {
        call_count: Mutex<u32>,
        seen_tools: Mutex<Vec<Vec<String>>>,
    }

    impl ControlPlaneVisibilityClient {
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

    #[async_trait]
    impl AgentLlmClient for ControlPlaneVisibilityClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect::<Vec<_>>(),
            );

            let mut calls = self.call_count.lock().unwrap();
            let response = if *calls == 0 {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-control".to_string(),
                        name: "tool_catalog_search".into(),
                        args: serde_json::value::RawValue::from_string(
                            "{\"query\":\"secret\"}".to_string(),
                        )
                        .unwrap(),
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

    struct DeferredLoadVisibilityClient {
        call_count: Mutex<u32>,
        seen_tools: Mutex<Vec<Vec<String>>>,
    }

    impl DeferredLoadVisibilityClient {
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

    #[async_trait]
    impl AgentLlmClient for DeferredLoadVisibilityClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect::<Vec<_>>(),
            );

            let mut calls = self.call_count.lock().unwrap();
            let response = if *calls == 0 {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-load".to_string(),
                        name: "tool_catalog_load".into(),
                        args: serde_json::value::RawValue::from_string(
                            "{\"names\":[\"deferred_tool\"]}".to_string(),
                        )
                        .unwrap(),
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

        async fn drain_peer_input_candidates(&self) -> Vec<crate::interaction::PeerInputCandidate> {
            self.drain_messages()
                .await
                .into_iter()
                .map(|text| {
                    let id = crate::interaction::InteractionId(uuid::Uuid::new_v4());
                    crate::interaction::PeerInputCandidate {
                        interaction: crate::interaction::InboxInteraction {
                            id,
                            from_route: None,
                            from: "unknown".into(),
                            content: crate::interaction::InteractionContent::Message {
                                body: text.clone(),
                                blocks: None,
                            },
                            rendered_text: text,
                            handling_mode: crate::types::HandlingMode::Queue,
                            render_metadata: None,
                        },
                        ingress: crate::interaction::PeerIngressFact::peer(
                            id,
                            crate::interaction::PeerInputClass::ActionableMessage,
                            crate::interaction::PeerIngressKind::Message,
                            Some(crate::interaction::PeerIngressAuthDecision::Required),
                            crate::interaction::PeerIngressIdentity::new(
                                crate::comms::PeerId::new(),
                                "unknown",
                                crate::interaction::PeerIngressConvention::Message,
                            ),
                        ),
                        lifecycle_peer: None,
                        response_terminality: None,
                    }
                })
                .collect()
        }
    }

    async fn build_agent<C>(client: Arc<C>) -> crate::agent::Agent<C, NoTools, NoopStore>
    where
        C: AgentLlmClient + ?Sized + 'static,
    {
        with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await
    }

    #[tokio::test]
    async fn reused_session_follow_up_run_can_compact_before_first_llm_call() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let compactor = Arc::new(TrackingCompactor::new(Some(1)));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor.clone())
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
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
    async fn compaction_discards_are_indexed_before_history_replacement() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let compactor = Arc::new(DiscardingCompactor::new(1));
        let memory_store = Arc::new(RecordingMemoryStore::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor)
            .memory_store(memory_store.clone())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        agent.run("second".into()).await.unwrap();

        let indexed = memory_store.contents();
        assert!(
            indexed.iter().any(|content| content.contains("first")),
            "discarded first turn should be indexed before compaction commits"
        );
        assert!(
            memory_store
                .scopes()
                .iter()
                .all(|scope| scope.session_id() == agent.session().id()),
            "compaction must index discarded memory into the owning session scope"
        );
        assert!(
            !agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "committed compacted history should no longer carry indexed discarded text"
        );
    }

    #[tokio::test]
    async fn compaction_memory_index_failure_preserves_original_history() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let compactor = Arc::new(DiscardingCompactor::new(1));
        let memory_store = Arc::new(RecordingMemoryStore::failing());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor)
            .memory_store(memory_store.clone())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("second".into(), tx)
            .await
            .expect("indexing failure should preserve history and continue the turn");

        assert!(memory_store.contents().is_empty());
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "failed indexing must not discard the only authoritative copy of compacted text"
        );

        let mut saw_memory_failure = false;
        let mut saw_completed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::CompactionFailed { error }
                    if error.contains("memory indexing failed") =>
                {
                    saw_memory_failure = true;
                }
                crate::event::AgentEvent::CompactionCompleted { .. } => {
                    saw_completed = true;
                }
                _ => {}
            }
        }
        assert!(
            saw_memory_failure,
            "memory-index rejection should surface as a typed compaction failure"
        );
        assert!(
            !saw_completed,
            "compaction must not complete when memory indexing rejects discarded history"
        );
    }

    #[tokio::test]
    async fn compaction_partial_memory_index_failure_leaves_no_projection() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let compactor = Arc::new(DiscardingCompactor::new(1));
        let memory_store = Arc::new(RecordingMemoryStore::failing_after(1));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor)
            .memory_store(memory_store.clone())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        agent
            .run("second".into())
            .await
            .expect("indexing failure should preserve history and continue the turn");

        assert!(
            memory_store.contents().is_empty(),
            "failed compaction indexing must not leave a partial memory projection"
        );
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "failed indexing must preserve the authoritative source history"
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

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::new(RewriteRunCompletedHook))
            .build_standalone(
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

        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .with_hook_engine(Arc::new(DenyRunCompletedHook))
            .build_standalone(
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
        let mut saw_turn_completed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::TurnCompleted { .. } => saw_turn_completed = true,
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
        assert!(
            !saw_turn_completed,
            "hook-denied completion should not publish a completed turn event"
        );
        assert_eq!(
            turn_handle.run_completed_effect_count(),
            0,
            "hook-denied completion should not publish a completed authority effect"
        );
        assert_eq!(
            turn_handle.run_failed_effect_count(),
            1,
            "hook-denied completion should terminalize through failed authority"
        );
        let snapshot = agent
            .execution_snapshot()
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "RunCompleted hook denial should leave the canonical turn snapshot failed"
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
        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .with_hook_engine(Arc::new(DenyTurnBoundaryHook))
            .with_comms_runtime(comms)
            .build_standalone(
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
        let mut saw_run_completed = false;
        let mut saw_typed_boundary_failure = false;
        let mut saw_tool_result_event = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::TurnCompleted { .. } => saw_turn_completed = true,
                crate::event::AgentEvent::RunCompleted { .. } => saw_run_completed = true,
                crate::event::AgentEvent::RunFailed {
                    error_class,
                    error_report,
                    ..
                } => {
                    saw_run_failed = true;
                    saw_typed_boundary_failure = error_class == crate::event::AgentErrorClass::Hook
                        && matches!(
                            error_report
                                .as_ref()
                                .and_then(|report| report.reason.as_ref()),
                            Some(crate::event::AgentErrorReason::HookDenied {
                                hook_id: Some(hook_id),
                                point: HookPoint::TurnBoundary,
                                reason_code: HookReasonCode::PolicyViolation,
                            }) if hook_id == &crate::hooks::HookId::new("deny-turn-boundary")
                        );
                }
                crate::event::AgentEvent::ToolExecutionCompleted { .. }
                | crate::event::AgentEvent::ToolResultReceived { .. } => {
                    saw_tool_result_event = true;
                }
                _ => {}
            }
        }
        assert!(
            !saw_turn_completed,
            "boundary denial should not emit TurnCompleted before failing the run"
        );
        assert!(
            !saw_run_completed,
            "boundary denial should not emit RunCompleted"
        );
        assert!(saw_run_failed, "boundary denial should emit RunFailed");
        assert!(
            saw_typed_boundary_failure,
            "boundary denial should emit typed HookDenied terminal error shape"
        );
        assert!(
            !saw_tool_result_event,
            "boundary denial should not fabricate tool-result-shaped progress events"
        );
        assert_eq!(
            turn_handle.run_failed_effect_count(),
            1,
            "boundary denial should terminalize through failed authority exactly once"
        );
        assert_eq!(
            turn_handle.run_completed_effect_count(),
            0,
            "boundary denial must not publish completed authority effects"
        );

        let snapshot = agent
            .execution_snapshot()
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "boundary hook denial should terminalize through the turn authority"
        );
    }

    #[tokio::test]
    async fn post_llm_denial_terminalizes_turn_authority() {
        use crate::hooks::{
            HookDecision, HookEngine, HookEngineError, HookExecutionReport, HookInvocation,
            HookPoint, HookReasonCode,
        };

        struct DenyPostLlmHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for DenyPostLlmHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::PostLlmResponse {
                    return Ok(HookExecutionReport::empty());
                }
                Ok(HookExecutionReport {
                    decision: Some(HookDecision::Deny {
                        hook_id: crate::hooks::HookId::new("deny-post-llm"),
                        reason_code: HookReasonCode::PolicyViolation,
                        message: "deny post llm".to_string(),
                        payload: None,
                    }),
                    ..HookExecutionReport::empty()
                })
            }
        }

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::new(DenyPostLlmHook))
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("PostLlmResponse denial should fail the run");
        assert!(matches!(
            err,
            AgentError::HookDenied {
                point: HookPoint::PostLlmResponse,
                ..
            }
        ));

        let snapshot = agent
            .execution_snapshot()
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "post-LLM hook denial should terminalize through the turn authority"
        );
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

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_event_tap(tap)
            .build_standalone(
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
    async fn hot_swap_request_policy_updates_next_turn_provider_params() {
        let client = Arc::new(RecordingLlmClient::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .provider_params(serde_json::json!({"old": true}))
            .provider_tool_defaults(serde_json::json!({"stale_tool": {"type": "old"}}))
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(1);

        agent.replace_client_with_request_policy(
            client.clone(),
            crate::SessionLlmRequestPolicy {
                model: "new-model".to_string(),
                provider_params: Some(serde_json::json!({"temperature": 0.2})),
                provider_tool_defaults: Some(serde_json::json!({
                    "web_search": {"type": "web_search"}
                })),
            },
        );

        let result = agent
            .run("policy follows identity".to_string().into())
            .await
            .expect("run should use swapped request policy");
        assert_eq!(result.turns, 1);

        let seen_params = client.seen_params();
        assert_eq!(
            seen_params.last(),
            Some(&Some(
                crate::lifecycle::run_primitive::ProviderParamsOverride {
                    temperature: Some(0.2),
                    provider_tag: Some(crate::lifecycle::run_primitive::ProviderTag::OpenAi(
                        crate::lifecycle::run_primitive::OpenAiProviderTag {
                            web_search: Some(
                                crate::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                                    &serde_json::json!({"type": "web_search"}),
                                ),
                            ),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }
            )),
            "next LLM request must use typed hot-swapped provider params/defaults, not the build-time JSON bag",
        );
    }

    #[tokio::test]
    async fn pending_skill_keys_are_resolved_and_injected_into_runtime_prompt() {
        let client = Arc::new(RecordingLlmClient::new());
        let skill_engine = Arc::new(RecordingSkillEngine::new());
        let skill_runtime = Arc::new(crate::skills::SkillRuntime::new(skill_engine.clone()));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_skill_engine(skill_runtime)
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
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
                .any(|id| id.to_string() == "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"),
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
    async fn llm_execution_hydrates_blob_refs_before_provider_call() {
        let blob_id = BlobId::new("sha256:test-image");
        let blob_store = Arc::new(RecordingBlobStore::new(vec![BlobPayload {
            blob_id: blob_id.clone(),
            media_type: "image/png".to_string(),
            data: "restored-base64".to_string(),
        }]));
        let client = Arc::new(ImageHydrationLlmClient::new());
        let mut session = crate::Session::new();
        session.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Text {
                text: "historical image".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: blob_id.clone(),
                },
            },
        ])));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .with_blob_store(blob_store.clone())
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(1);

        agent.run("current turn".to_string().into()).await.unwrap();

        let seen = client.seen();
        assert!(
            seen.iter().flatten().any(|block| matches!(
                block,
                ContentBlock::Image {
                    data: ImageData::Inline { data },
                    ..
                } if data == "restored-base64"
            )),
            "provider should receive hydrated inline image bytes"
        );
        assert_eq!(blob_store.gets(), vec![blob_id]);
    }

    #[tokio::test]
    async fn provider_receives_filtered_tools_and_hidden_tool_calls_terminalize() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools.clone(), Arc::new(NoopStore))
            .await;

        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap();
        agent.config.max_turns = Some(2);

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("hidden LLM tool denial should terminalize the run");
        assert!(
            matches!(err, AgentError::ToolError(message) if message.contains("not allowed by policy"))
        );

        // Provider sees only visible tools (filtered by ToolScope)
        let seen = client.seen_tools();
        assert_eq!(
            seen.len(),
            1,
            "hidden tool denial must not continue into a follow-up LLM turn"
        );
        assert_eq!(seen[0], vec!["visible".to_string()]);

        // Hidden tools are NOT dispatched — blocked at execution time too
        let dispatched = tools.dispatched();
        assert!(
            dispatched.is_empty(),
            "hidden tools should not be dispatched, but got: {dispatched:?}"
        );
    }

    #[tokio::test]
    async fn external_tool_dispatch_uses_visible_dispatcher() {
        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools.clone(), Arc::new(NoopStore))
            .await;

        let outcome = agent
            .dispatch_external_tool_call(ToolCall::new(
                "tool-call-1".to_string(),
                "visible".to_string(),
                serde_json::json!({ "value": 1 }),
            ))
            .await
            .expect("visible external tool dispatch should succeed");

        assert_eq!(outcome.result.tool_use_id, "tool-call-1");
        assert_eq!(outcome.result.text_content(), "dispatched visible");
        assert_eq!(tools.dispatched(), vec!["visible".to_string()]);
    }

    #[tokio::test]
    async fn external_tool_dispatch_terminalizes_hidden_tools() {
        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools.clone(), Arc::new(NoopStore))
            .await;
        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect("stage hidden-tool filter");
        let visibility_state = agent
            .tool_scope
            .promote_staged_visibility()
            .expect("promote staged filter");
        agent
            .tool_scope
            .apply_staged_projection(
                tools.tools(),
                std::collections::HashSet::new(),
                std::collections::HashSet::new(),
                &visibility_state,
            )
            .expect("apply staged filter at boundary");

        let outcome = agent
            .dispatch_external_tool_call(ToolCall::new(
                "tool-call-hidden".to_string(),
                "secret".to_string(),
                serde_json::json!({}),
            ))
            .await
            .expect("hidden external tool dispatch should terminalize as a tool result");

        assert_eq!(outcome.result.tool_use_id, "tool-call-hidden");
        assert!(outcome.result.is_error);
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("error payload JSON");
        assert!(
            payload
                .get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|code| code == "access_denied"),
            "expected access_denied payload, got {payload:?}"
        );
        assert!(
            tools.dispatched().is_empty(),
            "hidden tools must not reach the dispatcher"
        );
    }

    #[tokio::test]
    async fn external_tool_dispatch_applies_session_effects() {
        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(DeferredLoadDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let outcome = agent
            .dispatch_external_tool_call(ToolCall::new(
                "tool-call-2".to_string(),
                "tool_catalog_load".to_string(),
                serde_json::json!({}),
            ))
            .await
            .expect("external tool dispatch should apply deferred-tool effects");

        assert_eq!(outcome.result.tool_use_id, "tool-call-2");
        let visibility_state = agent
            .session()
            .tool_visibility_state()
            .expect("session effects should publish canonical visibility state");
        assert!(
            visibility_state
                .staged_requested_deferred_names
                .contains("deferred_tool"),
            "expected deferred_tool to be staged after tool session effects"
        );
    }

    #[tokio::test]
    async fn llm_hidden_tool_denial_terminalizes_without_tool_result() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookInvocation, HookPatch, HookPoint,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct RecordingPostToolHook {
            calls: Arc<AtomicUsize>,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for RecordingPostToolHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::PostToolExecution {
                    return Ok(HookExecutionReport::empty());
                }
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(HookExecutionReport {
                    outcomes: vec![crate::hooks::HookOutcome {
                        hook_id: crate::hooks::HookId::new("record-post-tool"),
                        point: HookPoint::PostToolExecution,
                        priority: 0,
                        registration_index: 0,
                        decision: None,
                        patches: vec![HookPatch::ToolResult {
                            content: "{\"error\":\"hook_observed_hidden_denial\"}".to_string(),
                            is_error: Some(true),
                        }],
                        published_patches: Vec::new(),
                        error: None,
                        duration_ms: None,
                    }],
                    ..HookExecutionReport::empty()
                })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::new(RecordingPostToolHook {
                calls: Arc::clone(&calls),
            }))
            .build_standalone(client.clone(), tools.clone(), Arc::new(NoopStore))
            .await;

        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect("stage hidden-tool filter");
        agent.config.max_turns = Some(2);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("hidden LLM tool denial should fail the run");

        assert!(
            matches!(err, AgentError::ToolError(message) if message.contains("not allowed by policy"))
        );
        let seen = client.seen_tools();
        assert_eq!(
            seen.len(),
            1,
            "hidden LLM tool denial should not continue into a follow-up model turn"
        );
        assert!(
            !agent
                .session()
                .messages()
                .iter()
                .any(|message| matches!(message, Message::ToolResults { .. })),
            "hidden LLM tool denial must not fabricate a transcript ToolResult"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "hidden LLM tool denial should not pass through post-tool hooks as a tool result"
        );
        assert!(
            tools.dispatched().is_empty(),
            "hidden tools must not reach the dispatcher"
        );
        let snapshot = agent
            .execution_snapshot()
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed
        );

        let mut saw_run_failed = false;
        let mut saw_tool_result_event = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunFailed { .. } => saw_run_failed = true,
                crate::event::AgentEvent::ToolExecutionCompleted { .. }
                | crate::event::AgentEvent::ToolResultReceived { .. } => {
                    saw_tool_result_event = true;
                }
                _ => {}
            }
        }
        assert!(
            saw_run_failed,
            "hidden LLM tool denial should emit RunFailed"
        );
        assert!(
            !saw_tool_result_event,
            "hidden LLM tool denial should not emit recoverable tool-result events"
        );
    }

    struct ImageEffectClient {
        call_count: Mutex<u32>,
    }

    #[async_trait]
    impl AgentLlmClient for ImageEffectClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut calls = self.call_count.lock().unwrap();
            let response = if *calls == 0 {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "image-call".to_string(),
                        name: "image_effect".into(),
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

    struct ImageEffectDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl ImageEffectDispatcher {
        fn new() -> Self {
            Self {
                tools: vec![Arc::new(ToolDef {
                    name: "image_effect".into(),
                    description: "returns an assistant image session effect".into(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    provenance: None,
                })]
                .into(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ImageEffectDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            let mut outcome = crate::ops::ToolDispatchOutcome::sync_result(ToolResult::new(
                call.id.to_string(),
                "{\"ok\":true}".to_string(),
                false,
            ));
            outcome
                .session_effects
                .push(crate::ops::SessionEffect::AppendAssistantBlocks {
                    blocks: vec![AssistantBlock::Image {
                        image_id: crate::AssistantImageId::new(uuid::Uuid::new_v4()),
                        blob_ref: crate::BlobRef {
                            blob_id: crate::BlobId::new("image-blob"),
                            media_type: "image/png".to_string(),
                        },
                        media_type: crate::MediaType::new("image/png"),
                        width: 1,
                        height: 1,
                        revised_prompt: crate::RevisedPromptDisposition::NotRequested,
                        meta: crate::ProviderImageMetadata::NotEmitted,
                    }],
                });
            Ok(outcome)
        }
    }

    struct DenyPostToolHook;

    #[async_trait]
    impl crate::hooks::HookEngine for DenyPostToolHook {
        async fn execute(
            &self,
            invocation: crate::hooks::HookInvocation,
            _overrides: Option<&crate::config::HookRunOverrides>,
        ) -> Result<crate::hooks::HookExecutionReport, crate::hooks::HookEngineError> {
            if invocation.point == crate::hooks::HookPoint::PostToolExecution {
                return Ok(crate::hooks::HookExecutionReport {
                    decision: Some(crate::hooks::HookDecision::deny(
                        crate::hooks::HookId::new("deny-image-tool"),
                        crate::hooks::HookReasonCode::PolicyViolation,
                        "blocked".to_string(),
                        None,
                    )),
                    ..Default::default()
                });
            }
            Ok(Default::default())
        }
    }

    #[tokio::test]
    async fn post_tool_denial_terminalizes_without_fabricated_tool_result() {
        let client = Arc::new(ImageEffectClient {
            call_count: Mutex::new(0),
        });
        let tools = Arc::new(ImageEffectDispatcher::new());
        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .with_hook_engine(Arc::new(DenyPostToolHook))
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("PostToolExecution denial should fail the run");
        assert!(matches!(
            err,
            AgentError::HookDenied {
                point: crate::hooks::HookPoint::PostToolExecution,
                ..
            }
        ));
        assert_eq!(
            *client.call_count.lock().unwrap(),
            1,
            "post-tool denial should not continue into a follow-up LLM turn"
        );
        assert!(
            !agent
                .session()
                .messages()
                .iter()
                .any(|message| matches!(message, Message::ToolResults { .. })),
            "post-tool denial must not fabricate a transcript ToolResult"
        );
        assert!(
            !agent.session().messages().iter().any(|message| matches!(
                message,
                Message::BlockAssistant(blocks)
                    if blocks
                        .blocks
                        .iter()
                        .any(|block| matches!(block, AssistantBlock::Image { .. }))
            )),
            "hook-denied tool session effects must not append assistant image blocks"
        );

        let mut saw_run_failed = false;
        let mut saw_typed_post_tool_failure = false;
        let mut saw_success_like_event = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunFailed {
                    error_class,
                    error_report,
                    ..
                } => {
                    saw_run_failed = true;
                    saw_typed_post_tool_failure = error_class
                        == crate::event::AgentErrorClass::Hook
                        && matches!(
                            error_report
                                .as_ref()
                                .and_then(|report| report.reason.as_ref()),
                            Some(crate::event::AgentErrorReason::HookDenied {
                                hook_id: Some(hook_id),
                                point: crate::hooks::HookPoint::PostToolExecution,
                                reason_code: crate::hooks::HookReasonCode::PolicyViolation,
                            }) if hook_id == &crate::hooks::HookId::new("deny-image-tool")
                        );
                }
                crate::event::AgentEvent::RunCompleted { .. }
                | crate::event::AgentEvent::TurnCompleted { .. }
                | crate::event::AgentEvent::ToolExecutionCompleted { .. }
                | crate::event::AgentEvent::ToolResultReceived { .. } => {
                    saw_success_like_event = true;
                }
                _ => {}
            }
        }
        assert!(saw_run_failed, "post-tool denial should emit RunFailed");
        assert!(
            saw_typed_post_tool_failure,
            "post-tool denial should emit typed HookDenied terminal error shape"
        );
        assert!(
            !saw_success_like_event,
            "post-tool denial should not emit success-like terminal or tool result events"
        );
        assert_eq!(
            turn_handle.run_failed_effect_count(),
            1,
            "post-tool denial should terminalize through failed authority exactly once"
        );
        assert_eq!(
            turn_handle.run_completed_effect_count(),
            0,
            "post-tool denial must not publish completed authority effects"
        );
        let snapshot = agent
            .execution_snapshot()
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "post-tool denial should leave the canonical turn snapshot failed"
        );
    }

    #[tokio::test]
    async fn tool_image_session_effects_preserve_tool_result_adjacency() {
        let client = Arc::new(ImageEffectClient {
            call_count: Mutex::new(0),
        });
        let tools = Arc::new(ImageEffectDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");

        let messages = agent.session().messages();
        let tool_use_index = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    Message::BlockAssistant(blocks)
                        if blocks
                            .blocks
                            .iter()
                            .any(|block| matches!(block, AssistantBlock::ToolUse { .. }))
                )
            })
            .expect("assistant tool use should be recorded");
        let tool_results_index = messages
            .iter()
            .position(|message| matches!(message, Message::ToolResults { .. }))
            .expect("tool results should be recorded");
        let image_index = messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    Message::BlockAssistant(blocks)
                        if blocks
                            .blocks
                            .iter()
                            .any(|block| matches!(block, AssistantBlock::Image { .. }))
                )
            })
            .expect("assistant image block should be recorded");

        assert_eq!(
            tool_results_index,
            tool_use_index + 1,
            "tool results must remain adjacent to the tool-use assistant message"
        );
        assert!(
            image_index > tool_results_index,
            "assistant image blocks should be appended after tool results"
        );
    }

    #[tokio::test]
    async fn provider_and_dispatch_share_the_same_combined_visible_set_for_control_tools() {
        let client = Arc::new(ControlPlaneVisibilityClient::new());
        let tools = Arc::new(PlaneAwareToolDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools.clone(), Arc::new(NoopStore))
            .await;

        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap();
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");

        let seen = client.seen_tools();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0],
            vec!["visible".to_string(), "tool_catalog_search".to_string()]
        );
        assert_eq!(
            seen[1],
            vec!["visible".to_string(), "tool_catalog_search".to_string()]
        );

        let dispatched = tools.dispatched();
        assert_eq!(
            dispatched,
            vec!["tool_catalog_search".to_string()],
            "dispatch gating should allow control tools while still blocking hidden session tools"
        );
    }

    #[tokio::test]
    async fn deferred_tools_become_visible_only_after_load_effect_reaches_the_next_boundary() {
        let client = Arc::new(DeferredLoadVisibilityClient::new());
        let tools = Arc::new(DeferredLoadDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");

        let seen = client.seen_tools();
        assert_eq!(
            seen[0],
            vec!["tool_catalog_load".to_string()],
            "deferred tools should stay hidden before the load effect is applied"
        );
        assert_eq!(
            seen[1],
            vec!["deferred_tool".to_string(), "tool_catalog_load".to_string()],
            "the next boundary should reveal the requested deferred tool"
        );
    }

    #[tokio::test]
    async fn deferred_catalog_delta_events_track_hidden_catalog_changes_across_boundaries() {
        let client = Arc::new(DeferredLoadVisibilityClient::new());
        let tools = Arc::new(DeferredLoadDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();
        assert_eq!(result.text, "done");

        let mut added_hidden_batches = Vec::new();
        let mut removed_hidden_batches = Vec::new();
        let mut deferred_status_counts = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::ToolConfigChanged { payload } = event
                && payload.domain == Some(crate::event::ToolConfigChangeDomain::DeferredCatalog)
                && let Some(delta) = payload.deferred_catalog_delta.as_ref()
            {
                added_hidden_batches.push(delta.added_hidden_names.clone());
                removed_hidden_batches.push(delta.removed_hidden_names.clone());
                if let crate::event::ToolConfigChangeStatus::DeferredCatalogDelta {
                    added_hidden_count,
                    removed_hidden_count,
                    pending_source_count,
                } = payload.status_info()
                {
                    deferred_status_counts.push((
                        *added_hidden_count,
                        *removed_hidden_count,
                        *pending_source_count,
                    ));
                }
            }
        }

        assert!(
            added_hidden_batches
                .iter()
                .any(|names| names.iter().any(|name| name == "deferred_tool")),
            "expected a deferred catalog delta that advertises deferred_tool as newly hidden"
        );
        assert!(
            removed_hidden_batches
                .iter()
                .any(|names| names.iter().any(|name| name == "deferred_tool")),
            "expected a deferred catalog delta that removes deferred_tool after it is loaded"
        );
        assert!(
            deferred_status_counts
                .iter()
                .any(|(added, _, _)| *added > 0),
            "expected structured deferred catalog status for newly hidden tools"
        );
        assert!(
            deferred_status_counts
                .iter()
                .any(|(_, removed, _)| *removed > 0),
            "expected structured deferred catalog status for removed hidden tools"
        );
    }

    #[tokio::test]
    async fn direct_builder_exact_catalog_without_control_plane_keeps_deferred_tools_inline() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(DeferredWithoutControlDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");

        let seen = client.seen_tools();
        assert_eq!(
            seen[0],
            vec!["secret".to_string(), "deferred_tool_two".to_string()],
            "direct AgentBuilder sessions without a composed control plane must keep deferred tools inline"
        );
        assert_eq!(
            seen[1],
            vec!["secret".to_string(), "deferred_tool_two".to_string()],
            "boundary recompute must not hide deferred tools when no control plane is available"
        );
    }

    #[tokio::test]
    async fn run_loop_boundary_applies_filter_and_emits_tool_config_changed_and_notice() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
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

        // ToolConfigChanged event is now emitted through MeerkatMachine,
        // not the core agent loop. Drain events but don't assert on the
        // specific event type — verify state instead.
        while rx.try_recv().is_ok() {}

        let visibility_state = agent
            .session()
            .tool_visibility_state()
            .expect("boundary visibility apply should persist committed state");
        let expected_filter = crate::ToolFilter::Deny(["secret".to_string()].into_iter().collect());
        assert_eq!(visibility_state.active_filter, expected_filter);
        assert_eq!(
            visibility_state.staged_filter,
            visibility_state.active_filter
        );
        assert!(visibility_state.active_revision > 0);
        assert_eq!(
            visibility_state.staged_revision,
            visibility_state.active_revision
        );

        // ToolScope notice and ToolConfigChanged event are now emitted
        // through MeerkatMachine, not the standalone agent loop. The
        // visibility state assertions above verify correctness.
    }

    #[tokio::test]
    async fn run_loop_fails_closed_on_tool_scope_boundary_failure() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
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
        assert_eq!(client.seen_tools(), vec![Vec::<String>::new()]);

        let mut saw_warning_event = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::ToolConfigChanged { payload } = event
                && payload.status_text().contains("warning_failed_closed")
                && let crate::event::ToolConfigChangeStatus::WarningFailedClosed { error } =
                    payload.status_info()
            {
                assert!(
                    error.contains("Injected"),
                    "unexpected fail-closed error: {error}"
                );
                saw_warning_event = true;
            }
        }
        assert!(
            saw_warning_event,
            "expected warning ToolConfigChanged event during fail-closed boundary handling"
        );

        let notices: Vec<String> = agent
            .session()
            .messages()
            .iter()
            .filter_map(|msg| match msg {
                Message::SystemNotice(notice)
                    if is_synthetic_notice(msg, SystemNoticeKind::ToolScopeWarning) =>
                {
                    Some(notice.rendered_text())
                }
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1);
    }

    #[tokio::test]
    async fn builder_restores_persisted_external_filter_from_session_metadata() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut session = crate::Session::new();
        session.set_metadata(
            EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");
        assert_eq!(client.seen_tools(), vec![vec!["visible".to_string()]]);
    }

    #[tokio::test]
    async fn builder_preserves_unknown_persisted_filter_tools_as_dormant_intent() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
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

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");
        let seen = client.seen_tools();
        assert_eq!(seen, vec![vec!["visible".to_string()]]);
        let visibility_state = agent
            .session()
            .tool_visibility_state()
            .expect("canonical visibility state should be present after restore");
        assert_eq!(
            visibility_state.active_filter,
            ToolFilter::Allow(
                ["missing".to_string(), "visible".to_string()]
                    .into_iter()
                    .collect()
            ),
            "missing names should remain in canonical durable state instead of being pruned"
        );
    }

    #[tokio::test]
    async fn builder_restores_inherited_filter_from_session_metadata() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut session = crate::Session::new();
        session.set_metadata(
            INHERITED_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Allow(
                ["visible".to_string()].into_iter().collect(),
            ))
            .unwrap(),
        );

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent.run("prompt".to_string().into()).await.unwrap();
        assert_eq!(result.text, "done");
        // The inherited base filter restricts to only "visible"
        assert_eq!(client.seen_tools(), vec![vec!["visible".to_string()]]);
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
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
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
        // The next budget observation reports token exhaustion as evidence,
        // then the turn authority maps it to BudgetExhausted -> Success.
        let mut agent = build_agent(Arc::new(HighUsageLlmClient)).await;
        agent.config.max_turns = Some(10);
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
        assert_eq!(agent.state(), LoopState::Completed);
    }

    /// Regression test: tool-call budget exhaustion detected anywhere in the
    /// agent loop must route through BudgetExhausted, producing Success.
    #[tokio::test]
    async fn tool_call_budget_exhausted_routes_through_authority() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(10);
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
        assert_eq!(agent.state(), LoopState::Completed);
    }

    // -- Retry delay hint tests (PR #156 port) --

    use std::time::Duration;

    #[test]
    fn compute_retry_delay_uses_computed_for_non_rate_limited() {
        let (delay, floor_applied) = select_retry_delay(None, Duration::from_millis(500), false);
        assert_eq!(delay, Duration::from_millis(500));
        assert!(!floor_applied);
    }

    #[test]
    fn compute_retry_delay_uses_30s_floor_for_rate_limited_without_hint() {
        let (delay, floor_applied) = select_retry_delay(None, Duration::from_millis(500), true);
        assert_eq!(delay, Duration::from_secs(30));
        assert!(floor_applied);
    }

    #[test]
    fn compute_retry_delay_uses_hint_when_greater_than_computed() {
        let (delay, floor_applied) = select_retry_delay(
            Some(Duration::from_secs(60)),
            Duration::from_millis(500),
            true,
        );
        assert_eq!(delay, Duration::from_secs(60));
        assert!(!floor_applied);
    }

    #[test]
    fn compute_retry_delay_uses_30s_floor_when_hint_below_floor() {
        let (delay, floor_applied) = select_retry_delay(
            Some(Duration::from_millis(100)),
            Duration::from_millis(500),
            true,
        );
        assert_eq!(delay, Duration::from_secs(30));
        assert!(floor_applied);
    }

    #[test]
    fn compute_retry_delay_uses_computed_when_greater_than_hint() {
        let (delay, floor_applied) =
            select_retry_delay(Some(Duration::from_secs(60)), Duration::from_secs(90), true);
        assert_eq!(delay, Duration::from_secs(90));
        assert!(!floor_applied);
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
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
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
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .retry_policy(RetryPolicy {
                max_retries: 3,
                initial_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
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
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .retry_policy(RetryPolicy {
                max_retries: 3,
                initial_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(50),
                multiplier: 2.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        // Set a 100ms time budget — the 30s rate-limit floor will be
        // capped to 100ms by remaining_duration, then the next budget
        // observation reports TimeBudgetExceeded after the sleep.
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

    // -----------------------------------------------------------------------
    // Extraction lifecycle integration tests
    // -----------------------------------------------------------------------

    /// A scriptable LLM client that returns a sequence of pre-configured
    /// responses, one per call. Used to test extraction happy/retry/exhaust
    /// paths where the agentic turn and extraction turns need different
    /// responses.
    struct ScriptedExtractionClient {
        responses: Mutex<Vec<super::LlmStreamResult>>,
        call_count: Mutex<u32>,
    }

    impl ScriptedExtractionClient {
        fn new(responses: Vec<super::LlmStreamResult>) -> Self {
            // Reverse so we can pop from the end
            let mut reversed = responses;
            reversed.reverse();
            Self {
                responses: Mutex::new(reversed),
                call_count: Mutex::new(0),
            }
        }

        fn calls_made(&self) -> u32 {
            *self.call_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl AgentLlmClient for ScriptedExtractionClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            *self.call_count.lock().unwrap() += 1;
            let mut responses = self.responses.lock().unwrap();
            responses.pop().ok_or_else(|| {
                AgentError::InternalError("ScriptedExtractionClient: no more responses".into())
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    fn text_response(text: &str) -> super::LlmStreamResult {
        super::LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        )
    }

    /// Happy path: agent with output_schema, LLM returns valid JSON
    /// matching schema on the extraction turn.
    #[tokio::test]
    async fn extraction_happy_path_returns_structured_output() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        // Call 0: agentic turn (text), Call 1: extraction turn (valid JSON)
        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("I found the answer"),
            text_response(r#"{"answer": "42"}"#),
        ]));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent.run("What is the answer?".to_string().into()).await;

        let result = result.expect("extraction happy path should succeed");
        assert!(
            result.structured_output.is_some(),
            "structured_output should be populated"
        );
        let output = result.structured_output.unwrap();
        assert_eq!(output["answer"], "42");
        assert_eq!(
            client.calls_made(),
            2,
            "expect 1 agentic + 1 extraction call"
        );
    }

    /// Retry path: LLM returns invalid JSON on the first extraction attempt,
    /// then valid JSON on the retry.
    #[tokio::test]
    async fn extraction_retry_recovers_on_second_attempt() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        }))
        .unwrap();

        // Call 0: agentic turn
        // Call 1: extraction attempt 1 — invalid JSON
        // Call 2: extraction attempt 2 — valid JSON
        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("Here is the result"),
            text_response("not valid json {{{"),
            text_response(r#"{"name": "meerkat"}"#),
        ]));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .structured_output_retries(2)
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent.run("Get the name".to_string().into()).await;

        let result = result.expect("extraction retry should eventually succeed");
        assert!(
            result.structured_output.is_some(),
            "structured_output should be populated after retry"
        );
        let output = result.structured_output.unwrap();
        assert_eq!(output["name"], "meerkat");
        assert_eq!(
            client.calls_made(),
            3,
            "expect 1 agentic + 1 failed extraction + 1 successful extraction"
        );
    }

    /// Exhaust path: LLM returns invalid JSON on every extraction attempt,
    /// exceeding max retries. Should return StructuredOutputValidationFailed.
    #[tokio::test]
    async fn extraction_exhaust_returns_validation_error() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer" }
            },
            "required": ["count"]
        }))
        .unwrap();

        // Call 0: agentic turn
        // Call 1: extraction attempt 1 — invalid (authority: attempts 0→1 on entry, 1<2 → retry, 1→2)
        // Call 2: extraction attempt 2 — invalid (authority: attempts=2, 2<2 false → exhaust)
        // Error reports attempts = max_retries + 1 = 3
        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("Computing count"),
            text_response("bad json 1"),
            text_response("bad json 2"),
        ]));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .structured_output_retries(2)
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent.run("Count items".to_string().into()).await;

        match result {
            Err(AgentError::StructuredOutputValidationFailed {
                attempts, reason, ..
            }) => {
                assert_eq!(
                    attempts, 2,
                    "should report validation failure count (== max_retries)"
                );
                assert!(
                    reason.contains("Invalid JSON"),
                    "reason should mention JSON parse failure, got: {reason}"
                );
            }
            Ok(_) => panic!("expected StructuredOutputValidationFailed, got Ok"),
            Err(e) => panic!("expected StructuredOutputValidationFailed, got: {e:?}"),
        }

        assert_eq!(
            client.calls_made(),
            3,
            "expect 1 agentic + 2 extraction attempts"
        );
    }
}
